//! CAN 过滤器（v0.47.0）.
//!
//! 提供 ID + 掩码匹配的 CAN 帧过滤器，支持标准帧/扩展帧互斥检查（D7 偏差）。
//!
//! # 偏差声明
//! - D7: `CanFilter::matches()` 实现 ID+掩码匹配 + 标准帧/扩展帧互斥检查。

use crate::frame::CanFrame;

/// CAN 帧过滤器
///
/// 通过 `filter_id` / `filter_mask` 实现硬件无关的 ID 掩码匹配，
/// `extended` 字段用于区分标准帧（11-bit）与扩展帧（29-bit），二者互斥。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanFilter {
    /// 过滤 ID（与 mask 按位与后比较）
    pub filter_id: u32,
    /// 过滤掩码（0 表示接收所有）
    pub filter_mask: u32,
    /// 是否匹配扩展帧（true=扩展帧，false=标准帧）
    pub extended: bool,
}

impl CanFilter {
    /// 接收所有帧（仅按 `extended` 区分标准/扩展）
    ///
    /// `filter_mask == 0`，任何同类型（标准/扩展）帧都匹配。
    pub fn accept_all() -> Self {
        Self {
            filter_id: 0,
            filter_mask: 0,
            extended: false,
        }
    }

    /// 精确匹配某个 ID
    ///
    /// # 参数
    /// - `id`: 目标 ID（标准帧用 11-bit，扩展帧用 29-bit）
    /// - `extended`: 是否为扩展帧
    pub fn match_exact(id: u32, extended: bool) -> Self {
        let mask = if extended { 0x1FFFFFFF } else { 0x7FF };
        Self {
            filter_id: id,
            filter_mask: mask,
            extended,
        }
    }

    /// 前缀匹配（高位 `prefix_bits` 位必须匹配）
    ///
    /// # 参数
    /// - `prefix`: 前缀值（高位 `prefix_bits` 位参与比较）
    /// - `prefix_bits`: 前缀位数（标准帧 ≤ 11，扩展帧 ≤ 29）
    /// - `extended`: 是否为扩展帧
    pub fn match_prefix(prefix: u32, prefix_bits: u8, extended: bool) -> Self {
        let (width_mask, width_bits): (u32, u8) = if extended {
            (0x1FFFFFFF, 29)
        } else {
            (0x7FF, 11)
        };
        // 钳制 prefix_bits 到有效范围
        let prefix_bits = prefix_bits.min(width_bits);
        let mask: u32 = if prefix_bits == 0 {
            0
        } else {
            // 高 prefix_bits 位置 1，其余位置 0
            (((1u32 << prefix_bits) - 1) << (width_bits - prefix_bits)) & width_mask
        };
        Self {
            filter_id: prefix & width_mask,
            filter_mask: mask,
            extended,
        }
    }

    /// 判断帧是否匹配过滤器
    ///
    /// 匹配规则（D7）：
    /// 1. 帧的扩展性与过滤器的 `extended` 必须一致（标准帧/扩展帧互斥）
    /// 2. `(frame_id & mask) == (filter_id & mask)`
    pub fn matches(&self, frame: &CanFrame) -> bool {
        // D7: 标准/扩展帧互斥
        if frame.id.is_extended() != self.extended {
            return false;
        }
        // ID + 掩码匹配
        (frame.id.raw() & self.filter_mask) == (self.filter_id & self.filter_mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{CanFrame, CanId};

    // ===== accept_all 测试 =====

    #[test]
    fn test_accept_all_mask_zero() {
        let f = CanFilter::accept_all();
        assert_eq!(f.filter_mask, 0);
        assert_eq!(f.filter_id, 0);
        assert!(!f.extended);
    }

    #[test]
    fn test_accept_all_matches_any_standard() {
        let f = CanFilter::accept_all();
        // 标准帧：任意 ID 都匹配
        assert!(f.matches(&CanFrame::new_standard(0x000, &[])));
        assert!(f.matches(&CanFrame::new_standard(0x7FF, &[])));
        assert!(f.matches(&CanFrame::new_standard(0x123, &[0x01])));
    }

    #[test]
    fn test_accept_all_rejects_extended() {
        let f = CanFilter::accept_all(); // extended = false
        assert!(!f.matches(&CanFrame::new_extended(0x123, &[])));
    }

    // ===== match_exact 测试 =====

    #[test]
    fn test_match_exact_standard() {
        let f = CanFilter::match_exact(0x123, false);
        assert_eq!(f.filter_mask, 0x7FF);
        assert!(!f.extended);

        // 精确匹配
        assert!(f.matches(&CanFrame::new_standard(0x123, &[])));
        // 不匹配其他 ID
        assert!(!f.matches(&CanFrame::new_standard(0x124, &[])));
        assert!(!f.matches(&CanFrame::new_standard(0x122, &[])));
    }

    #[test]
    fn test_match_exact_extended() {
        let f = CanFilter::match_exact(0x1FFFFFFF, true);
        assert_eq!(f.filter_mask, 0x1FFFFFFF);
        assert!(f.extended);

        assert!(f.matches(&CanFrame::new_extended(0x1FFFFFFF, &[])));
        assert!(!f.matches(&CanFrame::new_extended(0x1FFFFFFE, &[])));
    }

    #[test]
    fn test_match_exact_standard_rejects_extended() {
        let f = CanFilter::match_exact(0x123, false);
        // 标准 ID 0x123 的过滤器不应匹配扩展帧 0x123
        assert!(!f.matches(&CanFrame::new_extended(0x123, &[])));
    }

    #[test]
    fn test_match_exact_extended_rejects_standard() {
        let f = CanFilter::match_exact(0x123, true);
        assert!(!f.matches(&CanFrame::new_standard(0x123, &[])));
    }

    // ===== match_prefix 测试 =====

    #[test]
    fn test_match_prefix_standard_4bits() {
        // 标准帧，前 4 位匹配 0x780（0b111_1000_0000）
        let f = CanFilter::match_prefix(0x780, 4, false);
        assert_eq!(f.filter_mask, 0x780);
        // 匹配 0x780~0x7FF（高 4 位 = 0b1111）
        assert!(f.matches(&CanFrame::new_standard(0x780, &[])));
        assert!(f.matches(&CanFrame::new_standard(0x7FF, &[])));
        // 不匹配 0x77F（高 4 位 = 0b1110）
        assert!(!f.matches(&CanFrame::new_standard(0x77F, &[])));
    }

    #[test]
    fn test_match_prefix_extended_8bits() {
        // 扩展帧 29-bit，高 8 位掩码 = 0x1FE00000（bits 28..=21）
        let f = CanFilter::match_prefix(0x1FE00000, 8, true);
        assert_eq!(f.filter_mask, 0x1FE00000);
        assert!(f.matches(&CanFrame::new_extended(0x1FE00000, &[])));
        assert!(f.matches(&CanFrame::new_extended(0x1FE00001, &[])));
        assert!(f.matches(&CanFrame::new_extended(0x1FFFFFFF, &[])));
        // 不匹配 0x1FC00000（高 8 位 = 0xFE ≠ 0xFF）
        assert!(!f.matches(&CanFrame::new_extended(0x1FC00000, &[])));
    }

    #[test]
    fn test_match_prefix_zero_bits_accepts_all() {
        // prefix_bits = 0 → mask = 0 → 接收所有同类型帧
        let f = CanFilter::match_prefix(0, 0, false);
        assert_eq!(f.filter_mask, 0);
        assert!(f.matches(&CanFrame::new_standard(0x000, &[])));
        assert!(f.matches(&CanFrame::new_standard(0x7FF, &[])));
    }

    #[test]
    fn test_match_prefix_full_bits_matches_exact() {
        // prefix_bits = 全宽 → mask = 全 1 → 等价于精确匹配
        let f = CanFilter::match_prefix(0x123, 11, false);
        assert_eq!(f.filter_mask, 0x7FF);
        assert!(f.matches(&CanFrame::new_standard(0x123, &[])));
        assert!(!f.matches(&CanFrame::new_standard(0x124, &[])));
    }

    #[test]
    fn test_match_prefix_clamps_overflow_bits() {
        // prefix_bits 超过宽度时钳制到宽度
        let f = CanFilter::match_prefix(0x123, 20, false); // 标准帧宽度 11
        assert_eq!(f.filter_mask, 0x7FF);
    }

    // ===== 边界值测试 =====

    #[test]
    fn test_match_exact_id_zero_standard() {
        let f = CanFilter::match_exact(0, false);
        assert!(f.matches(&CanFrame::new_standard(0, &[])));
        assert!(!f.matches(&CanFrame::new_standard(1, &[])));
    }

    #[test]
    fn test_match_exact_id_max_standard() {
        let f = CanFilter::match_exact(0x7FF, false);
        assert!(f.matches(&CanFrame::new_standard(0x7FF, &[])));
        assert!(!f.matches(&CanFrame::new_standard(0x7FE, &[])));
    }

    #[test]
    fn test_match_exact_id_max_extended() {
        let f = CanFilter::match_exact(0x1FFFFFFF, true);
        assert!(f.matches(&CanFrame::new_extended(0x1FFFFFFF, &[])));
        assert!(!f.matches(&CanFrame::new_extended(0x1FFFFFFE, &[])));
    }

    #[test]
    fn test_filter_clone_eq() {
        let f1 = CanFilter::match_exact(0x123, false);
        let f2 = f1.clone();
        assert_eq!(f1, f2);
    }

    #[test]
    fn test_remote_frame_filtering() {
        // 远程帧也应能被过滤
        let f = CanFilter::match_exact(0x100, false);
        assert!(f.matches(&CanFrame::new_remote(CanId::Standard(0x100))));
        assert!(!f.matches(&CanFrame::new_remote(CanId::Standard(0x101))));
    }
}
