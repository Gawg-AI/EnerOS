//! 寄存器到测点的映射与数据转换.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::ModbusError;

/// Modbus 数据类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModbusDataType {
    /// 无符号 16 位整数
    U16,
    /// 有符号 16 位整数
    I16,
    /// 无符号 32 位整数（大端：高字在前）
    U32,
    /// IEEE 754 单精度浮点（大端：高字在前）
    F32,
    /// 位（u16 中的指定位，0..15）
    Bit(u8),
}

/// 访问模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    /// 只读
    ReadOnly,
    /// 只写
    WriteOnly,
    /// 读写
    ReadWrite,
}

/// 寄存器到测点的映射
#[derive(Debug, Clone)]
pub struct RegToPoint {
    /// 测点 ID
    pub point_id: u32,
    /// 测点名称
    pub point_name: String,
    /// 从站地址
    pub slave_addr: u8,
    /// 寄存器地址
    pub reg_addr: u16,
    /// 数据类型
    pub data_type: ModbusDataType,
    /// 缩放系数（raw * scale + offset）
    pub scale: f64,
    /// 偏移量
    pub offset: f64,
    /// 访问模式
    pub access: AccessMode,
}

impl RegToPoint {
    /// 返回该测点占用的寄存器字数
    pub fn word_count(&self) -> u16 {
        match self.data_type {
            ModbusDataType::U16 | ModbusDataType::I16 | ModbusDataType::Bit(_) => 1,
            ModbusDataType::U32 | ModbusDataType::F32 => 2,
        }
    }

    /// 将原始寄存器值转换为工程值（raw * scale + offset）。
    ///
    /// - U32/F32 采用大端字序（高字在前）
    /// - F32 在 f32 域内应用 scale/offset 后再扩展到 f64
    /// - 使用 i64 承载原始值，正确处理 I16 的负数
    pub fn convert(&self, regs: &[u16]) -> Result<f64, ModbusError> {
        let raw: i64 = match self.data_type {
            ModbusDataType::U16 => {
                if regs.is_empty() {
                    return Err(ModbusError::UnexpectedResponse);
                }
                regs[0] as i64
            }
            ModbusDataType::I16 => {
                if regs.is_empty() {
                    return Err(ModbusError::UnexpectedResponse);
                }
                (regs[0] as i16) as i64
            }
            ModbusDataType::U32 => {
                if regs.len() < 2 {
                    return Err(ModbusError::UnexpectedResponse);
                }
                (((regs[0] as u32) << 16) | (regs[1] as u32)) as i64
            }
            ModbusDataType::F32 => {
                if regs.len() < 2 {
                    return Err(ModbusError::UnexpectedResponse);
                }
                let raw_u32 = ((regs[0] as u32) << 16) | (regs[1] as u32);
                let f = f32::from_bits(raw_u32);
                // 在 f32 域内应用 scale/offset，再扩展为 f64
                return Ok(f64::from(f * (self.scale as f32) + (self.offset as f32)));
            }
            ModbusDataType::Bit(bit_index) => {
                if regs.is_empty() {
                    return Err(ModbusError::UnexpectedResponse);
                }
                if bit_index >= 16 {
                    return Err(ModbusError::InvalidRegisterAddr);
                }
                ((regs[0] >> bit_index) & 1) as i64
            }
        };
        Ok((raw as f64) * self.scale + self.offset)
    }
}

/// 点表映射集合
#[derive(Debug, Clone)]
pub struct PointMapping {
    /// 映射列表
    pub mappings: Vec<RegToPoint>,
}

/// 按从站地址分组（D6）。
///
/// 返回 `[(slave_addr, [RegToPoint 引用])]` 列表，保持首次出现顺序。
pub fn group_by_slave(mappings: &[RegToPoint]) -> Vec<(u8, Vec<&RegToPoint>)> {
    let mut groups: Vec<(u8, Vec<&RegToPoint>)> = Vec::new();
    for m in mappings {
        if let Some(group) = groups.iter_mut().find(|(addr, _)| *addr == m.slave_addr) {
            group.1.push(m);
        } else {
            groups.push((m.slave_addr, Vec::from([m])));
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    fn make_point(data_type: ModbusDataType, scale: f64, offset: f64) -> RegToPoint {
        RegToPoint {
            point_id: 1,
            point_name: String::from("test"),
            slave_addr: 1,
            reg_addr: 0,
            data_type,
            scale,
            offset,
            access: AccessMode::ReadOnly,
        }
    }

    #[test]
    fn test_word_count() {
        assert_eq!(make_point(ModbusDataType::U16, 1.0, 0.0).word_count(), 1);
        assert_eq!(make_point(ModbusDataType::I16, 1.0, 0.0).word_count(), 1);
        assert_eq!(make_point(ModbusDataType::Bit(0), 1.0, 0.0).word_count(), 1);
        assert_eq!(make_point(ModbusDataType::U32, 1.0, 0.0).word_count(), 2);
        assert_eq!(make_point(ModbusDataType::F32, 1.0, 0.0).word_count(), 2);
    }

    #[test]
    fn test_convert_u16() {
        // raw=100, scale=0.1, offset=0 -> 10.0
        let p = make_point(ModbusDataType::U16, 0.1, 0.0);
        let v = p.convert(&[100u16]).unwrap();
        assert!((v - 10.0).abs() < 1e-9);
    }

    #[test]
    fn test_convert_u16_with_offset() {
        // raw=100, scale=0.1, offset=5.0 -> 15.0
        let p = make_point(ModbusDataType::U16, 0.1, 5.0);
        let v = p.convert(&[100u16]).unwrap();
        assert!((v - 15.0).abs() < 1e-9);
    }

    #[test]
    fn test_convert_i16_negative() {
        // raw=0xFFFF as i16 = -1, scale=1.0, offset=0 -> -1.0
        let p = make_point(ModbusDataType::I16, 1.0, 0.0);
        let v = p.convert(&[0xFFFFu16]).unwrap();
        assert!((v - (-1.0)).abs() < 1e-9);
    }

    #[test]
    fn test_convert_i16_large_negative() {
        // raw=0x8000 as i16 = -32768
        let p = make_point(ModbusDataType::I16, 0.01, 0.0);
        let v = p.convert(&[0x8000u16]).unwrap();
        assert!((v - (-327.68)).abs() < 1e-6);
    }

    #[test]
    fn test_convert_u32() {
        // raw=0x00010002 -> 65538, scale=1.0, offset=0 -> 65538.0
        let p = make_point(ModbusDataType::U32, 1.0, 0.0);
        let v = p.convert(&[0x0001u16, 0x0002u16]).unwrap();
        assert!((v - 65538.0).abs() < 1e-9);
    }

    #[test]
    fn test_convert_f32() {
        // 0x41A00000 = 20.0 (IEEE 754)
        let p = make_point(ModbusDataType::F32, 1.0, 0.0);
        let v = p.convert(&[0x41A0u16, 0x0000u16]).unwrap();
        assert!((v - 20.0).abs() < 1e-6);
    }

    #[test]
    fn test_convert_f32_with_scale_offset() {
        // 0x41A00000 = 20.0, scale=2.0, offset=1.0 -> 41.0
        let p = make_point(ModbusDataType::F32, 2.0, 1.0);
        let v = p.convert(&[0x41A0u16, 0x0000u16]).unwrap();
        assert!((v - 41.0).abs() < 1e-5);
    }

    #[test]
    fn test_convert_bit() {
        // raw=0x0006, bit 1 = 1, bit 2 = 1, bit 0 = 0
        let p0 = RegToPoint {
            data_type: ModbusDataType::Bit(0),
            ..make_point(ModbusDataType::Bit(0), 1.0, 0.0)
        };
        let p1 = RegToPoint {
            data_type: ModbusDataType::Bit(1),
            ..make_point(ModbusDataType::Bit(1), 1.0, 0.0)
        };
        let p2 = RegToPoint {
            data_type: ModbusDataType::Bit(2),
            ..make_point(ModbusDataType::Bit(2), 1.0, 0.0)
        };
        assert_eq!(p0.convert(&[0x0006u16]).unwrap(), 0.0);
        assert_eq!(p1.convert(&[0x0006u16]).unwrap(), 1.0);
        assert_eq!(p2.convert(&[0x0006u16]).unwrap(), 1.0);
    }

    #[test]
    fn test_convert_bit_invalid_index() {
        // bit index >= 16 应返回错误
        let p = make_point(ModbusDataType::Bit(16), 1.0, 0.0);
        assert_eq!(
            p.convert(&[0x0001u16]),
            Err(ModbusError::InvalidRegisterAddr)
        );
    }

    #[test]
    fn test_convert_empty_regs() {
        let p = make_point(ModbusDataType::U16, 1.0, 0.0);
        assert_eq!(p.convert(&[]), Err(ModbusError::UnexpectedResponse));

        let p32 = make_point(ModbusDataType::U32, 1.0, 0.0);
        assert_eq!(
            p32.convert(&[0x0001u16]),
            Err(ModbusError::UnexpectedResponse)
        );
    }

    #[test]
    fn test_group_by_slave() {
        let mappings = vec![
            RegToPoint {
                point_id: 1,
                point_name: String::from("a"),
                slave_addr: 1,
                reg_addr: 0,
                data_type: ModbusDataType::U16,
                scale: 1.0,
                offset: 0.0,
                access: AccessMode::ReadOnly,
            },
            RegToPoint {
                point_id: 2,
                point_name: String::from("b"),
                slave_addr: 2,
                reg_addr: 0,
                data_type: ModbusDataType::U16,
                scale: 1.0,
                offset: 0.0,
                access: AccessMode::ReadOnly,
            },
            RegToPoint {
                point_id: 3,
                point_name: String::from("c"),
                slave_addr: 1,
                reg_addr: 1,
                data_type: ModbusDataType::U16,
                scale: 1.0,
                offset: 0.0,
                access: AccessMode::ReadOnly,
            },
        ];
        let groups = group_by_slave(&mappings);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, 1);
        assert_eq!(groups[0].1.len(), 2);
        assert_eq!(groups[1].0, 2);
        assert_eq!(groups[1].1.len(), 1);
    }

    #[test]
    fn test_group_by_slave_empty() {
        let groups = group_by_slave(&[]);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_access_mode_variants() {
        assert_ne!(AccessMode::ReadOnly, AccessMode::WriteOnly);
        assert_ne!(AccessMode::WriteOnly, AccessMode::ReadWrite);
        assert_ne!(AccessMode::ReadOnly, AccessMode::ReadWrite);
    }

    #[test]
    fn test_data_type_variants() {
        assert_ne!(ModbusDataType::U16, ModbusDataType::I16);
        assert_ne!(ModbusDataType::U32, ModbusDataType::F32);
        assert_ne!(ModbusDataType::Bit(0), ModbusDataType::Bit(1));
        assert_ne!(ModbusDataType::Bit(0), ModbusDataType::U16);
    }
}
