//! EnerOS IEC 60870-5 ASDU 应用层公共库（蓝图 48 WP-48.0）.
//!
//! 自 `eneros-iec104-slave/src/asdu.rs` **原样抽取**（行为零变更：diff 只允许
//! 移动+重导出+错误类型机械改名 `Iec104Error`→[`error::AsduError`]）。
//! 104 master/slave 与后续 101（WP-48.2）/103（WP-48.3）共同依赖本 crate 的
//! ASDU 层；各协议链路层（104 TCP APCI / 101·103 串口 FT1.2）留在各自 crate。
//!
//! # 核心类型（与抽取前逐一同义）
//! - [`asdu::Asdu`] — ASDU 应用服务数据单元
//! - [`asdu::TypeId`] — 类型标识（10 变体）
//! - [`asdu::Cot`] — 传送原因（9 变体）
//! - [`asdu::InformationObject`] — 信息对象（IOA + 值 + 品质 + 可选时标）
//! - [`asdu::IoValue`] — 信息对象值
//! - [`asdu::QualityDescriptor`] — 品质描述符
//! - [`asdu::TimeTag`] — CP56Time2a 7 字节时标
//! - [`asdu::Sco`] / [`asdu::Dco`] — 单点/双点遥控命令限定符
//! - [`error::AsduError`] — ASDU 编解码错误
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`，**零外部依赖**。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod asdu;
pub mod error;

pub use asdu::{
    Asdu, Cot, Dco, DoublePointValue, InformationObject, IoValue, QualityDescriptor, Sco,
    SinglePointValue, TimeTag, TypeId,
};
pub use error::AsduError;
