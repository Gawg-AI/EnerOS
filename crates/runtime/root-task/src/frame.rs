//! EL0 服务 IPC 帧（KRN-002）— 由共享 `eneros-ipc-wire::el0` 提供。
//!
//! root-task 侧保持 `frame::*` 路径（main.rs 引用不变），实现已上移
//! `crates/abi/ipc-wire/src/el0.rs`（KRN-003：services crate 同样复用）。

pub use eneros_ipc_wire::el0::*;
