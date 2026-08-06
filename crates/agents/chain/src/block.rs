//! 区块构建与哈希计算。

use alloc::vec::Vec;

use crate::types::{Block, Sm3Digest, TxRecord};

/// 计算 Merkle 根。
///
/// 简化实现：将所有交易的 tx_id + payload 拼接后做 SM3 哈希。
/// 若交易列表为空，则对 `[0u8; 32]` 做哈希。
pub fn compute_merkle_root(txs: &[TxRecord]) -> Sm3Digest {
    if txs.is_empty() {
        return Sm3Digest(eneros_crypto::sm3::hash(&[0u8; 32]));
    }
    let mut buf: Vec<u8> = Vec::new();
    for tx in txs {
        buf.extend_from_slice(&tx.tx_id.to_le_bytes());
        buf.extend_from_slice(&tx.payload);
    }
    Sm3Digest(eneros_crypto::sm3::hash(&buf))
}

/// 计算区块哈希：hash(index || prev_hash || merkle_root || timestamp)。
pub fn compute_block_hash(block: &Block) -> Sm3Digest {
    let mut buf: Vec<u8> = Vec::with_capacity(8 + 32 + 32 + 8);
    buf.extend_from_slice(&block.index.to_le_bytes());
    buf.extend_from_slice(&block.prev_hash.0);
    buf.extend_from_slice(&block.merkle_root.0);
    buf.extend_from_slice(&block.timestamp.to_le_bytes());
    Sm3Digest(eneros_crypto::sm3::hash(&buf))
}

/// 创建创世区块（index = 0, prev_hash 全零, 无交易）。
pub fn create_genesis() -> Block {
    let merkle_root = compute_merkle_root(&[]);
    Block {
        index: 0,
        prev_hash: Sm3Digest::ZERO,
        txs: Vec::new(),
        merkle_root,
        timestamp: 0,
        proposer: 0,
    }
}

/// 创建新区块，自动计算 merkle_root。
pub fn create_block(
    index: u64,
    prev_hash: Sm3Digest,
    txs: Vec<TxRecord>,
    timestamp: u64,
    proposer: u64,
) -> Block {
    let merkle_root = compute_merkle_root(&txs);
    Block {
        index,
        prev_hash,
        txs,
        merkle_root,
        timestamp,
        proposer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TxKind;

    #[test]
    fn genesis_block() {
        let g = create_genesis();
        assert_eq!(g.index, 0);
        assert_eq!(g.prev_hash, Sm3Digest::ZERO);
        assert!(g.txs.is_empty());
        assert_eq!(g.timestamp, 0);
        assert_eq!(g.proposer, 0);
    }

    #[test]
    fn block_hash_determinism() {
        let g = create_genesis();
        let h1 = compute_block_hash(&g);
        let h2 = compute_block_hash(&g);
        assert_eq!(h1, h2);
    }

    #[test]
    fn merkle_root_changes_with_different_txs() {
        let tx1 = TxRecord {
            tx_id: 1,
            kind: TxKind::Deal,
            payload: alloc::vec![0x01],
            at: 100,
            author: 0,
            signature: [0u8; 64],
        };
        let tx2 = TxRecord {
            tx_id: 2,
            kind: TxKind::Settlement,
            payload: alloc::vec![0x02],
            at: 200,
            author: 0,
            signature: [0u8; 64],
        };
        let r1 = compute_merkle_root(core::slice::from_ref(&tx1));
        let r2 = compute_merkle_root(&[tx2]);
        assert_ne!(r1, r2);
    }

    #[test]
    fn merkle_root_empty() {
        let r = compute_merkle_root(&[]);
        let expected = Sm3Digest(eneros_crypto::sm3::hash(&[0u8; 32]));
        assert_eq!(r, expected);
    }

    #[test]
    fn create_block_computes_merkle() {
        let tx = TxRecord {
            tx_id: 10,
            kind: TxKind::Reputation,
            payload: alloc::vec![0xFF],
            at: 500,
            author: 0,
            signature: [0u8; 64],
        };
        let blk = create_block(1, Sm3Digest::ZERO, alloc::vec![tx.clone()], 500, 1);
        assert_eq!(blk.merkle_root, compute_merkle_root(&[tx]));
        assert_eq!(blk.index, 1);
    }
}
