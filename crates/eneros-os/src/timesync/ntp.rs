use crate::timesync::TimeSyncSource;

#[derive(Debug, Clone, Default)]
pub struct NtpStatus {
    pub synchronized: bool,
    pub offset_millis: i64,
    pub server: Option<String>,
}

pub struct NtpClient {
    #[allow(dead_code)]
    servers: Vec<String>,
    status: NtpStatus,
}

impl NtpClient {
    pub fn new(servers: Vec<String>) -> Self {
        Self {
            servers,
            status: NtpStatus::default(),
        }
    }

    pub fn status(&self) -> &NtpStatus {
        &self.status
    }

    pub fn source(&self) -> TimeSyncSource {
        TimeSyncSource::Ntp
    }
}
