//! ASDU 应用层类型（蓝图 48 WP-48.0 抽取后本模块 = 重导出 shim）.
//!
//! 全部类型与编解码实现移至 [`eneros_iec60870_asdu`]（101/103/104 共用）；
//! 本 crate 经此模块保持 `crate::asdu::*` 与 `pub use asdu::{..}` 路径
//! 不变（行为零变更）。`Asdu::decode` 错误类型为 `AsduError`——经
//! [`crate::error::Iec104Error`] 的 `From` 实现在 `?` 处无损转换。

pub use eneros_iec60870_asdu::asdu::*;
pub use eneros_iec60870_asdu::error::AsduError;
