use std::collections::HashMap;
use std::sync::Arc;

use eneros_core::Result;
use eneros_eventbus::event::{EventPayload, EventType};
use eneros_eventbus::{Event, EventBus};
use eneros_timeseries::{SoeEventType, SoeRecorder, TimeSeriesEngine};
use parking_lot::RwLock;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::collector::ScadaCollector;

/// Data pipeline: collects data, writes to time-series, and optionally emits events
pub struct DataPipeline {
    collector: Arc<ScadaCollector>,
    ts_engine: Arc<TimeSeriesEngine>,
    event_bus: Option<Arc<EventBus>>,
    /// Optional SOE recorder for breaker/switch state-change detection
    /// (v0.10.0 — Task 4).
    soe_recorder: Option<Arc<SoeRecorder>>,
    /// Tracks the last seen boolean state of (element_id, parameter) pairs
    /// that look like breaker/switch/position/relay points. Used to detect
    /// 0↔1 transitions and emit SOE records.
    last_bool_states: RwLock<HashMap<(eneros_core::ElementId, String), bool>>,
}

impl DataPipeline {
    /// Create a new data pipeline
    pub fn new(collector: Arc<ScadaCollector>, ts_engine: Arc<TimeSeriesEngine>) -> Self {
        Self {
            collector,
            ts_engine,
            event_bus: None,
            soe_recorder: None,
            last_bool_states: RwLock::new(HashMap::new()),
        }
    }

    /// Set the event bus for emitting data received events
    pub fn with_event_bus(mut self, bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Wire an SOE recorder for breaker/switch state-change detection
    /// (v0.10.0 — Task 4).
    pub fn with_soe_recorder(mut self, recorder: Arc<SoeRecorder>) -> Self {
        self.soe_recorder = Some(recorder);
        self
    }

    /// Detect and record SOE events for boolean-looking readings whose
    /// parameter name matches breaker/switch/position/relay. Emits a
    /// `BreakerClose` event on 0→1 and `BreakerOpen` on 1→0.
    fn detect_soe_events(&self, readings: &[crate::collector::ScadaReading]) {
        let Some(ref recorder) = self.soe_recorder else {
            return;
        };

        let keywords = ["breaker", "switch", "position", "relay"];
        for reading in readings {
            let param_lower = reading.parameter.to_lowercase();
            if !keywords.iter().any(|k| param_lower.contains(k)) {
                continue;
            }
            // Only treat exact 0.0 / 1.0 as boolean states.
            let new_state = if reading.value == 1.0 {
                true
            } else if reading.value == 0.0 {
                false
            } else {
                continue;
            };

            let key = (reading.element_id, reading.parameter.clone());
            let mut states = self.last_bool_states.write();
            let prev = states.insert(key.clone(), new_state);

            // Only emit an SOE record on a real transition (prev differs).
            if let Some(old) = prev {
                if old != new_state {
                    let event_type = if new_state {
                        SoeEventType::BreakerClose
                    } else {
                        SoeEventType::BreakerOpen
                    };
                    let device_id = format!("element_{}", reading.element_id);
                    let value = format!("{} -> {}", old as u8, new_state as u8);
                    if let Err(e) = recorder.record_now(
                        &device_id,
                        event_type,
                        1,
                        &value,
                    ) {
                        tracing::warn!(
                            "SOE record failed for element_{} {}: {}",
                            reading.element_id,
                            reading.parameter,
                            e
                        );
                    }
                }
            }
        }
    }

    /// Run a single refresh + collect + write cycle.
    /// Returns the number of points written.
    ///
    /// This is the synchronous counterpart of `start()`'s loop body, useful
    /// for tests and one-shot collection. It refreshes the upstream data
    /// source before collecting.
    pub async fn run_once(&self) -> Result<usize> {
        // Refresh upstream cache before collecting (mirrors `start()` loop).
        self.collector.refresh_data_source().await;

        let readings = self.collector.collect_once();
        let mut count = 0usize;

        // Detect breaker/switch state changes and emit SOE records before
        // persisting time-series data (v0.10.0 — Task 4).
        self.detect_soe_events(&readings);

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
    ///
    /// Each cycle:
    /// 1. `data_source.refresh()` — pull fresh data from upstream (IEC 104,
    ///    Modbus, etc.). For push-based sources this is a no-op.
    /// 2. `collector.collect_once()` — read cached values, build `ScadaReading`s.
    /// 3. `ts_engine.record()` — persist to time-series.
    /// 4. `event_bus.publish()` — emit `DataReceived` events.
    ///
    /// Returns a JoinHandle that can be used to abort the task.
    pub fn start(&self, interval_ms: u64) -> JoinHandle<()> {
        // Use a watch channel that never fires (always false) so the loop
        // runs indefinitely — equivalent to the old behavior. This keeps
        // `start()` backward-compatible for tests and simple use cases.
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = self.start_with_shutdown(interval_ms, shutdown_rx);
        // Keep the sender alive for the task's lifetime by leaking it.
        // The task will be cancelled via `JoinHandle::abort()` in this mode.
        std::mem::forget(shutdown_tx);
        handle
    }

    /// Start a background task with graceful shutdown support.
    ///
    /// The loop runs until `shutdown_rx` receives `true` (or the sender is
    /// dropped, which also triggers shutdown). Use this in production code
    /// instead of `start()` to ensure the pipeline can drain its current
    /// cycle before exiting.
    ///
    /// Returns the `JoinHandle` for the background task. The caller should
    /// `await` this handle after sending the shutdown signal to ensure
    /// graceful termination.
    pub fn start_with_shutdown(
        &self,
        interval_ms: u64,
        shutdown_rx: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        let collector = self.collector.clone();
        let ts_engine = self.ts_engine.clone();
        let event_bus = self.event_bus.clone();

        tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            loop {
                // Check for shutdown signal before starting the cycle.
                if *shutdown_rx.borrow() {
                    tracing::info!("DataPipeline shutting down gracefully");
                    break;
                }

                // Refresh upstream cache before collecting. This is the
                // critical fix for F2: without this call, IEC 104 data never
                // enters the collection loop because `DataSource::read()`
                // only reads the local cache.
                collector.refresh_data_source().await;

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

                // Wait for the next interval, but also listen for shutdown.
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(interval_ms)) => {}
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            tracing::info!("DataPipeline shutting down gracefully (during sleep)");
                            break;
                        }
                    }
                }
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
            pool: Default::default(),
        };

        let collector = Arc::new(ScadaCollector::new(config, mock));
        let ts_engine = Arc::new(TimeSeriesEngine::new(1000));

        DataPipeline::new(collector, ts_engine)
    }

    #[tokio::test]
    async fn test_run_once() {
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

        let count = pipeline.run_once().await.unwrap();
        assert_eq!(count, 2);

        // Verify data was written to time-series engine
        let dp = pipeline.ts_engine.latest(1, "voltage_pu").unwrap();
        assert!((dp.value - 1.02).abs() < f64::EPSILON);

        let dp = pipeline.ts_engine.latest(2, "active_power_mw").unwrap();
        assert!((dp.value - 50.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_run_once_with_event_bus() {
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

        let count = pipeline.run_once().await.unwrap();
        assert_eq!(count, 1);

        // Verify event was published
        let event = rx.try_recv().unwrap();
        assert_eq!(event.event_type, EventType::DataReceived);
        assert_eq!(event.source, "eneros-scada");
    }

    #[tokio::test]
    async fn test_run_once_empty_config() {
        let pipeline = make_pipeline(vec![]);
        let count = pipeline.run_once().await.unwrap();
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
            pool: Default::default(),
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
