//! System logging service with rotation

pub mod rotate;

pub use rotate::{LogRotator, RotateConfig, RotatePolicy};
