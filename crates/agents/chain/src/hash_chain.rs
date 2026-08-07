//! 轻量哈希链实现。

use alloc::vec::Vec;

use crate::block::{compute_block_hash, create_block, create_genesis};
use crate::types::{Block, TxRecord};
use crate::{Chain, ChainError};

/// 轻量存证哈希链（v0.232.0 正名：单机存证，非区块链共识）。
///
/// 维护已确认区块列表与待打包交易池；入块强制 SM2 验签。
pub struct LightChain {
    /// 已确认区块（含创世块）。
    blocks: Vec<Block>,
    /// 待打包交易。
    pending: Vec<TxRecord>,
    /// 交易验签公钥（None = 未启用验签）。
    verifier_pubkey: Option<eneros_crypto::Sm2PublicKey>,
}

impl LightChain {
    /// 创建新链，自动包含创世区块。
    pub fn new() -> Self {
        Self {
            blocks: alloc::vec![create_genesis()],
            pending: Vec::new(),
            verifier_pubkey: None,
        }
    }

    /// 启用入块验签（v0.232.0）：设置交易验签公钥后，全零签名或
    /// 验签失败的交易在 `mine_block` 时被拒绝（不打包）。
    pub fn set_verifier_pubkey(&mut self, pubkey: eneros_crypto::Sm2PublicKey) {
        self.verifier_pubkey = Some(pubkey);
    }
}

impl Default for LightChain {
    fn default() -> Self {
        Self::new()
    }
}

impl Chain for LightChain {
    fn append(&mut self, tx: TxRecord) -> Result<u64, ChainError> {
        let id = tx.tx_id;
        self.pending.push(tx);
        Ok(id)
    }

    fn mine_block(&mut self, timestamp: u64, proposer: u64) -> Result<u64, ChainError> {
        if self.pending.is_empty() {
            return Err(ChainError::EmptyBlock);
        }
        // v0.232.0：入块强制验签（启用公钥时）——全零签名或验签失败
        // 的交易被拒绝（不静默打包）。
        if let Some(pubkey) = &self.verifier_pubkey {
            for tx in &self.pending {
                if tx.signature == [0u8; 64] {
                    return Err(ChainError::BadTxSignature);
                }
                if !tx.verify_signature(pubkey) {
                    return Err(ChainError::BadTxSignature);
                }
            }
        }
        let index = self.blocks.len() as u64;
        let prev_block = &self.blocks[index as usize - 1];
        let prev_hash = compute_block_hash(prev_block);
        let txs = core::mem::take(&mut self.pending);
        let block = create_block(index, prev_hash, txs, timestamp, proposer);
        self.blocks.push(block);
        Ok(index)
    }

    fn verify(&self) -> Result<bool, ChainError> {
        if self.blocks.is_empty() {
            return Err(ChainError::NotFound);
        }
        for i in 1..self.blocks.len() {
            let expected = compute_block_hash(&self.blocks[i - 1]);
            if self.blocks[i].prev_hash != expected {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn get_block(&self, index: u64) -> Result<&Block, ChainError> {
        self.blocks.get(index as usize).ok_or(ChainError::NotFound)
    }

    fn height(&self) -> u64 {
        self.blocks.len() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TxKind;

    fn sample_tx(id: u64) -> TxRecord {
        TxRecord {
            tx_id: id,
            kind: TxKind::Deal,
            payload: alloc::vec![id as u8],
            at: id * 100,
            author: 0,
            signature: [0u8; 64],
        }
    }

    #[test]
    fn append_and_mine() {
        let mut chain = LightChain::new();
        assert_eq!(chain.height(), 1); // genesis

        let id = chain.append(sample_tx(1)).unwrap();
        assert_eq!(id, 1);

        let idx = chain.mine_block(1000, 1).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(chain.height(), 2);
    }

    #[test]
    fn mine_empty_fails() {
        let mut chain = LightChain::new();
        assert_eq!(chain.mine_block(1000, 1), Err(ChainError::EmptyBlock));
    }

    #[test]
    fn verify_valid_chain() {
        let mut chain = LightChain::new();
        chain.append(sample_tx(1)).unwrap();
        chain.mine_block(100, 1).unwrap();
        chain.append(sample_tx(2)).unwrap();
        chain.mine_block(200, 1).unwrap();

        assert_eq!(chain.verify(), Ok(true));
    }

    #[test]
    fn tamper_detection() {
        let mut chain = LightChain::new();
        chain.append(sample_tx(1)).unwrap();
        chain.mine_block(100, 1).unwrap();
        chain.append(sample_tx(2)).unwrap();
        chain.mine_block(200, 1).unwrap();

        // Tamper with block 0
        chain.blocks[0].timestamp = 9999;
        assert_eq!(chain.verify(), Ok(false));
    }

    #[test]
    fn get_block_valid_and_invalid() {
        let chain = LightChain::new();
        assert!(chain.get_block(0).is_ok());
        assert_eq!(chain.get_block(99), Err(ChainError::NotFound));
    }

    // ===== v0.232.0（B3-09）：入块强制验签 =====

    fn signed_tx(
        id: u64,
        payload: &[u8],
        kp: &eneros_crypto::Sm2KeyPair,
        rng: &mut eneros_crypto::CsRng,
    ) -> TxRecord {
        let mut tx = TxRecord {
            tx_id: id,
            kind: crate::TxKind::Deal,
            payload: payload.to_vec(),
            at: 1000,
            author: 7,
            signature: [0u8; 64],
        };
        let digest = tx.digest();
        tx.signature = eneros_crypto::sm2_sign(&digest, &kp.private_key, &kp.public_key, rng)
            .unwrap()
            .to_bytes();
        tx
    }

    #[test]
    fn test_zero_signature_rejected() {
        let mut chain = LightChain::new();
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x31; 32]);
        let kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).unwrap();
        chain.set_verifier_pubkey(kp.public_key);
        let tx = TxRecord {
            tx_id: 1,
            kind: crate::TxKind::Deal,
            payload: alloc::vec![1, 2, 3],
            at: 1000,
            author: 7,
            signature: [0u8; 64], // 全零签名
        };
        chain.append(tx).expect("append");
        assert_eq!(
            chain.mine_block(2000, 1),
            Err(ChainError::BadTxSignature),
            "全零签名交易必须被拒绝"
        );
    }

    #[test]
    fn test_valid_signature_accepted() {
        let mut chain = LightChain::new();
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x32; 32]);
        let kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).unwrap();
        chain.set_verifier_pubkey(kp.public_key);
        let tx = signed_tx(1, b"deal-1", &kp, &mut rng);
        chain.append(tx).expect("append");
        chain.mine_block(2000, 1).expect("mine with valid sig");
    }

    #[test]
    fn test_tampered_tx_rejected() {
        let mut chain = LightChain::new();
        let mut rng = eneros_crypto::CsRng::from_seed(&[0x33; 32]);
        let kp = eneros_crypto::Sm2KeyPair::generate(&mut rng).unwrap();
        chain.set_verifier_pubkey(kp.public_key);
        let mut tx = signed_tx(1, b"deal-1", &kp, &mut rng);
        // 篡改 payload（签名不变）。
        tx.payload = alloc::vec![2];
        chain.append(tx).expect("append");
        assert_eq!(
            chain.mine_block(2000, 1),
            Err(ChainError::BadTxSignature),
            "篡改交易必须被拒绝"
        );
    }

    #[test]
    fn test_verification_optional_when_no_pubkey() {
        // 未启用验签（默认）：保持旧行为（存证链兼容路径）。
        let mut chain = LightChain::new();
        let tx = TxRecord {
            tx_id: 1,
            kind: crate::TxKind::Deal,
            payload: alloc::vec![1],
            at: 1000,
            author: 7,
            signature: [0u8; 64], // 全零但未启用验签
        };
        chain.append(tx).expect("append");
        chain.mine_block(2000, 1).expect("mine without verifier");
    }
}
