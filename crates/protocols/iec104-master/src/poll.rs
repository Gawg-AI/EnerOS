//! IEC 104 主站轮询调度（D10）.
//!
//! D10：`PollScheduler` 简化为基于 `now_ms` 的时间戳比较（无定时器对象，Simplicity First）。
//! 每个设备按各自 `poll_interval_ms` 独立调度，到期时返回 `common_addr` 供主站执行总召唤。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// 单个轮询任务
#[derive(Debug, Clone, Copy)]
pub struct PollTask {
    /// 公共地址（设备标识）
    pub common_addr: u16,
    /// 下次轮询时间戳（毫秒）
    pub next_poll_ms: u64,
    /// 轮询周期（毫秒）
    pub interval_ms: u32,
}

/// 轮询调度器（D10）
///
/// 基于 `now_ms` 时间戳比较，管理多个设备的轮询周期。
pub struct PollScheduler {
    /// 任务表（key: common_addr）
    pub tasks: BTreeMap<u16, PollTask>,
}

impl PollScheduler {
    /// 创建空调度器。
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
        }
    }

    /// 添加轮询任务，下次轮询时间为 `now_ms + interval_ms`。
    pub fn add_task(&mut self, common_addr: u16, interval_ms: u32, now_ms: u64) {
        self.tasks.insert(
            common_addr,
            PollTask {
                common_addr,
                next_poll_ms: now_ms + interval_ms as u64,
                interval_ms,
            },
        );
    }

    /// 移除轮询任务。
    pub fn remove_task(&mut self, common_addr: u16) {
        self.tasks.remove(&common_addr);
    }

    /// 返回当前到期的所有任务（`now_ms >= next_poll_ms`）的 `common_addr` 列表。
    pub fn due_tasks(&self, now_ms: u64) -> Vec<u16> {
        self.tasks
            .iter()
            .filter(|(_, task)| now_ms >= task.next_poll_ms)
            .map(|(addr, _)| *addr)
            .collect()
    }

    /// 更新指定任务的下次轮询时间为 `now_ms + interval_ms`。
    pub fn update_next(&mut self, common_addr: u16, now_ms: u64) {
        if let Some(task) = self.tasks.get_mut(&common_addr) {
            task.next_poll_ms = now_ms + task.interval_ms as u64;
        }
    }
}

impl Default for PollScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_empty() {
        let sched = PollScheduler::new();
        assert_eq!(sched.tasks.len(), 0);
        assert!(sched.due_tasks(0).is_empty());
    }

    #[test]
    fn test_add_task_and_due() {
        let mut sched = PollScheduler::new();
        sched.add_task(1, 30_000, 1000);

        // 未到期
        assert!(sched.due_tasks(1000).is_empty());
        assert!(sched.due_tasks(30_999).is_empty());

        // 到期
        let due = sched.due_tasks(31_000);
        assert_eq!(due, alloc::vec![1]);
    }

    #[test]
    fn test_remove_task() {
        let mut sched = PollScheduler::new();
        sched.add_task(1, 30_000, 0);
        sched.add_task(2, 30_000, 0);
        assert_eq!(sched.tasks.len(), 2);

        sched.remove_task(1);
        assert_eq!(sched.tasks.len(), 1);
        assert!(!sched.tasks.contains_key(&1));
        assert!(sched.tasks.contains_key(&2));
    }

    #[test]
    fn test_due_tasks_multiple() {
        let mut sched = PollScheduler::new();
        sched.add_task(1, 10_000, 0);
        sched.add_task(2, 20_000, 0);
        sched.add_task(3, 30_000, 0);

        // t=10000: 仅设备1到期
        let due = sched.due_tasks(10_000);
        assert_eq!(due, alloc::vec![1]);

        // t=20000: 设备1+2到期
        let due = sched.due_tasks(20_000);
        assert_eq!(due, alloc::vec![1, 2]);

        // t=30000: 全部到期
        let due = sched.due_tasks(30_000);
        assert_eq!(due, alloc::vec![1, 2, 3]);
    }

    #[test]
    fn test_update_next() {
        let mut sched = PollScheduler::new();
        sched.add_task(1, 30_000, 0);

        // 原本 t=30000 到期，t=29999 未到期
        assert!(sched.due_tasks(29_999).is_empty());
        let due = sched.due_tasks(30_000);
        assert_eq!(due, alloc::vec![1]);

        // 更新后：下次 = 30000 + 30000 = 60000
        sched.update_next(1, 30_000);
        assert!(sched.due_tasks(59_999).is_empty());
        let due = sched.due_tasks(60_000);
        assert_eq!(due, alloc::vec![1]);
    }

    #[test]
    fn test_update_next_nonexistent() {
        let mut sched = PollScheduler::new();
        // 不存在的任务，不应 panic
        sched.update_next(99, 1000);
        assert_eq!(sched.tasks.len(), 0);
    }

    #[test]
    fn test_due_tasks_empty_scheduler() {
        let sched = PollScheduler::new();
        assert!(sched.due_tasks(0).is_empty());
        assert!(sched.due_tasks(1_000_000).is_empty());
    }

    #[test]
    fn test_default() {
        let sched = PollScheduler::default();
        assert_eq!(sched.tasks.len(), 0);
    }
}
