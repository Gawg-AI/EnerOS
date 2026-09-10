//! 70.5（WP-70.5.6）：目标侧（no_std）TLS 传输工厂——`TransportFactory`
//! seam 的 TLS 1.3 包装实现。
//!
//! 职责：把任意 `&mut dyn TransportFactory`（如 smoltcp TCP 工厂）包装为
//! "open = TCP 建联 + 真实 TLS 1.3 握手（embedded-tls + 本仓 PKI 验证）"
//! 的安全工厂。MQTT 插件面零协议改动——它继续面向 `TransportHandle`，
//! 明文/安全由工厂决定（装配期机器校验见 protocol-abstract::channel）。
//!
//! # 资源合同（方向 3 §8A 实现合同）
//!
//! - 记录缓冲 = **静态固定复用**（读 16640B + 写 4096B，TLS 1.3 最大
//!   记录）：同一时刻至多一条 TLS 连接（AtomicBool 守卫，双重 open =
//!   显式 Unsupported）；重复连接固定复用同一缓冲——零泄漏零增长，不
//!   每连一次分配/泄漏一次；
//! - 握手失败/关闭 = 缓冲立即释放（busy 旗复位）；
//! - `reconnect` 显式拒绝（TLS 重连必须走新 `open` = 新 TCP + 新握手；
//!   明文重连冒充安全恢复 = 合同违反）。
//!
//! # 身份与失败语义
//!
//! broker 身份 = 证书链验证（CA 锚 + 有效期/撤销/签名）+ hostname
//! （SAN 优先 CN 兼容）+ ECDSA P-256 CertificateVerify；缺材料/错名/
//! 过期/吊销/握手中断一律 Err（调用方按建联失败处理，不降级明文）。
//! 协议版本/套件：TLS 1.3，`AES_128_GCM_SHA256`（RFC 8446 默认；握手
//! 签名仅 `EcdsaSecp256r1Sha256`）。熵合同：随机源由装配方注入
//! `TlsMaterial.rng`（生产 = `csrng_from_platform()`，熵纪律 R1-4；
//! 测试 = 显式种子）。

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, Ordering};

use eneros_crypto::pki::x509::X509Certificate;
use eneros_protocol_abstract::transport::{
    TransportError, TransportFactory, TransportHandle, TransportKind, TransportSpec,
};

use crate::embedded::TransportHandleIo;
use crate::embedded::{EmbeddedTlsStream, Identity, PkiProvider, SharedCsRng, VerifyRole};

/// TLS 材料（构建期嵌入面注入；DER 形态）。
#[derive(Clone)]
pub struct TlsMaterial {
    /// broker CA（SPKI 根比对锚）。
    pub ca: X509Certificate,
    /// 设备证书链（DER，叶子在前）。
    pub client_chain: Vec<Vec<u8>>,
    /// 设备私钥（SEC1 DER，ECDSA P-256）。
    pub client_key: Vec<u8>,
    /// 期望 broker 名（SNI/CN；RFC 6125 SAN 优先）。
    pub server_name: String,
    /// 验证基准 Unix 秒（可信时钟合同——X6 墙钟链派生）。
    pub now: u64,
    /// 握手随机源（共享槽——重握手不耗尽；生产 = 平台熵
    /// `csrng_from_platform()`，测试 = 显式种子，见模块文档熵合同）。
    pub rng: crate::embedded::SharedCsRng,
}

/// 单连接缓冲守卫（静态固定复用；busy 旗防别名）。
static TLS_BUF_BUSY: AtomicBool = AtomicBool::new(false);
/// 读记录缓冲（TLS 1.3 最大记录 16640B；固定复用——见模块文档资源合同）。
static mut TLS_READ_BUF: [u8; 16640] = [0; 16640];
/// 写记录缓冲（4096B）。
static mut TLS_WRITE_BUF: [u8; 4096] = [0; 4096];

/// 释放缓冲守卫（open 失败 / close / drop 路径共用）。
fn release_guard() {
    TLS_BUF_BUSY.store(false, Ordering::Release);
}

/// 单次读尝试耗时（ms）——握手重试预算的时间单位 = 数据面预算同值
/// （68.5 调优不回退；重试次数 = 预算面，非超时值放大）。
const HANDSHAKE_ATTEMPT_ROUNDS: u32 = 50;

/// 握手期 I/O 适配：底层 recv 超时（Timeout）不再致命——在重试预算内
/// 继续等（每轮 = 一次完整数据面超时窗，内层 stack 持续推进）。会话期
/// 预算 = 1（一次超时即返回错误——插件 poll 节奏不受影响）。
struct TlsSessionIo {
    inner: TransportHandleIo,
    /// 重试预算（共享——重连路径重建预算）。
    budget: Rc<Cell<u32>>,
}

impl TlsSessionIo {
    fn take_retry(&self) -> bool {
        let left = self.budget.get();
        if left > 0 {
            self.budget.set(left - 1);
            true
        } else {
            false
        }
    }

    /// 握手期预算注入（重连路径用；open 路径构造时即置满）。
    fn set_handshake_budget(&self) {
        self.budget.set(HANDSHAKE_ATTEMPT_ROUNDS);
    }
}

impl embedded_io::ErrorType for TlsSessionIo {
    type Error = embedded_io::ErrorKind;
}

impl embedded_io::Read for TlsSessionIo {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        loop {
            match self.inner.0.recv(buf) {
                Ok(n) => {
                    READ_DATA_COUNT.fetch_add(1, Ordering::Relaxed);
                    if READ_DATA_COUNT.load(Ordering::Relaxed) % 50 == 1 {
                        eneros_runtime::println!(
                            "[tls-diag] session read data n={} len={n}",
                            READ_DATA_COUNT.load(Ordering::Relaxed)
                        );
                    }
                    return Ok(n);
                }
                Err(TransportError::Timeout) if self.take_retry() => continue,
                Err(TransportError::Timeout) => {
                    let t = READ_TIMEOUT_COUNT.fetch_add(1, Ordering::Relaxed);
                    if t % 200 == 0 {
                        eneros_runtime::println!("[tls-diag] session read timeout n={t}");
                    }
                    return Err(embedded_io::ErrorKind::TimedOut);
                }
                Err(_) => return Err(embedded_io::ErrorKind::ConnectionReset),
            }
        }
    }
}

/// 诊断计数（目标/宿主通用；观测面非判据）。
static READ_DATA_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static READ_TIMEOUT_COUNT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

impl embedded_io::Write for TlsSessionIo {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.inner
            .0
            .send(buf)
            .map_err(|_| embedded_io::ErrorKind::WriteZero)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// 已建立 TLS 传输句柄（底层 socket 内嵌于 TLS 会话——close 返还；
/// 共享材料随句柄持有，重连 = TCP 重建 + TLS 重握手，明文永不回退）。
pub struct TlsTransportHandle {
    /// Some = 会话在位（握手成功）；None = 已关闭/失败。
    stream: Option<EmbeddedTlsStream<'static, TlsSessionIo>>,
    /// 共享材料（重握手用；rng 共享槽不随握手消耗）。
    material: TlsMaterial,
    /// 会话/握手预算共享槽（重连路径恢复握手预算）。
    budget: Rc<Cell<u32>>,
}

impl TlsTransportHandle {
    /// 会话在位且未关闭（TCP 半开由 send/recv 错误面暴露——插件
    /// mark_disconnected → reconnect/新 open = 新握手）。
    pub fn session_active(&self) -> bool {
        self.stream.is_some()
    }
}

impl TransportHandle for TlsTransportHandle {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        let stream = self.stream.as_mut().ok_or(TransportError::Closed)?;
        stream.write(data).map_err(|_| TransportError::Io)?;
        stream.flush().map_err(|_| TransportError::Io)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        let stream = self.stream.as_mut().ok_or(TransportError::Closed)?;
        match stream.read(buf) {
            Ok(n) => Ok(n),
            // 对端关闭（TLS close_notify/TCP FIN）= 会话终结（诚实
            // Closed——调用方按断链处理，经新 open 恢复）。
            Err(_) => Err(TransportError::Closed),
        }
    }

    fn close(&mut self) -> Result<(), TransportError> {
        if let Some(stream) = self.stream.take() {
            // close_notify 尽力而为；缓冲守卫必须释放（资源合同）。
            let _ = stream.close();
        }
        release_guard();
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// 重连 = TCP 重建 + **完整 TLS 重握手**（材料/熵源共享；绝不以
    /// 明文重建冒充安全恢复——WP-70.5.6 失败语义）。
    fn reconnect(&mut self, peer: &str) -> Result<(), TransportError> {
        // 旧会话清理：close 返还真实底层 socket；缓冲守卫随会话终结释放。
        let mut socket = match self.stream.take() {
            Some(stream) => match stream.close() {
                Ok(s) => s,
                Err((s, _)) => s,
            },
            None => {
                release_guard();
                return Err(TransportError::Closed);
            }
        };
        release_guard();
        // 重握手期恢复重试预算（完成后收回——open 同律）。
        socket.set_handshake_budget();
        socket.inner.0.reconnect(peer)?;
        match tls_handshake(socket, &self.material, peer) {
            Ok((stream, _dead)) => {
                self.stream = Some(stream);
                self.budget.set(1);
                Ok(())
            }
            Err(e) => {
                self.budget.set(1);
                Err(e)
            }
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

impl Drop for TlsTransportHandle {
    fn drop(&mut self) {
        if self.stream.take().is_some() {
            release_guard();
        }
    }
}

/// 目标侧 TLS 传输工厂（借用内层工厂——插件 `start(cfg, transports)`
/// 的 `&mut dyn TransportFactory` 原位包装）。材料经 `RefCell` 借用
/// （rng 为握手消耗品——`open` 内换出）。
pub struct TlsTransportFactory<'a> {
    inner: &'a mut dyn TransportFactory,
    /// None = 明文路径（工厂拒绝 open——装配面在启用 TLS 时必先注入材料）。
    material: &'a RefCell<Option<TlsMaterial>>,
}

impl<'a> TlsTransportFactory<'a> {
    pub fn new(
        inner: &'a mut dyn TransportFactory,
        material: &'a RefCell<Option<TlsMaterial>>,
    ) -> Self {
        Self { inner, material }
    }
}

impl TransportFactory for TlsTransportFactory<'_> {
    fn open(&mut self, spec: &TransportSpec) -> Result<Box<dyn TransportHandle>, TransportError> {
        let material = match self.material.borrow().clone() {
            Some(m) => m,
            None => return Err(TransportError::Unsupported),
        };
        let tcp = self.inner.open(spec)?;
        let peer: &str =
            core::str::from_utf8(&spec.peer[..spec.peer_len as usize]).unwrap_or("<peer>");
        let budget = Rc::new(Cell::new(HANDSHAKE_ATTEMPT_ROUNDS));
        match tls_handshake(
            TlsSessionIo {
                inner: TransportHandleIo(tcp),
                budget: Rc::clone(&budget),
            },
            &material,
            &peer,
        ) {
            Ok((stream, _)) => {
                // 会话期预算归一（一次超时即返回——68.5 数据面节奏保持）。
                budget.set(1);
                Ok(Box::new(TlsTransportHandle {
                    stream: Some(stream),
                    material,
                    budget,
                }))
            }
            Err(e) => Err(e),
        }
    }

    fn supports(&self, kind: TransportKind) -> bool {
        self.inner.supports(kind)
    }
}

/// TCP + TLS 完整握手共享路径（open 与 reconnect 同一实现——单一真相）。
/// 缓冲守卫在此获取；失败路径自动释放。成功 = TLS 会话在位（底层
/// socket 由 TlsConnection 内嵌持有，close 返还）。
fn tls_handshake(
    socket: TlsSessionIo,
    material: &TlsMaterial,
    peer: &str,
) -> Result<(EmbeddedTlsStream<'static, TlsSessionIo>, ()), TransportError> {
    // 单连接守卫（缓冲别名防线；单任务服务下正常不可达——显式错误
    // 优先于静默缓冲交错）。
    if TLS_BUF_BUSY
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        return Err(TransportError::Unsupported);
    }
    // SAFETY: 静态记录缓冲单线程独占（EL0 服务单任务；seL4 spawn 不
    // 多线程）+ `TLS_BUF_BUSY` AtomicBool 双重防线——两条 `&mut` 各自
    // 指向独立 static，无别名。
    let read_buf: &'static mut [u8] = unsafe { &mut TLS_READ_BUF };
    let write_buf: &'static mut [u8] = unsafe { &mut TLS_WRITE_BUF };
    let roots = alloc::vec![material.ca.clone()];
    let chain: Vec<&[u8]> = material.client_chain.iter().map(|c| c.as_slice()).collect();
    let provider = PkiProvider::with_shared_rng(
        roots,
        VerifyRole::Client,
        material.now,
        material.rng.clone(),
        Some(Identity {
            cert_chain: &chain,
            private_key: &material.client_key,
        }),
    );
    match EmbeddedTlsStream::open(
        socket,
        provider,
        Some(&material.server_name),
        read_buf,
        write_buf,
    ) {
        Ok(stream) => Ok((stream, ())),
        Err(e) => {
            release_guard();
            handshake_failed(peer, &alloc::format!("{e:?}"));
            Err(TransportError::ConnectFailed)
        }
    }
}

/// 握手失败日志（目标/宿主同词形；窗A 实弹教训：零日志握手失败不可
/// 诊断——fail-closed 必须可观测）。
fn handshake_failed(peer: &str, err: &str) {
    #[cfg(feature = "std")]
    eprintln!("[tls-target] handshake failed ({peer}): {err} — fail-closed 不降级明文");
    #[cfg(not(feature = "std"))]
    eneros_runtime::println!(
        "[tls-target] handshake failed ({peer}): {err} — fail-closed 不降级明文"
    );
}

#[cfg(all(test, feature = "embedded", feature = "std"))]
mod tests {
    use std::io::{Read as _, Write as _};

    use super::*;
    use crate::tests_support::{make_identity, NOW};

    /// 环回假件：直连宿主 TCP（真实 TLS 对端 = rustls 服务器线程）。
    struct HostTcpFactory {
        addr: std::net::SocketAddr,
    }
    impl TransportFactory for HostTcpFactory {
        fn open(
            &mut self,
            _spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            let s = std::net::TcpStream::connect(self.addr).expect("connect");
            // 测试面超时（阻塞读遇 EOF/超时须干净失败——embedded-tls
            // blocking 模式在无界阻塞 socket 上会永久等待）。
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(3)));
            let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(3)));
            Ok(Box::new(StdHandle { s: Some(s) }))
        }
        fn supports(&self, kind: TransportKind) -> bool {
            matches!(kind, TransportKind::Tcp)
        }
    }
    struct StdHandle {
        s: Option<std::net::TcpStream>,
    }
    impl TransportHandle for StdHandle {
        fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
            self.s
                .as_mut()
                .ok_or(TransportError::Closed)?
                .write_all(data)
                .map_err(|_| TransportError::Io)
        }
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
            self.s
                .as_mut()
                .ok_or(TransportError::Closed)?
                .read(buf)
                .map_err(|_| TransportError::Io)
        }
        fn close(&mut self) -> Result<(), TransportError> {
            self.s = None;
            Ok(())
        }
        fn is_connected(&self) -> bool {
            self.s.is_some()
        }
        fn reconnect(&mut self, _peer: &str) -> Result<(), TransportError> {
            Err(TransportError::Unsupported)
        }
        fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
            self
        }
    }

    fn spawn_rustls_server(
        listener: std::net::TcpListener,
        key: Vec<u8>,
        cert: Vec<u8>,
        expect: &'static [u8],
        reply: &'static [u8],
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let key = rustls::pki_types::PrivateKeyDer::Sec1(
                rustls::pki_types::PrivateSec1KeyDer::from(key),
            );
            let cert = rustls::pki_types::CertificateDer::from(cert);
            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![cert], key)
                .expect("server config");
            let (tcp, _) = listener.accept().expect("accept");
            let conn = rustls::ServerConnection::new(std::sync::Arc::new(config)).expect("conn");
            let mut stream = rustls::StreamOwned::new(conn, tcp);
            let mut buf = [0u8; 64];
            let n = stream.read(&mut buf).expect("read");
            assert_eq!(&buf[..n], expect);
            stream.write_all(reply).expect("write");
            stream.flush().expect("flush");
        })
    }

    fn material(ca: crate::tests_support::TestIdentity, name: &str) -> TlsMaterial {
        let chain = vec![ca.cert_der.clone()];
        TlsMaterial {
            ca: ca.x509.clone(),
            client_chain: chain,
            client_key: ca.key_der.clone(),
            server_name: String::from(name),
            now: NOW,
            rng: SharedCsRng(alloc::rc::Rc::new(core::cell::RefCell::new(
                eneros_crypto::rng::CsRng::from_seed(&[0x42; 32]),
            ))),
        }
    }

    /// 测试用 TransportSpec（TCP 定向；peer 仅日志用）。
    fn tcp_spec() -> TransportSpec {
        TransportSpec {
            kind: TransportKind::Tcp,
            peer: {
                let mut p = [0u8; 128];
                p[..9].copy_from_slice(b"127.0.0.1");
                p
            },
            peer_len: 9,
            mtu: 1500,
            connect_timeout_ms: 5_000,
            recv_timeout_ms: 5_000,
        }
    }

    /// 正例：工厂 open = TCP + 真实 TLS 1.3 握手 + 应用数据往返。
    #[test]
    fn factory_open_handshake_and_roundtrip() {
        let server_id = make_identity("eneros-broker", 0x21);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
            b"ping-tls",
            b"pong-tls",
        );
        let mut inner = HostTcpFactory { addr };
        let mat = RefCell::new(Some(material(server_id, "eneros-broker")));
        let mut factory = TlsTransportFactory::new(&mut inner, &mat);
        let mut h = factory.open(&tcp_spec()).expect("TLS open");
        h.send(b"ping-tls").expect("send");
        let mut buf = [0u8; 16];
        let n = h.recv(&mut buf).expect("recv");
        assert_eq!(&buf[..n], b"pong-tls");
        assert!(h.is_connected());
        h.close().expect("close");
        assert!(!h.is_connected());
        let _ = server.join();
    }

    /// 正例（根桥接）：服务器只发叶子证书（RFC 8446 允许省略根），
    /// 信任根经 verifier 桥接补路径——leaf+roots 形态握手成功。
    #[test]
    fn factory_open_bridges_root_from_leaf_only_chain() {
        use eneros_crypto::pki::parser::to_der;
        // leaf = CA 签发的 broker 证书（链上无根）。
        let ca_sk: p256::ecdsa::SigningKey =
            p256::ecdsa::SigningKey::from_bytes(&[0x33; 32].into()).unwrap();
        let ca_pk = eneros_crypto::pki::x509::SubjectPublicKey::EcdsaP256(
            ca_sk
                .verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        );
        let ca_dn = eneros_crypto::pki::x509::DistinguishedName::new("eneros-ca");
        let ca_dn2 = eneros_crypto::pki::x509::DistinguishedName::new("eneros-ca");
        let ca_dn3 = eneros_crypto::pki::x509::DistinguishedName::new("eneros-ca");
        let ca_dn4 = eneros_crypto::pki::x509::DistinguishedName::new("eneros-ca");
        let ca_cert = {
            use p256::ecdsa::signature::Signer;
            let tbs = eneros_crypto::pki::x509::X509Certificate::new(
                2,
                vec![1],
                eneros_crypto::pki::x509::SignatureAlgorithm::EcdsaWithSha256,
                ca_dn.clone(),
                ca_dn.clone(),
                crate::tests_support::NOW,
                crate::tests_support::NOW + 86400 * 365,
                ca_pk.clone(),
                Vec::new(),
                Vec::new(),
            );
            let tbs = tbs.encode_tbs().unwrap();
            let sig: p256::ecdsa::Signature = ca_sk.sign(&tbs);
            eneros_crypto::pki::x509::X509Certificate::new(
                2,
                vec![1],
                eneros_crypto::pki::x509::SignatureAlgorithm::EcdsaWithSha256,
                ca_dn.clone(),
                ca_dn.clone(),
                crate::tests_support::NOW,
                crate::tests_support::NOW + 86400 * 365,
                ca_pk,
                Vec::new(),
                sig.to_der().as_bytes().to_vec(),
            )
        };
        let broker_sk: p256::ecdsa::SigningKey =
            p256::ecdsa::SigningKey::from_bytes(&[0x21; 32].into()).unwrap();
        let broker_dn = eneros_crypto::pki::x509::DistinguishedName::new("eneros-broker");
        let broker_cert = {
            use p256::ecdsa::signature::Signer;
            let tbs = eneros_crypto::pki::x509::X509Certificate::new(
                2,
                vec![1],
                eneros_crypto::pki::x509::SignatureAlgorithm::EcdsaWithSha256,
                ca_dn2,
                broker_dn.clone(),
                crate::tests_support::NOW,
                crate::tests_support::NOW + 86400 * 365,
                eneros_crypto::pki::x509::SubjectPublicKey::EcdsaP256(
                    broker_sk
                        .verifying_key()
                        .to_encoded_point(false)
                        .as_bytes()
                        .to_vec(),
                ),
                Vec::new(),
                Vec::new(),
            );
            let tbs = tbs.encode_tbs().unwrap();
            let sig: p256::ecdsa::Signature = ca_sk.sign(&tbs);
            eneros_crypto::pki::x509::X509Certificate::new(
                2,
                vec![1],
                eneros_crypto::pki::x509::SignatureAlgorithm::EcdsaWithSha256,
                ca_dn3,
                broker_dn.clone(),
                crate::tests_support::NOW,
                crate::tests_support::NOW + 86400 * 365,
                eneros_crypto::pki::x509::SubjectPublicKey::EcdsaP256(
                    broker_sk
                        .verifying_key()
                        .to_encoded_point(false)
                        .as_bytes()
                        .to_vec(),
                ),
                Vec::new(),
                sig.to_der().as_bytes().to_vec(),
            )
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(
            listener,
            {
                // SEC1 私钥（RFC 5915）——scalar 0x21。
                let oid = [0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
                let point = broker_sk
                    .verifying_key()
                    .to_encoded_point(false)
                    .as_bytes()
                    .to_vec();
                let mut inner = vec![0x02, 0x01, 0x01, 0x04, 0x20];
                inner.extend_from_slice(&[0x21; 32]);
                inner.extend_from_slice(&[0xA0, 0x0A]);
                inner.extend_from_slice(&oid);
                inner.extend_from_slice(&[0xA1, 0x44, 0x03, 0x42, 0x00]);
                inner.extend_from_slice(&point);
                let mut k = vec![0x30, inner.len() as u8];
                k.extend(inner);
                k
            },
            to_der(&broker_cert).unwrap(),
            b"bridge",
            b"ok",
        );
        let mut inner = HostTcpFactory { addr };
        let mat = TlsMaterial {
            ca: ca_cert,
            client_chain: vec![to_der(&broker_cert).unwrap()],
            client_key: {
                let oid = [0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
                let point = broker_sk
                    .verifying_key()
                    .to_encoded_point(false)
                    .as_bytes()
                    .to_vec();
                let mut inner = vec![0x02, 0x01, 0x01, 0x04, 0x20];
                inner.extend_from_slice(&[0x21; 32]);
                inner.extend_from_slice(&[0xA0, 0x0A]);
                inner.extend_from_slice(&oid);
                inner.extend_from_slice(&[0xA1, 0x44, 0x03, 0x42, 0x00]);
                inner.extend_from_slice(&point);
                let mut k = vec![0x30, inner.len() as u8];
                k.extend(inner);
                k
            },
            server_name: String::from("eneros-broker"),
            now: crate::tests_support::NOW,
            rng: SharedCsRng(alloc::rc::Rc::new(core::cell::RefCell::new(
                eneros_crypto::rng::CsRng::from_seed(&[0x42; 32]),
            ))),
        };
        let mat_cell = RefCell::new(Some(mat));
        let mut factory = TlsTransportFactory::new(&mut inner, &mat_cell);
        let mut h = factory.open(&tcp_spec()).expect("leaf-only 桥接握手");
        h.send(b"bridge").expect("send");
        let mut buf = [0u8; 8];
        let n = h.recv(&mut buf).expect("recv");
        assert_eq!(&buf[..n], b"ok");
        let _ = server.join();
    }

    /// 探针：verify_chain 对 [leaf]+roots 桥接域直接验证。
    #[test]
    fn probe_verify_chain_bridge() {
        use eneros_crypto::pki::parser::to_der;
        use eneros_crypto::pki::verify::CertVerifier;
        use eneros_crypto::pki::x509::{
            DistinguishedName, SignatureAlgorithm, SubjectPublicKey, X509Certificate,
        };
        let ca_sk: p256::ecdsa::SigningKey =
            p256::ecdsa::SigningKey::from_bytes(&[0x33; 32].into()).unwrap();
        let ca_pk = SubjectPublicKey::EcdsaP256(
            ca_sk
                .verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        );
        let ca_dn = DistinguishedName::new("eneros-ca");
        // 构造器参数序 = (issuer, subject)——注意勿倒置（探针首版自咬）。
        let mk = |dn: &DistinguishedName,
                  issuer: &DistinguishedName,
                  pk: &SubjectPublicKey,
                  sign: bool| {
            use p256::ecdsa::signature::Signer;
            let unsigned = X509Certificate::new(
                2,
                vec![1],
                SignatureAlgorithm::EcdsaWithSha256,
                issuer.clone(),
                dn.clone(),
                crate::tests_support::NOW,
                crate::tests_support::NOW + 86400 * 365,
                pk.clone(),
                Vec::new(),
                Vec::new(),
            );
            let tbs = unsigned.encode_tbs().unwrap();
            let sig_der = if sign {
                let sig: p256::ecdsa::Signature = ca_sk.sign(&tbs);
                sig.to_der().as_bytes().to_vec()
            } else {
                Vec::new()
            };
            X509Certificate::new(
                2,
                vec![1],
                SignatureAlgorithm::EcdsaWithSha256,
                issuer.clone(),
                dn.clone(),
                crate::tests_support::NOW,
                crate::tests_support::NOW + 86400 * 365,
                pk.clone(),
                Vec::new(),
                sig_der,
            )
        };
        let ca_cert = mk(&ca_dn, &ca_dn, &ca_pk, true);
        let broker_sk: p256::ecdsa::SigningKey =
            p256::ecdsa::SigningKey::from_bytes(&[0x21; 32].into()).unwrap();
        let broker_pk = SubjectPublicKey::EcdsaP256(
            broker_sk
                .verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        );
        let broker_dn = DistinguishedName::new("eneros-broker");
        let broker_cert = mk(&broker_dn, &ca_dn, &broker_pk, true);
        let v = CertVerifier::new(vec![ca_cert.clone()]);
        eprintln!(
            "ca.subject={:?} broker.issuer={:?}",
            ca_cert.subject.cn, broker_cert.issuer.cn
        );
        // fullchain（显式双证书）→ Ok（桥接的前提路径）。
        let r = v.verify_chain(
            &[broker_cert.clone(), ca_cert.clone()],
            crate::tests_support::NOW,
        );
        eprintln!("verify_chain fullchain(显式): {r:?}");
        assert!(r.is_ok());
        // PkiVerifier 桥接语义（verify_certificate 1b 段）等价复算：
        // wire 只发叶子 + 信任根在位 → 桥接补根后验证通过。
        let client_roots = vec![ca_cert.clone()];
        let mut chain = vec![broker_cert.clone()];
        {
            let last = chain.last().expect("非空");
            let looks_root = last.issuer == last.subject
                && client_roots.iter().any(|r| r.subject == last.subject);
            if !looks_root {
                if let Some(root) = client_roots.iter().find(|r| r.subject == last.issuer) {
                    chain.push(root.clone());
                }
            }
        }
        assert_eq!(chain.len(), 2, "桥接必须补上根");
        let v2 = CertVerifier::new(client_roots);
        let r = v2.verify_chain(&chain, crate::tests_support::NOW);
        eprintln!("bridged chain verify: {r:?}");
        assert!(r.is_ok());
        // wire 往返（rustls 发出的 DER 必经 parse_der 重解析）。
        let broker_der = to_der(&broker_cert).unwrap();
        let parsed = eneros_crypto::pki::parser::parse_der(&broker_der);
        eprintln!(
            "parse_der roundtrip: {:?}",
            parsed.as_ref().map(|c| c.subject.cn.clone())
        );
        assert!(parsed.is_ok());
    }

    /// 探针（gate 真实材料）：openssl 产物 → parse → verify_certificate。
    /// 手工诊断面（依赖 gate 产物 product/tls——CI 不跑；gate 需要时加
    /// --ignored 执行）。
    #[test]
    #[ignore]
    fn probe_gate_openssl_material() {
        use eneros_crypto::pki::parser::to_der;

        use crate::embedded::DefaultCipherSuite;

        let dir = std::path::Path::new("../../../product/tls");
        let Ok(ca_der) = std::fs::read(dir.join("mqtt-ca.der")) else {
            panic!("材料不在位（应为 gate 产物）：{}", dir.display());
        };
        let client_der = std::fs::read(dir.join("mqtt-client.der")).unwrap();
        // 字节级 wire 往返：签名字节覆盖 TBS——编码器任何偏差 =
        // SignatureInvalid（openssl 产物互操作探针）。
        let ca_reparsed = eneros_crypto::pki::parser::parse_der(&ca_der).unwrap();
        let ca_reencoded = eneros_crypto::pki::parser::to_der(&ca_reparsed).unwrap();
        let leaf = eneros_crypto::pki::parser::parse_der(&client_der).unwrap();
        eprintln!(
            "CA roundtrip: in={} out={} identical={}",
            ca_der.len(),
            ca_reencoded.len(),
            ca_der == ca_reencoded
        );
        let leaf_reencoded = eneros_crypto::pki::parser::to_der(&leaf).unwrap();
        eprintln!(
            "leaf roundtrip: in={} out={} identical={}",
            client_der.len(),
            leaf_reencoded.len(),
            client_der == leaf_reencoded
        );
        if client_der != leaf_reencoded {
            eprintln!(
                "leaf in : {:02x?}",
                &client_der[..client_der.len().min(160)]
            );
            eprintln!(
                "leaf out: {:02x?}",
                &leaf_reencoded[..leaf_reencoded.len().min(160)]
            );
            eprintln!(
                "leaf in  tail: {:02x?}",
                &client_der[client_der.len().saturating_sub(80)..]
            );
            eprintln!(
                "leaf out tail: {:02x?}",
                &leaf_reencoded[leaf_reencoded.len().saturating_sub(80)..]
            );
        }
        let ca = eneros_crypto::pki::parser::parse_der(&ca_der).unwrap();
        let leaf = eneros_crypto::pki::parser::parse_der(&client_der).unwrap();
        eprintln!(
            "ca.subject={:?} leaf.issuer={:?} leaf.subject={:?}",
            ca.subject.cn, leaf.issuer.cn, leaf.subject.cn
        );
        eprintln!(
            "ca SPKI EcdsaP256={} leaf SPKI EcdsaP256={}",
            matches!(
                ca.public_key(),
                eneros_crypto::pki::x509::SubjectPublicKey::EcdsaP256(_)
            ),
            matches!(
                leaf.public_key(),
                eneros_crypto::pki::x509::SubjectPublicKey::EcdsaP256(_)
            )
        );
        // 实时钟（探针首版用固定旧时刻 → CertNotYetValid 假阴性自咬）。
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut provider = PkiProvider::<DefaultCipherSuite>::with_shared_rng(
            vec![ca.clone()],
            VerifyRole::Client,
            now,
            SharedCsRng(alloc::rc::Rc::new(core::cell::RefCell::new(
                eneros_crypto::rng::CsRng::from_seed(&[0x42; 32]),
            ))),
            None,
        );
        let verifier = provider.verifier().unwrap();
        use embedded_tls::blocking::{CryptoProvider, TlsCipherSuite, TlsVerifier};
        let transcript = <DefaultCipherSuite as TlsCipherSuite>::Hash::default();
        let r = {
            // 桥接已在 verify_certificate 内——wire 单叶子即可
            let mut cert_ref = embedded_tls::blocking::CertificateRef::with_context(&[]);
            cert_ref
                .entries
                .push(embedded_tls::blocking::CertificateEntryRef::X509(
                    &client_der,
                ));
            verifier.verify_certificate(&transcript, cert_ref)
        };
        eprintln!("verify_certificate(openssl leaf-only): {r:?}");
        assert!(r.is_ok());
    }

    /// 负例：错名（CN 不匹配）→ 握手失败（不降级明文）。
    #[test]
    fn factory_open_rejects_wrong_name() {
        let server_id = make_identity("eneros-broker", 0x21);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
            b"x",
            b"x",
        );
        let mut inner = HostTcpFactory { addr };
        let mat = RefCell::new(Some(material(server_id, "eneros-other-name")));
        let mut factory = TlsTransportFactory::new(&mut inner, &mat);
        let r = factory.open(&tcp_spec());
        assert!(r.is_err(), "错名必须握手失败");
        let _ = server.join();
    }

    /// 资源合同：close 后缓冲守卫释放（busy 位归零——open 重入不再
    /// Unsupported）。
    #[test]
    fn buffer_guard_released_after_close() {
        let server_id = make_identity("eneros-broker", 0x21);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
            b"a",
            b"b",
        );
        let mut inner = HostTcpFactory { addr };
        let mat = RefCell::new(Some(material(server_id, "eneros-broker")));
        let mut factory = TlsTransportFactory::new(&mut inner, &mat);
        let mut h = factory.open(&tcp_spec()).expect("first open");
        h.close().expect("close");
        assert!(!TLS_BUF_BUSY.load(Ordering::Acquire), "守卫必须已释放");
        let _ = server.join();
    }
}
