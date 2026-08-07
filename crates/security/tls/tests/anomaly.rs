//! NET-005 certificate anomaly evidence.
//!
//! These tests generate a real PKI with `rcgen` (root CA + leaf certs), spin
//! up a real `rustls` TLS 1.3 server on an ephemeral port, and connect with a
//! real `rustls` client over a real TCP socket. They verify that the rustls
//! certificate validator — NOT a custom check — rejects:
//!
//! - An **expired** server certificate.
//! - A server certificate with a **wrong SAN** (CN mismatch).
//! - An **untrusted self-signed** server certificate (not in the trust store).
//! - A **missing client certificate** in mTLS mode.
//!
//! And that it accepts:
//!
//! - A valid, in-trust-store server certificate (positive control).
//! - A valid mTLS handshake where the client presents a trusted client cert.
//!
//! Run: `cargo test -p eneros-tls --test anomaly`

#![allow(clippy::disallowed_macros)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use eneros_tls::{TlsAcceptor, TlsClientConfig, TlsConnector, TlsServerConfig};
use rcgen::{CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::ServerName;
use time::{Date, Month, OffsetDateTime, Time};

// ---------------------------------------------------------------------------
// Test PKI helpers
// ---------------------------------------------------------------------------

/// A self-contained test PKI: a root CA and various leaf certs signed (or not)
/// by it.
#[allow(dead_code)]
struct TestPki {
    /// Root CA certificate (PEM).
    ca_cert_pem: String,
    /// Root CA private key (PEM, PKCS#8).
    ca_key_pem: String,
    /// A valid server cert for `localhost` signed by the CA.
    server_cert_pem: String,
    server_key_pem: String,
    /// An expired server cert for `localhost` signed by the CA.
    expired_cert_pem: String,
    expired_key_pem: String,
    /// A server cert for `evil.example.com` (wrong SAN) signed by the CA.
    wrong_san_cert_pem: String,
    wrong_san_key_pem: String,
    /// A self-signed server cert for `localhost` NOT signed by the CA.
    self_signed_cert_pem: String,
    self_signed_key_pem: String,
    /// A client cert for mTLS, signed by the CA.
    client_cert_pem: String,
    client_key_pem: String,
}

fn make_key() -> KeyPair {
    KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("generate key")
}

/// Builds a date-time at a Y/M/D h:m:s in UTC.
fn dt_utc(year: i32, month: Month, day: u8, hour: u8, min: u8, sec: u8) -> OffsetDateTime {
    let d = Date::from_calendar_date(year, month, day).expect("date");
    let t = Time::from_hms(hour, min, sec).expect("time");
    d.with_time(t).assume_utc()
}

/// 并行测试下确定性安装 rustls 默认 crypto provider（蓝图/20 R2 修复：
/// 多个测试并发首次触发 `install_default` 时存在偶发竞争）。
fn ensure_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn build_test_pki() -> TestPki {
    // ---- Root CA ----
    let ca_key = make_key();
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
    ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "EnerOS Test Root CA");
    ca_dn.push(DnType::OrganizationName, "EnerOS");
    ca_params.distinguished_name = ca_dn;
    ca_params.not_before = dt_utc(2024, Month::January, 1, 0, 0, 0);
    ca_params.not_after = dt_utc(2034, Month::January, 1, 0, 0, 0);
    let ca_cert = ca_params.self_signed(&ca_key).expect("ca self-signed");
    let ca_cert_pem = ca_cert.pem();
    let ca_key_pem = ca_key.serialize_pem();

    // ---- Valid server cert for localhost ----
    let (server_cert_pem, server_key_pem) = sign_leaf(
        &ca_cert,
        &ca_key,
        "localhost",
        dt_utc(2024, Month::January, 1, 0, 0, 0),
        dt_utc(2034, Month::January, 1, 0, 0, 0),
    );

    // ---- Expired server cert (not_after in the past) ----
    let (expired_cert_pem, expired_key_pem) = sign_leaf(
        &ca_cert,
        &ca_key,
        "localhost",
        dt_utc(2020, Month::January, 1, 0, 0, 0),
        dt_utc(2021, Month::January, 1, 0, 0, 0),
    );

    // ---- Wrong-SAN server cert (evil.example.com, signed by the CA) ----
    let (wrong_san_cert_pem, wrong_san_key_pem) = sign_leaf(
        &ca_cert,
        &ca_key,
        "evil.example.com",
        dt_utc(2024, Month::January, 1, 0, 0, 0),
        dt_utc(2034, Month::January, 1, 0, 0, 0),
    );

    // ---- Self-signed server cert NOT signed by the CA ----
    let ss_key = make_key();
    let mut ss_params = CertificateParams::new(vec!["localhost".to_string()]).expect("ss params");
    ss_params.not_before = dt_utc(2024, Month::January, 1, 0, 0, 0);
    ss_params.not_after = dt_utc(2034, Month::January, 1, 0, 0, 0);
    let ss_cert = ss_params.self_signed(&ss_key).expect("ss self-signed");
    let self_signed_cert_pem = ss_cert.pem();
    let self_signed_key_pem = ss_key.serialize_pem();

    // ---- Client cert for mTLS (signed by the CA) ----
    let client_key = make_key();
    let mut client_params =
        CertificateParams::new(vec!["eneros-test-client".to_string()]).expect("client params");
    client_params.not_before = dt_utc(2024, Month::January, 1, 0, 0, 0);
    client_params.not_after = dt_utc(2034, Month::January, 1, 0, 0, 0);
    let client_cert = client_params
        .signed_by(&client_key, &ca_cert, &ca_key)
        .expect("client signed");
    let client_cert_pem = client_cert.pem();
    let client_key_pem = client_key.serialize_pem();

    TestPki {
        ca_cert_pem,
        ca_key_pem,
        server_cert_pem,
        server_key_pem,
        expired_cert_pem,
        expired_key_pem,
        wrong_san_cert_pem,
        wrong_san_key_pem,
        self_signed_cert_pem,
        self_signed_key_pem,
        client_cert_pem,
        client_key_pem,
    }
}

/// Signs a leaf server cert for `hostname` with the given validity window.
/// `CertificateParams::new(vec![hostname])` already sets the DNS SAN.
fn sign_leaf(
    ca_cert: &rcgen::Certificate,
    ca_key: &KeyPair,
    hostname: &str,
    not_before: OffsetDateTime,
    not_after: OffsetDateTime,
) -> (String, String) {
    let leaf_key = make_key();
    let mut params = CertificateParams::new(vec![hostname.to_string()]).expect("leaf params");
    params.not_before = not_before;
    params.not_after = not_after;
    let cert = params
        .signed_by(&leaf_key, ca_cert, ca_key)
        .expect("leaf signed");
    (cert.pem(), leaf_key.serialize_pem())
}

// ---------------------------------------------------------------------------
// Test harness: start a TLS server in a thread, connect with a TLS client
// ---------------------------------------------------------------------------

/// Spawns a TLS server on an ephemeral port. Returns the bound address and a
/// handle that joins when the server has served `expected_conns` connections
/// (each connection: accept TLS, read request, echo it back, close).
fn spawn_echo_server(
    server_config: TlsServerConfig,
    expected_conns: usize,
) -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let acceptor = TlsAcceptor::new(listener, server_config);
    let handle = std::thread::spawn(move || {
        for _ in 0..expected_conns {
            let mut stream = match acceptor.accept() {
                Ok(s) => s,
                Err(_) => continue, // handshake failed — the test will observe this on the client side
            };
            // Echo: read up to 64 bytes, write them back.
            let mut buf = [0u8; 64];
            let _ = stream.sock.set_read_timeout(Some(Duration::from_secs(2)));
            if let Ok(n) = stream.read(&mut buf) {
                if n > 0 {
                    let _ = stream.write_all(&buf[..n]);
                    let _ = stream.flush();
                }
            }
        }
    });
    (addr, handle)
}

/// Attempts a TLS client connection to `addr` with `client_config`, sends a
/// probe byte, and reads the echo. Returns Ok(()) on a successful TLS session
/// or Err on any handshake/IO error.
fn try_connect(addr: std::net::SocketAddr, client_config: TlsClientConfig) -> Result<(), String> {
    let server_name = ServerName::try_from("localhost".to_string()).expect("server name");
    let connector = TlsConnector::new(client_config, server_name);
    let mut stream = connector
        .connect(addr)
        .map_err(|e| format!("connect/handshake: {e}"))?;
    stream
        .write_all(b"ping")
        .map_err(|e| format!("write: {e}"))?;
    stream.flush().map_err(|e| format!("flush: {e}"))?;
    let mut buf = [0u8; 4];
    stream
        .read_exact(&mut buf)
        .map_err(|e| format!("read echo: {e}"))?;
    if &buf != b"ping" {
        return Err(format!("echo mismatch: {buf:?}"));
    }
    Ok(())
}

/// Builds a client config trusting `ca_pem` with no client cert.
fn client_trusting(ca_pem: &str) -> TlsClientConfig {
    TlsClientConfig::new_server_only(ca_pem.as_bytes()).expect("client config")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Positive control: a valid server cert signed by the trusted CA, correct
/// SAN (localhost). The handshake MUST succeed and the echo MUST round-trip.
#[test]
fn test_positive_valid_server_cert() {
    ensure_provider();
    let pki = build_test_pki();
    let server = TlsServerConfig::new_server_only(
        pki.server_cert_pem.as_bytes(),
        pki.server_key_pem.as_bytes(),
    )
    .expect("server config");
    let (addr, handle) = spawn_echo_server(server, 1);

    let result = try_connect(addr, client_trusting(&pki.ca_cert_pem));
    assert!(
        result.is_ok(),
        "valid cert should succeed: {:?}",
        result.err()
    );

    handle.join().unwrap();
}

/// Anomaly: the server presents an EXPIRED certificate. rustls-webpki must
/// reject it at the handshake — the client's `connect` fails.
#[test]
fn test_anomaly_expired_server_cert_rejected() {
    ensure_provider();
    let pki = build_test_pki();
    let server = TlsServerConfig::new_server_only(
        pki.expired_cert_pem.as_bytes(),
        pki.expired_key_pem.as_bytes(),
    )
    .expect("server config (expired cert)");
    let (addr, handle) = spawn_echo_server(server, 1);

    let result = try_connect(addr, client_trusting(&pki.ca_cert_pem));
    assert!(
        result.is_err(),
        "expired cert MUST be rejected by rustls, but handshake succeeded"
    );
    let err = result.unwrap_err();
    // The error message should mention expiry / validity.
    eprintln!("[expired] client error (expected): {err}");

    handle.join().unwrap();
}

/// Anomaly: the server cert is valid and signed by the trusted CA, but its
/// SAN is `evil.example.com` while the client connects expecting `localhost`.
/// rustls must reject the cert (name mismatch).
#[test]
fn test_anomaly_wrong_san_rejected() {
    ensure_provider();
    let pki = build_test_pki();
    let server = TlsServerConfig::new_server_only(
        pki.wrong_san_cert_pem.as_bytes(),
        pki.wrong_san_key_pem.as_bytes(),
    )
    .expect("server config (wrong SAN)");
    let (addr, handle) = spawn_echo_server(server, 1);

    let result = try_connect(addr, client_trusting(&pki.ca_cert_pem));
    assert!(
        result.is_err(),
        "wrong-SAN cert MUST be rejected (name mismatch), but handshake succeeded"
    );
    let err = result.unwrap_err();
    eprintln!("[wrong-san] client error (expected): {err}");

    handle.join().unwrap();
}

/// Anomaly: the server presents a self-signed certificate that is NOT in the
/// client's trust store. rustls must reject it (untrusted issuer).
#[test]
fn test_anomaly_untrusted_self_signed_rejected() {
    ensure_provider();
    let pki = build_test_pki();
    let server = TlsServerConfig::new_server_only(
        pki.self_signed_cert_pem.as_bytes(),
        pki.self_signed_key_pem.as_bytes(),
    )
    .expect("server config (self-signed)");
    let (addr, handle) = spawn_echo_server(server, 1);

    // Client trusts ONLY the CA — the self-signed cert is not in the store.
    let result = try_connect(addr, client_trusting(&pki.ca_cert_pem));
    assert!(
        result.is_err(),
        "untrusted self-signed cert MUST be rejected, but handshake succeeded"
    );
    let err = result.unwrap_err();
    eprintln!("[untrusted] client error (expected): {err}");

    handle.join().unwrap();
}

/// Anomaly: the server requires mTLS (client cert), but the client offers no
/// certificate. The server MUST abort the handshake.
#[test]
fn test_anomaly_mtls_client_cert_missing_rejected() {
    ensure_provider();
    let pki = build_test_pki();
    let server = TlsServerConfig::new_mtls(
        pki.server_cert_pem.as_bytes(),
        pki.server_key_pem.as_bytes(),
        pki.ca_cert_pem.as_bytes(), // trust the same CA for client certs
    )
    .expect("mtls server config");
    let (addr, handle) = spawn_echo_server(server, 1);

    // Client does NOT present a cert — server must reject.
    let result = try_connect(addr, client_trusting(&pki.ca_cert_pem));
    assert!(
        result.is_err(),
        "mTLS server MUST reject a client that offers no certificate, but handshake succeeded"
    );
    let err = result.unwrap_err();
    eprintln!("[mtls-missing] client error (expected): {err}");

    handle.join().unwrap();
}

/// Positive mTLS: the client presents a valid client cert signed by the
/// server's trusted CA. The handshake MUST succeed and the echo MUST
/// round-trip.
#[test]
fn test_positive_mtls_with_valid_client_cert() {
    ensure_provider();
    let pki = build_test_pki();
    let server = TlsServerConfig::new_mtls(
        pki.server_cert_pem.as_bytes(),
        pki.server_key_pem.as_bytes(),
        pki.ca_cert_pem.as_bytes(),
    )
    .expect("mtls server config");
    let (addr, handle) = spawn_echo_server(server, 1);

    let client = TlsClientConfig::new_mtls(
        pki.ca_cert_pem.as_bytes(),
        pki.client_cert_pem.as_bytes(),
        pki.client_key_pem.as_bytes(),
    )
    .expect("mtls client config");

    let result = try_connect(addr, client);
    assert!(
        result.is_ok(),
        "valid mTLS should succeed: {:?}",
        result.err()
    );

    handle.join().unwrap();
}
