use super::PreConditionChecker;
use crate::pipeline_types::{DecisionContext, PreConditionCheck, PreConditionResult};

impl PreConditionChecker {
    pub(super) fn check_generator_preconditions(
        &self,
        gen_id: u64,
        target_mw: f64,
        ctx: &DecisionContext,
        result: &mut PreConditionResult,
    ) {
        if !target_mw.is_finite() {
            result.add_check(PreConditionCheck {
                name: "generator_target_finite".to_string(),
                passed: false,
                description: format!("Generator {} target MW must be finite", gen_id),
                failure_reason: Some(format!(
                    "Generator {} target_mw={} is not finite",
                    gen_id, target_mw
                )),
            });
        } else {
            result.add_check(PreConditionCheck {
                name: "generator_target_finite".to_string(),
                passed: true,
                description: format!("Generator {} target MW={:.1} is finite", gen_id, target_mw),
                failure_reason: None,
            });
        }

        if target_mw < 0.0 {
            result.add_check(PreConditionCheck {
                name: "generator_target_positive".to_string(),
                passed: false,
                description: format!("Generator {} target MW must be non-negative", gen_id),
                failure_reason: Some(format!(
                    "Generator {} target_mw={:.1} is negative",
                    gen_id, target_mw
                )),
            });
        } else {
            result.add_check(PreConditionCheck {
                name: "generator_target_positive".to_string(),
                passed: true,
                description: format!(
                    "Generator {} target MW={:.1} is non-negative",
                    gen_id, target_mw
                ),
                failure_reason: None,
            });
        }

        if let Some(ref obs) = ctx.observation {
            if obs.frequency_hz < 47.5 || obs.frequency_hz > 51.5 {
                result.add_check(PreConditionCheck {
                    name: "generator_frequency_operable".to_string(),
                    passed: false,
                    description: format!(
                        "Frequency {:.2} Hz outside operable range [47.5, 51.5]",
                        obs.frequency_hz
                    ),
                    failure_reason: Some(format!(
                        "System frequency {:.2} Hz is outside generator operable range",
                        obs.frequency_hz
                    )),
                });
            } else {
                result.add_check(PreConditionCheck {
                    name: "generator_frequency_operable".to_string(),
                    passed: true,
                    description: format!(
                        "Frequency {:.2} Hz within operable range",
                        obs.frequency_hz
                    ),
                    failure_reason: None,
                });
            }
        }
    }
}
