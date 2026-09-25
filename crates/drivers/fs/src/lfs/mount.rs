//! mount-or-format 单源（2026-09-24 架构收口 C13）。
//!
//! 收口前该习语在 telemetry_history（TSDB 区段，CachedBlocks<16>）与
//! event_face（快照区段，CachedBlocks<4>）各手抄一份，缓存深度与错误
//! 映射已现漂移；本函数把「先 mount 保数据、仅坏超级块才格式化一次」
//! 的策略收为一处，缓存深度/区段几何经闭包显式传入。

use alloc::boxed::Box;

use eneros_storage::ScopedBlockDevice;

use super::filesystem::Lfs;
use crate::error::FsError;

/// 保数据优先打开 LFS 卷：先 [`Lfs::mount`]；仅 [`FsError::BadSuperBlock`]
/// （首次上电/异盘——超级块缺失或无效）以第二个 fresh 作用域显式
/// [`Lfs::format`] 一次；其余错误 fail-closed 原样吞并（不覆盖任何
/// 在卷数据）。
///
/// `make_scope` 每次调用产出一个**同介质**的新作用域——mount 与 format
/// 重试各消费一个（`Box<dyn ScopedBlockDevice>` 所有权让渡），`Clone`
/// face 的调用方以 `|| RegionScope::new(face.clone(), …)` 满足。作用域
/// 构造失败（区段越界等）与 mount/format 失败一并折叠为 `None`（与
/// 收口前两调用点的吞错口径一致；需区分错误因的调用方直接用
/// [`Lfs::mount`]）。
pub fn mount_or_format<T, F>(make_scope: F) -> Option<Lfs>
where
    T: ScopedBlockDevice + 'static,
    F: Fn() -> Option<T>,
{
    let scope = make_scope()?;
    match Lfs::mount(Box::new(scope)) {
        Ok(fs) => Some(fs),
        Err(FsError::BadSuperBlock) => {
            let dev2 = make_scope()?;
            Lfs::format(Box::new(dev2)).ok()
        }
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use alloc::rc::Rc;
    use core::cell::RefCell;

    use eneros_storage::partition::RegionScope;
    use eneros_storage::{BlockDevice, MockBlockDevice};

    use super::*;
    use crate::fs_trait::FileSystem;
    use crate::types::FileMode;

    /// 共享介质包装（mount 与 format 重试双 face 的 host 测试面；target
    /// 侧等价物 = `EipcBlockDevice: Clone`）。
    #[derive(Clone)]
    struct SharedDev(Rc<RefCell<MockBlockDevice>>);

    impl BlockDevice for SharedDev {
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

    /// 首次打开（空白介质）= mount 拒 BadSuperBlock → 显式格式化一次；
    /// 同介质再开走 mount 路径且**在卷数据保持**（格式化不可静默重置卷）
    /// ——这正是收口前两调用点各自的保数据语义，现为单源行为锁。
    #[test]
    fn first_open_formats_reopen_preserves_data() {
        let mem = SharedDev(Rc::new(RefCell::new(MockBlockDevice::new(64, 4096))));
        let make = || RegionScope::new(mem.clone(), 0, 64).ok();

        let mut fs = mount_or_format(make).expect("空白介质首开 = format + mount");
        let mut f = fs
            .create("/keep.txt", FileMode::default_file())
            .expect("create");
        f.write(&mut fs, b"survives").expect("write");
        drop(f);

        let fs2 = mount_or_format(make).expect("已格式化介质重开 = mount");
        assert_eq!(
            fs2.stat("/keep.txt").expect("stat").size,
            8,
            "重开不得二次 format（在卷数据必须保持）"
        );
    }

    /// 作用域构造失败（区段越界）→ None（fail-closed，不猜介质）。
    #[test]
    fn scope_construction_failure_is_none() {
        let mem = SharedDev(Rc::new(RefCell::new(MockBlockDevice::new(8, 4096))));
        assert!(
            mount_or_format(|| RegionScope::new(mem.clone(), 0, 64).ok()).is_none(),
            "越界区段必须整体拒绝"
        );
    }
}
