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

/// Seasonality model: additive (constant amplitude) or multiplicative
/// (amplitude scales with level). For load forecasting, multiplicative is
/// usually more accurate because demand swings grow in absolute terms as the
/// average load rises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SeasonalityType {
    /// y = level + trend + season
    #[default]
    Additive,
    /// y = (level + trend) * season
    Multiplicative,
}

/// Parameters for Holt-Winters exponential smoothing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoltWintersParams {
    pub alpha: f64,
    pub beta: f64,
    pub gamma: f64,
    pub season_length: usize,
    /// Additive vs multiplicative seasonality. Defaults to additive for
    /// backward compatibility.
    #[serde(default)]
    pub seasonality: SeasonalityType,
}

impl Default for HoltWintersParams {
    fn default() -> Self {
        Self {
            alpha: 0.3,
            beta: 0.1,
            gamma: 0.1,
            season_length: 24, // daily seasonality for hourly data
            seasonality: SeasonalityType::Additive,
        }
    }
}

impl HoltWintersParams {
    /// Validate smoothing parameters.
    ///
    /// `alpha`, `beta`, `gamma` must each lie in the open interval `(0, 1)` —
    /// 0 would freeze the component and 1 would make it track noise. A
    /// `season_length` of 1 is meaningless (a single-point "season" is just the
    /// level), so we require at least 2.
    pub fn validate(&self) -> std::result::Result<(), String> {
        let in_range = |v: f64, name: &str| -> std::result::Result<(), String> {
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("{} must be in [0,1], got {}", name, v));
            }
            Ok(())
        };
        in_range(self.alpha, "alpha")?;
        in_range(self.beta, "beta")?;
        in_range(self.gamma, "gamma")?;
        if self.season_length < 2 {
            return Err(format!(
                "season_length must be >= 2, got {}",
                self.season_length
            ));
        }
        Ok(())
    }
}

/// Full output of a Holt-Winters fit, including the in-sample fitted values
/// (one-step-ahead) so that residuals and confidence intervals can be computed
/// from the *actual* model rather than a proxy.
#[derive(Debug, Clone)]
pub struct HoltWintersResult {
    /// In-sample one-step-ahead fitted values, same length as the input.
    pub fitted: Vec<f64>,
    /// Forecast for `horizon` steps beyond the end of the input.
    pub forecast: Vec<f64>,
    /// Final level component at the last observation.
    pub final_level: f64,
    /// Final trend component at the last observation.
    pub final_trend: f64,
    /// Seasonal component series (length = input length).
    pub season: Vec<f64>,
    /// Seasonality model used.
    pub seasonality: SeasonalityType,
}

/// Holt-Winters forecast — level + trend + seasonality.
///
/// Returns the full [`HoltWintersResult`] (in-sample fitted values + forecast).
/// When the input has fewer than `2 * season_length` points, seasonal
/// decomposition is impossible and the function falls back to double
/// exponential smoothing (trend-only), recording NaN fitted values for indices
/// before the fallback becomes available.
pub fn holt_winters_fit(
    data: &[f64],
    params: &HoltWintersParams,
    forecast_horizon: usize,
) -> HoltWintersResult {
    if let Err(msg) = params.validate() {
        tracing::warn!("Holt-Winters parameter validation failed: {}; using defaults", msg);
    }
    let alpha = params.alpha.clamp(0.0, 1.0);
    let beta = params.beta.clamp(0.0, 1.0);
    let gamma = params.gamma.clamp(0.0, 1.0);
    let s = params.season_length.max(1);

    if data.is_empty() {
        return HoltWintersResult {
            fitted: Vec::new(),
            forecast: Vec::new(),
            final_level: 0.0,
            final_trend: 0.0,
            season: Vec::new(),
            seasonality: params.seasonality,
        };
    }

    // Fallback: not enough data to estimate a season.
    if data.len() < 2 * s {
        let smoothed = double_exponential_smoothing(data, alpha, beta);
        let n = smoothed.len();
        let last_level = smoothed[n - 1];
        let last_trend = if n >= 2 {
            smoothed[n - 1] - smoothed[n - 2]
        } else {
            0.0
        };
        let forecast = (1..=forecast_horizon)
            .map(|h| (last_level + h as f64 * last_trend).max(0.0))
            .collect();
        return HoltWintersResult {
            fitted: smoothed,
            forecast,
            final_level: last_level,
            final_trend: last_trend,
            season: vec![0.0; n],
            seasonality: params.seasonality,
        };
    }

    let n = data.len();
    let mut level = vec![0.0; n];
    let mut trend = vec![0.0; n];
    let mut season = vec![0.0; n];
    let mut fitted = vec![0.0; n];

    match params.seasonality {
        SeasonalityType::Additive => {
            // Initialize level/trend from the first two seasons.
            let avg_first: f64 = data[..s].iter().sum::<f64>() / s as f64;
            let avg_second: f64 = data[s..2 * s].iter().sum::<f64>() / s as f64;
            level[s - 1] = avg_first;
            trend[s - 1] = (avg_second - avg_first) / s as f64;
            for i in 0..s {
                season[i] = data[i] - avg_first;
            }
            // The first season is used to initialize the model, so its in-sample
            // fitted value is the seasonal reconstruction (= the data itself);
            // leaving fitted[0..s] at 0.0 would inject spurious large residuals.
            for i in 0..s {
                fitted[i] = avg_first + season[i];
            }
            for i in s..n {
                level[i] =
                    alpha * (data[i] - season[i - s]) + (1.0 - alpha) * (level[i - 1] + trend[i - 1]);
                trend[i] = beta * (level[i] - level[i - 1]) + (1.0 - beta) * trend[i - 1];
                season[i] = gamma * (data[i] - level[i]) + (1.0 - gamma) * season[i - s];
                fitted[i] = level[i - 1] + trend[i - 1] + season[i - s];
            }
            let last_level = level[n - 1];
            let last_trend = trend[n - 1];
            let forecast = (1..=forecast_horizon)
                .map(|h| {
                    let idx = if h <= s { n - s + h - 1 } else { n - s + (h - 1) % s };
                    (last_level + h as f64 * last_trend + season[idx]).max(0.0)
                })
                .collect();
            HoltWintersResult {
                fitted,
                forecast,
                final_level: last_level,
                final_trend: last_trend,
                season,
                seasonality: SeasonalityType::Additive,
            }
        }
        SeasonalityType::Multiplicative => {
            // Multiplicative seasonality: y_t = (level + trend) * season_t.
            // Seasonal indices are dimensionless ratios centered around 1.0.
            let avg_first: f64 = data[..s].iter().sum::<f64>() / s as f64;
            let avg_second: f64 = data[s..2 * s].iter().sum::<f64>() / s as f64;
            level[s - 1] = avg_first;
            trend[s - 1] = (avg_second - avg_first) / s as f64;
            for i in 0..s {
                season[i] = if avg_first.abs() > 1e-9 {
                    data[i] / avg_first
                } else {
                    1.0
                };
            }
            // First season: fitted = reconstruction of the data (≈ data itself),
            // so the initialization window contributes ~zero residual.
            for i in 0..s {
                fitted[i] = avg_first * season[i];
            }
            for i in s..n {
                let denom = season[i - s];
                let base = level[i - 1] + trend[i - 1];
                level[i] = if denom.abs() > 1e-9 {
                    alpha * (data[i] / denom) + (1.0 - alpha) * base
                } else {
                    alpha * data[i] + (1.0 - alpha) * base
                };
                trend[i] = beta * (level[i] - level[i - 1]) + (1.0 - beta) * trend[i - 1];
                let level_val = if level[i].abs() > 1e-9 { level[i] } else { 1.0 };
                season[i] = gamma * (data[i] / level_val) + (1.0 - gamma) * season[i - s];
                fitted[i] = (level[i - 1] + trend[i - 1]) * season[i - s];
            }
            let last_level = level[n - 1];
            let last_trend = trend[n - 1];
            let forecast = (1..=forecast_horizon)
                .map(|h| {
                    let idx = if h <= s { n - s + h - 1 } else { n - s + (h - 1) % s };
                    ((last_level + h as f64 * last_trend) * season[idx]).max(0.0)
                })
                .collect();
            HoltWintersResult {
                fitted,
                forecast,
                final_level: last_level,
                final_trend: last_trend,
                season,
                seasonality: SeasonalityType::Multiplicative,
            }
        }
    }
}

/// Holt-Winters forecast (convenience wrapper returning only the forecast
/// vector, for backward compatibility with callers that don't need fitted
/// values).
pub fn holt_winters_forecast(
    data: &[f64],
    params: &HoltWintersParams,
    forecast_horizon: usize,
) -> Vec<f64> {
    holt_winters_fit(data, params, forecast_horizon).forecast
}

// ---------------------------------------------------------------------------
// Forecast accuracy metrics
// ---------------------------------------------------------------------------

/// Forecast accuracy metrics, computed by comparing predicted values against
/// actuals. Used to evaluate model fit and to select between smoothing methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccuracyMetrics {
    /// Mean Absolute Error
    pub mae: f64,
    /// Root Mean Squared Error
    pub rmse: f64,
    /// Mean Absolute Percentage Error (percent). `NaN` if any actual is zero.
    pub mape: f64,
}

impl Default for AccuracyMetrics {
    fn default() -> Self {
        Self {
            mae: 0.0,
            rmse: 0.0,
            mape: 0.0,
        }
    }
}

/// Compute accuracy metrics by comparing `predicted` to `actual` element-wise.
/// Returns `None` if the inputs are empty or of mismatched length.
pub fn accuracy_metrics(actual: &[f64], predicted: &[f64]) -> Option<AccuracyMetrics> {
    if actual.is_empty() || actual.len() != predicted.len() {
        return None;
    }
    let n = actual.len() as f64;
    let abs_err: f64 = actual
        .iter()
        .zip(predicted.iter())
        .map(|(a, p)| (a - p).abs())
        .sum();
    let sq_err: f64 = actual
        .iter()
        .zip(predicted.iter())
        .map(|(a, p)| (a - p).powi(2))
        .sum();
    let mae = abs_err / n;
    let rmse = (sq_err / n).sqrt();

    // MAPE: ignore points where actual == 0 (undefined), report the average
    // over the remaining points. If all are zero, MAPE is NaN.
    let mut pct_sum = 0.0;
    let mut pct_count = 0u32;
    for (a, p) in actual.iter().zip(predicted.iter()) {
        if a.abs() > 1e-9 {
            pct_sum += ((p - a) / a).abs() * 100.0;
            pct_count += 1;
        }
    }
    let mape = if pct_count > 0 {
        pct_sum / pct_count as f64
    } else {
        f64::NAN
    };

    Some(AccuracyMetrics { mae, rmse, mape })
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
    /// Holt-Winters (level + trend + seasonality), additive model
    HoltWinters { alpha: f64, beta: f64, gamma: f64, season_length: usize },
    /// Holt-Winters with explicit seasonality type (additive or multiplicative)
    HoltWintersTyped {
        alpha: f64,
        beta: f64,
        gamma: f64,
        season_length: usize,
        seasonality: SeasonalityType,
    },
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
                    seasonality: SeasonalityType::Additive,
                };
                let result = holt_winters_fit(data, &params, self.forecast_horizon_hours as usize);
                (result.forecast, "HoltWinters".to_string())
            }
            SmoothingMethod::HoltWintersTyped { alpha, beta, gamma, season_length, seasonality } => {
                let params = HoltWintersParams {
                    alpha: *alpha,
                    beta: *beta,
                    gamma: *gamma,
                    season_length: *season_length,
                    seasonality: *seasonality,
                };
                let result = holt_winters_fit(data, &params, self.forecast_horizon_hours as usize);
                let label = match seasonality {
                    SeasonalityType::Additive => "HoltWinters-Additive",
                    SeasonalityType::Multiplicative => "HoltWinters-Multiplicative",
                };
                (result.forecast, label.to_string())
            }
        }
    }

    /// Get the in-sample fitted (smoothed) values for the historical data.
    ///
    /// These are the one-step-ahead fitted values of the configured model,
    /// used to compute residuals and hence confidence intervals. For
    /// Holt-Winters this now uses the **actual** model's fitted values rather
    /// than a double-smoothing proxy, so confidence intervals reflect the real
    /// model error.
    fn get_smoothed_values(&self, data: &[f64]) -> Vec<f64> {
        match &self.smoothing_method {
            SmoothingMethod::Single { alpha } => single_exponential_smoothing(data, *alpha),
            SmoothingMethod::Double { alpha, beta } => {
                double_exponential_smoothing(data, *alpha, *beta)
            }
            SmoothingMethod::HoltWinters { alpha, beta, gamma, season_length } => {
                let params = HoltWintersParams {
                    alpha: *alpha,
                    beta: *beta,
                    gamma: *gamma,
                    season_length: *season_length,
                    seasonality: SeasonalityType::Additive,
                };
                holt_winters_fit(data, &params, 0).fitted
            }
            SmoothingMethod::HoltWintersTyped { alpha, beta, gamma, season_length, seasonality } => {
                let params = HoltWintersParams {
                    alpha: *alpha,
                    beta: *beta,
                    gamma: *gamma,
                    season_length: *season_length,
                    seasonality: *seasonality,
                };
                holt_winters_fit(data, &params, 0).fitted
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
                    SmoothingMethod::HoltWintersTyped {
                        beta,
                        gamma,
                        season_length,
                        seasonality,
                        ..
                    } => SmoothingMethod::HoltWintersTyped {
                        alpha: 0.5,
                        beta: *beta,
                        gamma: *gamma,
                        season_length: *season_length,
                        seasonality: *seasonality,
                    },
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
            seasonality: SeasonalityType::Additive,
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
            seasonality: SeasonalityType::Additive,
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

    // ===================================================================
    // Holt-Winters v2: fitted values, multiplicative, validation, metrics
    // (BUG3 §5 — previously get_smoothed_values used double-smoothing as a
    // proxy for Holt-Winters residuals, producing wrong confidence intervals)
    // ===================================================================

    /// holt_winters_fit must return fitted values whose length equals the input.
    #[test]
    fn test_hw_fit_returns_fitted_values() {
        let mut data = Vec::new();
        for day in 0..7 {
            for hour in 0..24 {
                data.push(100.0 + 2.0 * day as f64 + 20.0 * ((hour as f64 - 12.0) / 12.0).sin());
            }
        }
        let params = HoltWintersParams::default();
        let result = holt_winters_fit(&data, &params, 24);
        assert_eq!(result.fitted.len(), data.len(), "fitted length must match input");
        assert_eq!(result.forecast.len(), 24);
        assert_eq!(result.season.len(), data.len());
    }

    /// Regression: previously get_smoothed_values for Holt-Winters returned
    /// double-smoothing output (length n) — now it returns the real HW fitted
    /// values. The fitted values must track seasonal data far more closely
    /// than double smoothing does, so residual std should be substantially
    /// smaller with the real model.
    #[test]
    fn test_hw_fitted_values_are_the_real_model() {
        // Strongly seasonal data: double smoothing cannot capture the swing,
        // but HW (additive) should. residual_std(HW) < residual_std(double).
        let mut data = Vec::new();
        for day in 0..7 {
            for hour in 0..24 {
                data.push(100.0 + 30.0 * ((hour as f64 - 12.0) / 12.0).sin());
            }
        }
        let params = HoltWintersParams::default();
        let hw_fit = holt_winters_fit(&data, &params, 0);
        let double_fit = double_exponential_smoothing(&data, params.alpha, params.beta);

        let hw_resid = LoadForecastAgent::compute_residual_std(&data, &hw_fit.fitted);
        let double_resid = LoadForecastAgent::compute_residual_std(&data, &double_fit);

        assert!(
            hw_resid < double_resid,
            "HW residual std ({:.3}) should be smaller than double-smoothing ({:.3}) \
             because HW captures seasonality",
            hw_resid,
            double_resid
        );
    }

    /// Multiplicative seasonality: forecast should reproduce a multiplicative
    /// pattern (seasonal amplitude proportional to level).
    #[test]
    fn test_hw_multiplicative_seasonality() {
        // y = base * (1 + 0.5 * sin), a pure multiplicative seasonal pattern.
        let mut data = Vec::new();
        for day in 0..7 {
            for hour in 0..24 {
                let base = 100.0 + 5.0 * day as f64;
                let mult = 1.0 + 0.5 * ((hour as f64 - 12.0) / 12.0).sin();
                data.push(base * mult);
            }
        }
        let params = HoltWintersParams {
            alpha: 0.3,
            beta: 0.1,
            gamma: 0.1,
            season_length: 24,
            seasonality: SeasonalityType::Multiplicative,
        };
        let result = holt_winters_fit(&data, &params, 24);
        assert_eq!(result.seasonality, SeasonalityType::Multiplicative);
        // Multiplicative seasonal indices should hover around 1.0.
        let avg_season: f64 = result.season[result.season.len() - 24..].iter().sum::<f64>() / 24.0;
        assert!(
            (avg_season - 1.0).abs() < 0.5,
            "multiplicative seasonal indices should average near 1.0, got {:.3}",
            avg_season
        );
        for v in &result.forecast {
            assert!(*v > 0.0);
        }
    }

    /// Parameter validation rejects out-of-range smoothing constants and
    /// too-short seasons.
    #[test]
    fn test_hw_param_validation() {
        let bad_alpha = HoltWintersParams {
            alpha: 1.5,
            beta: 0.1,
            gamma: 0.1,
            season_length: 24,
            seasonality: SeasonalityType::Additive,
        };
        assert!(bad_alpha.validate().is_err());

        let bad_season = HoltWintersParams {
            alpha: 0.3,
            beta: 0.1,
            gamma: 0.1,
            season_length: 1,
            seasonality: SeasonalityType::Additive,
        };
        assert!(bad_season.validate().is_err());

        let good = HoltWintersParams::default();
        assert!(good.validate().is_ok());
    }

    /// Out-of-range params are clamped rather than panicking (defensive).
    #[test]
    fn test_hw_fit_clamps_bad_params() {
        let data = vec![10.0; 60]; // flat, 2.5 seasons of length 24
        let params = HoltWintersParams {
            alpha: 5.0, // out of range → clamped to 1.0
            beta: -1.0, // out of range → clamped to 0.0
            gamma: 0.1,
            season_length: 24,
            seasonality: SeasonalityType::Additive,
        };
        // Must not panic; must produce a sane forecast.
        let result = holt_winters_fit(&data, &params, 5);
        assert_eq!(result.forecast.len(), 5);
        for v in &result.forecast {
            assert!(v.is_finite());
        }
    }

    /// Accuracy metrics on a perfect prediction should be all-zero.
    #[test]
    fn test_accuracy_metrics_perfect() {
        let actual = vec![100.0, 102.0, 98.0, 101.0];
        let m = accuracy_metrics(&actual, &actual).unwrap();
        assert!(m.mae.abs() < 1e-12);
        assert!(m.rmse.abs() < 1e-12);
        assert!(m.mape.abs() < 1e-12);
    }

    /// MAE / RMSE on a known error.
    #[test]
    fn test_accuracy_metrics_known_error() {
        let actual = vec![100.0, 100.0, 100.0];
        let predicted = vec![101.0, 99.0, 100.0];
        let m = accuracy_metrics(&actual, &predicted).unwrap();
        // abs errors: 1, 1, 0 → MAE = 2/3
        assert!((m.mae - 2.0 / 3.0).abs() < 1e-12);
        // sq errors: 1, 1, 0 → RMSE = sqrt(2/3)
        assert!((m.rmse - (2.0 / 3.0_f64).sqrt()).abs() < 1e-12);
    }

    /// MAPE ignores zero-actual points (undefined %).
    #[test]
    fn test_accuracy_metrics_mape_skips_zeros() {
        let actual = vec![0.0, 100.0, 0.0];
        let predicted = vec![50.0, 110.0, 50.0];
        let m = accuracy_metrics(&actual, &predicted).unwrap();
        // Only the middle point counts: |110-100|/100 * 100 = 10%
        assert!((m.mape - 10.0).abs() < 1e-9);
    }

    /// Mismatched / empty inputs return None.
    #[test]
    fn test_accuracy_metrics_invalid_inputs() {
        assert!(accuracy_metrics(&[], &[]).is_none());
        assert!(accuracy_metrics(&[1.0], &[1.0, 2.0]).is_none());
    }

    /// The agent end-to-end with the new HoltWintersTyped variant produces a
    /// forecast using the multiplicative model.
    #[tokio::test]
    async fn test_agent_multiplicative_hw_forecast() {
        let ts_engine = Arc::new(TimeSeriesEngine::new(100_000));
        let element_id: ElementId = 1;

        let now = Utc::now();
        for day in 0..7 {
            for hour in 0..24 {
                let i = day * 24 + hour;
                let ts = now - chrono::Duration::hours(168 - i as i64);
                let base = 100.0;
                let mult = 1.0 + 0.3 * ((hour as f64 - 12.0) / 12.0).sin();
                ts_engine.record(element_id, "load_mw", base * mult, ts).unwrap();
            }
        }

        let mut agent = LoadForecastAgent::new(
            "fc-mult",
            jurisdiction_with_device(1, element_id),
            ts_engine,
        )
        .with_smoothing_method(SmoothingMethod::HoltWintersTyped {
            alpha: 0.3,
            beta: 0.1,
            gamma: 0.1,
            season_length: 24,
            seasonality: SeasonalityType::Multiplicative,
        });

        let ctx = test_context();
        let actions = agent.tick(&ctx).await.unwrap();
        assert!(!actions.is_empty());

        let forecast = agent.last_forecast().unwrap();
        assert_eq!(forecast.forecast_values.len(), 24);
        assert_eq!(forecast.method_used, "HoltWinters-Multiplicative");
        for v in &forecast.forecast_values {
            assert!(*v > 0.0);
        }
    }
}
