//! 电网可靠性分析示例插件
//!
//! 本 crate 编译为 cdylib 动态库，由 EnerOS 插件加载器通过 C ABI 加载。
//! 元数据通过同目录 `manifest.toml` 提供，故不导出 `eneros_plugin_metadata`。
//!
//! 功能说明（基于 IEEE 1366 的可靠性指标计算）：
//! - 输入 JSON：{ "customers_affected", "duration_minutes", "total_customers" }
//! - 输出 JSON：{ "saifi", "saidi", "caidi" }
//! - SAIFI = customers_affected / total_customers
//!   （系统平均停电频率指标，单位：次/户）
//! - SAIDI = (customers_affected * duration_minutes) / total_customers
//!   （系统平均停电持续时间指标，单位：分钟/户）
//! - CAIDI = SAIDI / SAIFI
//!   （客户平均停电持续时间指标，单位：分钟/次）
//!
//! 注意：本示例为演示插件接口契约的 stub 实现，仅计算单次停电事件的指标，
//! 不涉及多事件聚合、时间窗口统计等复杂逻辑。

use eneros_plugin::analysis::{AnalysisPlugin, AnalysisResult};
use eneros_plugin::PluginError;
use std::ffi::c_void;

/// 可靠性分析插件
pub struct ReliabilityAnalysisPlugin;

impl AnalysisPlugin for ReliabilityAnalysisPlugin {
    fn analyze_type(&self) -> &str {
        "reliability"
    }

    fn description(&self) -> &str {
        "Power grid reliability analysis (SAIFI/SAIDI/CAIDI)"
    }

    fn analyze(
        &self,
        input: &serde_json::Value,
    ) -> Result<AnalysisResult<serde_json::Value>, PluginError> {
        // 解析输入字段
        let customers_affected = input
            .get("customers_affected")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                PluginError::InitFailed(
                    "missing or invalid field 'customers_affected'".to_string(),
                )
            })?;

        let duration_minutes = input
            .get("duration_minutes")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                PluginError::InitFailed(
                    "missing or invalid field 'duration_minutes'".to_string(),
                )
            })?;

        let total_customers = input
            .get("total_customers")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| {
                PluginError::InitFailed("missing or invalid field 'total_customers'".to_string())
            })?;

        // 校验输入合法性
        if total_customers <= 0.0 {
            return Err(PluginError::InitFailed(
                "'total_customers' must be positive".to_string(),
            ));
        }
        if customers_affected < 0.0 {
            return Err(PluginError::InitFailed(
                "'customers_affected' must be non-negative".to_string(),
            ));
        }
        if duration_minutes < 0.0 {
            return Err(PluginError::InitFailed(
                "'duration_minutes' must be non-negative".to_string(),
            ));
        }

        // 计算可靠性指标
        let saifi = customers_affected / total_customers;
        let saidi = (customers_affected * duration_minutes) / total_customers;
        // SAIFI 为 0 时（无停电），CAIDI 定义为 0 避免除零
        let caidi = if saifi > 0.0 { saidi / saifi } else { 0.0 };

        let mut warnings = Vec::new();
        // 停电用户数超过总用户数时给出告警（数据可能异常）
        if customers_affected > total_customers {
            warnings.push(format!(
                "customers_affected ({}) exceeds total_customers ({})",
                customers_affected, total_customers
            ));
        }

        let output = serde_json::json!({
            "saifi": saifi,
            "saidi": saidi,
            "caidi": caidi,
        });

        Ok(AnalysisResult::new(output).with_warnings(warnings))
    }
}

/// C ABI 入口：创建插件实例
///
/// 返回堆分配的 `Box<ReliabilityAnalysisPlugin>` 裸指针，调用方负责通过
/// `eneros_plugin_destroy` 释放。
///
/// 注意：通过 C ABI 传递的是瘦指针（具体类型），加载器在需要时
/// 可将其包装为 `dyn AnalysisPlugin` trait object。
///
/// # Safety
///
/// 调用方必须保证返回的指针仅通过 `eneros_plugin_destroy` 释放一次，
/// 且在销毁前不得解引用为其他类型。
#[no_mangle]
pub unsafe extern "C" fn eneros_plugin_create() -> *mut c_void {
    let plugin: Box<ReliabilityAnalysisPlugin> = Box::new(ReliabilityAnalysisPlugin);
    Box::into_raw(plugin) as *mut c_void
}

/// C ABI 入口：销毁插件实例
///
/// 接收 `eneros_plugin_create` 返回的指针并释放其内存。
/// 传入空指针时为空操作。
///
/// # Safety
///
/// `ptr` 必须为 `eneros_plugin_create` 的返回值或空指针，
/// 且同一指针不得销毁超过一次。
#[no_mangle]
pub unsafe extern "C" fn eneros_plugin_destroy(ptr: *mut c_void) {
    if !ptr.is_null() {
        // SAFETY: ptr 由 eneros_plugin_create 通过 Box::into_raw 产生，
        // 调用方保证仅释放一次。
        let _ = Box::from_raw(ptr as *mut ReliabilityAnalysisPlugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_type() {
        let plugin = ReliabilityAnalysisPlugin;
        assert_eq!(plugin.analyze_type(), "reliability");
        assert_eq!(
            plugin.description(),
            "Power grid reliability analysis (SAIFI/SAIDI/CAIDI)"
        );
    }

    #[test]
    fn test_analyze_success() {
        let plugin = ReliabilityAnalysisPlugin;
        let input = serde_json::json!({
            "customers_affected": 100,
            "duration_minutes": 30,
            "total_customers": 1000,
        });
        let result = plugin.analyze(&input).unwrap();
        assert!(result.converged);
        let saifi = result.result.get("saifi").and_then(|v| v.as_f64()).unwrap();
        let saidi = result.result.get("saidi").and_then(|v| v.as_f64()).unwrap();
        let caidi = result.result.get("caidi").and_then(|v| v.as_f64()).unwrap();
        assert!((saifi - 0.1).abs() < 1e-9);
        assert!((saidi - 3.0).abs() < 1e-9);
        assert!((caidi - 30.0).abs() < 1e-9);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_analyze_no_outage() {
        // 无停电事件：SAIFI=0, SAIDI=0, CAIDI=0（避免除零）
        let plugin = ReliabilityAnalysisPlugin;
        let input = serde_json::json!({
            "customers_affected": 0,
            "duration_minutes": 0,
            "total_customers": 1000,
        });
        let result = plugin.analyze(&input).unwrap();
        let saifi = result.result.get("saifi").and_then(|v| v.as_f64()).unwrap();
        let saidi = result.result.get("saidi").and_then(|v| v.as_f64()).unwrap();
        let caidi = result.result.get("caidi").and_then(|v| v.as_f64()).unwrap();
        assert!((saifi - 0.0).abs() < 1e-9);
        assert!((saidi - 0.0).abs() < 1e-9);
        assert!((caidi - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_analyze_warning_when_affected_exceeds_total() {
        let plugin = ReliabilityAnalysisPlugin;
        let input = serde_json::json!({
            "customers_affected": 1500,
            "duration_minutes": 30,
            "total_customers": 1000,
        });
        let result = plugin.analyze(&input).unwrap();
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("exceeds total_customers"));
    }

    #[test]
    fn test_analyze_missing_field() {
        let plugin = ReliabilityAnalysisPlugin;
        let input = serde_json::json!({
            "customers_affected": 100,
        });
        let err = plugin.analyze(&input).unwrap_err();
        assert!(matches!(err, PluginError::InitFailed(_)));
    }

    #[test]
    fn test_analyze_zero_total_customers() {
        let plugin = ReliabilityAnalysisPlugin;
        let input = serde_json::json!({
            "customers_affected": 100,
            "duration_minutes": 30,
            "total_customers": 0,
        });
        let err = plugin.analyze(&input).unwrap_err();
        assert!(matches!(err, PluginError::InitFailed(_)));
        assert!(err.to_string().contains("must be positive"));
    }

    #[test]
    fn test_analyze_negative_duration() {
        let plugin = ReliabilityAnalysisPlugin;
        let input = serde_json::json!({
            "customers_affected": 100,
            "duration_minutes": -5,
            "total_customers": 1000,
        });
        let err = plugin.analyze(&input).unwrap_err();
        assert!(matches!(err, PluginError::InitFailed(_)));
        assert!(err.to_string().contains("must be non-negative"));
    }
}
