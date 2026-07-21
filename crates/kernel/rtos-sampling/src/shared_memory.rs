//! 双缓冲共享内存快照 — SharedMemorySnapshot.
//!
//! [`SharedMemorySnapshot`] 采用乒乓双缓冲：写者写非活跃缓冲区后原子切换活跃索引，
//! 读者读活跃缓冲区并校验序列号未被切换，实现无锁一致性读取.
//!
//! # 偏差 D3/D4/D8
//!
//! - D3：不直接依赖 seL4 SharedMemory（用 in-memory 双缓冲，Phase 3 替换）.
//! - D4：`read()` 增加重试上限 `MAX_READ_RETRIES=10`（蓝图风险 8.3）.
//! - D8：使用 `core::sync::atomic::{AtomicU8, AtomicU64}` 进行活跃索引与序列号同步.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

use crate::snapshot::{SampledPoint, StateSnapshot, MAX_POINTS};

/// 读取重试上限（D4）.
pub const MAX_READ_RETRIES: usize = 10;

/// 乒乓双缓冲共享内存快照.
///
/// 单写者写非活跃缓冲区后原子切换 `active`，读者通过 seq 双读校验一致性.
/// 内部用 `UnsafeCell` 提供内部可变性（D8 原子同步）.
pub struct SharedMemorySnapshot {
    /// 双缓冲（buffer[0] 与 buffer[1] 乒乓切换）.
    buffers: UnsafeCell<[StateSnapshot; 2]>,
    /// 当前活跃缓冲区索引（0 或 1）.
    active: AtomicU8,
    /// 写序列号（每次写入递增）.
    write_seq: AtomicU64,
}

impl SharedMemorySnapshot {
    /// 创建双缓冲快照（active=0, write_seq=0, 两缓冲区全零）.
    pub fn new() -> Self {
        Self {
            buffers: UnsafeCell::new([StateSnapshot::new(), StateSnapshot::new()]),
            active: AtomicU8::new(0),
            write_seq: AtomicU64::new(0),
        }
    }

    /// 写入一帧快照到非活跃缓冲区，并原子切换活跃索引.
    ///
    /// 返回本次写入的序列号（从 1 开始递增）.
    ///
    /// # SAFETY（调用方契约）
    ///
    /// 单写者：同一时刻仅一个写者调用 `write`。写者写非活跃缓冲区
    /// （`active` 不指向它），写完后原子切换 `active`。
    pub fn write(&self, timestamp_us: u64, points: &[SampledPoint]) -> u64 {
        let inactive = 1 - self.active.load(Ordering::Acquire);
        // SAFETY: 单写者，inactive 缓冲区此时无读者（active 指向另一个缓冲区）。
        let buffers = unsafe { &mut *self.buffers.get() };
        let buf = &mut buffers[inactive as usize];
        buf.timestamp = timestamp_us;
        let seq = self.write_seq.fetch_add(1, Ordering::Relaxed) + 1;
        buf.seq = seq;
        let n = points.len().min(MAX_POINTS);
        buf.point_count = n as u32;
        buf.points[..n].copy_from_slice(&points[..n]);
        // 原子切换活跃缓冲区（Release 保证 buf 写入对读者可见）.
        self.active.store(inactive, Ordering::Release);
        seq
    }

    /// 读取当前活跃缓冲区的快照副本.
    ///
    /// 通过双读 active + seq 校验一致性：若读取期间发生写者切换，则重试，
    /// 重试上限 `MAX_READ_RETRIES`（D4）。返回 `None` 表示重试耗尽.
    pub fn read(&self) -> Option<StateSnapshot> {
        for _ in 0..MAX_READ_RETRIES {
            let active = self.active.load(Ordering::Acquire);
            // SAFETY: 读取 active 缓冲区（写者此时写 inactive 缓冲区）。
            let buffers = unsafe { &*self.buffers.get() };
            let buf = &buffers[active as usize];
            let seq = buf.seq;
            // 重新读取活跃索引和序列号，验证未发生切换.
            let active2 = self.active.load(Ordering::Acquire);
            let seq2 = buf.seq;
            if active == active2 && seq == seq2 {
                return Some(*buf);
            }
        }
        None
    }
}

impl Default for SharedMemorySnapshot {
    fn default() -> Self {
        Self::new()
    }
}
