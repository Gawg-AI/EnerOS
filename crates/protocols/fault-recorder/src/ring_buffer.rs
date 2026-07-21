//! 固定容量环形采样缓冲（溢出覆盖最旧，`get_recent` 按时间旧→新保序）。
//!
//! 蓝图 §4.5 基型（D5：`Box<[T]>` → `Vec<T>` 固定容量）。
//! 多通道场景按帧交错存储（D10）：帧 i 的通道 c 位于 `i × n_ch + c`。

use alloc::vec::Vec;

/// 固定容量环形采样缓冲，写满后覆盖最旧元素。
///
/// - `data`：预分配 `capacity` 个 `T::default()` 槽位
/// - `write_pos`：下一次写入的槽位下标
/// - `samples_written`：累计写入总数（用于未写满判定）
pub struct RingSampleBuffer<T: Copy> {
    data: Vec<T>,
    capacity: usize,
    write_pos: usize,
    samples_written: u64,
}

impl<T: Copy + Default> RingSampleBuffer<T> {
    /// 创建固定容量缓冲（容量 0 时退化为空缓冲：写入丢弃、读取恒空）。
    pub fn new(capacity: usize) -> Self {
        Self {
            data: alloc::vec![T::default(); capacity],
            capacity,
            write_pos: 0,
            samples_written: 0,
        }
    }

    /// 压入一个元素；写满后覆盖最旧元素。
    pub fn push(&mut self, value: T) {
        if self.capacity == 0 {
            return;
        }
        self.data[self.write_pos] = value;
        self.write_pos = (self.write_pos + 1) % self.capacity;
        self.samples_written += 1;
    }

    /// 逐元素压入切片。
    pub fn push_slice(&mut self, slice: &[T]) {
        for &v in slice {
            self.push(v);
        }
    }

    /// 取最近 `n` 个元素（旧→新保序）；实际返回 `min(n, len())` 个。
    pub fn get_recent(&self, n: usize) -> Vec<T> {
        let n = core::cmp::min(n, self.len());
        let mut out = Vec::with_capacity(n);
        if self.capacity == 0 {
            return out;
        }
        let start = (self.write_pos + self.capacity - n) % self.capacity;
        for i in 0..n {
            out.push(self.data[(start + i) % self.capacity]);
        }
        out
    }

    /// 当前有效元素数（`min(samples_written, capacity)`）。
    pub fn len(&self) -> usize {
        core::cmp::min(self.samples_written as usize, self.capacity)
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 缓冲容量。
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::RingSampleBuffer;

    #[test]
    fn rb1_push_and_get_recent_keep_time_order() {
        let mut rb = RingSampleBuffer::new(8);
        rb.push(10);
        rb.push(20);
        rb.push(30);
        assert_eq!(rb.get_recent(3), vec![10, 20, 30]);
        assert_eq!(rb.get_recent(2), vec![20, 30]);
    }

    #[test]
    fn rb2_overflow_overwrites_oldest_and_keeps_order() {
        let mut rb = RingSampleBuffer::new(4);
        for i in 1..=6 {
            rb.push(i);
        }
        assert_eq!(rb.get_recent(4), vec![3, 4, 5, 6]);
    }

    #[test]
    fn rb3_get_recent_more_than_written_returns_written() {
        let mut rb = RingSampleBuffer::new(10);
        rb.push(1);
        rb.push(2);
        rb.push(3);
        let got = rb.get_recent(10);
        assert_eq!(got, vec![1, 2, 3]);
        assert_eq!(rb.len(), 3);
    }

    #[test]
    fn rb4_capacity_one_keeps_only_latest() {
        let mut rb = RingSampleBuffer::new(1);
        rb.push(7);
        assert_eq!(rb.get_recent(1), vec![7]);
        rb.push(9);
        assert_eq!(rb.get_recent(1), vec![9]);
        assert_eq!(rb.len(), 1);
    }

    #[test]
    fn rb5_push_slice_writes_in_order() {
        let mut rb = RingSampleBuffer::new(4);
        rb.push_slice(&[1, 2, 3]);
        assert_eq!(rb.get_recent(3), vec![1, 2, 3]);
        rb.push_slice(&[4, 5]);
        assert_eq!(rb.get_recent(4), vec![2, 3, 4, 5]);
    }

    #[test]
    fn rb6_len_and_capacity_accessors() {
        let mut rb = RingSampleBuffer::<u32>::new(3);
        assert_eq!(rb.capacity(), 3);
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());
        rb.push(1);
        rb.push(2);
        rb.push(3);
        rb.push(4);
        assert_eq!(rb.len(), 3);
        assert!(!rb.is_empty());
    }
}
