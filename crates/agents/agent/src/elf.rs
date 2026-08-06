//! ELF64 PIE loader — 由共享 crate `eneros-elf-loader` 提供（KRN-002）。
//!
//! agent 侧保持 `agent::elf::*` 路径兼容（过程/descriptor 引用不变），
//! 实现已上移 `crates/abi/elf-loader`（no_std，root-task EL0 服务装载共用）。

pub use eneros_elf_loader::*;
