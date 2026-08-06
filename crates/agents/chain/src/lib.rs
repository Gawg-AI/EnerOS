//! EnerOS 轻量区块链 (workspace v0.192.0)。
//!
//! 提供基于 SM3 的哈希链、区块构建、Merkle 根计算与轻量共识机制，
//! 适用于边缘侧电力交易存证与可信审计。
//!
//! # 架构
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │  eneros-chain (workspace v0.192.0)                    │
//! │  ┌──────────┐  ┌──────────┐  ┌───────────┐ │
//! │  │ types    │  │ block    │  │ hash_chain│ │
//! │  │ (Block,  │  │ (merkle, │  │(LightChain│ │
//! │  │  Tx,     │  │  hash,   │  │  Chain    │ │
//! │  │  Digest) │  │  genesis)│  │  trait)   │ │
//! │  └──────────┘  └──────────┘  └───────────┘ │
//! │  ┌──────────────────────────────────────┐   │
//! │  │ consensus (SingleNodeConsensus)      │   │
//! │  └──────────────────────────────────────┘   │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! # no_std 合规
//! `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod block;
pub mod consensus;
pub mod hash_chain;
pub mod types;

pub use block::{compute_block_hash, compute_merkle_root, create_block, create_genesis};
pub use consensus::{Consensus, SingleNodeConsensus};
pub use hash_chain::LightChain;
pub use types::{Block, Sm3Digest, TxKind, TxRecord};

/// 哈希存证链错误类型（ARC-004：链语义 = 哈希链存证 + Merkle + 交易签名，
/// 非分布式共识/分叉系统——`InvalidSignature`/`Fork` 死代码变体已删除）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChainError {
    /// 哈希校验失败。
    InvalidHash,
    /// 交易签名无效。
    BadTxSignature,
    /// 共识失败（单节点共识预留）。
    ConsensusFailed,
    /// 区块/交易未找到。
    NotFound,
    /// 空区块（无待打包交易）。
    EmptyBlock,
}

/// 链操作 trait。
pub trait Chain {
    /// 追加交易到待打包池，返回 tx_id。
    fn append(&mut self, tx: TxRecord) -> Result<u64, ChainError>;
    /// 打包待处理交易为新区块，返回区块高度。
    fn mine_block(&mut self, timestamp: u64, proposer: u64) -> Result<u64, ChainError>;
    /// 验证链完整性。
    fn verify(&self) -> Result<bool, ChainError>;
    /// 按索引获取区块。
    fn get_block(&self, index: u64) -> Result<&Block, ChainError>;
    /// 当前链高度（区块数量）。
    fn height(&self) -> u64;
}
