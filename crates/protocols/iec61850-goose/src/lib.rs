//! EnerOS v0.107.0 IEC 61850 GOOSE 快速事件传输协议栈（P2-G 第 3 版）.
//!
//! 在 v0.105.0 信息模型（DaValue）与 v0.106.0 MMS BER 编解码基座上，
//! 实现 GOOSE 二层组播直发（EtherType 0x88B8）+ BER PDU 编解码 + st_num/sq_num 重传状态机，
//! 打通联邦保护协同的事件通道，端到端 MockL2 全链路 < 4ms。
//!
//! 为 v0.108.0 SV + IEC 62351 安全加固奠基。
//!
//! # 核心类型
//!
//! - [`dataset::GooseDataset`] / [`dataset::GooseEntry`] — GOOSE 数据集（path + DaValue）
//! - [`goose_tx::GooseControlBlock`] — 控制块（9 字段全 pub）
//! - [`goose_tx::GoosePublisher`] — GOOSE 发布者（泛型 L2 传输，update_value / publish / retransmit_if_needed）
//! - [`goose_rx::GoosePdu`] — 解码后的 GOOSE PDU
//! - [`goose_rx::RxStatus`] — 接收状态（New / Duplicate / StJump，D12）
//! - [`goose_rx::GooseSubscriber`] — GOOSE 订阅者（poll / set_callback / MAC+APPID 过滤）
//! - [`GooseError`] — 错误枚举（TransportError / BerEncodeError / BerDecodeError / InvalidConfig，D10）
//! - [`L2Transport`] — 二层传输抽象 trait（send / recv，D4）
//! - [`MockL2`] — 脚本化 mock 传输（帧队列 + 发送记录 + 错误注入 + loopback，D4/D11）
//!
//! # 偏差声明（D1~D12，相对蓝图 §3/§4/§6）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/iec61850_goose/` → `crates/protocols/iec61850-goose/`（eneros-iec61850-goose） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；与 mms/iec61850-model 同 protocols 子系统 |
//! | **D2** | 蓝图 `docs/phase2/goose.md` → `docs/protocols/iec61850-goose-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
//! | **D3** | 蓝图 `tests/goose_latency.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.106.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 删除蓝图 §4.5 `extern "C"` raw socket FFI + unsafe；新增 `L2Transport` trait（send/recv）+ `MockL2`（置于 lib.rs）；真实 raw socket 接线在集成层 | aarch64-unknown-none 无 libc 可链接 extern "C"；主机不可测；项目零 unsafe/零 C FFI 惯例；与 v0.106.0 D4 MmsTransport 同先例 |
//! | **D5** | `GoosePublisher<T: L2Transport>` / `GooseSubscriber<T: L2Transport>` 泛型化，transport 由 `new` 注入（蓝图内部建 socket 写死 "eth0"） | 可测试性 + 网卡选择属集成层决策（Karpathy Simplicity First） |
//! | **D6** | 时间注入：`publish(now: u64)` / `retransmit_if_needed(now)` 使用外部时间参数；蓝图 `current_time_ms()` 未定义 | no_std 无系统时间（v0.64.0 D1 时间注入先例）；重传间隔判定需要时钟源 |
//! | **D7** | 蓝图 bug 修复①：allData 0xAB 只有 tag 无长度字段（条目直接尾随）→ 补「tag + 长度 + 内容」完整 TLV | BER TLV 合规（X.690）；无长度则接收端无法确定条目边界 |
//! | **D8** | 蓝图 bug 修复②：allData 数据 tag（Bool 0x01 / Int32 0x03 / Float64 0x85）与 v0.106.0 MMS 解码约定冲突 → 统一 boolean 0x80 / integer 0x85 / floating-point 0x87（4B→Float32、8B→Float64） | IEC 61850-8-1 数据 tag 与 MMS 一致；栈内编解码对称，rx 可复用 v0.106.0 解码规则 |
//! | **D9** | `rx_callback: Box<dyn Fn + Send + Sync>` → 去 Send+Sync bound | 蓝图 §43.1 no_std 全项目去 bound 惯例（v0.64.0/v0.105.0/v0.106.0 一致） |
//! | **D10** | 错误模型统一：`GooseError` = TransportError / BerEncodeError / BerDecodeError / InvalidConfig（4 变体）；蓝图 SocketCreateFailed/SendFailed 随 FFI 删除合并为 TransportError | FFI 删除后原错误无来源；4 变体覆盖组帧/解码/传输/配置全部失败面（对齐 v0.106.0 D10 精简风格） |
//! | **D11** | 性能 < 4ms 落地为 cfg(test) Instant 断言（MockL2 回路，编码+传输+解码全链路口径，文档声明）；§6.2 真实网卡端到端为实验室硬件项，以 MockL2 脚本化帧替代 | 无真实网卡硬件（与 v0.106.0 D12 同口径） |
//! | **D12** | 接收侧 st_num 跳变检测以 `RxStatus`（New/Duplicate/StJump）随 PDU 返回；蓝图 §4.4 要求检测跳变但 §4.2 `poll -> Option<GoosePdu>` 无承载 → `poll -> Option<(GoosePdu, RxStatus)>` | 蓝图自相矛盾（要求检测但接口无处上报）；接收方必须能区分新事件/重传/丢帧 |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，第三方依赖仅 `eneros-iec61850-model`（path），
//! 零 unsafe，不调用 `panic!` / `todo!` / `unimplemented!`，
//! 可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod dataset;
pub mod goose_rx;
pub mod goose_tx;

pub use dataset::{GooseDataset, GooseEntry};
pub use goose_rx::{GoosePdu, GooseSubscriber, RxStatus};
pub use goose_tx::{GooseControlBlock, GoosePublisher};

/// GOOSE 协议错误（D10：统一错误模型，4 变体覆盖组帧/解码/传输/配置）。
#[derive(Debug, Clone, PartialEq)]
pub enum GooseError {
    /// 传输层错误（发送/接收失败）。
    TransportError,
    /// BER 编码失败（组帧时长度溢出或内部不一致）。
    BerEncodeError,
    /// BER 解码失败（报文畸形/截断/未知长度格式）。
    BerDecodeError,
    /// 配置无效（如 app_id == 0）。
    InvalidConfig,
}

/// 二层传输抽象（D4：v0.27.0 网卡真实接线在集成层）。
pub trait L2Transport {
    /// 发送一帧二层报文。
    fn send(&mut self, frame: &[u8]) -> Result<(), GooseError>;
    /// 接收一帧二层报文到 `buf`，返回实际字节数。
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, GooseError>;
}

/// 脚本化 Mock 二层传输（测试/集成占位，D4/D11）。
///
/// - `send`：记录已发帧（供时序/内容断言）；loopback 模式时自动压入 rx 队列
/// - `recv`：依次弹出预置帧；无帧 → 返回 0（空接收）
/// - 可注入一次性 send/recv 错误
/// - 可启用 loopback：publish 的帧立即可被 poll 收到
#[derive(Debug, Clone, PartialEq)]
pub struct MockL2 {
    rx_frames: alloc::collections::VecDeque<alloc::vec::Vec<u8>>,
    tx_frames: alloc::vec::Vec<alloc::vec::Vec<u8>>,
    inject_send_error: Option<GooseError>,
    inject_recv_error: Option<GooseError>,
    loopback: bool,
}

impl MockL2 {
    /// 创建空 mock（默认 loopback 关闭）。
    pub fn new() -> Self {
        Self {
            rx_frames: alloc::collections::VecDeque::new(),
            tx_frames: alloc::vec::Vec::new(),
            inject_send_error: None,
            inject_recv_error: None,
            loopback: false,
        }
    }

    /// 预置一段 recv 帧（按弹出顺序消费）。
    pub fn push_rx_frame(&mut self, bytes: &[u8]) {
        self.rx_frames.push_back(alloc::vec::Vec::from(bytes));
    }

    /// 已发送帧记录（供内容/时序断言）。
    pub fn tx_frames(&self) -> &[alloc::vec::Vec<u8>] {
        &self.tx_frames
    }

    /// 清空已发送记录。
    pub fn clear_tx(&mut self) {
        self.tx_frames.clear();
    }

    /// 注入一次性 send 错误。
    pub fn inject_send_error(&mut self, e: GooseError) {
        self.inject_send_error = Some(e);
    }

    /// 注入一次性 recv 错误。
    pub fn inject_recv_error(&mut self, e: GooseError) {
        self.inject_recv_error = Some(e);
    }

    /// 启用/禁用 loopback：send 的帧自动进入 rx 队列尾部。
    pub fn set_loopback(&mut self, enabled: bool) {
        self.loopback = enabled;
    }

    /// 查询 loopback 状态。
    pub fn loopback_enabled(&self) -> bool {
        self.loopback
    }
}

impl Default for MockL2 {
    fn default() -> Self {
        Self::new()
    }
}

impl L2Transport for MockL2 {
    fn send(&mut self, frame: &[u8]) -> Result<(), GooseError> {
        if let Some(e) = self.inject_send_error.take() {
            return Err(e);
        }
        let cloned = alloc::vec::Vec::from(frame);
        if self.loopback {
            self.rx_frames.push_back(cloned.clone());
        }
        self.tx_frames.push(cloned);
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, GooseError> {
        if let Some(e) = self.inject_recv_error.take() {
            return Err(e);
        }
        let Some(frame) = self.rx_frames.pop_front() else {
            return Ok(0);
        };
        let n = frame.len().min(buf.len());
        buf[..n].copy_from_slice(&frame[..n]);
        Ok(n)
    }
}
