use chrono::{DateTime, Utc};
use eneros_core::{ElementId, SeverityLevel};

use super::rules::ResponseStrategy;

/// Constraint violation record
#[derive(Debug, Clone)]
pub struct Violation {
    /// Constraint ID that was violated
    pub constraint_id: String,
    /// Element ID that violated the constraint
    pub element_id: ElementId,
    /// Actual measured value
    pub actual_value: f64,
    /// Minimum limit
    pub limit_min: f64,
    /// Maximum limit
    pub limit_max: f64,
    /// Severity level
    pub severity: SeverityLevel,
    /// Response strategy
    pub response_strategy: ResponseStrategy,
    /// Timestamp of violation
    pub timestamp: DateTime<Utc>,
}

impl Violation {
    /// Calculate violation percentage
    pub fn violation_percent(&self) -> f64 {
        if self.actual_value < self.limit_min {
            ((self.limit_min - self.actual_value) / self.limit_min) * 100.0
        } else if self.actual_value > self.limit_max {
            ((self.actual_value - self.limit_max) / self.limit_max) * 100.0
        } else {
            0.0
        }
    }

    /// Check if violation is above minimum limit
    pub fn below_minimum(&self) -> bool {
        self.actual_value < self.limit_min
    }

    /// Check if violation is above maximum limit
    pub fn above_maximum(&self) -> bool {
        self.actual_value > self.limit_max
    }
}
