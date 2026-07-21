//! 固定容量环形缓冲（溢出覆盖最旧）。

use alloc::vec::Vec;

/// 固定容量环形缓冲，溢出时覆盖最旧元素。
///
/// 内部使用 `Vec<T>` 管理存储，`head` 指向最旧元素（仅在满时有效）。
/// 简化说明：spec 接口列出的 `tail`/`len` 字段被 `head` + `buf.len()` 隐式替代，
/// 语义等价（满时覆盖最旧、drain 按旧→新顺序返回）。
pub struct RingBuffer<T> {
    buf: Vec<T>,
    head: usize,
    capacity: usize,
}

impl<T> RingBuffer<T> {
    /// 创建空缓冲，指定最大容量。
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::new(),
            head: 0,
            capacity,
        }
    }

    /// 压入元素；未满时追加，满则覆盖最旧元素。
    pub fn push(&mut self, item: T) {
        if self.buf.len() < self.capacity {
            self.buf.push(item);
        } else {
            self.buf[self.head] = item;
            self.head = (self.head + 1) % self.capacity;
        }
    }

    /// 取出全部元素（旧→新顺序）并清空缓冲。
    pub fn drain(&mut self) -> Vec<T> {
        if self.buf.is_empty() {
            return Vec::new();
        }
        if self.buf.len() < self.capacity {
            // 未满：直接取走全部，顺序即插入顺序
            let mut out = Vec::new();
            core::mem::swap(&mut out, &mut self.buf);
            self.head = 0;
            return out;
        }
        // 已满：rotate_left 使 head（最旧）移到索引 0，再整体取走
        self.buf.rotate_left(self.head);
        let mut out = Vec::new();
        core::mem::swap(&mut out, &mut self.buf);
        self.head = 0;
        out
    }

    /// 当前元素数。
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::RingBuffer;

    #[test]
    fn rb1_new_creates_empty_buffer() {
        let rb = RingBuffer::<i32>::new(4);
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());
    }

    #[test]
    fn rb2_push_under_capacity_appends() {
        let mut rb = RingBuffer::new(4);
        rb.push(10);
        rb.push(20);
        assert_eq!(rb.len(), 2);
        let drained = rb.drain();
        assert_eq!(drained, alloc::vec![10, 20]);
    }

    #[test]
    fn rb3_push_at_capacity_overwrites_oldest() {
        let mut rb = RingBuffer::new(3);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4);
        let drained = rb.drain();
        assert_eq!(drained, alloc::vec![2, 3, 4]);
    }

    #[test]
    fn rb4_drain_returns_all_and_empties() {
        let mut rb = RingBuffer::new(4);
        rb.push(1);
        rb.push(2);
        let first = rb.drain();
        assert_eq!(first, alloc::vec![1, 2]);
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());
        let second = rb.drain();
        assert!(second.is_empty());
    }

    #[test]
    fn rb5_len_and_is_empty_across_transitions() {
        let mut rb = RingBuffer::new(2);
        assert!(rb.is_empty());
        rb.push(1);
        assert_eq!(rb.len(), 1);
        assert!(!rb.is_empty());
        rb.push(2);
        assert_eq!(rb.len(), 2);
        rb.push(3);
        assert_eq!(rb.len(), 2);
        let _ = rb.drain();
        assert!(rb.is_empty());
    }

    #[test]
    fn rb6_order_preserved_after_multiple_overflows() {
        let mut rb = RingBuffer::new(4);
        for i in 1..=6 {
            rb.push(i);
        }
        let drained = rb.drain();
        assert_eq!(drained, alloc::vec![3, 4, 5, 6]);
    }
}
