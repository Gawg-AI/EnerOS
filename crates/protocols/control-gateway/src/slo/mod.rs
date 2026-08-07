//! Real-time SLO: frozen deadlines, workload profile, deadline-miss monitor (CTL-012).
//!
//! MASTER §7.6: "实时 SLO 不可凭空定值。先建立代表性工作负载和 platform
//! profile，然后冻结 P50/P99/P99.9 latency、jitter、deadline miss 与最大
//! 中断屏蔽时间；之后所有 release 做回归。"
//!
//! MASTER §9.5: "关键控制 deadline 在 AI 满载、日志风暴和网络攻击下仍满足
//! 冻结 SLO。" "所有 deadline miss、budget overrun 和 starvation 进入
//! 结构化 telemetry。"
//!
//! This module provides:
//! - [`SloSpec`] — a frozen SLO threshold (deadline, jitter, period) for a
//!   critical path, identified by a stable [`SloId`].
//! - [`WorkloadProfile`] — the representative workload that must be running
//!   when SLO is measured (control loop active, N agents, AI at load,
//!   log storm, network stress). SLO numbers are meaningless without this.
//! - [`DeadlineMonitor`] — records actual vs. deadline per critical path,
//!   classifies misses, and emits [`DegradationAction`] when thresholds
//!   are violated.
//! - [`DegradationAction`] — the deterministic degradation policy: on
//!   deadline miss, AI workers are throttled first, then non-critical
//!   agents, then the control plane is protected at all costs.
//!
//! # Design
//!
//! SLOs are **frozen const data** — they are not configurable at runtime.
//! Changing an SLO requires a code change + ADR + regression re-baseline,
//! matching MASTER's "冻结" requirement. The [`FROZEN_SLOS`] slice is the
//! single source of truth.
//!
//! The monitor is a real-time component: every critical path calls
//! [`DeadlineMonitor::record`] with the actual elapsed time. The monitor
//! tracks P50/P99/P99.9 via a bounded ring of recent samples and emits
//! degradation actions when percentiles breach thresholds.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ============================================================================
// SLO identifiers — stable enum, not a string
// ============================================================================

/// Stable identifier for a critical path with an SLO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SloId {
    /// Control loop: sense → decide → actuate. The most critical deadline.
    ControlLoop = 0,
    /// IRQ entry to ISR completion.
    IrqLatency = 1,
    /// IPC round-trip (send → reply).
    IpcRoundTrip = 2,
    /// Safety validator verdict (must be < control loop sub-budget).
    SafetyValidation = 3,
    /// Gateway authorize (decode → HMAC → lease → policy).
    GatewayAuthorize = 4,
    /// Context switch (schedule → first user instruction).
    ContextSwitch = 5,
    /// Scheduler tick to next dispatch.
    ScheduleLatency = 6,
}

impl SloId {
    /// Human-readable name (for telemetry/audit).
    pub const fn name(self) -> &'static str {
        match self {
            SloId::ControlLoop => "control_loop",
            SloId::IrqLatency => "irq_latency",
            SloId::IpcRoundTrip => "ipc_round_trip",
            SloId::SafetyValidation => "safety_validation",
            SloId::GatewayAuthorize => "gateway_authorize",
            SloId::ContextSwitch => "context_switch",
            SloId::ScheduleLatency => "schedule_latency",
        }
    }
}

// ============================================================================
// Frozen SLO specification
// ============================================================================

/// A frozen SLO threshold for one critical path.
///
/// All times are in **nanoseconds** (monotonic clock). The values here are
/// the **frozen QEMU virtual platform baseline** (MASTER G9: virtual
/// qualification). G10 may tighten them for real hardware.
///
/// Each SLO has:
/// - `deadline_ns`: the absolute deadline. Exceeding this is a miss.
/// - `p99_threshold_ns`: the P99 latency must stay below this.
/// - `p999_threshold_ns`: the P99.9 latency must stay below this.
/// - `max_jitter_ns`: maximum observed jitter (max - min in window).
/// - `period_ns`: the expected period of this path (0 = aperiodic).
#[derive(Debug, Clone, Copy)]
pub struct SloSpec {
    pub id: SloId,
    pub deadline_ns: u64,
    pub p99_threshold_ns: u64,
    pub p999_threshold_ns: u64,
    pub max_jitter_ns: u64,
    pub period_ns: u64,
}

/// The frozen SLO set for the QEMU virtual platform baseline.
///
/// These values are deliberately conservative for a virtualized ARM64
/// environment and will be tightened on real hardware (G10). They are
/// **not** configurable — changing them requires an ADR and re-baseline.
///
/// Rationale (energy storage control domain):
/// - Control loop 10 ms period (100 Hz), deadline 8 ms (80% utilization cap).
/// - IRQ latency < 50 µs (GICv3 virtual).
/// - IPC round-trip < 100 µs (seL4 endpoint).
/// - Safety validation < 1 ms (must fit inside control loop).
/// - Gateway authorize < 2 ms (HMAC + lease + policy).
/// - Context switch < 20 µs.
/// - Schedule latency < 50 µs.
pub const FROZEN_SLOS: &[SloSpec] = &[
    SloSpec {
        id: SloId::ControlLoop,
        deadline_ns: 8_000_000,
        p99_threshold_ns: 6_000_000,
        p999_threshold_ns: 7_000_000,
        max_jitter_ns: 2_000_000,
        period_ns: 10_000_000,
    },
    SloSpec {
        id: SloId::IrqLatency,
        deadline_ns: 50_000,
        p99_threshold_ns: 30_000,
        p999_threshold_ns: 40_000,
        max_jitter_ns: 20_000,
        period_ns: 0,
    },
    SloSpec {
        id: SloId::IpcRoundTrip,
        deadline_ns: 100_000,
        p99_threshold_ns: 70_000,
        p999_threshold_ns: 80_000,
        max_jitter_ns: 30_000,
        period_ns: 0,
    },
    SloSpec {
        id: SloId::SafetyValidation,
        deadline_ns: 1_000_000,
        p99_threshold_ns: 500_000,
        p999_threshold_ns: 700_000,
        max_jitter_ns: 300_000,
        period_ns: 0,
    },
    SloSpec {
        id: SloId::GatewayAuthorize,
        deadline_ns: 2_000_000,
        p99_threshold_ns: 1_000_000,
        p999_threshold_ns: 1_500_000,
        max_jitter_ns: 500_000,
        period_ns: 0,
    },
    SloSpec {
        id: SloId::ContextSwitch,
        deadline_ns: 20_000,
        p99_threshold_ns: 15_000,
        p999_threshold_ns: 18_000,
        max_jitter_ns: 5_000,
        period_ns: 0,
    },
    SloSpec {
        id: SloId::ScheduleLatency,
        deadline_ns: 50_000,
        p99_threshold_ns: 30_000,
        p999_threshold_ns: 40_000,
        max_jitter_ns: 20_000,
        period_ns: 0,
    },
];

/// Look up a frozen SLO by id. Returns `None` if not in [`FROZEN_SLOS`].
pub fn frozen_slo(id: SloId) -> Option<&'static SloSpec> {
    FROZEN_SLOS.iter().find(|s| s.id == id)
}

// ============================================================================
// Workload profile — the conditions under which SLO must hold
// ============================================================================

/// The representative workload under which SLOs must be met.
///
/// MASTER §9.5: "关键控制 deadline 在 AI 满载、日志风暴和网络攻击下仍满足
/// 冻结 SLO。" This struct captures that context. SLO measurements taken
/// under a different workload profile are **not comparable** and must not
/// be used for release gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkloadProfile {
    /// Number of active control loops (plant devices).
    pub control_loops: u32,
    /// Number of running Agent processes.
    pub agents: u32,
    /// AI/solver workers at full load (0 = AI off).
    pub ai_workers: u32,
    /// Log storm active (max-rate structured logging).
    pub log_storm: bool,
    /// Network stress active (flood + partition).
    pub network_stress: bool,
    /// SMP core count.
    pub cores: u32,
}

impl Default for WorkloadProfile {
    fn default() -> Self {
        Self::frozen_baseline()
    }
}

impl WorkloadProfile {
    /// The frozen baseline workload for QEMU virtual platform SLO qualification.
    ///
    /// This is the workload that must be running when SLO regression is
    /// measured for G9 release gate. It represents a realistic energy-storage
    /// site under stress: 8 control loops, 16 agents, AI at full load,
    /// log storm, network stress, 4 cores.
    pub const fn frozen_baseline() -> Self {
        WorkloadProfile {
            control_loops: 8,
            agents: 16,
            ai_workers: 4,
            log_storm: true,
            network_stress: true,
            cores: 4,
        }
    }

    /// Human-readable digest for evidence/audit.
    pub fn digest(&self) -> String {
        format!(
            "cl{}_ag{}_ai{}_log{}_net{}_c{}",
            self.control_loops,
            self.agents,
            self.ai_workers,
            self.log_storm as u8,
            self.network_stress as u8,
            self.cores,
        )
    }
}

// ============================================================================
// Deadline monitor — real-time tracking + degradation
// ============================================================================

/// Maximum samples kept per SLO path (ring buffer).
const RING_SIZE: usize = 1024;

/// A single latency sample.
#[derive(Debug, Clone, Copy)]
pub struct LatencySample {
    pub slo_id: SloId,
    pub elapsed_ns: u64,
    pub timestamp_ns: u64,
}

/// Classification of a deadline event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissClass {
    /// Within P99 threshold — nominal.
    Nominal,
    /// Between P99 and P999 — warning.
    Warning,
    /// Between P999 and deadline — critical.
    Critical,
    /// Exceeded deadline — miss.
    Missed,
}

impl MissClass {
    pub fn from_elapsed(spec: &SloSpec, elapsed_ns: u64) -> MissClass {
        if elapsed_ns > spec.deadline_ns {
            MissClass::Missed
        } else if elapsed_ns > spec.p999_threshold_ns {
            MissClass::Critical
        } else if elapsed_ns > spec.p99_threshold_ns {
            MissClass::Warning
        } else {
            MissClass::Nominal
        }
    }
}

/// Degradation action emitted when SLO is violated.
///
/// The policy is deterministic and ordered: protect the control plane
/// at all costs. AI is degraded first, then non-critical agents, then
/// optional services. The control loop is never sacrificed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DegradationAction {
    /// No action needed — within thresholds.
    None,
    /// Throttle AI workers (reduce concurrency / pause inference).
    ThrottleAi,
    /// Pause non-critical agents (diagnostics, optimization).
    PauseNonCriticalAgents,
    /// Shed optional services (dashboards, telemetry export).
    ShedOptionalServices,
    /// Control plane at risk — enter safe state (fail-safe).
    EnterSafeState,
}

impl DegradationAction {
    /// Severity ranking (higher = more severe).
    pub fn severity(self) -> u8 {
        match self {
            DegradationAction::None => 0,
            DegradationAction::ThrottleAi => 1,
            DegradationAction::PauseNonCriticalAgents => 2,
            DegradationAction::ShedOptionalServices => 3,
            DegradationAction::EnterSafeState => 4,
        }
    }

    /// Pick the more severe of two actions.
    pub fn max(self, other: DegradationAction) -> DegradationAction {
        if self.severity() >= other.severity() {
            self
        } else {
            other
        }
    }
}

/// Per-SLO statistics computed from the sample ring.
#[derive(Debug, Clone, Copy, Default)]
pub struct SloStats {
    pub count: u64,
    pub min_ns: u64,
    pub max_ns: u64,
    pub sum_ns: u64,
    pub misses: u64,
    pub warnings: u64,
    pub criticals: u64,
    /// Ring buffer index for next write.
    pub ring_head: usize,
    pub ring_len: usize,
}

impl SloStats {
    /// Mean latency in ns (0 if no samples).
    pub fn mean_ns(&self) -> u64 {
        self.sum_ns.checked_div(self.count).unwrap_or_default()
    }

    /// Jitter = max - min.
    pub fn jitter_ns(&self) -> u64 {
        self.max_ns.saturating_sub(self.min_ns)
    }
}

/// The deadline monitor. Owns a ring buffer of recent samples per SLO path
/// and computes statistics + degradation actions.
///
/// This is a real-time component: critical paths call `record()` with
/// measured elapsed time. The monitor tracks rolling stats and emits
/// degradation actions.
pub struct DeadlineMonitor {
    stats: BTreeMap<SloId, SloStats>,
    rings: BTreeMap<SloId, [u64; RING_SIZE]>,
    /// Current aggregate degradation level.
    current_degradation: DegradationAction,
    /// Workload profile under which measurements are taken.
    workload: WorkloadProfile,
}

impl Default for DeadlineMonitor {
    fn default() -> Self {
        Self::new(WorkloadProfile::frozen_baseline())
    }
}

/// A snapshot entry for one SLO path.
#[derive(Debug, Clone)]
pub struct SloSnapshotEntry {
    pub id: SloId,
    pub name: &'static str,
    pub count: u64,
    pub mean_ns: u64,
    pub p99_ns: u64,
    pub p999_ns: u64,
    pub min_ns: u64,
    pub max_ns: u64,
    pub jitter_ns: u64,
    pub misses: u64,
    pub deadline_ns: u64,
    /// True if no misses AND P99 within threshold.
    pub pass: bool,
}

/// A complete SLO snapshot for telemetry/audit/release evidence.
#[derive(Debug, Clone)]
pub struct SloSnapshot {
    pub workload: WorkloadProfile,
    pub degradation: DegradationAction,
    pub entries: Vec<SloSnapshotEntry>,
}

impl SloSnapshot {
    /// True if ALL SLOs pass (no misses, P99 within threshold).
    pub fn all_pass(&self) -> bool {
        self.entries.iter().all(|e| e.pass)
    }

    /// Count of passing SLOs.
    pub fn pass_count(&self) -> usize {
        self.entries.iter().filter(|e| e.pass).count()
    }

    /// Total SLO count.
    pub fn total(&self) -> usize {
        self.entries.len()
    }
}

// ============================================================================
// Tests
// ============================================================================

/// DeadlineMonitor 实现（ARCH-G03 拆分）。
mod monitor;

#[cfg(test)]
mod tests;
