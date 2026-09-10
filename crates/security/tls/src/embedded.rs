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

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::marker::PhantomData;

use digest::Digest;
use embedded_tls::blocking::{
    Certificate, CertificateEntryRef, CertificateRef, CertificateVerifyRef, CryptoProvider,
    TlsCipherSuite, TlsConfig, TlsContext, TlsError, TlsVerifier,
};
use embedded_tls::SignatureScheme;
use eneros_crypto::pki::parser::parse_der;
use eneros_crypto::pki::verify::CertVerifier;
use eneros_crypto::pki::x509::{SubjectPublicKey, X509Certificate};
use eneros_protocol_abstract::transport::TransportHandle;

/// 47（WP-47.5）：叶子证书 SAN（SubjectAltName，OID 2.5.29.17）dNSName
/// 集合。扩展缺失/解析失败 → None（调用方按兼容路径回退 CN——诚实登记）。
///
/// DER 布局：extensions[].value = OCTET STRING( GeneralNames )；
/// GeneralNames = SEQUENCE OF GeneralName；dNSName = [2] IA5String（0x82）。
fn leaf_san_dns_names(cert: &X509Certificate) -> Option<Vec<String>> {
    const SAN_OID: [u8; 5] = [0x06, 0x03, 0x55, 0x1D, 0x11];
    let ext = cert.extensions.iter().find(|e| e.oid == SAN_OID)?;
    // 本仓解析器已剥 OCTET STRING——ext.value = GeneralNames 内容。
    // GeneralNames SEQUENCE（0x30）。
    let (body, _) = read_tlv(&ext.value, 0x30)?;
    let mut names = Vec::new();
    let mut pos = 0usize;
    while pos < body.len() {
        // GeneralName：tag[0]=0x82（dNSName，上下文 [2] 原语）。
        if body[pos] != 0x82 {
            // 非 dNSName 条目（IP/URI 等）：跳过本条目（TLV 前进）。
            let (_, consumed) = read_tlv(&body[pos..], body[pos])?;
            pos += consumed;
            continue;
        }
        let (dns, consumed) = read_tlv(&body[pos..], 0x82)?;
        pos += consumed;
        if let Ok(n) = core::str::from_utf8(dns) {
            names.push(String::from(n));
        }
    }
    if names.is_empty() {
        return None;
    }
    Some(names)
}

/// 剥 OCTET STRING（0x04 len …）；长度不符 → None（fail-closed 解析）。
fn unwrap_octet_string(der: &[u8]) -> Option<&[u8]> {
    if der.first() != Some(&0x04) {
        return None;
    }
    let (body, consumed) = read_tlv(der, 0x04)?;
    if consumed != der.len() {
        return None; // 尾部多余字节 → 格式不符
    }
    Some(body)
}

/// 读取 TLV：返回 (value 切片, 总消耗字节)。`data_start = 1 tag + 长度
/// 字段总长`（短式 1B / 0x81 2B / 0x82 3B）；越界/格式不符 → None。
fn read_tlv(der: &[u8], expected_tag: u8) -> Option<(&[u8], usize)> {
    if der.first() != Some(&expected_tag) {
        return None;
    }
    let len_byte = *der.get(1)?;
    let (vlen, len_field_total) = match len_byte {
        0x81 => (*der.get(2)? as usize, 2usize),
        0x82 => {
            let hi = *der.get(2)? as usize;
            let lo = *der.get(3)? as usize;
            ((hi << 8) | lo, 3usize)
        }
        n if n < 0x80 => (n as usize, 1usize),
        _ => return None,
    };
    let data_start = 1 + len_field_total;
    let end = data_start.checked_add(vlen)?;
    let body = der.get(data_start..end)?;
    Some((body, end))
}

/// 读取 body[pos..] 的 TLV 长度字段字节数（跳过非 dNSName 条目用）。
fn tlv_len_bytes(der: &[u8]) -> Option<usize> {
    let len_byte = *der.get(1)?;
    match len_byte {
        0x81 => Some(2),
        0x82 => Some(3),
        n if n < 0x80 => Some(1),
        _ => None,
    }
}

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
    /// 已验叶子证书 CN（hostname 校验通过后；工厂对端身份来源）。
    verified_cn: Option<String>,
    /// 47：已验 CN 共享槽（provider 透传；None = 未启用）。
    verified_cell: Option<alloc::rc::Rc<core::cell::RefCell<Option<String>>>>,
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
            verified_cn: None,
            verified_cell: None,
            _cs: PhantomData,
        }
    }

    /// 已验叶子证书 CN（校验通过后）；未校验/失败 → None。
    pub fn verified_leaf_cn(&self) -> Option<&str> {
        self.verified_cn.as_deref()
    }
}

impl<CS: TlsCipherSuite> PkiVerifier<CS> {
    /// 47：启用已验 CN 共享槽（工厂场景）。
    pub fn share_verified_cn(&mut self, cell: alloc::rc::Rc<core::cell::RefCell<Option<String>>>) {
        self.verified_cell = Some(cell);
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
        // 1b. 70.5（WP-70.5.6）：信任根桥接——RFC 8446 允许服务端省略链
        // 中根证书；本仓 verify_chain 要求链尾=自签根。对链尾 issuer 在
        // 信任根中找 subject 匹配者并入链（签名验证仍由 verify_chain
        // 逐级执行——桥接只补路径，不放行任何未签名证书）。
        {
            let last = chain.last().expect("非空已检");
            let looks_root =
                last.issuer == last.subject && self.roots.iter().any(|r| r.subject == last.subject);
            if !looks_root {
                if let Some(root) = self.roots.iter().find(|r| r.subject == last.issuer) {
                    chain.push(root.clone());
                }
            }
        }

        // 2. 链验证（本仓 CertVerifier：有效期 + CRL + SM2/ECDSA 签名 + SPKI 根比对）
        #[cfg(feature = "std")]
        eprintln!(
            "[tls-diag] chain_len={} leaf_issuer={:?} leaf_subject={:?} roots={}",
            chain.len(),
            chain.last().map(|c| &c.issuer.cn),
            chain[0].subject.cn,
            self.roots
                .iter()
                .map(|r| &r.subject.cn)
                .collect::<Vec<_>>()
                .len()
        );
        let verifier = CertVerifier::new(self.roots.clone());
        verifier.verify_chain(&chain, self.now).map_err(|e| {
            #[cfg(feature = "std")]
            eprintln!("[tls-diag] verify_chain err: {e:?}");
            TlsError::InvalidCertificate
        })?;

        // 3. hostname 校验（客户端角色；47 WP-47.5：SAN 优先，CN 兼容回退）。
        //    RFC 6125：SAN 存在时必须命中 dNSName（不区分大小写）；无 SAN
        //    扩展才回退 CN（兼容既有证书）。SAN 解析失败视为无 SAN（兼容
        //    路径——诚实登记于验证器文档）。
        if self.role == VerifyRole::Client {
            if let Some(expected) = &self.expected_hostname {
                let leaf = &chain[0];
                let matched = match leaf_san_dns_names(leaf) {
                    Some(names) => names.iter().any(|n| n.eq_ignore_ascii_case(expected)),
                    None => leaf.subject.cn.eq_ignore_ascii_case(expected),
                };
                if !matched {
                    eneros_runtime::println!(
                        "[tls-diag] hostname mismatch expected={:?} san={:?} cn={:?}",
                        expected,
                        leaf_san_dns_names(leaf),
                        leaf.subject.cn
                    );
                    return Err(TlsError::InvalidCertificate);
                }
                self.verified_cn = Some(leaf.subject.cn.clone());
                if let Some(cell) = &self.verified_cell {
                    *cell.borrow_mut() = Some(leaf.subject.cn.clone());
                }
            }
        }

        // 4. 保存叶子证书 ECDSA P-256 公钥（CertificateVerify 验签用）；
        //    非 ECDSA 公钥（如 SM2）拒绝——TLS 1.3 握手签名仅 ECDSA P-256。
        match chain[0].public_key() {
            SubjectPublicKey::EcdsaP256(point) => {
                self.server_public_key = Some(point.clone());
            }
            other => {
                eneros_runtime::println!(
                    "[tls-diag] leaf SPKI 非 ECDSA P-256（{}）",
                    matches!(other, eneros_crypto::pki::x509::SubjectPublicKey::Sm2(_))
                        .then_some("SM2")
                        .unwrap_or("其他")
                );
                return Err(TlsError::InvalidCertificate);
            }
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
        vk.verify(&msg, &sig)
            .map_err(|_| TlsError::InvalidSignature)
    }
}

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

/// 证书（DER）与私钥（SEC1 DER）持有者，供 [`PkiProvider`] 使用。
pub struct Identity<'a> {
    /// 设备证书链（DER，叶子在前）
    pub cert_chain: &'a [&'a [u8]],
    /// 设备私钥（SEC1 DER，`EC PRIVATE KEY`，ECDSA P-256）
    pub private_key: &'a [u8],
}

/// 适配 `eneros_crypto::rng::CsRng`（本仓 CSRNG）到 `rand_core`（embedded-tls 要求）。
///
/// 70.5：内部持**共享槽** `Rc<RefCell<CsRng>>`——重握手场景下工厂与句柄
/// 共享同一熵源实例（rng 不随 provider 消耗）。
///
/// 生产路径：`CsRng` 必须由平台熵构造（`csrng_from_platform`），
/// 拒绝任何确定性种子注入（熵纪律 R1-4）。
pub struct CsRngAdapter(pub alloc::rc::Rc<core::cell::RefCell<eneros_crypto::rng::CsRng>>);

impl CsRngAdapter {
    fn with_rng(&mut self, f: impl FnOnce(&mut eneros_crypto::rng::CsRng)) {
        f(&mut self.0.borrow_mut());
    }
    fn try_with_rng(
        &mut self,
        f: impl FnOnce(&mut eneros_crypto::rng::CsRng) -> Result<(), eneros_crypto::rng::DrbgError>,
    ) -> Result<(), rand_core::Error> {
        f(&mut self.0.borrow_mut()).map_err(|_| entropy_error())
    }
}

impl rand_core::RngCore for CsRngAdapter {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }

    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }

    fn fill_bytes(&mut self, dst: &mut [u8]) {
        // FIX-BUGS-0810 B5：rand_core trait 契约要求 fill_bytes 以 panic 表达
        // 不可恢复错误——内部走"重播种+重试一次"（fill_bytes_or_reseed）；
        // 仅熵源彻底不可用（ReseedRequired 且平台熵失败）时 panic。
        // 豁免理由（铁律 3）：rand_core 接口契约 + 熵枯竭属 fatal。
        self.with_rng(|rng| {
            if eneros_crypto::rng::fill_bytes_or_reseed(rng, dst).is_err() {
                panic!("csrng: entropy exhausted and platform reseed unavailable");
            }
        });
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), rand_core::Error> {
        // FIX-BUGS-0810 B5：Result 路径直接承接（重播种+重试一次，
        // 熵不可用 → Err，不 panic）。no_std 无 std feature：
        // 用自定义错误码（CUSTOM_START 起）构造，不用 Error::new。
        self.try_with_rng(|rng| eneros_crypto::rng::fill_bytes_or_reseed(rng, dst))
    }
}

impl rand_core::CryptoRng for CsRngAdapter {}

/// 熵源不可用错误（no_std 自定义码；rand_core::Error::CUSTOM_START 起）。
fn entropy_error() -> rand_core::Error {
    rand_core::Error::from(
        core::num::NonZeroU32::new(rand_core::Error::CUSTOM_START).expect("custom 码非零"),
    )
}

/// 共享 CSRNG（`Rc<RefCell<CsRng>>`——TLS 重握手场景下工厂与句柄共享
/// 同一熵源实例，rng 不随 provider 消耗；熵合同由装配方保证来源）。
pub struct SharedCsRng(pub alloc::rc::Rc<core::cell::RefCell<eneros_crypto::rng::CsRng>>);

impl Clone for SharedCsRng {
    fn clone(&self) -> Self {
        SharedCsRng(alloc::rc::Rc::clone(&self.0))
    }
}

impl rand_core::RngCore for SharedCsRng {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        // 同 CsRngAdapter：重播种+重试一次；熵彻底不可用 = fatal panic
        //（豁免理由同 adapter——rand_core 契约 + 熵枯竭属 fatal）。
        if eneros_crypto::rng::fill_bytes_or_reseed(&mut self.0.borrow_mut(), dst).is_err() {
            panic!("csrng: entropy exhausted and platform reseed unavailable");
        }
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), rand_core::Error> {
        eneros_crypto::rng::fill_bytes_or_reseed(&mut self.0.borrow_mut(), dst)
            .map_err(|_| entropy_error())
    }
}
impl rand_core::CryptoRng for SharedCsRng {}

/// embedded-tls `CryptoProvider`：本仓 PKI 验证 + ECDSA P-256 签名 + mTLS 客户端证书。
pub struct PkiProvider<'a, CS: TlsCipherSuite> {
    verifier: PkiVerifier<CS>,
    identity: Option<Identity<'a>>,
    rng: CsRngAdapter,
    /// 47（WP-47.5）：已验叶子 CN 共享槽（工厂读取对端身份用）。
    verified_cn: alloc::rc::Rc<core::cell::RefCell<Option<String>>>,
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
        Self::with_verified_cell(
            roots,
            role,
            now,
            rng,
            identity,
            alloc::rc::Rc::new(core::cell::RefCell::new(None)),
        )
    }

    /// 47（WP-47.5）：构造并共享已验叶子 CN 槽（工厂/对端身份观测）。
    pub fn with_verified_cell(
        roots: Vec<X509Certificate>,
        role: VerifyRole,
        now: u64,
        rng: eneros_crypto::rng::CsRng,
        identity: Option<Identity<'a>>,
        verified_cn: alloc::rc::Rc<core::cell::RefCell<Option<String>>>,
    ) -> Self {
        let mut verifier = PkiVerifier::new(roots, role, now);
        verifier.share_verified_cn(alloc::rc::Rc::clone(&verified_cn));
        Self {
            verifier,
            identity,
            rng: CsRngAdapter(alloc::rc::Rc::new(core::cell::RefCell::new(rng))),
            verified_cn,
        }
    }

    /// 70.5（WP-70.5.6）：共享熵源构造（重握手不耗尽 rng——工厂与句柄
    /// 持同一 `Rc<RefCell<CsRng>>`；熵合同由装配方保证来源）。
    pub fn with_shared_rng(
        roots: Vec<X509Certificate>,
        role: VerifyRole,
        now: u64,
        rng: SharedCsRng,
        identity: Option<Identity<'a>>,
    ) -> Self {
        let verifier = PkiVerifier::new(roots, role, now);
        Self {
            verifier,
            identity,
            rng: CsRngAdapter(rng.0),
            verified_cn: alloc::rc::Rc::new(core::cell::RefCell::new(None)),
        }
    }

    /// 已验叶子 CN（握手后；工厂对端身份来源）。
    pub fn verified_leaf_cn(&self) -> Option<String> {
        self.verified_cn.borrow().clone()
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
        let mut conn = embedded_tls::blocking::TlsConnection::new(socket, read_buf, write_buf);
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
    use std::io::{Read as _, Write as _};

    use super::*;
    use crate::tests_support::{make_identity, TestIdentity};

    const NOW: u64 = 1_700_000_000;

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
        let server = spawn_rustls_server(
            listener,
            server_id.key_der.clone(),
            server_id.cert_der.clone(),
        );

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
        assert!(result.is_err(), "错误 CN 必须握手失败（R1-3 负例）");
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

#[cfg(all(test, feature = "embedded"))]
mod san_tests {
    use eneros_crypto::pki::parser::to_der;
    use eneros_crypto::pki::x509::{
        DistinguishedName, Extension, SignatureAlgorithm, SubjectPublicKey,
    };

    use super::*;

    const NOW2: u64 = 1_700_000_000;

    fn san_identity(cn: &str, dns: &str) -> X509Certificate {
        let sk: p256::ecdsa::SigningKey =
            p256::ecdsa::SigningKey::from_bytes(&[0x51; 32].into()).unwrap();
        let pk = SubjectPublicKey::EcdsaP256(
            sk.verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        );
        let dn = DistinguishedName::new(cn);
        let name = dns.as_bytes();
        let mut gn = vec![0x82, name.len() as u8];
        gn.extend_from_slice(name);
        let mut seq = vec![0x30, gn.len() as u8];
        seq.extend_from_slice(&gn);
        let extensions = vec![Extension {
            oid: vec![0x06, 0x03, 0x55, 0x1D, 0x11],
            critical: false,
            value: seq,
        }];
        let unsigned = X509Certificate::new(
            2,
            vec![1],
            SignatureAlgorithm::EcdsaWithSha256,
            dn.clone(),
            dn.clone(),
            NOW2,
            NOW2 + 86400 * 365,
            pk,
            extensions,
            Vec::new(),
        );
        let tbs = unsigned.encode_tbs().unwrap();
        use p256::ecdsa::signature::Signer;
        let sig: p256::ecdsa::Signature = sk.sign(&tbs);
        X509Certificate::new(
            2,
            vec![1],
            SignatureAlgorithm::EcdsaWithSha256,
            dn.clone(),
            dn.clone(),
            NOW2,
            NOW2 + 86400 * 365,
            SubjectPublicKey::EcdsaP256(
                sk.verifying_key()
                    .to_encoded_point(false)
                    .as_bytes()
                    .to_vec(),
            ),
            unsigned.extensions.clone(),
            sig.to_der().as_bytes().to_vec(),
        )
    }

    #[test]
    fn san_dns_extraction_roundtrip() {
        let cert = san_identity("wrong-cn", "eneros-server");
        let der = to_der(&cert).unwrap();
        let parsed = parse_der(&der).unwrap();
        let names = leaf_san_dns_names(&parsed).expect("SAN 必须可提取");
        assert_eq!(names, vec![String::from("eneros-server")]);
        // 无 SAN 证书 → None（CN 回退路径）。
        let sk2: p256::ecdsa::SigningKey =
            p256::ecdsa::SigningKey::from_bytes(&[0x52; 32].into()).unwrap();
        let pk2 = SubjectPublicKey::EcdsaP256(
            sk2.verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        );
        let dn2 = DistinguishedName::new("x");
        let u2 = X509Certificate::new(
            2,
            vec![1],
            SignatureAlgorithm::EcdsaWithSha256,
            dn2.clone(),
            dn2.clone(),
            NOW2,
            NOW2 + 3600,
            pk2,
            Vec::new(),
            Vec::new(),
        );
        let tbs2 = u2.encode_tbs().unwrap();
        use p256::ecdsa::signature::Signer;
        let sig2: p256::ecdsa::Signature = sk2.sign(&tbs2);
        let p2 = X509Certificate::new(
            2,
            vec![1],
            SignatureAlgorithm::EcdsaWithSha256,
            dn2.clone(),
            dn2.clone(),
            NOW2,
            NOW2 + 3600,
            SubjectPublicKey::EcdsaP256(
                sk2.verifying_key()
                    .to_encoded_point(false)
                    .as_bytes()
                    .to_vec(),
            ),
            Vec::new(),
            sig2.to_der().as_bytes().to_vec(),
        );
        let der2 = to_der(&p2).unwrap();
        let parsed2 = parse_der(&der2).unwrap();
        assert!(leaf_san_dns_names(&parsed2).is_none());
    }
}
#[cfg(all(test, feature = "embedded"))]
#[cfg(test)]
#[test]
fn san_tlv_debug_probe() {
    let value: Vec<u8> = vec![
        0x30, 0x0f, 0x82, 0x0d, 0x65, 0x6e, 0x65, 0x72, 0x6f, 0x73, 0x2d, 0x73, 0x65, 0x72, 0x76,
        0x65, 0x72,
    ];
    eprintln!(
        "probe value len={} first={:02x} second={:02x}",
        value.len(),
        value[0],
        value[1]
    );
    let (body, _) = read_tlv(&value, 0x30).expect("seq");
    assert_eq!(body.len(), 15);
    assert_eq!(body[0], 0x82);
    let (dns, consumed) = read_tlv(&body[..], 0x82).expect("dns");
    assert_eq!(consumed, 15);
    assert_eq!(core::str::from_utf8(dns).expect("utf8"), "eneros-server");
}
