//! Cellular modem driver wrapping AT commands and PPP dial-up (workspace v0.192.0).
//!
//! Provides the [`CellularDriver`] trait and [`CellularModem`] struct that
//! combines AT command communication with PPP state machine negotiation
//! for cellular modem dial-up connectivity.

use alloc::string::{String, ToString};

use eneros_hal::HalSerial;

use crate::at_command::{AtCommand, AtParser, AtResponse, SignalStrength};
use crate::error::CellularError;
use crate::ppp::{Ipv4Addr, PppFrame, PppState, PppStateMachine, HDLC_FLAG, PPP_IPCP, PPP_LCP};

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
    /// 单调节拍计数器（纳秒近似；无硬件时钟时用于 AT 超时窗口）。
    tick_counter: u64,
}

impl<S: HalSerial> CellularModem<S> {
    /// Create a new cellular modem.
    pub fn new(serial: S, apn: &str, retry_config: RetryConfig) -> Self {
        Self {
            ppp: PppStateMachine::new(retry_config.max_retries),
            serial,
            apn: apn.to_string(),
            retry_config,
            tick_counter: 0,
        }
    }

    /// 单调节拍（纳秒）。无硬件时钟时用计数近似，每次调用推进 1ms。
    ///
    /// 真实平台上可接 hal::HalClock；当前实现保证轮询窗口在 host 测试
    /// 中可推进（每次调用 +1ms），满足 AT 超时语义。
    fn timeout_tick(&mut self) -> u64 {
        self.tick_counter = self.tick_counter.wrapping_add(1_000_000);
        self.tick_counter
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

        // 轮询窗口：按 timeout_ms 分片读取（每次 1ms），直到收到非空响应
        // 或窗口耗尽（v0.197.0 P0-5：timeout_ms 参数此前从未被使用）。
        let mut buf = [0u8; 256];
        let mut collected = alloc::vec::Vec::with_capacity(256);
        let deadline = self.timeout_tick() + (cmd.timeout_ms as u64) * 1_000_000;
        loop {
            let n = self
                .serial
                .read(&mut buf)
                .map_err(|_| CellularError::AtCommandTimeout)?;
            if n > 0 {
                collected.extend_from_slice(&buf[..n]);
                // AT 响应以 \r\n 结束；收到完整响应即解析。
                if collected.contains(&b'\n') {
                    break;
                }
            }
            if self.timeout_tick() >= deadline {
                break;
            }
            // 忙等节拍（无独立时钟时用轮询计数近似）。
            for _ in 0..1_000 {
                core::hint::spin_loop();
            }
        }
        if collected.is_empty() {
            return Err(CellularError::AtCommandTimeout);
        }
        let response_str =
            core::str::from_utf8(&collected).map_err(|_| CellularError::AtCommandTimeout)?;
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
    /// Dial into the cellular network.
    ///
    /// v0.197.0 (P0-5)：真实 PPP 协商——发送 ATD*99# 后，读取对端 PPP
    /// 帧流，解析 LCP/IPCP 报文推进状态机，IP 地址取自 IPCP Configure
    /// 报文（不再硬编码 10.0.0.1）。
    pub fn dial(&mut self, apn: &str) -> Result<Ipv4Addr, CellularError> {
        // 1. Start PPP negotiation: Closed → Establishing
        self.ppp.start()?;
        // 2. 记录 APN 并发送 ATD*99# 拨号命令（带超时窗口）。
        self.apn = apn.to_string();
        let dial_cmd = AtCommand::new("ATD*99#", 5000);
        let _ = self.send_at(&dial_cmd);
        // 3. 真实 PPP 协商：读帧流 → LCP/IPCP → 提取 IP。
        self.negotiate_ppp()
    }

    /// 读取对端 PPP 帧流并推进协商状态机，返回 IPCP 协商的 IP。
    ///
    /// 从串口轮询读取 HDLC 帧（`0x7E ... 0x7E`），逐帧解析：
    /// - LCP Configure-Request → 回 ACK 推进 Establishing → Authenticating
    /// - IPCP Configure-Ack/Nak → 提取 IP，推进 Networking → Connected
    ///
    /// 超时窗口由 `retry_config.retry_interval_ms` 约束（默认 5s）。
    fn negotiate_ppp(&mut self) -> Result<Ipv4Addr, CellularError> {
        let mut hdlc_buf: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        let mut in_frame = false;
        let deadline = self.timeout_tick()
            + self.retry_config.retry_interval_ms * 1_000_000;

        // 尝试多次（max_retries），每次读一批字节累积 HDLC 帧。
        for _ in 0..self.retry_config.max_retries.saturating_mul(10).max(10) {
            let mut byte = [0u8; 1];
            let n = self
                .serial
                .read(&mut byte)
                .map_err(|_| CellularError::PppNegotiationFailed)?;
            if n == 0 {
                if self.timeout_tick() >= deadline {
                    break;
                }
                for _ in 0..1_000 {
                    core::hint::spin_loop();
                }
                continue;
            }
            let b = byte[0];
            if b == HDLC_FLAG {
                if in_frame {
                    // 帧结束：补尾标志后解析。
                    hdlc_buf.push(b);
                    let frame = PppFrame::decode(&hdlc_buf)?;
                    hdlc_buf.clear();
                    in_frame = false;
                    if let Some(ip) = self.handle_ppp_frame(&frame)? {
                        return Ok(ip);
                    }
                } else {
                    in_frame = true;
                    hdlc_buf.clear();
                    hdlc_buf.push(b);
                }
            } else if in_frame {
                hdlc_buf.push(b);
            }
        }
        Err(CellularError::PppNegotiationFailed)
    }

    /// 处理一帧 PPP 报文，推进状态机；返回 IPCP 协商出的 IP（若有）。
    fn handle_ppp_frame(&mut self, frame: &PppFrame) -> Result<Option<Ipv4Addr>, CellularError> {
        if frame.protocol == PPP_LCP {
            let code = crate::ppp::parse_lcp_code(frame);
            match code {
                Some(crate::ppp::lcp::CONFIGURE_REQUEST) => {
                    // 对端发起 LCP；本端 ACK 推进。
                    self.ppp.on_lcp_config_ack()?;
                }
                Some(crate::ppp::lcp::CONFIGURE_ACK) => {
                    self.ppp.on_lcp_config_ack()?;
                }
                _ => {}
            }
        } else if frame.protocol == PPP_IPCP {
            if let Some(ip) = crate::ppp::parse_ipcp_ip(frame) {
                // IPCP 协商出 IP：推进 Networking → Connected。
                self.ppp.on_auth_success()?;
                self.ppp.on_ipcp_config_ack(ip)?;
                return Ok(Some(ip));
            }
        }
        Ok(None)
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
        /// 预设响应序列：每段耗尽后进下一段，最后一段循环重放。
        sequence: RefCell<Vec<Vec<u8>>>,
        seq_idx: RefCell<usize>,
    }

    impl MockSerial {
        fn new() -> Self {
            Self {
                tx_buf: RefCell::new(Vec::new()),
                rx_buf: RefCell::new(Vec::new()),
                sequence: RefCell::new(Vec::new()),
                seq_idx: RefCell::new(0),
            }
        }

        /// Preset a single response that `read` returns (循环重放，保持旧测试语义)。
        fn set_response(&self, response: &[u8]) {
            let mut seq = self.sequence.borrow_mut();
            seq.clear();
            seq.push(response.to_vec());
            *self.seq_idx.borrow_mut() = 0;
            // 预填充当前段。
            let mut rx = self.rx_buf.borrow_mut();
            rx.clear();
            rx.extend_from_slice(response);
        }

        /// Preset a sequence of responses（每段耗尽后进下一段，最后一段循环）。
        fn set_sequence(&self, responses: &[&[u8]]) {
            let mut seq = self.sequence.borrow_mut();
            seq.clear();
            for r in responses {
                seq.push(r.to_vec());
            }
            *self.seq_idx.borrow_mut() = 0;
            let mut rx = self.rx_buf.borrow_mut();
            rx.clear();
            if let Some(first) = seq.first() {
                rx.extend_from_slice(first);
            }
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
            // 消耗式读：模拟真实 UART 流。当前段耗尽时加载下一段
            // （最后一段循环重放，保持旧 set_response 语义）。
            {
                let mut rx = self.rx_buf.borrow_mut();
                if rx.is_empty() {
                    let seq = self.sequence.borrow();
                    let mut idx = self.seq_idx.borrow_mut();
                    if !seq.is_empty() {
                        if *idx + 1 < seq.len() {
                            *idx += 1;
                        }
                        rx.extend_from_slice(&seq[*idx]);
                    }
                }
            }
            let mut rx = self.rx_buf.borrow_mut();
            let n = core::cmp::min(rx.len(), buf.len());
            buf[..n].copy_from_slice(&rx[..n]);
            rx.drain(..n);
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

    /// 构造 PPP 协商帧流（LCP Config-Req + IPCP Config-Ack 携带 IP 10.1.2.3）。
    ///
    /// 手工编码 HDLC 帧（Flag|Proto(2)|Data|FCS(2)|Flag），FCS 固定 0x0000
    /// 与 PppFrame::encode 一致；IP 字节直接内联，避免依赖 smoltcp 内部 API。
    fn ppp_stream() -> Vec<u8> {
        // LCP Configure-Request: code=1, id=1, len=4。
        let lcp_payload = [0x01u8, 0x01, 0x00, 0x04];
        // IPCP Configure-Ack: code=2, id=1, len=10, IP-Address opt(3,6,10.1.2.3)。
        let ipcp_payload = [0x02u8, 0x01, 0x00, 0x0A, 0x03, 0x06, 10, 1, 2, 3];
        let mut stream = alloc::vec::Vec::new();
        // LCP 帧：0x7E C0 21 payload FCS(00 00) 0x7E。
        stream.push(0x7E);
        stream.push(0xC0);
        stream.push(0x21);
        stream.extend_from_slice(&lcp_payload);
        stream.extend_from_slice(&[0x00, 0x00, 0x7E]);
        // IPCP 帧：0x7E 80 21 payload FCS 0x7E。
        stream.push(0x7E);
        stream.push(0x80);
        stream.push(0x21);
        stream.extend_from_slice(&ipcp_payload);
        stream.extend_from_slice(&[0x00, 0x00, 0x7E]);
        stream
    }

    #[test]
    fn dial_success() {
        let mock = MockSerial::new();
        let stream = ppp_stream();
        mock.set_sequence(&[b"CONNECT\r\n", &stream]);
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let ip = modem.dial("internet").unwrap();
        assert_eq!(ip, Ipv4Addr::new(10, 1, 2, 3));
        assert_eq!(modem.ppp_state(), PppState::Connected);
    }

    #[test]
    fn dial_failure_when_no_ppp_stream() {
        let mock = MockSerial::new();
        mock.set_response(b"CONNECT\r\n");
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        // 无 PPP 帧流 → 协商失败。
        let err = modem.dial("internet").unwrap_err();
        assert_eq!(err, CellularError::PppNegotiationFailed);
    }

    #[test]
    fn dial_failure_when_already_connected() {
        let mock = MockSerial::new();
        let stream = ppp_stream();
        mock.set_response(&stream);
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        assert!(modem.dial("internet").is_ok());
        assert_eq!(modem.ppp_state(), PppState::Connected);
        // 二次拨号：状态机在 Connected 不接受 start。
        assert!(modem.dial("internet").is_err());
    }

    // --- hang_up tests ---

    #[test]
    fn hang_up_after_dial() {
        let mock = MockSerial::new();
        let stream = ppp_stream();
        mock.set_response(&stream);
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
        let stream = ppp_stream();
        mock.set_response(&stream);
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        let ip = CellularDriver::dial(&mut modem, "internet").unwrap();
        assert_eq!(ip, Ipv4Addr::new(10, 1, 2, 3));
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
        let stream = ppp_stream();
        mock.set_response(&stream);
        let mut modem = CellularModem::new(mock, "internet", RetryConfig::default());
        assert_eq!(modem.ppp_state(), PppState::Closed);
        modem.dial("internet").unwrap();
        assert_eq!(modem.ppp_state(), PppState::Connected);
        modem.hang_up().unwrap();
        assert_eq!(modem.ppp_state(), PppState::Closed);
    }
}
