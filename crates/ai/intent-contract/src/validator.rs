//! 契约校验器（D12）.
//!
//! 实现 6 项校验规则 + 版本兼容性检查：
//! 1. 版本检查（schema_version ∈ supported_versions）
//! 2. request_id 非空
//! 3. intent.reason 非空（D12：契约比单步 Intent 严格）
//! 4. confidence ∈ [0.0, 1.0]
//! 5. priority ∈ [1, 5]
//! 6. time_range 顺序（start_period <= end_period）
//! 7. soc_target 范围（target_soc ∈ [0.0, 1.0]）

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::contract::IntentContract;
use crate::error::ContractError;

/// 契约校验器.
pub struct ContractValidator {
    /// 支持的 schema 版本列表.
    pub supported_versions: Vec<String>,
    /// 当前版本.
    pub current_version: String,
}

impl ContractValidator {
    /// 创建校验器.
    ///
    /// 默认支持版本：`1.0.0` / `1.1.0`，当前版本 `1.1.0`。
    pub fn new() -> Self {
        Self {
            supported_versions: vec![String::from("1.0.0"), String::from("1.1.0")],
            current_version: String::from("1.1.0"),
        }
    }

    /// 校验正向契约.
    ///
    /// 按 7 项规则依次校验，首项失败即返回（短路语义）。
    pub fn validate(&self, contract: &IntentContract) -> Result<(), ContractError> {
        // 1. 版本检查
        if !self.supported_versions.contains(&contract.schema_version) {
            return Err(ContractError::UnsupportedVersion(
                contract.schema_version.clone(),
            ));
        }
        // 2. request_id 非空
        if contract.request_id.is_empty() {
            return Err(ContractError::MissingField(String::from("request_id")));
        }
        // 3. intent.reason 非空（D12：契约比单步 Intent 严格）
        if contract.intent.reason.is_empty() {
            return Err(ContractError::MissingField(String::from("intent.reason")));
        }
        // 4. confidence ∈ [0.0, 1.0]
        if contract.intent.confidence < 0.0 || contract.intent.confidence > 1.0 {
            return Err(ContractError::InvalidValue(
                String::from("confidence"),
                String::from("confidence must be in [0.0, 1.0]"),
            ));
        }
        // 5. priority ∈ [1, 5]
        if contract.intent.priority < 1 || contract.intent.priority > 5 {
            return Err(ContractError::InvalidValue(
                String::from("priority"),
                String::from("priority must be in [1, 5]"),
            ));
        }
        // 6. time_range 顺序
        if let Some(time_range) = &contract.intent.time_range {
            if time_range.start_period > time_range.end_period {
                return Err(ContractError::InvalidValue(
                    String::from("time_range"),
                    String::from("start_period must be <= end_period"),
                ));
            }
        }
        // 7. soc_target 范围
        if let Some(soc_target) = &contract.intent.soc_target {
            if soc_target.target_soc < 0.0 || soc_target.target_soc > 1.0 {
                return Err(ContractError::InvalidValue(
                    String::from("soc_target"),
                    String::from("target_soc must be in [0.0, 1.0]"),
                ));
            }
        }
        Ok(())
    }

    /// 检查版本兼容性.
    pub fn is_compatible(&self, version: &str) -> bool {
        self.supported_versions.contains(&String::from(version))
    }
}

impl Default for ContractValidator {
    fn default() -> Self {
        Self::new()
    }
}
