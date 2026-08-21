//! ASDU 公共层复用桥（蓝图 48 WP-48.0/WP-48.2）.
//!
//! 101 与 104 共用应用层——本模块证明复用 seam：ASDU 经 WP-48.0 抽取的
//! `eneros-iec60870-asdu` 构造/解析（字节布局断言与 104 侧同一实现、
//! 同一测试族）。链路层（FT1.2）BLOCKED——桥的输出即未来 FT1.2 帧的
//! payload 接缝。

use alloc::vec::Vec;

use eneros_iec60870_asdu::{Asdu, Cot, InformationObject, IoValue, QualityDescriptor, TypeId};

/// 构造总召唤命令（C_IC NA(2)，TypeId=100，COT=6 激活）。
///
/// 应用层语义经共用 crate（104 在库测试覆盖）；101 特有 M_IT 计量召唤
/// 属规约补充——BLOCKED 待文本。
pub fn interrogation_request(common_addr: u16) -> Asdu {
    Asdu {
        type_id: TypeId::InterrogationCommand,
        cause_of_tx: Cot::Activation,
        common_addr,
        ioas: alloc::vec![InformationObject {
            ioa: 0,
            value: IoValue::SinglePoint(eneros_iec60870_asdu::SinglePointValue::Off),
            quality: QualityDescriptor::good(),
            time_tag: None,
        }],
    }
}

/// 编码为 ASDU 字节流（FT1.2 可变帧 payload 接缝；帧封装 BLOCKED）。
pub fn encode_application_layer(asdu: &Asdu) -> Vec<u8> {
    asdu.encode()
}

/// 解析 ASDU 字节流（错误类型 = 共用 crate 的 [`eneros_iec60870_asdu::AsduError`]）。
pub fn decode_application_layer(bytes: &[u8]) -> Result<Asdu, eneros_iec60870_asdu::AsduError> {
    Asdu::decode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 复用 seam：编码字节布局逐字节断言（与 104 侧同一实现）。
    ///
    /// 布局 = TypeId(1)|VSQ(1)|COT(1)|Originator(1)|CommonAddr(2 LE)|IOA(3 LE)
    /// （`Asdu::encode` 文档登记；104 在库 KAT 覆盖）。
    #[test]
    fn interrogation_roundtrip_via_shared_crate() {
        let asdu = interrogation_request(0x1234);
        let bytes = encode_application_layer(&asdu);
        assert_eq!(bytes[0], 100); // TypeId::InterrogationCommand
        assert_eq!(bytes[1], 1); // VSQ: SQ=0, 对象数=1
        assert_eq!(bytes[2], 6); // COT=激活
        assert_eq!(bytes[3], 0); // OriginatorAddr
        assert_eq!(&bytes[4..6], &[0x34, 0x12]); // CommonAddr LE
                                                 // IOA 3 字节 LE（值 0）。
        assert_eq!(&bytes[6..9], &[0x00, 0x00, 0x00]);
        // QOI 限定符（总召唤限定词，InformationObject::encode 命令类体）。
        let decoded = decode_application_layer(&bytes).expect("decode");
        assert_eq!(decoded.type_id, TypeId::InterrogationCommand);
        assert_eq!(decoded.cause_of_tx, Cot::Activation);
        assert_eq!(decoded.common_addr, 0x1234);
    }

    /// 坏类型标识 → 共用 crate 错误（AsduError::InvalidTypeId）。
    #[test]
    fn invalid_type_id_maps_to_shared_error() {
        let bytes = [0xFE, 0x01, 0x06, 0x00, 0x01, 0x00];
        assert_eq!(
            decode_application_layer(&bytes),
            Err(eneros_iec60870_asdu::AsduError::InvalidTypeId)
        );
    }

    /// 截断 → AsduError::Decode。
    #[test]
    fn truncated_input_is_decode_error() {
        assert_eq!(
            decode_application_layer(&[100, 1]),
            Err(eneros_iec60870_asdu::AsduError::Decode)
        );
    }
}
