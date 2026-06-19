//! OpenAPI 3.0 自动文档生成 (v0.10.0 — Task 8)。
//!
//! 使用 `utoipa` 从已注解的 handler 和 `ToSchema` 类型自动生成
//! OpenAPI 3.0 规范。通过 `/api/openapi.json` 返回 JSON 文档，
//! 通过 `/docs` 提供 Swagger UI。

use utoipa::OpenApi;

use crate::handlers::actions::{
    StructuredActionRequestSchema, StructuredActionResponseSchema, StructuredActionSchema,
};
use crate::handlers::auth::{LoginRequest, LoginResponse};
use crate::handlers::soe::SoeResponse;
use crate::handlers::timeseries::{DataPointDto, TimeseriesResponse};
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
        eneros_timeseries::SoeRecord,
        SoeResponse,
    )),
    info(
        title = "EnerOS API",
        version = "0.10.0",
        description = "Power-Native Agent Operating System for electrical grid control",
    )
)]
pub struct OpenApiDoc;
