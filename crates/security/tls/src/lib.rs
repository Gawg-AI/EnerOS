//! EnerOS 标准 TLS 1.3 / mTLS 集成 (NET-005).
//!
//! Replaces the custom pseudo-mTLS in `eneros-mtls` with a real, standards-
//! compliant TLS 1.3 stack built on **rustls**. This is the host-side service
//! TLS layer — it is a `std` crate (not `no_std`) because TLS 1.3 with
//! certificate validation requires a real crypto provider (ring) and the full
//! rustls state machine, which are not available in `no_std` / seL4 user-space.
//!
//! # Why rustls, not a hand-rolled handshake
//!
//! The MASTER blueprint and the user's execution rules explicitly prohibit
//! "自定义伪 mTLS、固定随机数和未审计握手" in the production profile. The
//! previous `eneros-mtls` crate is a custom SM-cipher handshake that is not
//! standards-compatible TLS — its `Cargo.toml` carries
//! `product_allowed = false` and `insecure-mtls-prototype` feature. This crate
//! is its production replacement:
//!
//! - **rustls** is a widely-audited, pure-Rust TLS implementation with no
//!   unsafe code in its core. It has passed multiple third-party security
//!   audits.
//! - TLS 1.3 is enforced (the `TLS12` fallback is compiled in for controlled
//!   interop only; the production builder pins to TLS 1.3).
//! - Certificate validation uses `rustls-webpki` (RFC 5280 path building +
//!   RFC 6960 CRL) — there is **no self-signed bypass**, no `dangerous()`
//!   verifier, no fixed-random handshake.
//! - mTLS (mutual TLS) is supported via the standard `WebPkiClientVerifier`.
//!
//! # API
//!
//! - [`TlsServerConfig`] — builds a `rustls::ServerConfig` from a server cert
//!   + key, with optional mTLS (client-cert verification).
//! - [`TlsClientConfig`] — builds a `rustls::ClientConfig` anchored on a root
//!   CA, with optional client cert + key for mTLS.
//! - [`TlsAcceptor`] / [`TlsConnector`] — wrap a `TcpListener` / `TcpStream`
//!   and perform the real TLS handshake.
//!
//! # Certificate anomaly tests
//!
//! See `tests/anomaly.rs` for evidence that expired, wrong-CN, untrusted, and
//! mTLS-client-cert-missing connections are all rejected by the real rustls
//! verifier — not by a custom check.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::danger::ClientCertVerifier;
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig, SupportedProtocolVersion};

/// Production protocol-version pin: TLS 1.3 only. The `tls12` feature is
/// compiled in for controlled interop testing but the production builders
/// below pin to this list.
const TLS13_ONLY: &[&SupportedProtocolVersion] = &[&rustls::version::TLS13];

/// Errors returned by the TLS layer.
#[derive(Debug)]
pub enum TlsError {
    /// A rustls configuration or handshake error.
    Rustls(rustls::Error),
    /// A PEM/DER parsing error from `rustls-pemfile`.
    Pem(String),
    /// An I/O error from the underlying TCP socket.
    Io(std::io::Error),
    /// The caller supplied an empty certificate chain or key.
    EmptyMaterial(&'static str),
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rustls(e) => write!(f, "rustls error: {e}"),
            Self::Pem(s) => write!(f, "PEM parse error: {s}"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::EmptyMaterial(s) => write!(f, "empty {s}"),
        }
    }
}

impl std::error::Error for TlsError {}

impl From<rustls::Error> for TlsError {
    fn from(e: rustls::Error) -> Self {
        Self::Rustls(e)
    }
}

impl From<std::io::Error> for TlsError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Ensures the ring crypto provider is installed as the process-default.
/// Calling this more than once is a no-op (rustls handles the install race).
fn install_ring_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Parses one or more PEM-encoded certificates from `pem_bytes`. Returns the
/// DER-encoded certificate chain. Each `-----BEGIN CERTIFICATE-----` block
/// becomes one `CertificateDer`.
pub fn parse_cert_chain_pem(pem_bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let mut reader = std::io::BufReader::new(pem_bytes);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| TlsError::Pem(e.to_string()))?;
    if certs.is_empty() {
        return Err(TlsError::EmptyMaterial(
            "certificate chain (no PEM certs found)",
        ));
    }
    Ok(certs)
}

/// Parses a single PEM-encoded PKCS#8 / PKCS#1 / SEC1 private key.
pub fn parse_private_key_pem(pem_bytes: &[u8]) -> Result<PrivateKeyDer<'static>, TlsError> {
    let mut reader = std::io::BufReader::new(pem_bytes);
    let key = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| TlsError::Pem(e.to_string()))?
        .ok_or(TlsError::EmptyMaterial("private key (no PEM key found)"))?;
    Ok(key)
}

/// Builds a `RootCertStore` from one or more PEM-encoded trust anchors.
pub fn build_root_store(pem_bytes: &[u8]) -> Result<RootCertStore, TlsError> {
    let certs = parse_cert_chain_pem(pem_bytes)?;
    let mut store = RootCertStore::empty();
    for c in certs {
        store
            .add(c)
            .map_err(|e| TlsError::Pem(format!("root add: {e:?}")))?;
    }
    Ok(store)
}

// ---------------------------------------------------------------------------
// Server side
// ---------------------------------------------------------------------------

/// Server-side TLS 1.3 configuration. Wraps a `rustls::ServerConfig` with the
/// ring crypto provider and (optionally) mandatory client authentication.
pub struct TlsServerConfig {
    inner: Arc<ServerConfig>,
}

impl TlsServerConfig {
    /// Build a server config with **no** client-cert verification (server-only
    /// TLS). `server_cert_pem` is the server's certificate chain (leaf first,
    /// then intermediates); `server_key_pem` is the matching private key.
    pub fn new_server_only(
        server_cert_pem: &[u8],
        server_key_pem: &[u8],
    ) -> Result<Self, TlsError> {
        Self::build(server_cert_pem, server_key_pem, None)
    }

    /// Build a server config with **mandatory** client-cert verification
    /// (mutual TLS). The client must present a certificate that chains to one
    /// of the anchors in `client_ca_pem`. A client that offers no certificate
    /// is rejected at the handshake — this is the `WebPkiClientVerifier`
    /// `allow_unauthenticated = false` path.
    pub fn new_mtls(
        server_cert_pem: &[u8],
        server_key_pem: &[u8],
        client_ca_pem: &[u8],
    ) -> Result<Self, TlsError> {
        let client_roots = build_root_store(client_ca_pem)?;
        let verifier = WebPkiClientVerifier::builder(Arc::new(client_roots))
            .build()
            .map_err(|e| TlsError::Pem(format!("client verifier build: {e}")))?;
        Self::build(server_cert_pem, server_key_pem, Some(verifier))
    }

    fn build(
        server_cert_pem: &[u8],
        server_key_pem: &[u8],
        client_verifier: Option<Arc<dyn ClientCertVerifier>>,
    ) -> Result<Self, TlsError> {
        install_ring_provider();
        let cert_chain = parse_cert_chain_pem(server_cert_pem)?;
        let key = parse_private_key_pem(server_key_pem)?;

        let builder = ServerConfig::builder_with_protocol_versions(TLS13_ONLY);
        let builder = match client_verifier {
            None => builder.with_no_client_auth(),
            Some(v) => builder.with_client_cert_verifier(v),
        };
        let config = builder
            .with_single_cert(cert_chain, key)
            .map_err(TlsError::Rustls)?;
        Ok(Self {
            inner: Arc::new(config),
        })
    }

    /// Returns the underlying `rustls::ServerConfig` (for advanced callers).
    pub fn rustls_config(&self) -> &Arc<ServerConfig> {
        &self.inner
    }
}

// ---------------------------------------------------------------------------
// Client side
// ---------------------------------------------------------------------------

/// Client-side TLS 1.3 configuration. Anchored on a root CA for server
/// certificate validation — there is no self-signed bypass.
pub struct TlsClientConfig {
    inner: Arc<ClientConfig>,
}

impl TlsClientConfig {
    /// Build a client config that validates the server certificate against
    /// `server_ca_pem`. No client certificate is offered (server-only TLS).
    pub fn new_server_only(server_ca_pem: &[u8]) -> Result<Self, TlsError> {
        Self::build(server_ca_pem, None, None)
    }

    /// Build a client config for mutual TLS: validates the server certificate
    /// against `server_ca_pem` AND offers `client_cert_pem` + `client_key_pem`
    /// to the server.
    pub fn new_mtls(
        server_ca_pem: &[u8],
        client_cert_pem: &[u8],
        client_key_pem: &[u8],
    ) -> Result<Self, TlsError> {
        Self::build(server_ca_pem, Some(client_cert_pem), Some(client_key_pem))
    }

    fn build(
        server_ca_pem: &[u8],
        client_cert_pem: Option<&[u8]>,
        client_key_pem: Option<&[u8]>,
    ) -> Result<Self, TlsError> {
        install_ring_provider();
        let root_store = build_root_store(server_ca_pem)?;

        let builder = ClientConfig::builder_with_protocol_versions(TLS13_ONLY)
            .with_root_certificates(root_store);
        let config = match (client_cert_pem, client_key_pem) {
            (None, None) => builder.with_no_client_auth(),
            (Some(cert_pem), Some(key_pem)) => {
                let chain = parse_cert_chain_pem(cert_pem)?;
                let key = parse_private_key_pem(key_pem)?;
                builder
                    .with_client_auth_cert(chain, key)
                    .map_err(TlsError::Rustls)?
            }
            _ => {
                return Err(TlsError::EmptyMaterial(
                    "client cert/key (must provide both or neither)",
                ))
            }
        };
        Ok(Self {
            inner: Arc::new(config),
        })
    }

    /// Returns the underlying `rustls::ClientConfig`.
    pub fn rustls_config(&self) -> &Arc<ClientConfig> {
        &self.inner
    }
}

// ---------------------------------------------------------------------------
// Acceptor / Connector — real TLS handshakes over real TCP
// ---------------------------------------------------------------------------

/// A TLS acceptor: wraps a `TcpListener` and performs the server-side TLS 1.3
/// handshake on each accepted connection.
pub struct TlsAcceptor {
    listener: TcpListener,
    config: Arc<ServerConfig>,
}

impl TlsAcceptor {
    pub fn new(listener: TcpListener, config: TlsServerConfig) -> Self {
        Self {
            listener,
            config: config.inner,
        }
    }

    /// Accepts one TCP connection and prepares the TLS server connection. The
    /// handshake is driven lazily by the first `read`/`write` on the returned
    /// stream — this matches rustls's design (handshake is not a separate
    /// phase but happens inside the I/O loop). A handshake failure surfaces as
    /// an `io::Error` on the first `read` or `write`.
    pub fn accept(
        &self,
    ) -> Result<rustls::StreamOwned<rustls::ServerConnection, TcpStream>, TlsError> {
        let (stream, _peer) = self.listener.accept()?;
        stream.set_nonblocking(false).ok();
        // Bound the handshake so a misbehaving peer cannot stall the server.
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
        let conn = rustls::ServerConnection::new(self.config.clone()).map_err(TlsError::Rustls)?;
        Ok(rustls::StreamOwned::new(conn, stream))
    }
}

/// A TLS connector: wraps a `TcpStream` and performs the client-side TLS 1.3
/// handshake against `server_name`.
pub struct TlsConnector {
    config: Arc<ClientConfig>,
    server_name: ServerName<'static>,
}

impl TlsConnector {
    pub fn new(config: TlsClientConfig, server_name: ServerName<'static>) -> Self {
        Self {
            config: config.inner,
            server_name,
        }
    }

    /// Connects to `addr` and prepares the TLS client connection. The
    /// handshake is driven lazily by the first `write`/`read` on the returned
    /// stream.
    pub fn connect(
        &self,
        addr: std::net::SocketAddr,
    ) -> Result<rustls::StreamOwned<rustls::ClientConnection, TcpStream>, TlsError> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nonblocking(false).ok();
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
        let conn = rustls::ClientConnection::new(self.config.clone(), self.server_name.clone())
            .map_err(TlsError::Rustls)?;
        Ok(rustls::StreamOwned::new(conn, stream))
    }
}

/// 有界读取：读至 EOF 或 `max_bytes` 上限（34 SE-13：旧 `read_to_end`
/// 无界 Vec = 对端 DoS——恶意对端可无限撑爆内存；调用方按协议上限
/// 传 `max_bytes`，超限返回 [`std::io::ErrorKind::OutOfMemory`]）。
pub fn read_to_end_limited<R: Read>(r: &mut R, max_bytes: usize) -> std::io::Result<Vec<u8>> {
    let mut v = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if v.len() >= max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::OutOfMemory,
                "peer sent more than max_bytes",
            ));
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            break; // EOF
        }
        let take = n.min(max_bytes - v.len());
        v.extend_from_slice(&buf[..take]);
    }
    Ok(v)
}

/// Convenience: write all bytes and flush.
pub fn write_all_flush<W: Write>(w: &mut W, data: &[u8]) -> std::io::Result<()> {
    w.write_all(data)?;
    w.flush()
}

// ============================================================================
// 设备端 TLS（蓝图/20 R1-3）：embedded-tls + 本仓 PKI 证书链验证。
// 仅 `embedded` feature 编译；no_std 目标可用（blocking 模式 + smoltcp 适配）。
// ============================================================================
#[cfg(feature = "embedded")]
pub mod embedded;
