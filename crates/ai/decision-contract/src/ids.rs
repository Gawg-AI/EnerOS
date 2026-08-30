//! ID 与 reason code 值对象（[06] §3.5.2：ID、reason code、版本值对象）。

use crate::error::ContractError;

/// 16 字节 ID 值对象基型（wire 小端原样；全零 = 无效）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlanId(pub [u8; 16]);

impl PlanId {
    /// 构造校验：全零拒绝。
    pub fn new(bytes: [u8; 16]) -> Result<Self, ContractError> {
        if bytes == [0u8; 16] {
            return Err(ContractError::ZeroId);
        }
        Ok(Self(bytes))
    }
}

/// 证书 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CertificateId(pub [u8; 16]);

impl CertificateId {
    pub fn new(bytes: [u8; 16]) -> Result<Self, ContractError> {
        if bytes == [0u8; 16] {
            return Err(ContractError::ZeroId);
        }
        Ok(Self(bytes))
    }
}

/// 命令意图 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IntentId(pub [u8; 16]);

impl IntentId {
    pub fn new(bytes: [u8; 16]) -> Result<Self, ContractError> {
        if bytes == [0u8; 16] {
            return Err(ContractError::ZeroId);
        }
        Ok(Self(bytes))
    }
}

/// 快照 ID（TrustedSnapshot 引用；全零 = 未绑定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SnapshotId(pub [u8; 16]);

impl SnapshotId {
    pub fn is_unbound(&self) -> bool {
        self.0 == [0u8; 16]
    }
}

/// 决策理由码（封闭数字表；[06] §3.6：稳定 + 未知码 fail-closed）。
///
/// 码位冻结：只能追加，禁止改义。0 与未登记码一律拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonCode {
    /// 价格套利。
    PriceArbitrage = 1,
    /// 削峰。
    PeakShave = 2,
    /// 填谷。
    ValleyFill = 3,
    /// 新能源消纳/限发。
    RenewableCurtail = 4,
    /// 电网支撑（调频/备用）。
    GridSupport = 5,
    /// SOC 平衡。
    SocBalance = 6,
    /// 人工操作员。
    ManualOperator = 7,
    /// 合同义务。
    ContractObligation = 8,
    /// 紧急限值。
    EmergencyLimit = 9,
    /// 模型推荐。
    ModelRecommendation = 10,
}

impl ReasonCode {
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// 数字码 → 枚举；0 与未登记码 → None（fail-closed）。
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::PriceArbitrage),
            2 => Some(Self::PeakShave),
            3 => Some(Self::ValleyFill),
            4 => Some(Self::RenewableCurtail),
            5 => Some(Self::GridSupport),
            6 => Some(Self::SocBalance),
            7 => Some(Self::ManualOperator),
            8 => Some(Self::ContractObligation),
            9 => Some(Self::EmergencyLimit),
            10 => Some(Self::ModelRecommendation),
            _ => None,
        }
    }
}
