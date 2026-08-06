//! 北斗授时真实输入接线（v0.216.0，B1-04）。
//!
//! 此前 `feed_nmea`/`on_pps_pulse` 依赖调用方按行/按时标注入；本模块
//! 提供真实输入路径：
//!
//! 1. [`NmeaFrameDecoder`] — 从 UART 字节流按 `$...*XX\r\n` 分帧，
//!    帧内校验和验证，坏帧计数（不静默丢帧）；
//! 2. [`PpsInput`] — 1PPS 边沿捕获：接收硬件时间戳 + 脉冲间隔测量，
//!    生成秒脉冲同步误差报告（间隔 vs 1s 的偏差/抖动/均值）；
//! 3. [`PpsSyncReport`] — 秒脉冲同步误差报告（验收项）。
//!
//! 接线方式：UART RX 中断逐字节喂 `feed_byte`；1PPS GPIO 中断喂
//! `on_pps_edge(local_ns)`。

use spin::Mutex;

use crate::beidou::nmea::parse_nmea;
use crate::beidou::{feed_nmea, on_pps_pulse, TimeStamp};

// ============================================================================
// NMEA 流式分帧
// ============================================================================

/// NMEA 帧解码器：从字节流中恢复完整 `$...*XX` 句子。
///
/// 状态机：等待 `$` → 收集到 `*` → 校验和 2 字节 → 可选 `\r\n` → 成帧。
/// 坏帧（校验和错误/超长）被拒绝并计数，不静默丢弃。
pub struct NmeaFrameDecoder {
    /// 帧缓冲（NMEA 最大 82 字节，留余量）。
    buf: [u8; 96],
    len: usize,
    /// 是否已见到 `$`。
    in_frame: bool,
    /// 校验和（帧内 XOR）。
    checksum: u8,
    /// 已见 `*`。
    seen_star: bool,
    /// 校验和字节计数（0..2）。
    cs_count: usize,
    /// 累计接收字节数。
    total_bytes: u64,
    /// 成帧数。
    frames_ok: u64,
    /// 坏帧数（校验和错误/超长/截断）。
    frames_bad: u64,
}

impl Default for NmeaFrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl NmeaFrameDecoder {
    /// 创建解码器。
    pub const fn new() -> Self {
        Self {
            buf: [0; 96],
            len: 0,
            in_frame: false,
            checksum: 0,
            seen_star: false,
            cs_count: 0,
            total_bytes: 0,
            frames_ok: 0,
            frames_bad: 0,
        }
    }

    /// 喂入一个 UART 字节。返回 `Some(sentence)` 表示成帧（不含 `\r\n`）。
    ///
    /// 校验和错误返回 `Some` 前先拒绝并计坏帧（返回 `None`）。
    pub fn feed_byte(&mut self, byte: u8) -> Option<[u8; 96]> {
        self.total_bytes += 1;
        match byte {
            b'$' => {
                // 新帧开始：丢弃未完成帧（计坏帧）。
                if self.in_frame {
                    self.frames_bad += 1;
                }
                self.in_frame = true;
                self.seen_star = false;
                self.cs_count = 0;
                self.checksum = 0;
                self.len = 0;
                self.buf[0] = b'$';
                self.len = 1;
                None
            }
            b'\r' | b'\n' if self.in_frame => {
                // 行结束。若已收齐校验和 → 成帧（校验和验证后计数）；
                // 否则坏帧（截断）。
                if self.seen_star && self.cs_count >= 2 {
                    let frame = self.buf;
                    let actual = frame
                        .iter()
                        .position(|&b| b == 0 || b == b'\r' || b == b'\n')
                        .unwrap_or(frame.len());
                    if Self::verify_checksum(&frame[..actual]) {
                        self.frames_ok += 1;
                        self.in_frame = false;
                        Some(frame)
                    } else {
                        self.frames_bad += 1;
                        self.in_frame = false;
                        None
                    }
                } else {
                    self.frames_bad += 1;
                    self.in_frame = false;
                    None
                }
            }
            _ if !self.in_frame => None,
            _ => {
                if self.seen_star && self.cs_count >= 2 {
                    // 校验和之后的多余字符：坏帧（帧格式错误）。
                    self.frames_bad += 1;
                    self.in_frame = false;
                    return None;
                }
                if self.len >= self.buf.len() {
                    // 超长帧：拒绝。
                    self.frames_bad += 1;
                    self.in_frame = false;
                    return None;
                }
                self.buf[self.len] = byte;
                self.len += 1;
                if self.seen_star {
                    self.cs_count += 1;
                } else if byte == b'*' {
                    self.seen_star = true;
                } else {
                    self.checksum ^= byte;
                }
                None
            }
        }
    }

    /// 校验一帧的 NMEA 校验和（`$...*XX` 内 `$` 后至 `*` 前的 XOR == XX）。
    fn verify_checksum(frame: &[u8]) -> bool {
        // frame 以 $ 开头；查找 *。
        let star = match frame.iter().position(|&b| b == b'*') {
            Some(p) => p,
            None => return false,
        };
        // 校验和两字节必须是 hex。
        if frame.len() < star + 3 {
            return false;
        }
        let hi = hex_val(frame[star + 1]);
        let lo = hex_val(frame[star + 2]);
        let (Some(hi), Some(lo)) = (hi, lo) else {
            return false;
        };
        let expected = (hi << 4) | lo;
        let mut xor = 0u8;
        for &b in &frame[1..star] {
            xor ^= b;
        }
        xor == expected
    }

    /// 统计信息（成帧/坏帧/字节数）。
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.frames_ok, self.frames_bad, self.total_bytes)
    }

    /// 复位解码器。
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// hex 字符转数值。
const fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// 处理一个完整帧：校验和验证通过后喂给 `feed_nmea`。
///
/// 返回 `true` 表示帧被接受并解析。
pub fn process_frame(frame: &[u8; 96]) -> bool {
    // 帧以 $ 开头；按实际长度（遇 0 或 \r\n 截断）。
    let actual = frame
        .iter()
        .position(|&b| b == 0 || b == b'\r' || b == b'\n')
        .unwrap_or(frame.len());
    let slice = &frame[..actual];
    if !NmeaFrameDecoder::verify_checksum(slice) {
        return false;
    }
    // 校验和通过：交给现有链路解析（失败不致命，仅计为解析失败）。
    feed_nmea(slice);
    let _ = parse_nmea(slice);
    true
}

// ============================================================================
// 1PPS 捕获与秒脉冲同步误差报告
// ============================================================================

/// PPS 捕获窗口大小（秒脉冲间隔样本数）。
const PPS_WINDOW: usize = 8;

/// 1PPS 输入：边沿时间戳捕获 + 间隔统计。
///
/// 每次 `on_pps_edge` 记录本地单调时钟的边沿时刻，计算相邻脉冲间隔
/// 与 1s 的偏差，输出 [`PpsSyncReport`]。
pub struct PpsInput {
    /// 最近边沿时间戳（纳秒）。
    last_edge_ns: Option<u64>,
    /// 脉冲间隔样本环（纳秒）。
    intervals: [u64; PPS_WINDOW],
    /// 有效间隔数。
    count: usize,
    /// 环写指针。
    head: usize,
    /// 脉冲总数。
    pulse_count: u64,
}

impl Default for PpsInput {
    fn default() -> Self {
        Self::new()
    }
}

impl PpsInput {
    /// 创建 PPS 输入。
    pub const fn new() -> Self {
        Self {
            last_edge_ns: None,
            intervals: [0; PPS_WINDOW],
            count: 0,
            head: 0,
            pulse_count: 0,
        }
    }

    /// 1PPS 边沿回调（硬件中断）。
    ///
    /// `local_ns` 为边沿时刻的本地单调时钟读数。同时调用
    /// [`on_pps_pulse`](crate::beidou::on_pps_pulse) 驱动既有授时链路。
    pub fn on_pps_edge(&mut self, local_ns: u64) {
        if let Some(last) = self.last_edge_ns {
            let interval = local_ns.saturating_sub(last);
            self.intervals[self.head] = interval;
            self.head = (self.head + 1) % PPS_WINDOW;
            if self.count < PPS_WINDOW {
                self.count += 1;
            }
        }
        self.last_edge_ns = Some(local_ns);
        self.pulse_count += 1;
        // 驱动既有链路（PPS 时标 + 抖动统计）。
        on_pps_pulse(TimeStamp {
            nanos_since_epoch: local_ns,
            leap_seconds: 0,
            fix_quality: crate::beidou::FixQuality::Fix3D { satellites: 0 },
        });
    }

    /// 生成秒脉冲同步误差报告。
    pub fn sync_report(&self) -> Option<PpsSyncReport> {
        if self.count == 0 {
            return None;
        }
        let n = self.count as u64;
        let mut sum = 0u64;
        let mut min = u64::MAX;
        let mut max = 0u64;
        for i in 0..self.count {
            let v = self.intervals[i];
            sum = sum.saturating_add(v);
            min = min.min(v);
            max = max.max(v);
        }
        let mean = sum / n;
        // 相对 1s（1_000_000_000 ns）的偏差。
        let mean_error = mean.abs_diff(1_000_000_000);
        let max_error = max.abs_diff(1_000_000_000);
        Some(PpsSyncReport {
            pulse_count: self.pulse_count,
            mean_interval_ns: mean,
            mean_error_ns: mean_error,
            max_error_ns: max_error,
            jitter_ns: max.saturating_sub(min),
        })
    }

    /// 复位。
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// 秒脉冲同步误差报告（验收项：北斗秒脉冲同步误差报告）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpsSyncReport {
    /// 累计脉冲数。
    pub pulse_count: u64,
    /// 平均脉冲间隔（纳秒，理想 1_000_000_000）。
    pub mean_interval_ns: u64,
    /// 平均间隔相对 1s 的误差（纳秒）。
    pub mean_error_ns: u64,
    /// 最大间隔相对 1s 的误差（纳秒）。
    pub max_error_ns: u64,
    /// 抖动（最大间隔 − 最小间隔，纳秒）。
    pub jitter_ns: u64,
}

// ============================================================================
// 全局接线（UART RX / PPS IRQ 直连入口）
// ============================================================================

/// 全局 NMEA 帧解码器。
static DECODER: Mutex<NmeaFrameDecoder> = Mutex::new(NmeaFrameDecoder::new());

/// 全局 PPS 输入。
static PPS: Mutex<PpsInput> = Mutex::new(PpsInput::new());

/// UART RX 字节入口（中断/轮询逐字节调用）。
pub fn feed_uart_byte(byte: u8) {
    let mut dec = DECODER.lock();
    if let Some(frame) = dec.feed_byte(byte) {
        process_frame(&frame);
    }
}

/// 1PPS 边沿入口（GPIO 中断）。
pub fn pps_edge(local_ns: u64) {
    PPS.lock().on_pps_edge(local_ns);
}

/// 查询当前秒脉冲同步误差报告。
pub fn pps_sync_report() -> Option<PpsSyncReport> {
    PPS.lock().sync_report()
}

/// 查询 NMEA 分帧统计。
pub fn decoder_stats() -> (u64, u64, u64) {
    let d = DECODER.lock();
    d.stats()
}

/// 复位全局接线状态（测试用）。
pub fn reset_inputs() {
    *DECODER.lock() = NmeaFrameDecoder::new();
    *PPS.lock() = PpsInput::new();
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feed_byte_frames_complete_sentence() {
        let mut dec = NmeaFrameDecoder::new();
        // $GNZDA,123456.00,01,01,2026,00,00*XX\r\n
        let mut sentence = b"$GNZDA,123456.00,01,01,2026,00,00".to_vec();
        // 计算校验和。
        let mut xor = 0u8;
        for &b in &sentence[1..] {
            xor ^= b;
        }
        sentence.push(b'*');
        sentence.push(hex_digit((xor >> 4) & 0xF));
        sentence.push(hex_digit(xor & 0xF));
        let mut out = None;
        for &b in sentence.iter().chain(b"\r\n".iter()) {
            if let Some(f) = dec.feed_byte(b) {
                out = Some(f);
            }
        }
        let frame = out.expect("frame should complete");
        assert!(process_frame(&frame));
        let (ok, bad, _) = dec.stats();
        assert_eq!(ok, 1);
        assert_eq!(bad, 0);
    }

    fn hex_digit(v: u8) -> u8 {
        if v < 10 {
            b'0' + v
        } else {
            b'A' + v - 10
        }
    }

    #[test]
    fn test_feed_byte_rejects_bad_checksum() {
        let mut dec = NmeaFrameDecoder::new();
        // 校验和错误（期望 XX 但给 00）。
        let mut sentence = b"$GNZDA,123456.00,01,01,2026,00,00".to_vec();
        sentence.push(b'*');
        sentence.push(b'0');
        sentence.push(b'0');
        let mut out = None;
        for &b in sentence.iter().chain(b"\r\n".iter()) {
            if let Some(f) = dec.feed_byte(b) {
                out = Some(f);
            }
        }
        assert!(out.is_none(), "坏校验和帧必须被拒绝，不产出帧");
        let (ok, bad, _) = dec.stats();
        assert_eq!(ok, 0);
        assert_eq!(bad, 1, "坏帧必须计数，不静默");
    }

    #[test]
    fn test_feed_byte_partial_then_next_frame() {
        let mut dec = NmeaFrameDecoder::new();
        // 半帧 + 完整帧：半帧被新帧取代 → 计 1 坏帧（截断），完整帧成功。
        for &b in b"$GNRMC,1234".iter() {
            assert!(dec.feed_byte(b).is_none());
        }
        // 完整帧。
        let mut sentence = b"$GNZDA,123456.00,01,01,2026,00,00".to_vec();
        let mut xor = 0u8;
        for &b in &sentence[1..] {
            xor ^= b;
        }
        sentence.push(b'*');
        sentence.push(hex_digit((xor >> 4) & 0xF));
        sentence.push(hex_digit(xor & 0xF));
        let mut done = false;
        for &b in sentence.iter().chain(b"\r\n".iter()) {
            if dec.feed_byte(b).is_some() {
                done = true;
            }
        }
        assert!(done);
        let (ok, bad, _) = dec.stats();
        assert_eq!(ok, 1);
        assert_eq!(bad, 1, "截断半帧计坏帧（不静默）");
    }

    #[test]
    fn test_pps_sync_report_ideal_interval() {
        let mut pps = PpsInput::new();
        for i in 0..4u64 {
            pps.on_pps_edge(1_000_000_000 * i);
        }
        let rep = pps.sync_report().expect("report");
        assert_eq!(rep.pulse_count, 4);
        assert_eq!(rep.mean_interval_ns, 1_000_000_000);
        assert_eq!(rep.mean_error_ns, 0);
        assert_eq!(rep.max_error_ns, 0);
        assert_eq!(rep.jitter_ns, 0);
    }

    #[test]
    fn test_pps_sync_report_detects_error() {
        let mut pps = PpsInput::new();
        // 间隔 1s, 1s, 1.005s（5ms 误差）。
        pps.on_pps_edge(0);
        pps.on_pps_edge(1_000_000_000);
        pps.on_pps_edge(2_000_000_000);
        pps.on_pps_edge(3_005_000_000);
        let rep = pps.sync_report().expect("report");
        // 平均间隔 = (1e9+1e9+1.005e9)/3 ≈ 1.00167e9 → 平均误差约 1.67ms。
        assert!(
            rep.mean_error_ns >= 1_000_000,
            "mean error {}",
            rep.mean_error_ns
        );
        assert_eq!(rep.max_error_ns, 5_000_000);
        assert_eq!(rep.jitter_ns, 5_000_000);
    }

    #[test]
    fn test_pps_sync_report_empty() {
        let pps = PpsInput::new();
        assert!(pps.sync_report().is_none());
    }

    #[test]
    fn test_global_uart_and_pps_wiring() {
        reset_inputs();
        // 喂一个真实格式的 ZDA 句子 + 若干 PPS 边沿。
        let mut sentence = b"$GNZDA,123456.00,01,01,2026,00,00".to_vec();
        let mut xor = 0u8;
        for &b in &sentence[1..] {
            xor ^= b;
        }
        sentence.push(b'*');
        sentence.push(hex_digit((xor >> 4) & 0xF));
        sentence.push(hex_digit(xor & 0xF));
        for &b in sentence.iter().chain(b"\r\n".iter()) {
            feed_uart_byte(b);
        }
        for i in 0..3u64 {
            pps_edge(1_000_000_000 * i);
        }
        let (ok, bad, _) = decoder_stats();
        assert_eq!(ok, 1);
        assert_eq!(bad, 0);
        let rep = pps_sync_report().expect("report");
        assert_eq!(rep.pulse_count, 3);
        assert_eq!(rep.mean_error_ns, 0);
    }

    #[test]
    fn test_beidou_state_updated_by_pps_edge() {
        reset_inputs();
        for i in 0..3u64 {
            pps_edge(1_000_000_000 * i);
        }
        // 既有链路状态应被驱动（disciplined = true）。
        let state = *crate::beidou::BEIDOU_STATE.lock();
        assert!(state.disciplined);
    }
}
