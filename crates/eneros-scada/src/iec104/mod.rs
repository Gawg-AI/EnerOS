//! IEC 60870-5-104 protocol adapter for EnerOS SCADA.
//!
//! This module re-exports the IEC 104 client and ASDU types from `eneros-device`
//! and provides SCADA-specific data source and mapping layers.
//!
//! # Architecture
//!
//! ```text
//! IEC 104 Server (RTU/IED)
//!         │
//!         │ TCP (port 2404)
//!         ▼
//! Iec104Client (from eneros-device)
//!   ├── APCI framing (STARTDT/STOPDT/TESTFR/I-S-U frames)
//!   ├── ASDU parsing (from eneros-device)
//!   └── Data cache (IOA → InformationObject)
//!         │
//!         ▼
//! IoaMappingTable (mapping.rs)
//!   └── IOA → (element_id, parameter, scale, offset)
//!         │
//!         ▼
//! Iec104DataSource (datasource.rs)
//!   └── Implements DataSource trait
//!         │
//!         ▼
//! ScadaCollector → SnapshotBuilder → ConstraintEngine
//! ```

// Re-export IEC 104 types from eneros-device
pub use eneros_device::adapters::iec104::asdu::{
    Asdu, CauseOfTransmission, InformationObject, TypeId,
    DoublePointValue, MeasuredQuality, SinglePointQuality,
    parse_asdu, build_interrogation_command, build_single_command, build_setpoint_short_float,
};
pub use eneros_device::adapters::iec104::client::{
    Iec104Client, Iec104Config, ConnectionState,
};

pub mod datasource;
pub mod mapping;

pub use datasource::Iec104DataSource;
pub use mapping::{build_ieee14_ioa_mapping, IoaMapping, IoaMappingTable};
