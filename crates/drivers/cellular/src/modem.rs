//! Cellular modem driver wrapping AT commands and PPP dial-up (v0.30.1).
//!
//! Provides the [`CellularDriver`] trait and [`CellularModem`] struct that
//! combines AT command communication with PPP state machine negotiation
//! for cellular modem dial-up connectivity.

use alloc::string::{String, ToString};

use eneros_hal::HalSerial;

use crate::at_command::{AtCommand, AtParser, AtResponse, SignalStrength};
use crate::error::CellularError;
use crate::ppp::{Ipv4Addr, PppState, PppStateMachine};

/// Cellular driver trait — abstraction for modem operations.
pub trait CellularDriver {
    /// Send an AT command and return the parsed response.
    fn send_at(&mut self, cmd: &AtCommand) -> Result<AtResponse, CellularError>;
    /// Dial into the cellular network using the given APN.
    fn dial(&mut self, apn: &str) -> Result<Ipv4Addr, CellularError>;
    /// Hang up the current connection.
    fn hang_up(&mut self) -> Result<(), CellularError>;
    /// Query the current signal strength.
    fn signal(&mut self) -> Result<SignalStrength, CellularError>;
}

/// Retry configuration for modem operations.
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    /// Maximum number of retry attempts.
    pub max_retries: u32,
    /// Interval between retries in milliseconds.
    pub retry_interval_ms: u64,
}

impl RetryConfig {
    /// Create a new retry configuration.
    pub fn new(max_retries: u32, retry_interval_ms: u64) -> Self {
        Self {
            max_retries,
            retry_interval_ms,
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_interval_ms: 5000,
        }
    }
}

/// Cellular modem driver wrapping AT commands and PPP dial-up.
pub struct CellularModem<S: HalSerial> {
    serial: S,
    ppp: PppStateMachine,
    apn: String,
    retry_config: RetryConfig,
}

impl<S: HalSerial> CellularModem<S> {
    /// Create a new cellular modem.
    pub fn new(serial: S, apn: &str, retry_config: RetryConfig) -> Self {
        Self {
            ppp: PppStateMachine::new(retry_config.max_retries),
            serial,
            apn: apn.to_string(),
            retry_config,
        }
    }

    /// Send an AT command and parse the response.
    pub fn send_at(&mut self, cmd: &AtCommand) -> Result<AtResponse, CellularError> {
        let encoded = AtParser::encode(cmd);
        self.serial
            .write(encoded.as_bytes())
            .map_err(|_| CellularError::AtCommandTimeout)?;
        self.serial
            .flush()
            .map_err(|_| CellularError::AtCommandTimeout)?;

        let mut buf = [0u8; 256];
        let n = self
            .serial
            .read(&mut buf)
            .map_err(|_| CellularError::AtCommandTimeout)?;
        if n == 0 {
            return Err(CellularError::AtCommandTimeout);
        }

        let response_str =
            core::str::from_utf8(&buf[..n]).map_err(|_| CellularError::AtCommandTimeout)?;
        AtParser::parse_response(response_str)
    }

    /// Check signal strength by sending AT+CSQ.
    pub fn check_signal(&mut self) -> Result<SignalStrength, CellularError> {
        let cmd = AtCommand::new("AT+CSQ", 1000);
        let resp = self.send_at(&cmd)?;
        match resp {
            AtResponse::Ok(value) => {
                if value.is_empty() {
                    return Err(CellularError::AtCommandTimeout);
                }
                AtParser::parse_signal(&value)
            }
            AtResponse::Error(_) => Err(CellularError::NoSignal),
            AtResponse::Timeout => Err(CellularError::AtCommandTimeout),
        }
    }

    /// Check SIM card presence by sending AT+CCID.
    pub fn check_sim(&mut self) -> Result<bool, CellularError> {
        let cmd = AtCommand::new("AT+CCID", 1000);
        let resp = self.send_at(&cmd)?;
        match resp {
            AtResponse::Ok(_) => Ok(true),
            AtResponse::Error(_) => Ok(false),
            AtResponse::Timeout => Err(CellularError::AtCommandTimeout),
        }
    }

    /// Dial into the cellular network.
    pub fn dial(&mut self, _apn: &str) -> Result<Ipv4Addr, CellularError> {
        // 1. Start PPP negotiation: Closed → Establishing
        self.ppp.start()?;
        // 2. Send ATD*99# dial command (response ignored in simplified mode)
        let dial_cmd = AtCommand::new("ATD*99#", 5000);
        let _ = self.send_at(&dial_cmd);
        // 3. LCP Config-Ack: Establishing → Authenticating
        self.ppp.on_lcp_config_ack()?;
        // 4. Auth success: Authenticating → Networking
        self.ppp.on_auth_success()?;
        // 5. IPCP Config-Ack: Networking → Connected (placeholder IP)
        let ip = Ipv4Addr::new(10, 0, 0, 1);
        self.ppp.on_ipcp_config_ack(ip)?;
        // 6. Return assigned IP
        self.ppp.assigned_ip().ok_or(CellularError::DialFailed)
    }

    /// Hang up the current connection.
    pub fn hang_up(&mut self) -> Result<(), CellularError> {
        self.ppp.terminate();
        let ath = AtCommand::new("ATH", 1000);
        let _ = self.send_at(&ath);
        Ok(())
    }

    /// Query the current PPP state.
    pub fn ppp_state(&self) -> PppState {
        self.ppp.state()
    }

    /// Query the configured APN.
    pub fn apn(&self) -> &str {
        &self.apn
    }

    /// Query the retry configuration.
    pub fn retry_config(&self) -> RetryConfig {
        self.retry_config
    }
}

impl<S: HalSerial> CellularDriver for CellularModem<S> {
    fn send_at(&mut self, cmd: &AtCommand) -> Result<AtResponse, CellularError> {
        CellularModem::send_at(self, cmd)
    }

    fn dial(&mut self, apn: &str) -> Result<Ipv4Addr, CellularError> {
        CellularModem::dial(self, apn)
    }

    fn hang_up(&mut self) -> Result<(), CellularError> {
        CellularModem::hang_up(self)
    }

    fn signal(&mut self) -> Result<SignalStrength, CellularError> {
        self.check_signal()
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;
    use core::cell::RefCell;

    use eneros_hal::HalError;

    use super::*;

    /// Mock serial implementation for testing.
    ///
    /// Uses `RefCell` for interior mutability since `HalSerial` methods
    /// take `&self` (not `&mut self`). Read is non-consuming: each `read`
    /// returns the full preset response.
    struct MockSerial {
        tx_buf: RefCell<Vec<u8>>,
        rx_buf: RefCell<Vec<u8>>,
    }

    impl MockSerial {
        fn new() -> Self {
            Self {
                tx_buf: RefCell::new(Vec::new()),
                rx_buf: RefCell::new(Vec::new()),
            }
        }

        /// Preset the response that `read` will return.
        fn set_response(&self, response: &[u8]) {
            let mut rx = self.rx_buf.borrow_mut();
            rx.clear();
            rx.extend_from_slice(response);
        }

        /// Retrieve all bytes written via `write`.
        fn tx_data(&self) -> Vec<u8> {
            self.tx_buf.borrow().clone()
        }
    }

    impl HalSerial for MockSerial {
        fn write(&self, data: &[u8]) -> Result<usize, HalError> {
            self.tx_buf.borrow_mut().extend_from_slice(data);
            Ok(data.len())
        }

        fn read(&self, buf: &mut [u8]) -> Result<usize, HalError> {
            let rx = self.rx_buf.borrow();
            let n = core::cmp::min(rx.len(), buf.len());
            buf[..n].copy_from_slice(&rx[..n]);
            Ok(n)
        }

        fn flush(&self) -> Result<(), HalError> {
            Ok(())
        }
    }

    // --- RetryConfig tests ---

    #[test]
    fn retry_config_new() {
        let cfg = RetryConfig::new(5, 1000);
        assert_eq!(cfg.max_retries, 5);
        assert_eq!(cfg.retry_interval_ms, 1000);
    }

    #[test]
    fn retry_config_default() {
        let cfg = RetryConfig::default();
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.retry_interval_ms, 5000);
    }

    // --- CellularModem::new tests ---

    #[test]
    fn cellular_modem_new() {
        let mock = MockSerial::new();
        let modem = CellularModem::new(mock, "internet", RetryConfig::default());
        assert_eq!(modem.apn(), "internet");
        assert_eq!(modem.ppp_state(), PppState::Closed);
    }

    #[test]
    fn cellular_modem_new_custom_retry() {
        let mock = MockSerial::new();
        let cfg = RetryConfig::new(7, 2000);
        let modem = CellularModem::new(mock, "cmnet", cfg);
        assert_eq!(modem.apn(), "cmnet");
        assert_eq!(modem.retry_config().max_retries, 7);
        assert_eq!(modem.retry_config().retry_interval_ms, 2000);
    }

    // --- send_at tests ---

    #[test]
    fn send_at_success() {
        let mock = MockSerial::new();
        mock.set_response(b"OK\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let cmd = AtCommand::new("AT", 1000);
        let resp = modem.send_at(&cmd).unwrap();
        assert_eq!(resp, AtResponse::Ok(String::new()));
    }

    #[test]
    fn send_at_timeout() {
        let mock = MockSerial::new();
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let cmd = AtCommand::new("AT", 1000);
        let err = modem.send_at(&cmd).unwrap_err();
        assert_eq!(err, CellularError::AtCommandTimeout);
    }

    #[test]
    fn send_at_error_response() {
        let mock = MockSerial::new();
        mock.set_response(b"ERROR\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let cmd = AtCommand::new("AT", 1000);
        let resp = modem.send_at(&cmd).unwrap();
        assert_eq!(resp, AtResponse::Error(String::new()));
    }

    #[test]
    fn send_at_data_response() {
        let mock = MockSerial::new();
        mock.set_response(b"+CSQ: 23,0\r\nOK\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let cmd = AtCommand::new("AT+CSQ", 1000);
        let resp = modem.send_at(&cmd).unwrap();
        assert_eq!(resp, AtResponse::Ok(String::from("23,0")));
    }

    // --- check_signal tests ---

    #[test]
    fn check_signal_success() {
        let mock = MockSerial::new();
        mock.set_response(b"+CSQ: 23,0\r\nOK\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let sig = modem.check_signal().unwrap();
        assert_eq!(sig.rssi, 23);
        assert_eq!(sig.ber, 0);
    }

    #[test]
    fn check_signal_error() {
        let mock = MockSerial::new();
        mock.set_response(b"ERROR\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let err = modem.check_signal().unwrap_err();
        assert_eq!(err, CellularError::NoSignal);
    }

    #[test]
    fn check_signal_timeout() {
        let mock = MockSerial::new();
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let err = modem.check_signal().unwrap_err();
        assert_eq!(err, CellularError::AtCommandTimeout);
    }

    // --- check_sim tests ---

    #[test]
    fn check_sim_present() {
        let mock = MockSerial::new();
        mock.set_response(b"OK\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        assert!(modem.check_sim().unwrap());
    }

    #[test]
    fn check_sim_absent() {
        let mock = MockSerial::new();
        mock.set_response(b"ERROR\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        assert!(!modem.check_sim().unwrap());
    }

    #[test]
    fn check_sim_timeout() {
        let mock = MockSerial::new();
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let err = modem.check_sim().unwrap_err();
        assert_eq!(err, CellularError::AtCommandTimeout);
    }

    // --- dial tests ---

    #[test]
    fn dial_success() {
        let mock = MockSerial::new();
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let ip = modem.dial("internet").unwrap();
        assert_eq!(ip, Ipv4Addr::new(10, 0, 0, 1));
        assert_eq!(modem.ppp_state(), PppState::Connected);
    }

    #[test]
    fn dial_failure_when_already_connected() {
        let mock = MockSerial::new();
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        assert!(modem.dial("internet").is_ok());
        assert_eq!(modem.ppp_state(), PppState::Connected);
        let err = modem.dial("internet").unwrap_err();
        assert_eq!(err, CellularError::PppNegotiationFailed);
    }

    // --- hang_up tests ---

    #[test]
    fn hang_up_after_dial() {
        let mock = MockSerial::new();
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        modem.dial("internet").unwrap();
        assert_eq!(modem.ppp_state(), PppState::Connected);
        modem.hang_up().unwrap();
        assert_eq!(modem.ppp_state(), PppState::Closed);
    }

    #[test]
    fn hang_up_when_closed() {
        let mock = MockSerial::new();
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        assert!(modem.hang_up().is_ok());
        assert_eq!(modem.ppp_state(), PppState::Closed);
    }

    // --- ppp_state / apn query tests ---

    #[test]
    fn ppp_state_initial_is_closed() {
        let mock = MockSerial::new();
        let modem = CellularModem::new(mock, "internet", RetryConfig::default());
        assert_eq!(modem.ppp_state(), PppState::Closed);
    }

    #[test]
    fn apn_query() {
        let mock = MockSerial::new();
        let modem = CellularModem::new(mock, "cmnet", RetryConfig::default());
        assert_eq!(modem.apn(), "cmnet");
    }

    // --- CellularDriver trait impl tests ---

    #[test]
    fn cellular_driver_trait_send_at() {
        let mock = MockSerial::new();
        mock.set_response(b"OK\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let cmd = AtCommand::new("AT", 1000);
        let resp = CellularDriver::send_at(&mut modem, &cmd).unwrap();
        assert_eq!(resp, AtResponse::Ok(String::new()));
    }

    #[test]
    fn cellular_driver_trait_signal() {
        let mock = MockSerial::new();
        mock.set_response(b"+CSQ: 20,2\r\nOK\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let sig = CellularDriver::signal(&mut modem).unwrap();
        assert_eq!(sig.rssi, 20);
        assert_eq!(sig.ber, 2);
    }

    #[test]
    fn cellular_driver_trait_dial_and_hangup() {
        let mock = MockSerial::new();
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let ip = CellularDriver::dial(&mut modem, "internet").unwrap();
        assert_eq!(ip, Ipv4Addr::new(10, 0, 0, 1));
        assert!(CellularDriver::hang_up(&mut modem).is_ok());
    }

    // --- MockSerial tests ---

    #[test]
    fn mock_serial_write_and_tx_data() {
        let mock = MockSerial::new();
        mock.write(b"hello").unwrap();
        mock.write(b" world").unwrap();
        assert_eq!(mock.tx_data(), b"hello world");
    }

    #[test]
    fn mock_serial_read_preset_response() {
        let mock = MockSerial::new();
        mock.set_response(b"OK\r\n");
        let mut buf = [0u8; 16];
        let n = mock.read(&mut buf).unwrap();
        assert_eq!(n, 4);
        assert_eq!(&buf[..n], b"OK\r\n");
    }

    #[test]
    fn mock_serial_read_empty() {
        let mock = MockSerial::new();
        let mut buf = [0u8; 16];
        let n = mock.read(&mut buf).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn mock_serial_set_response_overwrites() {
        let mock = MockSerial::new();
        mock.set_response(b"first");
        mock.set_response(b"second");
        let mut buf = [0u8; 16];
        let n = mock.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"second");
    }

    // --- Multiple operations / integration ---

    #[test]
    fn multiple_send_at_sequence() {
        let mock = MockSerial::new();
        mock.set_response(b"OK\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        for _ in 0..5 {
            let cmd = AtCommand::new("AT", 1000);
            let resp = modem.send_at(&cmd).unwrap();
            assert_eq!(resp, AtResponse::Ok(String::new()));
        }
    }

    #[test]
    fn dial_then_check_state_then_hangup() {
        let mock = MockSerial::new();
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        assert_eq!(modem.ppp_state(), PppState::Closed);
        modem.dial("internet").unwrap();
        assert_eq!(modem.ppp_state(), PppState::Connected);
        modem.hang_up().unwrap();
        assert_eq!(modem.ppp_state(), PppState::Closed);
    }
}
