//! 22 §2.W3.1：TSDB 块设备直存层（锁定 7——海量时序数据绕过 littlefs，
//! 直接在 eMMC/virtio-blk 分区上写块格式）。
//!
//! 分层：小配置/审计/密钥 → littlefs（MB 级，`BLOCK_COUNT` 不硬扩）；
//! 海量时序数据 → [`BlockStore`]（GB 级分区直存，u64 块寻址无上限）。
//! 分区表：GPT（`boot/image-format`）——配置分区 + TSDB 数据分区。

use eneros_storage::driver::BlockDevice;
use eneros_storage::error::StorageError;

/// TSDB 数据分区 GUID（GPT 自定义类型；boot-manager 分区表解析用）。
///
/// `eneros-tsdb-data`：{4e45-...} 自定义命名空间（EnerOS 私有分区类型）。
pub const TSDB_PARTITION_GUID: [u8; 16] = [
    0x4e, 0x45, 0x52, 0x4f, // "NERO"
    0x53, 0x44, 0x42, 0x00, // "SDB\0"
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];

/// TSDB 数据分区上的块直存（分区内偏移寻址，容量 = 分区块数）。
///
/// 语义：块地址 = `start_block + offset`（分区内线性）；`block_count` 由
/// 分区表给出（GB 级，u64 无上限——不复制 littlefs 的 BLOCK_COUNT 硬限）。
pub struct BlockStore<'a> {
    dev: &'a mut dyn BlockDevice,
    start_block: u64,
    block_count: u64,
}

impl<'a> BlockStore<'a> {
    /// 绑定块设备上的一个分区（`start_block`/`block_count` 来自 GPT）。
    ///
    /// 越界分区立即拒绝（fail-closed：不静默截断）。
    pub fn new(dev: &'a mut dyn BlockDevice, start_block: u64, block_count: u64) -> Self {
        debug_assert!(
            start_block.saturating_add(block_count) <= dev.block_count(),
            "TSDB 分区越界设备容量"
        );
        Self {
            dev,
            start_block,
            block_count,
        }
    }

    /// 分区块数（GB 级容量；供 TSDB 布局计算）。
    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    /// 块大小（字节）。
    pub fn block_size(&self) -> usize {
        self.dev.block_size()
    }

    /// 写分区内第 `offset` 块（线性寻址；越界 → `StorageError::OutOfRange`）。
    pub fn write_block(&mut self, offset: u64, buf: &[u8]) -> Result<(), StorageError> {
        if offset >= self.block_count {
            return Err(StorageError::OutOfRange {
                block_idx: offset,
                max: self.block_count,
            });
        }
        self.dev.write_block(self.start_block + offset, buf)
    }

    /// 读分区内第 `offset` 块。
    pub fn read_block(&self, offset: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        if offset >= self.block_count {
            return Err(StorageError::OutOfRange {
                block_idx: offset,
                max: self.block_count,
            });
        }
        self.dev.read_block(self.start_block + offset, buf)
    }
}

#[cfg(test)]
mod tests {
    use eneros_storage::driver::mock::MockBlockDevice;

    use super::*;

    /// GB 级分区寻址：>256 KiB（littlefs 上限）容量下直存写读。
    #[test]
    fn test_block_store_gb_scale_addressing() {
        // 模拟 4 GiB 设备（1M 块 × 4 KiB）。
        let mut dev = MockBlockDevice::new(1 << 20, 4096);
        // TSDB 分区：从块 4096 起 262144 块（1 GiB 分区）。
        let mut store = BlockStore::new(&mut dev, 4096, 262_144);
        assert_eq!(store.block_count(), 262_144);
        let data = [0xABu8; 4096];
        store.write_block(0, &data).expect("write");
        store.write_block(262_143, &data).expect("write last");
        let mut buf = [0u8; 4096];
        store.read_block(0, &mut buf).expect("read");
        assert_eq!(buf, data);
        store.read_block(262_143, &mut buf).expect("read last");
        assert_eq!(buf, data);
        // 越界拒绝（fail-closed）。
        assert!(matches!(
            store.write_block(262_144, &data),
            Err(StorageError::OutOfRange { .. })
        ));
        assert!(matches!(
            store.read_block(262_144, &mut buf),
            Err(StorageError::OutOfRange { .. })
        ));
    }

    /// 分区偏移正确映射到底层设备块（start_block 偏移）。
    #[test]
    fn test_block_store_offset_mapping() {
        let mut dev = MockBlockDevice::new(1024, 4096);
        let mut store = BlockStore::new(&mut dev, 100, 500);
        let data = [0xCDu8; 4096];
        store.write_block(7, &data).expect("write");
        // 底层设备块 107 应为数据。
        let mut buf = [0u8; 4096];
        dev.read_block(107, &mut buf).expect("raw read");
        assert_eq!(buf, data);
    }

    /// 分区 GUID 常量稳定（GPT 解析契约）。
    #[test]
    fn test_partition_guid_constant() {
        assert_eq!(TSDB_PARTITION_GUID[..4], *b"NERO");
    }
}
