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

/// 交易记录（ARC-004：哈希存证链交易，带 SM2 签名）。
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
    /// 交易作者（节点 ID）。
    pub author: u64,
    /// SM2 签名（r‖s 64 字节，对 tx_digest 签名）。
    pub signature: [u8; 64],
}

impl TxRecord {
    /// 交易摘要（签名消息）：SM3(tx_id ‖ kind ‖ payload ‖ at ‖ author)。
    pub fn digest(&self) -> [u8; 32] {
        let mut buf = alloc::vec::Vec::with_capacity(8 + 1 + self.payload.len() + 8 + 8);
        buf.extend_from_slice(&self.tx_id.to_le_bytes());
        buf.push(self.kind as u8);
        buf.extend_from_slice(&self.payload);
        buf.extend_from_slice(&self.at.to_le_bytes());
        buf.extend_from_slice(&self.author.to_le_bytes());
        eneros_crypto::sm3::hash(&buf)
    }

    /// SM2 验签（ARC-004：存证链交易签名校验；签名无效 → false）。
    pub fn verify_signature(&self, pubkey: &eneros_crypto::Sm2PublicKey) -> bool {
        let digest = self.digest();
        let sig = eneros_crypto::Sm2Signature::from_bytes(&self.signature);
        eneros_crypto::sm2_verify(&digest, &sig, pubkey).unwrap_or(false)
    }
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
            author: 0,
            signature: [0u8; 64],
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

#[cfg(test)]
mod arc004_tests {
    use super::*;
    use crate::Chain;
    use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair};

    fn signed_tx(id: u64, payload: &[u8], kp: &Sm2KeyPair, rng: &mut CsRng) -> TxRecord {
        let mut tx = TxRecord {
            tx_id: id,
            kind: TxKind::Deal,
            payload: payload.to_vec(),
            at: 1000,
            author: 7,
            signature: [0u8; 64],
        };
        let digest = tx.digest();
        tx.signature = sm2_sign(&digest, &kp.private_key, &kp.public_key, rng).unwrap().to_bytes();
        tx
    }

    #[test]
    fn test_arc004_tx_signature_valid() {
        let mut rng = CsRng::from_seed(&[0x11; 32]);
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let tx = signed_tx(1, b"deal-1", &kp, &mut rng);
        assert!(tx.verify_signature(&kp.public_key), "交易签名有效");
    }

    #[test]
    fn test_arc004_tx_signature_tampered_rejected() {
        let mut rng = CsRng::from_seed(&[0x22; 32]);
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut tx = signed_tx(2, b"deal-2", &kp, &mut rng);
        // 篡改 payload → 摘要变化 → 签名不匹配
        tx.payload[0] ^= 0xff;
        assert!(!tx.verify_signature(&kp.public_key), "篡改交易拒绝");
        // 篡改签名字节
        let mut tx2 = signed_tx(3, b"deal-3", &kp, &mut rng);
        tx2.signature[0] ^= 0xff;
        assert!(!tx2.verify_signature(&kp.public_key), "篡改签名拒绝");
        // 换公钥（他人公钥）
        let kp2 = Sm2KeyPair::generate(&mut rng).unwrap();
        let tx3 = signed_tx(4, b"deal-4", &kp, &mut rng);
        assert!(!tx3.verify_signature(&kp2.public_key), "错误公钥拒绝");
    }

    #[test]
    fn test_arc004_signed_tx_in_chain() {
        // 签名交易进入区块后链验证仍通过（哈希存证链闭环）
        let mut rng = CsRng::from_seed(&[0x33; 32]);
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let tx = signed_tx(5, b"sealed-deal", &kp, &mut rng);
        let mut chain = crate::LightChain::new();
        chain.append(tx.clone()).unwrap();
        chain.mine_block(2000, 1).unwrap();
        assert!(chain.verify().unwrap(), "存证链验证通过");
        // 篡改已入块交易 → 链验证失败
        let block = chain.get_block(1).unwrap();
        let _ = block; // 读借用；用独立验证路径
        let mut chain2 = crate::LightChain::new();
        let tx2 = tx.clone();
        chain2.append(tx2.clone()).unwrap();
        chain2.mine_block(2000, 1).unwrap();
        // 篡改 pending 前先入块
        let mut block2 = chain2.get_block(1).unwrap().clone();
        block2.txs[0].payload[0] ^= 0xff;
        // 重建链验证（merkle 变化）
        let merkle = crate::block::compute_merkle_root(&block2.txs);
        assert_ne!(merkle, block2.merkle_root, "篡改后 Merkle 根变化");
    }
}
