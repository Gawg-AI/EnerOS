//! COTP (Connection-Oriented Transport Protocol, ISO 8073) implementation.
//!
//! COTP is the transport layer used by IEC 61850 MMS, running over TCP.
//! This implements Class 0 (simple connection) which is the minimum
//! required for IEC 61850.
//!
//! Reference: ISO 8073, IEC 61850-8-1

use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, trace};

/// COTP TPDUI (Transport Protocol Data Unit) types
const CR: u8 = 0xE0;  // Connection Request
const CC: u8 = 0xD0;  // Connection Confirm
const DR: u8 = 0x80;  // Disconnect Request
#[allow(dead_code)]
const DC: u8 = 0xC0;  // Disconnect Confirm
const DT: u8 = 0xF0;  // Data Transfer
const ER: u8 = 0x70;  // Error

/// COTP connection parameters
#[derive(Debug, Clone)]
pub struct CotpParams {
    pub tpdu_size: u8,     // TPDU size code (7=128, 8=256, ... 12=8192)
    pub src_ref: u16,      // Source reference
    pub dst_ref: u16,      // Destination reference (0 for CR)
    pub class: u8,         // Protocol class (0 for Class 0)
}

impl Default for CotpParams {
    fn default() -> Self {
        Self {
            tpdu_size: 10,  // 1024 bytes
            src_ref: 1,
            dst_ref: 0,
            class: 0,
        }
    }
}

/// COTP connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CotpState {
    Closed,
    Waiting,
    Connected,
}

/// COTP transport layer over TCP
pub struct CotpTransport {
    stream: TcpStream,
    params: CotpParams,
    state: CotpState,
    /// Remote TSAP (Transport Service Access Point)
    remote_tsap: u16,
    /// Local TSAP
    local_tsap: u16,
    /// Receive buffer for partial TSDU reassembly
    recv_buf: Vec<u8>,
    /// Whether we're in the middle of a multi-DT TSDU
    tsdu_in_progress: bool,
}

impl CotpTransport {
    /// Connect to a COTP server over TCP
    pub async fn connect(
        addr: &str,
        local_tsap: u16,
        remote_tsap: u16,
    ) -> io::Result<Self> {
        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;

        let mut transport = Self {
            stream,
            params: CotpParams::default(),
            state: CotpState::Closed,
            remote_tsap,
            local_tsap,
            recv_buf: Vec::new(),
            tsdu_in_progress: false,
        };

        transport.send_cr().await?;
        transport.recv_cc().await?;
        transport.state = CotpState::Connected;

        debug!("COTP connected: local_tsap={}, remote_tsap={}", local_tsap, remote_tsap);
        Ok(transport)
    }

    /// Send a COTP Data (DT) TPDUI containing a complete TSDU
    pub async fn send_data(&mut self, tsdu: &[u8]) -> io::Result<()> {
        // For Class 0, the entire TSDU fits in one DT TPDUI
        // DT format: Length(1) + DT(1) + EOT+PDU_seq(1) + Data
        let mut tpdu = Vec::with_capacity(3 + tsdu.len());
        tpdu.push(DT);                      // TPDUI type
        tpdu.push(0x80);                    // EOT=1 (last unit), PDU seq=0
        tpdu.extend_from_slice(tsdu);

        // COTP frame: LI(1) + TPDU
        let li = tpdu.len() as u8;
        if li > 254 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "COTP TPDU too large"));
        }

        let mut frame = Vec::with_capacity(1 + tpdu.len());
        frame.push(li);
        frame.extend_from_slice(&tpdu);

        self.stream.write_all(&frame).await?;
        trace!("COTP DT sent: {} bytes", tsdu.len());
        Ok(())
    }

    /// Receive a complete TSDU (may span multiple DT TPDUIs)
    pub async fn recv_data(&mut self) -> io::Result<Vec<u8>> {
        self.recv_buf.clear();
        self.tsdu_in_progress = false;

        loop {
            let (_li, tpdu) = self.recv_tpdu().await?;

            let tpdu_type = tpdu.get(0).copied().unwrap_or(0);

            match tpdu_type {
                DT => {
                    // DT format: DT(1) + EOT+PDU_seq(1) + Data
                    if tpdu.len() < 2 {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "COTP DT too short"));
                    }
                    let eot = (tpdu[1] & 0x80) != 0;
                    let data = &tpdu[2..];
                    self.recv_buf.extend_from_slice(data);
                    self.tsdu_in_progress = !eot;

                    if eot {
                        trace!("COTP DT received: {} bytes", self.recv_buf.len());
                        return Ok(self.recv_buf.clone());
                    }
                }
                DR => {
                    self.state = CotpState::Closed;
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "COTP disconnect received",
                    ));
                }
                ER => {
                    let reason = tpdu.get(1).copied().unwrap_or(0);
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        format!("COTP error: reason={}", reason),
                    ));
                }
                _ => {
                    debug!("COTP unexpected TPDU type: 0x{:02X}", tpdu_type);
                }
            }
        }
    }

    /// Disconnect the COTP connection
    pub async fn disconnect(&mut self) -> io::Result<()> {
        if self.state == CotpState::Connected {
            self.send_dr().await?;
            self.state = CotpState::Closed;
        }
        Ok(())
    }

    /// Get the current connection state
    pub fn state(&self) -> CotpState {
        self.state
    }

    // ---- Internal methods ----

    async fn send_cr(&mut self) -> io::Result<()> {
        // CR format: LI(1) + CR(1) + dst_ref(2) + src_ref(2) + class(1) + variable_part
        let mut tpdu = Vec::with_capacity(32);
        tpdu.push(CR);                                  // TPDUI type
        tpdu.extend_from_slice(&0u16.to_be_bytes());    // Destination reference = 0
        tpdu.extend_from_slice(&self.params.src_ref.to_be_bytes()); // Source reference
        tpdu.push(self.params.class << 4);              // Class 0, no options

        // Variable part: TSAP calling/called
        // Parameter code 0xC0 = calling TSAP, 0xC1 = called TSAP
        tpdu.push(0xC1);  // Called TSAP parameter code
        tpdu.push(2);     // Parameter length
        tpdu.extend_from_slice(&self.remote_tsap.to_be_bytes());

        tpdu.push(0xC0);  // Calling TSAP parameter code
        tpdu.push(2);     // Parameter length
        tpdu.extend_from_slice(&self.local_tsap.to_be_bytes());

        // TPDU size parameter
        tpdu.push(0xC2);  // TPDU size parameter code
        tpdu.push(1);     // Parameter length
        tpdu.push(self.params.tpdu_size);

        let li = tpdu.len() as u8;
        let mut frame = vec![li];
        frame.extend_from_slice(&tpdu);

        self.stream.write_all(&frame).await?;
        debug!("COTP CR sent: local_tsap={}, remote_tsap={}", self.local_tsap, self.remote_tsap);
        Ok(())
    }

    async fn recv_cc(&mut self) -> io::Result<()> {
        let (_li, tpdu) = self.recv_tpdu().await?;

        if tpdu.is_empty() || tpdu[0] != CC {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("Expected COTP CC, got 0x{:02X}", tpdu.get(0).copied().unwrap_or(0)),
            ));
        }

        // Parse CC: CC(1) + dst_ref(2) + src_ref(2) + class(1)
        if tpdu.len() < 6 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "COTP CC too short"));
        }

        self.params.dst_ref = u16::from_be_bytes([tpdu[1], tpdu[2]]);
        self.params.src_ref = u16::from_be_bytes([tpdu[3], tpdu[4]]);

        debug!("COTP CC received: dst_ref={}, src_ref={}", self.params.dst_ref, self.params.src_ref);
        Ok(())
    }

    async fn send_dr(&mut self) -> io::Result<()> {
        let mut tpdu = Vec::with_capacity(7);
        tpdu.push(DR);                                  // TPDUI type
        tpdu.extend_from_slice(&self.params.dst_ref.to_be_bytes()); // Destination reference
        tpdu.extend_from_slice(&self.params.src_ref.to_be_bytes()); // Source reference
        tpdu.push(0x00);                                // Reason: normal disconnect

        let li = tpdu.len() as u8;
        let mut frame = vec![li];
        frame.extend_from_slice(&tpdu);

        self.stream.write_all(&frame).await?;
        debug!("COTP DR sent");
        Ok(())
    }

    async fn recv_tpdu(&mut self) -> io::Result<(u8, Vec<u8>)> {
        // Read LI (Length Indicator)
        let mut li_buf = [0u8; 1];
        self.stream.read_exact(&mut li_buf).await?;
        let li = li_buf[0] as usize;

        if li == 0 {
            return Ok((0, Vec::new()));
        }

        // Read TPDU data
        let mut tpdu = vec![0u8; li];
        self.stream.read_exact(&mut tpdu).await?;

        Ok((li_buf[0], tpdu))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cotp_params_default() {
        let params = CotpParams::default();
        assert_eq!(params.tpdu_size, 10);
        assert_eq!(params.class, 0);
    }

    #[test]
    fn test_cotp_state() {
        assert_eq!(CotpState::Closed, CotpState::Closed);
        assert_ne!(CotpState::Closed, CotpState::Connected);
    }
}
