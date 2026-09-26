//! LFS 区段几何单源（R16-M，蓝图 75-03 WP-75.3.3）。
//!
//! 「512B 扇区区段 → 4K 块」折算律（8 倍率 + 整除对齐）此前以算术
//! 惯用法散在 control-services 两文件多处（event_face / telemetry_
//! history 的本地 `FS_BLOCK_SIZE` const、`/ 8`、`% 8` 与同文案
//! assert——C13 只收编了 mount-or-format 决策，未收编几何折算），
//! 自此收编本模块；下游区段常量 `DATA_DISK_BLOCKS`（ota-ab）已单源，
//! 倍率与对齐是唯一残差。

/// LFS 逻辑块大小（单源；调用方本地同名 const 退役）。
pub const LFS_BLOCK_SIZE: usize = 4096;

/// 512B 扇区字节数（折算输入口径）。
const SECTOR_SIZE: u64 = 512;

/// 每 LFS 块的扇区数（8——折算倍率单源）。
const SECTORS_PER_BLOCK: u64 = LFS_BLOCK_SIZE as u64 / SECTOR_SIZE;

/// 折算后的区段几何（4K 块计）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionGeometry {
    /// 区段基址（4K 块号）。
    pub base_block: u64,
    /// 区段长度（4K 块数）。
    pub blocks: u64,
}

impl RegionGeometry {
    /// 512B 扇区口径的区段（基址 + 长度）折算为 4K 块几何。
    ///
    /// 整除/对齐律不满足（`base_block_512 % 8 != 0` 或
    /// `region_blocks_512 % 8 != 0`——等价于原 `(blocks*512) % 4096`
    /// 判式）→ `None`。调用方**钳制策略**（如 `.min(64)` 容量上限）
    /// 留在调用方——那是策略不是几何。
    pub fn from_blocks_512(base_block_512: u64, region_blocks_512: u64) -> Option<Self> {
        if base_block_512 % SECTORS_PER_BLOCK != 0 || region_blocks_512 % SECTORS_PER_BLOCK != 0 {
            return None;
        }
        Some(Self {
            base_block: base_block_512 / SECTORS_PER_BLOCK,
            blocks: region_blocks_512 / SECTORS_PER_BLOCK,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R16-M：换算正确（8×512B 扇区 = 1×4K 块）、4096 边界、对齐拒绝
    ///（base 非整块 / 块数非整除）。
    #[test]
    fn converts_and_rejects_misaligned() {
        let g = RegionGeometry::from_blocks_512(8, 16).expect("aligned");
        assert_eq!(g.base_block, 1);
        assert_eq!(g.blocks, 2);
        // 4096 边界：恰一块。
        let g = RegionGeometry::from_blocks_512(8, 8).expect("aligned");
        assert_eq!(g.base_block, 1);
        assert_eq!(g.blocks, 1);
        // 全盘区段（DATA_DISK_BLOCKS=32768 扇区 → 4096 块；base=0）。
        let g = RegionGeometry::from_blocks_512(0, 32768).expect("aligned");
        assert_eq!(g.blocks, 4096);
        // 零长区段：0 对齐（与原 `0 / 8 == 0` 行为等价，Some(0)）。
        let g = RegionGeometry::from_blocks_512(0, 0).expect("zero aligned");
        assert_eq!(g.blocks, 0);
        // 对齐拒绝：base 非整块。
        assert!(RegionGeometry::from_blocks_512(7, 8).is_none());
        // 块数非整除拒绝。
        assert!(RegionGeometry::from_blocks_512(0, 7).is_none());
        assert!(RegionGeometry::from_blocks_512(1, 9).is_none());
    }
}
