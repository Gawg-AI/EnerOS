use super::PreConditionChecker;
use crate::pipeline_types::{DecisionContext, PreConditionCheck, PreConditionResult};

impl PreConditionChecker {
    pub(super) fn check_shed_load_preconditions(
        &self,
        _zone_id: u32,
        amount_mw: f64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        if !amount_mw.is_finite() {
            result.add_check(PreConditionCheck {
                name: "shed_amount_finite".to_string(),
                passed: false,
                description: format!("Shed amount must be finite, got {}", amount_mw),
                failure_reason: Some(format!("Load shedding amount {} is not finite", amount_mw)),
            });
        } else {
            result.add_check(PreConditionCheck {
                name: "shed_amount_finite".to_string(),
                passed: true,
                description: format!("Shed amount {:.1} MW is finite", amount_mw),
                failure_reason: None,
            });
        }

        if amount_mw <= 0.0 {
            result.add_check(PreConditionCheck {
                name: "shed_amount_positive".to_string(),
                passed: false,
                description: format!("Shed amount must be positive, got {:.1} MW", amount_mw),
                failure_reason: Some(format!(
                    "Load shedding amount {:.1} MW is not positive",
                    amount_mw
                )),
            });
        } else {
            result.add_check(PreConditionCheck {
                name: "shed_amount_positive".to_string(),
                passed: true,
                description: format!("Shed amount {:.1} MW is positive", amount_mw),
                failure_reason: None,
            });
        }

        if let Some(ref obs) = ctx.observation {
            if obs.total_load_mw > 0.0 {
                let fraction = amount_mw / obs.total_load_mw;
                if fraction > self.max_shed_fraction {
                    result.add_check(PreConditionCheck {
                        name: "shed_amount_fraction".to_string(),
                        passed: false,
                        description: format!(
                            "Shed {:.1}MW is {:.0}% of total load {:.1}MW (max {:.0}%)",
                            amount_mw,
                            fraction * 100.0,
                            obs.total_load_mw,
                            self.max_shed_fraction * 100.0
                        ),
                        failure_reason: Some(format!(
                            "Load shedding {:.1}MW ({:.0}%) exceeds maximum {:.0}% of total load",
                            amount_mw,
                            fraction * 100.0,
                            self.max_shed_fraction * 100.0
                        )),
                    });
                } else {
                    result.add_check(PreConditionCheck {
                        name: "shed_amount_fraction".to_string(),
                        passed: true,
                        description: format!(
                            "Shed {:.1}MW is {:.0}% of total load (within limit)",
                            amount_mw,
                            fraction * 100.0
                        ),
                        failure_reason: None,
                    });
                }
            }
        }

        if let Some(ref obs) = ctx.observation {
            if obs.frequency_hz < self.min_frequency_for_shedding {
                result.add_check(PreConditionCheck {
                    name: "shed_frequency_critical".to_string(),
                    passed: false,
                    description: format!(
                        "Frequency {:.2}Hz below minimum {:.2}Hz for load shedding",
                        obs.frequency_hz, self.min_frequency_for_shedding
                    ),
                    failure_reason: Some(format!(
                        "Cannot shed load when frequency {:.2}Hz is below critical threshold {:.2}Hz",
                        obs.frequency_hz, self.min_frequency_for_shedding
                    )),
                });
            }
        }
    }
}
