//! 告警分级与管理（v0.118.0）.
//!
//! 提供告警处理器抽象 [`AlertHandler`]（供联动响应扩展，如 High/Critical
//! 联动 System Agent）与定容告警队列 [`AlertQueue`]：队列满时显式返回
//! [`AnomalyError::AlertQueueFull`]，不静默丢弃，保证告警可审计。

use alloc::vec::Vec;

use crate::baseline::Alert;
use crate::AnomalyError;

/// 告警处理器抽象（检测器产出告警后的联动响应扩展点）.
pub trait AlertHandler {
    /// 处理一条告警（实现方决定联动动作，如通知 System Agent）.
    fn on_alert(&mut self, alert: &Alert);
}

/// 定容告警队列（FIFO；满时 push 报错，不静默丢弃）.
#[derive(Debug, Clone)]
pub struct AlertQueue {
    /// 队列存储（按入队顺序，队首为最旧告警）.
    queue: Vec<Alert>,
    /// 容量上限.
    capacity: usize,
}

impl AlertQueue {
    /// 构造容量为 `capacity` 的空队列.
    pub fn new(capacity: usize) -> Self {
        AlertQueue {
            queue: Vec::new(),
            capacity,
        }
    }

    /// 入队一条告警；已达容量上限时返回
    /// [`AnomalyError::AlertQueueFull`]（告警不被丢弃，由调用方决策）.
    pub fn push(&mut self, alert: Alert) -> Result<(), AnomalyError> {
        if self.queue.len() >= self.capacity {
            return Err(AnomalyError::AlertQueueFull);
        }
        self.queue.push(alert);
        Ok(())
    }

    /// 弹出最旧的一条告警（FIFO）；空队列返回 `None`.
    pub fn pop_oldest(&mut self) -> Option<Alert> {
        if self.queue.is_empty() {
            return None;
        }
        Some(self.queue.remove(0))
    }

    /// 当前队列内告警数.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// 队列是否为空.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 按入队顺序迭代队列内告警（不取出）.
    pub fn iter(&self) -> core::slice::Iter<'_, Alert> {
        self.queue.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::baseline::{AlertLevel, AnomalyType};

    /// 构造指定 id 的测试告警.
    fn make_alert(id: u64) -> Alert {
        let mut detail = heapless::String::<256>::new();
        let _ = detail.push_str("test alert");
        Alert {
            id,
            timestamp_ms: 1_000 + id,
            agent_id: 7,
            anomaly: AnomalyType::RateSpike,
            level: AlertLevel::Low,
            detail,
            evidence_seq: 100 + id,
        }
    }

    /// ALT5：告警级别可排序，Critical>High>Medium>Low>Info.
    #[test]
    fn alt5_alert_level_ordering() {
        assert!(AlertLevel::Critical > AlertLevel::High);
        assert!(AlertLevel::High > AlertLevel::Medium);
        assert!(AlertLevel::Medium > AlertLevel::Low);
        assert!(AlertLevel::Low > AlertLevel::Info);
        assert_eq!(AlertLevel::Info as u8, 0);
        assert_eq!(AlertLevel::Critical as u8, 4);
    }

    /// ALT6：队列 push 后按 FIFO 顺序 pop.
    #[test]
    fn alt6_queue_fifo_order() {
        let mut q = AlertQueue::new(4);
        assert!(q.is_empty());
        q.push(make_alert(1)).expect("容量内应入队成功");
        q.push(make_alert(2)).expect("容量内应入队成功");
        q.push(make_alert(3)).expect("容量内应入队成功");
        assert_eq!(q.len(), 3);
        assert!(!q.is_empty());
        // 迭代顺序与入队一致
        let ids: Vec<u64> = q.iter().map(|a| a.id).collect();
        assert_eq!(ids, [1, 2, 3]);
        // FIFO：1 → 2 → 3 → 空
        assert_eq!(q.pop_oldest().map(|a| a.id), Some(1));
        assert_eq!(q.pop_oldest().map(|a| a.id), Some(2));
        assert_eq!(q.pop_oldest().map(|a| a.id), Some(3));
        assert_eq!(q.pop_oldest().map(|a| a.id), None);
        assert!(q.is_empty());
    }

    /// ALT7：队列满时 push 显式返回 AlertQueueFull，不静默丢弃.
    #[test]
    fn alt7_full_queue_returns_alert_queue_full() {
        let mut q = AlertQueue::new(2);
        q.push(make_alert(1)).expect("容量内应入队成功");
        q.push(make_alert(2)).expect("容量内应入队成功");
        let err = q.push(make_alert(3)).expect_err("满队应报错");
        assert_eq!(err, AnomalyError::AlertQueueFull);
        // 原有内容不受影响
        assert_eq!(q.len(), 2);
        assert_eq!(q.pop_oldest().map(|a| a.id), Some(1));
    }
}
