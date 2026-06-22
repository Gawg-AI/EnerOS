use std::net::SocketAddr;
use std::sync::Arc;

use eneros_runtime::constraint::ConstraintEngine;
use eneros_runtime::eventbus::EventBus;
use eneros_runtime::gateway::decision_pipeline::ConstrainedDecisionPipeline;
use eneros_runtime::network::PowerNetwork;
use eneros_powerflow::PowerFlowSolver;
use eneros_runtime::scada::{ScadaCollector, SnapshotBuilder};
use eneros_runtime::timeseries::TimeSeriesEngine;
use eneros_topology::TopologyEngine;

use eneros_runtime::agent::{AgentOrchestrator, DataDrivenAgentLoop};

use crate::app::{self, AppState};

/// TLS configuration for the API server (v0.7.0 — deferred from v0.6.0 S1).
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Path to the PEM-encoded certificate file.
    pub cert_path: String,
    /// Path to the PEM-encoded private key file.
    pub key_path: String,
}

/// 从 PEM 格式的证书文件和私钥文件加载 rustls 服务端配置。
///
/// 该函数执行以下步骤：
/// 1. 读取并解析 PEM 编码的证书链（支持多张证书，第一张为服务器证书，
///    后续为中间 CA 证书）。
/// 2. 读取并解析 PEM 编码的私钥（支持 PKCS#8、PKCS#1 和 SEC1 编码）。
/// 3. 构建 `rustls::ServerConfig`，禁用客户端证书认证（单向 TLS）。
///
/// # 参数
/// - `cert_path`: PEM 证书文件路径
/// - `key_path`: PEM 私钥文件路径
///
/// # 错误
/// - 证书文件无法打开或解析失败
/// - 私钥文件无法打开或解析失败
/// - 私钥文件中未找到任何私钥
/// - 证书与私钥不匹配或 rustls 配置构建失败
///
/// # 安全说明
/// 此函数用于工业级电力系统 TLS 终结，使用 rustls（内存安全 TLS 实现），
/// 不依赖 OpenSSL。证书和私钥在加载后由 rustls 持有，私钥不会被打日志。
pub fn load_rustls_server_config(
    cert_path: &str,
    key_path: &str,
) -> anyhow::Result<rustls::ServerConfig> {
    // 读取并解析证书链
    let cert_file = std::fs::File::open(cert_path)
        .map_err(|e| anyhow::anyhow!("failed to open TLS cert '{}': {}", cert_path, e))?;
    let mut reader = std::io::BufReader::new(cert_file);
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("failed to parse TLS cert '{}': {}", cert_path, e))?;
    if certs.is_empty() {
        return Err(anyhow::anyhow!(
            "no certificates found in TLS cert file '{}'",
            cert_path
        ));
    }

    // 读取并解析私钥
    let key_file = std::fs::File::open(key_path)
        .map_err(|e| anyhow::anyhow!("failed to open TLS key '{}': {}", key_path, e))?;
    let mut key_reader = std::io::BufReader::new(key_file);
    let key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| anyhow::anyhow!("failed to parse TLS key '{}': {}", key_path, e))?
        .ok_or_else(|| {
            anyhow::anyhow!("no private key found in TLS key file '{}'", key_path)
        })?;

    // 构建 rustls 服务端配置（单向 TLS，不要求客户端证书）
    // 显式指定 ring 作为 CryptoProvider，避免依赖进程级全局状态
    // （rustls 0.23 要求显式选择 CryptoProvider）
    rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| anyhow::anyhow!("failed to build TLS server config: {}", e))
}

/// API server for EnerOS
pub struct ApiServer {
    state: AppState,
    addr: SocketAddr,
    /// Optional TLS configuration. When set, the server uses HTTPS.
    tls: Option<TlsConfig>,
}

impl ApiServer {
    /// Create a new API server with default (empty) AppState
    pub fn new(host: &str, port: u16) -> Self {
        let addr = format!("{}:{}", host, port)
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], port)));
        Self {
            state: AppState::new(),
            addr,
            tls: None,
        }
    }

    /// Create with a custom AppState and address
    pub fn with_state(state: AppState, addr: SocketAddr) -> Self {
        Self {
            state,
            addr,
            tls: None,
        }
    }

    /// Enable TLS (v0.7.0). When set, the server uses HTTPS.
    pub fn with_tls(mut self, tls: Option<TlsConfig>) -> Self {
        self.tls = tls;
        self
    }

    /// Start the axum HTTP server. If TLS is configured, starts an HTTPS
    /// server using `axum_server::bind_rustls`; otherwise starts a plaintext
    /// HTTP server.
    pub async fn start(&self) -> anyhow::Result<()> {
        let app = app::create_router(self.state.clone());

        if let Some(ref tls) = self.tls {
            tracing::info!(
                addr = %self.addr,
                cert = %tls.cert_path,
                "EnerOS API server listening (HTTPS)"
            );
            // 加载证书和私钥，构建 rustls 服务端配置
            let config = load_rustls_server_config(&tls.cert_path, &tls.key_path)?;

            // 使用 axum_server 提供 TLS 支持（axum 0.7 标准模式）
            let rustls_config =
                axum_server::tls_rustls::RustlsConfig::from_config(std::sync::Arc::new(config));
            axum_server::bind_rustls(self.addr, rustls_config)
                .serve(app.into_make_service())
                .await?;
        } else {
            tracing::info!(addr = %self.addr, "EnerOS API server listening (HTTP)");
            let listener = tokio::net::TcpListener::bind(self.addr).await?;
            axum::serve(listener, app).await?;
        }
        Ok(())
    }

    // ---- Builder methods for injecting dependencies ----

    /// Inject a TopologyEngine
    pub fn with_topology_engine(mut self, engine: Arc<TopologyEngine>) -> Self {
        self.state.topology_engine = Some(engine);
        self
    }

    /// Inject a PowerFlowSolver
    pub fn with_powerflow_solver(mut self, solver: Arc<PowerFlowSolver>) -> Self {
        self.state.powerflow_solver = Some(solver);
        self
    }

    /// Inject a ConstraintEngine
    pub fn with_constraint_engine(mut self, engine: Arc<ConstraintEngine>) -> Self {
        self.state.constraint_engine = Some(engine);
        self
    }

    /// Inject a PowerNetwork
    pub fn with_network(mut self, network: Arc<PowerNetwork>) -> Self {
        self.state.network = Some(network);
        self
    }

    /// Inject a TimeSeriesEngine
    pub fn with_ts_engine(mut self, engine: Arc<TimeSeriesEngine>) -> Self {
        self.state.ts_engine = Some(engine);
        self
    }

    /// Inject a ScadaCollector
    pub fn with_scada_collector(mut self, collector: Arc<ScadaCollector>) -> Self {
        self.state.scada_collector = Some(collector);
        self
    }

    /// Inject an EventBus
    pub fn with_event_bus(mut self, bus: Arc<EventBus>) -> Self {
        self.state.event_bus = Some(bus);
        self
    }

    /// Inject an AgentOrchestrator
    pub fn with_agent_orchestrator(mut self, orchestrator: Arc<AgentOrchestrator>) -> Self {
        self.state.agent_orchestrator = Some(orchestrator);
        self
    }

    /// Inject a DataPipeline
    pub fn with_data_pipeline(mut self, pipeline: Arc<eneros_runtime::scada::DataPipeline>) -> Self {
        self.state.data_pipeline = Some(pipeline);
        self
    }

    /// Inject a SnapshotBuilder
    pub fn with_snapshot_builder(mut self, builder: Arc<SnapshotBuilder>) -> Self {
        self.state.snapshot_builder = Some(builder);
        self
    }

    /// Inject a DataDrivenAgentLoop
    pub fn with_data_driven_loop(mut self, dd_loop: Arc<DataDrivenAgentLoop>) -> Self {
        self.state.data_driven_loop = Some(dd_loop);
        self
    }

    pub fn with_decision_pipeline(mut self, pipeline: Arc<ConstrainedDecisionPipeline>) -> Self {
        self.state.decision_pipeline = Some(pipeline);
        self
    }

    /// Get a reference to the AppState
    pub fn state(&self) -> &AppState {
        &self.state
    }
}

impl Default for ApiServer {
    fn default() -> Self {
        Self::new("0.0.0.0", 8080)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::generate_simple_self_signed;
    use std::io::Write;

    /// 使用 rcgen 生成自签名测试证书，返回 (cert_pem, key_pem)。
    ///
    /// 该证书仅用于单元测试，不用于生产环境。证书的 SAN 包含 "localhost"，
    /// 便于在本地测试中通过 HTTPS 客户端验证。
    fn generate_test_cert_pem() -> (String, String) {
        let cert = generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("failed to generate self-signed test certificate");
        let cert_pem = cert
            .serialize_pem()
            .expect("failed to serialize test cert to PEM");
        let key_pem = cert.serialize_private_key_pem();
        (cert_pem, key_pem)
    }

    /// 将 PEM 内容写入临时文件，返回文件路径。
    fn write_temp_pem(content: &str, prefix: &str) -> std::path::PathBuf {
        let mut tmp = tempfile::NamedTempFile::with_prefix(prefix)
            .expect("failed to create temp file");
        tmp.write_all(content.as_bytes())
            .expect("failed to write PEM to temp file");
        // 保持文件存在：将 NamedTempFile 转为持久路径
        let path = tmp.path().to_path_buf();
        tmp.keep().expect("failed to keep temp file");
        path
    }

    /// 测试 TlsConfig 结构体的构建与字段访问。
    #[test]
    fn test_tls_config_construction() {
        let tls = TlsConfig {
            cert_path: "/etc/eneros/tls/server.crt".to_string(),
            key_path: "/etc/eneros/tls/server.key".to_string(),
        };
        assert_eq!(tls.cert_path, "/etc/eneros/tls/server.crt");
        assert_eq!(tls.key_path, "/etc/eneros/tls/server.key");
    }

    /// 测试 TlsConfig 的 Clone 派生。
    #[test]
    fn test_tls_config_clone() {
        let tls = TlsConfig {
            cert_path: "/path/cert.pem".to_string(),
            key_path: "/path/key.pem".to_string(),
        };
        let cloned = tls.clone();
        assert_eq!(tls.cert_path, cloned.cert_path);
        assert_eq!(tls.key_path, cloned.key_path);
    }

    /// 测试 ApiServer::with_tls(None) — 默认 HTTP 模式。
    /// 服务器不应携带 TLS 配置。
    #[test]
    fn test_api_server_without_tls() {
        let server = ApiServer::new("127.0.0.1", 8080);
        assert!(server.tls.is_none(), "default server should not have TLS");
    }

    /// 测试 ApiServer::with_tls(Some(...)) — HTTPS 模式选择。
    /// 设置 TLS 配置后，服务器应携带证书路径。
    #[test]
    fn test_api_server_with_tls() {
        let tls = TlsConfig {
            cert_path: "/etc/eneros/tls/server.crt".to_string(),
            key_path: "/etc/eneros/tls/server.key".to_string(),
        };
        let server = ApiServer::new("127.0.0.1", 8443).with_tls(Some(tls));
        assert!(server.tls.is_some(), "server should have TLS configured");
        let tls = server.tls.unwrap();
        assert_eq!(tls.cert_path, "/etc/eneros/tls/server.crt");
        assert_eq!(tls.key_path, "/etc/eneros/tls/server.key");
    }

    /// 测试 ApiServer::with_tls 的链式调用可以覆盖之前的 TLS 配置。
    #[test]
    fn test_api_server_with_tls_override() {
        let tls1 = TlsConfig {
            cert_path: "/path/cert1.pem".to_string(),
            key_path: "/path/key1.pem".to_string(),
        };
        let tls2 = TlsConfig {
            cert_path: "/path/cert2.pem".to_string(),
            key_path: "/path/key2.pem".to_string(),
        };
        let server = ApiServer::new("127.0.0.1", 8443)
            .with_tls(Some(tls1))
            .with_tls(Some(tls2));
        let tls = server.tls.expect("TLS should be configured");
        assert_eq!(tls.cert_path, "/path/cert2.pem");
        assert_eq!(tls.key_path, "/path/key2.pem");
    }

    /// 测试 ApiServer::with_tls(None) 可以清除之前的 TLS 配置，
    /// 回退到 HTTP 模式。
    #[test]
    fn test_api_server_with_tls_clear() {
        let tls = TlsConfig {
            cert_path: "/path/cert.pem".to_string(),
            key_path: "/path/key.pem".to_string(),
        };
        let server = ApiServer::new("127.0.0.1", 8443)
            .with_tls(Some(tls))
            .with_tls(None);
        assert!(server.tls.is_none(), "TLS should be cleared");
    }

    /// 测试 load_rustls_server_config 成功加载自签名证书。
    ///
    /// 使用 rcgen 生成自签名证书，写入临时文件，然后通过
    /// load_rustls_server_config 加载。验证返回的 ServerConfig 可用。
    #[test]
    fn test_load_rustls_server_config_success() {
        let (cert_pem, key_pem) = generate_test_cert_pem();
        let cert_path = write_temp_pem(&cert_pem, "test_cert");
        let key_path = write_temp_pem(&key_pem, "test_key");

        let config = load_rustls_server_config(
            cert_path.to_str().unwrap(),
            key_path.to_str().unwrap(),
        );
        assert!(config.is_ok(), "should successfully load TLS config");

        // 清理临时文件
        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
    }

    /// 测试 load_rustls_server_config 在证书文件不存在时返回清晰错误。
    #[test]
    fn test_load_rustls_server_config_missing_cert_file() {
        let (_, key_pem) = generate_test_cert_pem();
        let key_path = write_temp_pem(&key_pem, "test_key");

        let result = load_rustls_server_config("/nonexistent/cert.pem", key_path.to_str().unwrap());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("failed to open TLS cert"),
            "error should mention cert file open failure, got: {}",
            err
        );

        let _ = std::fs::remove_file(&key_path);
    }

    /// 测试 load_rustls_server_config 在私钥文件不存在时返回清晰错误。
    #[test]
    fn test_load_rustls_server_config_missing_key_file() {
        let (cert_pem, _) = generate_test_cert_pem();
        let cert_path = write_temp_pem(&cert_pem, "test_cert");

        let result = load_rustls_server_config(
            cert_path.to_str().unwrap(),
            "/nonexistent/key.pem",
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("failed to open TLS key"),
            "error should mention key file open failure, got: {}",
            err
        );

        let _ = std::fs::remove_file(&cert_path);
    }

    /// 测试 load_rustls_server_config 在证书文件为空时返回清晰错误。
    #[test]
    fn test_load_rustls_server_config_empty_cert_file() {
        let (_, key_pem) = generate_test_cert_pem();
        let cert_path = write_temp_pem("", "empty_cert");
        let key_path = write_temp_pem(&key_pem, "test_key");

        let result = load_rustls_server_config(
            cert_path.to_str().unwrap(),
            key_path.to_str().unwrap(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no certificates found"),
            "error should mention no certificates, got: {}",
            err
        );

        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
    }

    /// 测试 load_rustls_server_config 在私钥文件为空时返回清晰错误。
    #[test]
    fn test_load_rustls_server_config_empty_key_file() {
        let (cert_pem, _) = generate_test_cert_pem();
        let cert_path = write_temp_pem(&cert_pem, "test_cert");
        let key_path = write_temp_pem("", "empty_key");

        let result = load_rustls_server_config(
            cert_path.to_str().unwrap(),
            key_path.to_str().unwrap(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no private key found"),
            "error should mention no private key, got: {}",
            err
        );

        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
    }

    /// 测试 load_rustls_server_config 在证书文件包含无效 PEM 时返回错误。
    #[test]
    fn test_load_rustls_server_config_invalid_cert_pem() {
        let (_, key_pem) = generate_test_cert_pem();
        let cert_path = write_temp_pem("not a valid PEM certificate", "invalid_cert");
        let key_path = write_temp_pem(&key_pem, "test_key");

        let result = load_rustls_server_config(
            cert_path.to_str().unwrap(),
            key_path.to_str().unwrap(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no certificates found"),
            "error should mention no certificates for invalid PEM, got: {}",
            err
        );

        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
    }

    /// 测试 load_rustls_server_config 在私钥文件包含无效 PEM 时返回错误。
    #[test]
    fn test_load_rustls_server_config_invalid_key_pem() {
        let (cert_pem, _) = generate_test_cert_pem();
        let cert_path = write_temp_pem(&cert_pem, "test_cert");
        let key_path = write_temp_pem("not a valid PEM private key", "invalid_key");

        let result = load_rustls_server_config(
            cert_path.to_str().unwrap(),
            key_path.to_str().unwrap(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("no private key found"),
            "error should mention no private key for invalid PEM, got: {}",
            err
        );

        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
    }

    /// 测试 HTTPS 服务器实际启动并接受 TLS 连接。
    ///
    /// 这是一个集成测试：生成自签名证书，启动 HTTPS 服务器，
    /// 然后使用 tokio-rustls 客户端连接并验证 TLS 握手成功。
    /// 服务器在收到第一个连接后立即关闭。
    #[tokio::test]
    async fn test_https_server_starts_and_accepts_tls_connection() {
        use rustls::client::danger::HandshakeSignatureValid;
        use std::sync::Arc;
        use tokio_rustls::TlsConnector;

        // 生成自签名测试证书
        let (cert_pem, key_pem) = generate_test_cert_pem();
        let cert_path = write_temp_pem(&cert_pem, "https_test_cert");
        let key_path = write_temp_pem(&key_pem, "https_test_key");

        // 获取一个空闲端口，释放后由 ApiServer 重新绑定
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();
        drop(listener);

        let server = ApiServer::with_state(AppState::new(), bound_addr).with_tls(Some(TlsConfig {
            cert_path: cert_path.to_string_lossy().to_string(),
            key_path: key_path.to_string_lossy().to_string(),
        }));

        // 在后台启动服务器
        let server_handle = tokio::spawn(async move {
            let _ = server.start().await;
        });

        // 给服务器一点时间启动
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // 构建一个跳过证书验证的 rustls 客户端配置（自签名证书）
        #[derive(Debug)]
        struct NoVerifier;
        impl rustls::client::danger::ServerCertVerifier for NoVerifier {
            fn verify_server_cert(
                &self,
                _end_entity: &rustls::pki_types::CertificateDer<'_>,
                _intermediates: &[rustls::pki_types::CertificateDer<'_>],
                _server_name: &rustls::pki_types::ServerName<'_>,
                _ocsp_response: &[u8],
                _now: rustls::pki_types::UnixTime,
            ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            fn verify_tls12_signature(
                &self,
                _message: &[u8],
                _cert: &rustls::pki_types::CertificateDer<'_>,
                _dss: &rustls::DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }
            fn verify_tls13_signature(
                &self,
                _message: &[u8],
                _cert: &rustls::pki_types::CertificateDer<'_>,
                _dss: &rustls::DigitallySignedStruct,
            ) -> Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }
            fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
                vec![
                    rustls::SignatureScheme::RSA_PKCS1_SHA256,
                    rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
                    rustls::SignatureScheme::ED25519,
                    rustls::SignatureScheme::RSA_PSS_SHA256,
                ]
            }
        }

        let client_config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("safe default protocol versions should be available with ring")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerifier))
        .with_no_client_auth();

        // 连接到服务器的 TLS 端口并执行 TLS 握手
        let tcp_stream = tokio::net::TcpStream::connect(bound_addr).await;
        assert!(tcp_stream.is_ok(), "should connect to server TCP port");
        let tcp_stream = tcp_stream.unwrap();

        let connector = TlsConnector::from(Arc::new(client_config));
        let server_name = rustls::pki_types::ServerName::try_from("localhost")
            .expect("invalid server name");
        let tls_result = connector.connect(server_name, tcp_stream).await;

        // TLS 握手成功即表示 HTTPS 服务器正常工作
        assert!(
            tls_result.is_ok(),
            "TLS handshake should succeed, got error: {:?}",
            tls_result.err()
        );

        // 清理
        server_handle.abort();
        let _ = std::fs::remove_file(&cert_path);
        let _ = std::fs::remove_file(&key_path);
    }
}
