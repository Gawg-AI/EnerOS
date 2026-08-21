//! 47（WP-47.5）：`SecureChannelFactory` 第一实现——包装 embedded-tls
//! （TLS 1.3，P-256 ECDSA + AES-128-GCM，RFC 8446 默认套件）供 MQTT/TCP
//! 使用。
//!
//! - 证书链验证走本仓 PKI（`PkiVerifier`：有效期/CRL/签名/SPKI 根比对 +
//!   SAN/CN hostname 校验）；
//! - 只接受 ECDSA P-256 握手签名；无任何 danger bypass；
//! - `qemu-test` 装配允许明文（显式 `InsecureStream`，永不冒充）——
//!   policy/实现一致机器校验见 `protocol-abstract::channel` 注册表。

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use std::rc::Rc;

use eneros_crypto::pki::x509::X509Certificate;
use eneros_protocol_abstract::channel::{
    ChannelError, ChannelPolicy, ChannelSecurity, Endpoint, IdentityRef, SecureChannel,
    SecureChannelFactory,
};
use eneros_protocol_abstract::transport::{TransportFactory, TransportHandle, TransportSpec};

use crate::embedded::{DefaultCipherSuite, EmbeddedTlsStream, PkiProvider, VerifyRole};

/// embedded-io 适配：`TransportHandle` → `embedded_io::{Read, Write}`。
pub struct TransportHandleIo(pub Box<dyn TransportHandle>);

impl embedded_io::ErrorType for TransportHandleIo {
    type Error = embedded_io::ErrorKind;
}

impl embedded_io::Read for TransportHandleIo {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.0
            .recv(buf)
            .map_err(|_| embedded_io::ErrorKind::ConnectionReset)
    }
}

impl embedded_io::Write for TransportHandleIo {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.0
            .send(buf)
            .map_err(|_| embedded_io::ErrorKind::WriteZero)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// 已建立 TLS 通道（TLS 1.3；对端身份 = 已验叶子 CN）。
pub struct SecureChannelImpl {
    stream: Option<EmbeddedTlsStream<'static, TransportHandleIo>>,
    verified_cn: Option<String>,
}

impl SecureChannel for SecureChannelImpl {
    fn send(&mut self, data: &[u8]) -> Result<(), ChannelError> {
        let stream = self.stream.as_mut().ok_or(ChannelError::Closed)?;
        stream.write(data).map_err(|_| ChannelError::Io)?;
        stream.flush().map_err(|_| ChannelError::Io)
    }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, ChannelError> {
        let stream = self.stream.as_mut().ok_or(ChannelError::Closed)?;
        stream.read(buf).map_err(|_| ChannelError::Io)
    }
    fn close(&mut self) -> Result<(), ChannelError> {
        match self.stream.take() {
            Some(stream) => stream.close().map(|_| ()).map_err(|_| ChannelError::Io),
            None => Ok(()),
        }
    }
    fn negotiated_security(&self) -> ChannelSecurity {
        ChannelSecurity::Tls13
    }
    fn peer_identity(&self) -> Option<IdentityRef<'_>> {
        let cn = self.verified_cn.as_deref()?;
        Some(IdentityRef {
            subject: cn.as_bytes(),
            role: b"server",
        })
    }
}

/// 设备端 TLS 工厂（embedded-tls；TLS 1.3 P-256/AES-GCM）。
///
/// 记录缓冲（读 16640B / 写 4096B——TLS 1.3 最大记录）经 `Box::leak`
/// 与通道同生命周期（每条通道一次分配；无自引用）。
pub struct EmbeddedTlsFactory {
    roots: Vec<X509Certificate>,
    now: u64,
    transport: Box<dyn TransportFactory>,
    mtu: usize,
}

impl EmbeddedTlsFactory {
    pub fn new(
        roots: Vec<X509Certificate>,
        now: u64,
        transport: Box<dyn TransportFactory>,
        mtu: usize,
    ) -> Self {
        Self {
            roots,
            now,
            transport,
            mtu,
        }
    }
}

impl SecureChannelFactory for EmbeddedTlsFactory {
    fn connect(
        &mut self,
        endpoint: &Endpoint<'_>,
        _identity: Option<&IdentityRef<'_>>,
        policy: &ChannelPolicy,
        _deadline_ms: u64,
    ) -> Result<Box<dyn SecureChannel>, ChannelError> {
        // 策略：本实现提供 TLS 1.3 —— 策略不允许该等级即拒绝（fail-closed）。
        if !policy.allows(ChannelSecurity::Tls13) {
            return Err(ChannelError::PolicyDenied);
        }
        // 服务器名（SNI + hostname 校验输入）必须为 UTF-8。
        let host =
            core::str::from_utf8(endpoint.host).map_err(|_| ChannelError::PeerIdentityMismatch)?;
        let peer = alloc::format!("{host}:{}", endpoint.port);
        let mut spec = TransportSpec {
            kind: eneros_protocol_abstract::transport::TransportKind::Tcp,
            peer: [0; 128],
            peer_len: 0,
            mtu: self.mtu,
            connect_timeout_ms: 2000,
            recv_timeout_ms: 1000,
        };
        let bytes = peer.as_bytes();
        if bytes.len() > spec.peer.len() {
            return Err(ChannelError::Unsupported);
        }
        spec.peer[..bytes.len()].copy_from_slice(bytes);
        spec.peer_len = bytes.len() as u8;
        let handle = self.transport.open(&spec).map_err(|_| ChannelError::Io)?;
        let io = TransportHandleIo(handle);
        let cell: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let mut read_buf: &'static mut [u8] = Box::leak(vec![0u8; 16640].into_boxed_slice());
        let mut write_buf: &'static mut [u8] = Box::leak(vec![0u8; 4096].into_boxed_slice());
        let stream = EmbeddedTlsStream::open(
            io,
            PkiProvider::with_verified_cell(
                self.roots.clone(),
                VerifyRole::Client,
                self.now,
                eneros_crypto::rng::CsRng::new(),
                None,
                Rc::clone(&cell),
            ),
            Some(host),
            read_buf,
            write_buf,
        )
        .map_err(|_| ChannelError::HandshakeFailed)?;
        let verified_cn = cell.borrow().clone();
        Ok(Box::new(SecureChannelImpl {
            stream: Some(stream),
            verified_cn,
        }))
    }
}

#[cfg(all(test, feature = "embedded"))]
mod tests {
    use std::io::{Read as _, Write as _};

    use eneros_crypto::pki::parser::to_der;
    use eneros_crypto::pki::x509::{
        DistinguishedName, Extension, SignatureAlgorithm, SubjectPublicKey,
    };
    use eneros_protocol_abstract::channel::{ChannelPolicy, ChannelSecurity, Endpoint};

    use super::*;

    const NOW: u64 = 1_700_000_000;

    /// 测试身份（ECDSA P-256 自签；可带 SAN 扩展）。
    struct TestIdentity {
        cert_der: Vec<u8>,
        key_der: Vec<u8>,
        x509: X509Certificate,
    }

    fn make_identity(cn: &str, scalar: u8, san: Option<&str>) -> TestIdentity {
        let sk: p256::ecdsa::SigningKey =
            p256::ecdsa::SigningKey::from_bytes(&[scalar; 32].into()).unwrap();
        let pk = SubjectPublicKey::EcdsaP256(
            sk.verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        );
        let dn = DistinguishedName::new(cn);
        let extensions = match san {
            Some(dns) => {
                // Extension.value = GeneralNames 内容（编码器自行包 OCTET
                // STRING）：SEQUENCE { [2] dNSName }。
                let name = dns.as_bytes();
                let mut gn = vec![0x82, name.len() as u8];
                gn.extend_from_slice(name);
                let mut seq = vec![0x30, gn.len() as u8];
                seq.extend_from_slice(&gn);
                vec![Extension {
                    oid: vec![0x06, 0x03, 0x55, 0x1D, 0x11], // 2.5.29.17
                    critical: false,
                    value: seq,
                }]
            }
            None => Vec::new(),
        };
        let unsigned = X509Certificate::new(
            2,
            vec![1],
            SignatureAlgorithm::EcdsaWithSha256,
            dn.clone(),
            dn.clone(),
            NOW,
            NOW + 86400 * 365,
            pk,
            extensions,
            Vec::new(),
        );
        let tbs = unsigned.encode_tbs().unwrap();
        use p256::ecdsa::signature::Signer;
        let sig: p256::ecdsa::Signature = sk.sign(&tbs);
        let cert = X509Certificate::new(
            2,
            vec![1],
            SignatureAlgorithm::EcdsaWithSha256,
            dn.clone(),
            dn.clone(),
            NOW,
            NOW + 86400 * 365,
            SubjectPublicKey::EcdsaP256(
                sk.verifying_key()
                    .to_encoded_point(false)
                    .as_bytes()
                    .to_vec(),
            ),
            unsigned.extensions.clone(),
            sig.to_der().as_bytes().to_vec(),
        );
        let der = to_der(&cert).unwrap();
        let point = sk
            .verifying_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec();
        let oid = [0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
        let mut inner = vec![0x02, 0x01, 0x01, 0x04, 0x20];
        inner.extend_from_slice(&[scalar; 32]);
        inner.extend_from_slice(&[0xA0, 0x0A]);
        inner.extend_from_slice(&oid);
        inner.extend_from_slice(&[0xA1, 0x44, 0x03, 0x42, 0x00]);
        inner.extend_from_slice(&point);
        let mut key_der = vec![0x30, inner.len() as u8];
        key_der.extend(inner);
        TestIdentity {
            cert_der: der,
            key_der,
            x509: cert,
        }
    }

    /// rustls 服务器（host 侧；回显 pong）。
    fn spawn_rustls_server(
        listener: std::net::TcpListener,
        server_key: Vec<u8>,
        server_cert: Vec<u8>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let key = rustls::pki_types::PrivateKeyDer::Sec1(
                rustls::pki_types::PrivateSec1KeyDer::from(server_key),
            );
            let cert = rustls::pki_types::CertificateDer::from(server_cert);
            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .expect("server cert config");
            let tcp = listener.accept().expect("accept").0;
            let conn =
                rustls::ServerConnection::new(std::sync::Arc::new(config)).expect("server conn");
            let mut stream = rustls::StreamOwned::new(conn, tcp);
            let mut buf = [0u8; 64];
            let n = stream.read(&mut buf).expect("read");
            assert_eq!(&buf[..n], b"ping");
            stream.write_all(b"pong").expect("write pong");
            stream.flush().expect("flush");
        })
    }

    /// 直连 std TcpStream 的 TransportFactory（host 测试）。
    struct StdTcpFactory {
        addr: std::net::SocketAddr,
    }
    impl eneros_protocol_abstract::transport::TransportFactory for StdTcpFactory {
        fn open(
            &mut self,
            _spec: &eneros_protocol_abstract::transport::TransportSpec,
        ) -> Result<
            Box<dyn eneros_protocol_abstract::transport::TransportHandle>,
            eneros_protocol_abstract::transport::TransportError,
        > {
            let stream = std::net::TcpStream::connect(self.addr)
                .map_err(|_| eneros_protocol_abstract::transport::TransportError::ConnectFailed)?;
            stream
                .set_read_timeout(Some(std::time::Duration::from_millis(5000)))
                .ok();
            Ok(Box::new(StdTcpHandle(stream)))
        }
        fn supports(&self, kind: eneros_protocol_abstract::transport::TransportKind) -> bool {
            kind == eneros_protocol_abstract::transport::TransportKind::Tcp
        }
    }
    struct StdTcpHandle(std::net::TcpStream);
    impl eneros_protocol_abstract::transport::TransportHandle for StdTcpHandle {
        fn send(
            &mut self,
            data: &[u8],
        ) -> Result<(), eneros_protocol_abstract::transport::TransportError> {
            self.0
                .write_all(data)
                .map_err(|_| eneros_protocol_abstract::transport::TransportError::Io)
        }
        fn recv(
            &mut self,
            buf: &mut [u8],
        ) -> Result<usize, eneros_protocol_abstract::transport::TransportError> {
            self.0
                .read(buf)
                .map_err(|_| eneros_protocol_abstract::transport::TransportError::Io)
        }
        fn close(&mut self) -> Result<(), eneros_protocol_abstract::transport::TransportError> {
            Ok(())
        }
        fn is_connected(&self) -> bool {
            true
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    fn factory_with(server: &TestIdentity, addr: std::net::SocketAddr) -> EmbeddedTlsFactory {
        EmbeddedTlsFactory::new(
            vec![server.x509.clone()],
            NOW,
            Box::new(StdTcpFactory { addr }),
            260,
        )
    }

    /// 正例：TLS 1.3 握手 + 往返 + 对端身份（CN）。
    #[test]
    fn factory_handshake_roundtrip_and_peer_identity() {
        let server_id = make_identity("eneros-server", 0x21, None);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
        );
        let mut factory = factory_with(&server_id, addr);
        let policy = ChannelPolicy::default_release();
        let mut ch = factory
            .connect(
                &Endpoint {
                    host: b"eneros-server",
                    port: addr.port(),
                },
                None,
                &policy,
                5000,
            )
            .expect("TLS 1.3 握手（CN 匹配）");
        assert_eq!(ch.negotiated_security(), ChannelSecurity::Tls13);
        assert_eq!(
            ch.peer_identity().map(|i| i.subject),
            Some(&b"eneros-server"[..])
        );
        ch.send(b"ping").expect("send");
        let mut buf = [0u8; 16];
        let n = ch.recv(&mut buf).expect("recv pong");
        assert_eq!(&buf[..n], b"pong");
        let _ = ch.close();
        let _ = server.join().unwrap();
    }

    /// 负例：hostname 与 CN 不符（无 SAN）→ 握手失败（fail-closed）。
    #[test]
    fn factory_rejects_wrong_hostname() {
        let server_id = make_identity("eneros-server", 0x21, None);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
        );
        let mut factory = factory_with(&server_id, addr);
        assert!(matches!(
            factory.connect(
                &Endpoint {
                    host: b"wrong.example",
                    port: addr.port()
                },
                None,
                &ChannelPolicy::default_release(),
                5000,
            ),
            Err(ChannelError::HandshakeFailed)
        ));
        // 服务器线程在客户端中止握手时的读错误属预期——忽略 join 结果。
        let _ = server.join();
    }

    /// 47（WP-47.5）：SAN 存在时优先匹配（CN 不符但 SAN 命中 → 通过；
    /// SAN 未命中 → 拒绝——不再回退 CN）。
    #[test]
    fn factory_san_matching_semantics() {
        // 服务器证书：CN=wrong-cn，SAN=dNSName:eneros-server。
        let server_id = make_identity("wrong-cn", 0x21, Some("eneros-server"));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
        );
        let mut factory = factory_with(&server_id, addr);
        // SAN 命中 → 握手通过（CN 不匹配不再阻止）。
        let mut ch = factory
            .connect(
                &Endpoint {
                    host: b"eneros-server",
                    port: addr.port(),
                },
                None,
                &ChannelPolicy::default_release(),
                5000,
            )
            .expect("SAN 命中必须通过");
        ch.send(b"ping").expect("send");
        let mut buf = [0u8; 8];
        let n = ch.recv(&mut buf).expect("pong");
        assert_eq!(&buf[..n], b"pong");
        let _ = ch.close();
        let _ = server.join();

        // SAN 未命中（CN 匹配也拒绝）→ 第二次连接。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
        );
        let mut factory = factory_with(&server_id, addr);
        assert!(
            matches!(
                factory.connect(
                    &Endpoint {
                        host: b"wrong-cn",
                        port: addr.port()
                    },
                    None,
                    &ChannelPolicy::default_release(),
                    5000,
                ),
                Err(ChannelError::HandshakeFailed)
            ),
            "SAN 存在时 CN 不得回退"
        );
        let _ = server.join();
    }

    /// 负例：策略不允许 TLS 等级（min=Iec62351）→ PolicyDenied（零连接）。
    #[test]
    fn factory_policy_denied_when_tls_not_allowed() {
        let server_id = make_identity("eneros-server", 0x21, None);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let _server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
        );
        let mut factory = factory_with(&server_id, addr);
        let strict = ChannelPolicy {
            min_security: ChannelSecurity::Iec62351,
            allow_plain: false,
            allow_resumption: false,
            allow_zero_rtt: false,
            require_pinned_identity: true,
        };
        assert!(
            matches!(
                factory.connect(
                    &Endpoint {
                        host: b"eneros-server",
                        port: addr.port()
                    },
                    None,
                    &strict,
                    5000,
                ),
                Err(ChannelError::PolicyDenied)
            ),
            "策略拒绝 TLS1.3 等级"
        );
    }
}
