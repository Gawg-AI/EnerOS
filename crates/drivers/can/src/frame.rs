//! CAN 帧结构定义（v0.47.0）.
//!
//! 定义 CAN 2.0A/B 帧结构，支持标准帧（11-bit ID）与扩展帧（29-bit ID）。
//!
//! # 偏差声明
//! - D3: `CanFrame` 不含 `timestamp: MonotonicTime` 字段（EnerOS 无 `MonotonicTime` 类型）。
//!   时间戳由应用层注入。

use alloc::vec::Vec;

/// CAN 帧类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// 数据帧
    Data,
    /// 远程帧
    Remote,
    /// 错误帧
    Error,
    /// 过载帧
    Overload,
}

/// CAN 标识符
///
/// - `Standard(u16)`: 11-bit 标准帧 ID（范围 0x000~0x7FF）
/// - `Extended(u32)`: 29-bit 扩展帧 ID（范围 0x00000000~0x1FFFFFFF）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanId {
    /// 标准帧 ID（11-bit，掩码 0x7FF）
    Standard(u16),
    /// 扩展帧 ID（29-bit，掩码 0x1FFFFFFF）
    Extended(u32),
}

impl CanId {
    /// 是否为扩展帧
    pub fn is_extended(&self) -> bool {
        matches!(self, CanId::Extended(_))
    }

    /// 返回原始 ID 值（`u32`）
    ///
    /// - `Standard(v)` → `v as u32`
    /// - `Extended(v)` → `v`
    pub fn raw(&self) -> u32 {
        match self {
            CanId::Standard(v) => *v as u32,
            CanId::Extended(v) => *v,
        }
    }

    /// 从标准 ID 构造（自动掩码 0x7FF）
    pub fn standard(id: u16) -> Self {
        CanId::Standard(id & 0x7FF)
    }

    /// 从扩展 ID 构造（自动掩码 0x1FFFFFFF）
    pub fn extended(id: u32) -> Self {
        CanId::Extended(id & 0x1FFFFFFF)
    }
}

/// CAN 帧
///
/// CAN 2.0A/B 帧结构，包含标识符、帧类型、数据（0~8 字节）和 DLC。
/// 不含时间戳字段（D3 偏差）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanFrame {
    /// 帧标识符
    pub id: CanId,
    /// 帧类型
    pub frame_type: FrameType,
    /// 数据负载（0~8 字节）
    pub data: Vec<u8>,
    /// 数据长度码
    pub dlc: u8,
}

impl CanFrame {
    /// 创建标准数据帧
    ///
    /// # 参数
    /// - `id`: 11-bit 标准帧 ID（自动掩码 0x7FF）
    /// - `data`: 数据负载（最多 8 字节）
    pub fn new_standard(id: u16, data: &[u8]) -> Self {
        Self {
            id: CanId::standard(id),
            frame_type: FrameType::Data,
            data: Vec::from(data),
            dlc: data.len() as u8,
        }
    }

    /// 创建扩展数据帧
    ///
    /// # 参数
    /// - `id`: 29-bit 扩展帧 ID（自动掩码 0x1FFFFFFF）
    /// - `data`: 数据负载（最多 8 字节）
    pub fn new_extended(id: u32, data: &[u8]) -> Self {
        Self {
            id: CanId::extended(id),
            frame_type: FrameType::Data,
            data: Vec::from(data),
            dlc: data.len() as u8,
        }
    }

    /// 创建远程帧
    ///
    /// 远程帧无数据负载，`dlc` 为 0。
    ///
    /// # 参数
    /// - `id`: 帧 ID（标准或扩展）
    pub fn new_remote(id: CanId) -> Self {
        Self {
            id,
            frame_type: FrameType::Remote,
            data: Vec::new(),
            dlc: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== T3.1: FrameType 测试 =====

    #[test]
    fn test_frame_type_variants() {
        let types = [
            FrameType::Data,
            FrameType::Remote,
            FrameType::Error,
            FrameType::Overload,
        ];
        // 4 个变体两两不相等
        for i in 0..types.len() {
            for j in (i + 1)..types.len() {
                assert_ne!(types[i], types[j]);
            }
        }
        // Copy 语义
        let t = FrameType::Data;
        let t_copy = t;
        assert_eq!(t, t_copy);
    }

    // ===== T3.2: CanId 测试 =====

    #[test]
    fn test_can_id_standard() {
        let id = CanId::Standard(0x123);
        assert!(!id.is_extended());
        assert_eq!(id.raw(), 0x123);
    }

    #[test]
    fn test_can_id_extended() {
        let id = CanId::Extended(0x1FFFFFFF);
        assert!(id.is_extended());
        assert_eq!(id.raw(), 0x1FFFFFFF);
    }

    #[test]
    fn test_can_id_standard_masking() {
        // 0xFFFF & 0x7FF == 0x7FF
        let id = CanId::standard(0xFFFF);
        assert_eq!(id, CanId::Standard(0x7FF));
    }

    #[test]
    fn test_can_id_extended_masking() {
        // 0xFFFFFFFF & 0x1FFFFFFF == 0x1FFFFFFF
        let id = CanId::extended(0xFFFFFFFF);
        assert_eq!(id, CanId::Extended(0x1FFFFFFF));
    }

    #[test]
    fn test_can_id_raw_standard_as_u32() {
        let id = CanId::Standard(0x7FF);
        assert_eq!(id.raw(), 0x7FF_u32);
    }

    // ===== T3.3~T3.4: CanFrame::new_standard/new_extended 测试 =====

    #[test]
    fn test_new_standard_frame() {
        let frame = CanFrame::new_standard(0x123, &[0x01, 0x02]);
        assert_eq!(frame.id, CanId::Standard(0x123));
        assert_eq!(frame.frame_type, FrameType::Data);
        assert_eq!(frame.data, vec![0x01, 0x02]);
        assert_eq!(frame.dlc, 2);
    }

    #[test]
    fn test_new_extended_frame() {
        let frame = CanFrame::new_extended(0x1FFFFFFF, &[0xAA]);
        assert_eq!(frame.id, CanId::Extended(0x1FFFFFFF));
        assert_eq!(frame.frame_type, FrameType::Data);
        assert_eq!(frame.data, vec![0xAA]);
        assert_eq!(frame.dlc, 1);
    }

    #[test]
    fn test_standard_id_masking_in_constructor() {
        // 0xFFFF & 0x7FF == 0x7FF
        let frame = CanFrame::new_standard(0xFFFF, &[]);
        assert_eq!(frame.id, CanId::Standard(0x7FF));
    }

    #[test]
    fn test_extended_id_masking_in_constructor() {
        // 0xFFFFFFFF & 0x1FFFFFFF == 0x1FFFFFFF
        let frame = CanFrame::new_extended(0xFFFFFFFF, &[]);
        assert_eq!(frame.id, CanId::Extended(0x1FFFFFFF));
    }

    // ===== T3.5: CanFrame::new_remote 测试 =====

    #[test]
    fn test_new_remote_standard() {
        let frame = CanFrame::new_remote(CanId::Standard(0x100));
        assert_eq!(frame.id, CanId::Standard(0x100));
        assert_eq!(frame.frame_type, FrameType::Remote);
        assert!(frame.data.is_empty());
        assert_eq!(frame.dlc, 0);
    }

    #[test]
    fn test_new_remote_extended() {
        let frame = CanFrame::new_remote(CanId::Extended(0x12345678));
        assert_eq!(frame.id, CanId::Extended(0x12345678));
        assert_eq!(frame.frame_type, FrameType::Remote);
        assert!(frame.data.is_empty());
        assert_eq!(frame.dlc, 0);
    }

    // ===== 数据长度边界测试 =====

    #[test]
    fn test_zero_byte_data() {
        let frame = CanFrame::new_standard(0x100, &[]);
        assert_eq!(frame.dlc, 0);
        assert!(frame.data.is_empty());
    }

    #[test]
    fn test_eight_byte_data() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        let frame = CanFrame::new_standard(0x100, &data);
        assert_eq!(frame.dlc, 8);
        assert_eq!(frame.data, data.to_vec());
    }

    #[test]
    fn test_frame_clone_eq() {
        let f1 = CanFrame::new_standard(0x123, &[0x01, 0x02]);
        let f2 = f1.clone();
        assert_eq!(f1, f2);
    }
}
