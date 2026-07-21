//! 延迟分解测量（D1：alloc::format!）.

use alloc::format;
use alloc::string::String;

/// 延迟分解（7 环节 + total_ms）.
///
/// 记录双脑链路 7 个环节的耗时（ms），用于瓶颈识别与达标验证（< 2000ms）。
#[derive(Debug, Clone, Default)]
pub struct LatencyBreakdown {
    /// 感知层耗时（RealtimeState → SystemContext）.
    pub perception_ms: u64,
    /// LLM 推理耗时.
    pub llm_inference_ms: u64,
    /// 意图解析耗时（JSON → Intent → IntentContract）.
    pub intent_parse_ms: u64,
    /// LP 模型构建耗时.
    pub lp_build_ms: u64,
    /// LP 求解耗时.
    pub lp_solve_ms: u64,
    /// 安全校验耗时.
    pub safety_validate_ms: u64,
    /// 命令下发耗时.
    pub command_dispatch_ms: u64,
    /// 总耗时（7 环节之和）.
    pub total_ms: u64,
}

impl LatencyBreakdown {
    /// 累加 7 环节为 `total_ms`.
    pub fn calculate_total(&mut self) {
        self.total_ms = self.perception_ms
            + self.llm_inference_ms
            + self.intent_parse_ms
            + self.lp_build_ms
            + self.lp_solve_ms
            + self.safety_validate_ms
            + self.command_dispatch_ms;
    }

    /// 延迟达标（`total_ms < 2000`）.
    pub fn is_within_target(&self) -> bool {
        self.total_ms < 2000
    }

    /// 返回耗时最长环节名（全 0 返回 `"none"`）.
    pub fn bottleneck(&self) -> &'static str {
        let steps: [(&'static str, u64); 7] = [
            ("perception", self.perception_ms),
            ("llm_inference", self.llm_inference_ms),
            ("intent_parse", self.intent_parse_ms),
            ("lp_build", self.lp_build_ms),
            ("lp_solve", self.lp_solve_ms),
            ("safety_validate", self.safety_validate_ms),
            ("command_dispatch", self.command_dispatch_ms),
        ];
        match steps.iter().max_by_key(|&(_, ms)| ms) {
            Some(&(name, ms)) if ms > 0 => name,
            _ => "none",
        }
    }

    /// Markdown 表格格式化（D1：`alloc::format!`）.
    pub fn to_table(&self) -> String {
        format!(
            "| step | ms |\n|---|---|\n| perception | {} |\n| llm_inference | {} |\n| intent_parse | {} |\n| lp_build | {} |\n| lp_solve | {} |\n| safety_validate | {} |\n| command_dispatch | {} |\n| **total** | **{}** |",
            self.perception_ms,
            self.llm_inference_ms,
            self.intent_parse_ms,
            self.lp_build_ms,
            self.lp_solve_ms,
            self.safety_validate_ms,
            self.command_dispatch_ms,
            self.total_ms
        )
    }
}
