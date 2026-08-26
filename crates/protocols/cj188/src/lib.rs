//! EnerOS CJ/T 188-2018 户用计量仪表通信协议——master 插件（F-3 登记）.
//!
//! # 本 crate 交付边界（诚实登记，对齐 dlt645 SpecBlocked 先例）
//!
//! owner 规约原文（CJ/T 188）未在环境内提供——依「AI 不得凭记忆编造帧格式」
//! 纪律，本 crate 交付骨架：`ProtocolPlugin` 六方法 seam / 配置 typed 消费
//! （未知键拒绝）/ Serial 传输接线 / descriptor `interop_level = SpecBlocked`；
//! `read_point`/`write_point` 返回 `Err(Unsupported)` 不伪造读数。
//! **BLOCKED（owner 输入：规约文本）**：帧编解码字节层、附录示例帧 KAT、负例组。
//! 点表线码变体（ProtocolAddress::Cj188）挂载属 spec 明确 OUT，随后续卷。
extern crate alloc;

pub mod config;
pub mod plugin;

pub use config::Cj188Config;
pub use plugin::Cj188Plugin;
