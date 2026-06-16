//! IEC 60870-5-104 TCP client with connection management.
//!
//! Handles the APCI framing, connection lifecycle, and data caching.
//! Supports control commands (C_SC_NA_1, C_SE_NC_1) and subscribe callbacks.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::asdu::{self, InformationObject};

// APCI frame types
const STARTDT_ACT: u8 = 0x07;
const STARTDT_CON: u8 = 0x0B;
const STOPDT_ACT: u8 = 0x13;
const STOPDT_CON: u8 = 0x23;
const TESTFR_ACT: u8 = 0x43;
const TESTFR_CON: u8 = 0x83;
const I_FRAME: u8 = 0x00;

/// IEC 104 connection configuration
#[derive(Debug, Clone)]
pub struct Iec104Config {
    pub remote_addr: String,
    pub asdu_address: u16,
    pub connect_timeout: Duration,
    pub reconnect_interval: Duration,
    pub test_interval: Duration,
    pub auto_interrogation: bool,
}

impl Default for Iec104Config {
    fn default() -> Self {
        Self {
            remote_addr: "127.0.0.1:2404".to_string(),
            asdu_address: 1,
            connect_timeout: Duration::from_secs(5),
            reconnect_interval: Duration::from_secs(5),
            test_interval: Duration::from_secs(30),
            auto_interrogation: true,
        }
    }
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    StartDtSent,
    Active,
}

/// Callback type for data updates
type DataCallback = Box<dyn Fn(u32, &InformationObject) + Send + Sync>;

/// IEC 104 TCP client
pub struct Iec104Client {
    config: Iec104Config,
    state: Arc<Mutex<ConnectionState>>,
    stream: Arc<Mutex<Option<tokio::net::TcpStream>>>,
    send_seq: Arc<AtomicU16>,
    recv_seq: Arc<AtomicU16>,
    running: Arc<AtomicBool>,
    /// Latest data values keyed by IOA
    pub data: Arc<Mutex<HashMap<u32, InformationObject>>>,
    /// Callbacks for data updates
    callbacks: Arc<Mutex<Vec<DataCallback>>>,
}

impl Iec104Client {
    pub fn new(config: Iec104Config) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(ConnectionState::Disconnected)),
            stream: Arc::new(Mutex::new(None)),
            send_seq: Arc::new(AtomicU16::new(0)),
            recv_seq: Arc::new(AtomicU16::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            data: Arc::new(Mutex::new(HashMap::new())),
            callbacks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn connection_state(&self) -> ConnectionState {
        *self.state.lock().await
    }

    /// Connect to the IEC 104 server
    pub async fn connect(&self) -> std::io::Result<()> {
        let mut state = self.state.lock().await;
        if *state == ConnectionState::Active {
            return Ok(());
        }
        *state = ConnectionState::Connecting;
        drop(state);

        let stream = tokio::time::timeout(
            self.config.connect_timeout,
            TcpStream::connect(&self.config.remote_addr),
        )
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Connection timeout"))??;

        stream.set_nodelay(true)?;
        *self.stream.lock().await = Some(stream);
        *self.state.lock().await = ConnectionState::Connected;

        self.send_startdt().await?;
        *self.state.lock().await = ConnectionState::StartDtSent;

        Ok(())
    }

    /// Disconnect from the server
    pub async fn disconnect(&self) {
        self.running.store(false, Ordering::SeqCst);
        let mut stream_guard = self.stream.lock().await;
        if let Some(stream) = stream_guard.take() {
            let _ = Self::send_stopdt_to(stream).await;
        }
        *self.state.lock().await = ConnectionState::Disconnected;
    }

    /// Start the receive loop
    pub async fn start(&self) {
        self.running.store(true, Ordering::SeqCst);
        let stream = self.stream.clone();
        let state = self.state.clone();
        let data = self.data.clone();
        let config = self.config.clone();
        let running = self.running.clone();
        let recv_seq = self.recv_seq.clone();
        let send_seq = self.send_seq.clone();
        let callbacks = self.callbacks.clone();

        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            while running.load(Ordering::SeqCst) {
                let mut stream_guard = stream.lock().await;
                let stream = match stream_guard.as_mut() {
                    Some(s) => s,
                    None => {
                        drop(stream_guard);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };

                let n = match tokio::time::timeout(
                    Duration::from_secs(5),
                    stream.read(&mut buf),
                ).await {
                    Ok(Ok(0)) => {
                        *stream_guard = None;
                        *state.lock().await = ConnectionState::Disconnected;
                        drop(stream_guard);
                        warn!("IEC 104 connection closed by remote");
                        break;
                    }
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => {
                        error!("IEC 104 read error: {}", e);
                        *stream_guard = None;
                        *state.lock().await = ConnectionState::Disconnected;
                        drop(stream_guard);
                        break;
                    }
                    Err(_) => {
                        drop(stream_guard);
                        continue;
                    }
                };

                if n > 0 {
                    let mut offset = 0;
                    while offset + 2 <= n {
                        let start_byte = buf[offset];
                        if start_byte != 0x68 {
                            offset += 1;
                            continue;
                        }
                        let frame_len = buf[offset + 1] as usize;
                        if offset + 2 + frame_len > n {
                            break;
                        }

                        let frame_data = &buf[offset..offset + 2 + frame_len];
                        Self::process_frame(
                            frame_data,
                            &state,
                            &data,
                            &recv_seq,
                            &send_seq,
                            &config,
                            &mut *stream_guard,
                            &callbacks,
                        ).await;

                        offset += 2 + frame_len;
                    }
                }
            }
        });
    }

    /// Stop the client
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Get the latest value for a given IOA
    pub async fn get_value(&self, ioa: u32) -> Option<InformationObject> {
        self.data.lock().await.get(&ioa).cloned()
    }

    /// Get all current values
    pub async fn get_all_values(&self) -> HashMap<u32, InformationObject> {
        self.data.lock().await.clone()
    }

    /// Send general interrogation command
    pub async fn send_interrogation(&self) -> std::io::Result<()> {
        let cmd_asdu = asdu::build_interrogation_command(self.config.asdu_address, 0);
        self.send_i_frame(&cmd_asdu).await
    }

    /// Send a single command (C_SC_NA_1) to control a switch
    pub async fn send_single_command(&self, ioa: u32, value: bool) -> std::io::Result<()> {
        let cmd_asdu = asdu::build_single_command(self.config.asdu_address, ioa, value, 0, false);
        self.send_i_frame(&cmd_asdu).await
    }

    /// Send a setpoint short float command (C_SE_NC_1)
    pub async fn send_setpoint(&self, ioa: u32, value: f32) -> std::io::Result<()> {
        let cmd_asdu = asdu::build_setpoint_short_float(self.config.asdu_address, ioa, value, 0, false);
        self.send_i_frame(&cmd_asdu).await
    }

    /// Register a callback for data updates
    pub async fn on_data(&self, callback: Box<dyn Fn(u32, &InformationObject) + Send + Sync>) {
        self.callbacks.lock().await.push(callback);
    }

    // ---- Internal methods ----

    async fn send_startdt(&self) -> std::io::Result<()> {
        let frame = [0x68, 0x04, STARTDT_ACT, 0x00, 0x00, 0x00];
        self.send_raw(&frame).await
    }

    async fn send_stopdt_to(mut stream: tokio::net::TcpStream) -> std::io::Result<()> {
        let frame = [0x68, 0x04, STOPDT_ACT, 0x00, 0x00, 0x00];
        stream.write_all(&frame).await
    }

    async fn send_raw(&self, data: &[u8]) -> std::io::Result<()> {
        let mut stream_guard = self.stream.lock().await;
        if let Some(stream) = stream_guard.as_mut() {
            stream.write_all(data).await?;
        }
        Ok(())
    }

    async fn send_i_frame(&self, asdu_data: &[u8]) -> std::io::Result<()> {
        let send = self.send_seq.fetch_add(1, Ordering::SeqCst);
        let recv = self.recv_seq.load(Ordering::SeqCst);

        let frame_len = 4 + asdu_data.len();
        let mut frame = Vec::with_capacity(2 + frame_len);
        frame.push(0x68);
        frame.push(frame_len as u8);
        frame.push((send << 1) as u8);
        frame.push((send >> 7) as u8);
        frame.push((recv << 1) as u8);
        frame.push((recv >> 7) as u8);
        frame.extend_from_slice(asdu_data);

        self.send_raw(&frame).await
    }

    async fn process_frame(
        frame: &[u8],
        state: &Arc<Mutex<ConnectionState>>,
        data: &Arc<Mutex<HashMap<u32, InformationObject>>>,
        recv_seq: &Arc<AtomicU16>,
        _send_seq: &Arc<AtomicU16>,
        _config: &Iec104Config,
        stream: &mut Option<tokio::net::TcpStream>,
        callbacks: &Arc<Mutex<Vec<DataCallback>>>,
    ) {
        if frame.len() < 6 { return; }

        let control1 = frame[2];

        if control1 == STARTDT_CON {
            *state.lock().await = ConnectionState::Active;
            info!("IEC 104 STARTDT confirmed — connection active");
        } else if control1 == STOPDT_CON {
            *state.lock().await = ConnectionState::Connected;
        } else if control1 == TESTFR_ACT {
            debug!("IEC 104 TESTFR received, sending CON");
            if let Some(tcp_stream) = stream.as_mut() {
                let reply = [0x68, 0x04, TESTFR_CON, 0x00, 0x00, 0x00];
                let _ = tcp_stream.write_all(&reply).await;
            }
        } else if control1 == TESTFR_CON {
            debug!("IEC 104 TESTFR confirmed");
        } else if control1 & 0x01 == I_FRAME {
            recv_seq.fetch_add(1, Ordering::SeqCst);
            if frame.len() > 6 {
                let asdu_data = &frame[6..];
                if let Some(asdu) = asdu::parse_asdu(asdu_data) {
                    debug!(
                        "IEC 104 I-frame: TI={}, COT={:?}, {} objects",
                        asdu.type_id, asdu.cot, asdu.objects.len()
                    );
                    let mut data_guard = data.lock().await;
                    let cbs = callbacks.lock().await;
                    for obj in asdu.objects {
                        let ioa = obj.ioa();
                        data_guard.insert(ioa, obj.clone());
                        for cb in cbs.iter() {
                            cb(ioa, &obj);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iec104_config_default() {
        let config = Iec104Config::default();
        assert_eq!(config.remote_addr, "127.0.0.1:2404");
        assert_eq!(config.asdu_address, 1);
        assert!(config.auto_interrogation);
    }

    #[tokio::test]
    async fn test_client_creation() {
        let config = Iec104Config {
            remote_addr: "192.168.1.100:2404".to_string(),
            asdu_address: 2,
            ..Default::default()
        };
        let client = Iec104Client::new(config);
        assert_eq!(client.connection_state().await, ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_data_storage() {
        let client = Iec104Client::new(Iec104Config::default());
        let obj = InformationObject::MeasuredShortFloat {
            ioa: 100, value: 1.045f32, quality: asdu::MeasuredQuality::from_u8(0),
        };
        client.data.lock().await.insert(100, obj.clone());
        let retrieved = client.get_value(100).await.unwrap();
        assert_eq!(retrieved.ioa(), 100);
        assert!((retrieved.as_float().unwrap() - 1.045).abs() < 0.001);
    }

    #[test]
    fn test_apci_frame_constants() {
        assert_eq!(STARTDT_ACT, 0x07);
        assert_eq!(STARTDT_CON, 0x0B);
        assert_eq!(STOPDT_ACT, 0x13);
        assert_eq!(TESTFR_ACT, 0x43);
        assert_eq!(TESTFR_CON, 0x83);
    }
}
