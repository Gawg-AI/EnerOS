//! mTLS 双向认证握手状态机（蓝图 §4.3）.
//!
//! # 握手时序（双向模式，`verify_peer = true`）
//!
//! ```text
//! 客户端                                        服务端
//!   │  ── ClientHello(random_c, suites) ───────▶ │
//!   │ ◀─ ServerHello(random_s, suite) ───────── │
//!   │ ◀─ Certificate(server_cert) ───────────── │
//!   │ ◀─ CertRequest ────────────────────────── │
//!   │  ── Certificate(client_cert) ────────────▶ │
//!   │  ── KeyExchange(eph_pub_c ‖ sig_c) ──────▶ │
//!   │ ◀─ KeyExchange(eph_pub_s ‖ sig_s) ─────── │
//!   │  ── Finished(HMAC_client) ───────────────▶ │
//!   │ ◀─ Finished(HMAC_server) ──────────────── │
//! ```
//!
//! - 密钥交换：双方各自生成 SM2 临时密钥对（前向安全），线上临时公钥用
//!   本地证书私钥签名（证明私钥持有，防中间人）；`premaster = d_e · P_peer`
//!   的 x 坐标
//! - 主密钥：`master = SM3(premaster ‖ random_c ‖ random_s)`
//! - Finished：`HMAC-SM3(master, label ‖ random_c ‖ random_s)`，label 区分
//!   客户端/服务端方向
//! - 会话密钥：`SM3(master ‖ "session")` 前 16 字节（记录层再用
//!   `"enc"` / `"mac"` 标签二次派生，见 `record.rs`）
//!
//! `verify_peer = false` 为单向模式：跳过客户端证书交换与验证（蓝图 §4.3）。
//!
//! # no_std 合规
//! 仅使用 `core::*` / `alloc::*`，不依赖 `std::*`。

use alloc::vec::Vec;

use eneros_crypto::{
    ct_eq, hmac_sm3, parse_der, sm2_sign, sm2_verify, sm3_hash, to_der, CsRng, Sm2KeyPair,
    Sm2PrivateKey, Sm2PublicKey, Sm2Signature, Sm3Hasher, SubjectPublicKey, X509Certificate,
};

use crate::cert_mgr::CertManager;
use crate::cipher_suite::{negotiate, SmCipherSuite};
use crate::{MtlsTransport, TlsError, TlsStats};

// ============================================================
// 线上帧格式（type(1) ‖ len(2 BE) ‖ payload）
// ============================================================

/// ClientHello 消息类型.
pub(crate) const MSG_CLIENT_HELLO: u8 = 0x01;
/// ServerHello 消息类型.
pub(crate) const MSG_SERVER_HELLO: u8 = 0x02;
/// Certificate 消息类型（payload 为证书 DER）.
pub(crate) const MSG_CERTIFICATE: u8 = 0x03;
/// CertRequest 消息类型（payload 为空）.
pub(crate) const MSG_CERT_REQUEST: u8 = 0x04;
/// KeyExchange 消息类型（payload = 临时公钥 65B ‖ SM2 签名 64B）.
pub(crate) const MSG_KEY_EXCHANGE: u8 = 0x05;
/// Finished 消息类型（payload = SM3-HMAC 32B）.
pub(crate) const MSG_FINISHED: u8 = 0x06;

/// Finished 标签：客户端方向.
const LABEL_CLIENT_FINISHED: &[u8] = b"client finished";
/// Finished 标签：服务端方向.
const LABEL_SERVER_FINISHED: &[u8] = b"server finished";

/// 发送一个消息帧.
fn send_msg(transport: &mut impl MtlsTransport, ty: u8, payload: &[u8]) -> Result<(), TlsError> {
    let mut frame = Vec::with_capacity(3 + payload.len());
    frame.push(ty);
    frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    frame.extend_from_slice(payload);
    transport.send(&frame)
}

/// 接收并校验一个消息帧（类型不符 / 帧头损坏 → [`TlsError::InvalidMessage`]）.
fn recv_msg(transport: &mut impl MtlsTransport, expect: u8) -> Result<Vec<u8>, TlsError> {
    let frame = transport.recv()?;
    if frame.len() < 3 || frame[0] != expect {
        return Err(TlsError::InvalidMessage);
    }
    let len = u16::from_be_bytes([frame[1], frame[2]]) as usize;
    if frame.len() != 3 + len {
        return Err(TlsError::InvalidMessage);
    }
    Ok(frame[3..].to_vec())
}

/// 从证书中提取 SM2 公钥（非 SM2 → [`TlsError::InternalError`]，国密-only 路径）.
fn cert_sm2_pk(cert: &X509Certificate) -> Result<Sm2PublicKey, TlsError> {
    match cert.public_key() {
        SubjectPublicKey::Sm2(pk) => Ok(*pk),
        SubjectPublicKey::Rsa(_) => Err(TlsError::InternalError),
    }
}

/// 计算主密钥：SM3(premaster ‖ random_c ‖ random_s).
fn derive_master_secret(
    premaster: &[u8; 32],
    random_c: &[u8; 32],
    random_s: &[u8; 32],
) -> [u8; 32] {
    let mut h = Sm3Hasher::new();
    h.update(premaster);
    h.update(random_c);
    h.update(random_s);
    h.finalize()
}

/// 计算 Finished 校验值：HMAC-SM3(master, label ‖ random_c ‖ random_s).
fn finished_mac(
    master: &[u8; 32],
    label: &[u8],
    random_c: &[u8; 32],
    random_s: &[u8; 32],
) -> [u8; 32] {
    let mut msg = Vec::with_capacity(label.len() + 64);
    msg.extend_from_slice(label);
    msg.extend_from_slice(random_c);
    msg.extend_from_slice(random_s);
    hmac_sm3(master, &msg)
}

/// 派生会话密钥：SM3(master ‖ "session") 前 16 字节.
fn derive_session_key(master: &[u8; 32]) -> [u8; 16] {
    let mut h = Sm3Hasher::new();
    h.update(master);
    h.update(b"session");
    let digest = h.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

/// 密钥交换签名消息：random_c ‖ random_s ‖ eph_pub（绑定握手双方随机数，
/// 防重放与中间人替换临时公钥）.
fn kx_signed_msg(random_c: &[u8; 32], random_s: &[u8; 32], eph_pub: &[u8; 65]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(129);
    msg.extend_from_slice(random_c);
    msg.extend_from_slice(random_s);
    msg.extend_from_slice(eph_pub);
    msg
}

// ============================================================
// MtlsContext / HandshakeOutcome
// ============================================================

/// mTLS 握手上下文（本地身份 + 信任配置 + 套件列表 + 统计）.
///
/// 偏差说明：tasks 中 `ca` 字段落地为 [`CertManager`]（信任根集合 + 可选
/// CRL），以支撑 HS11 客户端吊销拒绝场景；单根 CA 即 `CertManager` 中唯一
/// 信任根。
pub struct MtlsContext {
    /// 本地证书（CA 签发的叶子证书）.
    pub local_cert: X509Certificate,
    /// 本地证书对应私钥（密钥交换签名用）.
    pub local_key: Sm2PrivateKey,
    /// 证书管理器（信任根 + 可选 CRL）.
    pub cert_mgr: CertManager,
    /// 是否验证对端证书的服务端侧开关：`true` 双向认证（默认），`false`
    /// 单向模式（跳过客户端证书交换与验证）.
    pub verify_peer: bool,
    /// 本地支持的密码套件列表（服务端为优先序）.
    pub cipher_suites: Vec<SmCipherSuite>,
    /// 可观测统计.
    pub stats: TlsStats,
}

/// 握手产物（会话密钥 + 协商套件 + 对端证书指纹）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeOutcome {
    /// 会话主密钥（16 字节；记录层二次派生 enc/mac 密钥）.
    pub session_key: [u8; 16],
    /// 协商出的密码套件.
    pub suite: SmCipherSuite,
    /// 对端证书 SM3 指纹（DER 摘要；单向模式下服务端侧为全零）.
    pub peer_cert_fingerprint: [u8; 32],
}

impl MtlsContext {
    /// 创建握手上下文.
    ///
    /// # 参数
    /// - `local_cert` / `local_key`：本地证书与私钥
    /// - `trusted_roots`：信任根列表（通常含唯一 CA 根证书）
    /// - `verify_peer`：双向/单向模式开关（两端配置必须一致）
    /// - `cipher_suites`：本地支持的套件列表
    pub fn new(
        local_cert: X509Certificate,
        local_key: Sm2PrivateKey,
        trusted_roots: Vec<X509Certificate>,
        verify_peer: bool,
        cipher_suites: Vec<SmCipherSuite>,
    ) -> Self {
        Self {
            local_cert,
            local_key,
            cert_mgr: CertManager::new(trusted_roots),
            verify_peer,
            cipher_suites,
            stats: TlsStats::default(),
        }
    }

    /// 记录拒绝并返回错误（stats.rejected + 1 + last_error）.
    fn reject(&mut self, e: TlsError) -> TlsError {
        self.stats.rejected += 1;
        self.stats.last_error = Some(e);
        e
    }

    // ========================================================
    // 客户端握手
    // ========================================================

    /// 客户端握手：验证服务端证书 → 密钥交换 → Finished 校验 → 会话密钥.
    pub fn handshake_client(
        &mut self,
        transport: &mut impl MtlsTransport,
        rng: &mut CsRng,
        now: u64,
    ) -> Result<HandshakeOutcome, TlsError> {
        // 1. ClientHello：random_c ‖ 套件列表
        let mut random_c = [0u8; 32];
        rng.fill_bytes(&mut random_c);
        let mut hello = Vec::with_capacity(32 + 3 * self.cipher_suites.len());
        hello.extend_from_slice(&random_c);
        for suite in &self.cipher_suites {
            hello.extend_from_slice(&suite.to_bytes());
        }
        send_msg(transport, MSG_CLIENT_HELLO, &hello)?;

        // 2. ServerHello：random_s ‖ 选定套件
        let sh = recv_msg(transport, MSG_SERVER_HELLO)?;
        if sh.len() != 35 {
            return Err(self.reject(TlsError::InvalidMessage));
        }
        let mut random_s = [0u8; 32];
        random_s.copy_from_slice(&sh[..32]);
        let suite = SmCipherSuite::from_bytes([sh[32], sh[33], sh[34]])
            .ok_or_else(|| self.reject(TlsError::InvalidMessage))?;
        if !self.cipher_suites.contains(&suite) {
            // 服务端选定了客户端未声明的套件（协议违规）
            return Err(self.reject(TlsError::HandshakeFailed));
        }

        // 3. Certificate：服务端证书（验签 → 有效期 → CRL）
        let cert_payload = recv_msg(transport, MSG_CERTIFICATE)?;
        let server_cert =
            parse_der(&cert_payload).map_err(|_| self.reject(TlsError::InvalidMessage))?;
        if let Err(ce) = self.cert_mgr.verify_cert(&server_cert, now) {
            return Err(self.reject(TlsError::CertInvalid(ce)));
        }
        let server_pk = cert_sm2_pk(&server_cert)?;

        // 4. 双向模式：CertRequest → 回送客户端证书
        if self.verify_peer {
            let _ = recv_msg(transport, MSG_CERT_REQUEST)?;
            let der = to_der(&self.local_cert).map_err(|_| TlsError::InternalError)?;
            send_msg(transport, MSG_CERTIFICATE, &der)?;
        }

        // 5. KeyExchange：临时 SM2 密钥对 + 本地私钥签名（证明私钥持有）
        let eph = Sm2KeyPair::generate(rng).map_err(|_| TlsError::InternalError)?;
        let eph_pub_bytes = eph.public_key.to_bytes_uncompressed();
        let local_pk = cert_sm2_pk(&self.local_cert)?;
        let sig_msg = kx_signed_msg(&random_c, &random_s, &eph_pub_bytes);
        let sig = sm2_sign(&sig_msg, &self.local_key, &local_pk, rng)
            .map_err(|_| TlsError::InternalError)?;
        let mut kx = Vec::with_capacity(129);
        kx.extend_from_slice(&eph_pub_bytes);
        kx.extend_from_slice(&sig.to_bytes());
        send_msg(transport, MSG_KEY_EXCHANGE, &kx)?;

        // 6. 服务端 KeyExchange：验签（用服务端证书公钥）
        let skx = recv_msg(transport, MSG_KEY_EXCHANGE)?;
        if skx.len() != 129 {
            return Err(self.reject(TlsError::InvalidMessage));
        }
        let mut server_eph_bytes = [0u8; 65];
        server_eph_bytes.copy_from_slice(&skx[..65]);
        let server_eph_pub = Sm2PublicKey::from_bytes(&server_eph_bytes)
            .map_err(|_| self.reject(TlsError::HandshakeFailed))?;
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&skx[65..]);
        let server_sig = Sm2Signature::from_bytes(&sig_bytes);
        let server_sig_msg = kx_signed_msg(&random_c, &random_s, &server_eph_bytes);
        match sm2_verify(&server_sig_msg, &server_sig, &server_pk) {
            Ok(true) => {}
            _ => return Err(self.reject(TlsError::HandshakeFailed)),
        }

        // 7. premaster = d_e · P_server_eph 的 x 坐标
        let shared = server_eph_pub.point.scalar_mult(&eph.private_key.d);
        if shared.is_infinity {
            return Err(self.reject(TlsError::HandshakeFailed));
        }
        let premaster = shared.x.to_be_bytes();
        let master = derive_master_secret(&premaster, &random_c, &random_s);

        // 8. Finished：先发客户端 HMAC，再校验服务端 HMAC
        let client_fin = finished_mac(&master, LABEL_CLIENT_FINISHED, &random_c, &random_s);
        send_msg(transport, MSG_FINISHED, &client_fin)?;
        let server_fin = recv_msg(transport, MSG_FINISHED)?;
        let expected = finished_mac(&master, LABEL_SERVER_FINISHED, &random_c, &random_s);
        if server_fin.len() != 32 || !ct_eq(&server_fin, &expected) {
            return Err(self.reject(TlsError::HandshakeFailed));
        }

        // 9. 派生会话密钥 + 对端证书指纹
        self.stats.handshakes += 1;
        Ok(HandshakeOutcome {
            session_key: derive_session_key(&master),
            suite,
            peer_cert_fingerprint: sm3_hash(&cert_payload),
        })
    }

    // ========================================================
    // 服务端握手
    // ========================================================

    /// 服务端握手：套件协商 → 证书交换 → 互验 → 密钥交换 → Finished.
    pub fn handshake_server(
        &mut self,
        transport: &mut impl MtlsTransport,
        rng: &mut CsRng,
        now: u64,
    ) -> Result<HandshakeOutcome, TlsError> {
        // 1. ClientHello：random_c ‖ 客户端套件列表
        let ch = recv_msg(transport, MSG_CLIENT_HELLO)?;
        if ch.len() < 32 || (ch.len() - 32) % 3 != 0 {
            return Err(self.reject(TlsError::InvalidMessage));
        }
        let mut random_c = [0u8; 32];
        random_c.copy_from_slice(&ch[..32]);
        let mut client_suites = Vec::with_capacity((ch.len() - 32) / 3);
        for chunk in ch[32..].chunks(3) {
            let suite = SmCipherSuite::from_bytes([chunk[0], chunk[1], chunk[2]])
                .ok_or_else(|| self.reject(TlsError::InvalidMessage))?;
            client_suites.push(suite);
        }

        // 2. 套件协商（服务端优先序）
        let suite = match negotiate(&client_suites, &self.cipher_suites) {
            Ok(s) => s,
            Err(e) => return Err(self.reject(e)),
        };

        // 3. ServerHello + Certificate +（双向模式）CertRequest
        let mut random_s = [0u8; 32];
        rng.fill_bytes(&mut random_s);
        let mut sh = Vec::with_capacity(35);
        sh.extend_from_slice(&random_s);
        sh.extend_from_slice(&suite.to_bytes());
        send_msg(transport, MSG_SERVER_HELLO, &sh)?;
        let local_der = to_der(&self.local_cert).map_err(|_| TlsError::InternalError)?;
        send_msg(transport, MSG_CERTIFICATE, &local_der)?;
        if self.verify_peer {
            send_msg(transport, MSG_CERT_REQUEST, &[])?;
        }

        // 4. 双向模式：接收并验证客户端证书
        let (client_cert, peer_fingerprint) = if self.verify_peer {
            let cert_payload = recv_msg(transport, MSG_CERTIFICATE)?;
            let cert =
                parse_der(&cert_payload).map_err(|_| self.reject(TlsError::InvalidMessage))?;
            if let Err(ce) = self.cert_mgr.verify_cert(&cert, now) {
                return Err(self.reject(TlsError::CertInvalid(ce)));
            }
            (Some(cert), sm3_hash(&cert_payload))
        } else {
            (None, [0u8; 32])
        };

        // 5. 客户端 KeyExchange：双向模式下用客户端证书公钥验签
        let ckx = recv_msg(transport, MSG_KEY_EXCHANGE)?;
        if ckx.len() != 129 {
            return Err(self.reject(TlsError::InvalidMessage));
        }
        let mut client_eph_bytes = [0u8; 65];
        client_eph_bytes.copy_from_slice(&ckx[..65]);
        let client_eph_pub = Sm2PublicKey::from_bytes(&client_eph_bytes)
            .map_err(|_| self.reject(TlsError::HandshakeFailed))?;
        if let Some(ref cert) = client_cert {
            let client_pk = cert_sm2_pk(cert)?;
            let mut sig_bytes = [0u8; 64];
            sig_bytes.copy_from_slice(&ckx[65..]);
            let client_sig = Sm2Signature::from_bytes(&sig_bytes);
            let client_sig_msg = kx_signed_msg(&random_c, &random_s, &client_eph_bytes);
            match sm2_verify(&client_sig_msg, &client_sig, &client_pk) {
                Ok(true) => {}
                _ => return Err(self.reject(TlsError::HandshakeFailed)),
            }
        }

        // 6. 服务端 KeyExchange：临时密钥对 + 服务端私钥签名
        let eph = Sm2KeyPair::generate(rng).map_err(|_| TlsError::InternalError)?;
        let eph_pub_bytes = eph.public_key.to_bytes_uncompressed();
        let local_pk = cert_sm2_pk(&self.local_cert)?;
        let sig_msg = kx_signed_msg(&random_c, &random_s, &eph_pub_bytes);
        let sig = sm2_sign(&sig_msg, &self.local_key, &local_pk, rng)
            .map_err(|_| TlsError::InternalError)?;
        let mut skx = Vec::with_capacity(129);
        skx.extend_from_slice(&eph_pub_bytes);
        skx.extend_from_slice(&sig.to_bytes());
        send_msg(transport, MSG_KEY_EXCHANGE, &skx)?;

        // 7. premaster = d_e · P_client_eph 的 x 坐标
        let shared = client_eph_pub.point.scalar_mult(&eph.private_key.d);
        if shared.is_infinity {
            return Err(self.reject(TlsError::HandshakeFailed));
        }
        let premaster = shared.x.to_be_bytes();
        let master = derive_master_secret(&premaster, &random_c, &random_s);

        // 8. Finished：先校验客户端 HMAC，再回送服务端 HMAC
        let client_fin = recv_msg(transport, MSG_FINISHED)?;
        let expected = finished_mac(&master, LABEL_CLIENT_FINISHED, &random_c, &random_s);
        if client_fin.len() != 32 || !ct_eq(&client_fin, &expected) {
            return Err(self.reject(TlsError::HandshakeFailed));
        }
        let server_fin = finished_mac(&master, LABEL_SERVER_FINISHED, &random_c, &random_s);
        send_msg(transport, MSG_FINISHED, &server_fin)?;

        // 9. 派生会话密钥
        self.stats.handshakes += 1;
        Ok(HandshakeOutcome {
            session_key: derive_session_key(&master),
            suite,
            peer_cert_fingerprint: peer_fingerprint,
        })
    }
}

// ============================================================
// Unit Tests（std 线程 + mpsc 通道传输，测试模块内允许 std）
// ============================================================

#[cfg(test)]
mod tests {
    use alloc::vec;
    use std::sync::mpsc;
    use std::thread;

    use eneros_crypto::{
        build_certificate, build_self_signed, CertRequest, Crl, DistinguishedName,
        RevocationReason, RevokedCert,
    };

    use super::*;
    use crate::cipher_suite::{Cipher, KeyExchange, MacAlgorithm};
    use crate::CertError;

    const NOW: u64 = 1_700_000_000;
    const DAY: u64 = 86_400;

    const SUITE: SmCipherSuite =
        SmCipherSuite::new(KeyExchange::Sm2Dhe, Cipher::Sm4Gcm, MacAlgorithm::Sm3Hmac);

    /// 基于 std::sync::mpsc 的双向通道传输（支持 Finished 篡改注入）.
    struct ChannelTransport {
        tx: mpsc::Sender<Vec<u8>>,
        rx: mpsc::Receiver<Vec<u8>>,
        tamper_finished: bool,
    }

    impl MtlsTransport for ChannelTransport {
        fn send(&mut self, data: &[u8]) -> Result<(), TlsError> {
            let mut d = data.to_vec();
            if self.tamper_finished && !d.is_empty() && d[0] == MSG_FINISHED {
                let n = d.len();
                d[n - 1] ^= 0xFF;
            }
            self.tx.send(d).map_err(|_| TlsError::TransportError)
        }

        fn recv(&mut self) -> Result<Vec<u8>, TlsError> {
            self.rx.recv().map_err(|_| TlsError::TransportError)
        }
    }

    /// 测试 PKI：CA + 服务端证书 + 客户端证书.
    struct TestPki {
        ca_cert: X509Certificate,
        server_cert: X509Certificate,
        server_kp: Sm2KeyPair,
        client_cert: X509Certificate,
        client_kp: Sm2KeyPair,
    }

    fn make_pki(now: u64, server_validity_days: u32) -> TestPki {
        let mut rng = CsRng::new();
        let ca_kp = Sm2KeyPair::generate(&mut rng).expect("CA 密钥对");
        let ca_subject = DistinguishedName::new("EnerOS mTLS Test CA")
            .with_o("EnerOS")
            .with_c("CN");
        let ca_req = CertRequest::new(ca_subject, SubjectPublicKey::Sm2(ca_kp.public_key));
        let ca_cert = build_self_signed(
            &ca_req,
            &ca_kp.private_key,
            &ca_kp.public_key,
            now,
            &mut rng,
        )
        .expect("CA 证书");

        let server_kp = Sm2KeyPair::generate(&mut rng).expect("服务端密钥对");
        let server_req = CertRequest::new(
            DistinguishedName::new("mtls-server")
                .with_o("EnerOS")
                .with_c("CN"),
            SubjectPublicKey::Sm2(server_kp.public_key),
        )
        .with_validity_days(server_validity_days);
        let server_cert = build_certificate(
            &server_req,
            &ca_cert.subject,
            &ca_kp.private_key,
            &ca_kp.public_key,
            &[0x10],
            now,
            &mut rng,
        )
        .expect("服务端证书");

        let client_kp = Sm2KeyPair::generate(&mut rng).expect("客户端密钥对");
        let client_req = CertRequest::new(
            DistinguishedName::new("mtls-client")
                .with_o("EnerOS")
                .with_c("CN"),
            SubjectPublicKey::Sm2(client_kp.public_key),
        )
        .with_validity_days(365);
        let client_cert = build_certificate(
            &client_req,
            &ca_cert.subject,
            &ca_kp.private_key,
            &ca_kp.public_key,
            &[0x20],
            now,
            &mut rng,
        )
        .expect("客户端证书");

        TestPki {
            ca_cert,
            server_cert,
            server_kp,
            client_cert,
            client_kp,
        }
    }

    /// 服务端线程入口：返回（握手结果，最终统计）.
    fn run_server(
        mut ctx: MtlsContext,
        rx: mpsc::Receiver<Vec<u8>>,
        tx: mpsc::Sender<Vec<u8>>,
        now: u64,
    ) -> (Result<HandshakeOutcome, TlsError>, TlsStats) {
        let mut transport = ChannelTransport {
            tx,
            rx,
            tamper_finished: false,
        };
        let mut rng = CsRng::new();
        let res = ctx.handshake_server(&mut transport, &mut rng, now);
        (res, ctx.stats)
    }

    /// HS9：双向认证成功 —— 双方证书有效，派生相同会话密钥与套件.
    #[test]
    fn hs9_mutual_handshake_success() {
        let pki = make_pki(NOW, 365);
        let server_der = to_der(&pki.server_cert).expect("服务端 DER");
        let client_der = to_der(&pki.client_cert).expect("客户端 DER");

        let server_ctx = MtlsContext::new(
            pki.server_cert.clone(),
            pki.server_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );
        let mut client_ctx = MtlsContext::new(
            pki.client_cert.clone(),
            pki.client_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );

        let (c2s_tx, c2s_rx) = mpsc::channel::<Vec<u8>>();
        let (s2c_tx, s2c_rx) = mpsc::channel::<Vec<u8>>();
        let handle = thread::spawn(move || run_server(server_ctx, c2s_rx, s2c_tx, NOW));

        let mut transport = ChannelTransport {
            tx: c2s_tx,
            rx: s2c_rx,
            tamper_finished: false,
        };
        let mut rng = CsRng::new();
        let client_res = client_ctx.handshake_client(&mut transport, &mut rng, NOW);
        let (server_res, server_stats) = handle.join().expect("服务端线程");

        let client_out = client_res.expect("客户端握手应成功");
        let server_out = server_res.expect("服务端握手应成功");
        assert_eq!(
            client_out.session_key, server_out.session_key,
            "双方会话密钥必须一致"
        );
        assert_eq!(client_out.suite, server_out.suite, "双方套件必须一致");
        assert_eq!(
            client_out.peer_cert_fingerprint,
            sm3_hash(&server_der),
            "客户端侧对端指纹应为服务端证书摘要"
        );
        assert_eq!(
            server_out.peer_cert_fingerprint,
            sm3_hash(&client_der),
            "服务端侧对端指纹应为客户端证书摘要"
        );
        assert_eq!(client_ctx.stats.handshakes, 1);
        assert_eq!(server_stats.handshakes, 1);
        assert_eq!(client_ctx.stats.rejected, 0);
    }

    /// HS10：服务端证书过期 → 客户端拒绝（CertInvalid(Expired) + rejected）.
    #[test]
    fn hs10_expired_server_cert_rejected() {
        // 服务端证书有效期仅 1 天，握手发生在 2 天后
        let pki = make_pki(NOW, 1);
        let later = NOW + 2 * DAY;

        let server_ctx = MtlsContext::new(
            pki.server_cert.clone(),
            pki.server_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );
        let mut client_ctx = MtlsContext::new(
            pki.client_cert.clone(),
            pki.client_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );

        let (c2s_tx, c2s_rx) = mpsc::channel::<Vec<u8>>();
        let (s2c_tx, s2c_rx) = mpsc::channel::<Vec<u8>>();
        let handle = thread::spawn(move || run_server(server_ctx, c2s_rx, s2c_tx, later));

        let mut transport = ChannelTransport {
            tx: c2s_tx,
            rx: s2c_rx,
            tamper_finished: false,
        };
        let mut rng = CsRng::new();
        let client_res = client_ctx.handshake_client(&mut transport, &mut rng, later);
        // 显式断开客户端→服务端通道：客户端先拒绝（证书过期）后不再发帧，
        // 释放 c2s_tx 使服务端阻塞的 recv 立即返回 TransportError，防测试死锁
        drop(transport);
        let _ = handle.join();

        assert_eq!(
            client_res,
            Err(TlsError::CertInvalid(CertError::Expired)),
            "过期服务端证书应被客户端拒绝"
        );
        assert_eq!(client_ctx.stats.rejected, 1, "拒绝计数应 +1");
        assert_eq!(
            client_ctx.stats.last_error,
            Some(TlsError::CertInvalid(CertError::Expired)),
            "last_error 应记录 Expired"
        );
    }

    /// HS11：客户端证书被吊销 → 服务端拒绝（CertInvalid(Revoked) + rejected）.
    #[test]
    fn hs11_revoked_client_cert_rejected() {
        let pki = make_pki(NOW, 365);

        let mut server_ctx = MtlsContext::new(
            pki.server_cert.clone(),
            pki.server_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );
        // 服务端加载含客户端序列号的 CRL
        let mut crl = Crl::new(pki.ca_cert.subject.clone(), NOW + 30 * DAY);
        crl.add_revoked(RevokedCert::new(
            &pki.client_cert.serial_number,
            NOW,
            RevocationReason::KeyCompromise,
        ));
        server_ctx.cert_mgr.load_crl(crl);

        let mut client_ctx = MtlsContext::new(
            pki.client_cert.clone(),
            pki.client_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );

        let (c2s_tx, c2s_rx) = mpsc::channel::<Vec<u8>>();
        let (s2c_tx, s2c_rx) = mpsc::channel::<Vec<u8>>();
        let handle = thread::spawn(move || run_server(server_ctx, c2s_rx, s2c_tx, NOW));

        let mut transport = ChannelTransport {
            tx: c2s_tx,
            rx: s2c_rx,
            tamper_finished: false,
        };
        let mut rng = CsRng::new();
        let _client_res = client_ctx.handshake_client(&mut transport, &mut rng, NOW);
        let (server_res, server_stats) = handle.join().expect("服务端线程");

        assert_eq!(
            server_res,
            Err(TlsError::CertInvalid(CertError::Revoked)),
            "被吊销客户端证书应被服务端拒绝"
        );
        assert_eq!(server_stats.rejected, 1, "服务端拒绝计数应 +1");
        assert_eq!(
            server_stats.last_error,
            Some(TlsError::CertInvalid(CertError::Revoked)),
            "last_error 应记录 Revoked"
        );
    }

    /// HS12：单向模式（verify_peer=false）—— 跳过客户端证书验证，握手成功.
    #[test]
    fn hs12_one_way_mode_success() {
        let pki = make_pki(NOW, 365);

        let server_ctx = MtlsContext::new(
            pki.server_cert.clone(),
            pki.server_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            false,
            vec![SUITE],
        );
        let mut client_ctx = MtlsContext::new(
            pki.client_cert.clone(),
            pki.client_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            false,
            vec![SUITE],
        );

        let (c2s_tx, c2s_rx) = mpsc::channel::<Vec<u8>>();
        let (s2c_tx, s2c_rx) = mpsc::channel::<Vec<u8>>();
        let handle = thread::spawn(move || run_server(server_ctx, c2s_rx, s2c_tx, NOW));

        let mut transport = ChannelTransport {
            tx: c2s_tx,
            rx: s2c_rx,
            tamper_finished: false,
        };
        let mut rng = CsRng::new();
        let client_res = client_ctx.handshake_client(&mut transport, &mut rng, NOW);
        let (server_res, server_stats) = handle.join().expect("服务端线程");

        let client_out = client_res.expect("单向模式客户端握手应成功");
        let server_out = server_res.expect("单向模式服务端握手应成功");
        assert_eq!(
            client_out.session_key, server_out.session_key,
            "单向模式双方会话密钥必须一致"
        );
        assert_eq!(
            server_out.peer_cert_fingerprint, [0u8; 32],
            "单向模式服务端无客户端证书指纹"
        );
        assert_eq!(server_stats.handshakes, 1);
    }

    /// HS13：Finished HMAC 不匹配 → 握手中止（HandshakeFailed + rejected）.
    #[test]
    fn hs13_finished_hmac_mismatch_aborts() {
        let pki = make_pki(NOW, 365);

        let server_ctx = MtlsContext::new(
            pki.server_cert.clone(),
            pki.server_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );
        let mut client_ctx = MtlsContext::new(
            pki.client_cert.clone(),
            pki.client_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );

        let (c2s_tx, c2s_rx) = mpsc::channel::<Vec<u8>>();
        let (s2c_tx, s2c_rx) = mpsc::channel::<Vec<u8>>();
        let handle = thread::spawn(move || run_server(server_ctx, c2s_rx, s2c_tx, NOW));

        // 客户端传输开启 Finished 篡改（模拟中间人改动 Finished 帧）
        let mut transport = ChannelTransport {
            tx: c2s_tx,
            rx: s2c_rx,
            tamper_finished: true,
        };
        let mut rng = CsRng::new();
        let _client_res = client_ctx.handshake_client(&mut transport, &mut rng, NOW);
        let (server_res, server_stats) = handle.join().expect("服务端线程");

        assert_eq!(
            server_res,
            Err(TlsError::HandshakeFailed),
            "Finished HMAC 不匹配应中止握手"
        );
        assert_eq!(server_stats.rejected, 1, "服务端拒绝计数应 +1");
        assert_eq!(
            server_stats.last_error,
            Some(TlsError::HandshakeFailed),
            "last_error 应记录 HandshakeFailed"
        );
    }
}
