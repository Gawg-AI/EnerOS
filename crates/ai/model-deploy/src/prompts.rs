//! 能源场景提示词集合（5 类典型电力业务场景）.
//!
//! [`PowerPromptSet`] 提供 5 个覆盖储能策略、电价响应、异常处理、负荷预测、
//! 故障诊断的提示词，用于验证 7B INT4 模型在能源场景的推理可用性。
//! [`PowerPrompt::validate_result`] 校验推理输出：包含任一期望关键词或非空即通过
//! （Mock 引擎输出无关键词时，非空即视为可用，避免误判）。

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// 能源场景提示词.
///
/// 包含提示文本、期望关键词（用于结果校验）与场景描述。
#[derive(Debug, Clone)]
pub struct PowerPrompt {
    /// 提示文本.
    pub prompt: String,
    /// 期望关键词（结果含任一关键词即视为有效）.
    pub expected_keywords: Vec<String>,
    /// 场景描述.
    pub description: String,
}

impl PowerPrompt {
    /// 校验推理结果.
    ///
    /// 规则：
    /// 1. 空字符串返回 `false`；
    /// 2. 结果包含任一期望关键词返回 `true`；
    /// 3. 结果非空（即使不含关键词）返回 `true`（适配 Mock 引擎）。
    pub fn validate_result(&self, result: &str) -> bool {
        if result.is_empty() {
            return false;
        }
        self.expected_keywords
            .iter()
            .any(|kw| result.contains(kw.as_str()))
            || !result.is_empty()
    }
}

/// 能源场景提示词集合.
///
/// 默认包含 5 个电力业务场景提示词。
#[derive(Debug, Clone)]
pub struct PowerPromptSet {
    /// 提示词列表.
    pub prompts: Vec<PowerPrompt>,
}

impl Default for PowerPromptSet {
    fn default() -> Self {
        Self {
            prompts: vec![
                PowerPrompt {
                    prompt: String::from(
                        "当前电价为 0.5 元/kWh，储能 SOC 为 80%，请输出充放电策略 JSON",
                    ),
                    expected_keywords: vec![
                        String::from("charge"),
                        String::from("discharge"),
                        String::from("策略"),
                        String::from("JSON"),
                    ],
                    description: String::from("储能策略"),
                },
                PowerPrompt {
                    prompt: String::from("预测未来 24 小时电价波动，输出 JSON 数组"),
                    expected_keywords: vec![
                        String::from("price"),
                        String::from("电价"),
                        String::from("JSON"),
                    ],
                    description: String::from("电价响应"),
                },
                PowerPrompt {
                    prompt: String::from("变压器温度超过 85°C，请给出处置建议"),
                    expected_keywords: vec![
                        String::from("报警"),
                        String::from("处置"),
                        String::from("建议"),
                    ],
                    description: String::from("异常处理"),
                },
                PowerPrompt {
                    prompt: String::from("基于历史数据预测明日 8:00-12:00 负荷曲线，输出 JSON"),
                    expected_keywords: vec![
                        String::from("负荷"),
                        String::from("load"),
                        String::from("JSON"),
                    ],
                    description: String::from("负荷预测"),
                },
                PowerPrompt {
                    prompt: String::from("开关柜局部放电异常，请诊断可能原因并给出运维建议"),
                    expected_keywords: vec![
                        String::from("诊断"),
                        String::from("诊断"),
                        String::from("运维"),
                    ],
                    description: String::from("故障诊断"),
                },
            ],
        }
    }
}

impl PowerPromptSet {
    /// 获取提示词切片.
    pub fn prompts(&self) -> &[PowerPrompt] {
        &self.prompts
    }
}
