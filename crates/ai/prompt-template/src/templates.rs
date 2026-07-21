//! 3 个电力场景 Prompt 模板（充放电策略 / 功率调度 / 告警处理）.
//!
//! 每个模板定义 `const` 静态 SchemaField 数组与 SchemaSpec 常量（D4：编译期常量，
//! 运行时零分配）。

use alloc::format;
use alloc::string::String;

use crate::context::TemplateContext;
use crate::schema::{SchemaField, SchemaSpec, SchemaType};
use crate::template::PromptTemplate;

// ===== ChargeDischargeTemplate（充放电策略）=====
const CHARGE_DISCHARGE_FIELDS: &[SchemaField] = &[
    SchemaField {
        name: "action",
        field_type: SchemaType::String,
        required: true,
        enum_values: &["charge", "discharge", "standby"],
        minimum: None,
        maximum: None,
    },
    SchemaField {
        name: "power_kw",
        field_type: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
    SchemaField {
        name: "reason",
        field_type: SchemaType::String,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
    SchemaField {
        name: "confidence",
        field_type: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: Some(0.0),
        maximum: Some(1.0),
    },
];
const CHARGE_DISCHARGE_SCHEMA: SchemaSpec = SchemaSpec::new(CHARGE_DISCHARGE_FIELDS);

/// 充放电策略模板.
pub struct ChargeDischargeTemplate;

impl PromptTemplate for ChargeDischargeTemplate {
    fn name(&self) -> &'static str {
        "charge_discharge"
    }

    fn build(&self, ctx: &TemplateContext) -> String {
        format!(
            concat!(
                "你是一个储能系统调度助手。根据以下信息输出充放电策略。\n\n",
                "当前电价: {} 元/kWh\n",
                "电池 SOC: {}%\n",
                "当前功率: {} kW\n",
                "温度: {}℃\n",
                "时段: {}\n\n",
                "请输出 JSON 格式的充放电策略，包含以下字段：\n",
                "{{ \"action\": \"charge\" | \"discharge\" | \"standby\", \"power_kw\": <浮点数>, \"reason\": \"<简短理由>\", \"confidence\": <0.0~1.0> }}\n\n",
                "只输出 JSON，不要其他文字。",
            ),
            ctx.market_price, ctx.soc, ctx.power_current, ctx.temperature, ctx.time_of_day
        )
    }

    fn output_schema(&self) -> &'static SchemaSpec {
        &CHARGE_DISCHARGE_SCHEMA
    }
}

// ===== DispatchTemplate（功率调度）=====
const DISPATCH_FIELDS: &[SchemaField] = &[
    SchemaField {
        name: "target_power",
        field_type: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
    SchemaField {
        name: "ramp_rate",
        field_type: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
    SchemaField {
        name: "duration_minutes",
        field_type: SchemaType::Number,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
    SchemaField {
        name: "reason",
        field_type: SchemaType::String,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
];
const DISPATCH_SCHEMA: SchemaSpec = SchemaSpec::new(DISPATCH_FIELDS);

/// 功率调度模板.
pub struct DispatchTemplate;

impl PromptTemplate for DispatchTemplate {
    fn name(&self) -> &'static str {
        "dispatch"
    }

    fn build(&self, ctx: &TemplateContext) -> String {
        format!(
            concat!(
                "你是一个储能系统功率调度助手。根据以下信息输出功率调度策略。\n\n",
                "当前电价: {} 元/kWh\n",
                "电池 SOC: {}%\n",
                "当前功率: {} kW\n",
                "温度: {}℃\n",
                "时段: {}\n\n",
                "请输出 JSON 格式的功率调度策略，包含以下字段：\n",
                "{{ \"target_power\": <目标功率 kW>, \"ramp_rate\": <爬坡率 kW/min>, \"duration_minutes\": <持续分钟>, \"reason\": \"<简短理由> }}\n\n",
                "只输出 JSON，不要其他文字。",
            ),
            ctx.market_price, ctx.soc, ctx.power_current, ctx.temperature, ctx.time_of_day
        )
    }

    fn output_schema(&self) -> &'static SchemaSpec {
        &DISPATCH_SCHEMA
    }
}

// ===== AlarmTemplate（告警处理）=====
const ALARM_FIELDS: &[SchemaField] = &[
    SchemaField {
        name: "alarm_type",
        field_type: SchemaType::String,
        required: true,
        enum_values: &[
            "overvoltage",
            "undervoltage",
            "overcurrent",
            "overtemperature",
            "fault",
        ],
        minimum: None,
        maximum: None,
    },
    SchemaField {
        name: "severity",
        field_type: SchemaType::String,
        required: true,
        enum_values: &["info", "warning", "critical"],
        minimum: None,
        maximum: None,
    },
    SchemaField {
        name: "action",
        field_type: SchemaType::String,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
    SchemaField {
        name: "target_device",
        field_type: SchemaType::String,
        required: true,
        enum_values: &[],
        minimum: None,
        maximum: None,
    },
];
const ALARM_SCHEMA: SchemaSpec = SchemaSpec::new(ALARM_FIELDS);

/// 告警处理模板.
pub struct AlarmTemplate;

impl PromptTemplate for AlarmTemplate {
    fn name(&self) -> &'static str {
        "alarm"
    }

    fn build(&self, ctx: &TemplateContext) -> String {
        format!(
            concat!(
                "你是一个储能系统告警处理助手。根据以下信息输出告警处理策略。\n\n",
                "电池 SOC: {}%\n",
                "当前功率: {} kW\n",
                "温度: {}℃\n",
                "时段: {}\n\n",
                "请输出 JSON 格式的告警处理策略，包含以下字段：\n",
                "{{ \"alarm_type\": \"overvoltage\" | \"undervoltage\" | \"overcurrent\" | \"overtemperature\" | \"fault\", \"severity\": \"info\" | \"warning\" | \"critical\", \"action\": \"<处理动作>\", \"target_device\": \"<目标设备>\" }}\n\n",
                "只输出 JSON，不要其他文字。",
            ),
            ctx.soc, ctx.power_current, ctx.temperature, ctx.time_of_day
        )
    }

    fn output_schema(&self) -> &'static SchemaSpec {
        &ALARM_SCHEMA
    }
}
