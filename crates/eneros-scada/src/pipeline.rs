use std::sync::Arc;

use eneros_core::Result;
use eneros_eventbus::event::{EventPayload, EventType};
use eneros_eventbus::{Event, EventBus};
use eneros_timeseries::TimeSeriesEngine;
use tokio::task::JoinHandle;

use crate::collector::ScadaCollector;

/// Data pipeline: collects data, writes to time-series, and optionally emits events
pub struct DataPipeline {
    collector: Arc<ScadaCollector>,
    ts_engine: Arc<TimeSeriesEngine>,
    event_bus: Option<Arc<EventBus>>,
}

impl DataPipeline {
    /// Create a new data pipeline
    pub fn new(collector: Arc<ScadaCollector>, ts_engine: Arc<TimeSeriesEngine>) -> Self {
        Self {
            collector,
            ts_engine,
            event_bus: None,
        }
    }

    /// Set the event bus for emitting data received events
    pub fn with_event_bus(mut self, bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Run a single collect + write cycle.
    /// Returns the number of points written.
    pub fn run_once(&self) -> Result<usize> {
        let readings = self.collector.collect_once();
        let mut count = 0usize;

        for reading in &readings {
            self.ts_engine.record(
                reading.element_id,
                &reading.parameter,
                reading.value,
                reading.timestamp,
            )?;

            count += 1;

            if let Some(ref bus) = self.event_bus {
                let event = Event::new(
                    EventType::DataReceived,
                    "eneros-scada",
                    EventPayload::Message(format!(
                        "element_id={}, parameter={}, value={:.4}, quality={:?}",
                        reading.element_id, reading.parameter, reading.value, reading.quality
                    )),
                );
                let _ = bus.publish(event);
            }
        }

        Ok(count)
    }

    /// Start a background task that runs the collect+write loop at the given interval.
    /// Returns a JoinHandle that can be used to abort the task.
    pub fn start(&self, interval_ms: u64) -> JoinHandle<()> {
        let collector = self.collector.clone();
        let ts_engine = self.ts_engine.clone();
        let event_bus = self.event_bus.clone();

        tokio::spawn(async move {
            loop {
                let readings = collector.collect_once();

                for reading in &readings {
                    if let Err(e) = ts_engine.record(
                        reading.element_id,
                        &reading.parameter,
                        reading.value,
                        reading.timestamp,
                    ) {
                        tracing::error!(
                            "Failed to record time-series data for element_id={}, parameter={}: {}",
                            reading.element_id,
                            reading.parameter,
                            e
                        );
                        continue;
                    }

                    if let Some(ref bus) = event_bus {
                        let event = Event::new(
                            EventType::DataReceived,
                            "eneros-scada",
                            EventPayload::Message(format!(
                                "element_id={}, parameter={}, value={:.4}, quality={:?}",
                                reading.element_id, reading.parameter, reading.value, reading.quality
                            )),
                        );
                        let _ = bus.publish(event);
                    }
                }

                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::MockDataSource;
    use crate::config::{ScadaConfig, ScadaPoint};

    fn make_pipeline(points: Vec<ScadaPoint>) -> DataPipeline {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);
        mock.insert(2, "active_power_mw", 50.0);

        let config = ScadaConfig {
            points,
            default_scan_rate_ms: 1000,
            timeout_ms: 5000,
            enable_quality_check: true,
        };

        let collector = Arc::new(ScadaCollector::new(config, mock));
        let ts_engine = Arc::new(TimeSeriesEngine::new(1000));

        DataPipeline::new(collector, ts_engine)
    }

    #[test]
    fn test_run_once() {
        let pipeline = make_pipeline(vec![
            ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            },
            ScadaPoint {
                element_id: 2,
                parameter: "active_power_mw".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.1,
                min_value: None,
                max_value: None,
            },
        ]);

        let count = pipeline.run_once().unwrap();
        assert_eq!(count, 2);

        // Verify data was written to time-series engine
        let dp = pipeline.ts_engine.latest(1, "voltage_pu").unwrap();
        assert!((dp.value - 1.02).abs() < f64::EPSILON);

        let dp = pipeline.ts_engine.latest(2, "active_power_mw").unwrap();
        assert!((dp.value - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_run_once_with_event_bus() {
        let pipeline = make_pipeline(vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: None,
            max_value: None,
        }]);

        let event_bus = Arc::new(EventBus::new(100));
        let mut rx = event_bus.subscribe();

        let pipeline = pipeline.with_event_bus(event_bus);

        let count = pipeline.run_once().unwrap();
        assert_eq!(count, 1);

        // Verify event was published
        let event = rx.try_recv().unwrap();
        assert_eq!(event.event_type, EventType::DataReceived);
        assert_eq!(event.source, "eneros-scada");
    }

    #[test]
    fn test_run_once_empty_config() {
        let pipeline = make_pipeline(vec![]);
        let count = pipeline.run_once().unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_start_background_task() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);

        let config = ScadaConfig {
            points: vec![ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            }],
            default_scan_rate_ms: 1000,
            timeout_ms: 5000,
            enable_quality_check: true,
        };

        let collector = Arc::new(ScadaCollector::new(config, mock));
        let ts_engine = Arc::new(TimeSeriesEngine::new(1000));

        let pipeline = DataPipeline::new(collector.clone(), ts_engine.clone());
        let handle = pipeline.start(100);

        // Wait for a few cycles
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;

        handle.abort();

        // Verify data was written
        let dp = ts_engine.latest(1, "voltage_pu").unwrap();
        assert!((dp.value - 1.02).abs() < f64::EPSILON);
    }
}
