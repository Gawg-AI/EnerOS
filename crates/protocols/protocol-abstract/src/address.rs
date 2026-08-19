//! 协议地址模型 — 统一表示 Modbus/IEC 104/CAN/MQTT/MMS 五种协议的寻址.
//!
//! [`ProtocolAddress`] 将多种协议的异构地址结构归一化为单一枚举，
//! 便于在 [`crate::mapping::ProtocolPointMapping`] 中统一存储与匹配。
//! MQTT/MMS 两变体为蓝图 45（点表数据模型）新增——点表 `addr.*` 直映本枚举，
//! 协议插件与配置源共享同一地址事实（禁止在配置层再造地址同义词）。

/// 协议地址（多协议统一表示）.
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
    /// MQTT 地址（45 点表新增）：主题（topic），协议插件按 topic 订阅 +
    /// payload 固定字段路径提取（禁自由 JSON 解析进 no_std 依赖）。
    Mqtt {
        /// 订阅主题（如 `plant/pv/power`）。
        topic: alloc::string::String,
    },
    /// IEC 61850 MMS 地址（45 点表新增）：对象引用（LD/LN/DO/DA 路径）。
    Mms {
        /// 对象引用路径（如 `LD0/LRPW.Analogue.MX.w`）。
        object_ref: alloc::string::String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 45 点表新增变体的构造/相等/克隆语义（映射表精确比较依赖）。
    #[test]
    fn mqtt_mms_variants_construct_and_compare() {
        let mqtt = ProtocolAddress::Mqtt {
            topic: alloc::string::String::from("plant/pv/power"),
        };
        assert!(matches!(mqtt, ProtocolAddress::Mqtt { ref topic } if topic == "plant/pv/power"));
        assert_eq!(
            mqtt,
            ProtocolAddress::Mqtt {
                topic: alloc::string::String::from("plant/pv/power")
            }
        );
        assert_eq!(mqtt.clone(), mqtt);

        let mms = ProtocolAddress::Mms {
            object_ref: alloc::string::String::from("LD0/LRPW.Analogue.MX.w"),
        };
        assert!(
            matches!(mms, ProtocolAddress::Mms { ref object_ref } if object_ref == "LD0/LRPW.Analogue.MX.w")
        );
        assert_eq!(
            mms,
            ProtocolAddress::Mms {
                object_ref: alloc::string::String::from("LD0/LRPW.Analogue.MX.w")
            }
        );
    }

    /// 新旧变体互不相等（地址路由不得串协议）。
    #[test]
    fn variants_are_distinct() {
        let mqtt = ProtocolAddress::Mqtt {
            topic: alloc::string::String::from("t"),
        };
        let mms = ProtocolAddress::Mms {
            object_ref: alloc::string::String::from("r"),
        };
        let modbus = ProtocolAddress::Modbus {
            slave_addr: 1,
            reg_addr: 0,
            func_code: 3,
        };
        assert_ne!(mqtt, mms);
        assert_ne!(mqtt, modbus);
        assert_ne!(mms, modbus);
    }
}
