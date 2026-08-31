//! 结构化错误（一切拒绝可观测；[11] §3.6 超时/OOM/未加载/无效输出/
//! tool error 都是结构化拒绝，不 fallback 到未认证默认命令）。

use core::fmt;

/// 模型 harness 错误（闭集）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessError {
    /// ModelOff：模型路径整体关闭（Phase 0-5 链不依赖模型）。
    ModelOff,
    /// 包不存在/未注册。
    UnknownPackage,
    /// 包生命周期不允许推理（Registered/OfflineEvaluated/Shadow 不推理，
    /// Suspended/Retired 停产）。
    NotApproved(LifecycleStage),
    /// 包有效期外。
    PackageExpired,
    /// 清单/内容校验失败（篡改/签名/摘要）。
    Manifest(&'static str),
    /// 运行时（runtime_id/runtime_version）与 manifest 不匹配。
    RuntimeMismatch,
    /// 硬件预算不足（memory_limit_bytes 超硬件能力）。
    HardwareInsufficient,
    /// 输入投影拒绝（schema 不匹配/分类越权/远端模型 + Restricted）。
    Projection(&'static str),
    /// 预算耗尽（deadline/token/tool/memory 任一）。
    BudgetExhausted(&'static str),
    /// 调度层错误（队满/超时/引擎）。
    Scheduler(&'static str),
    /// 模型未加载。
    ModelNotLoaded,
    /// 引擎推理失败。
    Infer(&'static str),
    /// 输出门拒绝（携带门步骤）。
    Gate(GateStage),
    /// 内部状态错误。
    Internal(&'static str),
}

/// 生命周期阶段（错误报告用；与 `lifecycle::Lifecycle` 一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleStage {
    Registered,
    OfflineEvaluated,
    Shadow,
    Canary,
    Approved,
    Suspended,
    Retired,
}

/// 输出门步骤（[11] §3.5 八步；fail-closed 可定位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateStage {
    /// 1. 字节/UTF-8/长度。
    Bytes,
    /// 2. 严格 JSON schema。
    Schema,
    /// 3. 未知字段/枚举/工具调用拒绝。
    UnknownField,
    /// 4. finite/range/unit/scope。
    Value,
    /// 5. 引用存在且版本一致。
    Reference,
    /// 6. confidence 只作元数据。
    Confidence,
    /// 7. policy/人工审批。
    Policy,
    /// 8. 候选产物构造。
    Construct,
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl core::error::Error for HarnessError {}

impl From<eneros_infer_scheduler::SchedulerError> for HarnessError {
    fn from(e: eneros_infer_scheduler::SchedulerError) -> Self {
        use eneros_infer_scheduler::SchedulerError as S;
        match e {
            S::QueueFull => HarnessError::Scheduler("queue-full"),
            S::Timeout => HarnessError::Scheduler("timeout"),
            S::CacheExhausted => HarnessError::Scheduler("cache-exhausted"),
            S::Engine(le) => HarnessError::from(le),
            S::NotScheduled => HarnessError::Internal("not-scheduled"),
            S::Cancelled => HarnessError::BudgetExhausted("cancelled"),
            S::LateResult => HarnessError::Internal("late-result"),
        }
    }
}

impl From<eneros_llm_engine::error::LlmError> for HarnessError {
    fn from(e: eneros_llm_engine::error::LlmError) -> Self {
        use eneros_llm_engine::error::LlmError as L;
        match e {
            L::LoadFailed => HarnessError::Infer("load failed"),
            L::InferFailed | L::Utf8Error | L::InvalidPrompt => HarnessError::Infer("infer"),
            L::InvalidPath => HarnessError::Internal("invalid path"),
            L::GpuUnavailable => HarnessError::HardwareInsufficient,
            L::ModelNotLoaded => HarnessError::ModelNotLoaded,
            L::OutOfMemory => HarnessError::BudgetExhausted("oom"),
        }
    }
}
