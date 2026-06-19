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
    /// M_DP_NA_1 — Double-point information without time tag (Type 3)
    DoublePoint,
    /// M_DP_TB_1 — Double-point with CP56Time2a (Type 31)
    DoublePointTimeTag,
    /// M_ST_NA_1 — Step position information without time tag (Type 5)
    StepPosition,
    /// M_BO_NA_1 — Binary counter reading / BCR (Type 8)
    BinaryCounterReading,
    /// M_ME_NC_1 — Measured value, short float (Type 13)
    MeasuredShortFloat,
    /// M_ME_TF_1 — Measured value, short float with CP56Time2a (Type 36)
    MeasuredShortFloatTimeTag,
    /// C_IC_NA_1 — Interrogation command (Type 100)
    InterrogationCommand,
    /// C_SC_NA_1 — Single command (Type 45)
    SingleCommand,
    /// C_DC_NA_1 — Double command (Type 46)
    DoubleCommand,
    /// C_SE_NC_1 — Setpoint command, short float (Type 50)
    SetpointShortFloat,
    /// C_CS_NA_1 — Clock synchronization command (Type 103)
    ClockSynchronization,
    /// P_PM_NA_1 — Parameter measured value, short float (Type 112)
    ParameterFloat,
    /// P_PM_NI_1 — Parameter scaled value (Type 111)
    ParameterScaled,
    /// Unknown type
    Unknown(u8),
}

impl TypeId {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => TypeId::SinglePoint,
            3 => TypeId::DoublePoint,
            5 => TypeId::StepPosition,
            8 => TypeId::BinaryCounterReading,
            13 => TypeId::MeasuredShortFloat,
            30 => TypeId::SinglePointTimeTag,
            31 => TypeId::DoublePointTimeTag,
            36 => TypeId::MeasuredShortFloatTimeTag,
            45 => TypeId::SingleCommand,
            46 => TypeId::DoubleCommand,
            50 => TypeId::SetpointShortFloat,
            100 => TypeId::InterrogationCommand,
            103 => TypeId::ClockSynchronization,
            111 => TypeId::ParameterScaled,
            112 => TypeId::ParameterFloat,
            _ => TypeId::Unknown(val),
        }
    }

    pub fn to_u8(&self) -> u8 {
        match self {
            TypeId::SinglePoint => 1,
            TypeId::DoublePoint => 3,
            TypeId::StepPosition => 5,
            TypeId::BinaryCounterReading => 8,
            TypeId::MeasuredShortFloat => 13,
            TypeId::SinglePointTimeTag => 30,
            TypeId::DoublePointTimeTag => 31,
            TypeId::MeasuredShortFloatTimeTag => 36,
            TypeId::SingleCommand => 45,
            TypeId::DoubleCommand => 46,
            TypeId::SetpointShortFloat => 50,
            TypeId::InterrogationCommand => 100,
            TypeId::ClockSynchronization => 103,
            TypeId::ParameterScaled => 111,
            TypeId::ParameterFloat => 112,
            TypeId::Unknown(v) => *v,
        }
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeId::SinglePoint => write!(f, "M_SP_NA_1(1)"),
            TypeId::DoublePoint => write!(f, "M_DP_NA_1(3)"),
            TypeId::StepPosition => write!(f, "M_ST_NA_1(5)"),
            TypeId::BinaryCounterReading => write!(f, "M_BO_NA_1(8)"),
            TypeId::MeasuredShortFloat => write!(f, "M_ME_NC_1(13)"),
            TypeId::SinglePointTimeTag => write!(f, "M_SP_TB_1(30)"),
            TypeId::DoublePointTimeTag => write!(f, "M_DP_TB_1(31)"),
            TypeId::MeasuredShortFloatTimeTag => write!(f, "M_ME_TF_1(36)"),
            TypeId::SingleCommand => write!(f, "C_SC_NA_1(45)"),
            TypeId::DoubleCommand => write!(f, "C_DC_NA_1(46)"),
            TypeId::SetpointShortFloat => write!(f, "C_SE_NC_1(50)"),
            TypeId::InterrogationCommand => write!(f, "C_IC_NA_1(100)"),
            TypeId::ClockSynchronization => write!(f, "C_CS_NA_1(103)"),
            TypeId::ParameterScaled => write!(f, "P_PM_NI_1(111)"),
            TypeId::ParameterFloat => write!(f, "P_PM_NA_1(112)"),
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
    DoublePoint {
        ioa: u32,
        value: DoublePointValue,
        quality: MeasuredQuality,
    },
    DoublePointTimeTag {
        ioa: u32,
        value: DoublePointValue,
        quality: MeasuredQuality,
        timestamp_ms: u64,
    },
    StepPosition {
        ioa: u32,
        value: i8,
        transient: bool,
        quality: MeasuredQuality,
    },
    BinaryCounterReading {
        ioa: u32,
        counter: u32,
        sequence: u8,
        carry: bool,
        invalid: bool,
        adjusted: bool,
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
            InformationObject::DoublePoint { ioa, .. } => *ioa,
            InformationObject::DoublePointTimeTag { ioa, .. } => *ioa,
            InformationObject::StepPosition { ioa, .. } => *ioa,
            InformationObject::BinaryCounterReading { ioa, .. } => *ioa,
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
            InformationObject::DoublePoint { value, .. } => match value {
                DoublePointValue::On => Some(1.0),
                DoublePointValue::Off => Some(0.0),
                _ => None,
            },
            InformationObject::DoublePointTimeTag { value, .. } => match value {
                DoublePointValue::On => Some(1.0),
                DoublePointValue::Off => Some(0.0),
                _ => None,
            },
            InformationObject::StepPosition { value, .. } => Some(*value as f64),
            InformationObject::BinaryCounterReading { counter, .. } => Some(*counter as f64),
        }
    }

    pub fn is_valid(&self) -> bool {
        match self {
            InformationObject::SinglePoint { quality, .. } => quality.is_valid(),
            InformationObject::SinglePointTimeTag { quality, .. } => quality.is_valid(),
            InformationObject::DoublePoint { quality, .. } => quality.is_valid(),
            InformationObject::DoublePointTimeTag { quality, .. } => quality.is_valid(),
            InformationObject::StepPosition { quality, .. } => quality.is_valid(),
            InformationObject::BinaryCounterReading { invalid, .. } => !*invalid,
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
            TypeId::DoublePoint => {
                let (ioa, new_off) = parse_ioa(buf, offset)?;
                offset = new_off;
                if offset >= buf.len() { return None; }
                let diq = buf[offset];
                let dpv = DoublePointValue::from_u8(diq & 0x03);
                let quality = MeasuredQuality::from_u8(diq & 0xF0);
                offset += 1;
                InformationObject::DoublePoint { ioa, value: dpv, quality }
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
            TypeId::StepPosition => {
                let (ioa, new_off) = parse_ioa(buf, offset)?;
                offset = new_off;
                if offset >= buf.len() { return None; }
                let vts = buf[offset]; // Value + transient + quality
                let value = ((vts & 0x7F) as i8) ^ 0x40; // 7-bit signed: -64..+63
                let value = value - 0x40; // Convert from offset binary
                let transient = (vts & 0x80) != 0;
                offset += 1;
                if offset >= buf.len() { return None; }
                let quality = MeasuredQuality::from_u8(buf[offset]);
                offset += 1;
                InformationObject::StepPosition { ioa, value, transient, quality }
            }
            TypeId::BinaryCounterReading => {
                let (ioa, new_off) = parse_ioa(buf, offset)?;
                offset = new_off;
                if offset + 5 > buf.len() { return None; }
                let counter = u32::from_le_bytes([buf[offset], buf[offset+1], buf[offset+2], buf[offset+3]]);
                let seq_carry = buf[offset + 4];
                let sequence = seq_carry & 0x1F;
                let carry = (seq_carry & 0x20) != 0;
                let invalid = (seq_carry & 0x40) != 0;
                let adjusted = (seq_carry & 0x80) != 0;
                offset += 5;
                InformationObject::BinaryCounterReading { ioa, counter, sequence, carry, invalid, adjusted }
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
    let buf = vec![
        TypeId::InterrogationCommand.to_u8(),
        0x01,
        CauseOfTransmission::Activation.to_u8(),
        0x00,
        (asdu_address & 0xFF) as u8,
        ((asdu_address >> 8) & 0xFF) as u8,
        (ioa & 0xFF) as u8,
        ((ioa >> 8) & 0xFF) as u8,
        ((ioa >> 16) & 0xFF) as u8,
        0x14, // station interrogation
        0x00,
        0x00,
    ];
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

/// Build a double command (C_DC_NA_1) ASDU for controlling a double-point device.
///
/// DCS values: 0=not permitted, 1=OFF, 2=ON, 3=not permitted
pub fn build_double_command(asdu_address: u16, ioa: u32, dcs: u8, qu: u8, s_e: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10);
    buf.push(TypeId::DoubleCommand.to_u8()); // TI = 46
    buf.push(0x01);
    buf.push(CauseOfTransmission::Activation.to_u8());
    buf.push(0x00);
    buf.push((asdu_address & 0xFF) as u8);
    buf.push(((asdu_address >> 8) & 0xFF) as u8);
    buf.push((ioa & 0xFF) as u8);
    buf.push(((ioa >> 8) & 0xFF) as u8);
    buf.push(((ioa >> 16) & 0xFF) as u8);
    // DCO: DCS(bit0-1) + QU(bit2-6) + S/E(bit7)
    let dco = (dcs & 0x03) | ((qu & 0x1F) << 2) | (if s_e { 0x80 } else { 0x00 });
    buf.push(dco);
    buf
}

/// Build a clock synchronization command (C_CS_NA_1) ASDU.
///
/// The timestamp is encoded as CP56Time2a (7 bytes).
pub fn build_clock_sync_command(asdu_address: u16, timestamp_ms: u64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(16);
    buf.push(TypeId::ClockSynchronization.to_u8()); // TI = 103
    buf.push(0x01);
    buf.push(CauseOfTransmission::Activation.to_u8()); // COT = 6
    buf.push(0x00);
    buf.push((asdu_address & 0xFF) as u8);
    buf.push(((asdu_address >> 8) & 0xFF) as u8);
    // IOA = 0 for clock sync
    buf.push(0x00);
    buf.push(0x00);
    buf.push(0x00);
    // CP56Time2a (7 bytes)
    let ms_of_day = (timestamp_ms % 86_400_000) as u32;
    let day = (timestamp_ms / 86_400_000) as u8;
    buf.push((ms_of_day & 0xFF) as u8);
    buf.push(((ms_of_day >> 8) & 0xFF) as u8);
    buf.push(((ms_of_day >> 16) & 0x0F) as u8);
    buf.push(0x00); // minute/hour (not used for sync, RTU sets its own)
    buf.push(day & 0x1F);
    buf.push(0x00); // month/year (not used)
    buf.push(0x00); // year
    buf
}

/// Build a parameter float command (P_PM_NA_1) ASDU for parameter download.
///
/// Used to download parameters (e.g., thresholds, scaling factors) to the RTU.
pub fn build_parameter_float(asdu_address: u16, ioa: u32, value: f32) -> Vec<u8> {
    let value_bytes = value.to_le_bytes();
    let mut buf = Vec::with_capacity(14);
    buf.push(TypeId::ParameterFloat.to_u8()); // TI = 112
    buf.push(0x01);
    buf.push(CauseOfTransmission::Activation.to_u8());
    buf.push(0x00);
    buf.push((asdu_address & 0xFF) as u8);
    buf.push(((asdu_address >> 8) & 0xFF) as u8);
    buf.push((ioa & 0xFF) as u8);
    buf.push(((ioa >> 8) & 0xFF) as u8);
    buf.push(((ioa >> 16) & 0xFF) as u8);
    buf.extend_from_slice(&value_bytes);
    buf
}

/// Build a parameter scaled command (P_PM_NI_1) ASDU for parameter download.
pub fn build_parameter_scaled(asdu_address: u16, ioa: u32, value: i16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(12);
    buf.push(TypeId::ParameterScaled.to_u8()); // TI = 111
    buf.push(0x01);
    buf.push(CauseOfTransmission::Activation.to_u8());
    buf.push(0x00);
    buf.push((asdu_address & 0xFF) as u8);
    buf.push(((asdu_address >> 8) & 0xFF) as u8);
    buf.push((ioa & 0xFF) as u8);
    buf.push(((ioa >> 8) & 0xFF) as u8);
    buf.push(((ioa >> 16) & 0xFF) as u8);
    buf.extend_from_slice(&value.to_le_bytes());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_id_from_u8() {
        assert_eq!(TypeId::from_u8(1), TypeId::SinglePoint);
        assert_eq!(TypeId::from_u8(3), TypeId::DoublePoint);
        assert_eq!(TypeId::from_u8(5), TypeId::StepPosition);
        assert_eq!(TypeId::from_u8(8), TypeId::BinaryCounterReading);
        assert_eq!(TypeId::from_u8(13), TypeId::MeasuredShortFloat);
        assert_eq!(TypeId::from_u8(30), TypeId::SinglePointTimeTag);
        assert_eq!(TypeId::from_u8(31), TypeId::DoublePointTimeTag);
        assert_eq!(TypeId::from_u8(36), TypeId::MeasuredShortFloatTimeTag);
        assert_eq!(TypeId::from_u8(45), TypeId::SingleCommand);
        assert_eq!(TypeId::from_u8(46), TypeId::DoubleCommand);
        assert_eq!(TypeId::from_u8(50), TypeId::SetpointShortFloat);
        assert_eq!(TypeId::from_u8(100), TypeId::InterrogationCommand);
        assert_eq!(TypeId::from_u8(103), TypeId::ClockSynchronization);
        assert_eq!(TypeId::from_u8(111), TypeId::ParameterScaled);
        assert_eq!(TypeId::from_u8(112), TypeId::ParameterFloat);
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

    #[test]
    fn test_parse_m_dp_na_1() {
        // M_DP_NA_1: Double-point without time tag
        let buf: Vec<u8> = vec![
            0x03, 0x01, 0x03, 0x00, 0x01, 0x00, // TI=3, Num=1, COT=Spontaneous, OA=0, ASDU=1
            0x64, 0x00, 0x00, // IOA = 100
            0x02, // DIQ: value=On (2), quality=Good
        ];
        let asdu = parse_asdu(&buf).unwrap();
        assert_eq!(asdu.type_id, TypeId::DoublePoint);
        assert_eq!(asdu.num_objects, 1);
        match &asdu.objects[0] {
            InformationObject::DoublePoint { value, .. } => {
                assert_eq!(*value, DoublePointValue::On);
            }
            _ => panic!("expected DoublePoint"),
        }
    }

    #[test]
    fn test_parse_m_st_na_1() {
        // M_ST_NA_1: Step position without time tag
        let buf: Vec<u8> = vec![
            0x05, 0x01, 0x03, 0x00, 0x01, 0x00,
            0x64, 0x00, 0x00, // IOA = 100
            0x14, // VTS: value=20 (7-bit), transient=0
            0x00, // Quality
        ];
        let asdu = parse_asdu(&buf).unwrap();
        assert_eq!(asdu.type_id, TypeId::StepPosition);
        assert_eq!(asdu.num_objects, 1);
    }

    #[test]
    fn test_parse_m_bo_na_1() {
        // M_BO_NA_1: Binary counter reading
        let counter: u32 = 123456;
        let cb = counter.to_le_bytes();
        let buf: Vec<u8> = vec![
            0x08, 0x01, 0x03, 0x00, 0x01, 0x00,
            0x64, 0x00, 0x00, // IOA = 100
            cb[0], cb[1], cb[2], cb[3], // Counter value
            0x00, // Sequence/carry/invalid/adjusted
        ];
        let asdu = parse_asdu(&buf).unwrap();
        assert_eq!(asdu.type_id, TypeId::BinaryCounterReading);
        match &asdu.objects[0] {
            InformationObject::BinaryCounterReading { counter: c, .. } => {
                assert_eq!(*c, 123456);
            }
            _ => panic!("expected BinaryCounterReading"),
        }
    }

    #[test]
    fn test_build_double_command() {
        let cmd = build_double_command(1, 1001, 2, 0, false); // DCS=2 (ON)
        assert_eq!(cmd[0], 46); // TI = C_DC_NA_1
        assert_eq!(cmd[9] & 0x03, 2); // DCS = ON
    }

    #[test]
    fn test_build_clock_sync_command() {
        let cmd = build_clock_sync_command(1, 3600000); // 1 hour
        assert_eq!(cmd[0], 103); // TI = C_CS_NA_1
        assert_eq!(cmd.len(), 16); // 6 header + 3 IOA + 7 CP56Time2a
    }

    #[test]
    fn test_build_parameter_float() {
        let cmd = build_parameter_float(1, 1001, 50.5f32);
        assert_eq!(cmd[0], 112); // TI = P_PM_NA_1
        assert_eq!(cmd.len(), 13); // 6 header + 3 IOA + 4 float
    }

    #[test]
    fn test_build_parameter_scaled() {
        let cmd = build_parameter_scaled(1, 1001, 100i16);
        assert_eq!(cmd[0], 111); // TI = P_PM_NI_1
        assert_eq!(cmd.len(), 11); // 6 header + 3 IOA + 2 int16
    }

    #[test]
    fn test_type_id_display() {
        assert_eq!(format!("{}", TypeId::DoublePoint), "M_DP_NA_1(3)");
        assert_eq!(format!("{}", TypeId::StepPosition), "M_ST_NA_1(5)");
        assert_eq!(format!("{}", TypeId::ClockSynchronization), "C_CS_NA_1(103)");
        assert_eq!(format!("{}", TypeId::ParameterFloat), "P_PM_NA_1(112)");
    }

    #[test]
    fn test_information_object_double_point() {
        let obj = InformationObject::DoublePoint {
            ioa: 100,
            value: DoublePointValue::On,
            quality: MeasuredQuality::from_u8(0),
        };
        assert_eq!(obj.ioa(), 100);
        assert!((obj.as_float().unwrap() - 1.0).abs() < 0.001);
        assert!(obj.is_valid());
    }

    #[test]
    fn test_information_object_step_position() {
        let obj = InformationObject::StepPosition {
            ioa: 200,
            value: 10,
            transient: false,
            quality: MeasuredQuality::from_u8(0),
        };
        assert_eq!(obj.ioa(), 200);
        assert!((obj.as_float().unwrap() - 10.0).abs() < 0.001);
    }

    #[test]
    fn test_information_object_bcr() {
        let obj = InformationObject::BinaryCounterReading {
            ioa: 300,
            counter: 999999,
            sequence: 5,
            carry: false,
            invalid: false,
            adjusted: false,
        };
        assert_eq!(obj.ioa(), 300);
        assert!((obj.as_float().unwrap() - 999999.0).abs() < 0.001);
        assert!(obj.is_valid());

        let invalid_obj = InformationObject::BinaryCounterReading {
            ioa: 300,
            counter: 0,
            sequence: 0,
            carry: false,
            invalid: true,
            adjusted: false,
        };
        assert!(!invalid_obj.is_valid());
    }
}
