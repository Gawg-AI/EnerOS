//! 轻量共识机制。

use crate::types::Block;
use crate::ChainError;

/// 共识 trait：提议与提交区块。
pub trait Consensus {
    /// 提议一个区块（验证阶段）。
    fn propose(&self, block: &Block) -> Result<(), ChainError>;
    /// 提交（确认）一个区块。
    fn commit(&self, block: &Block) -> Result<(), ChainError>;
}

/// 单节点共识：直接接受所有提议（适用于边缘单节点部署）。
pub struct SingleNodeConsensus;

impl Consensus for SingleNodeConsensus {
    fn propose(&self, _block: &Block) -> Result<(), ChainError> {
        Ok(())
    }

    fn commit(&self, _block: &Block) -> Result<(), ChainError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::create_genesis;

    #[test]
    fn single_node_accepts_genesis() {
        let consensus = SingleNodeConsensus;
        let genesis = create_genesis();
        assert!(consensus.propose(&genesis).is_ok());
        assert!(consensus.commit(&genesis).is_ok());
    }

    #[test]
    fn single_node_accepts_any_block() {
        use crate::block::create_block;
        use crate::types::Sm3Digest;

        let consensus = SingleNodeConsensus;
        let blk = create_block(1, Sm3Digest::ZERO, alloc::vec![], 100, 1);
        assert!(consensus.propose(&blk).is_ok());
        assert!(consensus.commit(&blk).is_ok());
    }
}
