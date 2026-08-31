//! 蓝图 61 Phase 6 §3/§4：`model-harness` — 生产模型唯一入口。
//!
//! 唯一不变量（[11] §3.1/§4.1）：生产模型只有在模型包、prompt、skill、
//! 工具、输入证据、输出 schema、预算和发布状态全部匹配 ProductManifest
//! 时才能推理；模型编排只能执行签名 Skill 中的白名单步骤和工具，在
//! max_steps/deadline/token/tool budgets 内终止，并提交有类型候选。
//!
//! # 依赖方向红线
//!
//! 本 crate 不依赖 writer/Gateway/control-dispatch；**不定义、不重导出
//! AuthorizedCommand**（[16] §5：model-harness 不依赖 writer）。输出形状
//! 只有候选产物：[`crate::drafts::Explanation`] / [`crate::drafts::ObjectiveDraft`]
//! / `eneros_decision_contract::IntentCandidate`——绝不生成授权命令。
//!
//! # 生产链路（固定顺序）
//!
//! `Harness 装载并验证 Skill → Loop 调工具和模型 → 输出候选 →
//! ObjectiveModule/人工审批 → solver → Certifier → Gateway`。
//!
//! ModelOff（全仓模型调用关闭）：Phase 0-5 全链完全可运行（本 crate 无
//! 任何被规则/solver 依赖的全局状态；`HarnessConfig::model_off` 直接短路
//! 一切模型路径）。

#![forbid(unsafe_code)]

extern crate alloc;

pub mod budget;
pub mod drafts;
pub mod error;
pub mod eval;
pub mod gate;
pub mod harness;
pub mod lifecycle;
pub mod r#loop;
pub mod manifest;
pub mod projector;
pub mod registry;
pub mod schema;
pub mod skill;
pub mod skills;
pub mod tools;

#[cfg(test)]
pub(crate) mod stubs;

pub use budget::{BudgetTracker, HarnessBudget};
pub use drafts::{Explanation, ObjectiveDraft};
pub use error::HarnessError;
pub use gate::{CandidateOutput, GateReject, OutputGate};
pub use harness::{HarnessConfig, HarnessOutcome, HarnessRequest, ModelHarness};
pub use lifecycle::{HealthSignal, Lifecycle, LifecyclePolicy};
pub use manifest::{ModelManifestV1, ModelPackage, PackageContents};
pub use projector::{InputProjector, ProjectedInput, ProjectionSpec};
pub use registry::ModelRegistry;
pub use schema::{SchemaField, SchemaType, StrictSchema};
pub use skill::{SkillManifestV1, SkillPackage};
pub use tools::{ToolCall, ToolId, ToolResult, ToolRuntime};

/// 契约 wire 版本（模型包 V1）。
pub const MODEL_PACKAGE_WIRE_V1: u16 = 1;

/// SHA-256（内容/文本摘要；manifest 字段名即算法）。
pub fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

/// 数据分类上限（telemetry-model `Classification` 码：0 Public .. 3
/// Restricted）。生产 manifest `data_class_max` 不得越过 Restricted，且
/// Restricted 数据默认拒绝远端模型（[11] §3.4）。
pub const DATA_CLASS_MAX_RESTRICTED: u8 = 3;

/// 能力闭包（[11] §3.8）：模型进程/本 crate 产物体不持有任何落设备面。
/// 类型上无 writer/Gateway key 字段——能力闭包是常量事实而非配置项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityClosure;

impl CapabilityClosure {
    /// 模型节点能力闭包：无 protocol writer、无 Gateway signing key、
    /// 无设备 capability（[11] §3.8/§4.3 禁止工具清单的静态对应）。
    pub const NONE: CapabilityClosure = CapabilityClosure;
    /// 可审计能力集合（只读投影/无副作用计算/候选提交/人工审批请求）。
    pub const fn allowed_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    /// [11] §3.5 步骤 8 / [16] §5 红线：本 crate 源码不得出现
    /// AuthorizedCommand 定义/导入/重导出（与 decision-contract 同纪律）。
    #[test]
    fn authorized_command_not_reachable() {
        for (file, src) in [
            ("budget.rs", include_str!("budget.rs")),
            ("drafts.rs", include_str!("drafts.rs")),
            ("error.rs", include_str!("error.rs")),
            ("eval.rs", include_str!("eval.rs")),
            ("gate.rs", include_str!("gate.rs")),
            ("harness.rs", include_str!("harness.rs")),
            ("lifecycle.rs", include_str!("lifecycle.rs")),
            ("loop.rs", include_str!("loop.rs")),
            ("manifest.rs", include_str!("manifest.rs")),
            ("projector.rs", include_str!("projector.rs")),
            ("registry.rs", include_str!("registry.rs")),
            ("schema.rs", include_str!("schema.rs")),
            ("skill.rs", include_str!("skill.rs")),
            ("skills.rs", include_str!("skills.rs")),
            ("tools.rs", include_str!("tools.rs")),
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

    /// [11] §3.10 退出 gate：ModelOff 时 Phase 0-5 链不依赖本 crate 任何
    /// 全局状态（源码级事实——无 static）。
    #[test]
    fn model_off_has_no_global_state() {
        for (file, src) in [
            ("budget.rs", include_str!("budget.rs")),
            ("gate.rs", include_str!("gate.rs")),
            ("harness.rs", include_str!("harness.rs")),
            ("manifest.rs", include_str!("manifest.rs")),
            ("registry.rs", include_str!("registry.rs")),
        ] {
            for line in src.lines() {
                let t = line.trim_start();
                if t.starts_with("//") || t.starts_with("pub const") {
                    continue;
                }
                assert!(!t.starts_with("static "), "{file} 出现全局 static：{line}");
            }
        }
    }
}
