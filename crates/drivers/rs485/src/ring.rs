//! no_std 环形缓冲（D4 偏差）.
//!
//! 蓝图假设 `RingBuffer<u8, 512>` 来自某外部库，但 no_std 环境下无该类型。
//! 本模块实现 const generics 环形缓冲 `RingBuffer<T, const N: usize>`，
//! 无外部依赖，使用固定大小数组 + 读/写指针。

use core::mem::MaybeUninit;

/// 固定容量环形缓冲
///
/// 使用 const generics 指定容量，内部用 `[MaybeUninit<T>; N]` 存储。
/// 支持单生产者-单消费者（SPSC）场景，用于 RS485 接收缓冲。
///
/// # 示例
/// ```
/// use eneros_rs485::RingBuffer;
/// let mut buf: RingBuffer<u8, 4> = RingBuffer::new();
/// assert!(buf.is_empty());
/// buf.push(0x42).ok();
/// assert_eq!(buf.pop(), Some(0x42));
/// ```
pub struct RingBuffer<T, const N: usize> {
    /// 存储槽
    buffer: [MaybeUninit<T>; N],
    /// 读指针（下一个弹出位置）
    read: usize,
    /// 写指针（下一个推入位置）
    write: usize,
    /// 当前元素数量
    count: usize,
}

impl<T, const N: usize> RingBuffer<T, N> {
    /// 创建空缓冲
    pub const fn new() -> Self {
        Self {
            buffer: [const { MaybeUninit::uninit() }; N],
            read: 0,
            write: 0,
            count: 0,
        }
    }

    /// 推入一个元素
    ///
    /// # 返回
    /// - `Ok(())`: 推入成功
    /// - `Err(item)`: 缓冲已满，返回被拒绝的元素
    pub fn push(&mut self, item: T) -> Result<(), T> {
        if self.is_full() {
            return Err(item);
        }
        self.buffer[self.write].write(item);
        self.write = (self.write + 1) % N;
        self.count += 1;
        Ok(())
    }

    /// 弹出一个元素
    ///
    /// # 返回
    /// - `Some(T)`: 弹出成功
    /// - `None`: 缓冲为空
    pub fn pop(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        // SAFETY: `read` 指向的位置此前已通过 `push` 写入有效值，
        // 且尚未被 `pop` 读取过（`count > 0` 保证）。读取后该槽位视为未初始化。
        let item = unsafe { self.buffer[self.read].assume_init_read() };
        self.read = (self.read + 1) % N;
        self.count -= 1;
        Some(item)
    }

    /// 当前元素数量
    pub fn len(&self) -> usize {
        self.count
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// 是否已满
    pub fn is_full(&self) -> bool {
        self.count == N
    }

    /// 容量
    pub const fn capacity(&self) -> usize {
        N
    }

    /// 清空缓冲
    pub fn clear(&mut self) {
        self.read = 0;
        self.write = 0;
        self.count = 0;
    }
}

impl<T, const N: usize> Default for RingBuffer<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_empty() {
        let buf: RingBuffer<u8, 4> = RingBuffer::new();
        assert!(buf.is_empty());
        assert!(!buf.is_full());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), 4);
    }

    #[test]
    fn test_push_pop_single() {
        let mut buf: RingBuffer<u8, 4> = RingBuffer::new();
        assert!(buf.push(0x42).is_ok());
        assert!(!buf.is_empty());
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.pop(), Some(0x42));
        assert!(buf.is_empty());
    }

    #[test]
    fn test_push_until_full() {
        let mut buf: RingBuffer<u8, 4> = RingBuffer::new();
        for i in 0..4u8 {
            assert!(buf.push(i).is_ok());
        }
        assert!(buf.is_full());
        assert_eq!(buf.len(), 4);
        // 第 5 个推入应失败
        let result = buf.push(0xFF);
        assert_eq!(result, Err(0xFF));
    }

    #[test]
    fn test_pop_from_empty() {
        let mut buf: RingBuffer<u8, 4> = RingBuffer::new();
        assert_eq!(buf.pop(), None);
    }

    #[test]
    fn test_wraparound() {
        let mut buf: RingBuffer<u8, 3> = RingBuffer::new();
        // 推入 3 个（满）
        buf.push(10).ok();
        buf.push(20).ok();
        buf.push(30).ok();
        // 弹出 2 个
        assert_eq!(buf.pop(), Some(10));
        assert_eq!(buf.pop(), Some(20));
        // 再推入 2 个（触发环绕）
        buf.push(40).ok();
        buf.push(50).ok();
        assert!(buf.is_full());
        // 弹出全部，验证顺序正确
        assert_eq!(buf.pop(), Some(30));
        assert_eq!(buf.pop(), Some(40));
        assert_eq!(buf.pop(), Some(50));
        assert!(buf.is_empty());
    }

    #[test]
    fn test_clear() {
        let mut buf: RingBuffer<u8, 4> = RingBuffer::new();
        buf.push(1).ok();
        buf.push(2).ok();
        assert_eq!(buf.len(), 2);
        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.pop(), None);
    }

    #[test]
    fn test_fifo_order() {
        let mut buf: RingBuffer<u8, 8> = RingBuffer::new();
        for i in 0..8u8 {
            buf.push(i).ok();
        }
        for i in 0..8u8 {
            assert_eq!(buf.pop(), Some(i));
        }
    }

    #[test]
    fn test_interleaved_push_pop() {
        let mut buf: RingBuffer<u8, 4> = RingBuffer::new();
        buf.push(1).ok();
        buf.push(2).ok();
        assert_eq!(buf.pop(), Some(1));
        buf.push(3).ok();
        assert_eq!(buf.pop(), Some(2));
        assert_eq!(buf.pop(), Some(3));
        assert!(buf.is_empty());
    }

    #[test]
    fn test_default() {
        let buf: RingBuffer<u8, 4> = RingBuffer::default();
        assert!(buf.is_empty());
        assert_eq!(buf.capacity(), 4);
    }

    #[test]
    fn test_capacity_zero() {
        // 容量为 0 的缓冲始终为空且已满
        let mut buf: RingBuffer<u8, 0> = RingBuffer::new();
        assert!(buf.is_empty());
        assert!(buf.is_full());
        assert_eq!(buf.push(1), Err(1));
        assert_eq!(buf.pop(), None);
    }
}
