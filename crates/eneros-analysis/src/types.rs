use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Generic analysis result wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult<T> {
    /// Whether the analysis converged
    pub converged: bool,
    /// Number of iterations performed
    pub iterations: u32,
    /// The analysis result data
    pub result: T,
    /// Warnings generated during analysis
    pub warnings: Vec<String>,
}

/// Errors that can occur during power system analysis
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum AnalysisError {
    #[error("Data incomplete: {0}")]
    DataIncomplete(String),

    #[error("Singular matrix: {0}")]
    SingularMatrix(String),

    #[error("No convergence after {0} iterations: {1}")]
    NoConvergence(u32, String),

    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),
}
