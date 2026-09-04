//! 蓝图 61 [16] §3.6：`decision-contract` — 决策有类型产物链合同 crate。
//!
//! 七类产物（[02] §7）：[`IntentCandidate`] / [`ObjectiveSpec`] /
//! [`DecisionRequest`] / [`CandidatePlan`] / [`CertifiedPlan`] /
//! [`CommandIntent`] / [`CommandBatch`]——不同且**不可互换**；公开字段尽量
//! 私有、经校验构造器产生（缺字段/过期/digest 错/scope 错在构造即拒绝）。
//!
//! # 构造权收口（[06] §3.3）
//!
//! - `CertifiedPlan::new` 只在认证侧可见：未启用 `transition-certifier`
//!   feature 的 crate **无法构造** [`CertifiedPlan`]（seal 类型无公开构造
//!   器；compile-fail 测试锁定）。外部只有 `decode`+`verify`。
//! - [`CommandBatch::compile`] 只接受 `&CertifiedPlan` 与
//!   `&dyn ResourceRegistry`（蓝图 65 WP-65.2：原直填隐藏零值的第二
//!   免检构造门删除——零值 compiler_digest 与误装配不可分；批级元数据
//!   经 builder 显式编译）。
//! - `AuthorizedCommand` **不在本 crate 定义或重导出**（[06] §3.2 红线）：
//!   它仍由唯一 ControlGateway 构造（telemetry-model::command 单源）。
//!
//! # 依赖方向
//!
//! `adapters/model/solver → decision-contract ← certifier/compiler`；本
//! crate 只依赖 telemetry-model 值对象 + sha2。无 std、无 LLM/solver/
//! protocol plugin/dispatch 依赖。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod batch;
pub mod candidate;
pub mod certificate;
pub mod error;
pub mod ids;
pub mod intent;
pub mod keyed;
pub mod objective;
pub mod wire;

pub use batch::{CommandBatch, EmptyResourceRegistry, ResourceRegistry, MAX_BATCH_SIZE};
pub use candidate::{Assignment, CandidatePlan, CandidatePlanParts, SolveStatusKind};
pub use certificate::{CertifiedPlan, CertifiedPlanFields, CertifierSeal};
pub use error::ContractError;
pub use ids::{CertificateId, IntentId, PlanId, ReasonCode, SnapshotId};
pub use intent::{CommandIntent, CommandIntentParts, CompensationHint, Precondition};
pub use objective::{DecisionRequest, IntentCandidate, ObjectiveSpec};

/// 合同 wire 版本（V1；[06] §4.3 兼容矩阵 N-1 读窗口一个版本）。
pub const CONTRACT_WIRE_V1: u16 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    /// [06] §3.2 红线：AuthorizedCommand 不得从本 crate 泄漏（编译期
    /// 事实——telemetry-model::command 存在，本 crate 无 re-export）。
    #[test]
    fn authorized_command_not_reexported() {
        // telemetry-model 有（唯一权威）：
        let _ = core::any::type_name::<eneros_telemetry_model::command::CommandValue>();
        // 源码级审计（排除 /// // 注释行——文档提及不算泄漏）：任何代码行
        // 不得出现 AuthorizedCommand 定义/导入/重导出符号。
        for (file, src) in [
            ("batch.rs", include_str!("batch.rs")),
            ("intent.rs", include_str!("intent.rs")),
            ("certificate.rs", include_str!("certificate.rs")),
            ("candidate.rs", include_str!("candidate.rs")),
            ("objective.rs", include_str!("objective.rs")),
            ("wire.rs", include_str!("wire.rs")),
            ("ids.rs", include_str!("ids.rs")),
            ("error.rs", include_str!("error.rs")),
            ("keyed.rs", include_str!("keyed.rs")),
        ] {
            for line in src.lines() {
                let t = line.trim_start();
                if t.starts_with("//") {
                    continue;
                }
                assert!(
                    !t.contains("AuthorizedCommand"),
                    "{file} 代码行出现 AuthorizedCommand：{line}"
                );
            }
        }
    }

    /// reason code 封闭枚举稳定性（[06] §3.6：码稳定 + 未知 fail-closed）。
    #[test]
    fn reason_codes_stable_and_unknown_fail_closed() {
        for (code, expected) in [
            (1, ReasonCode::PriceArbitrage),
            (2, ReasonCode::PeakShave),
            (3, ReasonCode::ValleyFill),
            (4, ReasonCode::RenewableCurtail),
            (5, ReasonCode::GridSupport),
            (6, ReasonCode::SocBalance),
            (7, ReasonCode::ManualOperator),
            (8, ReasonCode::ContractObligation),
            (9, ReasonCode::EmergencyLimit),
            (10, ReasonCode::ModelRecommendation),
        ] {
            assert_eq!(ReasonCode::from_code(code), Some(expected));
            assert_eq!(expected.code(), code);
        }
        assert_eq!(ReasonCode::from_code(0), None, "0 保留");
        assert_eq!(ReasonCode::from_code(11), None, "未登记码 fail-closed");
        assert_eq!(ReasonCode::from_code(u8::MAX), None);
    }
}
