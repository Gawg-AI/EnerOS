pub mod engine;
pub mod strategy;
pub mod context;
pub mod llm_prompt;
pub mod structured_output;
pub mod feedback;

#[cfg(feature = "rig")]
pub mod rig_engine;
#[cfg(feature = "rig")]
pub mod rig_tools;

pub use engine::{ReasoningEngine, ReasoningInput, ReasoningOutput, RuleBasedEngine, NumericRule, NumericField, ComparisonOperator, NumericRuleResult};
pub use strategy::ReasoningStrategy;
pub use context::ReasoningContextBuilder;

#[cfg(feature = "rig")]
pub use rig_engine::{RigReasoningEngine, RigConfig};
#[cfg(feature = "rig")]
pub use rig_tools::PowerSystemToolSet;
