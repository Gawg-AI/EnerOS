use crate::timesync::TimeSyncSource;

#[derive(Debug, Clone, Default)]
pub struct PtpStatus {
    pub locked: bool,
    pub offset_nanos: i64,
    pub grandmaster_id: Option<String>,
}

pub struct PtpClient {
    #[allow(dead_code)]
    interface: String,
    status: PtpStatus,
}

impl PtpClient {
    pub fn new(interface: &str) -> Self {
        Self {
            interface: interface.to_string(),
            status: PtpStatus::default(),
        }
    }

    pub fn status(&self) -> &PtpStatus {
        &self.status
    }

    pub fn source(&self) -> TimeSyncSource {
        TimeSyncSource::Ptp
    }
}
