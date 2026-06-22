use thiserror::Error;

#[derive(Error, Debug)]
pub enum EnerOSError {
    #[error("Topology error: {0}")]
    Topology(String),

    #[error("Power flow error: {0}")]
    PowerFlow(String),

    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),

    #[error("Equipment error: {0}")]
    Equipment(String),

    #[error("Device error: {0}")]
    Device(String),

    #[error("Gateway error: {0}")]
    Gateway(String),

    #[error("Event bus error: {0}")]
    EventBus(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Safety violation: {0}")]
    Safety(String),

    #[error("Plugin error: {0}")]
    Plugin(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, EnerOSError>;
