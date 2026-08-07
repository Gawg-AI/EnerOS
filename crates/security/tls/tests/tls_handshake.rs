//! v0.210.0（P0-14）：设备端 TLS 选型落地——host 侧真实 TLS 1.3 握手 +
//! 应用数据往返证据（rustls + webpki，真实证书链验证）。
//!
//! 本测试用 rcgen 生成自签 CA + 服务器证书，经真实 TCP 完成 TLS 1.3
//! 握手并往返应用数据。设备端（aarch64-unknown-none）rustls no_std
//! 编译路径在 CI（nightly build-std）验证（本地 rustup 只读无法装目标）。

use eneros_tls::{TlsAcceptor, TlsClientConfig, TlsConnector, TlsServerConfig};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

/// 生成自签 CA 证书（PEM）。
fn gen_ca() -> (rcgen::Certificate, KeyPair) {
    let mut params = CertificateParams::new(vec!["Test CA".to_string()]).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let kp = KeyPair::generate().unwrap();
    let cert = params.self_signed(&kp).unwrap();
    (cert, kp)
}

/// 用 CA 签发服务器证书（SAN=localhost）。
fn gen_server_cert(ca: &rcgen::Certificate, ca_kp: &KeyPair) -> (rcgen::Certificate, KeyPair) {
    let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    params.is_ca = IsCa::NoCa;
    let kp = KeyPair::generate().unwrap();
    let cert = params.signed_by(&kp, ca, ca_kp).unwrap();
    (cert, kp)
}

/// 并行测试下确定性安装 rustls 默认 crypto provider（蓝图/20 R2 修复：
/// 多个测试并发首次触发 install_default 时存在偶发竞争）。
fn ensure_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[test]
fn tls13_handshake_and_echo_roundtrip() {
    ensure_provider();
    let (ca, ca_kp) = gen_ca();
    let (server_cert, server_kp) = gen_server_cert(&ca, &ca_kp);

    let server_cert_pem = server_cert.pem();
    let server_key_pem = server_kp.serialize_pem();
    let ca_pem = ca.pem();

    // 服务端：监听本地端口，接受连接并回显。
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server_cfg =
        TlsServerConfig::new_server_only(server_cert_pem.as_bytes(), server_key_pem.as_bytes())
            .unwrap();

    let server_thread = thread::spawn(move || {
        let acceptor = TlsAcceptor::new(listener, server_cfg);
        let mut stream = acceptor.accept().unwrap();
        let mut buf = [0u8; 64];
        let n = stream.read(&mut buf).unwrap();
        stream.write_all(&buf[..n]).unwrap();
        stream.flush().unwrap();
    });

    // 客户端：连接并发送数据，读取回显。
    let client_cfg = TlsClientConfig::new_server_only(ca_pem.as_bytes()).unwrap();
    let connector = TlsConnector::new(client_cfg, "localhost".try_into().unwrap());
    let mut stream = connector.connect(addr).unwrap();

    stream.write_all(b"hello-enelos-tls13").unwrap();
    stream.flush().unwrap();
    let mut buf = [0u8; 64];
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"hello-enelos-tls13", "TLS 应用数据往返");

    server_thread.join().unwrap();
}

#[test]
fn tls13_mtls_handshake() {
    ensure_provider();
    // mTLS：服务器要求客户端证书（TlsServerConfig::new_mtls），
    // 客户端用 new_mtls 提供证书。
    let (ca, ca_kp) = gen_ca();
    let (server_cert, server_kp) = gen_server_cert(&ca, &ca_kp);
    let (client_cert, client_kp) = gen_server_cert(&ca, &ca_kp);

    let ca_pem = ca.pem();
    let server_cfg = TlsServerConfig::new_mtls(
        server_cert.pem().as_bytes(),
        server_kp.serialize_pem().as_bytes(),
        ca_pem.as_bytes(),
    )
    .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server_thread = thread::spawn(move || {
        let acceptor = TlsAcceptor::new(listener, server_cfg);
        let mut stream = acceptor.accept().unwrap();
        let mut buf = [0u8; 32];
        let n = stream.read(&mut buf).unwrap();
        stream.write_all(&buf[..n]).unwrap();
        stream.flush().unwrap();
    });

    let client_cfg = TlsClientConfig::new_mtls(
        ca_pem.as_bytes(),
        client_cert.pem().as_bytes(),
        client_kp.serialize_pem().as_bytes(),
    )
    .unwrap();
    let connector = TlsConnector::new(client_cfg, "localhost".try_into().unwrap());
    let mut stream = connector.connect(addr).unwrap();
    stream.write_all(b"mtls-ok").unwrap();
    stream.flush().unwrap();
    let mut buf = [0u8; 32];
    let n = stream.read(&mut buf).unwrap();
    assert_eq!(&buf[..n], b"mtls-ok");

    server_thread.join().unwrap();
}
