//! EnerOS v0.105.0 IEC 61850 信息模型（P2-G 第 1 版：IEC 61850 协议栈起点）.
//!
//! 实现 IEC 61850 LD/LN/DO/DA 四层信息模型与 SCL（Substation Configuration
//! Language）文件解析，建立电力设备数据模型建模能力，为 v0.106.0 MMS、
//! v0.107.0 GOOSE、v0.108.0 SV + IEC 62351 提供统一语义模型底座。
//!
//! # 核心类型
//!
//! - [`ld::LogicalDevice`] — 逻辑设备（ld_name/ref_name/lns）
//! - [`ln::LogicalNode`] — 逻辑节点（ln_class/ln_inst/ln_prefix/do_list）
//! - [`ln::LnClass`] — LN 类别（XCBR/MMXU/PTRC/CSWI/GGIO/Other）
//! - [`do_da::DataObject`] — 数据对象（do_name/da_list/cdc）
//! - [`do_da::CommonDataClass`] — 公共数据类（SPS/DPS/MV/ENS/ACT/ASG/Other）
//! - [`do_da::DataAttribute`] — 数据属性（da_name/fc/value/quality/timestamp）
//! - [`do_da::FunctionalConstraint`] — 功能约束（ST/MX/CO/SP/SG/SE/BR/OR）
//! - [`do_da::DaValue`] — DA 值（Bool/Int32/Float32/Float64/Enum/StringVal/Timestamp）
//! - [`do_da::Quality`] / [`do_da::Validity`] / [`do_da::Source`] — 品质标志
//! - [`scl_parser::SclParser`] — SCL 解析器 + 路径索引（[`Iec61850Model`] 实现）
//! - [`ModelError`] — 模型错误（SclParseError/NotFound/TypeMismatch）
//!
//! # 偏差声明（D1~D13）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/iec61850_model/` → `crates/protocols/iec61850-model/`（eneros-iec61850-model） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；IEC 61850 属设备协议栈，与 modbus/iec104 同 protocols 子系统 |
//! | **D2** | 蓝图 `docs/phase2/iec61850_model.md` → `docs/protocols/iec61850-model-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
//! | **D3** | 蓝图 `tests/iec61850_model.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.104.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | 蓝图 roxmltree + SlotMap + HashMap → 零第三方依赖：内置迷你 XML DOM 解析器（私有 `xml` 模块，仅 SCL 子集：元素/属性/文本/嵌套/自闭合/声明/注释/CDATA/实体转义）+ `Vec` 存储 + `alloc::collections::BTreeMap` 路径索引 | 全项目 no_std + aarch64-unknown-none 交叉编译（记忆 §4.3）；cargo deny 离线 SBOM 零新增依赖先例；v0.104.0 D4 内置 PRNG 替代 rand 先例；SlotMap/HashMap 非 no_std/需 RandomState |
//! | **D5** | 蓝图 `String`/`Vec`/`format!`/`HashMap`（std）→ `alloc::string::String`/`alloc::vec::Vec`/`alloc::format!`/BTreeMap | 蓝图 §43.1 + 记忆 §4.3：全项目 Rust 代码必须 no_std |
//! | **D6** | 蓝图 `Iec61850Model: Send + Sync` → 去除 bound | 与 v0.64.0 `Solver` / v0.104.0 `ParetoSolver` 惯例一致；单线程数据模型无跨线程需求 |
//! | **D7** | 蓝图 bug 修复①：`path_index` 值三元组 `(LnId, do_idx, da_idx)` 缺 LN 索引（DA 无法回溯）→ 四元组 `(ld, ln, do_, da)`；路径语法统一为 `LD/LN.DO.DA`（蓝图 §4.2 注释 "LD/LN.DO.DA" 与 §4.5 `format!("{}/{}/{}.{}")` = "LD/LN/DO.DA" 自相矛盾） | IEC 61850 引用惯例即 `LD/LN.DO.DA`；不修复则 get_da/set_da 无法实现（Karpathy：surface inconsistencies） |
//! | **D8** | 蓝图 bug 修复②：`parse_fc` 缺 `"OR"` 分支（落入 `_ => ST`）→ 补齐 8 分支；未知 FC 静默映射 ST → 返回 `SclParseError` | 静默映射丢失语义且与蓝图 §9"解析容错"的容错语义（可诊断）冲突；故障注入测试（§6.5）要求错误可报告 |
//! | **D9** | 蓝图 bug 修复③：`parse_da_value` 的 stVal 分支 `"true"` 被 `unwrap_or(Bool(val=="1"))` 误判为 `Bool(false)` → 明确规则：true/false→Bool、可解析整数→Int32、其余→StringVal | 蓝图代码逻辑错误，直接运行将产生错误模型值（Karpathy：不带着疑问照抄） |
//! | **D10** | 蓝图仅过滤 `"LN"` 元素 → 同时解析 `"LN0"`（真实 SCL 每个 LDevice 必含 LLN0；ln_class=`Other("LLN0")`，inst=0，ln_ref 不带实例号后缀即 "LLN0"） | IEC 61850-6：LLN0 为每 LD 强制节点；蓝图遗漏导致真实 SCL 模型不完整 |
//! | **D11** | 重复 LD 名 → `SclParseError`（蓝图未定义重复行为） | IEC 61850 LD 名系统内唯一；静默接受将致路径索引覆盖、查询结果不确定（确定性优先） |
//! | **D12** | `ModelError::NotFound` / `TypeMismatch` 裸变体 → `NotFound(String)` 携带路径 / `TypeMismatch(String)` 携带期望与实际 variant；`set_da` 类型检查定义为 DaValue variant 判别式一致（quality/timestamp 不参与） | 蓝图 §4.4 仅给变体名未定义载荷；诊断信息为运维必需（蓝图 §9 可观测） |
//! | **D13** | 蓝图 §6.4"点表映射一致性"回归 → 零依赖交付：设计文档给出 `Quality.validity`↔UPA `PointQuality`、`DaValue`↔`PointValue` 语义映射表 + 语义对齐测试；性能 1000 查找 < 1ms 落地为 cfg(test) `Instant` 断言 | UPA 协议适配属 v0.51.0 适配层职责（upa-model D6 零耦合先例）；本 crate 不引入 eneros 依赖（v0.104.0 D12 测试计时先例） |
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，**零第三方依赖**、零 unsafe，
//! 不调用 `panic!` / `todo!` / `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod do_da;
pub mod ld;
pub mod ln;
pub mod scl_parser;
mod xml;

use alloc::string::String;
use alloc::vec::Vec;

pub use do_da::{
    CommonDataClass, DaValue, DataAttribute, DataObject, FunctionalConstraint, Quality, Source,
    Validity,
};
pub use ld::LogicalDevice;
pub use ln::{LnClass, LogicalNode};
pub use scl_parser::SclParser;

/// IEC 61850 信息模型错误。
#[derive(Debug, Clone, PartialEq)]
pub enum ModelError {
    /// SCL XML 解析失败（消息携带 line:column 位置）。
    SclParseError(String),
    /// 路径不存在（消息携带查询路径）。
    NotFound(String),
    /// 值类型不匹配（消息携带路径与期望/实际类型）。
    TypeMismatch(String),
}

/// IEC 61850 信息模型接口（D5：无 Send + Sync bound）。
pub trait Iec61850Model {
    /// 解析 SCL XML 并追加构建信息模型（可多次调用，D12 重复 LD 报错）。
    fn load_scl(&mut self, scl_xml: &str) -> Result<(), ModelError>;
    /// 按 LD 名查询逻辑设备。
    fn get_ld(&self, ld_name: &str) -> Option<&LogicalDevice>;
    /// 按 LD 名 + LN 引用名（如 "XCBR1"/"LLN0"）查询逻辑节点。
    fn get_ln(&self, ld_name: &str, ln_ref: &str) -> Option<&LogicalNode>;
    /// 按路径查询 DA（格式 "LD/LN.DO.DA"，D9）。
    fn get_da(&self, path: &str) -> Option<&DataAttribute>;
    /// 按路径写入 DA 值（路径缺失 → `NotFound`；值判别式不一致 → `TypeMismatch`）。
    fn set_da(&mut self, path: &str, value: DaValue) -> Result<(), ModelError>;
    /// 列出全部 LD 名。
    fn list_lds(&self) -> Vec<&str>;
}
