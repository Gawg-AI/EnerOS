//! 轻量哈希链实现。

use alloc::vec::Vec;

use crate::block::{compute_block_hash, create_block, create_genesis};
use crate::types::{Block, TxRecord};
use crate::{Chain, ChainError};

/// 轻量区块链：维护已确认区块列表与待打包交易池。
pub struct LightChain {
    /// 已确认区块（含创世块）。
    blocks: Vec<Block>,
    /// 待打包交易。
    pending: Vec<TxRecord>,
}

impl LightChain {
    /// 创建新链，自动包含创世区块。
    pub fn new() -> Self {
        Self {
            blocks: alloc::vec![create_genesis()],
            pending: Vec::new(),
        }
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
}
