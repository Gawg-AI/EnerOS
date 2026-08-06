//! EnerOS v0.117.0 审计日志与哈希链（P2-I 安全体系第 5 版）.
//!
//! 等保 2.0 三级「安全审计」要求：对登录、配置变更、控制命令、密钥使用等
//! 关键安全事件进行完整记录，并保证审计记录**不可篡改、可验证、可追溯**。
//! 本 crate 以 SM3 哈希链实现审计日志的密码学存证：每条审计事件经定长编码
//! 后计算事件哈希，再与前一条链上哈希串联计算组合哈希（`SM3(prev ‖
//! event)`），形成单向递进的哈希链；任一历史条目被篡改或删除都会破坏后续
//! 链接关系，可被 [`ChainVerifier`] 检出。链状态可经 [`AuditLoggerImpl::recover`]
//! 从存储中恢复，支持断电重启后续链。
//!
//! # 国标引用
//!
//! - GM/T 0004-2012（GB/T 32905-2016）SM3 密码杂凑算法（事件哈希与链哈希）
//!
//! # 核心类型
//!
//! - [`AuditEventType`] / [`AuditEvent`] — 审计事件：11 类事件 + 定长字段
//!   （actor/target/detail 用 `heapless::String` 定容存储）
//! - [`ChainEntry`] / [`HashChain`] — 链上条目与链头状态（含创世哈希）
//! - [`AuditLogger`] / [`AuditLoggerImpl`] — 日志器：序号检查、事件编码、
//!   哈希串联、落盘追加、链状态推进与崩溃恢复
//! - [`ChainVerifier`] / [`ChainVerifierImpl`] — 链校验：逐条重算组合哈希，
//!   定位断链序号并生成 [`VerifyReport`]
//! - [`AuditStore`] / [`MemStore`] — 存储抽象与内存实现（易失，供测试与
//!   无盘场景）
//!
//! # 偏差声明（相对蓝图）
//!
//! 1. **D1 crate 路径**：蓝图声明 `crates/audit/`，按记忆文件 §2.3.1 子系统
//!    分组规则归入 `crates/security/audit/`（security 子系统）。
//! 2. **D2 FFI 移除**：蓝图原拟 SM3 C FFI 调用，已决策移除，改复用
//!    `eneros_crypto::sm3_hash` 纯 Rust 实现，零 unsafe 零 `extern "C"`。
//! 3. **D3 事件哈希仅对已编码前缀计算**：蓝图为 512 字节定长缓冲整体哈希，
//!    本实现仅对实际写入的前缀 `buf[..pos]` 做 SM3，避免尾部零字节参与
//!    哈希导致语义含混。
//! 4. **D4 新增 [`MemStore`]**：蓝图仅声明存储 trait，本实现附带内存存储
//!    （`Vec`  backed），供单元测试与易失性场景使用。
//! 5. **D5 文档归属**：设计文档归入 `docs/security/`（记忆文件 §2.3.3 新增
//!    方向），非本 crate 交付范围。
//!
//! # v0.118.0 行为审计扩展（P2-I 安全体系第 6 版）
//!
//! 在审计日志与哈希链（事后追溯）之上叠加「事中检测」能力：以滑动窗口
//! 统计 + 基线偏离建立 Agent 行为基线模型，检测频率异常、越权访问与
//! 非工作时段操作，分级告警并供联动响应扩展，满足等保 2.0「入侵防范」
//! 要求。
//!
//! - [`baseline`] — 行为维度 [`BehaviorDim`]、统计基线 [`StatBaseline`]、
//!   Agent 行为基线 [`BehaviorBaseline`]、异常类型 [`AnomalyType`]、
//!   告警级别 [`AlertLevel`]、告警 [`Alert`] 与基线训练器
//!   [`BaselineTrainer`] / [`BaselineTrainerImpl`]（按分钟分桶速率统计）
//! - [`alert`] — 告警处理器抽象 [`AlertHandler`] 与定容告警队列
//!   [`AlertQueue`]（满时显式返回 [`AnomalyError::AlertQueueFull`]，
//!   不静默丢弃）
//! - [`anomaly`] — 滑动窗口异常检测：阈值配置 [`ThresholdConfig`]、
//!   检测器抽象 [`AnomalyDetector`] 与统计检测器 [`StatAnomalyDetector`]
//!   （频率偏离分级告警 + 非工作时段 + 越权访问最小规则）
//! - [`AnomalyError`] — 异常检测全流程错误模型
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `core::*` / `alloc::*` / `heapless::*`，依赖仅 eneros-crypto
//! （workspace 内 path 依赖）与 heapless，零 unsafe，不调用 `panic!` /
//! `todo!` / `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod alert;
pub mod anomaly;
pub mod baseline;
pub mod chain;
pub mod events;
pub mod store;

pub use alert::{AlertHandler, AlertQueue};
pub use anomaly::{AnomalyDetector, StatAnomalyDetector, ThresholdConfig};
pub use baseline::{
    Alert, AlertLevel, AnomalyType, BaselineTrainer, BaselineTrainerImpl, BehaviorBaseline,
    BehaviorDim, StatBaseline,
};
pub use chain::{AuditLogger, AuditLoggerImpl, ChainVerifier, ChainVerifierImpl};
pub use events::{AuditEvent, AuditEventType, ChainEntry, HashChain};
pub use store::{AuditStore, MemStore};

/// 审计错误模型（哈希链全流程错误；`SeqOutOfOrder`/`ChainBroken` 携带序号定位）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditError {
    /// 哈希计算失败（底层 SM3 运算异常）.
    HashComputeFailed,
    /// 事件序号乱序：`expect` 为期望序号，`got` 为实际提交序号.
    SeqOutOfOrder {
        /// 期望的下一序号（`last_seq + 1`）.
        expect: u64,
        /// 实际提交的事件序号.
        got: u64,
    },
    /// 哈希链断裂：`at_seq` 为断点处条目序号.
    ChainBroken {
        /// 检测到断链的条目序号.
        at_seq: u64,
    },
    /// 存储写入失败（追加链上条目时）.
    StorageWriteFailed,
    /// 存储读取失败（范围/尾部读取时）.
    StorageReadFailed,
    /// 缓冲区已满（定容结构溢出）.
    BufferFull,
    /// 创世哈希非法（如全零占位）.
    InvalidGenesis,
}

/// 链校验报告（`valid` = 全链完整；`broken_at` 定位首个断点序号）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// 被校验的条目总数.
    pub total: u64,
    /// 首个断链点序号；`None` 表示无断点.
    pub broken_at: Option<u64>,
    /// 全链是否完整（`broken_at.is_none()`）.
    pub valid: bool,
}

/// 异常检测错误模型（v0.118.0 行为审计全流程错误）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnomalyError {
    /// 基线未训练/未加载，无法执行检测.
    BaselineNotTrained,
    /// 训练样本不足：`min` 为要求的最少样本数，`got` 为实际提供数.
    InsufficientSamples {
        /// 要求的最少样本数.
        min: u32,
        /// 实际提供的样本数.
        got: u32,
    },
    /// 行为维度越界（查询的维度不在基线表内）.
    DimOutOfRange,
    /// 告警队列已满（定容队列溢出，不静默丢弃）.
    AlertQueueFull,
    /// 基线已过期（超出训练窗口有效期，需重训）.
    BaselineExpired,
}
