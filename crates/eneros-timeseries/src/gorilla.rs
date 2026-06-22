//! Gorilla 时序数据压缩编码（T029-13）
//!
//! 实现 Facebook Gorilla 论文的压缩算法，用于时序数据（时间戳 + 浮点值 +
//! 质量码）的紧凑存储。
//!
//! # 算法概述
//!
//! Gorilla 压缩针对时序数据的两个维度：
//!
//! 1. **时间戳压缩**：delta-of-delta 编码
//!    - 计算连续时间戳的差值（delta）
//!    - 再计算 delta 的差值（delta-of-delta）
//!    - 大多数 delta-of-delta 为 0（固定间隔采样），用 1 bit 编码
//!
//! 2. **浮点值压缩**：XOR 编码
//!    - 计算连续值的 XOR
//!    - 如果 XOR 为 0（值不变），用 1 bit 编码
//!    - 否则用前导零 + 有效位 + 尾随零编码，复用上次的零计数以节省比特
//!
//! 3. **质量码压缩**：2 bits/点（Good=0, Uncertain=1, Bad=2）
//!
//! # 编码格式
//!
//! 编码后的字节流以 4 字节小端序计数头开始（数据点数），后跟位级编码的
//! 数据流。计数头使解码器能精确知道何时停止，避免将末字节的填充比特
//! 误读为有效数据。
//!
//! # 参考
//!
//! T. Pelkonen et al., "Gorilla: A Fast, Scalable, In-Memory Time Series
//! Database", VLDB 2015.

use crate::engine::DataQuality;

// =====================================================================
// BitWriter / BitReader — 位级读写
// =====================================================================

/// 位写入器：按位（MSB 优先）写入字节缓冲区
pub struct BitWriter {
    buffer: Vec<u8>,
    current_byte: u8,
    /// 当前字节已写入的位数（0..8）
    bit_pos: u8,
}

impl BitWriter {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            current_byte: 0,
            bit_pos: 0,
        }
    }

    /// 写入单个比特
    #[inline]
    pub fn write_bit(&mut self, bit: bool) {
        if bit {
            // MSB 优先：第一个比特写入最高位
            self.current_byte |= 1 << (7 - self.bit_pos);
        }
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.buffer.push(self.current_byte);
            self.current_byte = 0;
            self.bit_pos = 0;
        }
    }

    /// 写入 `nbits` 位（从 value 的最高位开始输出）
    #[inline]
    pub fn write_bits(&mut self, value: u64, nbits: u8) {
        debug_assert!(nbits <= 64, "nbits 超过 64");
        if nbits == 0 {
            return;
        }
        let mut remaining = nbits;
        // 从最高位开始输出，每次写入 min(remaining, 当前字节剩余位) 位
        while remaining > 0 {
            let available = 8 - self.bit_pos;
            let to_write = remaining.min(available);
            let shift = remaining - to_write;
            // 取 value 的高 to_write 位
            // 使用 u16 避免 to_write=8 时 1u8<<8 溢出
            let bit_mask = ((1u16 << to_write) - 1) as u8;
            let bits = ((value >> shift) as u8) & bit_mask;
            // MSB 优先：写入当前字节的高位
            self.current_byte |= bits << (available - to_write);
            self.bit_pos += to_write;
            remaining -= to_write;
            if self.bit_pos == 8 {
                self.buffer.push(self.current_byte);
                self.current_byte = 0;
                self.bit_pos = 0;
            }
        }
    }

    /// 完成写入，返回字节缓冲区（最后一字节不足 8 位时补零）
    pub fn finish(mut self) -> Vec<u8> {
        if self.bit_pos > 0 {
            self.buffer.push(self.current_byte);
        }
        self.buffer
    }

    /// 当前已写入的字节数（含未满的当前字节）
    pub fn byte_len(&self) -> usize {
        self.buffer.len() + if self.bit_pos > 0 { 1 } else { 0 }
    }
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// 位读取器：按位（MSB 优先）从字节缓冲区读取
pub struct BitReader<'a> {
    buffer: &'a [u8],
    byte_pos: usize,
    /// 当前字节已读取的位数（0..8）
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    pub fn new(buffer: &'a [u8]) -> Self {
        Self {
            buffer,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    /// 读取单个比特，返回 `None` 表示数据已耗尽
    #[inline]
    pub fn read_bit(&mut self) -> Option<bool> {
        if self.byte_pos >= self.buffer.len() {
            return None;
        }
        let bit = (self.buffer[self.byte_pos] >> (7 - self.bit_pos)) & 1 == 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.byte_pos += 1;
            self.bit_pos = 0;
        }
        Some(bit)
    }

    /// 读取 `nbits` 位（MSB 优先），返回 `None` 表示数据不足
    #[inline]
    pub fn read_bits(&mut self, nbits: u8) -> Option<u64> {
        debug_assert!(nbits <= 64, "nbits 超过 64");
        if nbits == 0 {
            return Some(0);
        }
        let mut value: u64 = 0;
        let mut remaining = nbits;
        // 每次读取 min(remaining, 当前字节剩余位) 位
        while remaining > 0 {
            if self.byte_pos >= self.buffer.len() {
                return None;
            }
            let available = 8 - self.bit_pos;
            let to_read = remaining.min(available);
            let shift = available - to_read;
            // 从当前字节提取高 to_read 位（MSB 优先）
            // 使用 u16 避免 to_read=8 时 1u8<<8 溢出
            let bit_mask = ((1u16 << to_read) - 1) as u8;
            let bits = ((self.buffer[self.byte_pos] >> shift) & bit_mask) as u64;
            value = (value << to_read) | bits;
            self.bit_pos += to_read;
            remaining -= to_read;
            if self.bit_pos == 8 {
                self.byte_pos += 1;
                self.bit_pos = 0;
            }
        }
        Some(value)
    }
}

// =====================================================================
// 质量码编解码
// =====================================================================

/// 将质量码编码为 2 bits
#[inline]
fn quality_to_bits(q: &DataQuality) -> u8 {
    match q {
        DataQuality::Good => 0,
        DataQuality::Uncertain => 1,
        DataQuality::Bad => 2,
    }
}

/// 从 2 bits 解码质量码（3 = 保留，按 Good 处理）
#[inline]
fn bits_to_quality(bits: u8) -> DataQuality {
    match bits {
        1 => DataQuality::Uncertain,
        2 => DataQuality::Bad,
        _ => DataQuality::Good,
    }
}

// =====================================================================
// GorillaEncoder — Gorilla 压缩编码器
// =====================================================================

/// Gorilla 压缩编码器
///
/// 对 (timestamp, value, quality) 三元组序列进行增量编码：
/// - 第一个点：完整写入 64 bits 时间戳 + 64 bits 值 + 2 bits 质量
/// - 后续点：delta-of-delta 时间戳 + XOR 值 + 2 bits 质量
///
/// 编码完成后调用 [`GorillaEncoder::finish`] 获取压缩字节流。
/// 字节流以 4 字节小端序计数头开始，后跟位级编码数据。
pub struct GorillaEncoder {
    bit_writer: BitWriter,
    last_timestamp: i64,
    last_delta: i64,
    last_value: f64,
    last_leading_zeros: u8,
    last_trailing_zeros: u8,
    count: u32,
    initialized: bool,
}

impl GorillaEncoder {
    pub fn new() -> Self {
        Self {
            bit_writer: BitWriter::new(),
            last_timestamp: 0,
            last_delta: 0,
            last_value: 0.0,
            // 初始化为 64 表示"尚未设置"（首次 XOR 必走新前导零路径）
            last_leading_zeros: 64,
            last_trailing_zeros: 0,
            count: 0,
            initialized: false,
        }
    }

    /// 编码一个数据点
    pub fn encode(&mut self, timestamp: i64, value: f64, quality: &DataQuality) {
        if !self.initialized {
            // 第一个点：完整写入时间戳、值和质量码
            self.bit_writer.write_bits(timestamp as u64, 64);
            self.bit_writer.write_bits(value.to_bits(), 64);
            self.bit_writer.write_bits(quality_to_bits(quality) as u64, 2);
            self.last_timestamp = timestamp;
            self.last_value = value;
            self.initialized = true;
        } else {
            // 时间戳 delta-of-delta 编码
            let delta = timestamp - self.last_timestamp;
            let dod = delta - self.last_delta;
            self.encode_timestamp_dod(dod);
            self.last_delta = delta;
            self.last_timestamp = timestamp;

            // 值 XOR 编码
            self.encode_value_xor(value);
            self.last_value = value;

            // 质量码：2 bits
            self.bit_writer
                .write_bits(quality_to_bits(quality) as u64, 2);
        }
        self.count += 1;
    }

    /// 时间戳 delta-of-delta 编码
    ///
    /// 编码规则（前缀码）：
    /// - `== 0`: `0`（1 bit）
    /// - `[-63, 64]`: `10` + 7 bits（偏移 +63）
    /// - `[-255, 256]`: `110` + 9 bits（偏移 +255）
    /// - `[-2047, 2048]`: `1110` + 12 bits（偏移 +2047）
    /// - 其他: `1111` + 32 bits（i32 二进制补码）
    fn encode_timestamp_dod(&mut self, dod: i64) {
        if dod == 0 {
            self.bit_writer.write_bit(false);
        } else if (-63..=64).contains(&dod) {
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bit(false);
            self.bit_writer.write_bits((dod + 63) as u64, 7);
        } else if (-255..=256).contains(&dod) {
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bit(false);
            self.bit_writer.write_bits((dod + 255) as u64, 9);
        } else if (-2047..=2048).contains(&dod) {
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bit(false);
            self.bit_writer.write_bits((dod + 2047) as u64, 12);
        } else {
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bit(true);
            // i32 二进制补码 → u32 → 32 bits
            self.bit_writer
                .write_bits(dod as i32 as u32 as u64, 32);
        }
    }

    /// 浮点值 XOR 编码
    ///
    /// 编码规则：
    /// - XOR == 0: `0`（1 bit，值不变）
    /// - XOR != 0:
    ///   - `1` 标记值变化
    ///   - 若前导零 >= last_leading_zeros 且尾随零 >= last_trailing_zeros:
    ///     `0` + 有效位（复用上次的零计数）
    ///   - 否则: `1` + 6 bits 前导零 + 6 bits (有效位数-1) + 有效位
    fn encode_value_xor(&mut self, value: f64) {
        let xor = value.to_bits() ^ self.last_value.to_bits();

        if xor == 0 {
            // 值相同：1 bit '0'
            self.bit_writer.write_bit(false);
            return;
        }

        // 值不同：1 bit '1'
        self.bit_writer.write_bit(true);

        let leading = xor.leading_zeros() as u8;
        let trailing = xor.trailing_zeros() as u8;
        // xor != 0 保证 leading + trailing <= 64，有效位 >= 1

        // 检查是否可复用上次的前导零/尾随零
        // last_leading_zeros == 64 表示首次（尚未设置），必走"新值"路径
        if leading >= self.last_leading_zeros && trailing >= self.last_trailing_zeros {
            // 复用：1 bit '0' + 有效位
            self.bit_writer.write_bit(false);
            let meaningful_bits = 64 - self.last_leading_zeros - self.last_trailing_zeros;
            let meaningful = xor >> self.last_trailing_zeros;
            self.bit_writer.write_bits(meaningful, meaningful_bits);
        } else {
            // 新值：1 bit '1' + 6 bits 前导零 + 6 bits (有效位数-1) + 有效位
            self.bit_writer.write_bit(true);
            self.bit_writer.write_bits(leading as u64, 6);
            let meaningful_bits = 64 - leading - trailing;
            // 有效位数范围 1..=64，存储为 (meaningful_bits - 1) ∈ 0..=63
            self.bit_writer
                .write_bits((meaningful_bits - 1) as u64, 6);
            let meaningful = xor >> trailing;
            self.bit_writer.write_bits(meaningful, meaningful_bits);
            self.last_leading_zeros = leading;
            self.last_trailing_zeros = trailing;
        }
    }

    /// 完成编码，返回压缩字节流
    ///
    /// 字节流格式：4 字节小端序计数头 + 位级编码数据
    pub fn finish(self) -> Vec<u8> {
        let mut result = Vec::with_capacity(4 + self.bit_writer.byte_len());
        // 4 字节小端序计数头
        result.extend_from_slice(&self.count.to_le_bytes());
        result.extend(self.bit_writer.finish());
        result
    }

    /// 已编码的数据点数
    pub fn count(&self) -> u32 {
        self.count
    }
}

impl Default for GorillaEncoder {
    fn default() -> Self {
        Self::new()
    }
}

// =====================================================================
// GorillaDecoder — Gorilla 压缩解码器
// =====================================================================

/// Gorilla 压缩解码器
///
/// 从压缩字节流中逐点解码 (timestamp, value, quality) 三元组。
/// 字节流以 4 字节计数头开始，解码器在解码完指定数量的点后返回 `None`。
pub struct GorillaDecoder<'a> {
    bit_reader: BitReader<'a>,
    last_timestamp: i64,
    last_delta: i64,
    last_value: f64,
    last_leading_zeros: u8,
    last_trailing_zeros: u8,
    /// 总点数（从计数头读取）
    total_count: u32,
    /// 已解码的点数
    decoded_count: u32,
    initialized: bool,
}

impl<'a> GorillaDecoder<'a> {
    /// 从压缩字节流创建解码器
    ///
    /// 自动读取前 4 字节的计数头，确定要解码的点数。
    /// 如果缓冲区不足 4 字节，视为空序列。
    pub fn new(buffer: &'a [u8]) -> Self {
        let (total_count, data) = if buffer.len() >= 4 {
            let count = u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
            (count, &buffer[4..])
        } else {
            (0, buffer)
        };
        Self {
            bit_reader: BitReader::new(data),
            last_timestamp: 0,
            last_delta: 0,
            last_value: 0.0,
            // 与编码器保持一致：64 表示"尚未设置"，首个值走新值路径时会正确设置
            last_leading_zeros: 64,
            last_trailing_zeros: 0,
            total_count,
            decoded_count: 0,
            initialized: false,
        }
    }

    /// 解码下一个数据点，返回 `None` 表示数据已耗尽或已解码完全部点
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<(i64, f64, DataQuality)> {
        if self.decoded_count >= self.total_count {
            return None;
        }

        if !self.initialized {
            // 读取第一个点：64 bits 时间戳 + 64 bits 值 + 2 bits 质量
            let ts = self.bit_reader.read_bits(64)? as i64;
            let val_bits = self.bit_reader.read_bits(64)?;
            let q_bits = self.bit_reader.read_bits(2)? as u8;
            let val = f64::from_bits(val_bits);
            self.last_timestamp = ts;
            self.last_value = val;
            self.initialized = true;
            self.decoded_count += 1;
            return Some((ts, val, bits_to_quality(q_bits)));
        }

        // 解码时间戳
        let timestamp = self.decode_timestamp_dod()?;
        // 解码值
        let value = self.decode_value_xor()?;
        // 解码质量码
        let q_bits = self.bit_reader.read_bits(2)? as u8;

        self.last_timestamp = timestamp;
        self.last_value = value;
        self.decoded_count += 1;
        Some((timestamp, value, bits_to_quality(q_bits)))
    }

    /// 解码时间戳 delta-of-delta
    ///
    /// 注意：必须更新 `self.last_delta`，与编码器保持对称。
    fn decode_timestamp_dod(&mut self) -> Option<i64> {
        let b = self.bit_reader.read_bit()?;
        if !b {
            // dod == 0，delta 不变
            let ts = self.last_timestamp + self.last_delta;
            // last_delta 保持不变
            return Some(ts);
        }
        let b = self.bit_reader.read_bit()?;
        let dod = if !b {
            // '10' + 7 bits
            let val = self.bit_reader.read_bits(7)? as i64;
            val - 63
        } else {
            let b = self.bit_reader.read_bit()?;
            if !b {
                // '110' + 9 bits
                let val = self.bit_reader.read_bits(9)? as i64;
                val - 255
            } else {
                let b = self.bit_reader.read_bit()?;
                if !b {
                    // '1110' + 12 bits
                    let val = self.bit_reader.read_bits(12)? as i64;
                    val - 2047
                } else {
                    // '1111' + 32 bits（i32 二进制补码）
                    self.bit_reader.read_bits(32)? as u32 as i32 as i64
                }
            }
        };
        // 更新 last_delta（与编码器对称）
        self.last_delta += dod;
        Some(self.last_timestamp + self.last_delta)
    }

    /// 解码浮点值 XOR
    fn decode_value_xor(&mut self) -> Option<f64> {
        let b = self.bit_reader.read_bit()?;
        if !b {
            // 值不变
            return Some(self.last_value);
        }

        // 值变化
        let reuse = self.bit_reader.read_bit()?;
        let (trailing, meaningful_bits) = if !reuse {
            // 复用上次的前导零/尾随零
            (
                self.last_trailing_zeros,
                64 - self.last_leading_zeros - self.last_trailing_zeros,
            )
        } else {
            // 读取新的前导零和有效位数
            let leading = self.bit_reader.read_bits(6)? as u8;
            let meaningful_bits = self.bit_reader.read_bits(6)? as u8 + 1;
            let trailing = 64 - leading - meaningful_bits;
            self.last_leading_zeros = leading;
            self.last_trailing_zeros = trailing;
            (trailing, meaningful_bits)
        };

        let meaningful = self.bit_reader.read_bits(meaningful_bits)?;
        let xor = meaningful << trailing;
        let val = f64::from_bits(self.last_value.to_bits() ^ xor);
        Some(val)
    }

    /// 已解码的数据点数
    pub fn count(&self) -> u32 {
        self.decoded_count
    }

    /// 总数据点数（从计数头读取）
    pub fn total_count(&self) -> u32 {
        self.total_count
    }
}

// =====================================================================
// 便捷函数：一次性编码/解码
// =====================================================================

/// 将 (timestamp, value, quality) 序列编码为 Gorilla 压缩字节流
pub fn encode_series(points: &[(i64, f64, DataQuality)]) -> Vec<u8> {
    let mut enc = GorillaEncoder::new();
    for (ts, val, q) in points {
        enc.encode(*ts, *val, q);
    }
    enc.finish()
}

/// 从 Gorilla 压缩字节流解码出 (timestamp, value, quality) 序列
pub fn decode_series(buffer: &[u8]) -> Vec<(i64, f64, DataQuality)> {
    let mut dec = GorillaDecoder::new(buffer);
    let mut result = Vec::with_capacity(dec.total_count() as usize);
    while let Some(point) = dec.next() {
        result.push(point);
    }
    result
}

// =====================================================================
// 单元测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DataQuality;

    // --- BitWriter / BitReader ---

    #[test]
    fn test_bit_writer_reader_round_trip() {
        let mut writer = BitWriter::new();
        writer.write_bits(0b1010_1010, 8);
        writer.write_bits(0b1100, 4);
        writer.write_bit(true);
        writer.write_bit(false);
        writer.write_bits(0xDEAD_BEEF, 32);
        // 总共 8+4+1+1+32 = 46 bits → 6 bytes (末尾 2 bits 填充 0)
        let buf = writer.finish();
        assert_eq!(buf.len(), 6);

        let mut reader = BitReader::new(&buf);
        assert_eq!(reader.read_bits(8), Some(0b1010_1010));
        assert_eq!(reader.read_bits(4), Some(0b1100));
        assert_eq!(reader.read_bit(), Some(true));
        assert_eq!(reader.read_bit(), Some(false));
        assert_eq!(reader.read_bits(32), Some(0xDEAD_BEEF));
        // 46 bits 已读完，剩余 2 bits 为填充零
        assert_eq!(reader.read_bit(), Some(false));
        assert_eq!(reader.read_bit(), Some(false));
        // 缓冲区耗尽
        assert_eq!(reader.read_bit(), None);
    }

    #[test]
    fn test_bit_writer_pads_last_byte() {
        let mut writer = BitWriter::new();
        writer.write_bits(0b101, 3); // 3 bits → 1 byte (padded with 0s)
        let buf = writer.finish();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0], 0b1010_0000);
    }

    #[test]
    fn test_bit_reader_exhausted() {
        let buf = [0xFF];
        let mut reader = BitReader::new(&buf);
        for _ in 0..8 {
            assert_eq!(reader.read_bit(), Some(true));
        }
        assert_eq!(reader.read_bit(), None);
    }

    #[test]
    fn test_bit_writer_zero_bits() {
        let mut writer = BitWriter::new();
        writer.write_bits(0, 0);
        let buf = writer.finish();
        assert!(buf.is_empty());
    }

    // --- GorillaEncoder / GorillaDecoder ---

    fn make_quality(q: u8) -> DataQuality {
        match q % 3 {
            0 => DataQuality::Good,
            1 => DataQuality::Uncertain,
            _ => DataQuality::Bad,
        }
    }

    #[test]
    fn test_gorilla_single_point() {
        let points = vec![(1_700_000_000_000i64, 220.5, DataQuality::Good)];
        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].0, 1_700_000_000_000);
        assert_eq!(decoded[0].1, 220.5);
        assert_eq!(decoded[0].2, DataQuality::Good);
    }

    #[test]
    fn test_gorilla_constant_interval_constant_value() {
        // 固定 1s 间隔，值不变 → 最高压缩比
        let points: Vec<(i64, f64, DataQuality)> = (0..1000)
            .map(|i| (1_700_000_000_000 + i * 1000, 220.0, DataQuality::Good))
            .collect();

        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);

        assert_eq!(decoded.len(), points.len());
        for (i, (ts, val, q)) in decoded.iter().enumerate() {
            assert_eq!(*ts, points[i].0, "时间戳不匹配 @ {}", i);
            assert!((val - points[i].1).abs() < 1e-12, "值不匹配 @ {}", i);
            assert_eq!(*q, points[i].2, "质量码不匹配 @ {}", i);
        }

        // 压缩比：原始 1000 点 × (8+8+1) = 17000 字节
        let original_bytes = points.len() * (8 + 8 + 1);
        let ratio = original_bytes as f64 / encoded.len() as f64;
        eprintln!(
            "固定间隔固定值：原始 {} 字节 → 压缩 {} 字节，压缩比 {:.1}x",
            original_bytes,
            encoded.len(),
            ratio
        );
        // 固定间隔 + 固定值：每点约 4 bits（1 bit 时间戳 + 1 bit 值 + 2 bits 质量）
        assert!(ratio > 5.0, "压缩比应 > 5x，实际 {:.1}x", ratio);
    }

    #[test]
    fn test_gorilla_constant_interval_varying_value() {
        // 固定 1s 间隔，值缓慢变化（电力电压典型场景）
        let points: Vec<(i64, f64, DataQuality)> = (0..1000)
            .map(|i| {
                let v = 220.0 + (i as f64 * 0.001).sin() * 0.5;
                (1_700_000_000_000 + i * 1000, v, DataQuality::Good)
            })
            .collect();

        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);

        assert_eq!(decoded.len(), points.len());
        for (i, (ts, val, q)) in decoded.iter().enumerate() {
            assert_eq!(*ts, points[i].0, "时间戳不匹配 @ {}", i);
            assert!((val - points[i].1).abs() < 1e-12, "值不匹配 @ {}", i);
            assert_eq!(*q, points[i].2, "质量码不匹配 @ {}", i);
        }
    }

    #[test]
    fn test_gorilla_varying_interval_varying_value() {
        // 变化间隔，变化值（最差场景之一）
        let points: Vec<(i64, f64, DataQuality)> = (0..500)
            .map(|i| {
                let ts = 1_700_000_000_000 + i * (1000 + (i % 100));
                let v = 50.0 + (i as f64).sqrt() * 1.7;
                (ts, v, make_quality(i as u8))
            })
            .collect();

        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);

        assert_eq!(decoded.len(), points.len());
        for (i, (ts, val, q)) in decoded.iter().enumerate() {
            assert_eq!(*ts, points[i].0, "时间戳不匹配 @ {}", i);
            assert!((val - points[i].1).abs() < 1e-12, "值不匹配 @ {}", i);
            assert_eq!(*q, points[i].2, "质量码不匹配 @ {}", i);
        }
    }

    #[test]
    fn test_gorilla_timestamp_dod_all_buckets() {
        // 覆盖所有 delta-of-delta 编码桶
        let deltas = [
            0i64,         // dod == 0
            10,           // [-63, 64]
            -10,          // [-63, 64]
            63,           // [-63, 64] 边界
            -63,          // [-63, 64] 边界
            64,           // [-63, 64] 边界
            100,          // [-255, 256]
            -100,         // [-255, 256]
            256,          // [-255, 256] 边界
            -255,         // [-255, 256] 边界
            500,          // [-2047, 2048]
            -500,         // [-2047, 2048]
            2048,         // [-2047, 2048] 边界
            -2047,        // [-2047, 2048] 边界
            10000,        // 其他（32 bits）
            -10000,       // 其他（32 bits）
            1_000_000,    // 其他（32 bits）
            -1_000_000,   // 其他（32 bits）
        ];

        let mut ts = 1_700_000_000_000i64;
        let mut points = Vec::new();
        for &d in &deltas {
            ts += 1000 + d; // 基础间隔 1000ms + dod
            points.push((ts, 100.0, DataQuality::Good));
        }

        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);

        assert_eq!(decoded.len(), points.len());
        for (i, (ts, _val, _q)) in decoded.iter().enumerate() {
            assert_eq!(*ts, points[i].0, "时间戳不匹配 @ {}", i);
        }
    }

    #[test]
    fn test_gorilla_value_xor_all_cases() {
        // 覆盖 XOR 编码的所有路径
        let values = [
            220.0,                      // 基准值
            220.0,                      // XOR == 0（值不变）
            220.001,                    // 小变化（少有效位）
            220.001,                    // XOR == 0
            500.0,                      // 大变化（多有效位）
            500.0,                      // XOR == 0
            0.0,                        // 极大变化
            f64::INFINITY,              // 特殊值
            0.0,                        // 回到 0
            -100.5,                     // 负值
            f64::NAN,                   // NaN
            1.0,                        // 从 NaN 恢复
        ];

        let points: Vec<(i64, f64, DataQuality)> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| (1_700_000_000_000 + i as i64 * 1000, v, DataQuality::Good))
            .collect();

        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);

        assert_eq!(decoded.len(), points.len());
        for (i, (_ts, val, _q)) in decoded.iter().enumerate() {
            if points[i].1.is_nan() {
                assert!(val.is_nan(), "应为 NaN @ {}", i);
            } else if points[i].1.is_infinite() {
                // 无穷大不能用差值比较（inf - inf = NaN）
                assert_eq!(
                    val.to_bits(),
                    points[i].1.to_bits(),
                    "无穷大不匹配 @ {}",
                    i
                );
            } else {
                assert!(
                    (val - points[i].1).abs() < 1e-12,
                    "值不匹配 @ {}: 期望 {} 实际 {}",
                    i,
                    points[i].1,
                    val
                );
            }
        }
    }

    #[test]
    fn test_gorilla_quality_codes() {
        let points: Vec<(i64, f64, DataQuality)> = (0..300)
            .map(|i| {
                let q = match i % 3 {
                    0 => DataQuality::Good,
                    1 => DataQuality::Uncertain,
                    _ => DataQuality::Bad,
                };
                (1_700_000_000_000 + i * 1000, 100.0, q)
            })
            .collect();

        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);

        assert_eq!(decoded.len(), points.len());
        for (i, (_ts, _val, q)) in decoded.iter().enumerate() {
            assert_eq!(*q, points[i].2, "质量码不匹配 @ {}", i);
        }
    }

    #[test]
    fn test_gorilla_empty_series() {
        let points: Vec<(i64, f64, DataQuality)> = vec![];
        let encoded = encode_series(&points);
        // 空序列仍有 4 字节计数头
        assert_eq!(encoded.len(), 4);
        let decoded = decode_series(&encoded);
        assert!(decoded.is_empty());
    }

    #[test]
    fn test_gorilla_two_points() {
        let points = vec![
            (1000i64, 1.5, DataQuality::Good),
            (2000, 1.5, DataQuality::Good), // 值不变
        ];
        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].0, 1000);
        assert_eq!(decoded[1].0, 2000);
        assert_eq!(decoded[0].1, 1.5);
        assert_eq!(decoded[1].1, 1.5);
    }

    // --- 压缩比测试（真实电力数据模式） ---

    #[test]
    fn test_compression_ratio_power_voltage_data() {
        // 模拟真实电力电压数据：1s 采样，220V 附近小幅波动
        // 电力系统中电压变化缓慢，且量化到固定精度，多数相邻样本值相同
        let n = 10_000;
        let points: Vec<(i64, f64, DataQuality)> = (0..n)
            .map(|i| {
                let t = i as f64 * 0.001;
                // 电压在 220V 附近缓慢波动，量化到 0.01V
                let raw = 220.0 + (t * 0.5).sin() * 0.3 + (t * 2.0).sin() * 0.1;
                let v = (raw * 100.0).round() / 100.0;
                (1_700_000_000_000 + i as i64 * 1000, v, DataQuality::Good)
            })
            .collect();

        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);

        // 正确性
        assert_eq!(decoded.len(), n);
        for (i, (ts, val, _q)) in decoded.iter().enumerate() {
            assert_eq!(*ts, points[i].0);
            assert!((val - points[i].1).abs() < 1e-12);
        }

        // 压缩比
        let original_bytes = n * (8 + 8 + 1); // timestamp + value + quality
        let ratio = original_bytes as f64 / encoded.len() as f64;
        eprintln!(
            "电力电压数据 ({} 点)：原始 {} 字节 → 压缩 {} 字节，压缩比 {:.2}x",
            n,
            original_bytes,
            encoded.len(),
            ratio
        );
        assert!(ratio > 5.0, "压缩比应 > 5x，实际 {:.2}x", ratio);
    }

    #[test]
    fn test_compression_ratio_power_load_data() {
        // 模拟真实电力负荷数据：1min 采样，负荷非常缓慢变化
        // 负荷量化到 0.1MW，多数相邻样本值相同（XOR=0 → 1 bit）
        // 真实 SCADA 系统中，1 分钟间隔的负荷变化通常 < 0.1MW
        let n = 10_000;
        let points: Vec<(i64, f64, DataQuality)> = (0..n)
            .map(|i| {
                let t = i as f64;
                // 负荷曲线：基础 500MW，日周期缓慢波动（幅度 ±10MW）
                // 量化到 0.1MW，相邻样本变化 < 0.1MW → 多数 XOR=0
                let raw = 500.0 + (t * 2.0 * std::f64::consts::PI / 1440.0).sin() * 10.0;
                let v = (raw * 10.0).round() / 10.0;
                (1_700_000_000_000 + i as i64 * 60_000, v, DataQuality::Good)
            })
            .collect();

        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);

        assert_eq!(decoded.len(), n);
        for (i, (ts, val, _q)) in decoded.iter().enumerate() {
            assert_eq!(*ts, points[i].0);
            assert!((val - points[i].1).abs() < 1e-12);
        }

        let original_bytes = n * (8 + 8 + 1);
        let ratio = original_bytes as f64 / encoded.len() as f64;
        eprintln!(
            "电力负荷数据 ({} 点)：原始 {} 字节 → 压缩 {} 字节，压缩比 {:.2}x",
            n,
            original_bytes,
            encoded.len(),
            ratio
        );
        assert!(ratio > 5.0, "压缩比应 > 5x，实际 {:.2}x", ratio);
    }

    #[test]
    fn test_compression_ratio_mixed_quality() {
        // 混合质量码的数据，值缓慢变化
        let n = 5_000;
        let points: Vec<(i64, f64, DataQuality)> = (0..n)
            .map(|i| {
                let q = if i % 100 == 0 {
                    DataQuality::Uncertain
                } else if i % 500 == 0 {
                    DataQuality::Bad
                } else {
                    DataQuality::Good
                };
                // 值量化到 0.1，多数相邻样本相同
                let raw = 100.0 + i as f64 * 0.01;
                let v = (raw * 10.0).round() / 10.0;
                (1_700_000_000_000 + i as i64 * 1000, v, q)
            })
            .collect();

        let encoded = encode_series(&points);
        let decoded = decode_series(&encoded);

        assert_eq!(decoded.len(), n);
        for (i, (ts, val, q)) in decoded.iter().enumerate() {
            assert_eq!(*ts, points[i].0);
            assert!((val - points[i].1).abs() < 1e-12);
            assert_eq!(*q, points[i].2);
        }

        let original_bytes = n * (8 + 8 + 1);
        let ratio = original_bytes as f64 / encoded.len() as f64;
        eprintln!(
            "混合质量码数据 ({} 点)：原始 {} 字节 → 压缩 {} 字节，压缩比 {:.2}x",
            n,
            original_bytes,
            encoded.len(),
            ratio
        );
        assert!(ratio > 5.0, "压缩比应 > 5x，实际 {:.2}x", ratio);
    }

    // --- 查询延迟测试 ---

    #[test]
    fn test_decode_latency_under_50ms() {
        // 10 万点解码延迟应 < 50ms
        let n = 100_000;
        let points: Vec<(i64, f64, DataQuality)> = (0..n)
            .map(|i| {
                let v = 220.0 + (i as f64 * 0.001).sin() * 0.5;
                (1_700_000_000_000 + i as i64 * 1000, v, DataQuality::Good)
            })
            .collect();

        let encoded = encode_series(&points);

        let start = std::time::Instant::now();
        let decoded = decode_series(&encoded);
        let elapsed = start.elapsed();

        assert_eq!(decoded.len(), n);
        eprintln!(
            "解码 {} 点耗时: {:.2}ms",
            n,
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(
            elapsed.as_millis() < 50,
            "解码延迟应 < 50ms，实际 {}ms",
            elapsed.as_millis()
        );
    }
}
