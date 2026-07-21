//! Prompt 模板上下文（电力场景运行态数据）.

use alloc::string::String;
use alloc::vec::Vec;

/// Prompt 模板上下文.
///
/// 封装储能系统运行态数据，作为模板 `build` 的输入参数。
#[derive(Debug, Clone)]
pub struct TemplateContext {
    /// 当前市场电价（元/kWh）.
    pub market_price: f64,
    /// 电池荷电状态 SOC（百分比，0~100）.
    pub soc: f64,
    /// 当前充放电功率（kW，正=放电，负=充电）.
    pub power_current: f64,
    /// 电池温度（℃）.
    pub temperature: f64,
    /// 当前时段（"峰时" / "平时" / "谷时"）.
    pub time_of_day: String,
    /// 历史功率数据（最近 N 个采样点）.
    pub historical_data: Vec<f64>,
}

impl TemplateContext {
    /// 构造模板上下文.
    pub fn new(
        market_price: f64,
        soc: f64,
        power_current: f64,
        temperature: f64,
        time_of_day: String,
        historical_data: Vec<f64>,
    ) -> Self {
        Self {
            market_price,
            soc,
            power_current,
            temperature,
            time_of_day,
            historical_data,
        }
    }
}

/// 默认上下文（price=0.5, soc=50.0, power=0.0, temp=25.0, time="谷时", history=空）.
impl Default for TemplateContext {
    fn default() -> Self {
        Self {
            market_price: 0.5,
            soc: 50.0,
            power_current: 0.0,
            temperature: 25.0,
            time_of_day: String::from("谷时"),
            historical_data: Vec::new(),
        }
    }
}
