use eneros_core::ElementId;
use super::engine::DataPoint;

/// Time-series storage abstraction
pub trait TimeSeriesStorage: Send + Sync {
    /// Store a data point
    fn store(
        &self,
        element_id: ElementId,
        parameter: &str,
        point: DataPoint,
    ) -> Result<(), String>;

    /// Retrieve data points
    fn retrieve(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<DataPoint>, String>;

    /// Get latest data point
    fn latest(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Result<Option<DataPoint>, String>;

    /// Delete old data
    fn cleanup(&self, before: i64) -> Result<usize, String>;
}

/// In-memory storage implementation
pub struct InMemoryStorage {
    data: std::sync::RwLock<std::collections::HashMap<(ElementId, String), Vec<DataPoint>>>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self {
            data: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeSeriesStorage for InMemoryStorage {
    fn store(
        &self,
        element_id: ElementId,
        parameter: &str,
        point: DataPoint,
    ) -> Result<(), String> {
        let mut data = self.data.write().map_err(|e| e.to_string())?;
        let key = (element_id, parameter.to_string());
        data.entry(key).or_default().push(point);
        Ok(())
    }

    fn retrieve(
        &self,
        element_id: ElementId,
        parameter: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<DataPoint>, String> {
        let data = self.data.read().map_err(|e| e.to_string())?;
        let key = (element_id, parameter.to_string());

        Ok(data
            .get(&key)
            .map(|points| {
                points
                    .iter()
                    .filter(|p| {
                        let ts = p.timestamp.timestamp_millis();
                        ts >= start && ts <= end
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    fn latest(
        &self,
        element_id: ElementId,
        parameter: &str,
    ) -> Result<Option<DataPoint>, String> {
        let data = self.data.read().map_err(|e| e.to_string())?;
        let key = (element_id, parameter.to_string());

        Ok(data.get(&key).and_then(|points| points.last().cloned()))
    }

    fn cleanup(&self, before: i64) -> Result<usize, String> {
        let mut data = self.data.write().map_err(|e| e.to_string())?;
        let mut removed = 0;

        for points in data.values_mut() {
            let original_len = points.len();
            points.retain(|p| p.timestamp.timestamp_millis() >= before);
            removed += original_len - points.len();
        }

        Ok(removed)
    }
}
