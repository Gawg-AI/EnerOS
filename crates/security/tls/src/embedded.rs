//! 设备端 TLS 1.3（蓝图/20 R1-3）：embedded-tls + 本仓 PKI 证书链验证。
//!
//! 安全硬性约束：
//! - **默认验证书链**：`PkiVerifier` 实现 embedded-tls 的 `TlsVerifier`，
//!   证书链验证走本仓 `crypto::pki`（X.509 DER 解析 + `CertVerifier::verify_chain`
//!   有效期/CRL/签名/SPKI 根比对）；
//! - **禁止任何 danger bypass**：绝不使用 embedded-tls 的 `NoVerify` /
//!   `UnsecureProvider`；verifier 缺失时握手直接失败；
//! - 握手签名仅接受 ECDSA P-256（`EcdsaSecp256r1Sha256`，RFC 8446 标准），
//!   其余方案一律拒绝；
//! - hostname 校验：客户端角色设置服务器名后必须与叶子证书 CN 一致；
//! - 会话密钥、私钥、签名值不进入任何日志路径。

extern crate alloc;

use core::marker::PhantomData;

use alloc::string::String;
use alloc::vec::Vec;

use embedded_tls::blocking::{
    Certificate, CertificateEntryRef, CertificateRef, CertificateVerifyRef, CryptoProvider,
    TlsCipherSuite, TlsConfig, TlsContext, TlsError, TlsVerifier,
};
use embedded_tls::SignatureScheme;
use digest::Digest;
use eneros_crypto::pki::parser::parse_der;
use eneros_crypto::pki::verify::CertVerifier;
use eneros_crypto::pki::x509::{SubjectPublicKey, X509Certificate};

/// 本 crate 使用的 TLS 1.3 密码套件（AES-128-GCM + SHA-256，RFC 8446 默认）。
pub type DefaultCipherSuite = embedded_tls::blocking::Aes128GcmSha256;

/// 验证者角色（决定 CertificateVerify 上下文串，RFC 8446 §4.4.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyRole {
    /// 客户端角色：验证服务器证书，上下文 `"TLS 1.3, server CertificateVerify"`，
    /// 且启用 hostname（CN）校验。
    Client,
    /// 服务器角色（mTLS）：验证客户端证书，上下文
    /// `"TLS 1.3, client CertificateVerify"`，不比对 CN（客户端身份由上层
    /// 按证书链验证结果 + 设备 ID 约定）。
    Server,
}

/// 设备端 TLS 证书链验证器（接入 embedded-tls 的 `TlsVerifier`）。
///
/// 证书链验证复用本仓 PKI：`parse_der` 解析（SM2 / ECDSA P-256 证书），
/// `CertVerifier::verify_chain` 做有效期、CRL、签名与 SPKI 根比对。
pub struct PkiVerifier<CS: TlsCipherSuite> {
    roots: Vec<X509Certificate>,
    role: VerifyRole,
    expected_hostname: Option<String>,
    now: u64,
    server_public_key: Option<Vec<u8>>,
    transcript: Option<CS::Hash>,
    _cs: PhantomData<CS>,
}

impl<CS: TlsCipherSuite> PkiVerifier<CS> {
    /// 创建验证器。
    ///
    /// # 参数
    /// - `roots`：信任根证书（本仓 `X509Certificate`，SPKI 根比对依据）
    /// - `role`：客户端 / 服务器角色（mTLS 双向）
    /// - `now`：当前 Unix 时间戳（秒），有效期校验基准；设备端由 RTC 提供
    pub fn new(roots: Vec<X509Certificate>, role: VerifyRole, now: u64) -> Self {
        Self {
            roots,
            role,
            expected_hostname: None,
            now,
            server_public_key: None,
            transcript: None,
            _cs: PhantomData,
        }
    }
}

impl<CS: TlsCipherSuite> TlsVerifier<CS> for PkiVerifier<CS> {
    fn set_hostname_verification(&mut self, hostname: &str) -> Result<(), TlsError> {
        self.expected_hostname = Some(String::from(hostname));
        Ok(())
    }

    fn verify_certificate(
        &mut self,
        transcript: &CS::Hash,
        cert: CertificateRef,
    ) -> Result<(), TlsError> {
        // 1. 解析证书链（本仓 parser：SM2 / ECDSA P-256 证书 DER）
        let mut chain: Vec<X509Certificate> = Vec::new();
        for entry in &cert.entries {
            match entry {
                CertificateEntryRef::X509(der) => {
                    let x509 = parse_der(der).map_err(|_| TlsError::InvalidCertificate)?;
                    chain.push(x509);
                }
                CertificateEntryRef::RawPublicKey(_) => {
                    return Err(TlsError::InvalidCertificateEntry);
                }
            }
        }
        if chain.is_empty() {
            return Err(TlsError::InvalidCertificate);
        }

        // 2. 链验证（本仓 CertVerifier：有效期 + CRL + SM2/ECDSA 签名 + SPKI 根比对）
        let verifier = CertVerifier::new(self.roots.clone());
        verifier
            .verify_chain(&chain, self.now)
            .map_err(|_| TlsError::InvalidCertificate)?;

        // 3. hostname 校验（客户端角色：服务器证书 CN 必须匹配）
        if self.role == VerifyRole::Client {
            if let Some(expected) = &self.expected_hostname {
                if &chain[0].subject.cn != expected {
                    return Err(TlsError::InvalidCertificate);
                }
            }
        }

        // 4. 保存叶子证书 ECDSA P-256 公钥（CertificateVerify 验签用）；
        //    非 ECDSA 公钥（如 SM2）拒绝——TLS 1.3 握手签名仅 ECDSA P-256。
        match chain[0].public_key() {
            SubjectPublicKey::EcdsaP256(point) => {
                self.server_public_key = Some(point.clone());
            }
            _ => return Err(TlsError::InvalidCertificate),
        }
        self.transcript = Some(transcript.clone());
        Ok(())
    }

    fn verify_signature(&mut self, verify: CertificateVerifyRef) -> Result<(), TlsError> {
        // 仅接受 ECDSA P-256（RFC 8446 §4.2.3 标准方案）
        if verify.signature_scheme != SignatureScheme::EcdsaSecp256r1Sha256 {
            return Err(TlsError::InvalidSignatureScheme);
        }
        let transcript = self.transcript.take().ok_or(TlsError::InvalidHandshake)?;
        let pk = self
            .server_public_key
            .take()
            .ok_or(TlsError::InvalidCertificate)?;

        // RFC 8446 §4.4.3：64×0x20 + 上下文串 + transcript_hash
        let ctx_str = match self.role {
            VerifyRole::Client => b"TLS 1.3, server CertificateVerify\x00",
            VerifyRole::Server => b"TLS 1.3, client CertificateVerify\x00",
        };
        let mut msg: Vec<u8> = Vec::with_capacity(64 + ctx_str.len() + 64);
        msg.resize(64, 0x20);
        msg.extend_from_slice(ctx_str);
        msg.extend_from_slice(&transcript.clone().finalize());

        let vk = p256::ecdsa::VerifyingKey::from_sec1_bytes(&pk)
            .map_err(|_| TlsError::InvalidCertificate)?;
        let sig = p256::ecdsa::Signature::from_der(verify.signature)
            .map_err(|_| TlsError::InvalidSignature)?;
        use p256::ecdsa::signature::Verifier;
        vk.verify(&msg, &sig).map_err(|_| TlsError::InvalidSignature)
    }
}

/// 证书（DER）与私钥（SEC1 DER）持有者，供 [`PkiProvider`] 使用。
pub struct Identity<'a> {
    /// 设备证书链（DER，叶子在前）
    pub cert_chain: &'a [&'a [u8]],
    /// 设备私钥（SEC1 DER，`EC PRIVATE KEY`，ECDSA P-256）
    pub private_key: &'a [u8],
}

/// 适配 `eneros_crypto::rng::CsRng`（本仓 CSRNG）到 `rand_core`（embedded-tls 要求）。
///
/// 生产路径：`CsRng` 必须由平台熵构造（`csrng_from_platform`），
/// 拒绝任何确定性种子注入（熵纪律 R1-4）。
pub struct CsRngAdapter(pub eneros_crypto::rng::CsRng);

impl rand_core::RngCore for CsRngAdapter {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.0.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }

    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.0.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }

    fn fill_bytes(&mut self, dst: &mut [u8]) {
        self.0.fill_bytes(dst);
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), rand_core::Error> {
        self.0.fill_bytes(dst);
        Ok(())
    }
}

impl rand_core::CryptoRng for CsRngAdapter {}

/// embedded-tls `CryptoProvider`：本仓 PKI 验证 + ECDSA P-256 签名 + mTLS 客户端证书。
pub struct PkiProvider<'a, CS: TlsCipherSuite> {
    verifier: PkiVerifier<CS>,
    identity: Option<Identity<'a>>,
    rng: CsRngAdapter,
}

impl<'a, CS: TlsCipherSuite> PkiProvider<'a, CS> {
    /// 创建设备端 provider。
    ///
    /// - `roots`：信任根（验证对端证书链）
    /// - `role`：客户端 / 服务器角色
    /// - `now`：当前时间戳
    /// - `rng`：本仓 CSRNG（必须由平台熵构造）
    /// - `identity`：本端证书与私钥（mTLS 双向认证必填；`None` 时客户端
    ///   不发证书、服务器端拒绝未认证客户端）
    pub fn new(
        roots: Vec<X509Certificate>,
        role: VerifyRole,
        now: u64,
        rng: eneros_crypto::rng::CsRng,
        identity: Option<Identity<'a>>,
    ) -> Self {
        Self {
            verifier: PkiVerifier::new(roots, role, now),
            identity,
            rng: CsRngAdapter(rng),
        }
    }
}

impl<'a, CS: TlsCipherSuite> CryptoProvider for PkiProvider<'a, CS> {
    type CipherSuite = CS;
    type Signature = p256::ecdsa::DerSignature;

    fn rng(&mut self) -> impl embedded_tls::CryptoRngCore {
        &mut self.rng
    }

    fn verifier(&mut self) -> Result<&mut impl TlsVerifier<Self::CipherSuite>, TlsError> {
        Ok(&mut self.verifier)
    }

    fn signer(
        &mut self,
    ) -> Result<
        (
            impl p256::ecdsa::signature::SignerMut<Self::Signature>,
            SignatureScheme,
        ),
        TlsError,
    > {
        let identity = self.identity.as_ref().ok_or(TlsError::InvalidPrivateKey)?;
        let scalar = parse_sec1_der_private_key(identity.private_key)?;
        let sk = p256::ecdsa::SigningKey::from_bytes(&scalar.into())
            .map_err(|_| TlsError::InvalidPrivateKey)?;
        Ok((sk, SignatureScheme::EcdsaSecp256r1Sha256))
    }

    fn client_cert(&mut self) -> Option<Certificate<impl AsRef<[u8]>>> {
        self.identity
            .as_ref()
            .map(|id| Certificate::X509(id.cert_chain[0]))
    }
}

/// 从 SEC1 DER（`ECPrivateKey`，RFC 5915）提取 32 字节私钥标量。
///
/// 结构：`SEQUENCE { INTEGER(1), OCTET STRING(key), [0] curve OID?, [1] pubkey? }`。
/// 自实现避免 elliptic-curve 的 pkcs8+sec1 双 feature 依赖（no_std 合规）。
pub fn parse_sec1_der_private_key(der: &[u8]) -> Result<[u8; 32], TlsError> {
    let mut p = 0usize;
    // SEQUENCE 头（支持短/长长度编码）
    if der.get(p) != Some(&0x30) {
        return Err(TlsError::InvalidPrivateKey);
    }
    p += 1;
    let len_byte = *der.get(p).ok_or(TlsError::InvalidPrivateKey)?;
    p += 1;
    if len_byte & 0x80 != 0 {
        let n = (len_byte & 0x7f) as usize;
        if n > 4 {
            return Err(TlsError::InvalidPrivateKey);
        }
        p += n;
    }
    // INTEGER version（跳过）
    if der.get(p) != Some(&0x02) {
        return Err(TlsError::InvalidPrivateKey);
    }
    p += 1;
    let il = *der.get(p).ok_or(TlsError::InvalidPrivateKey)? as usize;
    p += 1 + il;
    // OCTET STRING privateKey
    if der.get(p) != Some(&0x04) {
        return Err(TlsError::InvalidPrivateKey);
    }
    p += 1;
    let kl = *der.get(p).ok_or(TlsError::InvalidPrivateKey)? as usize;
    p += 1;
    if kl != 32 {
        return Err(TlsError::InvalidPrivateKey);
    }
    let end = p + 32;
    if end > der.len() {
        return Err(TlsError::InvalidPrivateKey);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&der[p..end]);
    Ok(key)
}

/// 设备端 TLS 连接包装（embedded-tls blocking 模式）。
///
/// `S` 为实现 `embedded_io::{Read, Write}` 的 socket（smoltcp 适配后）；
/// 记录缓冲由调用方持有（与连接同生命周期，避免自引用）。
pub struct EmbeddedTlsStream<'b, S: embedded_io::Read + embedded_io::Write> {
    conn: embedded_tls::blocking::TlsConnection<'b, S, DefaultCipherSuite>,
    _bufs: core::marker::PhantomData<&'b mut [u8]>,
}

impl<'b, S: embedded_io::Read + embedded_io::Write + 'b> EmbeddedTlsStream<'b, S> {
    /// 建立 TLS 连接（握手 + 证书链验证，[`PkiProvider`] 已含验证器）。
    ///
    /// # 参数
    /// - `socket`：底层 socket
    /// - `provider`：本仓 PKI provider（验证器 + 身份）
    /// - `server_name`：服务器名（客户端角色必填，服务器角色可 `None`）
    /// - `read_buf` / `write_buf`：记录缓冲（建议 ≥ 16640 / 4096 字节）
    pub fn open(
        socket: S,
        mut provider: PkiProvider<'_, DefaultCipherSuite>,
        server_name: Option<&str>,
        read_buf: &'b mut [u8],
        write_buf: &'b mut [u8],
    ) -> Result<Self, TlsError> {
        let config = if let Some(name) = server_name {
            TlsConfig::new().with_server_name(name)
        } else {
            TlsConfig::new()
        };
        let mut conn =
            embedded_tls::blocking::TlsConnection::new(socket, read_buf, write_buf);
        conn.open(TlsContext::new(&config, &mut provider))?;
        Ok(Self {
            conn,
            _bufs: core::marker::PhantomData,
        })
    }

    /// 读取解密后数据。
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, TlsError> {
        self.conn.read(buf)
    }

    /// 写入加密数据（未落盘）。
    pub fn write(&mut self, buf: &[u8]) -> Result<usize, TlsError> {
        self.conn.write(buf)
    }

    /// 刷出加密数据。
    pub fn flush(&mut self) -> Result<(), TlsError> {
        self.conn.flush()
    }

    /// 关闭连接（返回底层 socket）。
    pub fn close(self) -> Result<S, (S, TlsError)> {
        self.conn.close()
    }
}

// ============================================================================
// 测试：host 环回 TLS 1.3 握手（embedded-tls 客户端 + rustls 服务器）
// 蓝图/20 R1-3 验收：握手+往返 ✓；伪造证书/错误 CN 握手必败 ✓
// ============================================================================
#[cfg(all(test, feature = "embedded"))]
mod tests {
    use super::*;
    use eneros_crypto::pki::parser::to_der;
    use eneros_crypto::pki::x509::{DistinguishedName, SignatureAlgorithm};
    use p256::ecdsa::signature::Signer;
    use std::io::{Read as _, Write as _};

    const NOW: u64 = 1_700_000_000;

    /// 测试身份：本仓生成的 ECDSA P-256 自签证书 + SEC1 私钥。
    struct TestIdentity {
        cert_der: Vec<u8>,
        key_der: Vec<u8>,
        x509: X509Certificate,
        cn: String,
    }

    fn make_identity(cn: &str, scalar: u8) -> TestIdentity {
        let sk: p256::ecdsa::SigningKey =
            p256::ecdsa::SigningKey::from_bytes(&[scalar; 32].into()).unwrap();
        let pk = SubjectPublicKey::EcdsaP256(
            sk.verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        );
        let dn = DistinguishedName::new(cn);
        let unsigned = X509Certificate::new(
            2,
            vec![1],
            SignatureAlgorithm::EcdsaWithSha256,
            dn.clone(),
            dn.clone(),
            NOW,
            NOW + 86400 * 365,
            pk,
            Vec::new(),
            Vec::new(),
        );
        let tbs = unsigned.encode_tbs().unwrap();
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
            Vec::new(),
            sig.to_der().as_bytes().to_vec(),
        );
        let der = to_der(&cert).unwrap();
        // SEC1 DER（RFC 5915）：SEQUENCE { INTEGER(1), OCTET STRING(scalar),
        //   [0] prime256v1 OID, [1] BIT STRING 公钥 }——rustls 私钥解析器
        //   要求曲线参数 OID。
        let point = sk
            .verifying_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec();
        let oid = [0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07]; // prime256v1
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
            cn: cn.to_string(),
        }
    }

    fn provider<'a>(
        roots: Vec<X509Certificate>,
        role: VerifyRole,
        identity: Option<Identity<'a>>,
    ) -> PkiProvider<'a, DefaultCipherSuite> {
        PkiProvider::new(
            roots,
            role,
            NOW,
            eneros_crypto::rng::CsRng::from_seed(&[0x42; 32]),
            identity,
        )
    }

    /// rustls 服务器（host 侧，mTLS 简化：不要求客户端证书）。
    fn spawn_rustls_server(
        listener: std::net::TcpListener,
        server_key: Vec<u8>,
        server_cert: Vec<u8>,
    ) -> std::thread::JoinHandle<Vec<u8>> {
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
            assert_eq!(&buf[..n], b"ping", "服务器必须收到客户端明文 ping");
            stream.write_all(b"pong").expect("write pong");
            stream.flush().expect("flush");
            b"pong".to_vec()
        })
    }

    #[test]
    fn test_mtls_client_handshake_roundtrip() {
        // 客户端/服务器身份（本仓 ECDSA 证书 + SEC1 私钥）
        let client_id = make_identity("eneros-client", 0x42);
        let server_id = make_identity("eneros-server", 0x21);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(listener, server_id.key_der.clone(), server_id.cert_der.clone());

        // 客户端：embedded-tls + 本仓 PKI 验证（roots = 服务器自签根）
        let tcp = std::net::TcpStream::connect(addr).unwrap();
        let mut read_buf = vec![0u8; 16640];
        let mut write_buf = vec![0u8; 4096];
        let chain = [client_id.cert_der.clone()];
        let chain_refs: Vec<&[u8]> = chain.iter().map(|c| c.as_slice()).collect();
        let mut tls = EmbeddedTlsStream::open(
            embedded_io_adapters::std::FromStd::new(tcp),
            provider(
                vec![server_id.x509.clone()],
                VerifyRole::Client,
                Some(Identity {
                    cert_chain: &chain_refs,
                    private_key: &client_id.key_der,
                }),
            ),
            Some("eneros-server"),
            &mut read_buf,
            &mut write_buf,
        )
        .expect("TLS 1.3 握手 + 证书链验证必须成功（R1-3 验收）");
        drop(client_id);

        tls.write(b"ping").expect("write");
        tls.flush().expect("flush");
        let mut buf = [0u8; 64];
        let n = tls.read(&mut buf).expect("read pong");
        assert_eq!(&buf[..n], b"pong", "应用数据往返必须成功");
        let _ = tls.close();
        let _ = server.join().expect("server thread");
    }

    #[test]
    fn test_client_rejects_forged_cert() {
        // 伪造证书：另一把密钥签发的"eneros-server"证书
        let real_server = make_identity("eneros-server", 0x21);
        let forged_server = make_identity("eneros-server", 0x99);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // 服务器用伪造证书
        let server = spawn_rustls_server(
            listener,
            forged_server.key_der.clone(),
            forged_server.cert_der.clone(),
        );

        let tcp = std::net::TcpStream::connect(addr).unwrap();
        let mut read_buf = vec![0u8; 16640];
        let mut write_buf = vec![0u8; 4096];
        let result = EmbeddedTlsStream::open(
            embedded_io_adapters::std::FromStd::new(tcp),
            provider(vec![real_server.x509.clone()], VerifyRole::Client, None),
            Some("eneros-server"),
            &mut read_buf,
            &mut write_buf,
        );
        assert!(
            result.is_err(),
            "伪造证书（不同密钥签发）必须握手失败（R1-3 负例）"
        );
        // 服务器线程可能因客户端提前关闭而 panic——忽略其 join 结果
        let _ = server.join();
    }

    #[test]
    fn test_client_rejects_wrong_cn() {
        let server_id = make_identity("eneros-wrong-host", 0x21);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
        );

        let tcp = std::net::TcpStream::connect(addr).unwrap();
        let mut read_buf = vec![0u8; 16640];
        let mut write_buf = vec![0u8; 4096];
        // 服务器名必须与证书 CN 一致：eneros-wrong-host ≠ eneros-server
        let result = EmbeddedTlsStream::open(
            embedded_io_adapters::std::FromStd::new(tcp),
            provider(vec![server_id.x509.clone()], VerifyRole::Client, None),
            Some("eneros-server"),
            &mut read_buf,
            &mut write_buf,
        );
        assert!(
            result.is_err(),
            "错误 CN 必须握手失败（R1-3 负例）"
        );
        let _ = server.join();
    }

    #[test]
    fn test_sec1_private_key_parser() {
        let id = make_identity("eneros-sec1", 0x5a);
        let scalar = parse_sec1_der_private_key(&id.key_der).expect("SEC1 解析");
        assert_eq!(scalar, [0x5a; 32]);
        // 篡改/截断 → 拒绝
        assert!(parse_sec1_der_private_key(&id.key_der[..20]).is_err());
        let mut bad = id.key_der.clone();
        bad[0] = 0x31;
        assert!(parse_sec1_der_private_key(&bad).is_err());
    }
}
