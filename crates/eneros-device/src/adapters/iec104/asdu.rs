//! IEC 60870-5-104 ASDU (Application Service Data Unit) types and parser.
//!
//! Supports the most common ASDU types used in Chinese power grid SCADA:
//! - M_SP_NA_1 (Type 1): Single-point information without time tag
//! - M_ME_NC_1 (Type 13): Measured value, short floating point without time tag
//! - M_DP_TB_1 (Type 31): Double-point information with CP56Time2a time tag
//!
//! Reference: IEC 60870-5-101/104

use std::fmt;

/// ASDU type identifier (TI)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeId {
    /// M_SP_NA_1 — Single-point information (Type 1)
    SinglePoint,
    /// M_SP_TB_1 — Single-point with CP56Time2a (Type 30)
    SinglePointTimeTag,
    /// M_DP_TB_1 — Double-point with CP56Time2a (Type 31)
    DoublePointTimeTag,
    /// M_ME_NC_1 — Measured value, short float (Type 13)
    MeasuredShortFloat,
    /// M_ME_TF_1 — Measured value, short float with CP56Time2a (Type 36)
    MeasuredShortFloatTimeTag,
    /// C_IC_NA_1 — Interrogation command (Type 100)
    InterrogationCommand,
    /// C_SC_NA_1 — Single command (Type 45)
    SingleCommand,
    /// C_SE_NC_1 — Setpoint command, short float (Type 50)
    SetpointShortFloat,
    /// Unknown type
    Unknown(u8),
}

impl TypeId {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => TypeId::SinglePoint,
            13 => TypeId::MeasuredShortFloat,
            30 => TypeId::SinglePointTimeTag,
            31 => TypeId::DoublePointTimeTag,
            36 => TypeId::MeasuredShortFloatTimeTag,
            45 => TypeId::SingleCommand,
            50 => TypeId::SetpointShortFloat,
            100 => TypeId::InterrogationCommand,
            _ => TypeId::Unknown(val),
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            TypeId::SinglePoint => 1,
            TypeId::MeasuredShortFloat => 13,
            TypeId::SinglePointTimeTag => 30,
            TypeId::DoublePointTimeTag => 31,
            TypeId::MeasuredShortFloatTimeTag => 36,
            TypeId::SingleCommand => 45,
            TypeId::SetpointShortFloat => 50,
            TypeId::InterrogationCommand => 100,
            TypeId::Unknown(v) => *v,
        }
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeId::SinglePoint => write!(f, "M_SP_NA_1(1)"),
            TypeId::MeasuredShortFloat => write!(f, "M_ME_NC_1(13)"),
            TypeId::SinglePointTimeTag => write!(f, "M_SP_TB_1(30)"),
            TypeId::DoublePointTimeTag => write!(f, "M_DP_TB_1(31)"),
            TypeId::MeasuredShortFloatTimeTag => write!(f, "M_ME_TF_1(36)"),
            TypeId::SingleCommand => write!(f, "C_SC_NA_1(45)"),
            TypeId::SetpointShortFloat => write!(f, "C_SE_NC_1(50)"),
            TypeId::InterrogationCommand => write!(f, "C_IC_NA_1(100)"),
            TypeId::Unknown(v) => write!(f, "Unknown({})", v),
        }
    }
}

/// Single-point information quality descriptor (SIQ)
#[derive(Debug, Clone, Copy)]
pub struct SinglePointQuality {
    pub spi: bool,
    pub bl: bool,
    pub sb: bool,
    pub nt: bool,
    pub iv: bool,
}

impl SinglePointQuality {
    pub fn from_u8(val: u8) -> Self {
        Self {
            spi: (val & 0x01) != 0,
            bl: (val & 0x10) != 0,
            sb: (val & 0x20) != 0,
            nt: (val & 0x40) != 0,
            iv: (val & 0x80) != 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.iv && !self.nt
    }
}

/// Double-point information quality descriptor (DIQ)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoublePointValue {
    Indeterminate = 0,
    Off = 1,
    On = 2,
    Indeterminate2 = 3,
}

impl DoublePointValue {
    pub fn from_u8(val: u8) -> Self {
        match val & 0x03 {
            0 => DoublePointValue::Indeterminate,
            1 => DoublePointValue::Off,
            2 => DoublePointValue::On,
            _ => DoublePointValue::Indeterminate2,
        }
    }
}

/// Measured value quality descriptor (QDS)
#[derive(Debug, Clone, Copy)]
pub struct MeasuredQuality {
    pub ov: bool,
    pub bl: bool,
    pub sb: bool,
    pub nt: bool,
    pub iv: bool,
}

impl MeasuredQuality {
    pub fn from_u8(val: u8) -> Self {
        Self {
            ov: (val & 0x01) != 0,
            bl: (val & 0x10) != 0,
            sb: (val & 0x20) != 0,
            nt: (val & 0x40) != 0,
            iv: (val & 0x80) != 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.iv && !self.nt && !self.ov
    }
}

/// Cause of transmission (COT)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CauseOfTransmission {
    Periodic,
    Background,
    Spontaneous,
    Initialized,
    Request,
    Activation,
    ActivationConfirmation,
    Deactivation,
    DeactivationConfirmation,
    InterrogatedByStation,
    Unknown(u8),
}

impl CauseOfTransmission {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => CauseOfTransmission::Periodic,
            2 => CauseOfTransmission::Background,
            3 => CauseOfTransmission::Spontaneous,
            4 => CauseOfTransmission::Initialized,
            5 => CauseOfTransmission::Request,
            6 => CauseOfTransmission::Activation,
            7 => CauseOfTransmission::ActivationConfirmation,
            8 => CauseOfTransmission::Deactivation,
            9 => CauseOfTransmission::DeactivationConfirmation,
            10 => CauseOfTransmission::InterrogatedByStation,
            _ => CauseOfTransmission::Unknown(val),
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            CauseOfTransmission::Periodic => 1,
            CauseOfTransmission::Background => 2,
            CauseOfTransmission::Spontaneous => 3,
            CauseOfTransmission::Initialized => 4,
            CauseOfTransmission::Request => 5,
            CauseOfTransmission::Activation => 6,
            CauseOfTransmission::ActivationConfirmation => 7,
            CauseOfTransmission::Deactivation => 8,
            CauseOfTransmission::DeactivationConfirmation => 9,
            CauseOfTransmission::InterrogatedByStation => 10,
            CauseOfTransmission::Unknown(v) => *v,
        }
    }
}

/// Parsed information object from an ASDU
#[derive(Debug, Clone)]
pub enum InformationObject {
    SinglePoint {
        ioa: u32,
        value: bool,
        quality: SinglePointQuality,
    },
    SinglePointTimeTag {
        ioa: u32,
        value: bool,
        quality: SinglePointQuality,
        timestamp_ms: u64,
    },
    DoublePointTimeTag {
        ioa: u32,
        value: DoublePointValue,
        quality: MeasuredQuality,
        timestamp_ms: u64,
    },
    MeasuredShortFloat {
        ioa: u32,
        value: f32,
        quality: MeasuredQuality,
    },
    MeasuredShortFloatTimeTag {
        ioa: u32,
        value: f32,
        quality: MeasuredQuality,
        timestamp_ms: u64,
    },
}

impl InformationObject {
    pub fn ioa(&self) -> u32 {
        match self {
            InformationObject::SinglePoint { ioa, .. } => *ioa,
            InformationObject::SinglePointTimeTag { ioa, .. } => *ioa,
            InformationObject::DoublePointTimeTag { ioa, .. } => *ioa,
            InformationObject::MeasuredShortFloat { ioa, .. } => *ioa,
            InformationObject::MeasuredShortFloatTimeTag { ioa, .. } => *ioa,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            InformationObject::MeasuredShortFloat { value, .. } => Some(*value as f64),
            InformationObject::MeasuredShortFloatTimeTag { value, .. } => Some(*value as f64),
            InformationObject::SinglePoint { value, .. } => Some(if *value { 1.0 } else { 0.0 }),
            InformationObject::SinglePointTimeTag { value, .. } => Some(if *value { 1.0 } else { 0.0 }),
            InformationObject::DoublePointTimeTag { value, .. } => match value {
                DoublePointValue::On => Some(1.0),
                DoublePointValue::Off => Some(0.0),
                _ => None,
            },
        }
    }

    pub fn is_valid(&self) -> bool {
        match self {
            InformationObject::SinglePoint { quality, .. } => quality.is_valid(),
            InformationObject::SinglePointTimeTag { quality, .. } => quality.is_valid(),
            InformationObject::DoublePointTimeTag { quality, .. } => quality.is_valid(),
            InformationObject::MeasuredShortFloat { quality, .. } => quality.is_valid(),
            InformationObject::MeasuredShortFloatTimeTag { quality, .. } => quality.is_valid(),
        }
    }
}

/// Parsed ASDU
#[derive(Debug, Clone)]
pub struct Asdu {
    pub type_id: TypeId,
    pub num_objects: u8,
    pub cot: CauseOfTransmission,
    pub oa: u8,
    pub asdu_address: u16,
    pub objects: Vec<InformationObject>,
}

fn parse_ioa(buf: &[u8], offset: usize) -> Option<(u32, usize)> {
    if offset + 3 > buf.len() { return None; }
    let ioa = buf[offset] as u32
        | (buf[offset + 1] as u32) << 8
        | (buf[offset + 2] as u32) << 16;
    Some((ioa, offset + 3))
}

fn parse_cp56time2a(buf: &[u8], offset: usize) -> Option<(u64, usize)> {
    if offset + 7 > buf.len() { return None; }
    let ms_of_day = buf[offset] as u32
        | ((buf[offset + 1] as u32) << 8)
        | ((buf[offset + 2] as u32 & 0x0F) << 16);
    let day = (buf[offset + 4] & 0x1F) as u32;
    let timestamp_ms = day as u64 * 86_400_000 + ms_of_day as u64;
    Some((timestamp_ms, offset + 7))
}

/// Parse an ASDU from raw bytes (after the APCI header).
pub fn parse_asdu(buf: &[u8]) -> Option<Asdu> {
    if buf.len() < 6 { return None; }

    let type_id = TypeId::from_u8(buf[0]);
    let sq_and_num = buf[1];
    let num_objects = sq_and_num & 0x7F;
    let cot = CauseOfTransmission::from_u8(buf[2] & 0x3F);
    let oa = buf[3];
    let asdu_address = buf[4] as u16 | ((buf[5] as u16) << 8);

    let mut objects = Vec::with_capacity(num_objects as usize);
    let mut offset = 6;

    for _ in 0..num_objects {
        let obj = match type_id {
            TypeId::SinglePoint => {
                let (ioa, new_off) = parse_ioa(buf, offset)?;
                offset = new_off;
                if offset >= buf.len() { return None; }
                let q = SinglePointQuality::from_u8(buf[offset]);
                offset += 1;
                InformationObject::SinglePoint { ioa, value: q.spi, quality: q }
            }
            TypeId::SinglePointTimeTag => {
                let (ioa, new_off) = parse_ioa(buf, offset)?;
                offset = new_off;
                if offset >= buf.len() { return None; }
                let q = SinglePointQuality::from_u8(buf[offset]);
                offset += 1;
                let (ts, new_off) = parse_cp56time2a(buf, offset)?;
                offset = new_off;
                InformationObject::SinglePointTimeTag { ioa, value: q.spi, quality: q, timestamp_ms: ts }
            }
            TypeId::DoublePointTimeTag => {
                let (ioa, new_off) = parse_ioa(buf, offset)?;
                offset = new_off;
                if offset >= buf.len() { return None; }
                let diq = buf[offset];
                let dpv = DoublePointValue::from_u8(diq & 0x03);
                let quality = MeasuredQuality::from_u8(diq & 0xF0);
                offset += 1;
                let (ts, new_off) = parse_cp56time2a(buf, offset)?;
                offset = new_off;
                InformationObject::DoublePointTimeTag { ioa, value: dpv, quality, timestamp_ms: ts }
            }
            TypeId::MeasuredShortFloat => {
                let (ioa, new_off) = parse_ioa(buf, offset)?;
                offset = new_off;
                if offset + 4 > buf.len() { return None; }
                let value = f32::from_le_bytes([buf[offset], buf[offset+1], buf[offset+2], buf[offset+3]]);
                offset += 4;
                if offset >= buf.len() { return None; }
                let quality = MeasuredQuality::from_u8(buf[offset]);
                offset += 1;
                InformationObject::MeasuredShortFloat { ioa, value, quality }
            }
            TypeId::MeasuredShortFloatTimeTag => {
                let (ioa, new_off) = parse_ioa(buf, offset)?;
                offset = new_off;
                if offset + 4 > buf.len() { return None; }
                let value = f32::from_le_bytes([buf[offset], buf[offset+1], buf[offset+2], buf[offset+3]]);
                offset += 4;
                if offset >= buf.len() { return None; }
                let quality = MeasuredQuality::from_u8(buf[offset]);
                offset += 1;
                let (ts, new_off) = parse_cp56time2a(buf, offset)?;
                offset = new_off;
                InformationObject::MeasuredShortFloatTimeTag { ioa, value, quality, timestamp_ms: ts }
            }
            _ => break,
        };
        objects.push(obj);
    }

    Some(Asdu { type_id, num_objects: objects.len() as u8, cot, oa, asdu_address, objects })
}

/// Build an interrogation command (C_IC_NA_1) ASDU.
pub fn build_interrogation_command(asdu_address: u16, ioa: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(12);
    buf.push(TypeId::InterrogationCommand.to_u8());
    buf.push(0x01);
    buf.push(CauseOfTransmission::Activation.to_u8());
    buf.push(0x00);
    buf.push((asdu_address & 0xFF) as u8);
    buf.push(((asdu_address >> 8) & 0xFF) as u8);
    buf.push((ioa & 0xFF) as u8);
    buf.push(((ioa >> 8) & 0xFF) as u8);
    buf.push(((ioa >> 16) & 0xFF) as u8);
    buf.push(0x14); // station interrogation
    buf.push(0x00);
    buf.push(0x00);
    buf
}

/// Build a single command (C_SC_NA_1) ASDU for controlling a single-point.
pub fn build_single_command(asdu_address: u16, ioa: u32, value: bool, qu: u8, s_e: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    buf.push(TypeId::SingleCommand.to_u8()); // TI = 45
    buf.push(0x01); // SQ=0, Num=1
    buf.push(CauseOfTransmission::Activation.to_u8()); // COT = Activation
    buf.push(0x00); // OA
    buf.push((asdu_address & 0xFF) as u8);
    buf.push(((asdu_address >> 8) & 0xFF) as u8);
    // IOA (3 bytes)
    buf.push((ioa & 0xFF) as u8);
    buf.push(((ioa >> 8) & 0xFF) as u8);
    buf.push(((ioa >> 16) & 0xFF) as u8);
    // SCO: SCS(bit0) + QU(bit1-6) + S/E(bit7)
    let sco = (if value { 0x01 } else { 0x00 }) | ((qu & 0x1F) << 1) | (if s_e { 0x80 } else { 0x00 });
    buf.push(sco);
    buf
}

/// Build a setpoint short float command (C_SE_NC_1) ASDU.
pub fn build_setpoint_short_float(asdu_address: u16, ioa: u32, value: f32, qu: u8, s_e: bool) -> Vec<u8> {
    let value_bytes = value.to_le_bytes();
    let mut buf = Vec::with_capacity(14);
    buf.push(TypeId::SetpointShortFloat.to_u8()); // TI = 50
    buf.push(0x01); // SQ=0, Num=1
    buf.push(CauseOfTransmission::Activation.to_u8()); // COT = Activation
    buf.push(0x00); // OA
    buf.push((asdu_address & 0xFF) as u8);
    buf.push(((asdu_address >> 8) & 0xFF) as u8);
    // IOA (3 bytes)
    buf.push((ioa & 0xFF) as u8);
    buf.push(((ioa >> 8) & 0xFF) as u8);
    buf.push(((ioa >> 16) & 0xFF) as u8);
    // Value (4 bytes IEEE 754)
    buf.extend_from_slice(&value_bytes);
    // QOS: QU(bit0-5) + S/E(bit7)
    let qos = (qu & 0x1F) | (if s_e { 0x80 } else { 0x00 });
    buf.push(qos);
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_id_from_u8() {
        assert_eq!(TypeId::from_u8(1), TypeId::SinglePoint);
        assert_eq!(TypeId::from_u8(13), TypeId::MeasuredShortFloat);
        assert_eq!(TypeId::from_u8(30), TypeId::SinglePointTimeTag);
        assert_eq!(TypeId::from_u8(31), TypeId::DoublePointTimeTag);
        assert_eq!(TypeId::from_u8(36), TypeId::MeasuredShortFloatTimeTag);
        assert_eq!(TypeId::from_u8(45), TypeId::SingleCommand);
        assert_eq!(TypeId::from_u8(50), TypeId::SetpointShortFloat);
        assert_eq!(TypeId::from_u8(100), TypeId::InterrogationCommand);
        assert_eq!(TypeId::from_u8(99), TypeId::Unknown(99));
    }

    #[test]
    fn test_parse_m_sp_na_1() {
        let buf: Vec<u8> = vec![
            0x01, 0x01, 0x03, 0x00, 0x01, 0x00,
            0x64, 0x00, 0x00, 0x01,
        ];
        let asdu = parse_asdu(&buf).unwrap();
        assert_eq!(asdu.type_id, TypeId::SinglePoint);
        assert_eq!(asdu.num_objects, 1);
    }

    #[test]
    fn test_parse_m_me_nc_1() {
        let value: f32 = 1.045;
        let value_bytes = value.to_le_bytes();
        let buf: Vec<u8> = vec![
            0x0D, 0x01, 0x01, 0x00, 0x01, 0x00,
            0xC8, 0x00, 0x00,
            value_bytes[0], value_bytes[1], value_bytes[2], value_bytes[3],
            0x00,
        ];
        let asdu = parse_asdu(&buf).unwrap();
        assert_eq!(asdu.type_id, TypeId::MeasuredShortFloat);
    }

    #[test]
    fn test_parse_too_short() {
        assert!(parse_asdu(&[]).is_none());
        assert!(parse_asdu(&[0x01]).is_none());
    }

    #[test]
    fn test_build_interrogation_command() {
        let cmd = build_interrogation_command(1, 0);
        assert_eq!(cmd[0], 100);
        assert_eq!(cmd[9], 0x14);
    }

    #[test]
    fn test_build_single_command() {
        let cmd = build_single_command(1, 1001, true, 0, false);
        assert_eq!(cmd[0], 45); // TI = C_SC_NA_1
        assert_eq!(cmd[9] & 0x01, 1); // SCS = ON
    }

    #[test]
    fn test_build_setpoint_short_float() {
        let cmd = build_setpoint_short_float(1, 2001, 50.0f32, 0, false);
        assert_eq!(cmd[0], 50); // TI = C_SE_NC_1
    }

    #[test]
    fn test_information_object_as_float() {
        let obj = InformationObject::MeasuredShortFloat {
            ioa: 100, value: 1.045f32, quality: MeasuredQuality::from_u8(0),
        };
        assert!((obj.as_float().unwrap() - 1.045).abs() < 0.001);
    }

    #[test]
    fn test_cause_of_transmission() {
        assert_eq!(CauseOfTransmission::from_u8(3), CauseOfTransmission::Spontaneous);
        assert_eq!(CauseOfTransmission::from_u8(99), CauseOfTransmission::Unknown(99));
    }
}
