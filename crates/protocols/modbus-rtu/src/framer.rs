//! Modbus RTU 3.5 字符间隔分帧器（v0.218.0，B2-01）。
//!
//! RTU 协议以帧间静默间隔 ≥ 3.5 字符时间作为帧边界（无显式长度字段）。
//! [`RtuFramer`] 从字节流中按间隔分帧：帧内任意两字节间隔必须 < 3.5
//! 字符时间；帧间间隔 ≥ 3.5 字符时间即帧结束。
//!
//! 使用方式：UART RX 中断逐字节喂 `feed_byte(byte, now_ns)`；当检测到
//! 帧结束（新帧首字节到达时上一帧已超时，或显式 `flush`）返回完整帧。
//!
//! 字符时间 = 11 bit / baud（RTU 一字符 = 1 起始位 + 8 数据位 + 1 校验
//! + 1 停止位）；3.5 字符时间 = 38.5 bit / baud。

/// 默认波特率（用于字符时间计算）。
pub const DEFAULT_BAUD: u32 = 9600;

/// 3.5 字符时间（纳秒）= 38.5 bit / baud * 1e9。
pub fn char_time_35_ns(baud: u32) -> u64 {
    if baud == 0 {
        return u64::MAX;
    }
    // 38.5 bit / baud 秒 → 纳秒；取整并加 1 保证严格大于 3.5 字符。
    (38_500_000_000u64 / baud as u64) + 1
}

/// RTU 3.5 字符间隔分帧器。
///
/// no_std：固定容量缓冲（RTU 帧上限 256 字节）。
pub struct RtuFramer {
    /// 帧缓冲。
    buf: [u8; 256],
    /// 缓冲长度。
    len: usize,
    /// 波特率（字符时间基准）。
    baud: u32,
    /// 上一字节到达时间（纳秒）。
    last_byte_ns: u64,
    /// 是否已有数据。
    has_data: bool,
    /// 成帧数。
    frames_ok: u64,
    /// 溢出（超长）丢弃数。
    frames_overflow: u64,
}

impl RtuFramer {
    /// 创建分帧器。
    pub const fn new(baud: u32) -> Self {
        Self {
            buf: [0; 256],
            len: 0,
            baud,
            last_byte_ns: 0,
            has_data: false,
            frames_ok: 0,
            frames_overflow: 0,
        }
    }

    /// 喂入一个字节；返回 `Ok(Some(frame))` 表示收到一个完整帧（前帧
    /// 因间隔超时结束），`Ok(None)` 表示仍在帧内，`Err(())` 表示溢出。
    ///
    /// `now_ns` 为当前时间戳（单调时钟）。
    #[allow(clippy::result_unit_err)] // Err(()) = 溢出丢弃信号，无附加错误信息
    pub fn feed_byte(&mut self, byte: u8, now_ns: u64) -> Result<Option<&[u8]>, ()> {
        let gap = if self.has_data {
            now_ns.saturating_sub(self.last_byte_ns)
        } else {
            0
        };
        self.last_byte_ns = now_ns;

        if self.has_data && gap >= char_time_35_ns(self.baud) {
            // 间隔 ≥ 3.5 字符时间：上一帧结束。
            let frame_len = self.len;
            self.len = 0;
            self.has_data = false;
            // 新字节作为下一帧首字节。
            self.buf[0] = byte;
            self.len = 1;
            self.has_data = true;
            self.frames_ok += 1;
            return Ok(Some(&self.buf[..frame_len]));
        }

        // 帧内字节。
        if self.len >= self.buf.len() {
            // 超长帧：丢弃并从当前字节重新开始。
            self.frames_overflow += 1;
            self.len = 1;
            self.buf[0] = byte;
            self.has_data = true;
            return Err(());
        }
        self.buf[self.len] = byte;
        self.len += 1;
        self.has_data = true;
        Ok(None)
    }

    /// 强制结束当前帧（串口空闲超时/轮询周期结束）。
    ///
    /// 返回当前帧内容（若有）。
    pub fn flush(&mut self) -> Option<&[u8]> {
        if !self.has_data {
            return None;
        }
        let frame_len = self.len;
        self.len = 0;
        self.has_data = false;
        self.frames_ok += 1;
        Some(&self.buf[..frame_len])
    }

    /// 统计：成帧数 / 溢出丢弃数。
    pub fn stats(&self) -> (u64, u64) {
        (self.frames_ok, self.frames_overflow)
    }

    /// 复位。
    pub fn reset(&mut self) {
        self.len = 0;
        self.has_data = false;
        self.frames_ok = 0;
        self.frames_overflow = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_char_time_35_ns_9600() {
        // 9600 baud：38.5 bit / 9600 = 4.0104 ms。
        let t = char_time_35_ns(9600);
        assert!((4_010_000..=4_020_000).contains(&t), "{}", t);
    }

    #[test]
    fn test_char_time_35_ns_higher_baud_shorter() {
        let t9600 = char_time_35_ns(9600);
        let t115200 = char_time_35_ns(115_200);
        assert!(t115200 < t9600);
    }

    #[test]
    fn test_char_time_35_ns_zero_baud() {
        assert_eq!(char_time_35_ns(0), u64::MAX);
    }

    #[test]
    fn test_frames_separated_by_35_char_gap() {
        let mut f = RtuFramer::new(9600);
        let gap = char_time_35_ns(9600);
        let mut t = 0u64;

        // 帧 1：0x01 0x03 0x00 CRC 等（帧内间隔 < gap）。
        let frame1 = [0x01u8, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A];
        for b in frame1 {
            assert!(f.feed_byte(b, t).unwrap().is_none());
            t += 100_000; // 100 us 帧内间隔
        }
        // 帧 2 首字节在 ≥ 3.5 字符时间后到达 → 返回帧 1。
        t += gap;
        let got = f.feed_byte(0x01, t).unwrap();
        assert_eq!(got.unwrap(), &frame1);
        // 帧 2 其余字节（首字节已作为新帧开头喂入）。
        let frame2 = [0x01u8, 0x06, 0x00, 0x01, 0x00, 0x02, 0x18, 0x0B];
        for b in &frame2[1..] {
            assert!(f.feed_byte(*b, t).unwrap().is_none());
            t += 100_000;
        }
        let got2 = f.flush().unwrap();
        assert_eq!(got2, &frame2);
        let (ok, _overflow) = f.stats();
        assert_eq!(ok, 2);
    }

    #[test]
    fn test_frame_internal_gap_keeps_frame() {
        let mut f = RtuFramer::new(9600);
        let mut t = 0u64;
        // 帧内间隔 < 3.5 字符时间：不切帧。
        for b in [0x01u8, 0x03, 0x00, 0x00] {
            assert!(f.feed_byte(b, t).unwrap().is_none());
            t += 500_000; // 500 us < 4 ms
        }
        // flush 取出整帧。
        let frame = f.flush().unwrap();
        assert_eq!(frame, &[0x01, 0x03, 0x00, 0x00]);
    }

    #[test]
    fn test_flush_empty_returns_none() {
        let mut f = RtuFramer::new(9600);
        assert!(f.flush().is_none());
    }

    #[test]
    fn test_overflow_discards_and_restarts() {
        let mut f = RtuFramer::new(9600);
        let mut t = 0u64;
        // 超过 256 字节 → 溢出。
        for i in 0..300 {
            let r = f.feed_byte(i as u8, t);
            t += 100_000;
            if i < 256 {
                assert!(r.is_ok());
            }
            // 溢出后从第 257 字节重新开始缓冲（不 panic）。
        }
        let (ok, overflow) = f.stats();
        assert_eq!(ok, 0);
        assert!(overflow >= 1, "overflow {overflow}");
    }
}
