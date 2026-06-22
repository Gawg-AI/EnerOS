pub mod gateway;
pub mod safety;
pub mod command;
pub mod interlocking;
pub mod constraint_validator;
pub mod decision_pipeline;
pub mod decision_cache;
pub mod priority_queue;
pub mod rt_executor;
pub mod watchdog;
pub mod pipeline_types;
pub mod precondition;
pub mod postcondition;
pub mod decomposer;
pub mod executor;
pub mod client;
pub mod server;

pub use gateway::SafetyGateway;
pub use safety::SafetyCheck;
pub use command::{Command, CommandPriority, CommandType, DeviceValue};
pub use priority_queue::{PriorityCommandQueue, SharedPriorityCommandQueue};
pub use rt_executor::{CommandResult, ExecutorConfig, ExecutorStats, RealtimeExecutor};
pub use watchdog::{WatchdogAction, WatchdogGuard, WatchdogTimeoutRecord, WatchdogTimer};
pub use pipeline_types::{
    DecisionContext, PreConditionResult, PreConditionCheck,
    PostConditionResult, PostConditionVerification,
    ActionStep, DecomposedAction,
    RollbackStep, RollbackPlan, RollbackStrategy, RollbackExecution,
    PipelineStatistics, PipelineStatisticsSnapshot, EnhancedPipelineDecision, PipelineAuditEntry,
};
pub use precondition::PreConditionChecker;
pub use postcondition::PostConditionVerifier;
pub use decomposer::ActionDecomposer;
pub use decision_pipeline::{ConstrainedDecisionPipeline, ObservationProvider};
pub use decision_cache::{DecisionCache, DecisionCacheStats};
pub use executor::{CommandExecutor, DeviceCommandExecutor, LoggingExecutor, ExecutionResult};
pub use client::{
    LocalGatewayClient, RemoteGatewayClient, GatewayRequest, GatewayResponse,
};
pub use server::GatewayServer;
