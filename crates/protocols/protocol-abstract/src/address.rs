//! 协议地址模型 — 统一表示 Modbus/IEC 104/CAN 三种协议的寻址.
//!
//! [`ProtocolAddress`] 将三种协议的异构地址结构归一化为单一枚举，
//! 便于在 [`crate::mapping::ProtocolPointMapping`] 中统一存储与匹配。

/// 协议地址（三协议统一表示）.
///
/// 派生 `Debug`/`Clone`/`PartialEq`/`Eq`，可在测试与映射表中精确比较。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolAddress {
    /// Modbus 地址（RTU/TCP 共用）：从站地址 + 寄存器地址 + 功能码。
    Modbus {
        /// 从站地址（1~247）。
        slave_addr: u8,
        /// 寄存器地址（0~65535）。
        reg_addr: u16,
        /// 功能码（1/2/3/4/5/6/15/16）。
        func_code: u8,
    },
    /// IEC 60870-5-104 地址：公共地址 + 信息对象地址 + 类型标识。
    Iec104 {
        /// 公共地址（ASDU common address）。
        common_addr: u16,
        /// 信息对象地址（IOA）。
        ioa: u16,
        /// 类型标识（TypeId）。
        type_id: u8,
    },
    /// CAN 总线地址：CAN ID + 起始字节 + 长度。
    Can {
        /// CAN 标识符（标准 11 位 / 扩展 29 位）。
        can_id: u32,
        /// 数据帧起始字节偏移。
        start_byte: u8,
        /// 数据长度（字节数）。
        length: u8,
    },
}
