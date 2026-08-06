//! `DeadlineMonitor` 实现（ARCH-G03：interface 与 implementation 分离）。

use super::*;

impl DeadlineMonitor {
    /// Create a monitor with a specific workload profile.
    pub fn new(workload: WorkloadProfile) -> Self {
        let mut stats = BTreeMap::new();
        let mut rings = BTreeMap::new();
        for spec in FROZEN_SLOS {
            stats.insert(
                spec.id,
                SloStats {
                    ring_head: 0,
                    ring_len: 0,
                    ..Default::default()
                },
            );
            rings.insert(spec.id, [0u64; RING_SIZE]);
        }
        DeadlineMonitor {
            stats,
            rings,
            current_degradation: DegradationAction::None,
            workload,
        }
    }

    /// Record a latency sample for a critical path.
    ///
    /// Returns the `MissClass` for this sample and the recommended
    /// `DegradationAction` based on the updated rolling stats.
    pub fn record(
        &mut self,
        slo_id: SloId,
        elapsed_ns: u64,
        timestamp_ns: u64,
    ) -> (MissClass, DegradationAction) {
        let _ = timestamp_ns; // retained for audit/export
        let spec = match frozen_slo(slo_id) {
            Some(s) => s,
            None => return (MissClass::Nominal, DegradationAction::None),
        };

        let class = MissClass::from_elapsed(spec, elapsed_ns);

        let st = self.stats.entry(slo_id).or_default();
        st.count += 1;
        if st.min_ns == 0 || elapsed_ns < st.min_ns {
            st.min_ns = elapsed_ns;
        }
        if elapsed_ns > st.max_ns {
            st.max_ns = elapsed_ns;
        }
        st.sum_ns = st.sum_ns.saturating_add(elapsed_ns);
        match class {
            MissClass::Missed => st.misses += 1,
            MissClass::Critical => st.criticals += 1,
            MissClass::Warning => st.warnings += 1,
            MissClass::Nominal => {}
        }

        // Push into ring buffer for percentile approximation.
        if let Some(ring) = self.rings.get_mut(&slo_id) {
            let head = st.ring_head;
            ring[head] = elapsed_ns;
            st.ring_head = (head + 1) % RING_SIZE;
            if st.ring_len < RING_SIZE {
                st.ring_len += 1;
            }
        }

        // Recompute degradation.
        let action = self.recompute_degradation();
        self.current_degradation = action;
        (class, action)
    }

    /// Recompute the aggregate degradation level from all SLO stats.
    ///
    /// Policy (deterministic, ordered by control-plane protection):
    /// 1. Any control-loop miss → EnterSafeState (control plane at risk).
    /// 2. Any safety-validation miss → EnterSafeState.
    /// 3. Any gateway-authorize miss → ShedOptionalServices + investigate.
    /// 4. Any IRQ/IPC/schedule/context-switch miss → PauseNonCriticalAgents.
    /// 5. Any P99 threshold breach (warning rate > 1%) → ThrottleAi.
    fn recompute_degradation(&self) -> DegradationAction {
        let mut action = DegradationAction::None;

        for spec in FROZEN_SLOS {
            let Some(st) = self.stats.get(&spec.id) else {
                continue;
            };
            // Hard miss on control-critical paths → safe state.
            if st.misses > 0 {
                match spec.id {
                    SloId::ControlLoop | SloId::SafetyValidation => {
                        return DegradationAction::EnterSafeState;
                    }
                    SloId::GatewayAuthorize => {
                        action = action.max(DegradationAction::ShedOptionalServices);
                    }
                    _ => {
                        action = action.max(DegradationAction::PauseNonCriticalAgents);
                    }
                }
            }
            // P99 threshold breach: if >1% of samples exceed P99 threshold,
            // throttle AI to reduce load.
            if st.count > 100 {
                let warning_rate = st.warnings + st.criticals + st.misses;
                if warning_rate * 100 > st.count {
                    action = action.max(DegradationAction::ThrottleAi);
                }
            }
            // Jitter breach.
            if st.jitter_ns() > spec.max_jitter_ns && st.count > 10 {
                action = action.max(DegradationAction::ThrottleAi);
            }
        }
        action
    }

    /// Current aggregate degradation level.
    pub fn current_degradation(&self) -> DegradationAction {
        self.current_degradation
    }

    /// Get stats for a specific SLO.
    pub fn stats(&self, id: SloId) -> Option<&SloStats> {
        self.stats.get(&id)
    }

    /// Workload profile under which measurements are taken.
    pub fn workload(&self) -> WorkloadProfile {
        self.workload
    }

    /// Approximate P99 from the ring buffer (nearest-rank method).
    ///
    /// Returns 0 if insufficient samples.
    pub fn p99_ns(&self, id: SloId) -> u64 {
        self.percentile_ns(id, 990)
    }

    /// Approximate P99.9 from the ring buffer.
    pub fn p999_ns(&self, id: SloId) -> u64 {
        self.percentile_ns(id, 999)
    }

    /// Nearest-rank percentile from the ring buffer.
    /// `pct` is 0..=1000 (e.g. 99 = P99, 999 = P99.9).
    pub fn percentile_ns(&self, id: SloId, pct: u32) -> u64 {
        let Some(ring) = self.rings.get(&id) else {
            return 0;
        };
        let Some(st) = self.stats.get(&id) else {
            return 0;
        };
        if st.ring_len == 0 {
            return 0;
        }
        // Collect valid samples into a vec and sort.
        let mut buf: Vec<u64> = ring[..st.ring_len].to_vec();
        buf.sort_unstable();
        // Nearest-rank: ceil(pct/1000 * n)
        let n = buf.len();
        let rank = (pct as u64 * n as u64).div_ceil(1000) as usize;
        let rank = rank.clamp(1, n);
        buf[rank - 1]
    }

    /// Produce a snapshot of all SLO stats for telemetry/export.
    pub fn snapshot(&self) -> SloSnapshot {
        let entries: Vec<SloSnapshotEntry> = FROZEN_SLOS
            .iter()
            .filter_map(|spec| {
                self.stats.get(&spec.id).map(|st| SloSnapshotEntry {
                    id: spec.id,
                    name: spec.id.name(),
                    count: st.count,
                    mean_ns: st.mean_ns(),
                    p99_ns: self.p99_ns(spec.id),
                    p999_ns: self.p999_ns(spec.id),
                    min_ns: st.min_ns,
                    max_ns: st.max_ns,
                    jitter_ns: st.jitter_ns(),
                    misses: st.misses,
                    deadline_ns: spec.deadline_ns,
                    pass: st.misses == 0 && self.p99_ns(spec.id) <= spec.p99_threshold_ns,
                })
            })
            .collect();
        SloSnapshot {
            workload: self.workload,
            degradation: self.current_degradation,
            entries,
        }
    }
}

