//! OpenAPI 3.0 自动文档生成 (v0.10.0 — Task 8)。
//!
//! 使用 `utoipa` 从已注解的 handler 和 `ToSchema` 类型自动生成
//! OpenAPI 3.0 规范。通过 `/api/openapi.json` 返回 JSON 文档，
//! 通过 `/docs` 提供 Swagger UI。

use utoipa::OpenApi;

use crate::handlers::actions::{
    StructuredActionRequestSchema, StructuredActionResponseSchema, StructuredActionSchema,
};
use crate::handlers::agent_control::{AgentControlRequest, AgentControlResponse};
use crate::handlers::audit_query::{AuditEntryResponse, AuditQueryResponseSchema};
use crate::handlers::auth::{LoginRequest, LoginResponse};
use crate::handlers::log_level::{LogLevelRequest, LogLevelResponse};
use crate::handlers::compliance::{
    ComplianceFindingSchema, ComplianceRequestSchema, ComplianceResponseSchema,
    ComplianceStatusSchema,
};
use crate::handlers::planning::{
    CandidatePlanSchema, LoadingLimitsSchema, PlanningRequestSchema, PlanningResponseSchema,
    SupplyAreaClassSchema, SupplyRadiusSchema, VoltageLimitsSchema,
};
use crate::handlers::plugin_market::{
    MarketInstallRequest, MarketInstallResponse, MarketSearchResponse, MarketSearchResultEntry,
};
use crate::handlers::simulator::{
    FaultScenarioDto, ObservationDto, SimulatorRunRequest, SimulatorRunResponse,
    SimulatorScenariosResponse, SimulatorValidateRequest, SimulatorValidateResponse,
};
use crate::handlers::soe::SoeResponse;
use crate::handlers::timeseries::{DataPointDto, TimeseriesResponse};
use crate::handlers::validation::{
    ValidationFindingSchema, ValidationRequestSchema, ValidationResponseSchema,
    ValidationStatusSchema, ValidationSummarySchema,
};
use crate::handlers::whatif::{
    StructuredActionWhatIfSchema, WhatIfRequestSchema, WhatIfResponseSchema,
};
use crate::types::{
    BranchFlowResponse, BranchLimitRequest, BusVoltageResponse, GenBidRequest, OpfRequest,
    OpfResponse, PowerFlowRequest, PowerFlowResponse, ScadaLatestResponse, ScadaReadingResponse,
};

/// EnerOS API OpenAPI 文档根类型。
///
/// 派生 `OpenApi` 后，调用 `OpenApiDoc::openapi()` 即可获取完整的
/// `utoipa::openapi::OpenApi` 对象，可序列化为 JSON。
#[derive(OpenApi)]
#[openapi(
    paths(
        crate::handlers::powerflow::power_flow_handler,
        crate::handlers::analysis::opf_handler,
        crate::handlers::actions::structured_action_handler,
        crate::handlers::scada::scada_latest_handler,
        crate::handlers::timeseries::query_handler,
        crate::handlers::auth::login_handler,
        crate::handlers::soe::query_handler,
        crate::handlers::soe::latest_handler,
        // v0.28.0 — Task 17: 模拟器与插件市场端点
        crate::handlers::simulator::run_handler,
        crate::handlers::simulator::scenarios_handler,
        crate::handlers::simulator::validate_handler,
        crate::handlers::plugin_market::search_handler,
        crate::handlers::plugin_market::install_handler,
        // T029-08: Agent 控制 API
        crate::handlers::agent_control::control_handler,
        // T029-09: 校验/合规/规划/WhatIf/审计 API
        crate::handlers::validation::check_handler,
        crate::handlers::compliance::check_handler,
        crate::handlers::planning::evaluate_handler,
        crate::handlers::whatif::whatif_handler,
        crate::handlers::audit_query::query_handler,
        // T029-05: 日志级别动态调整 API
        crate::handlers::log_level::set_level_handler,
        crate::handlers::log_level::get_level_handler,
        // SSE Dashboard 实时流（详见 sse.rs，utoipa 不支持流式响应 schema）
        crate::handlers::sse::dashboard_stream,
    ),
    components(schemas(
        PowerFlowRequest,
        PowerFlowResponse,
        BusVoltageResponse,
        BranchFlowResponse,
        OpfRequest,
        OpfResponse,
        GenBidRequest,
        BranchLimitRequest,
        StructuredActionSchema,
        StructuredActionRequestSchema,
        StructuredActionResponseSchema,
        ScadaLatestResponse,
        ScadaReadingResponse,
        TimeseriesResponse,
        DataPointDto,
        LoginRequest,
        LoginResponse,
        eneros_runtime::timeseries::SoeRecord,
        SoeResponse,
        // v0.28.0 — Task 17: 模拟器与插件市场 schema
        SimulatorRunRequest,
        SimulatorRunResponse,
        ObservationDto,
        FaultScenarioDto,
        SimulatorScenariosResponse,
        SimulatorValidateRequest,
        SimulatorValidateResponse,
        MarketSearchResultEntry,
        MarketSearchResponse,
        MarketInstallRequest,
        MarketInstallResponse,
        // T029-08: Agent 控制 schema
        AgentControlRequest,
        AgentControlResponse,
        // T029-09: 校验 API schema
        ValidationStatusSchema,
        ValidationFindingSchema,
        ValidationSummarySchema,
        ValidationRequestSchema,
        ValidationResponseSchema,
        // T029-09: 合规 API schema
        ComplianceStatusSchema,
        ComplianceFindingSchema,
        ComplianceRequestSchema,
        ComplianceResponseSchema,
        // T029-09: 规划 API schema
        SupplyAreaClassSchema,
        VoltageLimitsSchema,
        LoadingLimitsSchema,
        SupplyRadiusSchema,
        CandidatePlanSchema,
        PlanningRequestSchema,
        PlanningResponseSchema,
        // T029-09: WhatIf API schema
        StructuredActionWhatIfSchema,
        WhatIfRequestSchema,
        WhatIfResponseSchema,
        // T029-09: 审计 API schema
        AuditEntryResponse,
        AuditQueryResponseSchema,
        // T029-05: 日志级别 API schema
        LogLevelRequest,
        LogLevelResponse,
    )),
    tags(
        (name = "simulator", description = "场景模拟器 API — 运行/校验场景脚本、列出内置故障场景"),
        (name = "plugin_market", description = "插件市场 API — 搜索/安装远程插件"),
        (name = "agent_control", description = "Agent 控制 API — start/stop/pause/resume/status"),
        (name = "validation", description = "系统校验 API — 电压/频率/谐波/N-1/短路等国标校验"),
        (name = "compliance", description = "合规检查 API — 设备合规性检查（GB/T 标准）"),
        (name = "planning", description = "配电网规划 API — 规划评估与候选方案生成"),
        (name = "whatif", description = "What-If 推演 API — 动作可行性分析与投影"),
        (name = "audit", description = "审计日志 API — 安全操作审计日志查询"),
        (name = "log_level", description = "日志级别 API — 运行时动态调整日志级别"),
        (name = "dashboard", description = "Dashboard API — SSE 实时指标推送"),
    ),
    info(
        title = "EnerOS API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Power-Native Agent Operating System for electrical grid control",
    )
)]
pub struct OpenApiDoc;
