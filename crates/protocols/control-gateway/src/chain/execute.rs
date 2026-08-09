//! `ControlChain::execute`/`replay_cache_len` 实现（ARCH-G03：interface 与 implementation 分离）。

use super::*;

impl ControlChain {
    /// Execute the full chain on an envelope buffer.
    ///
    /// `now` is epoch seconds (injected for testability). `feedback` is the
    /// raw telemetry bundle; the chain reconciles it (CTL-010) before the
    /// safety validator (CTL-008) consumes it.
    pub fn execute(&mut self, buf: &[u8], now: u64, feedback: &FeedbackBundle) -> ChainOutcome {
        // 1. Gateway: decode + HMAC + expiry + future + replay + structural lease/policy.
        let cmd = match self.gateway.authorize(buf, now) {
            Ok(c) => c,
            Err(e) => return ChainOutcome::Denied(ChainDenyReason::Gateway(e)),
        };

        // 2. Lease revocation (CTL-006) — real tombstone check. The gateway's
        //    structural checker only verified fencing_token > 0; here we
        //    enforce that the lease is still active and the token is current.
        match self
            .lease_service
            .check(&cmd.lease_id(), cmd.fencing_token(), now)
        {
            LeaseStatus::Valid => {}
            LeaseStatus::Revoked => {
                return ChainOutcome::Denied(ChainDenyReason::LeaseRevoked);
            }
            LeaseStatus::Stale => {
                return ChainOutcome::Denied(ChainDenyReason::StaleFencing);
            }
            LeaseStatus::Unknown => {
                return ChainOutcome::Denied(ChainDenyReason::LeaseRevoked);
            }
        }

        // 3. Idempotency (CTL-007) — exactly-once for retransmits.
        if let Some(prior) = self.idempotency.check(&cmd.idempotency_key(), now) {
            return ChainOutcome::IdempotentHit(prior);
        }

        // 4. Override authority (CTL-009) — local priority.
        let obligations = match self.override_auth.decide(&cmd, now) {
            OverrideDecision::SupersededByOverride => {
                self.idempotency
                    .record(cmd.idempotency_key(), OpOutcome::Rejected, now);
                return ChainOutcome::Denied(ChainDenyReason::SupersededByOverride);
            }
            OverrideDecision::EmergencyBypass => {
                // Emergency bypasses PDP (CTL-005) but NOT safety (CTL-008).
                Vec::new()
            }
            OverrideDecision::Normal => {
                // 5. Policy PDP (CTL-005) — semantic authorization, default-deny.
                let scope: LeaseScope = match self.lease_service.scope(&cmd.lease_id(), now) {
                    Some(s) => s.clone(),
                    None => {
                        // Lease vanished between check and scope — treat as revoked.
                        self.idempotency
                            .record(cmd.idempotency_key(), OpOutcome::Rejected, now);
                        return ChainOutcome::Denied(ChainDenyReason::LeaseRevoked);
                    }
                };
                match Pdp::new(&self.policy).decide(&cmd, &scope, now) {
                    Decision::Allow { obligations } => obligations,
                    Decision::Deny(reason) => {
                        self.idempotency
                            .record(cmd.idempotency_key(), OpOutcome::Rejected, now);
                        return ChainOutcome::Denied(ChainDenyReason::Policy(reason));
                    }
                }
            }
        };

        // 6. Select-before-execute (CTL-007) — discrete commands require a
        //    prior, non-expired select.
        match self.sbo.execute(cmd.point_id(), now) {
            SboExecuteResult::Executed => {}
            SboExecuteResult::NotSelected => {
                self.idempotency
                    .record(cmd.idempotency_key(), OpOutcome::Rejected, now);
                return ChainOutcome::Denied(ChainDenyReason::NotSelected);
            }
            SboExecuteResult::SelectExpired => {
                self.idempotency
                    .record(cmd.idempotency_key(), OpOutcome::Rejected, now);
                return ChainOutcome::Denied(ChainDenyReason::SelectExpired);
            }
        }

        // 7. Feedback reconciliation (CTL-010) → plant context for the
        //    independent safety validator (CTL-008).
        let (ctx, untrusted) = self.reconcile_context(feedback, now);
        if let Some(reason) = untrusted {
            // An untrusted sample drove the context to fail-safe. If the
            // command is a dangerous one, the safety validator will deny it
            // below. We still run the validator so the verdict reflects the
            // real safety rule (e.g. opening PCC on stale grid feedback may
            // still be safe). We surface the untrusted reason alongside the
            // safety verdict when the validator denies.
            let v = self.safety.validate(&cmd, &ctx);
            if let SafetyVerdict::Unsafe(safety_reason) = v {
                self.idempotency
                    .record(cmd.idempotency_key(), OpOutcome::Rejected, now);
                return ChainOutcome::Denied(ChainDenyReason::FeedbackUntrusted(format!(
                    "{reason}; safety={safety_reason}"
                )));
            }
            // Validator said Safe despite untrusted feedback — but we still
            // deny: acting on untrusted feedback is forbidden by CTL-010.
            self.idempotency
                .record(cmd.idempotency_key(), OpOutcome::Rejected, now);
            return ChainOutcome::Denied(ChainDenyReason::FeedbackUntrusted(reason));
        }

        // 8. Independent safety validator (CTL-008) with fully-trusted context.
        match self.safety.validate(&cmd, &ctx) {
            SafetyVerdict::Safe => {}
            SafetyVerdict::Unsafe(reason) => {
                self.idempotency
                    .record(cmd.idempotency_key(), OpOutcome::Rejected, now);
                return ChainOutcome::Denied(ChainDenyReason::Unsafe(reason));
            }
        }

        // 9. PEP: DryRun obligation — intercept before execution.
        if obligations.iter().any(|o| matches!(o, Obligation::DryRun)) {
            self.idempotency
                .record(cmd.idempotency_key(), OpOutcome::Applied, now);
            return ChainOutcome::DryRun { cmd, obligations };
        }

        // 10. All checks passed — record as applied for idempotency.
        self.idempotency
            .record(cmd.idempotency_key(), OpOutcome::Applied, now);
        ChainOutcome::Executed { cmd, obligations }
    }

    /// Reconcile every feedback sample (CTL-010) and build the plant context.
    /// Returns `(context, Option<untrusted_reason>)`: if any sample was stale,
    /// bad-quality or out-of-range, the context is driven to fail-safe values
    /// and the reason is returned so the chain can deny on untrusted feedback.
    fn reconcile_context(&self, fb: &FeedbackBundle, now: u64) -> (PlantContext, Option<String>) {
        let max_age = self.feedback_max_age_secs;
        let mut untrusted: Option<String> = None;

        // grid_up: 0.0..=1.0. Untrusted → false (PCC close denied).
        let grid_up = match reconcile(&fb.grid_up, now, max_age, 0.0, 1.0) {
            Reconciled::Ok(v) => v != 0.0,
            other => {
                untrusted = Some(format!("grid_up feedback {:?}", other));
                false
            }
        };
        // soc_pct: 0..=100. Untrusted → 0.0 (discharge denied at floor).
        let soc_pct = match reconcile(&fb.soc_pct, now, max_age, 0.0, 100.0) {
            Reconciled::Ok(v) => v,
            other => {
                if untrusted.is_none() {
                    untrusted = Some(format!("soc_pct feedback {:?}", other));
                }
                0.0
            }
        };
        // dc_bus_v: 0..=1000. Informational; untrusted → 0.0.
        let dc_bus_v = match reconcile(&fb.dc_bus_v, now, max_age, 0.0, 1000.0) {
            Reconciled::Ok(v) => v,
            other => {
                if untrusted.is_none() {
                    untrusted = Some(format!("dc_bus_v feedback {:?}", other));
                }
                0.0
            }
        };
        // overcurrent: 0.0..=1.0. Untrusted → true (inverter unblock denied).
        let overcurrent = match reconcile(&fb.overcurrent, now, max_age, 0.0, 1.0) {
            Reconciled::Ok(v) => v != 0.0,
            other => {
                if untrusted.is_none() {
                    untrusted = Some(format!("overcurrent feedback {:?}", other));
                }
                true
            }
        };
        // overtemp: 0.0..=1.0. Untrusted → true (charge denied).
        let overtemp = match reconcile(&fb.overtemp, now, max_age, 0.0, 1.0) {
            Reconciled::Ok(v) => v != 0.0,
            other => {
                if untrusted.is_none() {
                    untrusted = Some(format!("overtemp feedback {:?}", other));
                }
                true
            }
        };

        (
            PlantContext {
                grid_up,
                soc_pct,
                dc_bus_v,
                overcurrent,
                overtemp,
                maintenance_lockout: fb.maintenance_lockout,
            },
            untrusted,
        )
    }

    /// Number of entries currently held in the gateway replay cache.
    pub fn replay_cache_len(&self) -> usize {
        self.gateway.replay_cache_len()
    }
}

// ============================================================================
// Tests — full positive path + every deny path + fault scenarios
// ============================================================================
