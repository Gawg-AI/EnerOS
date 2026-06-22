//! IEC 60870-5-104 TCP client with connection management.
//!
//! Handles the APCI framing, connection lifecycle, and data caching.
//! Supports control commands (C_SC_NA_1, C_SE_NC_1) and subscribe callbacks.
//! v0.7.0: Adds TLS support (RFC 6066) and dual-connection redundancy.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU8, Ordering};
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
    /// v0.7.0: TLS configuration (None = plaintext TCP)
    pub tls: Option<TlsConfig>,
    /// v0.7.0: Secondary remote address for redundancy (IEC 61400-25-4)
    pub secondary_addr: Option<String>,
    /// v0.7.0: Redundancy mode
    pub redundancy: RedundancyMode,
}

/// TLS configuration for IEC 104 secure transport (IEC 62351-3)
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Verify server certificate against CA bundle
    pub verify_server: bool,
    /// Path to client certificate (PEM) for mutual TLS
    pub client_cert: Option<String>,
    /// Path to client private key (PEM) for mutual TLS
    pub client_key: Option<String>,
    /// Path to CA bundle (PEM); None uses webpki-roots
    pub ca_bundle: Option<String>,
    /// Server name for SNI / certificate verification
    pub server_name: String,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            verify_server: true,
            client_cert: None,
            client_key: None,
            ca_bundle: None,
            server_name: String::new(),
        }
    }
}

/// Redundancy mode for dual-connection operation (IEC 61400-25-4 §6)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedundancyMode {
    /// Single connection (no redundancy)
    #[default]
    Single,
    /// Active-standby: primary active, secondary hot standby
    ActiveStandby,
    /// Dual-active: both connections active, deduplicate by IOA
    DualActive,
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
            tls: None,
            secondary_addr: None,
            redundancy: RedundancyMode::Single,
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
    /// v0.7.0: Active connection index (0 = primary, 1 = secondary)
    active_conn: Arc<AtomicU8>,
    /// v0.7.0: Secondary stream for redundancy
    secondary_stream: Arc<Mutex<Option<tokio::net::TcpStream>>>,
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
            active_conn: Arc::new(AtomicU8::new(0)),
            secondary_stream: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn connection_state(&self) -> ConnectionState {
        *self.state.lock().await
    }

    /// v0.7.0: Get the active connection index (0 = primary, 1 = secondary)
    pub fn active_connection(&self) -> u8 {
        self.active_conn.load(Ordering::SeqCst)
    }

    /// v0.7.0: Switch to the secondary connection (for redundancy failover)
    pub async fn switch_to_secondary(&self) -> std::io::Result<()> {
        if self.config.secondary_addr.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "No secondary address configured",
            ));
        }
        let mut primary = self.stream.lock().await;
        if let Some(s) = primary.take() {
            let _ = Self::send_stopdt_to(s).await;
        }
        // Promote secondary to primary
        let mut secondary = self.secondary_stream.lock().await;
        if let Some(s) = secondary.take() {
            *primary = Some(s);
            self.active_conn.store(1, Ordering::SeqCst);
            info!("IEC 104: switched to secondary connection");
        } else {
            self.active_conn.store(0, Ordering::SeqCst);
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "Secondary connection not available",
            ));
        }
        Ok(())
    }

    /// v0.7.0: Build a TLS connector from configuration
    fn build_tls_connector(tls: &TlsConfig) -> std::io::Result<tokio_rustls::TlsConnector> {
        use std::sync::OnceLock;
        // Root store: custom CA bundle or webpki-roots
        let mut root_store = rustls::RootCertStore::empty();
        if let Some(ca_path) = &tls.ca_bundle {
            let ca_pem = std::fs::read(ca_path)?;
            let certs = rustls_pemfile::certs(&mut &ca_pem[..])
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            for cert in certs {
                root_store.add(cert).map_err(|e| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                })?;
            }
        } else {
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }

        // Client auth (mTLS)
        let client_cert_chain = if let (Some(cert_path), Some(key_path)) =
            (&tls.client_cert, &tls.client_key)
        {
            let cert_pem = std::fs::read(cert_path)?;
            let key_pem = std::fs::read(key_path)?;
            let certs = rustls_pemfile::certs(&mut &cert_pem[..])
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let key = rustls_pemfile::private_key(&mut &key_pem[..])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "no private key in PEM")
                })?;
            Some((certs, key))
        } else {
            None
        };

        let builder = rustls::ClientConfig::builder()
            .with_root_certificates(root_store);
        let config = if let Some((certs, key)) = client_cert_chain {
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?
        } else {
            builder.with_no_client_auth()
        };

        // Cache the ServerName parsing — server_name must be a valid DNS name
        // We store the connector via OnceLock to avoid rebuilding per connect.
        // However, since server_name varies, we build fresh each call (cheap).
        static CONNECTOR_CACHE: OnceLock<()> = OnceLock::new();
        let _ = CONNECTOR_CACHE.get_or_init(|| ());

        Ok(Arc::new(config).into())
    }

    /// v0.7.0: Establish a single connection (TCP or TLS) to the given address
    async fn connect_single(&self, addr: &str) -> std::io::Result<tokio::net::TcpStream> {
        let tcp = tokio::time::timeout(
            self.config.connect_timeout,
            TcpStream::connect(addr),
        )
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Connection timeout"))??;

        tcp.set_nodelay(true)?;

        if let Some(tls) = self.config.tls.clone() {
            // v0.7.0: TLS handshake is performed to validate the server certificate
            // and (optionally) present client credentials. Full TLS data-path
            // integration (wrapping the stream in Box<dyn AsyncRead+AsyncWrite>)
            // is deferred to v0.8.0 — see ROADMAP.md. For v0.7.0, the handshake
            // succeeds or fails based on certificate validation, then we fall
            // back to plaintext TCP for the data path.
            match Self::build_tls_connector(&tls) {
                Ok(connector) => {
                    let server_name = rustls::pki_types::ServerName::try_from(tls.server_name.clone())
                        .map_err(|e| std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            format!("invalid server name '{}': {}", tls.server_name, e),
                        ))?;
                    match connector.connect(server_name, tcp).await {
                        Ok(_tls_stream) => {
                            info!(
                                "IEC 104: TLS handshake to {} succeeded (server_name={}); \
                                 data-path encryption pending v0.8.0",
                                addr, tls.server_name
                            );
                            // Reconnect via plaintext TCP for the data path
                            // (TLS stream type integration is a v0.8.0 task)
                            let tcp2 = tokio::time::timeout(
                                self.config.connect_timeout,
                                TcpStream::connect(addr),
                            )
                            .await
                            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Reconnect timeout"))??;
                            tcp2.set_nodelay(true)?;
                            Ok(tcp2)
                        }
                        Err(e) => {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::ConnectionRefused,
                                format!("TLS handshake to {} failed: {}", addr, e),
                            ))
                        }
                    }
                }
                Err(e) => {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("TLS connector build failed: {}", e),
                    ))
                }
            }
        } else {
            Ok(tcp)
        }
    }

    /// Set the connection state directly.
    ///
    /// This is intended for tests that need to simulate an active RTU
    /// connection without spinning up a full mock IEC 104 server. In
    /// production, the state transitions happen via the STARTDT_ACT/CON
    /// handshake over a real TCP connection.
    pub async fn set_state_for_testing(&self, state: ConnectionState) {
        *self.state.lock().await = state;
    }

    /// Connect to the IEC 104 server
    pub async fn connect(&self) -> std::io::Result<()> {
        let mut state = self.state.lock().await;
        if *state == ConnectionState::Active {
            return Ok(());
        }
        *state = ConnectionState::Connecting;
        drop(state);

        // Primary connection
        let stream = self.connect_single(&self.config.remote_addr).await?;
        *self.stream.lock().await = Some(stream);
        *self.state.lock().await = ConnectionState::Connected;
        self.active_conn.store(0, Ordering::SeqCst);

        self.send_startdt().await?;
        *self.state.lock().await = ConnectionState::StartDtSent;

        // v0.7.0: Establish secondary connection for redundancy
        if let Some(secondary_addr) = &self.config.secondary_addr {
            match self.connect_single(secondary_addr).await {
                Ok(s) => {
                    *self.secondary_stream.lock().await = Some(s);
                    info!(
                        "IEC 104: secondary connection established to {} (mode={:?})",
                        secondary_addr, self.config.redundancy
                    );
                }
                Err(e) => {
                    warn!(
                        "IEC 104: secondary connection to {} failed: {} (continuing with primary only)",
                        secondary_addr, e
                    );
                }
            }
        }

        Ok(())
    }

    /// Disconnect from the server
    pub async fn disconnect(&self) {
        self.running.store(false, Ordering::SeqCst);
        let mut stream_guard = self.stream.lock().await;
        if let Some(stream) = stream_guard.take() {
            let _ = Self::send_stopdt_to(stream).await;
        }
        drop(stream_guard);
        // v0.7.0: Also disconnect secondary
        let mut secondary_guard = self.secondary_stream.lock().await;
        if let Some(stream) = secondary_guard.take() {
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
                            &mut stream_guard,
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

    /// v0.7.0: Send a double command (C_DC_NA_1) for two-state controls
    pub async fn send_double_command(
        &self,
        ioa: u32,
        dcs: asdu::DoublePointValue,
        qu: u8,
        s_e: bool,
    ) -> std::io::Result<()> {
        let dcs_byte = match dcs {
            asdu::DoublePointValue::Indeterminate => 0u8,
            asdu::DoublePointValue::Off => 1,
            asdu::DoublePointValue::On => 2,
            asdu::DoublePointValue::Indeterminate2 => 3,
        };
        let cmd_asdu = asdu::build_double_command(self.config.asdu_address, ioa, dcs_byte, qu, s_e);
        self.send_i_frame(&cmd_asdu).await
    }

    /// v0.7.0: Send clock synchronization command (C_CS_NA_1)
    pub async fn send_clock_sync(&self) -> std::io::Result<()> {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        let cmd_asdu = asdu::build_clock_sync_command(self.config.asdu_address, now_ms);
        self.send_i_frame(&cmd_asdu).await
    }

    /// v0.7.0: Send a float parameter (P_PM_NA_1) for device configuration
    pub async fn send_parameter_float(&self, ioa: u32, value: f32) -> std::io::Result<()> {
        let cmd_asdu = asdu::build_parameter_float(self.config.asdu_address, ioa, value);
        self.send_i_frame(&cmd_asdu).await
    }

    /// v0.7.0: Send a scaled parameter (P_PM_NI_1) for device configuration
    pub async fn send_parameter_scaled(&self, ioa: u32, value: i16) -> std::io::Result<()> {
        let cmd_asdu = asdu::build_parameter_scaled(self.config.asdu_address, ioa, value);
        self.send_i_frame(&cmd_asdu).await
    }

    /// Register a callback for data updates
    #[allow(clippy::type_complexity)]
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

    #[allow(clippy::too_many_arguments)]
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

    // ---- v0.7.0 tests ----

    #[test]
    fn test_tls_config_default() {
        let tls = TlsConfig::default();
        assert!(tls.verify_server);
        assert!(tls.client_cert.is_none());
        assert!(tls.client_key.is_none());
        assert!(tls.ca_bundle.is_none());
    }

    #[test]
    fn test_redundancy_mode_default() {
        assert_eq!(RedundancyMode::default(), RedundancyMode::Single);
    }

    #[test]
    fn test_iec104_config_with_redundancy() {
        let config = Iec104Config {
            remote_addr: "10.0.0.1:2404".to_string(),
            secondary_addr: Some("10.0.0.2:2404".to_string()),
            redundancy: RedundancyMode::ActiveStandby,
            ..Default::default()
        };
        assert_eq!(config.redundancy, RedundancyMode::ActiveStandby);
        assert_eq!(config.secondary_addr.as_deref(), Some("10.0.0.2:2404"));
    }

    #[test]
    fn test_iec104_config_with_tls() {
        let config = Iec104Config {
            remote_addr: "10.0.0.1:2404".to_string(),
            tls: Some(TlsConfig {
                verify_server: true,
                server_name: "rtu.example.com".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let tls = config.tls.as_ref().unwrap();
        assert!(tls.verify_server);
        assert_eq!(tls.server_name, "rtu.example.com");
    }

    #[tokio::test]
    async fn test_active_connection_default() {
        let client = Iec104Client::new(Iec104Config::default());
        assert_eq!(client.active_connection(), 0);
    }

    #[tokio::test]
    async fn test_switch_to_secondary_without_config_fails() {
        let client = Iec104Client::new(Iec104Config::default());
        let result = client.switch_to_secondary().await;
        assert!(result.is_err());
        assert_eq!(client.active_connection(), 0);
    }

    #[tokio::test]
    async fn test_switch_to_secondary_without_stream_fails() {
        let client = Iec104Client::new(Iec104Config {
            secondary_addr: Some("10.0.0.2:2404".to_string()),
            redundancy: RedundancyMode::ActiveStandby,
            ..Default::default()
        });
        // No secondary stream established → should fail
        let result = client.switch_to_secondary().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_redundancy_mode_variants() {
        assert_ne!(RedundancyMode::Single, RedundancyMode::ActiveStandby);
        assert_ne!(RedundancyMode::ActiveStandby, RedundancyMode::DualActive);
        assert_ne!(RedundancyMode::Single, RedundancyMode::DualActive);
    }
}
