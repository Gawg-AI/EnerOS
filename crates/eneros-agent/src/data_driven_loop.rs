use std::collections::HashMap;
use std::sync::Arc;

use eneros_constraint::ConstraintEngine;
use eneros_core::{ElementId, Result};
use eneros_eventbus::event::{EventPayload, EventType};
use eneros_eventbus::Event;
use eneros_scada::{DataPipeline, ScadaCollector, ScadaReading, SnapshotBuilder};
use parking_lot::RwLock;

use crate::emergency::EmergencyResponsePipeline;
use crate::orchestrator::AgentOrchestrator;
use crate::system_state::{StateTransitionTrigger, SystemStateMachine};

/// Emergency trigger detected from SCADA readings
#[derive(Debug, Clone, PartialEq)]
pub enum EmergencyTrigger {
    /// Voltage collapse detected: element_id, voltage value
    VoltageCollapse(ElementId, f64),
    /// Frequency deviation detected: frequency value
    FrequencyDeviation(f64),
}

/// Result of a single data-driven agent loop cycle
#[derive(Debug, Clone)]
pub struct DataDrivenCycleResult {
    /// Number of SCADA points collected
    pub points_collected: usize,
    /// Data changes exceeding deadband: (element_id, parameter, new_value)
    pub data_changes: Vec<(ElementId, String, f64)>,
    /// Emergency trigger if detected
    pub emergency_triggered: Option<EmergencyTrigger>,
    /// Whether a power system snapshot was built
    pub snapshot_built: bool,
    /// Number of constraint violations found
    pub constraint_violations: usize,
    /// Number of agents triggered via orchestrator
    pub agents_triggered: usize,
}

/// Data-driven agent loop: collects SCADA data, detects changes and emergencies,
/// builds snapshots, checks constraints, and dispatches events to agents.
pub struct DataDrivenAgentLoop {
    pipeline: Arc<DataPipeline>,
    collector: Arc<ScadaCollector>,
    snapshot_builder: Arc<SnapshotBuilder>,
    orchestrator: Arc<AgentOrchestrator>,
    constraint_engine: Option<Arc<ConstraintEngine>>,
    emergency_pipeline: Option<Arc<EmergencyResponsePipeline>>,
    state_machine: Arc<SystemStateMachine>,
    emergency_voltage_threshold: f64,
    emergency_frequency_low: f64,
    emergency_frequency_high: f64,
    previous_values: RwLock<HashMap<(ElementId, String), f64>>,
}

impl DataDrivenAgentLoop {
    /// Create a new DataDrivenAgentLoop
    pub fn new(
        pipeline: Arc<DataPipeline>,
        collector: Arc<ScadaCollector>,
        snapshot_builder: Arc<SnapshotBuilder>,
        orchestrator: Arc<AgentOrchestrator>,
        state_machine: Arc<SystemStateMachine>,
    ) -> Self {
        Self {
            pipeline,
            collector,
            snapshot_builder,
            orchestrator,
            constraint_engine: None,
            emergency_pipeline: None,
            state_machine,
            emergency_voltage_threshold: 0.90,
            emergency_frequency_low: 49.0,
            emergency_frequency_high: 51.0,
            previous_values: RwLock::new(HashMap::new()),
        }
    }

    /// Set the constraint engine
    pub fn with_constraint_engine(mut self, engine: Arc<ConstraintEngine>) -> Self {
        self.constraint_engine = Some(engine);
        self
    }

    /// Set the emergency response pipeline
    pub fn with_emergency_pipeline(mut self, pipeline: Arc<EmergencyResponsePipeline>) -> Self {
        self.emergency_pipeline = Some(pipeline);
        self
    }

    /// Set emergency thresholds: voltage (p.u.), frequency low (Hz), frequency high (Hz)
    pub fn with_emergency_thresholds(
        mut self,
        voltage: f64,
        freq_low: f64,
        freq_high: f64,
    ) -> Self {
        self.emergency_voltage_threshold = voltage;
        self.emergency_frequency_low = freq_low;
        self.emergency_frequency_high = freq_high;
        self
    }

    /// Check for data changes exceeding deadband compared to previous values.
    /// Returns a list of (element_id, parameter, new_value) for changed readings.
    pub fn check_data_changes(&self, readings: &[ScadaReading]) -> Vec<(ElementId, String, f64)> {
        let previous = self.previous_values.read();
        let mut changes = Vec::new();

        for reading in readings {
            let key = (reading.element_id, reading.parameter.clone());
            if let Some(&prev_value) = previous.get(&key) {
                let diff = (reading.value - prev_value).abs();
                if diff > f64::EPSILON {
                    changes.push((reading.element_id, reading.parameter.clone(), reading.value));
                }
            } else {
                // First time seeing this key — count as a change
                changes.push((reading.element_id, reading.parameter.clone(), reading.value));
            }
        }

        changes
    }

    /// Check if any reading triggers an emergency condition.
    /// - Voltage below threshold → VoltageCollapse
    /// - Frequency out of range → FrequencyDeviation
    pub fn check_emergency_triggers(&self, readings: &[ScadaReading]) -> Option<EmergencyTrigger> {
        for reading in readings {
            // Check voltage parameters
            if reading.parameter.contains("voltage")
                && reading.value < self.emergency_voltage_threshold
            {
                return Some(EmergencyTrigger::VoltageCollapse(
                    reading.element_id,
                    reading.value,
                ));
            }

            // Check frequency parameters
            if reading.parameter.contains("frequency")
                && (reading.value < self.emergency_frequency_low
                    || reading.value > self.emergency_frequency_high)
            {
                return Some(EmergencyTrigger::FrequencyDeviation(reading.value));
            }
        }
        None
    }

    /// Execute one complete data-driven cycle.
    ///
    /// 1. Collect data via pipeline.run_once()
    /// 2. Check for emergency triggers
    /// 3. Check for data changes exceeding deadband
    /// 4. If significant changes: build snapshot
    /// 5. If snapshot built: check constraints
    /// 6. If violations or significant changes: dispatch events to agents
    /// 7. Update previous_values
    /// 8. Return DataDrivenCycleResult
    pub async fn run_cycle(&self) -> Result<DataDrivenCycleResult> {
        // 1. Collect data
        let points_collected = self.pipeline.run_once()?;
        let readings = self.collector.latest_all();

        // 2. Check emergency triggers
        let emergency_triggered = self.check_emergency_triggers(&readings);

        if let Some(ref trigger) = emergency_triggered {
            // Transition state machine to Emergency
            self.state_machine
                .transition(StateTransitionTrigger::ManualOverride(
                    eneros_core::SystemOperatingState::Emergency,
                ));

            // Trigger emergency pipeline if available
            if let Some(ref emg_pipeline) = self.emergency_pipeline {
                let (freq, min_v) = match trigger {
                    EmergencyTrigger::VoltageCollapse(_, v) => (50.0, *v),
                    EmergencyTrigger::FrequencyDeviation(f) => (*f, 1.0),
                };
                emg_pipeline.auto_respond(
                    freq,
                    0,
                    min_v,
                    1,
                    eneros_core::SystemOperatingState::Emergency,
                );
            }
        }

        // 3. Check for data changes
        let data_changes = self.check_data_changes(&readings);

        // 4. Build snapshot if there are significant data changes
        let mut snapshot_built = false;
        let mut constraint_violations = 0usize;

        if !data_changes.is_empty() {
            match self.snapshot_builder.build(&self.collector) {
                Ok(state) => {
                    snapshot_built = true;

                    // 5. Check constraints if snapshot was built
                    if let Some(ref ce) = self.constraint_engine {
                        let bus_voltages: Vec<(ElementId, f64)> = state
                            .bus_voltages
                            .iter()
                            .map(|bv| (bv.bus_id, bv.voltage_magnitude))
                            .collect();
                        let branch_loadings: Vec<(ElementId, f64)> = state
                            .branch_flows
                            .iter()
                            .map(|bf| (bf.branch_id, bf.loading_percent))
                            .collect();
                        let violations = ce.check_all(&bus_voltages, &branch_loadings, state.frequency);
                        constraint_violations = violations.len();
                    }
                }
                Err(_) => {
                    // Snapshot build failed (e.g., missing required fields)
                }
            }
        }

        // 6. Dispatch events to agents if there are violations or significant changes
        let mut agents_triggered = 0usize;

        if constraint_violations > 0 || !data_changes.is_empty() {
            let event_type = if constraint_violations > 0 {
                EventType::ConstraintViolation
            } else {
                EventType::DataReceived
            };

            let payload = if constraint_violations > 0 {
                EventPayload::Message(format!(
                    "Data-driven loop: {} constraint violations detected, {} data changes",
                    constraint_violations,
                    data_changes.len()
                ))
            } else {
                EventPayload::Message(format!(
                    "Data-driven loop: {} data changes detected",
                    data_changes.len()
                ))
            };

            let event = Event::new(event_type, "data_driven_loop", payload);

            // Dispatch the event through the orchestrator
            let dispatch_results = self.orchestrator.process_event(event).await?;
            agents_triggered = dispatch_results.len();
        }

        // 7. Update previous values
        {
            let mut prev = self.previous_values.write();
            for reading in &readings {
                prev.insert(
                    (reading.element_id, reading.parameter.clone()),
                    reading.value,
                );
            }
        }

        Ok(DataDrivenCycleResult {
            points_collected,
            data_changes,
            emergency_triggered,
            snapshot_built,
            constraint_violations,
            agents_triggered,
        })
    }

    /// Start a background task running run_cycle in a loop at the given interval.
    /// Returns a JoinHandle that can be used to abort the task.
    pub fn start(&self, cycle_interval_ms: u64) -> tokio::task::JoinHandle<()> {
        let loop_instance = self.clone_inner();

        tokio::spawn(async move {
            loop {
                if let Err(e) = loop_instance.run_cycle().await {
                    tracing::error!("Data-driven agent loop cycle failed: {}", e);
                }
                tokio::time::sleep(std::time::Duration::from_millis(cycle_interval_ms)).await;
            }
        })
    }

    /// Clone the inner state for use in the spawned task.
    /// Since we use Arc everywhere, this is cheap.
    fn clone_inner(&self) -> Self {
        Self {
            pipeline: self.pipeline.clone(),
            collector: self.collector.clone(),
            snapshot_builder: self.snapshot_builder.clone(),
            orchestrator: self.orchestrator.clone(),
            constraint_engine: self.constraint_engine.clone(),
            emergency_pipeline: self.emergency_pipeline.clone(),
            state_machine: self.state_machine.clone(),
            emergency_voltage_threshold: self.emergency_voltage_threshold,
            emergency_frequency_low: self.emergency_frequency_low,
            emergency_frequency_high: self.emergency_frequency_high,
            previous_values: RwLock::new(HashMap::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_constraint::rules::Constraint;
    use eneros_constraint::ConstraintType;
    use eneros_eventbus::EventBus;
    use eneros_gateway::SafetyGateway;
    use eneros_memory::InMemoryMemory;
    use eneros_network::PowerNetwork;
    use eneros_reasoning::RuleBasedEngine;
    use eneros_scada::collector::MockDataSource;
    use eneros_scada::config::{ScadaConfig, ScadaPoint};
    use eneros_scada::snapshot::{MeasurementField, MeasurementMapping};
    use eneros_tool::ToolEngine;
    use eneros_timeseries::TimeSeriesEngine;

    use crate::agent::{AgentType, MockAgent};
    use crate::context::AgentContext;
    use crate::event_adapter::AgentEventHandler;

    fn make_scada_config(points: Vec<ScadaPoint>) -> ScadaConfig {
        ScadaConfig {
            points,
            default_scan_rate_ms: 1000,
            timeout_ms: 5000,
            enable_quality_check: true,
        }
    }

    fn make_agent_context() -> AgentContext {
        AgentContext::new(
            Arc::new(EventBus::new(64)),
            Arc::new(SafetyGateway::new(100)),
            Arc::new(RwLock::new(ToolEngine::new())),
            Arc::new(RwLock::new(PowerNetwork::from_ieee14())),
            Arc::new(InMemoryMemory::default()),
            Arc::new(RuleBasedEngine::new()),
        )
    }

    fn make_orchestrator() -> AgentOrchestrator {
        AgentOrchestrator::new(make_agent_context())
    }

    /// Helper: create a DataDrivenAgentLoop with a mock data source and basic config
    fn make_loop(
        mock: Arc<MockDataSource>,
        points: Vec<ScadaPoint>,
        mappings: Vec<MeasurementMapping>,
    ) -> DataDrivenAgentLoop {
        let config = make_scada_config(points);
        let collector = Arc::new(ScadaCollector::new(config, mock));
        let ts_engine = Arc::new(TimeSeriesEngine::new(1000));
        let pipeline = Arc::new(DataPipeline::new(collector.clone(), ts_engine));
        let snapshot_builder = Arc::new(SnapshotBuilder::new(mappings));
        let orchestrator = Arc::new(make_orchestrator());
        let state_machine = Arc::new(SystemStateMachine::new());

        DataDrivenAgentLoop::new(
            pipeline,
            collector,
            snapshot_builder,
            orchestrator,
            state_machine,
        )
    }

    // === Builder pattern tests ===

    #[test]
    fn test_builder_pattern_default_thresholds() {
        let mock = Arc::new(MockDataSource::new());
        let loop_instance = make_loop(mock, vec![], vec![]);

        assert!((loop_instance.emergency_voltage_threshold - 0.90).abs() < f64::EPSILON);
        assert!((loop_instance.emergency_frequency_low - 49.0).abs() < f64::EPSILON);
        assert!((loop_instance.emergency_frequency_high - 51.0).abs() < f64::EPSILON);
        assert!(loop_instance.constraint_engine.is_none());
        assert!(loop_instance.emergency_pipeline.is_none());
    }

    #[test]
    fn test_builder_pattern_with_constraint_engine() {
        let mock = Arc::new(MockDataSource::new());
        let ce = Arc::new(ConstraintEngine::new());
        let loop_instance = make_loop(mock, vec![], vec![]).with_constraint_engine(ce);

        assert!(loop_instance.constraint_engine.is_some());
    }

    #[test]
    fn test_builder_pattern_with_emergency_pipeline() {
        let mock = Arc::new(MockDataSource::new());
        let ep = Arc::new(EmergencyResponsePipeline::new());
        let loop_instance = make_loop(mock, vec![], vec![]).with_emergency_pipeline(ep);

        assert!(loop_instance.emergency_pipeline.is_some());
    }

    #[test]
    fn test_builder_pattern_with_emergency_thresholds() {
        let mock = Arc::new(MockDataSource::new());
        let loop_instance = make_loop(mock, vec![], vec![])
            .with_emergency_thresholds(0.85, 48.0, 52.0);

        assert!((loop_instance.emergency_voltage_threshold - 0.85).abs() < f64::EPSILON);
        assert!((loop_instance.emergency_frequency_low - 48.0).abs() < f64::EPSILON);
        assert!((loop_instance.emergency_frequency_high - 52.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_builder_pattern_all_options() {
        let mock = Arc::new(MockDataSource::new());
        let ce = Arc::new(ConstraintEngine::new());
        let ep = Arc::new(EmergencyResponsePipeline::new());
        let loop_instance = make_loop(mock, vec![], vec![])
            .with_constraint_engine(ce)
            .with_emergency_pipeline(ep)
            .with_emergency_thresholds(0.80, 47.5, 52.5);

        assert!(loop_instance.constraint_engine.is_some());
        assert!(loop_instance.emergency_pipeline.is_some());
        assert!((loop_instance.emergency_voltage_threshold - 0.80).abs() < f64::EPSILON);
        assert!((loop_instance.emergency_frequency_low - 47.5).abs() < f64::EPSILON);
        assert!((loop_instance.emergency_frequency_high - 52.5).abs() < f64::EPSILON);
    }

    // === check_data_changes tests ===

    #[test]
    fn test_check_data_changes_first_reading() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);

        let points = vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: None,
            max_value: None,
        }];

        let loop_instance = make_loop(mock, points, vec![]);

        // Collect data first
        let readings = loop_instance.collector.collect_once();
        let changes = loop_instance.check_data_changes(&readings);

        // First reading: all values are new, so all are changes
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, 1);
        assert_eq!(changes[0].1, "voltage_pu");
        assert!((changes[0].2 - 1.02).abs() < f64::EPSILON);
    }

    #[test]
    fn test_check_data_changes_no_change() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);

        let points = vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: None,
            max_value: None,
        }];

        let loop_instance = make_loop(mock.clone(), points, vec![]);

        // First cycle: populate previous_values
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(loop_instance.run_cycle()).unwrap();

        // Second cycle with same data: no changes expected
        let readings = loop_instance.collector.collect_once();
        let changes = loop_instance.check_data_changes(&readings);

        assert!(changes.is_empty());
    }

    #[test]
    fn test_check_data_changes_with_change() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);

        let points = vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: None,
            max_value: None,
        }];

        let loop_instance = make_loop(mock.clone(), points, vec![]);

        // First cycle
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(loop_instance.run_cycle()).unwrap();

        // Change the data
        mock.insert(1, "voltage_pu", 0.95);

        // Re-collect
        let readings = loop_instance.collector.collect_once();
        let changes = loop_instance.check_data_changes(&readings);

        assert_eq!(changes.len(), 1);
        assert!((changes[0].2 - 0.95).abs() < f64::EPSILON);
    }

    // === check_emergency_triggers tests ===

    #[test]
    fn test_check_emergency_triggers_voltage_collapse() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 0.85);

        let points = vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: None,
            max_value: None,
        }];

        let loop_instance = make_loop(mock, points, vec![]);
        let readings = loop_instance.collector.collect_once();
        let trigger = loop_instance.check_emergency_triggers(&readings);

        assert!(trigger.is_some());
        match trigger.unwrap() {
            EmergencyTrigger::VoltageCollapse(id, v) => {
                assert_eq!(id, 1);
                assert!((v - 0.85).abs() < f64::EPSILON);
            }
            EmergencyTrigger::FrequencyDeviation(_) => panic!("Expected VoltageCollapse"),
        }
    }

    #[test]
    fn test_check_emergency_triggers_frequency_low() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(0, "frequency_hz", 48.5);

        let points = vec![ScadaPoint {
            element_id: 0,
            parameter: "frequency_hz".to_string(),
            scan_rate_ms: 1000,
            deadband: 0.0,
            min_value: None,
            max_value: None,
        }];

        let loop_instance = make_loop(mock, points, vec![]);
        let readings = loop_instance.collector.collect_once();
        let trigger = loop_instance.check_emergency_triggers(&readings);

        assert!(trigger.is_some());
        match trigger.unwrap() {
            EmergencyTrigger::FrequencyDeviation(f) => {
                assert!((f - 48.5).abs() < f64::EPSILON);
            }
            EmergencyTrigger::VoltageCollapse(_, _) => panic!("Expected FrequencyDeviation"),
        }
    }

    #[test]
    fn test_check_emergency_triggers_frequency_high() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(0, "frequency_hz", 51.5);

        let points = vec![ScadaPoint {
            element_id: 0,
            parameter: "frequency_hz".to_string(),
            scan_rate_ms: 1000,
            deadband: 0.0,
            min_value: None,
            max_value: None,
        }];

        let loop_instance = make_loop(mock, points, vec![]);
        let readings = loop_instance.collector.collect_once();
        let trigger = loop_instance.check_emergency_triggers(&readings);

        assert!(trigger.is_some());
        match trigger.unwrap() {
            EmergencyTrigger::FrequencyDeviation(f) => {
                assert!((f - 51.5).abs() < f64::EPSILON);
            }
            EmergencyTrigger::VoltageCollapse(_, _) => panic!("Expected FrequencyDeviation"),
        }
    }

    #[test]
    fn test_check_emergency_triggers_no_trigger() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);
        mock.insert(0, "frequency_hz", 50.0);

        let points = vec![
            ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            },
            ScadaPoint {
                element_id: 0,
                parameter: "frequency_hz".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.0,
                min_value: None,
                max_value: None,
            },
        ];

        let loop_instance = make_loop(mock, points, vec![]);
        let readings = loop_instance.collector.collect_once();
        let trigger = loop_instance.check_emergency_triggers(&readings);

        assert!(trigger.is_none());
    }

    // === run_cycle tests ===

    #[tokio::test]
    async fn test_run_cycle_no_emergency_no_changes() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);
        mock.insert(0, "frequency_hz", 50.0);

        let points = vec![
            ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            },
            ScadaPoint {
                element_id: 0,
                parameter: "frequency_hz".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.0,
                min_value: None,
                max_value: None,
            },
        ];

        let loop_instance = make_loop(mock, points, vec![]);

        // First cycle: data changes (first time)
        let result = loop_instance.run_cycle().await.unwrap();
        assert_eq!(result.points_collected, 2);
        assert!(result.emergency_triggered.is_none());
        assert!(!result.data_changes.is_empty()); // first time = all changes
        // With no mappings, snapshot_builder.build() returns an empty but Ok state
        // snapshot_built is true because data_changes triggered a build attempt
        assert!(result.snapshot_built);

        // Second cycle: same data, no changes
        let result2 = loop_instance.run_cycle().await.unwrap();
        assert_eq!(result2.points_collected, 2);
        assert!(result2.emergency_triggered.is_none());
        assert!(result2.data_changes.is_empty());
    }

    #[tokio::test]
    async fn test_run_cycle_with_data_changes_triggering_snapshot() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);
        mock.insert(0, "frequency_hz", 50.0);

        let points = vec![
            ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            },
            ScadaPoint {
                element_id: 0,
                parameter: "frequency_hz".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.0,
                min_value: None,
                max_value: None,
            },
        ];

        let mappings = vec![
            MeasurementMapping {
                scada_parameter: "voltage_pu".to_string(),
                target_field: MeasurementField::BusVoltage(1),
            },
            MeasurementMapping {
                scada_parameter: "frequency_hz".to_string(),
                target_field: MeasurementField::Frequency,
            },
        ];

        let loop_instance = make_loop(mock.clone(), points, mappings);

        // First cycle: data changes + snapshot built
        let result = loop_instance.run_cycle().await.unwrap();
        assert_eq!(result.points_collected, 2);
        assert!(!result.data_changes.is_empty());
        assert!(result.snapshot_built);

        // Change data
        mock.insert(1, "voltage_pu", 0.98);
        let result2 = loop_instance.run_cycle().await.unwrap();
        assert!(!result2.data_changes.is_empty());
        assert!(result2.snapshot_built);
    }

    #[tokio::test]
    async fn test_run_cycle_with_emergency_trigger() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 0.85);
        mock.insert(0, "frequency_hz", 50.0);

        let points = vec![
            ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            },
            ScadaPoint {
                element_id: 0,
                parameter: "frequency_hz".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.0,
                min_value: None,
                max_value: None,
            },
        ];

        let ep = Arc::new(EmergencyResponsePipeline::new());
        let loop_instance = make_loop(mock, points, vec![]).with_emergency_pipeline(ep);

        let result = loop_instance.run_cycle().await.unwrap();
        assert_eq!(result.points_collected, 2);
        assert!(result.emergency_triggered.is_some());
        match result.emergency_triggered.unwrap() {
            EmergencyTrigger::VoltageCollapse(id, v) => {
                assert_eq!(id, 1);
                assert!((v - 0.85).abs() < f64::EPSILON);
            }
            EmergencyTrigger::FrequencyDeviation(_) => panic!("Expected VoltageCollapse"),
        }

        // State machine should have transitioned to Emergency
        assert_eq!(
            loop_instance.state_machine.current_state(),
            eneros_core::SystemOperatingState::Emergency
        );
    }

    #[tokio::test]
    async fn test_run_cycle_with_constraint_violations() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 0.90);
        mock.insert(0, "frequency_hz", 50.0);

        let points = vec![
            ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            },
            ScadaPoint {
                element_id: 0,
                parameter: "frequency_hz".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.0,
                min_value: None,
                max_value: None,
            },
        ];

        let mappings = vec![
            MeasurementMapping {
                scada_parameter: "voltage_pu".to_string(),
                target_field: MeasurementField::BusVoltage(1),
            },
            MeasurementMapping {
                scada_parameter: "frequency_hz".to_string(),
                target_field: MeasurementField::Frequency,
            },
        ];

        let ce = Arc::new(ConstraintEngine::new());
        let mut constraint = Constraint::new(
            "v_low".to_string(),
            "Voltage low check".to_string(),
            ConstraintType::Voltage,
            0.95,
            1.05,
        );
        constraint.element_ids = vec![1];
        ce.register(constraint);

        let loop_instance = make_loop(mock, points, mappings).with_constraint_engine(ce);

        let result = loop_instance.run_cycle().await.unwrap();
        assert!(result.snapshot_built);
        assert!(result.constraint_violations > 0);
    }

    #[tokio::test]
    async fn test_run_cycle_with_agents_triggered() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);

        let points = vec![ScadaPoint {
            element_id: 1,
            parameter: "voltage_pu".to_string(),
            scan_rate_ms: 500,
            deadband: 0.01,
            min_value: None,
            max_value: None,
        }];

        let mappings = vec![MeasurementMapping {
            scada_parameter: "voltage_pu".to_string(),
            target_field: MeasurementField::BusVoltage(1),
        }];

        // Create orchestrator with an agent registered that handles DataReceived
        let mut orchestrator = make_orchestrator();
        let agent = MockAgent::new("op-1", "Operator Agent", AgentType::Operator);
        let handler = AgentEventHandler::new(
            Box::new(agent),
            vec![EventType::DataReceived],
        );
        orchestrator.register_agent(handler);

        let config = make_scada_config(points);
        let collector = Arc::new(ScadaCollector::new(config, mock));
        let ts_engine = Arc::new(TimeSeriesEngine::new(1000));
        let pipeline = Arc::new(DataPipeline::new(collector.clone(), ts_engine));
        let snapshot_builder = Arc::new(SnapshotBuilder::new(mappings));
        let state_machine = Arc::new(SystemStateMachine::new());

        let loop_instance = DataDrivenAgentLoop::new(
            pipeline,
            collector,
            snapshot_builder,
            Arc::new(orchestrator),
            state_machine,
        );

        let result = loop_instance.run_cycle().await.unwrap();
        // First cycle has data changes, so agents_triggered should be > 0
        assert!(result.agents_triggered > 0);
    }

    #[tokio::test]
    async fn test_run_cycle_emergency_frequency_deviation() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 1.02);
        mock.insert(0, "frequency_hz", 48.0);

        let points = vec![
            ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            },
            ScadaPoint {
                element_id: 0,
                parameter: "frequency_hz".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.0,
                min_value: None,
                max_value: None,
            },
        ];

        let ep = Arc::new(EmergencyResponsePipeline::new());
        let loop_instance = make_loop(mock, points, vec![]).with_emergency_pipeline(ep);

        let result = loop_instance.run_cycle().await.unwrap();
        assert!(result.emergency_triggered.is_some());
        match result.emergency_triggered.unwrap() {
            EmergencyTrigger::FrequencyDeviation(f) => {
                assert!((f - 48.0).abs() < f64::EPSILON);
            }
            EmergencyTrigger::VoltageCollapse(_, _) => panic!("Expected FrequencyDeviation"),
        }
    }

    #[test]
    fn test_emergency_trigger_custom_thresholds() {
        let mock = Arc::new(MockDataSource::new());
        mock.insert(1, "voltage_pu", 0.88);
        mock.insert(0, "frequency_hz", 49.5);

        let points = vec![
            ScadaPoint {
                element_id: 1,
                parameter: "voltage_pu".to_string(),
                scan_rate_ms: 500,
                deadband: 0.01,
                min_value: None,
                max_value: None,
            },
            ScadaPoint {
                element_id: 0,
                parameter: "frequency_hz".to_string(),
                scan_rate_ms: 1000,
                deadband: 0.0,
                min_value: None,
                max_value: None,
            },
        ];

        // With default thresholds: voltage 0.88 < 0.90 → trigger
        let loop_default = make_loop(mock.clone(), points.clone(), vec![]);
        let readings = loop_default.collector.collect_once();
        assert!(loop_default.check_emergency_triggers(&readings).is_some());

        // With custom thresholds: voltage 0.88 > 0.85 → no trigger
        let loop_custom = make_loop(mock, points, vec![])
            .with_emergency_thresholds(0.85, 49.0, 51.0);
        let readings2 = loop_custom.collector.collect_once();
        assert!(loop_custom.check_emergency_triggers(&readings2).is_none());
    }
}
