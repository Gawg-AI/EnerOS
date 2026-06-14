/// Reasoning strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningStrategy {
    /// Event-driven, immediate response
    Reactive,
    /// Deep reasoning, multi-step planning
    Deliberative,
    /// Hybrid approach
    #[default]
    Hybrid,
}
