//! IEC 60870-5 ASDU 应用层公共库（TypeId/Cot/QualityDescriptor/IoValue/Asdu 等）.
//!
//! 蓝图 48 WP-48.0 自 eneros-iec104-slave 原样抽取（行为零变更，diff 只允许
//! 移动+重导出+错误类型机械改名）：101/103 与 104 共用本 crate 的 ASDU 层，
//! slave/master 保留各自链路层。实现 IEC 60870-5-104 ASDU 编解码（SQ=0 非
//! 序列模式）；浮点值小端序 IEEE 754（D6）；CP56Time2a 7 字节时标（D10）。
//!
//! 错误类型 `AsduError`（Decode/InvalidTypeId——原 asdu.rs 实际使用的全部
//! 变体）；iec104-slave 经 `From<AsduError>` 无损映射回 `Iec104Error`。

use alloc::vec::Vec;

use crate::error::AsduError;

// ===== TypeId =====

/// 类型标识（10 变体）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeId {
    /// 单点遥信（TI=1）
    SinglePointInformation = 1,
    /// 双点遥信（TI=3）
    DoublePointInformation = 3,
    /// 归一化遥测（TI=9）
    MeasuredValueNormalized = 9,
    /// 标度化遥测（TI=11）
    MeasuredValueScaled = 11,
    /// 短浮点遥测（TI=13）
    MeasuredValueFloat = 13,
    /// 计数量（TI=15）
    Counter = 15,
    /// 46（WP-46.4）：步位置（TI=5，M_ST_NA，VTI 字节）。
    StepPosition = 5,
    /// 46（WP-46.4）：比特串 32（TI=7，M_BO_NA，4B + 品质）。
    Bitstring = 7,
    /// 46（WP-46.4）：归一化设点命令（TI=48，C_SE_NA，发送/确认）。
    SetpointNormalized = 48,
    /// 46（WP-46.4）：短浮点设点命令（TI=50，C_SE_NC，发送/确认）。
    SetpointFloat = 50,
    /// 单点遥控（TI=45）
    SingleCommand = 45,
    /// 双点遥控（TI=46）
    DoubleCommand = 46,
    /// 总召唤命令（TI=100）
    InterrogationCommand = 100,
    /// 时钟同步命令（TI=103）
    ClockSyncCommand = 103,
    /// 单点遥信带时标（TI=30，M_SP_TB_1，v0.220.0）
    SinglePointTimeTagged = 30,
    /// 归一化遥测带时标（TI=34，M_ME_NB_1，v0.220.0）
    MeasuredValueNormalizedTimeTagged = 34,
    /// 短浮点遥测带时标（TI=36，M_ME_TD_1，v0.220.0）
    MeasuredValueFloatTimeTagged = 36,
}

impl TypeId {
    /// 从 `u8` 转换为 `TypeId`，非法值返回 `None`。
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::SinglePointInformation),
            3 => Some(Self::DoublePointInformation),
            9 => Some(Self::MeasuredValueNormalized),
            11 => Some(Self::MeasuredValueScaled),
            13 => Some(Self::MeasuredValueFloat),
            15 => Some(Self::Counter),
            5 => Some(Self::StepPosition),
            7 => Some(Self::Bitstring),
            48 => Some(Self::SetpointNormalized),
            50 => Some(Self::SetpointFloat),
            45 => Some(Self::SingleCommand),
            46 => Some(Self::DoubleCommand),
            100 => Some(Self::InterrogationCommand),
            103 => Some(Self::ClockSyncCommand),
            30 => Some(Self::SinglePointTimeTagged),
            34 => Some(Self::MeasuredValueNormalizedTimeTagged),
            36 => Some(Self::MeasuredValueFloatTimeTagged),
            _ => None,
        }
    }

    /// 转换为 `u8`。
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    /// 该类型是否强制携带时标（CP56Time2a，v0.220.0）。
    pub fn is_time_tagged(self) -> bool {
        matches!(
            self,
            Self::SinglePointTimeTagged
                | Self::MeasuredValueNormalizedTimeTagged
                | Self::MeasuredValueFloatTimeTagged
        )
    }

    /// 该类型是否携带品质描述符。
    fn has_quality(self) -> bool {
        matches!(
            self,
            Self::SinglePointInformation
                | Self::DoublePointInformation
                | Self::MeasuredValueNormalized
                | Self::MeasuredValueScaled
                | Self::MeasuredValueFloat
                | Self::Counter
                | Self::SinglePointTimeTagged
                | Self::MeasuredValueNormalizedTimeTagged
                | Self::MeasuredValueFloatTimeTagged
                | Self::Bitstring
        )
    }
}

// ===== Cot =====

/// 传送原因（9 变体）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cot {
    /// 周期传送（=1）
    Periodic = 1,
    /// 背景扫描（=2）
    Background = 2,
    /// 突发/变化上报（=3）
    Spontaneous = 3,
    /// 初始化（=4）
    Initialized = 4,
    /// 请求（=5）
    Request = 5,
    /// 激活（=6）
    Activation = 6,
    /// 激活确认（=7）
    ActivationConfirm = 7,
    /// 停止激活（=8）
    Deactivation = 8,
    /// 激活终止（=10，PTC-003：总召唤/遥控终止确认必须用此 COT，
    /// 旧实现误用 ActivationConfirm=7）
    ActivationTermination = 10,
    /// 被总召唤（=20）
    InterrogatedByStation = 20,
}

impl Cot {
    /// 从 `u8` 转换为 `Cot`，非法值返回 `None`。
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Periodic),
            2 => Some(Self::Background),
            3 => Some(Self::Spontaneous),
            4 => Some(Self::Initialized),
            5 => Some(Self::Request),
            6 => Some(Self::Activation),
            7 => Some(Self::ActivationConfirm),
            8 => Some(Self::Deactivation),
            10 => Some(Self::ActivationTermination),
            20 => Some(Self::InterrogatedByStation),
            _ => None,
        }
    }

    /// 转换为 `u8`。
    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

// ===== QualityDescriptor =====

/// 品质描述符（5 个标志位）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QualityDescriptor {
    /// 无效
    pub invalid: bool,
    /// 非当前
    pub not_topical: bool,
    /// 替代值
    pub substituted: bool,
    /// 闭锁
    pub blocked: bool,
    /// 溢出（遥测）
    pub overflow: bool,
}

impl QualityDescriptor {
    /// 创建全良好品质（所有标志为 false）。
    pub fn good() -> Self {
        Self::default()
    }

    /// 编码为 1 字节（bit0=OV, bit1=BL, bit2=SB, bit3=NT, bit4=IV）。
    pub fn encode(&self) -> u8 {
        let mut b = 0u8;
        if self.overflow {
            b |= 0x01;
        }
        if self.blocked {
            b |= 0x02;
        }
        if self.substituted {
            b |= 0x04;
        }
        if self.not_topical {
            b |= 0x08;
        }
        if self.invalid {
            b |= 0x10;
        }
        b
    }

    /// 从 1 字节解码。
    pub fn decode(b: u8) -> Self {
        Self {
            overflow: b & 0x01 != 0,
            blocked: b & 0x02 != 0,
            substituted: b & 0x04 != 0,
            not_topical: b & 0x08 != 0,
            invalid: b & 0x10 != 0,
        }
    }
}

// ===== SinglePointValue / DoublePointValue =====

/// 单点遥信值
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinglePointValue {
    /// 分（=0）
    Off = 0,
    /// 合（=1）
    On = 1,
}

impl SinglePointValue {
    /// 从 `u8` 转换。
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Off),
            1 => Some(Self::On),
            _ => None,
        }
    }
}

/// 双点遥信值
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoublePointValue {
    /// 中间态（=0）
    Intermediate = 0,
    /// 分（=1）
    Off = 1,
    /// 合（=2）
    On = 2,
    /// 错误（=3）
    Bad = 3,
}

impl DoublePointValue {
    /// 从 `u8` 转换。
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Intermediate),
            1 => Some(Self::Off),
            2 => Some(Self::On),
            3 => Some(Self::Bad),
            _ => None,
        }
    }
}

// ===== Sco / Dco =====

/// 单点遥控命令限定符（SCO）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sco {
    /// 命令值（true=合，false=分）
    pub value: bool,
    /// 限定符（2 bits）
    pub qu: u8,
    /// 选择/执行标志（true=选择，false=执行）
    pub select: bool,
}

impl Sco {
    /// 创建默认单点遥控命令（无限定符、执行）。
    pub fn new(value: bool) -> Self {
        Self {
            value,
            qu: 0,
            select: false,
        }
    }

    /// 编码为 1 字节（蓝图/20 R1-6 按 IEC 60870-5-101 QOC 修正）：
    /// bit0=SCS、bits2-6=QU（5 位，bit1 保留恒 0）、bit7=S/E。
    pub fn encode(&self) -> u8 {
        let mut b = 0u8;
        if self.value {
            b |= 0x01;
        }
        b |= (self.qu & 0x1F) << 2;
        if self.select {
            b |= 0x80;
        }
        b
    }

    /// 从 1 字节解码。
    pub fn decode(b: u8) -> Self {
        Self {
            value: b & 0x01 != 0,
            qu: (b >> 2) & 0x1F,
            select: b & 0x80 != 0,
        }
    }
}

/// 双点遥控命令限定符（DCO）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dco {
    /// 命令值
    pub value: DoublePointValue,
    /// 限定符（2 bits）
    pub qu: u8,
    /// 选择/执行标志
    pub select: bool,
}

impl Dco {
    /// 创建默认双点遥控命令。
    pub fn new(value: DoublePointValue) -> Self {
        Self {
            value,
            qu: 0,
            select: false,
        }
    }

    /// 编码为 1 字节（v0.211.0 P0-10 按 IEC 60870-5-101 重编码：
    /// bits0-1=DCS、bits2-6=QU（5 位）、bit7=S/E）。
    pub fn encode(&self) -> u8 {
        let mut b = 0u8;
        b |= (self.value as u8) & 0x03;
        b |= (self.qu & 0x1F) << 2;
        if self.select {
            b |= 0x80;
        }
        b
    }

    /// 从 1 字节解码。
    pub fn decode(b: u8) -> Self {
        Self {
            value: DoublePointValue::from_u8(b & 0x03).unwrap_or(DoublePointValue::Intermediate),
            qu: (b >> 2) & 0x1F,
            select: b & 0x80 != 0,
        }
    }
}

// ===== TimeTag (CP56Time2a, D10) =====

/// CP56Time2a 7 字节时标（D10）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeTag {
    /// 年（0-99，表示 2000-2099）
    pub year: u8,
    /// 月（1-12）
    pub month: u8,
    /// 日（1-31）
    pub day: u8,
    /// 时（0-23）
    pub hour: u8,
    /// 分（0-59）
    pub minute: u8,
    /// 秒（0-59）
    pub second: u8,
    /// 无效标志
    pub iv: bool,
    /// 夏令时标志
    pub su: bool,
    /// 毫秒（0-999，秒内毫秒）
    pub millis: u16,
}

impl TimeTag {
    /// 编码为 7 字节 CP56Time2a。
    ///
    /// 字节布局：
    /// - Byte 0-1: 毫秒（秒内毫秒 + 秒 × 1000，0-59999），16 bits LE
    /// - Byte 2: 分(6 bits) + IV(bit6) + reserved(bit7)
    /// - Byte 3: 时(5 bits) + SU(bit5) + reserved(bits 6-7)
    /// - Byte 4: 日(5 bits) + day-of-week(bits 5-7)
    /// - Byte 5: 月(4 bits) + reserved(bits 4-7)
    /// - Byte 6: 年(7 bits) + reserved(bit7)
    pub fn encode(&self) -> [u8; 7] {
        let mut b = [0u8; 7];
        // 毫秒字段 = second * 1000 + millis（0-59999，需 16 bits）
        let total_ms = (self.second as u16) * 1000 + (self.millis % 1000);
        b[0] = (total_ms & 0xFF) as u8;
        b[1] = ((total_ms >> 8) & 0xFF) as u8;
        b[2] = (self.minute & 0x3F) | (if self.iv { 0x40 } else { 0x00 });
        b[3] = (self.hour & 0x1F) | (if self.su { 0x20 } else { 0x00 });
        b[4] = self.day & 0x1F;
        b[5] = self.month & 0x0F;
        b[6] = self.year & 0x7F;
        b
    }

    /// 从 7 字节解码。
    pub fn decode(bytes: &[u8; 7]) -> Self {
        let total_ms = (bytes[0] as u16) | ((bytes[1] as u16) << 8);
        Self {
            year: bytes[6] & 0x7F,
            month: bytes[5] & 0x0F,
            day: bytes[4] & 0x1F,
            hour: bytes[3] & 0x1F,
            minute: bytes[2] & 0x3F,
            second: (total_ms / 1000) as u8,
            iv: bytes[2] & 0x40 != 0,
            su: bytes[3] & 0x20 != 0,
            millis: total_ms % 1000,
        }
    }

    /// 蓝图 61 [07] §3.3（v0.314.31）：CP56Time2a → Unix 纳秒（UTC）。
    ///
    /// - 年 = 2000 + year（CP56Time2a 百年域 2000-2099）；
    /// - 日期 civil → days 自 1970-01-01（Howard Hinnant `days_from_civil`
    ///   算法——无 chrono 依赖的 no_std 纯整数实现）；
    /// - 秒内毫秒（`millis`）折进纳秒。
    ///
    /// 非法日期（`decode` 位掩码可产出 month=0/13..15、day=0 等越界形态）
    /// → `None`——调用方回退传输接收时刻，时钟质量另行标注（不伪造时标）。
    pub fn to_unix_nanos(&self) -> Option<u64> {
        if self.month < 1 || self.month > 12 || self.day < 1 || self.day > 31 {
            return None;
        }
        let year = 2000i64 + i64::from(self.year);
        let m = i64::from(self.month);
        let d = i64::from(self.day);
        // days_from_civil（公有域算法）：March-based 年/月变换 → era 展开。
        let y = if m <= 2 { year - 1 } else { year };
        let era = y / 400; // y ≥ 0 → 截断除即 era
        let yoe = y - era * 400; // [0, 399]
        let mp = (m + 9) % 12; // Mar=0 .. Feb=11
        let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
        let days = era * 146_097 + doe - 719_468; // 自 1970-01-01 的天数
        let secs = days * 86_400
            + i64::from(self.hour) * 3_600
            + i64::from(self.minute) * 60
            + i64::from(self.second);
        let nanos = secs
            .checked_mul(1_000_000_000)?
            .checked_add(i64::from(self.millis) * 1_000_000)?;
        if nanos < 0 {
            return None;
        }
        Some(nanos as u64)
    }
}

// ===== IoValue =====

/// 信息对象值（8 变体）
///
/// 注意：`Float(f32)` 不实现 `Eq`，故 `IoValue` 仅派生 `PartialEq`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IoValue {
    /// 归一化遥测（-32768~32767）
    Normalized(i16),
    /// 标度化遥测
    Scaled(i16),
    /// 短浮点遥测（小端序 IEEE 754，D6）
    Float(f32),
    /// 单点遥信
    SinglePoint(SinglePointValue),
    /// 双点遥信
    DoublePoint(DoublePointValue),
    /// 单点遥控命令
    SingleCommand(Sco),
    /// 双点遥控命令
    DoubleCommand(Dco),
    /// 计数量
    Counter(u32),
    /// 46（WP-46.4）：步位置（M_ST；VTI 字节 = 低 3 位步值 + 高 5 位品质）。
    StepPosition(u8),
    /// 46（WP-46.4）：比特串 32（M_BO）。
    Bitstring(u32),
    /// 46（WP-46.4）：归一化设点（C_SE_NA；发送/确认）。
    SetpointNormalized(i16),
    /// 46（WP-46.4）：短浮点设点（C_SE_NC；发送/确认）。
    SetpointFloat(f32),
}

impl IoValue {
    /// 返回对应的 `TypeId`。
    pub fn type_id(&self) -> TypeId {
        match self {
            Self::SinglePoint(_) => TypeId::SinglePointInformation,
            Self::DoublePoint(_) => TypeId::DoublePointInformation,
            Self::Normalized(_) => TypeId::MeasuredValueNormalized,
            Self::Scaled(_) => TypeId::MeasuredValueScaled,
            Self::Float(_) => TypeId::MeasuredValueFloat,
            Self::Counter(_) => TypeId::Counter,
            Self::SingleCommand(_) => TypeId::SingleCommand,
            Self::DoubleCommand(_) => TypeId::DoubleCommand,
            Self::StepPosition(_) => TypeId::StepPosition,
            Self::Bitstring(_) => TypeId::Bitstring,
            Self::SetpointNormalized(_) => TypeId::SetpointNormalized,
            Self::SetpointFloat(_) => TypeId::SetpointFloat,
        }
    }
}

// ===== InformationObject =====

/// 信息对象
#[derive(Debug, Clone, PartialEq)]
pub struct InformationObject {
    /// 信息对象地址（IOA，3 字节 24-bit，v0.220.0 支持 >65535）
    pub ioa: u32,
    /// 值
    pub value: IoValue,
    /// 品质描述符
    pub quality: QualityDescriptor,
    /// 可选时标（CP56Time2a）
    pub time_tag: Option<TimeTag>,
}

impl InformationObject {
    /// 编码 IOA 为 3 字节小端序（24-bit，v0.220.0）。
    fn encode_ioa(ioa: u32) -> [u8; 3] {
        let lo = (ioa & 0xFF) as u8;
        let mid = ((ioa >> 8) & 0xFF) as u8;
        let hi = ((ioa >> 16) & 0xFF) as u8;
        [lo, mid, hi]
    }

    /// 从 3 字节小端序解码 IOA（24-bit，v0.220.0）。
    fn decode_ioa(bytes: &[u8]) -> u32 {
        (bytes[0] as u32) | ((bytes[1] as u32) << 8) | ((bytes[2] as u32) << 16)
    }

    /// 按 `type_id` 编码单个信息对象（IOA + 值 + 品质 + 可选时标）。
    fn encode(&self, type_id: TypeId) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&Self::encode_ioa(self.ioa));
        buf.extend_from_slice(&self.encode_body(type_id));
        buf
    }

    /// 编码信息对象体（不含 IOA；SQ=1 序列帧后续对象复用，v0.220.0）。
    fn encode_body(&self, type_id: TypeId) -> Vec<u8> {
        let mut buf = Vec::new();
        match type_id {
            TypeId::SinglePointInformation | TypeId::SinglePointTimeTagged => {
                // SIQ: bit0=value, bits1-5=quality
                let mut b = self.value.as_single_point() as u8;
                b |= self.quality.encode() << 1;
                buf.push(b);
            }
            TypeId::DoublePointInformation => {
                // DIQ: bits0-1=value, bits2-6=quality
                let mut b = self.value.as_double_point() as u8;
                b |= self.quality.encode() << 2;
                buf.push(b);
            }
            TypeId::MeasuredValueNormalized
            | TypeId::MeasuredValueScaled
            | TypeId::MeasuredValueNormalizedTimeTagged => {
                let v = self.value.as_i16();
                buf.extend_from_slice(&v.to_le_bytes());
                buf.push(self.quality.encode());
            }
            TypeId::MeasuredValueFloat | TypeId::MeasuredValueFloatTimeTagged => {
                let v = self.value.as_f32();
                buf.extend_from_slice(&v.to_le_bytes()); // D6: 小端序
                buf.push(self.quality.encode());
            }
            TypeId::Counter => {
                let v = self.value.as_u32();
                buf.extend_from_slice(&v.to_le_bytes());
                buf.push(self.quality.encode());
            }
            TypeId::StepPosition => {
                buf.push(self.value.as_step_position() & 0x07); // VTI 低 3 位步值
            }
            TypeId::Bitstring => {
                buf.extend_from_slice(&self.value.as_u32().to_le_bytes());
                buf.push(self.quality.encode());
            }
            TypeId::SetpointNormalized | TypeId::SetpointFloat => {
                // C_SE_NA/NC：值 + QDS（bit0 = S/E，命令侧默认 0 = 执行）。
                let q = self.quality.encode() & !0x01;
                match type_id {
                    TypeId::SetpointNormalized => {
                        buf.extend_from_slice(&self.value.as_i16().to_le_bytes())
                    }
                    _ => buf.extend_from_slice(&self.value.as_f32().to_le_bytes()),
                }
                buf.push(q);
            }
            TypeId::SingleCommand => {
                buf.push(self.value.as_sco().encode());
            }
            TypeId::DoubleCommand => {
                buf.push(self.value.as_dco().encode());
            }
            TypeId::InterrogationCommand => {
                // QOI 字节（从 Normalized 取低字节，默认 20）
                let qoi = match &self.value {
                    IoValue::Normalized(n) => (*n & 0xFF) as u8,
                    _ => 20,
                };
                buf.push(qoi);
            }
            TypeId::ClockSyncCommand => {
                // 7 字节 CP56Time2a
                if let Some(t) = &self.time_tag {
                    buf.extend_from_slice(&t.encode());
                } else {
                    buf.extend_from_slice(&[0u8; 7]);
                }
            }
        }
        // 附加时标（v0.220.0）：强制时标类型必须写（缺失时写 0）；
        // 其他带品质类型可选。
        if type_id.is_time_tagged() {
            if let Some(t) = &self.time_tag {
                buf.extend_from_slice(&t.encode());
            } else {
                buf.extend_from_slice(&[0u8; 7]);
            }
        } else if type_id.has_quality() {
            if let Some(t) = &self.time_tag {
                buf.extend_from_slice(&t.encode());
            }
        }
        buf
    }

    /// 按 `type_id` 解码单个信息对象，返回 `(InformationObject, 消费字节数)`。
    fn decode(bytes: &[u8], type_id: TypeId) -> Result<(Self, usize), AsduError> {
        if bytes.len() < 3 {
            return Err(AsduError::Decode);
        }
        let ioa = Self::decode_ioa(&bytes[0..3]);
        let (body, consumed) = Self::decode_body(&bytes[3..], type_id)?;
        Ok((InformationObject { ioa, ..body }, 3 + consumed))
    }

    /// 按 `type_id` 解码信息对象体（不含 IOA；SQ=1 序列帧用，v0.220.0）。
    fn decode_body(bytes: &[u8], type_id: TypeId) -> Result<(Self, usize), AsduError> {
        let mut off = 0usize;
        let (value, quality, time_tag) = match type_id {
            TypeId::SinglePointInformation | TypeId::SinglePointTimeTagged => {
                if bytes.len() < off + 1 {
                    return Err(AsduError::Decode);
                }
                let b = bytes[off];
                off += 1;
                let v = SinglePointValue::from_u8(b & 0x01).unwrap_or(SinglePointValue::Off);
                let q = QualityDescriptor::decode((b >> 1) & 0x1F);
                (IoValue::SinglePoint(v), q, None)
            }
            TypeId::DoublePointInformation => {
                if bytes.len() < off + 1 {
                    return Err(AsduError::Decode);
                }
                let b = bytes[off];
                off += 1;
                let v =
                    DoublePointValue::from_u8(b & 0x03).unwrap_or(DoublePointValue::Intermediate);
                let q = QualityDescriptor::decode((b >> 2) & 0x1F);
                (IoValue::DoublePoint(v), q, None)
            }
            TypeId::MeasuredValueNormalized | TypeId::MeasuredValueNormalizedTimeTagged => {
                if bytes.len() < off + 3 {
                    return Err(AsduError::Decode);
                }
                let v = i16::from_le_bytes([bytes[off], bytes[off + 1]]);
                let q = QualityDescriptor::decode(bytes[off + 2]);
                off += 3;
                (IoValue::Normalized(v), q, None)
            }
            TypeId::MeasuredValueScaled => {
                if bytes.len() < off + 3 {
                    return Err(AsduError::Decode);
                }
                let v = i16::from_le_bytes([bytes[off], bytes[off + 1]]);
                let q = QualityDescriptor::decode(bytes[off + 2]);
                off += 3;
                (IoValue::Scaled(v), q, None)
            }
            TypeId::MeasuredValueFloat | TypeId::MeasuredValueFloatTimeTagged => {
                if bytes.len() < off + 5 {
                    return Err(AsduError::Decode);
                }
                let v = f32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
                let q = QualityDescriptor::decode(bytes[off + 4]);
                off += 5;
                (IoValue::Float(v), q, None)
            }
            TypeId::Counter => {
                if bytes.len() < off + 5 {
                    return Err(AsduError::Decode);
                }
                let v = u32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
                let q = QualityDescriptor::decode(bytes[off + 4]);
                off += 5;
                (IoValue::Counter(v), q, None)
            }
            TypeId::StepPosition => {
                if bytes.len() < off + 1 {
                    return Err(AsduError::Decode);
                }
                let b = bytes[off];
                off += 1;
                // VTI：低 3 位步值 + 高 5 位品质。
                (
                    IoValue::StepPosition(b & 0x07),
                    QualityDescriptor::decode((b >> 3) & 0x1F),
                    None,
                )
            }
            TypeId::Bitstring => {
                if bytes.len() < off + 5 {
                    return Err(AsduError::Decode);
                }
                let v = u32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
                let q = QualityDescriptor::decode(bytes[off + 4]);
                off += 5;
                (IoValue::Bitstring(v), q, None)
            }
            TypeId::SetpointNormalized => {
                if bytes.len() < off + 3 {
                    return Err(AsduError::Decode);
                }
                let v = i16::from_le_bytes([bytes[off], bytes[off + 1]]);
                let q = QualityDescriptor::decode(bytes[off + 2]);
                off += 3;
                (IoValue::SetpointNormalized(v), q, None)
            }
            TypeId::SetpointFloat => {
                if bytes.len() < off + 5 {
                    return Err(AsduError::Decode);
                }
                let v = f32::from_le_bytes([
                    bytes[off],
                    bytes[off + 1],
                    bytes[off + 2],
                    bytes[off + 3],
                ]);
                let q = QualityDescriptor::decode(bytes[off + 4]);
                off += 5;
                (IoValue::SetpointFloat(v), q, None)
            }
            TypeId::SingleCommand => {
                if bytes.len() < off + 1 {
                    return Err(AsduError::Decode);
                }
                let sco = Sco::decode(bytes[off]);
                off += 1;
                (IoValue::SingleCommand(sco), QualityDescriptor::good(), None)
            }
            TypeId::DoubleCommand => {
                if bytes.len() < off + 1 {
                    return Err(AsduError::Decode);
                }
                let dco = Dco::decode(bytes[off]);
                off += 1;
                (IoValue::DoubleCommand(dco), QualityDescriptor::good(), None)
            }
            TypeId::InterrogationCommand => {
                if bytes.len() < off + 1 {
                    return Err(AsduError::Decode);
                }
                let qoi = bytes[off];
                off += 1;
                (
                    IoValue::Normalized(qoi as i16),
                    QualityDescriptor::good(),
                    None,
                )
            }
            TypeId::ClockSyncCommand => {
                if bytes.len() < off + 7 {
                    return Err(AsduError::Decode);
                }
                let mut tt = [0u8; 7];
                tt.copy_from_slice(&bytes[off..off + 7]);
                off += 7;
                let tag = TimeTag::decode(&tt);
                (
                    IoValue::SinglePoint(SinglePointValue::On),
                    QualityDescriptor::good(),
                    Some(tag),
                )
            }
        };
        let mut obj = Self {
            ioa: 0, // SQ=1 由调用方回填；SQ=0 走 decode() 入口
            value,
            quality,
            time_tag,
        };
        // 结构化时标解码（v0.220.0，替代"恰好 7 字节"启发式）：
        // 强制时标类型必须读 7 字节 CP56Time2a。
        if type_id.is_time_tagged() {
            if bytes.len() < off + 7 {
                return Err(AsduError::Decode);
            }
            let mut tt = [0u8; 7];
            tt.copy_from_slice(&bytes[off..off + 7]);
            off += 7;
            obj.time_tag = Some(TimeTag::decode(&tt));
        }
        Ok((obj, off))
    }
}

// ===== IoValue 私有辅助转换（用于 InformationObject 编解码）=====

impl IoValue {
    fn as_single_point(&self) -> SinglePointValue {
        match self {
            Self::SinglePoint(v) => *v,
            _ => SinglePointValue::Off,
        }
    }
    fn as_double_point(&self) -> DoublePointValue {
        match self {
            Self::DoublePoint(v) => *v,
            _ => DoublePointValue::Intermediate,
        }
    }
    fn as_i16(&self) -> i16 {
        match self {
            Self::Normalized(v) | Self::Scaled(v) | Self::SetpointNormalized(v) => *v,
            _ => 0,
        }
    }
    fn as_f32(&self) -> f32 {
        match self {
            Self::Float(v) | Self::SetpointFloat(v) => *v,
            _ => 0.0,
        }
    }
    fn as_u32(&self) -> u32 {
        match self {
            Self::Counter(v) | Self::Bitstring(v) => *v,
            _ => 0,
        }
    }
    /// 46.4：步位置 VTI 字节。
    fn as_step_position(&self) -> u8 {
        match self {
            Self::StepPosition(v) => *v,
            _ => 0,
        }
    }
    fn as_sco(&self) -> Sco {
        match self {
            Self::SingleCommand(s) => *s,
            _ => Sco::new(false),
        }
    }
    fn as_dco(&self) -> Dco {
        match self {
            Self::DoubleCommand(d) => *d,
            _ => Dco::new(DoublePointValue::Intermediate),
        }
    }
}

// ===== Asdu =====

/// ASDU（Application Service Data Unit）
#[derive(Debug, Clone, PartialEq)]
pub struct Asdu {
    /// 类型标识
    pub type_id: TypeId,
    /// 传送原因
    pub cause_of_tx: Cot,
    /// 公共地址（ASDU 地址）
    pub common_addr: u16,
    /// 信息对象列表
    pub ioas: Vec<InformationObject>,
}

impl Asdu {
    /// 编码 ASDU 为字节流（SQ 自动选择，v0.220.0）。
    ///
    /// 布局：TypeId(1) | VarStruct(1) | COT(1) | OriginatorAddr(1) | CommonAddr(2 LE) | InfoObjects
    ///
    /// SQ=1 序列压缩：当 ≥2 个对象的 IOA 连续（相邻差 1）时启用——
    /// 首对象带完整 IOA，后续对象仅编码体（IOA 递增）。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(self.type_id.to_u8());
        let num = self.ioas.len().min(127);
        // 连续 IOA 检测（仅当 >1 个对象且类型非命令类）。
        let use_sq = num > 1
            && self.ioas.windows(2).all(|w| w[1].ioa == w[0].ioa + 1)
            && !matches!(
                self.type_id,
                TypeId::SingleCommand
                    | TypeId::DoubleCommand
                    | TypeId::InterrogationCommand
                    | TypeId::ClockSyncCommand
            );
        // VarStruct: SQ (bit7) | number of objects (bits 0-6)
        buf.push(if use_sq { num as u8 | 0x80 } else { num as u8 });
        // COT + OriginatorAddr (originator = 0)
        buf.push(self.cause_of_tx.to_u8());
        buf.push(0u8);
        // CommonAddr (2 bytes LE)
        buf.extend_from_slice(&self.common_addr.to_le_bytes());
        if use_sq {
            // 首对象完整 IOA，后续仅体。
            buf.extend_from_slice(&self.ioas[0].encode(self.type_id));
            for io in &self.ioas[1..] {
                buf.extend_from_slice(&io.encode_body(self.type_id));
            }
        } else {
            for io in &self.ioas {
                buf.extend_from_slice(&io.encode(self.type_id));
            }
        }
        buf
    }

    /// 从字节流解码 ASDU。
    pub fn decode(bytes: &[u8]) -> Result<Self, AsduError> {
        // 头部至少 6 字节
        if bytes.len() < 6 {
            return Err(AsduError::Decode);
        }
        let type_id = TypeId::from_u8(bytes[0]).ok_or(AsduError::InvalidTypeId)?;
        let var_struct = bytes[1];
        let sq = var_struct & 0x80 != 0;
        let num = (var_struct & 0x7F) as usize;
        let cot = Cot::from_u8(bytes[2]).ok_or(AsduError::Decode)?;
        let _originator = bytes[3];
        let common_addr = u16::from_le_bytes([bytes[4], bytes[5]]);
        let mut off = 6usize;
        let mut ioas = Vec::with_capacity(num);
        if sq {
            // SQ=1 序列压缩：首对象带完整 IOA，后续 IOA 递增。
            if num == 0 {
                return Err(AsduError::Decode);
            }
            let (first, consumed) = InformationObject::decode(&bytes[off..], type_id)?;
            off += consumed;
            let base_ioa = first.ioa;
            ioas.push(first);
            for idx in 1..num {
                // 仅解码体（IOA = base + idx）。
                let (obj, consumed) = InformationObject::decode_body(&bytes[off..], type_id)?;
                off += consumed;
                ioas.push(InformationObject {
                    ioa: base_ioa + idx as u32,
                    ..obj
                });
            }
        } else {
            for _ in 0..num {
                let (obj, consumed) = InformationObject::decode(&bytes[off..], type_id)?;
                off += consumed;
                ioas.push(obj);
            }
        }
        Ok(Self {
            type_id,
            cause_of_tx: cot,
            common_addr,
            ioas,
        })
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;

    // ===== TypeId 测试 =====

    #[test]
    fn test_typeid_roundtrip_all_variants() {
        let variants = [
            TypeId::SinglePointInformation,
            TypeId::DoublePointInformation,
            TypeId::MeasuredValueNormalized,
            TypeId::MeasuredValueScaled,
            TypeId::MeasuredValueFloat,
            TypeId::Counter,
            TypeId::StepPosition,
            TypeId::Bitstring,
            TypeId::SetpointNormalized,
            TypeId::SetpointFloat,
            TypeId::SingleCommand,
            TypeId::DoubleCommand,
            TypeId::InterrogationCommand,
            TypeId::ClockSyncCommand,
        ];
        for v in &variants {
            let u = v.to_u8();
            assert_eq!(TypeId::from_u8(u), Some(*v));
        }
    }

    #[test]
    fn test_typeid_from_u8_invalid() {
        assert_eq!(TypeId::from_u8(0), None);
        assert_eq!(TypeId::from_u8(2), None);
        assert_eq!(TypeId::from_u8(255), None);
    }

    #[test]
    fn test_typeid_known_values() {
        assert_eq!(TypeId::SinglePointInformation.to_u8(), 1);
        assert_eq!(TypeId::MeasuredValueFloat.to_u8(), 13);
        assert_eq!(TypeId::InterrogationCommand.to_u8(), 100);
        assert_eq!(TypeId::ClockSyncCommand.to_u8(), 103);
    }

    // ===== Cot 测试 =====

    #[test]
    fn test_cot_roundtrip_all_variants() {
        let variants = [
            Cot::Periodic,
            Cot::Background,
            Cot::Spontaneous,
            Cot::Initialized,
            Cot::Request,
            Cot::Activation,
            Cot::ActivationConfirm,
            Cot::Deactivation,
            Cot::InterrogatedByStation,
        ];
        for v in &variants {
            let u = v.to_u8();
            assert_eq!(Cot::from_u8(u), Some(*v));
        }
    }

    #[test]
    fn test_cot_from_u8_invalid() {
        assert_eq!(Cot::from_u8(0), None);
        assert_eq!(Cot::from_u8(9), None);
        assert_eq!(Cot::from_u8(21), None);
    }

    // ===== QualityDescriptor 测试 =====

    #[test]
    fn test_quality_good() {
        let q = QualityDescriptor::good();
        assert!(!q.invalid);
        assert!(!q.not_topical);
        assert!(!q.substituted);
        assert!(!q.blocked);
        assert!(!q.overflow);
        assert_eq!(q.encode(), 0);
    }

    #[test]
    fn test_quality_encode_bits() {
        let q = QualityDescriptor {
            overflow: true,
            ..Default::default()
        };
        assert_eq!(q.encode(), 0x01);
        let q = QualityDescriptor {
            blocked: true,
            ..Default::default()
        };
        assert_eq!(q.encode(), 0x02);
        let q = QualityDescriptor {
            substituted: true,
            ..Default::default()
        };
        assert_eq!(q.encode(), 0x04);
        let q = QualityDescriptor {
            not_topical: true,
            ..Default::default()
        };
        assert_eq!(q.encode(), 0x08);
        let q = QualityDescriptor {
            invalid: true,
            ..Default::default()
        };
        assert_eq!(q.encode(), 0x10);
    }

    #[test]
    fn test_quality_all_flags() {
        let q = QualityDescriptor {
            invalid: true,
            not_topical: true,
            substituted: true,
            blocked: true,
            overflow: true,
        };
        assert_eq!(q.encode(), 0x1F);
    }

    #[test]
    fn test_quality_roundtrip_all_combinations() {
        for b in 0..=0x1F {
            let q = QualityDescriptor::decode(b);
            assert_eq!(q.encode(), b);
        }
    }

    // ===== Sco / Dco 测试 =====

    #[test]
    fn test_sco_encode_decode() {
        let sco = Sco::new(true);
        assert_eq!(sco.encode(), 0x01);
        let decoded = Sco::decode(0x01);
        assert_eq!(decoded, sco);
    }

    #[test]
    fn test_sco_with_qu_and_select() {
        let sco = Sco {
            value: true,
            qu: 2,
            select: true,
        };
        let encoded = sco.encode();
        // 蓝图/20 R1-6（IEC 60870-5-101 QOC）：QU 5 位（bits2-6）、S/E=bit7。
        assert_eq!(encoded, 0x01 | (2 << 2) | 0x80);
        let decoded = Sco::decode(encoded);
        assert_eq!(decoded, sco);
        // 标准 select 帧 0x81：select=true、value=true、qu=0。
        let std_select = Sco::decode(0x81);
        assert!(
            std_select.select,
            "标准 select 帧 0x81 必须解出 select=true"
        );
        assert!(std_select.value);
        assert_eq!(std_select.qu, 0);
    }

    /// 权威测试向量（蓝图/20 R1-6，可对照 IEC 60870-5-101 标准条文）：
    /// - select + ON + QU=0 → 0x81（bit0=SCS、bit7=S/E）
    /// - execute + ON + QU=1 → 0x05（bit0=SCS、bit2=QU bit0；旧编码给 0x03 即不互通）
    /// - QU=31 → bits2-6 全 1（0x7C 布局）
    #[test]
    fn test_sco_authoritative_vectors() {
        // 向量 1：select + ON + QU=0 → 0x81
        let sco = Sco {
            value: true,
            qu: 0,
            select: true,
        };
        assert_eq!(sco.encode(), 0x81, "select+ON+QU=0 必须编码为 0x81");
        assert_eq!(Sco::decode(0x81), sco);

        // 向量 2：execute + ON + QU=1 → 0x05（bit1 必须恒 0）
        let sco = Sco {
            value: true,
            qu: 1,
            select: false,
        };
        assert_eq!(sco.encode(), 0x05, "execute+ON+QU=1 必须编码为 0x05");
        assert_eq!(Sco::decode(0x05), sco);
        assert_eq!(0x05 & 0x02, 0, "bit1 是保留位，必须恒 0");

        // 向量 3：QU=31 → bits2-6 全 1（0x7C 布局）
        let sco = Sco {
            value: true,
            qu: 31,
            select: false,
        };
        assert_eq!(sco.encode(), 0x7D, "ON+QU=31 必须编码为 0x7D");
        assert_eq!(Sco::decode(0x7D), sco);
        let sco = Sco {
            value: false,
            qu: 31,
            select: false,
        };
        assert_eq!(sco.encode(), 0x7C, "OFF+QU=31 必须编码为 0x7C");
        assert_eq!(Sco::decode(0x7C), sco);
        // 高位布局断言：QU 占 bits2-6，bit1 保留
        assert_eq!(Sco::decode(0x7D).qu, 31);
        assert!(Sco::decode(0x7D).value);
        assert!(!Sco::decode(0x7D).select);
    }

    /// 回归证明（蓝图/20 R1-6 硬性约束）：旧编码 qu<<1 给出的 0x03 必须
    /// 被新解码判为 QU=0（即旧行为在标准语义下错误，防同源自证）。
    #[test]
    fn test_sco_old_encoding_is_rejected_by_standard_layout() {
        // 旧编码 execute+ON+QU=1 → 0x03；标准布局解码 0x03 → QU=0、value=ON、
        // 丢失限定符 → 对端将按 QU=0 执行，行为不可区分（即互操作破坏点）。
        let legacy = Sco {
            value: true,
            qu: 1,
            select: false,
        };
        let legacy_byte = 0x01 | (1 << 1); // 旧实现产物
        assert_ne!(legacy.encode(), legacy_byte, "新编码不得再产出旧布局字节");
        assert_eq!(Sco::decode(legacy_byte).qu, 0, "0x03 在标准布局下 QU=0");
    }

    #[test]
    fn test_dco_encode_decode() {
        let dco = Dco::new(DoublePointValue::On);
        assert_eq!(dco.encode(), 0x02);
        let decoded = Dco::decode(0x02);
        assert_eq!(decoded, dco);
    }

    #[test]
    fn test_dco_with_qu_and_select() {
        let dco = Dco {
            value: DoublePointValue::Off,
            qu: 3,
            select: true,
        };
        let encoded = dco.encode();
        let decoded = Dco::decode(encoded);
        assert_eq!(decoded, dco);
    }

    /// DCO 权威向量（蓝图/20 R1-6 同步核对，IEC 60870-5-101 标准布局）：
    /// bits0-1=DCS、bits2-6=QU、bit7=S/E。
    #[test]
    fn test_dco_authoritative_vectors() {
        // select + ON + QU=0 → 0x82
        let dco = Dco {
            value: DoublePointValue::On,
            qu: 0,
            select: true,
        };
        assert_eq!(dco.encode(), 0x82, "select+ON+QU=0 必须编码为 0x82");
        assert_eq!(Dco::decode(0x82), dco);
        // execute + ON + QU=1 → 0x06
        let dco = Dco {
            value: DoublePointValue::On,
            qu: 1,
            select: false,
        };
        assert_eq!(dco.encode(), 0x06, "execute+ON+QU=1 必须编码为 0x06");
        assert_eq!(Dco::decode(0x06), dco);
        // QU=31 → bits2-6 全 1（Off=1 → DCS bits0-1=01）
        let dco = Dco {
            value: DoublePointValue::Off,
            qu: 31,
            select: true,
        };
        assert_eq!(dco.encode(), 0xFD, "OFF+QU=31+select 必须编码为 0xFD");
        assert_eq!(Dco::decode(0xFD), dco);
    }

    /// 互操作链路测试（蓝图/20 R1-6 第 3 项）：lib60870 客户端 SBO 帧字节
    /// （C_SC_NA TI=45，COT=6 激活，QU≠0）→ 本栈 ASDU 完整解析路径必须解出
    /// select==true && qu 一致。字节为外部对端产物（非本栈自编码，防同源）。
    #[test]
    fn test_sbo_standard_frames_qu_nonzero_full_path() {
        // lib60870 风格 select 帧：TI=45, VSQ=1, COT=6(ACT), CA=1, IOA=100, SCO=0x85
        // SCO 0x85 = bit7 S/E + bit0 SCS + bit2 QU bit0 → select=true, value=true, qu=1
        let select_frame: &[u8] = &[45, 1, 6, 0, 1, 0, 100, 0, 0, 0x85];
        let asdu = Asdu::decode(select_frame).expect("lib60870 select 帧必须可解析");
        assert_eq!(asdu.type_id, TypeId::SingleCommand);
        assert_eq!(asdu.ioas.len(), 1);

        match &asdu.ioas[0].value {
            IoValue::SingleCommand(sco) => {
                assert!(sco.select, "外部 select 帧必须解出 select=true");
                assert!(sco.value);
                assert_eq!(sco.qu, 1, "外部 QU≠0 必须一致解析");
            }
            other => unreachable!("期望 SingleCommand，实际 {other:?}"),
        }

        // lib60870 风格 execute 帧：SCO=0x05 → execute, value=true, qu=1
        let execute_frame: &[u8] = &[45, 1, 6, 0, 1, 0, 100, 0, 0, 0x05];
        let asdu = Asdu::decode(execute_frame).expect("lib60870 execute 帧必须可解析");

        match &asdu.ioas[0].value {
            IoValue::SingleCommand(sco) => {
                assert!(!sco.select, "execute 帧必须解出 select=false");
                assert!(sco.value);
                assert_eq!(sco.qu, 1);
            }
            other => unreachable!("期望 SingleCommand，实际 {other:?}"),
        }

        // 旧编码产物 0x03（execute+ON+QU=1）在标准布局下 QU=0：
        // 本栈不得把 0x03 误判为 QU=1（互操作破坏点回归证明）
        let legacy_frame: &[u8] = &[45, 1, 6, 0, 1, 0, 100, 0, 0, 0x03];
        let asdu = Asdu::decode(legacy_frame).expect("旧布局字节可解析");

        match &asdu.ioas[0].value {
            IoValue::SingleCommand(sco) => assert_eq!(sco.qu, 0, "0x03 在标准布局下 QU=0"),
            other => unreachable!("期望 SingleCommand，实际 {other:?}"),
        }
    }

    // ===== TimeTag 测试 =====

    #[test]
    fn test_timetag_roundtrip() {
        let tag = TimeTag {
            year: 25,
            month: 7,
            day: 15,
            hour: 10,
            minute: 30,
            second: 45,
            iv: false,
            su: false,
            millis: 500,
        };
        let bytes = tag.encode();
        assert_eq!(bytes.len(), 7);
        let decoded = TimeTag::decode(&bytes);
        assert_eq!(decoded, tag);
    }

    #[test]
    fn test_timetag_edge_values() {
        let tag = TimeTag {
            year: 0,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            iv: false,
            su: false,
            millis: 0,
        };
        let decoded = TimeTag::decode(&tag.encode());
        assert_eq!(decoded, tag);

        let tag = TimeTag {
            year: 99,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            second: 59,
            iv: true,
            su: true,
            millis: 999,
        };
        let decoded = TimeTag::decode(&tag.encode());
        assert_eq!(decoded, tag);
    }

    #[test]
    fn test_timetag_ms_encoding() {
        // second=45, millis=500 → total_ms = 45500
        let tag = TimeTag {
            year: 0,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 45,
            iv: false,
            su: false,
            millis: 500,
        };
        let bytes = tag.encode();
        let total_ms = (bytes[0] as u16) | ((bytes[1] as u16) << 8);
        assert_eq!(total_ms, 45500);
    }

    // ===== 蓝图 61 [07] §3.3（v0.314.31）：CP56Time2a → Unix 纳秒 =====

    /// 已知日期向量（手算）：2026-08-30 12:00:00.500 UTC
    /// = Unix 1_788_091_200 s + 0.5 s = 1_788_091_200_500_000_000 ns。
    #[test]
    fn test_to_unix_nanos_known_vector_2026() {
        let tag = TimeTag {
            year: 26,
            month: 8,
            day: 30,
            hour: 12,
            minute: 0,
            second: 0,
            iv: false,
            su: false,
            millis: 500,
        };
        assert_eq!(tag.to_unix_nanos(), Some(1_788_091_200_500_000_000));
    }

    /// Unix 纪元锚点：2000-01-01 00:00:00.000 = 946_684_800 s（手算：
    /// 1970→2000 三十年 + 7 个闰日 = 10957 天）。
    #[test]
    fn test_to_unix_nanos_y2000_anchor() {
        let tag = TimeTag {
            year: 0,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            iv: false,
            su: false,
            millis: 0,
        };
        assert_eq!(tag.to_unix_nanos(), Some(946_684_800_000_000_000));
    }

    /// 闰日向量：2024-02-29 00:00:00 = 1_709_164_800 s（含世纪闰年
    /// 2000 全覆盖的 days_from_civil 展开正确性）。
    #[test]
    fn test_to_unix_nanos_leap_day() {
        let tag = TimeTag {
            year: 24,
            month: 2,
            day: 29,
            hour: 0,
            minute: 0,
            second: 0,
            iv: false,
            su: false,
            millis: 0,
        };
        assert_eq!(tag.to_unix_nanos(), Some(1_709_164_800_000_000_000));
    }

    /// 世纪闰年：2000-02-29 = 951_782_400 s（÷400 规则分支）。
    #[test]
    fn test_to_unix_nanos_century_leap() {
        let tag = TimeTag {
            year: 0,
            month: 2,
            day: 29,
            hour: 0,
            minute: 0,
            second: 0,
            iv: false,
            su: false,
            millis: 0,
        };
        assert_eq!(tag.to_unix_nanos(), Some(951_782_400_000_000_000));
    }

    /// 非法日期（decode 位掩码越界形态）→ None：不伪造时标。
    #[test]
    fn test_to_unix_nanos_rejects_invalid_civil_date() {
        let mut tag = TimeTag {
            year: 26,
            month: 8,
            day: 30,
            hour: 12,
            minute: 0,
            second: 0,
            iv: false,
            su: false,
            millis: 0,
        };
        for month in [0u8, 13, 15] {
            tag.month = month;
            assert_eq!(tag.to_unix_nanos(), None, "month={month}");
        }
        for day in [0u8, 32] {
            tag.month = 8;
            tag.day = day;
            assert_eq!(tag.to_unix_nanos(), None, "day={day}");
        }
    }

    // ===== IoValue type_id 测试 =====

    #[test]
    fn test_iovalue_type_id() {
        assert_eq!(
            IoValue::SinglePoint(SinglePointValue::On).type_id(),
            TypeId::SinglePointInformation
        );
        assert_eq!(
            IoValue::DoublePoint(DoublePointValue::On).type_id(),
            TypeId::DoublePointInformation
        );
        assert_eq!(
            IoValue::Normalized(0).type_id(),
            TypeId::MeasuredValueNormalized
        );
        assert_eq!(IoValue::Scaled(0).type_id(), TypeId::MeasuredValueScaled);
        assert_eq!(IoValue::Float(0.0).type_id(), TypeId::MeasuredValueFloat);
        assert_eq!(IoValue::Counter(0).type_id(), TypeId::Counter);
        assert_eq!(
            IoValue::SingleCommand(Sco::new(false)).type_id(),
            TypeId::SingleCommand
        );
        assert_eq!(
            IoValue::DoubleCommand(Dco::new(DoublePointValue::Off)).type_id(),
            TypeId::DoubleCommand
        );
    }

    // ===== Asdu encode/decode 各 TypeId 往返 =====

    fn make_asdu(type_id: TypeId, value: IoValue) -> Asdu {
        Asdu {
            type_id,
            cause_of_tx: Cot::Periodic,
            common_addr: 1,
            ioas: vec![InformationObject {
                ioa: 100,
                value,
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        }
    }

    #[test]
    fn test_asdu_single_point_roundtrip() {
        let asdu = make_asdu(
            TypeId::SinglePointInformation,
            IoValue::SinglePoint(SinglePointValue::On),
        );
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 1); // type_id
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_asdu_double_point_roundtrip() {
        let asdu = make_asdu(
            TypeId::DoublePointInformation,
            IoValue::DoublePoint(DoublePointValue::On),
        );
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 3);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_asdu_normalized_roundtrip() {
        let asdu = make_asdu(TypeId::MeasuredValueNormalized, IoValue::Normalized(-1234));
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 9);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_asdu_scaled_roundtrip() {
        let asdu = make_asdu(TypeId::MeasuredValueScaled, IoValue::Scaled(5678));
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 11);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_asdu_float_roundtrip() {
        let asdu = make_asdu(TypeId::MeasuredValueFloat, IoValue::Float(1.5));
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 13);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        // f32 用 matches! 比较避免 PartialEq 浮点精度问题（此处应精确）
        assert!(matches!(decoded.ioas[0].value, IoValue::Float(v) if (v - 1.5).abs() < 1e-6));
        assert_eq!(decoded.type_id, TypeId::MeasuredValueFloat);
        assert_eq!(decoded.cause_of_tx, Cot::Periodic);
        assert_eq!(decoded.common_addr, 1);
    }

    #[test]
    fn test_asdu_float_little_endian_d6() {
        // D6: 浮点值必须以小端序编码
        let asdu = make_asdu(TypeId::MeasuredValueFloat, IoValue::Float(1.5));
        let bytes = asdu.encode();
        // ASDU 头部 6 字节 + IOA 3 字节 = 9，浮点从字节 9 开始
        let le_bytes = &bytes[9..13];
        assert_eq!(le_bytes, 1.5f32.to_le_bytes());
    }

    #[test]
    fn test_asdu_counter_roundtrip() {
        let asdu = make_asdu(TypeId::Counter, IoValue::Counter(0xDEADBEEF));
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 15);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    /// 46（WP-46.4）M_ST（TI=5，步位置）：向量自造（IEC 60870-5-101
    /// 表 M_ST_NA_1：VTI 1B = 低 3 位步值 + 高 5 位品质）。推导依据=标准
    /// 对象结构——非权威来源，测试注释登记。手测字节：[05 01 01 00 00 64 12]。
    #[test]
    fn test_asdu_step_position_roundtrip_and_vector() {
        let asdu = make_asdu(TypeId::StepPosition, IoValue::StepPosition(0x05));
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 5, "type id");
        // [type 05][VSQ 01][COT 01 00][CA 01 00][IOA 64 0000][VTI 12]
        let n = bytes.len();
        assert_eq!(&bytes[n - 1..], &[0x05], "VTI = 步值低 3 位 + 品质高 5 位");
        assert_eq!(bytes[n - 1] & 0x07, 0x05, "低 3 位步值");
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
        assert_eq!(decoded.ioas[0].value, IoValue::StepPosition(0x05));
    }

    /// 46（WP-46.4）M_BO（TI=7，比特串 32）：向量自造（M_BO_NA_1：
    /// BS32 4B + QDS 1B）。非权威来源，手写向量按标准对象结构推导。
    #[test]
    fn test_asdu_bitstring_roundtrip() {
        let asdu = make_asdu(TypeId::Bitstring, IoValue::Bitstring(0x01020304));
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 7, "type id");
        // BS32 小端 04 03 02 01 + QDS 00（good）——体尾部 5 字节。
        let n = bytes.len();
        assert_eq!(&bytes[n - 5..n - 1], &[0x04, 0x03, 0x02, 0x01]);
        assert_eq!(bytes[n - 1], 0x00);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    /// 46（WP-46.4）C_SE_NA（TI=48，归一化设点）：向量自造（C_SE_NA_1：
    /// NVA 2B 小端 + QDS 1B（bit0=S/E=执行 0））。手写向量推导依据=标准对象结构。
    #[test]
    fn test_asdu_setpoint_normalized_roundtrip() {
        let asdu = make_asdu(
            TypeId::SetpointNormalized,
            IoValue::SetpointNormalized(-1234),
        );
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 48, "type id");
        // NVA = -1234 = 0xFB2E 小端 → 2E FB；QDS=00（S/E=0 执行）。
        let n = bytes.len();
        assert_eq!(&bytes[n - 3..n - 1], &[0x2E, 0xFB]);
        assert_eq!(bytes[n - 1], 0x00);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    /// 46（WP-46.4）C_SE_NC（TI=50，短浮点设点）：向量自造（C_SE_NC_1：
    /// 4B 小端 IEEE 754 + QDS（S/E=执行））。推导依据=标准对象结构。
    #[test]
    fn test_asdu_setpoint_float_roundtrip() {
        let asdu = make_asdu(TypeId::SetpointFloat, IoValue::SetpointFloat(3.5f32));
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 50, "type id");
        // 3.5f32 LE = 00 00 60 40；QDS=00（执行）。
        let n = bytes.len();
        assert_eq!(&bytes[n - 5..n - 1], &[0x00, 0x00, 0x60, 0x40]);
        assert_eq!(bytes[n - 1], 0x00);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_asdu_single_command_roundtrip() {
        let asdu = make_asdu(
            TypeId::SingleCommand,
            IoValue::SingleCommand(Sco::new(true)),
        );
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 45);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_asdu_double_command_roundtrip() {
        let asdu = make_asdu(
            TypeId::DoubleCommand,
            IoValue::DoubleCommand(Dco::new(DoublePointValue::On)),
        );
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 46);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_asdu_interrogation_roundtrip() {
        let asdu = make_asdu(TypeId::InterrogationCommand, IoValue::Normalized(20));
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 100);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_asdu_clock_sync_roundtrip() {
        let asdu = Asdu {
            type_id: TypeId::ClockSyncCommand,
            cause_of_tx: Cot::Activation,
            common_addr: 1,
            ioas: vec![InformationObject {
                ioa: 0,
                value: IoValue::SinglePoint(SinglePointValue::On),
                quality: QualityDescriptor::good(),
                time_tag: Some(TimeTag {
                    year: 25,
                    month: 7,
                    day: 15,
                    hour: 10,
                    minute: 30,
                    second: 45,
                    iv: false,
                    su: false,
                    millis: 500,
                }),
            }],
        };
        let bytes = asdu.encode();
        assert_eq!(bytes[0], 103);
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded.type_id, TypeId::ClockSyncCommand);
        assert_eq!(decoded.ioas[0].time_tag, asdu.ioas[0].time_tag);
    }

    // ===== 边界测试 =====

    #[test]
    fn test_asdu_empty_ioas() {
        let asdu = Asdu {
            type_id: TypeId::SinglePointInformation,
            cause_of_tx: Cot::Periodic,
            common_addr: 1,
            ioas: Vec::new(),
        };
        let bytes = asdu.encode();
        assert_eq!(bytes[1], 0); // 0 objects
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded.ioas.len(), 0);
    }

    #[test]
    fn test_asdu_multiple_ioas() {
        let asdu = Asdu {
            type_id: TypeId::SinglePointInformation,
            cause_of_tx: Cot::Spontaneous,
            common_addr: 1,
            ioas: vec![
                InformationObject {
                    ioa: 1,
                    value: IoValue::SinglePoint(SinglePointValue::On),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                },
                InformationObject {
                    ioa: 2,
                    value: IoValue::SinglePoint(SinglePointValue::Off),
                    quality: QualityDescriptor {
                        invalid: true,
                        ..Default::default()
                    },
                    time_tag: None,
                },
                InformationObject {
                    ioa: 3,
                    value: IoValue::SinglePoint(SinglePointValue::On),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                },
                InformationObject {
                    ioa: 4,
                    value: IoValue::SinglePoint(SinglePointValue::Off),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                },
                InformationObject {
                    ioa: 5,
                    value: IoValue::SinglePoint(SinglePointValue::On),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                },
            ],
        };
        let bytes = asdu.encode();
        // v0.220.0：IOA 1..=5 连续 → 自动 SQ=1 序列压缩（bit7=1 | 5）。
        assert_eq!(bytes[1], 0x85, "SQ=1 | 5 objects");
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded.ioas.len(), 5);
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_asdu_with_time_tag() {
        // v0.220.0：带时标类型用 TI=36（M_ME_TD_1）——TI=13 无强制时标。
        let asdu = Asdu {
            type_id: TypeId::MeasuredValueFloatTimeTagged,
            cause_of_tx: Cot::Spontaneous,
            common_addr: 1,
            ioas: vec![InformationObject {
                ioa: 1,
                value: IoValue::Float(1.5),
                quality: QualityDescriptor::good(),
                time_tag: Some(TimeTag {
                    year: 25,
                    month: 1,
                    day: 1,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    iv: false,
                    su: false,
                    millis: 0,
                }),
            }],
        };
        let bytes = asdu.encode();
        let decoded = Asdu::decode(&bytes).expect("decode ok");
        assert_eq!(decoded.ioas[0].time_tag, asdu.ioas[0].time_tag);
    }

    #[test]
    fn test_asdu_decode_too_short() {
        assert_eq!(Asdu::decode(&[1, 1]), Err(AsduError::Decode));
    }

    #[test]
    fn test_asdu_decode_invalid_type_id() {
        let bytes = [200u8, 1, 1, 0, 1, 0];
        assert_eq!(Asdu::decode(&bytes), Err(AsduError::InvalidTypeId));
    }

    // ===== v0.220.0（B2-03）：3 字节 IOA / SQ=1 / 带时标类型 =====

    #[test]
    fn test_ioa_24bit_roundtrip() {
        // >65535 点寻址：IOA 0x010203。
        let asdu = Asdu {
            type_id: TypeId::SinglePointInformation,
            cause_of_tx: Cot::Spontaneous,
            common_addr: 1,
            ioas: vec![InformationObject {
                ioa: 0x010203,
                value: IoValue::SinglePoint(SinglePointValue::On),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };
        let bytes = asdu.encode();
        // IOA 3 字节小端：0x03 0x02 0x01（在 TypeId+VarStruct+COT+Orig+CommonAddr 之后）。
        assert_eq!(bytes[6], 0x03);
        assert_eq!(bytes[7], 0x02);
        assert_eq!(bytes[8], 0x01);
        let decoded = Asdu::decode(&bytes).expect("decode");
        assert_eq!(decoded.ioas[0].ioa, 0x010203);
    }

    #[test]
    fn test_ioa_24bit_large_sequential() {
        // 70000 起的连续 IOA（旧 u16 溢出场景）。
        let asdu = Asdu {
            type_id: TypeId::MeasuredValueNormalized,
            cause_of_tx: Cot::Spontaneous,
            common_addr: 1,
            ioas: (70_000..70_004)
                .map(|ioa| InformationObject {
                    ioa,
                    value: IoValue::Normalized(100),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                })
                .collect(),
        };
        let bytes = asdu.encode();
        // 连续 → SQ=1 压缩。
        assert_eq!(bytes[1], 0x84, "SQ=1 | 4 objects");
        let decoded = Asdu::decode(&bytes).expect("decode");
        assert_eq!(decoded.ioas.len(), 4);
        assert_eq!(decoded.ioas[0].ioa, 70_000);
        assert_eq!(decoded.ioas[3].ioa, 70_003);
    }

    #[test]
    fn test_sq1_compressed_roundtrip() {
        // 显式 SQ=1：单点遥信连续 IOA 序列压缩往返。
        let asdu = Asdu {
            type_id: TypeId::SinglePointInformation,
            cause_of_tx: Cot::Spontaneous,
            common_addr: 5,
            ioas: vec![
                InformationObject {
                    ioa: 10,
                    value: IoValue::SinglePoint(SinglePointValue::On),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                },
                InformationObject {
                    ioa: 11,
                    value: IoValue::SinglePoint(SinglePointValue::Off),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                },
                InformationObject {
                    ioa: 12,
                    value: IoValue::SinglePoint(SinglePointValue::On),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                },
            ],
        };
        let bytes = asdu.encode();
        assert_eq!(bytes[1], 0x83);
        let decoded = Asdu::decode(&bytes).expect("decode");
        assert_eq!(decoded, asdu);
        // 压缩后长度：6 头 + 4(首 IOA+SIQ) + 2×1(后续仅 SIQ) = 12。
        assert_eq!(bytes.len(), 12);
    }

    #[test]
    fn test_non_sequential_ioas_use_sq0() {
        // 非连续 IOA → 保持 SQ=0（每个对象完整 IOA）。
        let asdu = Asdu {
            type_id: TypeId::SinglePointInformation,
            cause_of_tx: Cot::Spontaneous,
            common_addr: 1,
            ioas: vec![
                InformationObject {
                    ioa: 1,
                    value: IoValue::SinglePoint(SinglePointValue::On),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                },
                InformationObject {
                    ioa: 100,
                    value: IoValue::SinglePoint(SinglePointValue::Off),
                    quality: QualityDescriptor::good(),
                    time_tag: None,
                },
            ],
        };
        let bytes = asdu.encode();
        assert_eq!(bytes[1], 0x02, "非连续 → SQ=0");
        let decoded = Asdu::decode(&bytes).expect("decode");
        assert_eq!(decoded, asdu);
    }

    #[test]
    fn test_single_point_time_tagged_roundtrip() {
        // M_SP_TB_1（TI=30）：单点遥信 + 强制时标。
        let tag = TimeTag {
            year: 25,
            month: 3,
            day: 14,
            hour: 8,
            minute: 30,
            second: 5,
            iv: false,
            su: false,
            millis: 250,
        };
        let asdu = Asdu {
            type_id: TypeId::SinglePointTimeTagged,
            cause_of_tx: Cot::Spontaneous,
            common_addr: 1,
            ioas: vec![InformationObject {
                ioa: 42,
                value: IoValue::SinglePoint(SinglePointValue::On),
                quality: QualityDescriptor::good(),
                time_tag: Some(tag),
            }],
        };
        let bytes = asdu.encode();
        // 3(IOA) + 1(SIQ) + 7(CP56Time2a) = 11。
        assert_eq!(bytes.len(), 6 + 11);
        let decoded = Asdu::decode(&bytes).expect("decode");
        assert_eq!(decoded.ioas[0].ioa, 42);
        assert_eq!(decoded.ioas[0].time_tag, Some(tag));
    }

    #[test]
    fn test_measured_value_float_time_tagged_roundtrip() {
        // M_ME_TD_1（TI=36）：短浮点遥测 + 强制时标。
        let tag = TimeTag {
            year: 25,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            iv: false,
            su: false,
            millis: 0,
        };
        let asdu = Asdu {
            type_id: TypeId::MeasuredValueFloatTimeTagged,
            cause_of_tx: Cot::Spontaneous,
            common_addr: 2,
            ioas: vec![InformationObject {
                ioa: 7,
                value: IoValue::Float(3.25),
                quality: QualityDescriptor::good(),
                time_tag: Some(tag),
            }],
        };
        let bytes = asdu.encode();
        let decoded = Asdu::decode(&bytes).expect("decode");
        assert_eq!(decoded.ioas[0].ioa, 7);
        assert_eq!(decoded.ioas[0].time_tag, Some(tag));
        let v = decoded.ioas[0].value.as_f32();
        assert!((v - 3.25).abs() < 1e-6);
    }

    #[test]
    fn test_time_tagged_missing_time_tag_encodes_zeros() {
        // 强制时标类型缺失 time_tag → 编码 7 字节 0，解码得到 Some(0 时标)。
        let asdu = Asdu {
            type_id: TypeId::SinglePointTimeTagged,
            cause_of_tx: Cot::Spontaneous,
            common_addr: 1,
            ioas: vec![InformationObject {
                ioa: 1,
                value: IoValue::SinglePoint(SinglePointValue::Off),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        };
        let bytes = asdu.encode();
        assert_eq!(bytes.len(), 6 + 11);
        let decoded = Asdu::decode(&bytes).expect("decode");
        assert!(
            decoded.ioas[0].time_tag.is_some(),
            "缺失时标编码为 0 后解码得 Some"
        );
    }

    #[test]
    fn test_time_tagged_decode_truncated_rejected() {
        // 强制时标类型但字节不足 7 字节时标 → Decode 错误（结构化校验）。
        let mut bytes = [0u8; 6 + 3 + 1 + 3]; // 头 + IOA + SIQ + 半截时标
        bytes[0] = 30; // M_SP_TB_1
        bytes[1] = 1;
        bytes[2] = 3; // Spontaneous
        bytes[4] = 1;
        assert_eq!(Asdu::decode(&bytes), Err(AsduError::Decode));
    }
}
