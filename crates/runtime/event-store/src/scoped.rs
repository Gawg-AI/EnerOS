//! 区段作用域打开薄壳（蓝图 68 卷 68.4，WP-68.4.0 裁决 1）。
//!
//! 生产接线注意（crate 文档 §1"durable-log 收尾复验同款纪律"的类型门
//! 落地）：调用方以**区段常量**（ota-ab `DATA_EVENT_*`/`DATA_FACT_*`）
//! 构造区段作用域——`base_block`/`blocks` 以 `D::block_size()`（必须
//! 4096）计块；整盘裸 handle 在 [`RegionScope`] 之下不得触 LBA0/GPT，
//! 越区段写一律 `OutOfRange`（t3 负例锁）。
//!
//! open-or-format 语义与 telemetry_history 的 LFS mount-or-format 同律：
//! open 保数据；仅 `NotFormatted`（全零/全 FF 超级块 = 首次上电/异盘重划）
//! 显式格式化一次；`ForeignFormat`（外来格式）fail-closed 拒绝不覆盖。
//!
//! 返回 [`crate::RecoveryReport`]：open 扫描后 [`EventStore::recover`]
//! 的恢复摘要（装配面串口词形 `EVENT-REPLAY recovered=<n>` 的数据源；
//! CachedBlocks 热缓存下二次扫描块取近零）。

use eneros_storage::partition::RegionScope;
use eneros_storage::BlockDevice;

use crate::block::BlockEventStore;
use crate::{EventStore, RecoveryReport, StoreConfig, StoreError};

/// 打开（或首次格式化后打开）区段作用域的分区事件日志。
///
/// `make_dev` 每次调用产出一个**同介质**的新设备面（open 与 format 重试
/// 各需一个 fresh face——format/open 消费所有权；`EipcBlockDevice: Clone`
/// 的调用方以 `|| face.clone()` 满足）。`base_block + blocks` 越出底层
/// 容量 → [`StoreError::BadConfig`]（fail-closed）。
pub fn open_region_scoped<D, F>(
    make_dev: F,
    base_block: u64,
    blocks: u64,
    config: StoreConfig,
) -> Result<(BlockEventStore<RegionScope<D>>, RecoveryReport), StoreError>
where
    D: BlockDevice + Clone,
    F: Fn() -> D,
{
    let scope =
        RegionScope::new(make_dev(), base_block, blocks).map_err(|_| StoreError::BadConfig)?;
    let mut store = match BlockEventStore::open(scope, config) {
        Ok(s) => s,
        Err(StoreError::NotFormatted) => {
            let scope2 = RegionScope::new(make_dev(), base_block, blocks)
                .map_err(|_| StoreError::BadConfig)?;
            BlockEventStore::format(scope2, config)?
        }
        Err(e) => return Err(e),
    };
    let report = store.recover()?;
    Ok((store, report))
}

#[cfg(test)]
mod tests {
    use alloc::rc::Rc;
    use core::cell::RefCell;

    use super::*;
    use crate::format::BLOCK_SIZE;
    use crate::memdev::MemBlockDevice;
    use crate::{RecordRef, RecordVisitor};

    /// 共享介质包装（open/format 双 face 的 host 测试面；target 侧等价物
    /// = `EipcBlockDevice: Clone`）。
    #[derive(Clone)]
    struct SharedMem(Rc<RefCell<MemBlockDevice>>);

    impl BlockDevice for SharedMem {
        fn read_block(&self, idx: u64, buf: &mut [u8]) -> Result<(), eneros_storage::StorageError> {
            self.0.borrow().read_block(idx, buf)
        }
        fn write_block(
            &mut self,
            idx: u64,
            buf: &[u8],
        ) -> Result<(), eneros_storage::StorageError> {
            self.0.borrow_mut().write_block(idx, buf)
        }
        fn erase_block(&mut self, idx: u64) -> Result<(), eneros_storage::StorageError> {
            self.0.borrow_mut().erase_block(idx)
        }
        fn block_count(&self) -> u64 {
            self.0.borrow().block_count()
        }
        fn block_size(&self) -> usize {
            self.0.borrow().block_size()
        }
        fn flush(&mut self) -> Result<(), eneros_storage::StorageError> {
            self.0.borrow_mut().flush()
        }
        fn health_status(&self) -> eneros_storage::DeviceHealth {
            self.0.borrow().health_status()
        }
    }

    /// 收集全部 payload 的 visitor（断言辅助）。
    struct Collector<'a>(pub &'a mut alloc::vec::Vec<alloc::vec::Vec<u8>>);

    impl RecordVisitor for Collector<'_> {
        fn record(&mut self, r: RecordRef<'_>) -> bool {
            self.0.push(r.payload.to_vec());
            true
        }
    }

    fn config() -> StoreConfig {
        StoreConfig {
            segment_bytes: 16384,
            ..StoreConfig::default()
        }
    }

    /// 首次打开 = 显式格式化一次；再打开走 mount 路径且数据保持
    /// （open-or-format 语义）。
    #[test]
    fn scoped_open_or_format_and_remount() {
        let mem = SharedMem(Rc::new(RefCell::new(MemBlockDevice::new(36, BLOCK_SIZE))));
        let make = || mem.clone();
        // 36 块：3 固定 + 2×16KiB 段槽（4 块/段）+ 1 余。
        let (mut store, report) =
            open_region_scoped(make, 4, 32, config()).expect("首次打开 = format");
        assert_eq!(report.valid_records, 0);
        store
            .append(7, [1u8; 16], 0, b"hello-fact")
            .expect("append");
        store.flush().expect("flush");
        // 掉电重启：同介质重新 open（不得二次 format——格式化不可静默
        // 重置卷）。
        let (store2, report2) = open_region_scoped(make, 4, 32, config()).expect("重开");
        assert_eq!(report2.valid_records, 1, "已 ACK 记录恢复");
        let mut got = alloc::vec::Vec::new();
        store2.read(7, 0, &mut Collector(&mut got)).expect("read");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], b"hello-fact");
    }

    /// 越区段基址/长度 → 构造拒绝（fail-closed；t3 的 crate 级形态）。
    #[test]
    fn scoped_rejects_out_of_range_region() {
        let mem = SharedMem(Rc::new(RefCell::new(MemBlockDevice::new(8, BLOCK_SIZE))));
        let make = || mem.clone();
        let r = open_region_scoped(make, 4, 32, config())
            .err()
            .expect("必须拒绝");
        assert_eq!(r, StoreError::BadConfig, "越界区段必须拒绝");
    }
}
