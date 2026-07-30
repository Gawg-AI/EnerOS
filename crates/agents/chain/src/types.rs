//! 区块链核心类型定义。

use alloc::vec::Vec;

/// SM3 摘要（32 字节）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sm3Digest(pub [u8; 32]);

impl Sm3Digest {
    /// 全零摘要。
    pub const ZERO: Self = Self([0u8; 32]);
}

/// 交易类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TxKind {
    /// 电力交易。
    Deal,
    /// 结算。
    Settlement,
    /// 信誉记录。
    Reputation,
}

/// 交易记录。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TxRecord {
    /// 交易 ID。
    pub tx_id: u64,
    /// 交易类型。
    pub kind: TxKind,
    /// 负载数据。
    pub payload: Vec<u8>,
    /// 时间戳。
    pub at: u64,
}

/// 区块。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Block {
    /// 区块高度（从 0 开始）。
    pub index: u64,
    /// 前一区块哈希。
    pub prev_hash: Sm3Digest,
    /// 交易列表。
    pub txs: Vec<TxRecord>,
    /// Merkle 根。
    pub merkle_root: Sm3Digest,
    /// 时间戳。
    pub timestamp: u64,
    /// 提议者节点 ID。
    pub proposer: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sm3_digest_zero() {
        let d = Sm3Digest::ZERO;
        assert_eq!(d.0, [0u8; 32]);
    }

    #[test]
    fn sm3_digest_clone_eq() {
        let a = Sm3Digest([1u8; 32]);
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn tx_record_debug() {
        let tx = TxRecord {
            tx_id: 1,
            kind: TxKind::Deal,
            payload: alloc::vec![0xAA, 0xBB],
            at: 1000,
        };
        assert_eq!(tx.tx_id, 1);
        assert_eq!(tx.kind, TxKind::Deal);
        assert_eq!(tx.payload.len(), 2);
    }

    #[test]
    fn block_fields() {
        let blk = Block {
            index: 0,
            prev_hash: Sm3Digest::ZERO,
            txs: Vec::new(),
            merkle_root: Sm3Digest::ZERO,
            timestamp: 100,
            proposer: 1,
        };
        assert_eq!(blk.index, 0);
        assert_eq!(blk.proposer, 1);
    }
}
