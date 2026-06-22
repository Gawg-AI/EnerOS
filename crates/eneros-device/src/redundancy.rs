//! 协议冗余路径模块
//!
//! 实现 PRP（Parallel Redundancy Protocol）和 HSR（High-availability Seamless
//! Redundancy）基础框架，提供重复帧检测和双链路管理。
//!
//! # 协议参考
//!
//! - **PRP**：IEC 62439-3 Clause 4，双链路并行冗余。节点通过两个独立局域网
//!   （LAN A / LAN B）同时发送相同帧，接收方收到第一帧后处理，第二帧（重复帧）丢弃。
//!   重复帧检测基于「源 MAC + 序列号」。PRP 在帧尾附加 RCT（Redundancy Control
//!   Trailer）。
//! - **HSR**：IEC 62439-3 Clause 5，环网冗余。节点组成环，帧沿两个方向发送，
//!   接收方收到第一帧后处理，第二帧丢弃。HSR 在以太网头之后附加 HSR Tag。
//!
//! # 设计约束
//!
//! - 纯 Rust 实现，仅依赖 `std`，不依赖外部网络库。
//! - 本模块只负责冗余控制（RCT/Tag 编解码、重复帧检测、双链路状态），
//!   实际帧收发由上层（如 `af_packet`）调用本模块完成。
//! - 重复帧检测缓存采用 LRU + 老化策略，限制内存占用，适配电力 OS 资源约束。

use std::collections::HashMap;
use std::time::{Duration, Instant};

// ============================================================================
// 冗余模式与链路标识
// ============================================================================

/// 冗余模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedundancyMode {
    /// PRP 并行冗余
    Prp,
    /// HSR 环网冗余
    Hsr,
    /// 无冗余
    None,
}

/// 冗余链路标识
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkId {
    /// 链路 A
    A,
    /// 链路 B
    B,
}

// ============================================================================
// PRP RCT（Redundancy Control Trailer）
// ============================================================================

/// PRP RCT（Redundancy Control Trailer）结构
///
/// 标准 RCT 格式（6 字节，IEC 62439-3 Clause 4）：
/// ```text
/// +------------------------+-----------------------------+
/// | LSDU_size (2 字节,大端)| Sequence (4 字节, 大端)     |
/// +------------------------+-----------------------------+
/// ```
/// - LSDU_size：链路层服务数据单元大小（本实现解析但不在重复检测中使用）
/// - Sequence：4 字节序列号，用于重复帧检测
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrpRct {
    /// 序列号（4 字节）
    pub sequence: u32,
    /// LSDU 大小（2 字节）
    pub lsdu_size: u16,
}

impl PrpRct {
    /// PRP RCT 长度（6 字节）
    pub const LEN: usize = 6;

    /// 从字节解析 RCT（取最后 6 字节）
    ///
    /// 标准 PRP RCT: LSDU_size(16位) | SeqNr(32位)。
    /// 本实现只关心 SeqNr，LSDU_size 字段被解析保存但不参与重复检测。
    /// 不再要求前两字节为 0x00，以兼容所有标准 RCT。
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::LEN {
            return None;
        }
        let start = data.len() - Self::LEN;
        // LSDU_size(2字节) | SeqNr(4字节大端)
        let lsdu_size = u16::from_be_bytes([data[start], data[start + 1]]);
        let sequence = u32::from_be_bytes([
            data[start + 2],
            data[start + 3],
            data[start + 4],
            data[start + 5],
        ]);
        Some(Self {
            sequence,
            lsdu_size,
        })
    }

    /// 编码 RCT 为 6 字节数组
    ///
    /// 输出格式：LSDU_size(2字节大端) | SeqNr(4字节大端)
    pub fn to_bytes(&self) -> [u8; 6] {
        let mut buf = [0u8; 6];
        buf[0..2].copy_from_slice(&self.lsdu_size.to_be_bytes());
        buf[2..6].copy_from_slice(&self.sequence.to_be_bytes());
        buf
    }
}

// ============================================================================
// HSR Tag
// ============================================================================

/// HSR Tag 结构
///
/// HSR Tag 格式（6 字节，紧跟以太网头之后，IEC 62439-3 Clause 5）：
/// ```text
/// +------+----------+----------------------+----------------------+----------+
/// | Path | Reserved | LSDU type (2 字节)   | Sequence (2 字节)    | Reserved |
/// | 高2位| 低6位    | (EtherType, 大端)    | (大端, 16 位)        | (0x00)   |
/// +------+----------+----------------------+----------------------+----------+
///   字节0            字节1-2                 字节3-4                字节5
/// ```
/// - Path: 高 2 位编码，0b01 = 从 A 口收（0x40），0b10 = 从 B 口收（0x80）
/// - Reserved: 字节 0 低 6 位与字节 5 必须为 0
/// - LSDU type: EtherType（如 0x88B8 GOOSE, 0x0800 IPv4）
/// - Sequence: 2 字节序列号，回绕后从 0 重新开始
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HsrTag {
    /// 路径标识（逻辑值：0x01=A 口, 0x02=B 口）
    pub path: u8,
    /// LSDU 类型（EtherType，大端）
    pub lsdu_type: u16,
    /// 序列号（2 字节）
    pub sequence: u16,
}

/// HSR Path 逻辑值: 从 A 口接收
pub const HSR_PATH_A: u8 = 0x01;
/// HSR Path 逻辑值: 从 B 口接收
pub const HSR_PATH_B: u8 = 0x02;

impl HsrTag {
    /// HSR Tag 长度（6 字节）
    pub const LEN: usize = 6;

    /// Path A 的编码值（字节 0 高 2 位 = 0b01 → 0x40）
    pub const PATH_A: u8 = 0x40;
    /// Path B 的编码值（字节 0 高 2 位 = 0b10 → 0x80）
    pub const PATH_B: u8 = 0x80;

    /// 从字节解析 HSR Tag（取前 6 字节）
    ///
    /// 要求 `data` 长度至少为 6。字节 0 低 6 位与字节 5（reserved）必须为 0。
    /// path 在字节 0 高 2 位：0b01=A, 0b10=B。
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < Self::LEN {
            return None;
        }
        let path_byte = data[0];
        // path 在高 2 位
        let path = match path_byte & 0xC0 {
            Self::PATH_A => HSR_PATH_A, // A
            Self::PATH_B => HSR_PATH_B, // B
            _ => return None,           // 无效 path
        };
        // 检查字节 0 低 6 位是否为 0（reserved）
        if path_byte & 0x3F != 0 {
            return None;
        }
        // 检查字节 5（reserved）是否为 0
        if data[5] != 0x00 {
            return None;
        }
        let lsdu_type = u16::from_be_bytes([data[1], data[2]]);
        let sequence = u16::from_be_bytes([data[3], data[4]]);
        Some(Self {
            path,
            lsdu_type,
            sequence,
        })
    }

    /// 编码 HSR Tag 为 6 字节数组
    ///
    /// 输出格式：path(高2位)+reserved(低6位) | LSDU type(2字节) | Sequence(2字节) | reserved(0x00)
    pub fn to_bytes(&self) -> [u8; 6] {
        let path_byte = match self.path {
            HSR_PATH_A => Self::PATH_A,
            HSR_PATH_B => Self::PATH_B,
            _ => 0x00,
        };
        let mut buf = [0u8; 6];
        buf[0] = path_byte;
        buf[1..3].copy_from_slice(&self.lsdu_type.to_be_bytes());
        buf[3..5].copy_from_slice(&self.sequence.to_be_bytes());
        // buf[5] 是 reserved，保持 0x00
        buf
    }
}

// ============================================================================
// 冗余统计
// ============================================================================

/// 冗余统计
#[derive(Debug, Clone, Default)]
pub struct RedundancyStats {
    /// 接收帧总数
    pub total_received: u64,
    /// 重复帧丢弃数
    pub duplicates_dropped: u64,
    /// 链路 A 接收数
    pub link_a_received: u64,
    /// 链路 B 接收数
    pub link_b_received: u64,
    /// 单链路故障切换次数
    pub failovers: u64,
}

// ============================================================================
// 重复帧检测缓存
// ============================================================================

/// 重复帧检测缓存条目
#[derive(Debug, Clone)]
struct DuplicateEntry {
    /// 最近一次见到的序列号
    sequence: u64,
    /// 最近一次见到的时间
    last_seen: Instant,
}

/// 冗余管理器
///
/// 提供基于「源 MAC + 序列号」的重复帧检测，支持 PRP 与 HSR 两种模式。
/// 缓存采用 LRU + 老化策略，限制条目数以适配电力 OS 内存约束。
///
/// # 序列号回绕检测
///
/// HSR 序列号为 u16（65535），4kHz SV 下约 16 秒回绕一次；PRP 序列号为 u32。
/// 本实现采用「序列号窗口算法」判断回绕：当新序列号与缓存中序列号的 wrapping
/// 差值落在 `[1, WRAP_THRESHOLD)` 内时视为前进；差值为 0 视为重复；差值大于
/// 阈值视为回绕。阈值取 `2^(sequence_bits-1)`，即序列号空间的一半。
pub struct RedundancyManager {
    /// 冗余模式
    mode: RedundancyMode,
    /// 重复帧检测缓存: 源 MAC -> 最近序列号条目
    dup_cache: HashMap<[u8; 6], DuplicateEntry>,
    /// 缓存最大条目数
    max_cache_size: usize,
    /// 序列号老化时间
    aging_time: Duration,
    /// 序列号位宽（HSR=16, PRP=32），用于计算回绕阈值
    sequence_bits: u32,
    /// 统计
    stats: RedundancyStats,
}

impl RedundancyManager {
    /// 创建冗余管理器
    ///
    /// 默认参数：
    /// - 最大缓存条目数：256
    /// - 老化时间：2 秒
    /// - 序列号位宽：HSR=16, PRP/None=32
    pub fn new(mode: RedundancyMode) -> Self {
        let sequence_bits = match mode {
            RedundancyMode::Hsr => 16,
            RedundancyMode::Prp | RedundancyMode::None => 32,
        };
        Self {
            mode,
            dup_cache: HashMap::new(),
            max_cache_size: 256,
            aging_time: Duration::from_secs(2),
            sequence_bits,
            stats: RedundancyStats::default(),
        }
    }

    /// 检查是否为重复帧
    ///
    /// 返回 `true` 表示是重复帧（应丢弃），`false` 表示新帧（应处理）。
    ///
    /// # 参数
    /// - `src_mac`: 源 MAC 地址
    /// - `sequence`: 帧序列号（PRP 为 u32 扩展，HSR 为 u16 扩展）
    ///
    /// # 序列号回绕算法
    ///
    /// 使用 wrapping 差值与回绕阈值（`2^(sequence_bits-1)`）比较：
    /// - `diff == 0`：完全相同的序列号 → 重复帧
    /// - `0 < diff < WRAP_THRESHOLD`：序列号前进 → 新帧
    /// - `diff >= WRAP_THRESHOLD`：序列号回绕 → 新帧
    ///
    /// 重复帧也会刷新 `last_seen`，避免条目在老化窗口内被误淘汰后，
    /// 回绕后的相同序列号被误判为新帧（修复 H12）。
    pub fn check_duplicate(&mut self, src_mac: &[u8; 6], sequence: u64) -> bool {
        self.stats.total_received += 1;
        self.cleanup_expired();

        // 回绕阈值：2^(sequence_bits-1)
        // 对于 u16 序列号阈值为 32768；对于 u32 序列号阈值为 2^31
        let wrap_threshold = 1u64 << (self.sequence_bits.saturating_sub(1));

        if let Some(entry) = self.dup_cache.get_mut(src_mac) {
            let last_seq = entry.sequence;
            let diff = sequence.wrapping_sub(last_seq);

            if diff == 0 {
                // 完全相同的序列号 → 重复帧
                self.stats.duplicates_dropped += 1;
                // 修复 H12：重复帧也刷新 last_seen，避免条目过早老化
                entry.last_seen = Instant::now();
                return true;
            } else if diff < wrap_threshold {
                // 序列号前进 → 新帧
                entry.sequence = sequence;
                entry.last_seen = Instant::now();
            } else {
                // 序列号回绕 → 新帧
                entry.sequence = sequence;
                entry.last_seen = Instant::now();
            }
        } else {
            // 新源地址
            if self.dup_cache.len() >= self.max_cache_size {
                self.evict_oldest();
            }
            self.dup_cache.insert(
                *src_mac,
                DuplicateEntry {
                    sequence,
                    last_seen: Instant::now(),
                },
            );
        }
        false
    }

    /// 记录链路接收
    pub fn record_link_receive(&mut self, link: LinkId) {
        match link {
            LinkId::A => self.stats.link_a_received += 1,
            LinkId::B => self.stats.link_b_received += 1,
        }
    }

    /// 记录故障切换
    pub fn record_failover(&mut self) {
        self.stats.failovers += 1;
    }

    /// 获取统计引用
    pub fn stats(&self) -> &RedundancyStats {
        &self.stats
    }

    /// 获取冗余模式
    pub fn mode(&self) -> RedundancyMode {
        self.mode
    }

    /// 清理过期条目
    fn cleanup_expired(&mut self) {
        let now = Instant::now();
        self.dup_cache
            .retain(|_, entry| now.duration_since(entry.last_seen) < self.aging_time);
    }

    /// 淘汰最旧条目（LRU 简化版：按 last_seen 最小者淘汰）
    fn evict_oldest(&mut self) {
        if let Some((&oldest_mac, _)) = self
            .dup_cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_seen)
        {
            self.dup_cache.remove(&oldest_mac);
        }
    }

    /// 设置缓存最大条目数
    pub fn set_cache_size(&mut self, size: usize) {
        self.max_cache_size = size;
    }

    /// 设置老化时间
    pub fn set_aging_time(&mut self, time: Duration) {
        self.aging_time = time;
    }

    /// 设置序列号位宽（用于回绕检测）
    ///
    /// HSR 默认 16 位，PRP 默认 32 位。若上层使用非标准位宽可手动覆盖。
    /// 回绕阈值为 `2^(sequence_bits-1)`。
    pub fn set_sequence_bits(&mut self, bits: u32) {
        self.sequence_bits = bits;
    }

    /// 获取当前序列号位宽
    pub fn sequence_bits(&self) -> u32 {
        self.sequence_bits
    }

    /// 清空缓存
    pub fn clear_cache(&mut self) {
        self.dup_cache.clear();
    }

    /// 当前缓存条目数（用于测试与监控）
    pub fn cache_len(&self) -> usize {
        self.dup_cache.len()
    }
}

// ============================================================================
// 双链路状态管理
// ============================================================================

/// 链路状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// 链路正常
    Up,
    /// 链路故障
    Down,
}

/// 双链路管理器
///
/// 跟踪链路 A/B 的 Up/Down 状态，并在单链路故障/恢复时计算故障切换事件。
/// 当双链路均 Up 时，`active_link` 为 `None`（表示双链路并行工作）；
/// 当其中一条 Down 时，`active_link` 切换为另一条。
pub struct DualLinkManager {
    /// 链路 A 状态
    link_a: LinkState,
    /// 链路 B 状态
    link_b: LinkState,
    /// 活跃链路（单链路工作时使用；双链路 Up 时为 None）
    active_link: Option<LinkId>,
}

impl DualLinkManager {
    /// 创建双链路管理器，默认双链路均 Up
    pub fn new() -> Self {
        Self {
            link_a: LinkState::Up,
            link_b: LinkState::Up,
            active_link: None,
        }
    }

    /// 设置链路状态
    ///
    /// 返回 `true` 表示触发了故障切换事件（应调用方记录到统计）。
    pub fn set_link_state(&mut self, link: LinkId, state: LinkState) -> bool {
        let old_a = self.link_a;
        let old_b = self.link_b;

        match link {
            LinkId::A => self.link_a = state,
            LinkId::B => self.link_b = state,
        }

        // 检测故障切换
        match (link, state) {
            // A 故障，B 正常 -> 切换到 B
            (LinkId::A, LinkState::Down)
                if old_a == LinkState::Up && self.link_b == LinkState::Up =>
            {
                self.active_link = Some(LinkId::B);
                true
            }
            // B 故障，A 正常 -> 切换到 A
            (LinkId::B, LinkState::Down)
                if old_b == LinkState::Up && self.link_a == LinkState::Up =>
            {
                self.active_link = Some(LinkId::A);
                true
            }
            // A 恢复，B 仍故障 -> 切换到 A
            (LinkId::A, LinkState::Up)
                if old_a == LinkState::Down && self.link_b == LinkState::Down =>
            {
                self.active_link = Some(LinkId::A);
                true
            }
            // B 恢复，A 仍故障 -> 切换到 B
            (LinkId::B, LinkState::Up)
                if old_b == LinkState::Down && self.link_a == LinkState::Down =>
            {
                self.active_link = Some(LinkId::B);
                true
            }
            _ => {
                // 双链路均 Up 时清除活跃链路
                if self.link_a == LinkState::Up && self.link_b == LinkState::Up {
                    self.active_link = None;
                }
                false
            }
        }
    }

    /// 获取活跃链路
    pub fn active_link(&self) -> Option<LinkId> {
        self.active_link
    }

    /// 是否双链路都正常
    pub fn both_up(&self) -> bool {
        self.link_a == LinkState::Up && self.link_b == LinkState::Up
    }

    /// 是否双链路都故障
    pub fn both_down(&self) -> bool {
        self.link_a == LinkState::Down && self.link_b == LinkState::Down
    }

    /// 获取指定链路状态
    pub fn link_state(&self, link: LinkId) -> LinkState {
        match link {
            LinkId::A => self.link_a,
            LinkId::B => self.link_b,
        }
    }
}

impl Default for DualLinkManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // PRP RCT 测试
    // ------------------------------------------------------------------

    #[test]
    fn test_prp_rct_encode_decode() {
        let rct = PrpRct {
            sequence: 0x1234_5678,
            lsdu_size: 0,
        };
        let bytes = rct.to_bytes();
        assert_eq!(bytes.len(), PrpRct::LEN);
        assert_eq!(bytes[0], 0x00);
        assert_eq!(bytes[1], 0x00);

        let decoded = PrpRct::from_bytes(&bytes).expect("RCT 解码失败");
        assert_eq!(decoded.sequence, 0x1234_5678);
        assert_eq!(decoded, rct);
    }

    #[test]
    fn test_prp_rct_bad_prefix() {
        // 长度不足
        let short = [0u8; 5];
        assert!(PrpRct::from_bytes(&short).is_none());
    }

    #[test]
    fn test_prp_rct_nonzero_lsdu_size() {
        // 非零 LSDU_size 应被正确解析（标准兼容，修复 M12）
        let rct = PrpRct {
            sequence: 0x1234_5678,
            lsdu_size: 0x0ABC,
        };
        let bytes = rct.to_bytes();
        assert_eq!(bytes.len(), PrpRct::LEN);
        // 前两字节为 LSDU_size（非 0x00 0x00）
        assert_eq!(bytes[0], 0x0A);
        assert_eq!(bytes[1], 0xBC);

        let decoded = PrpRct::from_bytes(&bytes).expect("非零 LSDU_size RCT 解码失败");
        assert_eq!(decoded.sequence, 0x1234_5678);
        assert_eq!(decoded.lsdu_size, 0x0ABC);
        assert_eq!(decoded, rct);
    }

    #[test]
    fn test_prp_rct_from_full_frame() {
        // 模拟一个完整帧：payload + RCT
        let payload = [0xAA, 0xBB, 0xCC, 0xDD];
        let rct = PrpRct {
            sequence: 0xDEAD_BEEF,
            lsdu_size: 0,
        };
        let mut frame = payload.to_vec();
        frame.extend_from_slice(&rct.to_bytes());

        let decoded = PrpRct::from_bytes(&frame).expect("从完整帧解析 RCT 失败");
        assert_eq!(decoded.sequence, 0xDEAD_BEEF);
    }

    // ------------------------------------------------------------------
    // HSR Tag 测试
    // ------------------------------------------------------------------

    #[test]
    fn test_hsr_tag_encode_decode() {
        let tag = HsrTag {
            path: HSR_PATH_A,
            lsdu_type: 0x88B8, // GOOSE
            sequence: 0x1234,
        };
        let bytes = tag.to_bytes();
        assert_eq!(bytes.len(), HsrTag::LEN);
        // path 在字节 0 高 2 位（0x40=A），低 6 位 reserved 为 0
        assert_eq!(bytes[0], HsrTag::PATH_A);
        assert_eq!(bytes[0] & 0x3F, 0x00);
        // 字节 5 reserved 为 0
        assert_eq!(bytes[5], 0x00);

        let decoded = HsrTag::from_bytes(&bytes).expect("HSR Tag 解码失败");
        assert_eq!(decoded, tag);
    }

    #[test]
    fn test_hsr_tag_path_b() {
        let tag = HsrTag {
            path: HSR_PATH_B,
            lsdu_type: 0x0800, // IPv4
            sequence: 0xFFFF,
        };
        let bytes = tag.to_bytes();
        // path B 编码为 0x80
        assert_eq!(bytes[0], HsrTag::PATH_B);
        let decoded = HsrTag::from_bytes(&bytes).expect("HSR Tag 解码失败");
        assert_eq!(decoded.path, HSR_PATH_B);
        assert_eq!(decoded.lsdu_type, 0x0800);
        assert_eq!(decoded.sequence, 0xFFFF);
    }

    #[test]
    fn test_hsr_tag_bad_reserved() {
        // 字节 0 低 6 位非 0（非法 reserved）
        let mut bad = [0u8; 6];
        bad[0] = HsrTag::PATH_A | 0x01; // 低 6 位非 0
        bad[1..3].copy_from_slice(&0x88B8u16.to_be_bytes());
        bad[3..5].copy_from_slice(&0x1234u16.to_be_bytes());
        // bad[5] = 0x00
        assert!(HsrTag::from_bytes(&bad).is_none());

        // 字节 5（reserved）非 0
        let mut bad2 = [0u8; 6];
        bad2[0] = HsrTag::PATH_B;
        bad2[1..3].copy_from_slice(&0x88B8u16.to_be_bytes());
        bad2[3..5].copy_from_slice(&0x1234u16.to_be_bytes());
        bad2[5] = 0xFF; // 非法 reserved
        assert!(HsrTag::from_bytes(&bad2).is_none());
    }

    #[test]
    fn test_hsr_tag_invalid_path() {
        // path 高 2 位为 0b00（无效）
        let mut bad = [0u8; 6];
        bad[0] = 0x00; // path = 0b00, 无效
        bad[1..3].copy_from_slice(&0x88B8u16.to_be_bytes());
        bad[3..5].copy_from_slice(&0x1234u16.to_be_bytes());
        assert!(HsrTag::from_bytes(&bad).is_none());

        // path 高 2 位为 0b11（无效）
        let mut bad2 = [0u8; 6];
        bad2[0] = 0xC0; // path = 0b11, 无效
        bad2[1..3].copy_from_slice(&0x88B8u16.to_be_bytes());
        bad2[3..5].copy_from_slice(&0x1234u16.to_be_bytes());
        assert!(HsrTag::from_bytes(&bad2).is_none());
    }

    #[test]
    fn test_hsr_tag_path_high_bits_encoding() {
        // 验证 path 在高 2 位编码（修复 M13）
        let tag_a = HsrTag {
            path: HSR_PATH_A,
            lsdu_type: 0x88B8,
            sequence: 0x0001,
        };
        let bytes_a = tag_a.to_bytes();
        assert_eq!(bytes_a[0], 0x40); // 0b01 << 6
        assert_eq!(bytes_a[0] & 0xC0, 0x40);

        let tag_b = HsrTag {
            path: HSR_PATH_B,
            lsdu_type: 0x88B8,
            sequence: 0x0002,
        };
        let bytes_b = tag_b.to_bytes();
        assert_eq!(bytes_b[0], 0x80); // 0b10 << 6
        assert_eq!(bytes_b[0] & 0xC0, 0x80);

        // 往返验证
        let decoded_a = HsrTag::from_bytes(&bytes_a).unwrap();
        assert_eq!(decoded_a.path, HSR_PATH_A);
        let decoded_b = HsrTag::from_bytes(&bytes_b).unwrap();
        assert_eq!(decoded_b.path, HSR_PATH_B);
    }

    #[test]
    fn test_hsr_tag_too_short() {
        let short = [0u8; 5];
        assert!(HsrTag::from_bytes(&short).is_none());
    }

    // ------------------------------------------------------------------
    // 重复帧检测测试
    // ------------------------------------------------------------------

    #[test]
    fn test_duplicate_detection_first_frame() {
        let mut mgr = RedundancyManager::new(RedundancyMode::Prp);
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        // 首帧不应被识别为重复
        assert!(!mgr.check_duplicate(&mac, 100));
        assert_eq!(mgr.stats().total_received, 1);
        assert_eq!(mgr.stats().duplicates_dropped, 0);
    }

    #[test]
    fn test_duplicate_detection_same_sequence() {
        let mut mgr = RedundancyManager::new(RedundancyMode::Prp);
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        assert!(!mgr.check_duplicate(&mac, 100));
        // 相同序列号 -> 重复
        assert!(mgr.check_duplicate(&mac, 100));
        assert_eq!(mgr.stats().duplicates_dropped, 1);
    }

    #[test]
    fn test_duplicate_detection_different_sequence() {
        let mut mgr = RedundancyManager::new(RedundancyMode::Hsr);
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        assert!(!mgr.check_duplicate(&mac, 100));
        // 不同序列号 -> 不重复
        assert!(!mgr.check_duplicate(&mac, 101));
        assert_eq!(mgr.stats().duplicates_dropped, 0);
    }

    #[test]
    fn test_duplicate_detection_different_mac() {
        let mut mgr = RedundancyManager::new(RedundancyMode::Prp);
        let mac_a = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let mac_b = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        // 不同源 MAC，相同序列号 -> 不重复
        assert!(!mgr.check_duplicate(&mac_a, 100));
        assert!(!mgr.check_duplicate(&mac_b, 100));
        assert_eq!(mgr.stats().duplicates_dropped, 0);
    }

    #[test]
    fn test_cache_aging() {
        let mut mgr = RedundancyManager::new(RedundancyMode::Prp);
        mgr.set_aging_time(Duration::from_millis(50));
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        assert!(!mgr.check_duplicate(&mac, 100));
        assert_eq!(mgr.cache_len(), 1);

        // 等待老化
        std::thread::sleep(Duration::from_millis(80));
        // 老化后再次发送相同序列号，应被视为新帧（缓存已清理）
        assert!(!mgr.check_duplicate(&mac, 100));
        assert_eq!(mgr.stats().duplicates_dropped, 0);
    }

    #[test]
    fn test_cache_eviction() {
        let mut mgr = RedundancyManager::new(RedundancyMode::Prp);
        mgr.set_cache_size(2);
        let mac_a = [0x01; 6];
        let mac_b = [0x02; 6];
        let mac_c = [0x03; 6];

        assert!(!mgr.check_duplicate(&mac_a, 1));
        assert!(!mgr.check_duplicate(&mac_b, 1));
        assert_eq!(mgr.cache_len(), 2);

        // 插入第三条，应触发淘汰（mac_a 最旧）
        assert!(!mgr.check_duplicate(&mac_c, 1));
        assert_eq!(mgr.cache_len(), 2);
    }

    #[test]
    fn test_stats() {
        let mut mgr = RedundancyManager::new(RedundancyMode::Prp);
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];

        mgr.record_link_receive(LinkId::A);
        mgr.record_link_receive(LinkId::A);
        mgr.record_link_receive(LinkId::B);
        mgr.record_failover();

        // 触发一些重复检测
        mgr.check_duplicate(&mac, 1);
        mgr.check_duplicate(&mac, 1); // 重复

        let stats = mgr.stats();
        assert_eq!(stats.link_a_received, 2);
        assert_eq!(stats.link_b_received, 1);
        assert_eq!(stats.failovers, 1);
        assert_eq!(stats.total_received, 2);
        assert_eq!(stats.duplicates_dropped, 1);
    }

    #[test]
    fn test_sequence_wraparound() {
        let mut mgr = RedundancyManager::new(RedundancyMode::Hsr);
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];

        // HSR 序列号为 u16，回绕场景：0xFFFF -> 0x0000
        let seq_max = u16::MAX as u64;
        let seq_zero = 0u64;

        assert!(!mgr.check_duplicate(&mac, seq_max));
        // 回绕到 0，应被视为新帧
        assert!(!mgr.check_duplicate(&mac, seq_zero));
        assert_eq!(mgr.stats().duplicates_dropped, 0);

        // 再次发送 0 -> 重复
        assert!(mgr.check_duplicate(&mac, seq_zero));
        assert_eq!(mgr.stats().duplicates_dropped, 1);
    }

    #[test]
    fn test_sequence_wraparound_u16_comprehensive() {
        // 序列号回绕窗口算法综合测试（修复 H11）
        // HSR u16 序列号空间 0..=65535，回绕阈值 32768
        let mut mgr = RedundancyManager::new(RedundancyMode::Hsr);
        assert_eq!(mgr.sequence_bits(), 16);
        let mac = [0x10, 0x20, 0x30, 0x40, 0x50, 0x60];

        // 场景 1: 正常前进 100 -> 101
        assert!(!mgr.check_duplicate(&mac, 100));
        assert!(!mgr.check_duplicate(&mac, 101));
        assert_eq!(mgr.stats().duplicates_dropped, 0);

        // 场景 2: 回绕 65535 -> 0（diff = 1，前进）
        assert!(!mgr.check_duplicate(&mac, 65535));
        assert!(!mgr.check_duplicate(&mac, 0));
        assert_eq!(mgr.stats().duplicates_dropped, 0);

        // 场景 3: 后退 65535 -> 32767（sequence < last_seq，diff 为巨大值 > 阈值 → 回绕，新帧）
        assert!(!mgr.check_duplicate(&mac, 65535));
        assert!(!mgr.check_duplicate(&mac, 32767));
        assert_eq!(mgr.stats().duplicates_dropped, 0);

        // 场景 4: 后退 40000 -> 100（sequence < last_seq，diff 为巨大值 > 阈值 → 回绕，新帧）
        assert!(!mgr.check_duplicate(&mac, 40000));
        assert!(!mgr.check_duplicate(&mac, 100));
        assert_eq!(mgr.stats().duplicates_dropped, 0);

        // 场景 5: 完全相同序列号 → 重复
        assert!(!mgr.check_duplicate(&mac, 5000));
        assert!(mgr.check_duplicate(&mac, 5000));
        assert_eq!(mgr.stats().duplicates_dropped, 1);
    }

    #[test]
    fn test_sequence_wraparound_prp_u32() {
        // PRP u32 序列号回绕测试，回绕阈值 2^31
        let mut mgr = RedundancyManager::new(RedundancyMode::Prp);
        assert_eq!(mgr.sequence_bits(), 32);
        let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];

        // u32::MAX -> 0 回绕
        let seq_max = u32::MAX as u64;
        assert!(!mgr.check_duplicate(&mac, seq_max));
        assert!(!mgr.check_duplicate(&mac, 0));
        assert_eq!(mgr.stats().duplicates_dropped, 0);

        // 0 重复
        assert!(mgr.check_duplicate(&mac, 0));
        assert_eq!(mgr.stats().duplicates_dropped, 1);
    }

    #[test]
    fn test_duplicate_refreshes_last_seen() {
        // 重复帧应刷新 last_seen，避免条目过早老化（修复 H12）
        let mut mgr = RedundancyManager::new(RedundancyMode::Prp);
        // 老化时间设为 100ms
        mgr.set_aging_time(Duration::from_millis(100));
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];

        // 首帧
        assert!(!mgr.check_duplicate(&mac, 42));
        assert_eq!(mgr.cache_len(), 1);

        // 等待 60ms（未老化）
        std::thread::sleep(Duration::from_millis(60));
        // 重复帧应刷新 last_seen
        assert!(mgr.check_duplicate(&mac, 42));
        assert_eq!(mgr.stats().duplicates_dropped, 1);
        assert_eq!(mgr.cache_len(), 1);

        // 再等待 60ms（若未刷新 last_seen，则总时间 120ms > 100ms 会老化）
        // 由于重复帧刷新了 last_seen，此时距上次刷新仅 60ms，不应老化
        std::thread::sleep(Duration::from_millis(60));
        // 再次发送相同序列号，应仍被识别为重复（缓存未老化）
        assert!(mgr.check_duplicate(&mac, 42));
        assert_eq!(mgr.stats().duplicates_dropped, 2);
        assert_eq!(mgr.cache_len(), 1);

        // 若 last_seen 未被刷新，此处缓存已被清理，相同序列号会被视为新帧
        // 验证 duplicates_dropped 仍为 2（没有因为老化而误判为新帧）
    }

    // ------------------------------------------------------------------
    // 双链路管理器测试
    // ------------------------------------------------------------------

    #[test]
    fn test_dual_link_failover_a_down() {
        let mut mgr = DualLinkManager::new();
        assert!(mgr.both_up());
        assert_eq!(mgr.active_link(), None);

        // A 故障 -> 切换到 B
        let failover = mgr.set_link_state(LinkId::A, LinkState::Down);
        assert!(failover, "A 故障应触发切换");
        assert_eq!(mgr.active_link(), Some(LinkId::B));
        assert!(!mgr.both_up());
        assert!(!mgr.both_down());
    }

    #[test]
    fn test_dual_link_failover_b_down() {
        let mut mgr = DualLinkManager::new();
        // B 故障 -> 切换到 A
        let failover = mgr.set_link_state(LinkId::B, LinkState::Down);
        assert!(failover, "B 故障应触发切换");
        assert_eq!(mgr.active_link(), Some(LinkId::A));
    }

    #[test]
    fn test_dual_link_both_up() {
        let mut mgr = DualLinkManager::new();
        assert!(mgr.both_up());

        // A 故障
        mgr.set_link_state(LinkId::A, LinkState::Down);
        assert!(!mgr.both_up());

        // A 恢复 -> 双链路 Up，active_link 清空
        let failover = mgr.set_link_state(LinkId::A, LinkState::Up);
        // A 恢复时 B 仍 Up，不触发 failover（双链路恢复）
        assert!(!failover);
        assert!(mgr.both_up());
        assert_eq!(mgr.active_link(), None);
    }

    #[test]
    fn test_dual_link_both_down() {
        let mut mgr = DualLinkManager::new();
        mgr.set_link_state(LinkId::A, LinkState::Down);
        // B 也故障 -> 双链路 Down
        mgr.set_link_state(LinkId::B, LinkState::Down);
        assert!(mgr.both_down());
        // 双链路 Down 时 active_link 仍为上一次切换的 B
        assert_eq!(mgr.active_link(), Some(LinkId::B));

        // A 恢复 -> 切换到 A
        let failover = mgr.set_link_state(LinkId::A, LinkState::Up);
        assert!(failover, "双链路 Down 后 A 恢复应触发切换");
        assert_eq!(mgr.active_link(), Some(LinkId::A));
    }

    #[test]
    fn test_dual_link_link_state_query() {
        let mut mgr = DualLinkManager::new();
        assert_eq!(mgr.link_state(LinkId::A), LinkState::Up);
        assert_eq!(mgr.link_state(LinkId::B), LinkState::Up);

        mgr.set_link_state(LinkId::B, LinkState::Down);
        assert_eq!(mgr.link_state(LinkId::A), LinkState::Up);
        assert_eq!(mgr.link_state(LinkId::B), LinkState::Down);
    }

    // ------------------------------------------------------------------
    // 冗余模式与配置测试
    // ------------------------------------------------------------------

    #[test]
    fn test_redundancy_mode() {
        let prp = RedundancyManager::new(RedundancyMode::Prp);
        assert_eq!(prp.mode(), RedundancyMode::Prp);

        let hsr = RedundancyManager::new(RedundancyMode::Hsr);
        assert_eq!(hsr.mode(), RedundancyMode::Hsr);

        let none = RedundancyManager::new(RedundancyMode::None);
        assert_eq!(none.mode(), RedundancyMode::None);
    }

    #[test]
    fn test_clear_cache() {
        let mut mgr = RedundancyManager::new(RedundancyMode::Prp);
        let mac = [0x01; 6];
        mgr.check_duplicate(&mac, 1);
        assert_eq!(mgr.cache_len(), 1);

        mgr.clear_cache();
        assert_eq!(mgr.cache_len(), 0);

        // 清空后相同序列号应被视为新帧
        assert!(!mgr.check_duplicate(&mac, 1));
    }
}
