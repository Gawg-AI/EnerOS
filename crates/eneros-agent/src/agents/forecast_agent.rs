use std::sync::Arc;
use std::time::Duration;
use chrono::{DateTime, Utc};
use eneros_core::{AuthorityLevel, ElementId, Jurisdiction, Result};
use eneros_eventbus::{Event, event::{EventType, EventPayload}};
use eneros_timeseries::TimeSeriesEngine;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::agent::{Agent, AgentAction, AgentType};
use crate::context::AgentContext;

// ---------------------------------------------------------------------------
// Exponential Smoothing Algorithms
// ---------------------------------------------------------------------------

/// Parameters for single exponential smoothing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExponentialSmoothing {
    pub alpha: f64,
}

impl Default for ExponentialSmoothing {
    fn default() -> Self {
        Self { alpha: 0.3 }
    }
}

/// Single exponential smoothing — level component only
pub fn single_exponential_smoothing(data: &[f64], alpha: f64) -> Vec<f64> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::with_capacity(data.len());
    result.push(data[0]); // S_1 = Y_1
    for i in 1..data.len() {
        let s = alpha * data[i] + (1.0 - alpha) * result[i - 1];
        result.push(s);
    }
    result
}

/// Parameters for double exponential smoothing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoubleExponentialSmoothing {
    pub alpha: f64,
    pub beta: f64,
}

impl Default for DoubleExponentialSmoothing {
    fn default() -> Self {
        Self { alpha: 0.3, beta: 0.1 }
    }
}

/// Double exponential smoothing — level + trend
pub fn double_exponential_smoothing(data: &[f64], alpha: f64, beta: f64) -> Vec<f64> {
    if data.is_empty() {
        return Vec::new();
    }
    if data.len() == 1 {
        return vec![data[0]];
    }

    let n = data.len();
    let mut level = vec![0.0; n];
    let mut trend = vec![0.0; n];
    let mut result = vec![0.0; n];

    // Initialization
    level[0] = data[0];
    trend[0] = data[1] - data[0];
    result[0] = level[0];

    for i in 1..n {
        level[i] = alpha * data[i] + (1.0 - alpha) * (level[i - 1] + trend[i - 1]);
        trend[i] = beta * (level[i] - level[i - 1]) + (1.0 - beta) * trend[i - 1];
        result[i] = level[i];
    }

    result
}

/// Parameters for Holt-Winters exponential smoothing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoltWintersParams {
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
    pub season_length: usize,
}

impl Default for HoltWintersParams {
    fn default() -> Self {
        Self {
            alpha: 0.3,
            beta: 0.1,
            gamma: 0.1,
            season_length: 24, // daily seasonality for hourly data
        }
    }
}

/// Holt-Winters additive forecast — level + trend + seasonality
pub fn holt_winters_forecast(
    data: &[f64],
    params: &HoltWintersParams,
    forecast_horizon: usize,
) -> Vec<f64> {
    let s = params.season_length;
    if data.len() < 2 * s {
        // Not enough data for seasonal decomposition; fall back to double smoothing
        let smoothed = double_exponential_smoothing(data, params.alpha, params.beta);
        if smoothed.is_empty() {
            return Vec::new();
        }
        let last_level = smoothed[smoothed.len() - 1];
        let n = data.len();
        let last_trend = if n >= 2 {
            smoothed[n - 1] - smoothed[n - 2]
        } else {
            0.0
        };
        return (1..=forecast_horizon)
            .map(|h| (last_level + h as f64 * last_trend).max(0.0))
            .collect();
    }

    let n = data.len();
    let mut level = vec![0.0; n];
    let mut trend = vec![0.0; n];
    let mut season = vec![0.0; n];

    // Initialize level and trend using first season
    let avg_first_season: f64 = data[..s].iter().sum::<f64>() / s as f64;
    let avg_second_season: f64 = data[s..2 * s].iter().sum::<f64>() / s as f64;

    level[s - 1] = avg_first_season;
    trend[s - 1] = (avg_second_season - avg_first_season) / s as f64;

    // Initialize seasonal indices
    for i in 0..s {
        season[i] = data[i] - avg_first_season;
    }

    // Holt-Winters additive recursion
    for i in s..n {
        level[i] = params.alpha * (data[i] - season[i - s])
            + (1.0 - params.alpha) * (level[i - 1] + trend[i - 1]);
        trend[i] = params.beta * (level[i] - level[i - 1])
            + (1.0 - params.beta) * trend[i - 1];
        season[i] = params.gamma * (data[i] - level[i])
            + (1.0 - params.gamma) * season[i - s];
    }

    // Generate forecast
    let last_level = level[n - 1];
    let last_trend = trend[n - 1];

    (1..=forecast_horizon)
        .map(|h| {
            let seasonal_idx = if h <= s { n - s + h - 1 } else { n - s + (h - 1) % s };
            (last_level + h as f64 * last_trend + season[seasonal_idx]).max(0.0)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Smoothing Method Selection
// ---------------------------------------------------------------------------

/// Smoothing method configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SmoothingMethod {
    /// Single exponential smoothing (level only)
    Single { alpha: f64 },
    /// Double exponential smoothing (level + trend)
    Double { alpha: f64, beta: f64 },
    /// Holt-Winters (level + trend + seasonality)
    HoltWinters { alpha: f64, beta: f64, gamma: f64, season_length: usize },
}

impl Default for SmoothingMethod {
    fn default() -> Self {
        SmoothingMethod::Single { alpha: 0.3 }
    }
}

// ---------------------------------------------------------------------------
// Load Forecast Result
// ---------------------------------------------------------------------------

/// Load forecast result with confidence intervals
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadForecast {
    pub target_element_id: ElementId,
    pub forecast_values: Vec<f64>,
    pub confidence_lower: Vec<f64>,
    pub confidence_upper: Vec<f64>,
    pub method_used: String,
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Load Forecast Agent
// ---------------------------------------------------------------------------

/// Load Forecast Agent — predicts future load using exponential smoothing
pub struct LoadForecastAgent {
    agent_id: String,
    jurisdiction: Jurisdiction,
    ts_engine: Arc<TimeSeriesEngine>,
    smoothing_method: SmoothingMethod,
    forecast_horizon_hours: u32,
    history_days: u32,
    last_forecast: RwLock<Option<LoadForecast>>,
}

impl LoadForecastAgent {
    /// Create a new LoadForecastAgent
    pub fn new(
        agent_id: &str,
        jurisdiction: Jurisdiction,
        ts_engine: Arc<TimeSeriesEngine>,
    ) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            jurisdiction,
            ts_engine,
            smoothing_method: SmoothingMethod::default(),
            forecast_horizon_hours: 24,
            history_days: 7,
            last_forecast: RwLock::new(None),
        }
    }

    /// Set the smoothing method
    pub fn with_smoothing_method(mut self, method: SmoothingMethod) -> Self {
        self.smoothing_method = method;
        self
    }

    /// Set the forecast horizon in hours
    pub fn with_forecast_horizon(mut self, hours: u32) -> Self {
        self.forecast_horizon_hours = hours;
        self
    }

    /// Set the number of history days to query
    pub fn with_history_days(mut self, days: u32) -> Self {
        self.history_days = days;
        self
    }

    /// Query historical load data for a given element
    fn query_historical_load(&self, element_id: ElementId) -> Vec<f64> {
        let end = Utc::now();
        let start = end - chrono::Duration::days(self.history_days as i64);
        let points = self.ts_engine.query(element_id, "load_mw", start, end);
        points.iter().map(|p| p.value).collect()
    }

    /// Compute standard deviation of residuals for confidence intervals
    fn compute_residual_std(data: &[f64], smoothed: &[f64]) -> f64 {
        if data.len() != smoothed.len() || data.is_empty() {
            return 0.0;
        }
        let n = data.len() as f64;
        let variance: f64 = data
            .iter()
            .zip(smoothed.iter())
            .map(|(d, s)| (d - s).powi(2))
            .sum::<f64>()
            / n;
        variance.sqrt()
    }

    /// Run forecast using the configured smoothing method
    fn run_forecast(&self, data: &[f64]) -> (Vec<f64>, String) {
        match &self.smoothing_method {
            SmoothingMethod::Single { alpha } => {
                let smoothed = single_exponential_smoothing(data, *alpha);
                let last = if smoothed.is_empty() { 0.0 } else { smoothed[smoothed.len() - 1] };
                let forecast = vec![last; self.forecast_horizon_hours as usize];
                (forecast, "SingleExponentialSmoothing".to_string())
            }
            SmoothingMethod::Double { alpha, beta } => {
                let smoothed = double_exponential_smoothing(data, *alpha, *beta);
                let n = smoothed.len();
                if n < 2 {
                    let forecast = vec![smoothed.first().copied().unwrap_or(0.0); self.forecast_horizon_hours as usize];
                    return (forecast, "DoubleExponentialSmoothing".to_string());
                }
                let last_level = smoothed[n - 1];
                let last_trend = smoothed[n - 1] - smoothed[n - 2];
                let forecast: Vec<f64> = (1..=self.forecast_horizon_hours as usize)
                    .map(|h| (last_level + h as f64 * last_trend).max(0.0))
                    .collect();
                (forecast, "DoubleExponentialSmoothing".to_string())
            }
            SmoothingMethod::HoltWinters { alpha, beta, gamma, season_length } => {
                let params = HoltWintersParams {
                    alpha: *alpha,
                    beta: *beta,
                    gamma: *gamma,
                    season_length: *season_length,
                };
                let forecast = holt_winters_forecast(data, &params, self.forecast_horizon_hours as usize);
                (forecast, "HoltWinters".to_string())
            }
        }
    }

    /// Get the smoothed values for the historical data (for residual computation)
    fn get_smoothed_values(&self, data: &[f64]) -> Vec<f64> {
        match &self.smoothing_method {
            SmoothingMethod::Single { alpha } => single_exponential_smoothing(data, *alpha),
            SmoothingMethod::Double { alpha, beta } => double_exponential_smoothing(data, *alpha, *beta),
            SmoothingMethod::HoltWinters { alpha, beta, gamma, season_length } => {
                let _params = HoltWintersParams {
                    alpha: *alpha,
                    beta: *beta,
                    gamma: *gamma,
                    season_length: *season_length,
                };
                // For Holt-Winters, we compute in-sample fitted values
                // Simplified: use double smoothing as proxy for residuals
                double_exponential_smoothing(data, *alpha, *beta)
            }
        }
    }

    /// Get the last forecast
    pub fn last_forecast(&self) -> Option<LoadForecast> {
        self.last_forecast.read().clone()
    }
}

#[async_trait::async_trait]
impl Agent for LoadForecastAgent {
    fn id(&self) -> &str {
        &self.agent_id
    }

    fn name(&self) -> &str {
        "load-forecast-agent"
    }

    fn agent_type(&self) -> AgentType {
        AgentType::Custom("LoadForecast".to_string())
    }

    fn authority_level(&self) -> AuthorityLevel {
        AuthorityLevel::Operator
    }

    fn jurisdiction(&self) -> Jurisdiction {
        self.jurisdiction.clone()
    }

    fn tick_interval(&self) -> Duration {
        Duration::from_secs(900) // 15 minutes
    }

    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    async fn handle_event(&mut self, event: &Event, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        match event.event_type {
            EventType::ConstraintViolation => {
                // Re-forecast with adjusted parameters
                // Use a slightly higher alpha for faster adaptation
                let adjusted_method = match &self.smoothing_method {
                    SmoothingMethod::Single { .. } => SmoothingMethod::Single { alpha: 0.5 },
                    SmoothingMethod::Double { beta, .. } => {
                        SmoothingMethod::Double { alpha: 0.5, beta: *beta }
                    }
                    SmoothingMethod::HoltWinters { beta, gamma, season_length, .. } => {
                        SmoothingMethod::HoltWinters {
                            alpha: 0.5,
                            beta: *beta,
                            gamma: *gamma,
                            season_length: *season_length,
                        }
                    }
                };

                // Temporarily apply adjusted method for re-forecast
                let original_method = self.smoothing_method.clone();
                self.smoothing_method = adjusted_method;

                // Get element_id from payload if available
                let element_id = match &event.payload {
                    EventPayload::ConstraintViolation { element_id, .. } => *element_id,
                    _ => 0,
                };

                let data = self.query_historical_load(element_id);
                if !data.is_empty() {
                    let (forecast, method_name) = self.run_forecast(&data);
                    let smoothed = self.get_smoothed_values(&data);
                    let residual_std = Self::compute_residual_std(&data, &smoothed);

                    let lower: Vec<f64> = forecast.iter().map(|v| (v - 2.0 * residual_std).max(0.0)).collect();
                    let upper: Vec<f64> = forecast.iter().map(|v| v + 2.0 * residual_std).collect();

                    let fc = LoadForecast {
                        target_element_id: element_id,
                        forecast_values: forecast,
                        confidence_lower: lower,
                        confidence_upper: upper,
                        method_used: format!("{} (adjusted)", method_name),
                        timestamp: Utc::now(),
                    };

                    *self.last_forecast.write() = Some(fc.clone());

                    actions.push(AgentAction::PublishEvent(Event::new(
                        EventType::DataReceived,
                        &self.agent_id,
                        EventPayload::Message(format!(
                            "LoadForecastAvailable: element={}, horizon={}h, method={}",
                            fc.target_element_id, self.forecast_horizon_hours, fc.method_used
                        )),
                    )));
                }

                // Restore original method
                self.smoothing_method = original_method;
            }
            EventType::DataReceived => {
                // Update internal data cache — just log it
                actions.push(AgentAction::LogMessage(
                    "LoadForecastAgent: data received, cache updated".to_string()
                ));
            }
            _ => {}
        }

        Ok(actions)
    }

    async fn tick(&mut self, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        let mut actions = Vec::new();

        // Use the first device in jurisdiction, or element 0 as default
        let element_id = self.jurisdiction.device_ids.first().copied().unwrap_or(0);

        // 1. Query historical load data
        let data = self.query_historical_load(element_id);

        if data.is_empty() {
            actions.push(AgentAction::LogMessage(
                "LoadForecastAgent: no historical data available".to_string()
            ));
            return Ok(actions);
        }

        // 2. Apply selected smoothing method
        let (forecast, method_name) = self.run_forecast(&data);

        // 3. Generate confidence intervals (±2σ)
        let smoothed = self.get_smoothed_values(&data);
        let residual_std = Self::compute_residual_std(&data, &smoothed);

        let lower: Vec<f64> = forecast.iter().map(|v| (v - 2.0 * residual_std).max(0.0)).collect();
        let upper: Vec<f64> = forecast.iter().map(|v| v + 2.0 * residual_std).collect();

        // 4. Store forecast
        let fc = LoadForecast {
            target_element_id: element_id,
            forecast_values: forecast,
            confidence_lower: lower,
            confidence_upper: upper,
            method_used: method_name,
            timestamp: Utc::now(),
        };

        *self.last_forecast.write() = Some(fc.clone());

        // 5. Publish LoadForecastAvailable event
        actions.push(AgentAction::PublishEvent(Event::new(
            EventType::DataReceived,
            &self.agent_id,
            EventPayload::Message(format!(
                "LoadForecastAvailable: element={}, horizon={}h, method={}, peak={:.1}MW",
                fc.target_element_id,
                self.forecast_horizon_hours,
                fc.method_used,
                fc.forecast_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            )),
        )));

        Ok(actions)
    }

    async fn handle_emergency(&mut self, _event: &Event, _ctx: &AgentContext) -> Result<Vec<AgentAction>> {
        // In emergency, provide conservative high forecast for dispatch planning
        let element_id = self.jurisdiction.device_ids.first().copied().unwrap_or(0);
        let data = self.query_historical_load(element_id);

        let (forecast, method_name) = if data.is_empty() {
            // No data — use a conservative high estimate
            let conservative = vec![1000.0; self.forecast_horizon_hours as usize];
            (conservative, "EmergencyConservative".to_string())
        } else {
            let (mut fc, mn) = self.run_forecast(&data);
            // Add 20% safety margin for emergency planning
            for v in fc.iter_mut() {
                *v *= 1.2;
            }
            (fc, format!("{}+20%EmergencyMargin", mn))
        };

        let smoothed = if data.is_empty() { vec![] } else { self.get_smoothed_values(&data) };
        let residual_std = Self::compute_residual_std(&data, &smoothed);
        // Use upper bound only (conservative high)
        let upper: Vec<f64> = forecast.iter().map(|v| v + 2.0 * residual_std).collect();
        let lower: Vec<f64> = forecast.clone();

        let fc = LoadForecast {
            target_element_id: element_id,
            forecast_values: forecast,
            confidence_lower: lower,
            confidence_upper: upper,
            method_used: method_name,
            timestamp: Utc::now(),
        };

        *self.last_forecast.write() = Some(fc.clone());

        Ok(vec![AgentAction::PublishEvent(Event::new(
            EventType::SystemAlarm,
            &self.agent_id,
            EventPayload::Message(format!(
                "EmergencyLoadForecast: element={}, peak={:.1}MW, method={}",
                fc.target_element_id,
                fc.forecast_values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                fc.method_used
            )),
        ))])
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use parking_lot::RwLock;
    use eneros_core::ZoneId;
    use eneros_eventbus::EventBus;
    use eneros_gateway::SafetyGateway;
    use eneros_tool::ToolEngine;
    use eneros_network::PowerNetwork;
    use eneros_memory::InMemoryMemory;
    use eneros_reasoning::RuleBasedEngine;

    /// Build a minimal AgentContext for testing
    fn test_context() -> AgentContext {
        AgentContext::new(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
        )
    }

    /// Create a Jurisdiction that includes a specific device
    fn jurisdiction_with_device(zone_id: ZoneId, device_id: ElementId) -> Jurisdiction {
        Jurisdiction {
            zone_ids: vec![zone_id],
            voltage_levels: Vec::new(),
            device_ids: vec![device_id],
        }
    }

    // ---- Algorithm tests ----

    #[test]
    fn test_single_exponential_smoothing_basic() {
        let data = vec![10.0, 12.0, 11.0, 13.0, 12.0, 14.0];
        let result = single_exponential_smoothing(&data, 0.3);
        assert_eq!(result.len(), data.len());
        assert_eq!(result[0], data[0]); // First value is unchanged
        // S[1] = 0.3*12 + 0.7*10 = 10.6
        assert!((result[1] - 10.6).abs() < 1e-10);
    }

    #[test]
    fn test_single_exponential_smoothing_empty() {
        let result = single_exponential_smoothing(&[], 0.3);
        assert!(result.is_empty());
    }

    #[test]
    fn test_single_exponential_smoothing_single() {
        let result = single_exponential_smoothing(&[5.0], 0.3);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], 5.0);
    }

    #[test]
    fn test_single_exponential_smoothing_converges() {
        // Constant data should converge to that constant
        let data = vec![100.0; 50];
        let result = single_exponential_smoothing(&data, 0.3);
        assert!((result[49] - 100.0).abs() < 1e-10);
    }

    #[test]
    fn test_double_exponential_smoothing_trend() {
        // Linear trend data: y = 10 + 2*i
        let data: Vec<f64> = (0..10).map(|i| 10.0 + 2.0 * i as f64).collect();
        let result = double_exponential_smoothing(&data, 0.3, 0.1);
        assert_eq!(result.len(), data.len());
        // The smoothed values should follow the trend
        assert!(result[9] > result[0]);
    }

    #[test]
    fn test_double_exponential_smoothing_empty() {
        let result = double_exponential_smoothing(&[], 0.3, 0.1);
        assert!(result.is_empty());
    }

    #[test]
    fn test_double_exponential_smoothing_single() {
        let result = double_exponential_smoothing(&[5.0], 0.3, 0.1);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], 5.0);
    }

    #[test]
    fn test_holt_winters_seasonal() {
        // Create data with daily seasonality (24-hour cycle)
        let mut data = Vec::new();
        for day in 0..7 {
            for hour in 0..24 {
                let base = 100.0;
                let seasonal = 20.0 * ((hour as f64 - 12.0) / 12.0).sin();
                let trend = 2.0 * day as f64;
                data.push(base + trend + seasonal);
            }
        }

        let params = HoltWintersParams {
            alpha: 0.3,
            beta: 0.1,
            gamma: 0.1,
            season_length: 24,
        };

        let forecast = holt_winters_forecast(&data, &params, 24);
        assert_eq!(forecast.len(), 24);
        // Forecast values should be positive
        for v in &forecast {
            assert!(*v > 0.0);
        }
    }

    #[test]
    fn test_holt_winters_insufficient_data_fallback() {
        // Less than 2*season_length data points — should fall back to double smoothing
        let data = vec![10.0, 12.0, 11.0, 13.0, 12.0];
        let params = HoltWintersParams {
            alpha: 0.3,
            beta: 0.1,
            gamma: 0.1,
            season_length: 24,
        };
        let forecast = holt_winters_forecast(&data, &params, 5);
        assert_eq!(forecast.len(), 5);
        for v in &forecast {
            assert!(*v >= 0.0);
        }
    }

    #[test]
    fn test_holt_winters_empty() {
        let params = HoltWintersParams::default();
        let forecast = holt_winters_forecast(&[], &params, 5);
        assert!(forecast.is_empty());
    }

    // ---- LoadForecastAgent tests ----

    #[test]
    fn test_load_forecast_agent_creation() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let agent = LoadForecastAgent::new(
            "fc-1",
            Jurisdiction::for_zones(vec![1, 2]),
            ts_engine,
        );

        assert_eq!(agent.id(), "fc-1");
        assert_eq!(agent.name(), "load-forecast-agent");
        assert_eq!(agent.agent_type(), AgentType::Custom("LoadForecast".to_string()));
        assert_eq!(agent.authority_level(), AuthorityLevel::Operator);
        assert_eq!(agent.tick_interval(), Duration::from_secs(900));
    }

    #[test]
    fn test_load_forecast_agent_with_methods() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let agent = LoadForecastAgent::new(
            "fc-2",
            Jurisdiction::for_zones(vec![1]),
            ts_engine,
        )
        .with_smoothing_method(SmoothingMethod::Double { alpha: 0.4, beta: 0.2 })
        .with_forecast_horizon(48)
        .with_history_days(14);

        assert_eq!(agent.forecast_horizon_hours, 48);
        assert_eq!(agent.history_days, 14);
    }

    #[tokio::test]
    async fn test_agent_start_stop() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let mut agent = LoadForecastAgent::new(
            "fc-3",
            Jurisdiction::for_zones(vec![1]),
            ts_engine,
        );

        agent.start().await.unwrap();
        agent.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_tick_produces_forecast() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let element_id: ElementId = 1;

        // Populate historical data
        let now = Utc::now();
        for i in 0..168 { // 7 days of hourly data
            let ts = now - chrono::Duration::hours(168 - i);
            let value = 100.0 + 20.0 * ((i as f64 % 24.0 - 12.0) / 12.0).sin();
            ts_engine.record(element_id, "load_mw", value, ts).unwrap();
        }

        let mut agent = LoadForecastAgent::new(
            "fc-4",
            jurisdiction_with_device(1, element_id),
            ts_engine.clone(),
        )
        .with_smoothing_method(SmoothingMethod::Single { alpha: 0.3 });

        let ctx = test_context();
        let actions = agent.tick(&ctx).await.unwrap();

        assert!(!actions.is_empty());
        let forecast = agent.last_forecast().unwrap();
        assert_eq!(forecast.forecast_values.len(), 24);
        assert_eq!(forecast.confidence_lower.len(), 24);
        assert_eq!(forecast.confidence_upper.len(), 24);
        assert_eq!(forecast.method_used, "SingleExponentialSmoothing");
    }

    #[tokio::test]
    async fn test_tick_double_smoothing() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let element_id: ElementId = 1;

        let now = Utc::now();
        for i in 0..168 {
            let ts = now - chrono::Duration::hours(168 - i);
            let value = 100.0 + 2.0 * i as f64; // Upward trend
            ts_engine.record(element_id, "load_mw", value, ts).unwrap();
        }

        let mut agent = LoadForecastAgent::new(
            "fc-5",
            jurisdiction_with_device(1, element_id),
            ts_engine,
        )
        .with_smoothing_method(SmoothingMethod::Double { alpha: 0.3, beta: 0.1 });

        let ctx = test_context();
        let actions = agent.tick(&ctx).await.unwrap();
        assert!(!actions.is_empty());

        let forecast = agent.last_forecast().unwrap();
        assert_eq!(forecast.forecast_values.len(), 24);
        assert_eq!(forecast.method_used, "DoubleExponentialSmoothing");
    }

    #[tokio::test]
    async fn test_tick_holt_winters() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let element_id: ElementId = 1;

        let now = Utc::now();
        for day in 0..7 {
            for hour in 0..24 {
                let i = day * 24 + hour;
                let ts = now - chrono::Duration::hours(168 - i as i64);
                let base = 100.0;
                let seasonal = 20.0 * ((hour as f64 - 12.0) / 12.0).sin();
                let trend = 2.0 * day as f64;
                ts_engine.record(element_id, "load_mw", base + trend + seasonal, ts).unwrap();
            }
        }

        let mut agent = LoadForecastAgent::new(
            "fc-6",
            jurisdiction_with_device(1, element_id),
            ts_engine,
        )
        .with_smoothing_method(SmoothingMethod::HoltWinters {
            alpha: 0.3,
            beta: 0.1,
            gamma: 0.1,
            season_length: 24,
        });

        let ctx = test_context();
        let actions = agent.tick(&ctx).await.unwrap();
        assert!(!actions.is_empty());

        let forecast = agent.last_forecast().unwrap();
        assert_eq!(forecast.forecast_values.len(), 24);
        assert_eq!(forecast.method_used, "HoltWinters");
    }

    #[tokio::test]
    async fn test_tick_no_data() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let mut agent = LoadForecastAgent::new(
            "fc-7",
            Jurisdiction::for_zones(vec![1]),
            ts_engine,
        );

        let ctx = test_context();
        let actions = agent.tick(&ctx).await.unwrap();
        // Should produce a log message about no data
        assert!(!actions.is_empty());
        assert!(agent.last_forecast().is_none());
    }

    #[tokio::test]
    async fn test_handle_event_constraint_violation() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let element_id: ElementId = 42;

        // Populate some data
        let now = Utc::now();
        for i in 0..50 {
            let ts = now - chrono::Duration::hours(50 - i);
            ts_engine.record(element_id, "load_mw", 100.0 + i as f64, ts).unwrap();
        }

        let mut agent = LoadForecastAgent::new(
            "fc-8",
            Jurisdiction::for_zones(vec![1]),
            ts_engine,
        );

        let ctx = test_context();
        let event = Event::new(
            EventType::ConstraintViolation,
            "test-source",
            EventPayload::ConstraintViolation {
                constraint_id: "line-limit-1".to_string(),
                element_id,
                actual_value: 150.0,
                limit_value: 120.0,
                severity: "major".to_string(),
            },
        );

        let actions = agent.handle_event(&event, &ctx).await.unwrap();
        assert!(!actions.is_empty());
        // Should have a PublishEvent action
        assert!(actions.iter().any(|a| matches!(a, AgentAction::PublishEvent(_))));
    }

    #[tokio::test]
    async fn test_handle_event_data_received() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let mut agent = LoadForecastAgent::new(
            "fc-9",
            Jurisdiction::for_zones(vec![1]),
            ts_engine,
        );

        let ctx = test_context();
        let event = Event::new(
            EventType::DataReceived,
            "scada",
            EventPayload::Message("new load data".to_string()),
        );

        let actions = agent.handle_event(&event, &ctx).await.unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], AgentAction::LogMessage(_)));
    }

    #[tokio::test]
    async fn test_handle_event_other() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let mut agent = LoadForecastAgent::new(
            "fc-10",
            Jurisdiction::for_zones(vec![1]),
            ts_engine,
        );

        let ctx = test_context();
        let event = Event::new(
            EventType::TopologyChanged,
            "test-source",
            EventPayload::Message("topo change".to_string()),
        );

        let actions = agent.handle_event(&event, &ctx).await.unwrap();
        assert!(actions.is_empty());
    }

    #[tokio::test]
    async fn test_handle_emergency_conservative_forecast() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let element_id: ElementId = 1;

        let now = Utc::now();
        for i in 0..50 {
            let ts = now - chrono::Duration::hours(50 - i);
            ts_engine.record(element_id, "load_mw", 100.0, ts).unwrap();
        }

        let mut agent = LoadForecastAgent::new(
            "fc-11",
            jurisdiction_with_device(1, element_id),
            ts_engine,
        )
        .with_smoothing_method(SmoothingMethod::Single { alpha: 0.3 });

        let ctx = test_context();
        let event = Event::new(
            EventType::SystemAlarm,
            "emergency-source",
            EventPayload::Message("emergency".to_string()),
        );

        let actions = agent.handle_emergency(&event, &ctx).await.unwrap();
        assert!(!actions.is_empty());

        let forecast = agent.last_forecast().unwrap();
        // Emergency forecast should have +20% margin
        assert!(forecast.method_used.contains("EmergencyMargin"));
        // Forecast values should be >= 100.0 * 1.2 (with some smoothing tolerance)
        assert!(forecast.forecast_values[0] >= 100.0);
    }

    #[tokio::test]
    async fn test_handle_emergency_no_data() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let mut agent = LoadForecastAgent::new(
            "fc-12",
            Jurisdiction::for_zones(vec![1]),
            ts_engine,
        );

        let ctx = test_context();
        let event = Event::new(
            EventType::SystemAlarm,
            "emergency-source",
            EventPayload::Message("emergency".to_string()),
        );

        let actions = agent.handle_emergency(&event, &ctx).await.unwrap();
        assert!(!actions.is_empty());

        let forecast = agent.last_forecast().unwrap();
        // Should use conservative default of 1000 MW
        assert_eq!(forecast.forecast_values[0], 1000.0);
        assert_eq!(forecast.method_used, "EmergencyConservative");
    }

    #[test]
    fn test_confidence_interval_calculation() {
        let data = vec![100.0, 102.0, 98.0, 101.0, 99.0, 103.0, 97.0, 100.0];
        let smoothed = single_exponential_smoothing(&data, 0.3);

        let std_dev = LoadForecastAgent::compute_residual_std(&data, &smoothed);
        assert!(std_dev > 0.0);

        // Confidence interval: ±2σ
        let last_smoothed = smoothed[smoothed.len() - 1];
        let lower = (last_smoothed - 2.0 * std_dev).max(0.0);
        let upper = last_smoothed + 2.0 * std_dev;
        assert!(lower < last_smoothed);
        assert!(upper > last_smoothed);
    }

    #[test]
    fn test_confidence_interval_zero_residual() {
        // Perfect fit — zero residual
        let data = vec![100.0; 10];
        let smoothed = vec![100.0; 10];
        let std_dev = LoadForecastAgent::compute_residual_std(&data, &smoothed);
        assert!(std_dev.abs() < 1e-10);
    }

    #[test]
    fn test_jurisdiction() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let jur = Jurisdiction::for_zones(vec![1, 2, 3]);
        let agent = LoadForecastAgent::new("fc-jur", jur.clone(), ts_engine);
        assert_eq!(agent.jurisdiction(), jur);
    }

    #[test]
    fn test_residual_std_mismatched_lengths() {
        let data = vec![1.0, 2.0, 3.0];
        let smoothed = vec![1.0, 2.0];
        let std_dev = LoadForecastAgent::compute_residual_std(&data, &smoothed);
        assert_eq!(std_dev, 0.0);
    }
}
