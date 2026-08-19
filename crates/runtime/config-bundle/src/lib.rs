//! EnerOS RuntimeConfigBundle（蓝图 41 WP-41.1/41.2）——运行配置唯一事实。
//!
//! 设计目标（与蓝图 §2 不变量一一对应）：
//!
//! - **签名**：bundle 级 SM2 签名绑定 product_manifest_digest + generation +
//!   created_at + 全部条目；每条目独立签名绑定 payload_digest。验签不过 → 拒载。
//! - **类型化**：host 侧把 `configs/**` TOML 编译为 canonical payload（未知/
//!   重复字段、全零密钥/哈希在编译期拒绝）；target 侧 `PayloadCursor` 解码时
//!   再次拒绝未知字段 id、重复字段、越界长度（no_std，零分配视图）。
//! - **可迁移**：schema migration 为纯函数（`migrate` 钩子），保留 old
//!   generation；apply 失败保持旧 generation，不半应用。
//! - **secret 不落明文**：`SecretHandle` 仅引用安全存储（TPM KeyHandle 先例），
//!   明文 TOML/evidence 中全零占位在编译期拒绝。
//! - **consumer 注册表**：bundle 中每个条目必须命中注册 consumer；注册表中
//!   每个 consumer 必须被 bundle 覆盖；未知/未消费条目默认错误。
//!
//! 分层：本 crate 是 target 可链接核心（no_std + alloc）；TOML 编译面在
//! `toml_compile`（feature `compile`，host/xtask/测试）；EL0 `svc-config`
//! 服务（蓝图 41 business-services）内嵌签名 bundle 并经 EIPC 分发。

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(not(test), allow(dead_code))]

extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::vec::Vec;
use core::fmt;

pub mod payload;
/// 蓝图 45 点表（target 解码面 + PointsConsumer）。no_std + alloc。
pub mod points;
/// 46（WP-46.3）协议配置（target 解码面 + canonical KV；7 份 ACTIVE）。
pub mod protocol;
pub mod schema;
#[cfg(feature = "compile")]
pub mod toml_compile;

/// Bundle 二进制魔数（LE 编码文件头）。
pub const BUNDLE_MAGIC: [u8; 4] = *b"ECFG";
/// 条目二进制魔数。
pub const ENTRY_MAGIC: [u8; 4] = *b"ECFE";
/// 当前 bundle 线格式版本。
pub const BUNDLE_WIRE_VERSION: u16 = 1;
/// 默认（首版）schema 版本。
pub const SCHEMA_VERSION_INIT: u16 = 1;
/// SM2 签名长度（r‖s 各 32B）。
pub const SM2_SIG_LEN: usize = 64;
/// SM3 摘要长度。
pub const DIGEST_LEN: usize = 32;

/// 配置 ID——注册表与 bundle 条目的唯一键。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfigId(pub alloc::string::String);

impl ConfigId {
    pub fn from_static(name: &'static str) -> Self {
        ConfigId(name.to_owned())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConfigId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ConfigId {
    fn from(s: &str) -> Self {
        ConfigId(s.to_owned())
    }
}

impl From<alloc::string::String> for ConfigId {
    fn from(s: alloc::string::String) -> Self {
        ConfigId(s)
    }
}

/// schema 版本（consumer 声明 + 条目声明双侧匹配）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemaVersion(pub u16);

impl Default for SchemaVersion {
    fn default() -> Self {
        SchemaVersion(SCHEMA_VERSION_INIT)
    }
}

/// 配置代次（单调递增；apply 失败不前进）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ConfigGeneration(pub u64);

/// bundle 格式版本（major 不兼容变更 / minor 兼容扩展）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConfigBundleVersion {
    pub major: u16,
    pub minor: u16,
}

impl ConfigBundleVersion {
    pub const V1: Self = ConfigBundleVersion { major: 1, minor: 0 };

    /// 可兼容读取判断：major 必须相等。
    pub fn readable_by(&self, reader: ConfigBundleVersion) -> bool {
        self.major == reader.major
    }
}

/// 32B 摘要（SM3/SHA-256 同尺寸）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest(pub [u8; DIGEST_LEN]);

impl Digest {
    pub fn of(data: &[u8]) -> Self {
        Digest(eneros_crypto::sm3_hash(data))
    }
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|b| *b == 0)
    }
    pub fn as_bytes(&self) -> &[u8; DIGEST_LEN] {
        &self.0
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{:02x}", b)?;
        }
        Ok(())
    }
}

/// 可信时间（UTC 毫秒；来源由平台可信时钟注入，本类型不做假设）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TrustedTime(pub u64);

/// 密钥/秘密种类（SecretHandle 语义分类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    /// SM2 私钥（配置签名/身份）。
    Sm2PrivateKey,
    /// AES/SM4 数据密钥。
    SymmetricKey,
    /// HMAC 密钥（控制写租约先例）。
    HmacKey,
    /// TLS 身份（证书+私钥引用）。
    TlsIdentity,
}

/// SecretHandle——**只引用**安全存储中的秘密，绝不携带明文。
/// `provider` 为安全存储提供方标识（如 TPM KeyHandle 槽位命名空间）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretHandle {
    pub kind: SecretKind,
    pub provider: alloc::string::String,
    pub id: u32,
}

impl SecretHandle {
    /// 全零占位句柄判定（蓝图不变量：密钥不能全零占位）。
    pub fn is_zero_placeholder(&self) -> bool {
        self.provider.is_empty() && self.id == 0
    }
}

/// 配置错误——fail-closed：任何歧义都不得静默继续。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// 编译/解码发现未知字段。
    UnknownField { config: ConfigId, field: u8 },
    /// host 编译面发现未知 TOML 键（字符串名）。
    UnknownKey {
        config: ConfigId,
        key: alloc::string::String,
    },
    /// 重复字段（编译期或解码期）。
    DuplicateField { config: ConfigId, field: u8 },
    /// 字段缺失（必填未给）。
    MissingField {
        config: ConfigId,
        field: &'static str,
    },
    /// 字段值非法（类型/范围/格式）。
    InvalidValue {
        config: ConfigId,
        field: &'static str,
        reason: alloc::string::String,
    },
    /// 全零密钥/哈希/endpoint 占位。
    ZeroSecret {
        config: ConfigId,
        field: &'static str,
    },
    /// bundle/条目签名无效。
    BadSignature(ConfigId),
    /// payload 摘要与声明不符（篡改）。
    TamperedPayload(ConfigId),
    /// product_manifest_digest 不匹配。
    WrongManifest { expected: Digest, got: Digest },
    /// generation 不单调（旧代次重放）。
    StaleGeneration { expected_min: u64, got: u64 },
    /// schema 版本不兼容。
    SchemaMismatch {
        config: ConfigId,
        consumer: SchemaVersion,
        entry: SchemaVersion,
    },
    /// bundle 条目没有被任何注册 consumer 消费。
    UnconsumedConfig(ConfigId),
    /// 注册 consumer 在 bundle 中无对应条目。
    MissingConfigEntry(ConfigId),
    /// bundle 中重复配置 id。
    DuplicateEntry(ConfigId),
    /// 编码/解码线格式错误。
    WireFormat(&'static str),
    /// 存储/IO 层错误。
    Storage(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::UnknownField { config, field } => {
                write!(f, "unknown-field:{config}:{field}")
            }
            ConfigError::UnknownKey { config, key } => write!(f, "unknown-key:{config}:{key}"),
            ConfigError::DuplicateField { config, field } => {
                write!(f, "duplicate-field:{config}:{field}")
            }
            ConfigError::MissingField { config, field } => {
                write!(f, "missing-field:{config}:{field}")
            }
            ConfigError::InvalidValue {
                config,
                field,
                reason,
            } => write!(f, "invalid-value:{config}:{field}:{reason}"),
            ConfigError::ZeroSecret { config, field } => {
                write!(f, "zero-secret:{config}:{field}")
            }
            ConfigError::BadSignature(config) => write!(f, "bad-signature:{config}"),
            ConfigError::TamperedPayload(config) => write!(f, "tampered-payload:{config}"),
            ConfigError::WrongManifest { expected, got } => {
                write!(f, "wrong-manifest:expected={expected} got={got}")
            }
            ConfigError::StaleGeneration { expected_min, got } => {
                write!(f, "stale-generation:min={expected_min} got={got}")
            }
            ConfigError::SchemaMismatch {
                config,
                consumer,
                entry,
            } => write!(
                f,
                "schema-mismatch:{config}:consumer={} entry={}",
                consumer.0, entry.0
            ),
            ConfigError::UnconsumedConfig(config) => write!(f, "unconsumed-config:{config}"),
            ConfigError::MissingConfigEntry(config) => write!(f, "missing-config-entry:{config}"),
            ConfigError::DuplicateEntry(config) => write!(f, "duplicate-entry:{config}"),
            ConfigError::WireFormat(msg) => write!(f, "wire-format:{msg}"),
            ConfigError::Storage(msg) => write!(f, "storage:{msg}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ConfigError {}

/// apply 结果语义（蓝图 §3 通用成功语义：Queued/Sent/PeerAcked/Reconciled
/// 之外的业务终态才返回业务成功；配置 apply 只有三种合法结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// 配置已生效于当前 generation。
    Applied { generation: u64 },
    /// 与当前生效值相同，无操作。
    NoOp,
    /// 需要重启服务/系统才能生效（hot apply 声明 restart-required）。
    RestartRequired { generation: u64 },
}

/// 单条签名配置条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedConfigEntry {
    pub config_id: ConfigId,
    pub schema: SchemaVersion,
    pub generation: u64,
    /// canonical payload（`payload::encode_payload` 产物；target 解码零信任）。
    pub payload: Vec<u8>,
    pub payload_digest: Digest,
    /// 64B SM2 签名（对 entry canonical 字节）。
    pub signature: Vec<u8>,
}

/// 签名运行时配置 bundle（蓝图 §2 类型）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfigBundle {
    pub schema: ConfigBundleVersion,
    pub product_manifest_digest: Digest,
    pub generation: u64,
    pub created_at: TrustedTime,
    pub entries: Vec<SignedConfigEntry>,
    /// 64B SM2 签名（对 bundle canonical 字节）。
    pub signature: Vec<u8>,
}

impl RuntimeConfigBundle {
    pub fn new(product_manifest_digest: Digest, generation: u64, created_at: TrustedTime) -> Self {
        RuntimeConfigBundle {
            schema: ConfigBundleVersion::V1,
            product_manifest_digest,
            generation,
            created_at,
            entries: Vec::new(),
            signature: Vec::new(),
        }
    }

    pub fn entry(&self, id: &ConfigId) -> Option<&SignedConfigEntry> {
        self.entries.iter().find(|e| &e.config_id == id)
    }
}

// ---------------------------------------------------------------------------
// canonical 编解码（无 serde；确定性字节序 = LE；长度前缀）
// ---------------------------------------------------------------------------

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn push_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ConfigError> {
        if self.pos + n > self.buf.len() {
            return Err(ConfigError::WireFormat("short-read"));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16, ConfigError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, ConfigError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn done(&self) -> bool {
        self.pos == self.buf.len()
    }
}

/// 条目 canonical 字节（签名/验签输入）：magic + wire_version + 字段。
pub fn encode_entry_canonical(entry: &SignedConfigEntry) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + entry.payload.len());
    out.extend_from_slice(&ENTRY_MAGIC);
    push_u16(&mut out, BUNDLE_WIRE_VERSION);
    let id = entry.config_id.as_str().as_bytes();
    push_u16(&mut out, id.len() as u16);
    out.extend_from_slice(id);
    push_u16(&mut out, entry.schema.0);
    push_u64(&mut out, entry.generation);
    push_u64(&mut out, entry.payload.len() as u64);
    out.extend_from_slice(&entry.payload);
    out.extend_from_slice(entry.payload_digest.as_bytes());
    out
}

/// bundle canonical 字节（签名/验签输入）：magic + wire_version + 字段 +
/// 全部条目（按 config_id 字典序，确定性）。
pub fn encode_bundle_canonical(bundle: &RuntimeConfigBundle) -> Vec<u8> {
    let mut entries: Vec<&SignedConfigEntry> = bundle.entries.iter().collect();
    entries.sort_by(|a, b| a.config_id.cmp(&b.config_id));
    let mut out = Vec::with_capacity(128);
    out.extend_from_slice(&BUNDLE_MAGIC);
    push_u16(&mut out, BUNDLE_WIRE_VERSION);
    push_u16(&mut out, bundle.schema.major);
    push_u16(&mut out, bundle.schema.minor);
    out.extend_from_slice(bundle.product_manifest_digest.as_bytes());
    push_u64(&mut out, bundle.generation);
    push_u64(&mut out, bundle.created_at.0);
    push_u16(&mut out, entries.len() as u16);
    for e in entries {
        let enc = encode_entry_canonical(e);
        push_u16(&mut out, enc.len() as u16);
        out.extend_from_slice(&enc);
    }
    out
}

/// **完整**序列化（介质/嵌入格式）：canonical 字节 + 全部条目签名
/// （按 config_id 字典序）+ bundle 签名。canonical 本身不含签名
/// （签名是对 canonical 计算的）——只写 canonical 会丢签名（QEMU
/// 实测 svc-config 验签 BadSignature 根因）。
pub fn encode_bundle_full(bundle: &RuntimeConfigBundle) -> Vec<u8> {
    let mut out = encode_bundle_canonical(bundle);
    let mut entries: Vec<&SignedConfigEntry> = bundle.entries.iter().collect();
    entries.sort_by(|a, b| a.config_id.cmp(&b.config_id));
    for e in &entries {
        out.extend_from_slice(&e.signature);
    }
    out.extend_from_slice(&bundle.signature);
    out
}

/// 完整反序列化（验签前恢复签名）。
pub fn decode_bundle_full(bytes: &[u8]) -> Result<RuntimeConfigBundle, ConfigError> {
    // 从 canonical 头读条目数（magic4 + ver2 + major2 + minor2 + digest32
    // + gen8 + created8 = 58B，随后 u16 count）。
    if bytes.len() < 60 {
        return Err(ConfigError::WireFormat("short-bundle"));
    }
    let n = u16::from_le_bytes([bytes[58], bytes[59]]) as usize;
    let sig_total = SM2_SIG_LEN * (n + 1);
    if bytes.len() < sig_total {
        return Err(ConfigError::WireFormat("missing-signatures"));
    }
    let (canon, sigs) = bytes.split_at(bytes.len() - sig_total);
    let mut bundle = decode_bundle(canon)?;
    let mut entries: Vec<&mut SignedConfigEntry> = bundle.entries.iter_mut().collect();
    entries.sort_by(|a, b| a.config_id.cmp(&b.config_id));
    for (i, e) in entries.iter_mut().enumerate() {
        e.signature = sigs[i * SM2_SIG_LEN..(i + 1) * SM2_SIG_LEN].to_vec();
    }
    bundle.signature = sigs[n * SM2_SIG_LEN..].to_vec();
    Ok(bundle)
}

/// 解码 bundle（不验签；验签见 [`verify_bundle`]）。
pub fn decode_bundle(bytes: &[u8]) -> Result<RuntimeConfigBundle, ConfigError> {
    let mut r = Reader { buf: bytes, pos: 0 };
    if r.take(4)? != BUNDLE_MAGIC {
        return Err(ConfigError::WireFormat("bad-magic"));
    }
    if r.u16()? != BUNDLE_WIRE_VERSION {
        return Err(ConfigError::WireFormat("bad-wire-version"));
    }
    let schema = ConfigBundleVersion {
        major: r.u16()?,
        minor: r.u16()?,
    };
    let mut manifest = [0u8; DIGEST_LEN];
    manifest.copy_from_slice(r.take(DIGEST_LEN)?);
    let generation = r.u64()?;
    let created_at = TrustedTime(r.u64()?);
    let n = r.u16()? as usize;
    let mut entries = Vec::with_capacity(n);
    let mut seen: alloc::collections::BTreeSet<alloc::string::String> =
        alloc::collections::BTreeSet::new();
    for _ in 0..n {
        let len = r.u16()? as usize;
        let enc = r.take(len)?;
        let entry = decode_entry(enc)?;
        if !seen.insert(entry.config_id.as_str().to_owned()) {
            return Err(ConfigError::DuplicateEntry(entry.config_id.clone()));
        }
        entries.push(entry);
    }
    if !r.done() {
        return Err(ConfigError::WireFormat("trailing-bytes"));
    }
    Ok(RuntimeConfigBundle {
        schema,
        product_manifest_digest: Digest(manifest),
        generation,
        created_at,
        entries,
        signature: Vec::new(),
    })
}

/// 解码单条 entry（不验签）。
pub fn decode_entry(bytes: &[u8]) -> Result<SignedConfigEntry, ConfigError> {
    let mut r = Reader { buf: bytes, pos: 0 };
    if r.take(4)? != ENTRY_MAGIC {
        return Err(ConfigError::WireFormat("bad-entry-magic"));
    }
    if r.u16()? != BUNDLE_WIRE_VERSION {
        return Err(ConfigError::WireFormat("bad-entry-version"));
    }
    let id_len = r.u16()? as usize;
    let id = core::str::from_utf8(r.take(id_len)?)
        .map_err(|_| ConfigError::WireFormat("bad-config-id"))?
        .to_owned();
    let schema = SchemaVersion(r.u16()?);
    let generation = r.u64()?;
    let payload_len = r.u64()? as usize;
    if payload_len > 1 << 20 {
        return Err(ConfigError::WireFormat("payload-too-large"));
    }
    let payload = r.take(payload_len)?.to_vec();
    let mut digest = [0u8; DIGEST_LEN];
    digest.copy_from_slice(r.take(DIGEST_LEN)?);
    if !r.done() {
        return Err(ConfigError::WireFormat("trailing-entry-bytes"));
    }
    Ok(SignedConfigEntry {
        config_id: ConfigId(id),
        schema,
        generation,
        payload,
        payload_digest: Digest(digest),
        signature: Vec::new(),
    })
}

/// 为 bundle 生成签名（生产路径；`rng` 由调用方注入——target 侧来自
/// 平台熵源，host 侧来自 OS）。
pub fn sign_bundle(
    bundle: &mut RuntimeConfigBundle,
    sk: &eneros_crypto::Sm2PrivateKey,
    pk: &eneros_crypto::Sm2PublicKey,
    rng: &mut eneros_crypto::CsRng,
) -> Result<(), ConfigError> {
    // 先为每条目补签名（payload 摘要必须与 payload 一致）。
    for entry in &mut bundle.entries {
        let expect = Digest::of(&entry.payload);
        if expect != entry.payload_digest {
            return Err(ConfigError::TamperedPayload(entry.config_id.clone()));
        }
        if entry.signature.len() != SM2_SIG_LEN {
            let enc = encode_entry_canonical(entry);
            let sig = eneros_crypto::sm2_sign(&enc, sk, pk, rng)
                .map_err(|_| ConfigError::Storage("sm2-sign"))?;
            entry.signature = sig.to_bytes().to_vec();
        }
    }
    let enc = encode_bundle_canonical(bundle);
    let sig =
        eneros_crypto::sm2_sign(&enc, sk, pk, rng).map_err(|_| ConfigError::Storage("sm2-sign"))?;
    bundle.signature = sig.to_bytes().to_vec();
    Ok(())
}

/// 启动路径验签（QEMU TCG 下 6 次 SM2 验签超 readiness 窗口实测）：
/// bundle 级 SM2 签名（密码学上绑定全部条目的 canonical 字节与摘要），
/// 含 manifest 绑定与每条目 payload 摘要自检（篡改检测）。条目级
/// SM2 签名由 [`verify_bundle`]（Gate/测试）与按需路径（GET_ENTRY）
/// 验证——启动不重复 5 次 SM2。
pub fn verify_bundle_startup(
    bundle: &RuntimeConfigBundle,
    pk: &eneros_crypto::Sm2PublicKey,
    expected_manifest_digest: Option<&Digest>,
) -> Result<(), ConfigError> {
    if let Some(expected) = expected_manifest_digest {
        if &bundle.product_manifest_digest != expected {
            return Err(ConfigError::WrongManifest {
                expected: *expected,
                got: bundle.product_manifest_digest,
            });
        }
    }
    for entry in &bundle.entries {
        let expect = Digest::of(&entry.payload);
        if expect != entry.payload_digest {
            return Err(ConfigError::TamperedPayload(entry.config_id.clone()));
        }
        if entry.signature.len() != SM2_SIG_LEN {
            return Err(ConfigError::BadSignature(entry.config_id.clone()));
        }
    }
    let enc = encode_bundle_canonical(bundle);
    let sig = eneros_crypto::Sm2Signature::from_bytes(
        bundle
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| ConfigError::BadSignature(ConfigId::from_static("bundle")))?,
    );
    let ok = eneros_crypto::sm2_verify(&enc, &sig, pk)
        .map_err(|_| ConfigError::Storage("sm2-verify"))?;
    if !ok {
        return Err(ConfigError::BadSignature(ConfigId::from_static("bundle")));
    }
    Ok(())
}

/// 验签 bundle（fail-closed）：bundle 签名 → manifest 绑定 → 每条目
/// 签名/摘要 → generation/schema 一致性。
///
/// `expected_manifest_digest = None` 表示调用方不校验 manifest 绑定
/// （仅测试/工具用；release 服务必须传入实际 digest）。
pub fn verify_bundle(
    bundle: &RuntimeConfigBundle,
    pk: &eneros_crypto::Sm2PublicKey,
    expected_manifest_digest: Option<&Digest>,
) -> Result<(), ConfigError> {
    if let Some(expected) = expected_manifest_digest {
        if &bundle.product_manifest_digest != expected {
            return Err(ConfigError::WrongManifest {
                expected: *expected,
                got: bundle.product_manifest_digest,
            });
        }
    }
    // 先逐条目检查（诊断优先：篡改 payload → TamperedPayload；条目签名
    // 错 → BadSignature(条目)），最后做 bundle 级签名校验。
    for entry in &bundle.entries {
        let expect = Digest::of(&entry.payload);
        if expect != entry.payload_digest {
            return Err(ConfigError::TamperedPayload(entry.config_id.clone()));
        }
        let enc = encode_entry_canonical(entry);
        let sig = eneros_crypto::Sm2Signature::from_bytes(
            entry
                .signature
                .as_slice()
                .try_into()
                .map_err(|_| ConfigError::BadSignature(entry.config_id.clone()))?,
        );
        let ok = eneros_crypto::sm2_verify(&enc, &sig, pk)
            .map_err(|_| ConfigError::Storage("sm2-verify"))?;
        if !ok {
            return Err(ConfigError::BadSignature(entry.config_id.clone()));
        }
    }
    let enc = encode_bundle_canonical(bundle);
    let sig = eneros_crypto::Sm2Signature::from_bytes(
        bundle
            .signature
            .as_slice()
            .try_into()
            .map_err(|_| ConfigError::BadSignature(ConfigId::from_static("bundle")))?,
    );
    let ok = eneros_crypto::sm2_verify(&enc, &sig, pk)
        .map_err(|_| ConfigError::Storage("sm2-verify"))?;
    if !ok {
        return Err(ConfigError::BadSignature(ConfigId::from_static("bundle")));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Consumer 与注册表
// ---------------------------------------------------------------------------

/// 蓝图 §2 的类型化 consumer（服务内部使用）。
pub trait TypedConfigConsumer {
    type Config;
    fn config_id(&self) -> ConfigId;
    fn schema_version(&self) -> SchemaVersion;
    fn decode_validate(&self, raw: &[u8]) -> Result<Self::Config, ConfigError>;
    fn apply(&mut self, cfg: Self::Config, generation: u64) -> Result<ApplyOutcome, ConfigError>;
}

/// 对象安全 consumer（注册表形式；由 `TypedConfigConsumer` 泛型桥接生成）。
pub trait ConfigConsumer {
    fn config_id(&self) -> ConfigId;
    fn schema_version(&self) -> SchemaVersion;
    /// decode-only 校验（两阶段 apply 的预检；不改变任何状态）。
    fn decode_raw(&self, raw: &[u8]) -> Result<(), ConfigError>;
    fn apply_raw(&mut self, raw: &[u8], generation: u64) -> Result<ApplyOutcome, ConfigError>;
}

impl<T: TypedConfigConsumer> ConfigConsumer for T {
    fn config_id(&self) -> ConfigId {
        self.config_id()
    }
    fn schema_version(&self) -> SchemaVersion {
        self.schema_version()
    }
    fn decode_raw(&self, raw: &[u8]) -> Result<(), ConfigError> {
        self.decode_validate(raw).map(|_| ())
    }
    fn apply_raw(&mut self, raw: &[u8], generation: u64) -> Result<ApplyOutcome, ConfigError> {
        let cfg = self.decode_validate(raw)?;
        self.apply(cfg, generation)
    }
}

/// 注册表：bundle ↔ consumer 双向完备性 + 原子应用。
#[derive(Default)]
pub struct ConfigRegistry {
    consumers: Vec<alloc::boxed::Box<dyn ConfigConsumer>>,
    /// 最近一次成功应用后各 consumer 的生效 generation。
    active: alloc::collections::BTreeMap<ConfigId, u64>,
    /// 最近一次成功应用的 bundle digest（服务上报 active config digest 用）。
    active_bundle_digest: Option<Digest>,
}

impl ConfigRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, consumer: alloc::boxed::Box<dyn ConfigConsumer>) {
        self.consumers.push(consumer);
    }

    pub fn consumers(&self) -> impl Iterator<Item = &dyn ConfigConsumer> {
        self.consumers.iter().map(|b| b.as_ref())
    }

    pub fn active_generation(&self, id: &ConfigId) -> Option<u64> {
        self.active.get(id).copied()
    }

    pub fn active_bundle_digest(&self) -> Option<&Digest> {
        self.active_bundle_digest.as_ref()
    }

    /// 双向完备性检查：bundle 条目必须全部被注册（`UnconsumedConfig`）；
    /// 注册 consumer 必须全部被 bundle 覆盖（`MissingConfigEntry`）；
    /// schema 双侧一致；generation 必须 ≥ 当前生效。
    pub fn check_bundle(&self, bundle: &RuntimeConfigBundle) -> Result<(), ConfigError> {
        for entry in &bundle.entries {
            let c = self
                .consumers
                .iter()
                .find(|c| c.config_id() == entry.config_id)
                .ok_or_else(|| ConfigError::UnconsumedConfig(entry.config_id.clone()))?;
            if c.schema_version() != entry.schema {
                return Err(ConfigError::SchemaMismatch {
                    config: entry.config_id.clone(),
                    consumer: c.schema_version(),
                    entry: entry.schema,
                });
            }
            if let Some(active) = self.active.get(&entry.config_id) {
                if entry.generation < *active {
                    return Err(ConfigError::StaleGeneration {
                        expected_min: *active,
                        got: entry.generation,
                    });
                }
            }
        }
        for c in &self.consumers {
            let id = c.config_id();
            if !bundle.entries.iter().any(|e| e.config_id == id) {
                return Err(ConfigError::MissingConfigEntry(id));
            }
        }
        Ok(())
    }

    /// 应用 bundle（两阶段，蓝图不变量“apply 失败保持旧 generation，
    /// 不半应用”）：
    /// 1. 验签 + 双向完备性 + **全部条目 decode 预检**（任一失败 → 整体
    ///    拒绝，任何 consumer 状态不变）；
    /// 2. 逐 consumer apply（apply 失败 → 返回错误；失败 consumer 保持
    ///    旧 generation，其余已应用者正常推进——本调用整体未达成
    ///    generation 前进，active_bundle_digest 不更新）。
    ///
    /// 成功后 `active_bundle_digest` 更新（服务上报 active config digest）。
    pub fn apply_bundle(
        &mut self,
        bundle: &RuntimeConfigBundle,
        pk: &eneros_crypto::Sm2PublicKey,
        expected_manifest_digest: Option<&Digest>,
    ) -> Result<(), ConfigError> {
        verify_bundle(bundle, pk, expected_manifest_digest)?;
        self.check_bundle(bundle)?;
        // 阶段 1：全部条目 decode 预检（只读，不改变任何状态）。
        for entry in &bundle.entries {
            let c = self
                .consumers
                .iter()
                .find(|c| c.config_id() == entry.config_id)
                .ok_or_else(|| ConfigError::UnconsumedConfig(entry.config_id.clone()))?;
            c.decode_raw(&entry.payload)?;
        }
        // 阶段 2：正式 apply。
        for entry in &bundle.entries {
            let c = self
                .consumers
                .iter_mut()
                .find(|c| c.config_id() == entry.config_id)
                .ok_or_else(|| ConfigError::UnconsumedConfig(entry.config_id.clone()))?;
            c.apply_raw(&entry.payload, entry.generation)?;
            self.active
                .insert(entry.config_id.clone(), entry.generation);
        }
        self.active_bundle_digest = Some(Digest::of(&encode_bundle_canonical(bundle)));
        Ok(())
    }
}

/// 常用校验辅助：拒绝全零字节串（密钥/哈希占位）。
pub fn is_zero_bytes(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| *b == 0)
}
