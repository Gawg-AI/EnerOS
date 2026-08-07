//! SLO 基线与 workload profile (OBS-011).
//!
//! SLO 基线是冻结的常量数据，代表在特定 workload profile 下
//! 各关键路径的延迟/jitter/deadline-miss 阈值。
//!
//! 设计原则（MASTER §7.6）：
//! - SLO 不可运行时配置，变更需代码修改 + ADR + 回归重测
//! - 每个基线绑定一个 workload profile（AI 负载、Agent 数量等）
//! - 合规状态由实际测量计算，禁止硬编码 Passed

use alloc::format;
use alloc::string::String;

/// SLO 基线 ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SloBaselineId {
    /// 控制回路延迟（10ms 周期）。
    ControlLoopLatency,
    /// IPC 往返延迟。
    IpcRoundTrip,
    /// 安全验证器裁决延迟。
    SafetyValidation,
    /// 网关授权延迟。
    GatewayAuthorize,
    /// 上下文切换延迟。
    ContextSwitch,
    /// Agent 响应延迟。
    AgentResponse,
    /// LLM 推理延迟（离线，非实时路径）。
    LlmInference,
    /// Solver 求解延迟。
    SolverSolve,
}

impl SloBaselineId {
    pub fn name(self) -> &'static str {
        match self {
            Self::ControlLoopLatency => "control-loop-latency",
            Self::IpcRoundTrip => "ipc-round-trip",
            Self::SafetyValidation => "safety-validation",
            Self::GatewayAuthorize => "gateway-authorize",
            Self::ContextSwitch => "context-switch",
            Self::AgentResponse => "agent-response",
            Self::LlmInference => "llm-inference",
            Self::SolverSolve => "solver-solve",
        }
    }
}

/// Workload profile — SLO 测量时的代表负载。
///
/// SLO 数值在无 workload 上下文时无意义。每个基线必须绑定一个 profile。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadProfile {
    /// Profile 名称。
    pub name: &'static str,
    /// 运行中的 Agent 数量。
    pub agent_count: u32,
    /// AI 推理负载（0=空闲, 100=满载）。
    pub ai_load_percent: u32,
    /// 日志速率（条/秒）。
    pub log_rate_per_sec: u32,
    /// 网络压力（0=无, 100=DDoS 级别）。
    pub network_stress_percent: u32,
    /// 是否有控制回路活跃。
    pub control_loop_active: bool,
}

/// SLO 基线 — 冻结的延迟阈值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SloBaseline {
    /// 基线 ID。
    pub id: SloBaselineId,
    /// 绑定的 workload profile。
    pub workload: WorkloadProfile,
    /// P50 目标（纳秒）。
    pub p50_target_ns: u64,
    /// P99 目标（纳秒）。
    pub p99_target_ns: u64,
    /// P99.9 目标（纳秒）。
    pub p999_target_ns: u64,
    /// 最大允许 deadline-miss 率（每百万次中的 miss 数）。
    pub max_deadline_miss_per_million: u64,
    /// 最大 jitter（纳秒，P99-P50）。
    pub max_jitter_ns: u64,
}

/// 冻结的 SLO 基线集合。
///
/// 这些是当前版本的冻结基线。变更需 ADR + 回归重测。
pub static FROZEN_BASELINES: &[SloBaseline] = &[
    SloBaseline {
        id: SloBaselineId::ControlLoopLatency,
        workload: WorkloadProfile {
            name: "edge-standard",
            agent_count: 8,
            ai_load_percent: 80,
            log_rate_per_sec: 1000,
            network_stress_percent: 50,
            control_loop_active: true,
        },
        p50_target_ns: 2_000_000,  // 2ms
        p99_target_ns: 5_000_000,  // 5ms
        p999_target_ns: 8_000_000, // 8ms
        max_deadline_miss_per_million: 100,
        max_jitter_ns: 3_000_000, // 3ms
    },
    SloBaseline {
        id: SloBaselineId::IpcRoundTrip,
        workload: WorkloadProfile {
            name: "edge-standard",
            agent_count: 8,
            ai_load_percent: 80,
            log_rate_per_sec: 1000,
            network_stress_percent: 50,
            control_loop_active: true,
        },
        p50_target_ns: 500_000,    // 0.5ms
        p99_target_ns: 2_000_000,  // 2ms
        p999_target_ns: 5_000_000, // 5ms
        max_deadline_miss_per_million: 500,
        max_jitter_ns: 1_500_000,
    },
    SloBaseline {
        id: SloBaselineId::SafetyValidation,
        workload: WorkloadProfile {
            name: "edge-standard",
            agent_count: 8,
            ai_load_percent: 80,
            log_rate_per_sec: 1000,
            network_stress_percent: 50,
            control_loop_active: true,
        },
        p50_target_ns: 100_000,    // 0.1ms
        p99_target_ns: 500_000,    // 0.5ms
        p999_target_ns: 1_000_000, // 1ms
        max_deadline_miss_per_million: 10,
        max_jitter_ns: 400_000,
    },
    SloBaseline {
        id: SloBaselineId::GatewayAuthorize,
        workload: WorkloadProfile {
            name: "edge-standard",
            agent_count: 8,
            ai_load_percent: 80,
            log_rate_per_sec: 1000,
            network_stress_percent: 50,
            control_loop_active: true,
        },
        p50_target_ns: 200_000,    // 0.2ms
        p99_target_ns: 1_000_000,  // 1ms
        p999_target_ns: 2_000_000, // 2ms
        max_deadline_miss_per_million: 100,
        max_jitter_ns: 800_000,
    },
    SloBaseline {
        id: SloBaselineId::AgentResponse,
        workload: WorkloadProfile {
            name: "edge-standard",
            agent_count: 8,
            ai_load_percent: 80,
            log_rate_per_sec: 1000,
            network_stress_percent: 50,
            control_loop_active: true,
        },
        p50_target_ns: 50_000_000,   // 50ms
        p99_target_ns: 200_000_000,  // 200ms
        p999_target_ns: 500_000_000, // 500ms
        max_deadline_miss_per_million: 1000,
        max_jitter_ns: 150_000_000,
    },
    SloBaseline {
        id: SloBaselineId::LlmInference,
        workload: WorkloadProfile {
            name: "edge-standard",
            agent_count: 8,
            ai_load_percent: 100,
            log_rate_per_sec: 1000,
            network_stress_percent: 0,
            control_loop_active: true,
        },
        p50_target_ns: 5_000_000_000,   // 5s
        p99_target_ns: 15_000_000_000,  // 15s
        p999_target_ns: 30_000_000_000, // 30s
        max_deadline_miss_per_million: 50_000,
        max_jitter_ns: 10_000_000_000,
    },
    SloBaseline {
        id: SloBaselineId::SolverSolve,
        workload: WorkloadProfile {
            name: "edge-standard",
            agent_count: 8,
            ai_load_percent: 80,
            log_rate_per_sec: 1000,
            network_stress_percent: 0,
            control_loop_active: true,
        },
        p50_target_ns: 100_000_000,    // 100ms
        p99_target_ns: 500_000_000,    // 500ms
        p999_target_ns: 1_000_000_000, // 1s
        max_deadline_miss_per_million: 5000,
        max_jitter_ns: 400_000_000,
    },
    // v0.241.0（B5-04）：补齐 ContextSwitch 基线（FROZEN_BASELINES 遗漏）。
    SloBaseline {
        id: SloBaselineId::ContextSwitch,
        workload: WorkloadProfile {
            name: "edge-standard",
            agent_count: 8,
            ai_load_percent: 80,
            log_rate_per_sec: 1000,
            network_stress_percent: 50,
            control_loop_active: true,
        },
        p50_target_ns: 10_000,   // 10us
        p99_target_ns: 50_000,   // 50us
        p999_target_ns: 100_000, // 100us
        max_deadline_miss_per_million: 1000,
        max_jitter_ns: 80_000,
    },
];

/// SLO 合规状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SloStatus {
    /// 合规（所有指标在阈值内）。
    Compliant,
    /// 接近违规（P99 > 80% 目标）。
    Warning,
    /// 违规（P99 超过目标或 deadline-miss 超限）。
    Violated,
    /// 无数据（未测量）。
    NoData,
}

impl SloStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Compliant => "COMPLIANT",
            Self::Warning => "WARNING",
            Self::Violated => "VIOLATED",
            Self::NoData => "NO_DATA",
        }
    }
}

/// SLO 合规报告 — 由实际测量数据计算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SloCompliance {
    /// 基线 ID。
    pub baseline_id: SloBaselineId,
    /// 实测 P50（纳秒）。
    pub measured_p50_ns: u64,
    /// 实测 P99（纳秒）。
    pub measured_p99_ns: u64,
    /// 实测 P99.9（纳秒）。
    pub measured_p999_ns: u64,
    /// 实测 deadline-miss 数（每百万次）。
    pub measured_deadline_miss_per_million: u64,
    /// 实测 jitter（纳秒）。
    pub measured_jitter_ns: u64,
    /// 合规状态。
    pub status: SloStatus,
}

impl SloCompliance {
    /// 从实测数据计算合规状态（禁止硬编码 Passed）。
    pub fn from_measurements(
        baseline: &SloBaseline,
        p50_ns: u64,
        p99_ns: u64,
        p999_ns: u64,
        deadline_miss_per_million: u64,
        jitter_ns: u64,
    ) -> Self {
        let p99_breach = p99_ns > baseline.p99_target_ns;
        let p999_breach = p999_ns > baseline.p999_target_ns;
        let miss_breach = deadline_miss_per_million > baseline.max_deadline_miss_per_million;
        let jitter_breach = jitter_ns > baseline.max_jitter_ns;

        let status = if p99_breach || p999_breach || miss_breach || jitter_breach {
            SloStatus::Violated
        } else if p99_ns > baseline.p99_target_ns * 80 / 100 {
            SloStatus::Warning
        } else {
            SloStatus::Compliant
        };

        Self {
            baseline_id: baseline.id,
            measured_p50_ns: p50_ns,
            measured_p99_ns: p99_ns,
            measured_p999_ns: p999_ns,
            measured_deadline_miss_per_million: deadline_miss_per_million,
            measured_jitter_ns: jitter_ns,
            status,
        }
    }

    /// 生成仪表盘 JSON 表示。
    pub fn to_dashboard_json(&self) -> String {
        format!(
            r#"{{"slo":"{}","p50_ns":{},"p99_ns":{},"p999_ns":{},"miss_per_million":{},"jitter_ns":{},"status":"{}"}}"#,
            self.baseline_id.name(),
            self.measured_p50_ns,
            self.measured_p99_ns,
            self.measured_p999_ns,
            self.measured_deadline_miss_per_million,
            self.measured_jitter_ns,
            self.status.label(),
        )
    }
}

/// 查找冻结基线。
pub fn find_baseline(id: SloBaselineId) -> Option<&'static SloBaseline> {
    FROZEN_BASELINES.iter().find(|b| b.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_baselines_exist() {
        assert!(FROZEN_BASELINES.len() >= 7);
    }

    #[test]
    fn control_loop_baseline_is_strict() {
        let baseline = find_baseline(SloBaselineId::ControlLoopLatency).unwrap();
        // 控制回路 P99 必须 < 10ms（MASTER 硬要求）
        assert!(baseline.p99_target_ns < 10_000_000);
        // deadline miss 率必须 < 1000/million
        assert!(baseline.max_deadline_miss_per_million < 1000);
    }

    #[test]
    fn safety_validation_is_fastest() {
        let safety = find_baseline(SloBaselineId::SafetyValidation).unwrap();
        let control = find_baseline(SloBaselineId::ControlLoopLatency).unwrap();
        assert!(safety.p99_target_ns < control.p99_target_ns);
    }

    #[test]
    fn compliance_compliant_when_under_target() {
        let baseline = find_baseline(SloBaselineId::ControlLoopLatency).unwrap();
        let compliance = SloCompliance::from_measurements(
            baseline, 1_000_000, // P50 < 2ms
            3_000_000, // P99 < 5ms
            6_000_000, // P999 < 8ms
            50,        // miss < 100/million
            2_000_000, // jitter < 3ms
        );
        assert_eq!(compliance.status, SloStatus::Compliant);
    }

    #[test]
    fn compliance_violated_when_p99_exceeds() {
        let baseline = find_baseline(SloBaselineId::ControlLoopLatency).unwrap();
        let compliance = SloCompliance::from_measurements(
            baseline, 3_000_000, 6_000_000, // P99 > 5ms target
            7_000_000, 50, 2_000_000,
        );
        assert_eq!(compliance.status, SloStatus::Violated);
    }

    #[test]
    fn compliance_violated_when_deadline_miss_exceeds() {
        let baseline = find_baseline(SloBaselineId::ControlLoopLatency).unwrap();
        let compliance = SloCompliance::from_measurements(
            baseline, 1_000_000, 3_000_000, 6_000_000, 200, // miss > 100/million limit
            2_000_000,
        );
        assert_eq!(compliance.status, SloStatus::Violated);
    }

    #[test]
    fn compliance_warning_when_p99_near_target() {
        let baseline = find_baseline(SloBaselineId::ControlLoopLatency).unwrap();
        // P99 = 4.5ms = 90% of 5ms target
        let compliance = SloCompliance::from_measurements(
            baseline, 2_000_000, 4_500_000, // > 80% of 5ms
            6_000_000, 50, 2_000_000,
        );
        assert_eq!(compliance.status, SloStatus::Warning);
    }

    #[test]
    fn compliance_violated_when_jitter_exceeds() {
        let baseline = find_baseline(SloBaselineId::ControlLoopLatency).unwrap();
        let compliance = SloCompliance::from_measurements(
            baseline, 1_000_000, 3_000_000, 6_000_000, 50, 4_000_000, // jitter > 3ms limit
        );
        assert_eq!(compliance.status, SloStatus::Violated);
    }

    #[test]
    fn dashboard_json_contains_status() {
        let baseline = find_baseline(SloBaselineId::IpcRoundTrip).unwrap();
        let compliance = SloCompliance::from_measurements(
            baseline, 400_000, 1_500_000, 3_000_000, 100, 1_000_000,
        );
        let json = compliance.to_dashboard_json();
        assert!(json.contains(r#""status":"COMPLIANT""#));
        assert!(json.contains(r#""slo":"ipc-round-trip""#));
    }

    #[test]
    fn all_baselines_have_workload_profile() {
        for baseline in FROZEN_BASELINES {
            assert!(!baseline.workload.name.is_empty());
            assert!(baseline.workload.agent_count > 0);
        }
    }

    #[test]
    fn llm_inference_is_not_realtime() {
        let llm = find_baseline(SloBaselineId::LlmInference).unwrap();
        let control = find_baseline(SloBaselineId::ControlLoopLatency).unwrap();
        // LLM P99 应远大于控制回路
        assert!(llm.p99_target_ns > control.p99_target_ns * 100);
    }
}
