//! `CommandIntent` — 编译后的命令意图（[02] §7：PlanCompiler 唯一生产者；
//! 不能直接写设备）。

use alloc::vec::Vec;

use crate::error::ContractError;
use crate::ids::IntentId;
use crate::wire::{get_u16, put_u16, put_u32, put_u64};

/// 前置条件（执行前必须成立；迁移期最小闭集）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Precondition {
    /// 无前置（许可性动作）。
    None,
    /// 反馈点带值域（如 SOC ≥ 下限才允许放电）。
    FeedbackInRange { point_id: u32, min: f64, max: f64 },
}

/// 补偿提示（Saga/对账用；迁移期最小闭集）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompensationHint {
    /// 无补偿动作（对账收敛即可）。
    None,
    /// 回设到给定功率（kW）。
    RestorePowerKw(f64),
}

/// 命令意图（typed value 单源 = telemetry-model `CommandValue`；禁止
/// 第二份命令值——[16] §2）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandIntent {
    intent_id: IntentId,
    /// 绑定计划。
    plan_id: crate::ids::PlanId,
    /// 计划 wire 摘要（与证书 plan_sha256 同值——三向绑定）。
    plan_sha256: [u8; 32],
    /// 全局资产（0 = 拒绝）。
    asset_id: u64,
    device_id: u16,
    point_id: u32,
    /// 有功功率 kW（迁移期单 PCS 唯一语义；.39 多资源扩 CommandValue）。
    power_kw: f64,
    unit_code: u16,
    valid_from_ms: u64,
    valid_until_ms: u64,
    precondition: Precondition,
    compensation: CompensationHint,
}

impl CommandIntent {
    /// 校验构造器（PlanCompiler 唯一入口）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        intent_id: IntentId,
        plan_id: crate::ids::PlanId,
        plan_sha256: [u8; 32],
        asset_id: u64,
        device_id: u16,
        point_id: u32,
        power_kw: f64,
        unit_code: u16,
        valid_from_ms: u64,
        valid_until_ms: u64,
        precondition: Precondition,
        compensation: CompensationHint,
    ) -> Result<Self, ContractError> {
        if asset_id == 0 {
            return Err(ContractError::ScopeMismatch("asset_id=0"));
        }
        if unit_code == 0 {
            return Err(ContractError::MissingField("unit_code"));
        }
        if !power_kw.is_finite() {
            return Err(ContractError::OutOfRange("power_kw"));
        }
        if valid_until_ms <= valid_from_ms {
            return Err(ContractError::InvalidWindow);
        }
        if plan_sha256 == [0u8; 32] {
            return Err(ContractError::MissingField("plan_sha256"));
        }
        if let Precondition::FeedbackInRange { min, max, .. } = precondition {
            if !min.is_finite() || !max.is_finite() || min > max {
                return Err(ContractError::OutOfRange("precondition.range"));
            }
        }
        if let CompensationHint::RestorePowerKw(v) = compensation {
            if !v.is_finite() {
                return Err(ContractError::OutOfRange("compensation.power_kw"));
            }
        }
        Ok(Self {
            intent_id,
            plan_id,
            plan_sha256,
            asset_id,
            device_id,
            point_id,
            power_kw,
            unit_code,
            valid_from_ms,
            valid_until_ms,
            precondition,
            compensation,
        })
    }

    pub fn intent_id(&self) -> &IntentId {
        &self.intent_id
    }
    pub fn plan_id(&self) -> &crate::ids::PlanId {
        &self.plan_id
    }
    pub fn plan_sha256(&self) -> &[u8; 32] {
        &self.plan_sha256
    }
    pub fn asset_id(&self) -> u64 {
        self.asset_id
    }
    pub fn device_id(&self) -> u16 {
        self.device_id
    }
    pub fn point_id(&self) -> u32 {
        self.point_id
    }
    pub fn power_kw(&self) -> f64 {
        self.power_kw
    }
    pub fn unit_code(&self) -> u16 {
        self.unit_code
    }
    pub fn valid_from_ms(&self) -> u64 {
        self.valid_from_ms
    }
    pub fn valid_until_ms(&self) -> u64 {
        self.valid_until_ms
    }
    pub fn precondition(&self) -> &Precondition {
        &self.precondition
    }
    pub fn compensation(&self) -> &CompensationHint {
        &self.compensation
    }

    /// 生效窗判定。
    pub fn is_valid_at(&self, now_ms: u64) -> bool {
        now_ms >= self.valid_from_ms && now_ms <= self.valid_until_ms
    }
}

/// wire V1（定长 96B；迁移期 IPC 内直传 + .29 golden vectors）。
pub const INTENT_MAGIC: [u8; 4] = *b"CINT";
pub const INTENT_WIRE_LEN: usize = 146;

impl CommandIntent {
    pub fn encode_v1(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(INTENT_WIRE_LEN);
        out.extend_from_slice(&INTENT_MAGIC);
        put_u16(&mut out, crate::CONTRACT_WIRE_V1);
        put_u16(&mut out, 0);
        out.extend_from_slice(&self.intent_id.0);
        out.extend_from_slice(&self.plan_id.0);
        out.extend_from_slice(&self.plan_sha256);
        put_u64(&mut out, self.asset_id);
        put_u16(&mut out, self.device_id);
        put_u16(&mut out, 0); // reserved
        put_u32(&mut out, self.point_id);
        out.extend_from_slice(&self.power_kw.to_le_bytes());
        put_u16(&mut out, self.unit_code);
        put_u16(&mut out, 0); // reserved
        put_u64(&mut out, self.valid_from_ms);
        put_u64(&mut out, self.valid_until_ms);
        // precondition: 0=none / 1=feedback-in-range
        match self.precondition {
            Precondition::None => {
                out.push(0);
                put_u32(&mut out, 0);
                out.extend_from_slice(&0f64.to_le_bytes());
                out.extend_from_slice(&0f64.to_le_bytes());
            }
            Precondition::FeedbackInRange { point_id, min, max } => {
                out.push(1);
                put_u32(&mut out, point_id);
                out.extend_from_slice(&min.to_le_bytes());
                out.extend_from_slice(&max.to_le_bytes());
            }
        }
        // compensation: 0=none / 1=restore-power
        match self.compensation {
            CompensationHint::None => {
                out.push(0);
                out.extend_from_slice(&0f64.to_le_bytes());
            }
            CompensationHint::RestorePowerKw(v) => {
                out.push(1);
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    pub fn decode_v1(buf: &[u8]) -> Result<Self, ContractError> {
        if buf.len() != INTENT_WIRE_LEN || buf[0..4] != INTENT_MAGIC {
            return Err(ContractError::Malformed("magic/len"));
        }
        let version = u16::from_le_bytes(get_u16(buf, 4)?);
        if version != crate::CONTRACT_WIRE_V1 {
            return Err(ContractError::VersionUnsupported {
                found: version,
                supported: crate::CONTRACT_WIRE_V1,
            });
        }
        if u16::from_le_bytes(get_u16(buf, 6)?) != 0 {
            return Err(ContractError::Malformed("flags"));
        }
        let mut pos = 8usize;
        let take = |pos: &mut usize, n: usize| -> Result<&[u8], ContractError> {
            if *pos + n > buf.len() {
                return Err(ContractError::Malformed("truncated"));
            }
            let s = &buf[*pos..*pos + n];
            *pos += n;
            Ok(s)
        };
        let intent_id = IntentId(take(&mut pos, 16)?.try_into().unwrap());
        let plan_id = crate::ids::PlanId(take(&mut pos, 16)?.try_into().unwrap());
        let mut plan_sha256 = [0u8; 32];
        plan_sha256.copy_from_slice(take(&mut pos, 32)?);
        let asset_id = u64::from_le_bytes(take(&mut pos, 8)?.try_into().unwrap());
        let device_id = u16::from_le_bytes(take(&mut pos, 2)?.try_into().unwrap());
        let _reserved = take(&mut pos, 2)?;
        let point_id = u32::from_le_bytes(take(&mut pos, 4)?.try_into().unwrap());
        let power_kw = f64::from_le_bytes(take(&mut pos, 8)?.try_into().unwrap());
        let unit_code = u16::from_le_bytes(take(&mut pos, 2)?.try_into().unwrap());
        let _reserved2 = take(&mut pos, 2)?;
        let valid_from_ms = u64::from_le_bytes(take(&mut pos, 8)?.try_into().unwrap());
        let valid_until_ms = u64::from_le_bytes(take(&mut pos, 8)?.try_into().unwrap());
        let pre_tag = take(&mut pos, 1)?[0];
        let pre_point = u32::from_le_bytes(take(&mut pos, 4)?.try_into().unwrap());
        let pre_min = f64::from_le_bytes(take(&mut pos, 8)?.try_into().unwrap());
        let pre_max = f64::from_le_bytes(take(&mut pos, 8)?.try_into().unwrap());
        let precondition = match pre_tag {
            0 => Precondition::None,
            1 => Precondition::FeedbackInRange {
                point_id: pre_point,
                min: pre_min,
                max: pre_max,
            },
            _ => return Err(ContractError::Malformed("precondition")),
        };
        let comp_tag = take(&mut pos, 1)?[0];
        let comp_val = f64::from_le_bytes(take(&mut pos, 8)?.try_into().unwrap());
        let compensation = match comp_tag {
            0 => CompensationHint::None,
            1 => CompensationHint::RestorePowerKw(comp_val),
            _ => return Err(ContractError::Malformed("compensation")),
        };
        if pos != buf.len() {
            return Err(ContractError::Malformed("trailing"));
        }
        Self::new(
            intent_id,
            plan_id,
            plan_sha256,
            asset_id,
            device_id,
            point_id,
            power_kw,
            unit_code,
            valid_from_ms,
            valid_until_ms,
            precondition,
            compensation,
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn intent() -> CommandIntent {
        CommandIntent::new(
            IntentId([3u8; 16]),
            crate::ids::PlanId([1u8; 16]),
            [0xAB; 32],
            1,
            1,
            10,
            -50.0,
            6,
            1_000,
            61_000,
            Precondition::FeedbackInRange {
                point_id: 4,
                min: 0.0,
                max: 1.0,
            },
            CompensationHint::None,
        )
        .unwrap()
    }

    #[test]
    fn roundtrip() {
        let i = intent();
        let wire = i.encode_v1();
        assert_eq!(wire.len(), INTENT_WIRE_LEN);
        assert_eq!(CommandIntent::decode_v1(&wire).unwrap(), i);
    }

    #[test]
    fn negatives() {
        // asset 0。
        assert_eq!(
            CommandIntent::new(
                IntentId([3u8; 16]),
                crate::ids::PlanId([1u8; 16]),
                [1; 32],
                0,
                1,
                10,
                1.0,
                6,
                0,
                1,
                Precondition::None,
                CompensationHint::None,
            ),
            Err(ContractError::ScopeMismatch("asset_id=0"))
        );
        // 倒窗。
        assert_eq!(
            CommandIntent::new(
                IntentId([3u8; 16]),
                crate::ids::PlanId([1u8; 16]),
                [1; 32],
                1,
                1,
                10,
                1.0,
                6,
                5,
                5,
                Precondition::None,
                CompensationHint::None,
            ),
            Err(ContractError::InvalidWindow)
        );
        // 未知单位。
        assert_eq!(
            CommandIntent::new(
                IntentId([3u8; 16]),
                crate::ids::PlanId([1u8; 16]),
                [1; 32],
                1,
                1,
                10,
                1.0,
                0,
                0,
                1,
                Precondition::None,
                CompensationHint::None,
            ),
            Err(ContractError::MissingField("unit_code"))
        );
        // wire: 版本不支持。
        let mut wire = intent().encode_v1();
        wire[4] = 9;
        assert!(matches!(
            CommandIntent::decode_v1(&wire),
            Err(ContractError::VersionUnsupported { .. })
        ));
    }
}
