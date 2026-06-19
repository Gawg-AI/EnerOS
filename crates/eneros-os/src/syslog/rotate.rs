use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RotatePolicy {
    Size(u64),       // Rotate when file exceeds N bytes
    Daily,           // Rotate daily
    Both(u64),       // Rotate on size OR daily, whichever first
}

impl Default for RotatePolicy {
    fn default() -> Self {
        Self::Size(100 * 1024 * 1024) // 100MB
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotateConfig {
    pub policy: RotatePolicy,
    pub max_files: u32,
    pub compress: bool,
    pub log_dir: PathBuf,
}

impl Default for RotateConfig {
    fn default() -> Self {
        Self {
            policy: RotatePolicy::default(),
            max_files: 7,
            compress: true,
            log_dir: PathBuf::from("/var/log/eneros"),
        }
    }
}

pub struct LogRotator {
    config: RotateConfig,
}

impl LogRotator {
    pub fn new(config: RotateConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &RotateConfig {
        &self.config
    }
}
