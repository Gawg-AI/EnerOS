use chrono::{DateTime, Utc};
use eneros_core::ElementId;

/// Time-series query builder
pub struct TimeSeriesQuery {
    element_id: ElementId,
    parameter: String,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    aggregation: Option<Aggregation>,
    limit: Option<usize>,
}

/// Aggregation function
#[derive(Debug, Clone)]
pub enum Aggregation {
    Average,
    Min,
    Max,
    Sum,
    Count,
    First,
    Last,
}

impl TimeSeriesQuery {
    /// Create a new query
    pub fn new(element_id: ElementId, parameter: &str) -> Self {
        Self {
            element_id,
            parameter: parameter.to_string(),
            start: None,
            end: None,
            aggregation: None,
            limit: None,
        }
    }

    /// Set time range start
    pub fn start(mut self, start: DateTime<Utc>) -> Self {
        self.start = Some(start);
        self
    }

    /// Set time range end
    pub fn end(mut self, end: DateTime<Utc>) -> Self {
        self.end = Some(end);
        self
    }

    /// Set aggregation function
    pub fn aggregate(mut self, aggregation: Aggregation) -> Self {
        self.aggregation = Some(aggregation);
        self
    }

    /// Set result limit
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Get element ID
    pub fn element_id(&self) -> ElementId {
        self.element_id
    }

    /// Get parameter name
    pub fn parameter(&self) -> &str {
        &self.parameter
    }

    /// Get start time
    pub fn start_time(&self) -> Option<DateTime<Utc>> {
        self.start
    }

    /// Get end time
    pub fn end_time(&self) -> Option<DateTime<Utc>> {
        self.end
    }

    /// Get aggregation
    pub fn aggregation(&self) -> Option<&Aggregation> {
        self.aggregation.as_ref()
    }

    /// Get limit
    pub fn limit_value(&self) -> Option<usize> {
        self.limit
    }
}
