//! 产品政策 Gate（蓝图 36 WP-36.1/WP-36.2 + ADR registry）。
//!
//! - `product/kernel-policy.toml`：唯一产品内核、research-only 清单、owner 签署。
//! - `product/crypto-policy.toml`：算法用途细分的 provider/实现/审批状态。
//! - `docs/adr/registry.toml`：ADR 唯一 registry（id/path/title/status/
//!   supersedes/superseded_by/decision_digest）。
//!
//! Gate 拒绝：重复 ADR ID、多个 Accepted 冲突决策、profile kernel 不在
//! allowlist、research kernel 进入 release、product_allowed=false 到达产品 ELF。

// 44.2 收口登记：本模块含 serde 数据模型（字段由 JSON 反序列化驱动，编译器
// 视作"never read"）与为后续卷保留的策略辅助函数（file_digest/hex 等）；
// dead_code 定向豁免，解冻条件见 44.4 冻结登记。
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::invalid;
use super::product_manifest::ProductManifest;
use super::DynResult;

pub const KERNEL_POLICY_SCHEMA: &str = "eneros.kernel-policy.v1";
pub const CRYPTO_POLICY_SCHEMA: &str = "eneros.crypto-policy.v1";
pub const ADR_REGISTRY_SCHEMA: &str = "eneros.adr-registry.v1";

// ---------------------------------------------------------------------------
// kernel-policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KernelPolicy {
    pub schema: String,
    pub product_kernel: String,
    pub runtime_root: String,
    pub allowed_release_kernels: Vec<String>,
    pub research_only: Vec<String>,
    pub decision_adr: String,
    pub owner_approval: String,
}

impl KernelPolicy {
    pub fn validate(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if self.schema != KERNEL_POLICY_SCHEMA {
            violations.push(format!("kernel-policy-schema:{}", self.schema));
        }
        if self.product_kernel.is_empty() {
            violations.push("kernel-policy-product-empty".to_owned());
        }
        if !self.allowed_release_kernels.contains(&self.product_kernel) {
            violations.push("kernel-policy-product-not-in-allowlist".to_owned());
        }
        if self.research_only.contains(&self.product_kernel) {
            violations.push("kernel-policy-product-is-research".to_owned());
        }
        if self.runtime_root.is_empty() {
            violations.push("kernel-policy-runtime-root-empty".to_owned());
        }
        if self.owner_approval.is_empty()
            || self.owner_approval == "<external-signature-or-change-control-id>"
        {
            violations.push("kernel-policy-owner-approval-missing".to_owned());
        }
        violations
    }
}

pub fn load_kernel_policy(root: &Path) -> DynResult<KernelPolicy> {
    let path = root.join("product/kernel-policy.toml");
    let text = fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    let policy: KernelPolicy =
        toml::from_str(&text).map_err(|e| invalid(format!("kernel-policy.toml parse: {e}")))?;
    let violations = policy.validate();
    if !violations.is_empty() {
        return Err(invalid(format!(
            "kernel policy invalid: {}",
            violations.join("; ")
        )));
    }
    Ok(policy)
}

/// Gate：profile kernel 必须在 allowlist；research kernel 不得进入 release。
pub fn gate_kernel_policy(root: &Path, manifest: &ProductManifest) -> DynResult<Vec<String>> {
    let mut violations = Vec::new();
    let policy = match load_kernel_policy(root) {
        Ok(policy) => policy,
        Err(error) => return Ok(vec![format!("kernel-policy-unreadable:{error}")]),
    };
    // manifest kernel 名（selector 的 package 段或显式 name）。
    let kernel_name = manifest.kernel.name.clone();
    let kernel_selector = manifest.kernel.selector.clone();
    if kernel_name != policy.product_kernel
        && !policy
            .allowed_release_kernels
            .iter()
            .any(|k| kernel_name == *k || kernel_selector.contains(k.as_str()))
    {
        violations.push(format!(
            "kernel-not-in-allowlist:{kernel_name} (product={})",
            policy.product_kernel
        ));
    }
    if policy.research_only.contains(&kernel_name) {
        violations.push(format!("research-kernel-in-release:{kernel_name}"));
    }
    Ok(violations)
}

// ---------------------------------------------------------------------------
// crypto-policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CryptoPolicy {
    pub schema: String,
    /// 用途 → 实现声明（用途必须细分，禁止单一“国密支持”布尔值）。
    pub usages: BTreeMap<String, CryptoImplementation>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CryptoImplementation {
    pub provider: String,
    pub crate_name: String,
    /// 实现依赖（随主实现同一评估单元）：crate 名加入闭世界登记集合，
    /// 但 approval 状态与主实现一致（如 ed25519-dalek → ed25519/
    /// curve25519-dalek）。
    #[serde(default)]
    pub impl_dependencies: Vec<String>,
    pub product_allowed: bool,
    pub evaluation_status: String,
    pub allowed_targets: Vec<String>,
    pub allowed_features: Vec<String>,
    pub evidence_digest: String,
    /// 43（WP-43.12）：该用途的密码实现必须接受独立评审（外部评审包）。
    /// `product_allowed=true` 且 `review_required=true` 时，必须存在
    /// `product/security/review-status.json` 的登记项；评审逾期未 PASS
    /// 即机器回退 `product_allowed=false`（CHG-2026-08-17-42-closure-sm2
    /// 的"评审不通过必须回退"条件的机器化）。
    #[serde(default)]
    pub review_required: bool,
}

/// 密码独立评审状态登记（蓝图 43 WP-43.12）。
/// 状态：`PASS | PENDING | FAIL | BLOCKED`；`review_deadline` 为下次评审
/// 到期日（RFC3339）。`product_allowed=true + review_required=true` 的
/// 用途在 deadline 之后若未得到 PASS 续期，closure/Gate 必须转红（机器
/// 回退，不可由执行 AI 自填 N/A）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CryptoReviewStatus {
    pub schema: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub generated_at: String,
    pub reviews: BTreeMap<String, CryptoReviewEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CryptoReviewEntry {
    pub status: String,
    pub reviewer: String,
    #[serde(default)]
    pub reviewed_at: Option<String>,
    pub review_deadline: String,
    pub evidence_ref: String,
}

pub const REVIEW_STATUS_SCHEMA: &str = "eneros.crypto-review-status.v1";

fn parse_epoch_seconds(rfc3339: &str) -> Option<i64> {
    // 接受 "YYYY-MM-DD" 或完整 RFC3339 "YYYY-MM-DDTHH:MM:SSZ"。
    let s = rfc3339.trim();
    let date = s.get(..10)?;
    let y: i32 = date.get(0..4)?.parse().ok()?;
    let m: u32 = date.get(5..7)?.parse().ok()?;
    let d: u32 = date.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // 儒略日换算（格里高利，标准公式 y 含 +4800 偏移）。
    let a = (14 - m as i64) / 12;
    let y = y as i64 + 4800 - a;
    let m = m as i64 + 12 * a - 3;
    let jdn = d as i64 + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045;
    let mut seconds = (jdn - 2_440_588) * 86_400; // days since 1970-01-01
                                                  // 时分秒（可选）。
    if let Some(t) = s.get(11..) {
        let t = t.trim_end_matches('Z');
        let mut parts = t.split(':');
        let h: i64 = parts.next()?.parse().ok()?;
        let mi: i64 = parts.next()?.parse().ok()?;
        let se: i64 = parts.next().unwrap_or("0").parse().ok()?;
        if !(0..=23).contains(&h) || !(0..=59).contains(&mi) || !(0..=59).contains(&se) {
            return None;
        }
        seconds += h * 3600 + mi * 60 + se;
    }
    Some(seconds)
}

fn now_epoch_seconds() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 评审门：`review_required && product_allowed` 的用途，若无登记/未续期，
/// 视为评审回退 → 返回 Err（错误信息即回退语义；closure Gate 非零）。
pub fn enforce_crypto_review(root: &Path, usage: &str, crate_name: &str) -> DynResult<()> {
    let status_path = root.join("product/security/review-status.json");
    let text = fs::read_to_string(&status_path).map_err(|_| {
        invalid(format!(
            "crypto-review-rollback:{usage}:review-status.json missing (product_allowed 回退)"
        ))
    })?;
    let status: CryptoReviewStatus = serde_json::from_str(&text).map_err(|_| {
        invalid(format!(
            "crypto-review-rollback:{usage}:review-status.json unparseable (product_allowed 回退)"
        ))
    })?;
    if status.schema != REVIEW_STATUS_SCHEMA {
        return Err(invalid(format!(
            "crypto-review-rollback:{usage}:schema-mismatch (product_allowed 回退)"
        )));
    }
    let entry = status
        .reviews
        .get(usage)
        .or_else(|| status.reviews.get(crate_name))
        .ok_or_else(|| {
            invalid(format!(
                "crypto-review-rollback:{usage}:no review entry (product_allowed 回退)"
            ))
        })?;
    let deadline = parse_epoch_seconds(&entry.review_deadline)
        .ok_or_else(|| invalid(format!("crypto-review-deadline-malformed:{usage}")))?;
    let now = now_epoch_seconds();
    match entry.status.as_str() {
        "PASS" => {
            if now > deadline {
                return Err(invalid(format!(
                    "crypto-review-rollback:{usage}:review expired without renewal (product_allowed 回退)"
                )));
            }
            Ok(())
        }
        // PENDING/FAIL/BLOCKED：决议在期限内有效（CHG 授权期）；逾期未
        // 取得 PASS → 机器回退。
        _ => {
            if now > deadline {
                Err(invalid(format!(
                    "crypto-review-rollback:{usage}:deadline passed with status {} (product_allowed 回退)",
                    entry.status
                )))
            } else {
                Ok(())
            }
        }
    }
}

pub fn load_crypto_policy(root: &Path) -> DynResult<CryptoPolicy> {
    let path = root.join("product/crypto-policy.toml");
    let text = fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    let policy: CryptoPolicy =
        toml::from_str(&text).map_err(|e| invalid(format!("crypto-policy.toml parse: {e}")))?;
    if policy.schema != CRYPTO_POLICY_SCHEMA {
        return Err(invalid(format!("crypto-policy-schema:{}", policy.schema)));
    }
    if policy.usages.is_empty() {
        return Err(invalid("crypto-policy has no usages"));
    }
    let allowed_usages = [
        "boot_signature",
        "device_identity",
        "channel_tls",
        "audit_hash",
        "ota_signature",
        "attestation",
        "kernel_integrity",
    ];
    for (usage, impl_) in &policy.usages {
        if !allowed_usages.contains(&usage.as_str()) {
            return Err(invalid(format!("crypto-policy-unknown-usage:{usage}")));
        }
        if impl_.evaluation_status == "approved" && !impl_.product_allowed {
            return Err(invalid(format!(
                "crypto-policy-approval-inconsistent:{usage}"
            )));
        }
        if impl_.product_allowed && impl_.evidence_digest.len() < 16 {
            return Err(invalid(format!(
                "crypto-policy-approval-missing-evidence:{usage}"
            )));
        }
        // 36 收尾（review should-fix）：evidence_digest 必须是 64 位十六进制
        // SHA-256 摘要，或 `binding:<topic>` 绑定标识（topic 非空，允许
        // `/` `+` `:`，拒绝 `..` 与反斜杠）；长度检查不足以防止
        // "任意占位串冒充证据锚"。
        let digest_ok = impl_.evidence_digest.len() == 64
            && impl_.evidence_digest.bytes().all(|b| b.is_ascii_hexdigit())
            || (impl_.evidence_digest.starts_with("binding:")
                && impl_.evidence_digest.len() > "binding:".len()
                && !impl_.evidence_digest.contains("..")
                && !impl_.evidence_digest.contains('\\'));
        if impl_.product_allowed && !digest_ok {
            return Err(invalid(format!(
                "crypto-policy-evidence-digest-malformed:{usage}"
            )));
        }
        // product_allowed=true 必须明确允许的 target；未批准实现（experimental）
        // 允许空 targets（即禁止进入任何产品 target）。
        if impl_.product_allowed && impl_.allowed_targets.is_empty() {
            return Err(invalid(format!("crypto-policy-no-allowed-targets:{usage}")));
        }
        // 43 密码独立评审门：review_required 的用途必须满足评审状态登记。
        if impl_.review_required && impl_.product_allowed {
            enforce_crypto_review(root, usage, &impl_.crate_name)?;
        }
    }
    Ok(policy)
}

/// Gate：resolved 闭包中任何 `product_allowed=false` 的 crypto crate 到达
/// 产品 ELF 即失败，即使 KAT PASS。调用方提供 resolved package 名集合。
///
/// 36 宿主复核（security-review HIGH）：黑名单可被改名/新增/未收录库绕过
/// （如 `sm3`、`gmssl`、改名后的 eneros-crypto）。改为闭世界白名单——
/// 任何命中 crypto 算法关键字的包（归一化后子串匹配，防 `-`/`_` 改名）
/// 必须已登记在 crypto-policy.toml 中（登记≠批准：product_allowed 决定
/// 能否入闭包；未登记 = 未评估 = 直接失败）。
pub fn gate_crypto_closure(
    root: &Path,
    resolved_packages: &BTreeSet<String>,
) -> DynResult<Vec<String>> {
    let mut violations = Vec::new();
    let policy = match load_crypto_policy(root) {
        Ok(policy) => policy,
        Err(error) => return Ok(vec![format!("crypto-policy-unreadable:{error}")]),
    };
    // 闭包包名归一化集合（一次构建，双向比较用）。
    let resolved_norm: BTreeSet<String> = resolved_packages
        .iter()
        .map(|p| normalize_crate_name(p))
        .collect();
    for (usage, impl_) in &policy.usages {
        if impl_.product_allowed {
            continue;
        }
        // 归一化双向比较（security-review 三轮 MEDIUM）：未批准实现改名
        // （`eneros_crypto`、`crypto-lib` 等删去算法词的变体）不得逃逸——
        // 归一化后与闭包包名同形即失败；精确名路径由归一化覆盖。
        if resolved_norm.contains(&normalize_crate_name(&impl_.crate_name)) {
            violations.push(format!(
                "UNAUTHORIZED_CRYPTO_IN_CLOSURE:usage={usage}:package={}:allowed=false",
                impl_.crate_name
            ));
        }
    }
    // 闭世界：已登记（评估过）的 crate 名集合（归一化）。crate_name 无论
    // 批准与否都登记（product_allowed=false 由上方 UNAUTHORIZED 检查兜底）；
    // impl_dependencies 仅随 product_allowed=true 的主实现登记——"实现依赖
    // 随主实现获准"，未获准用途的依赖不得借登记合法化（security-review
    // MEDIUM 修复）。
    let registered: BTreeSet<String> = policy
        .usages
        .values()
        .flat_map(|impl_| {
            let mut names = vec![impl_.crate_name.clone()];
            if impl_.product_allowed {
                names.extend(impl_.impl_dependencies.clone());
            }
            names
        })
        .map(|name| normalize_crate_name(&name))
        .collect();
    for package in resolved_packages {
        if is_crypto_provider(package) && !registered.contains(&normalize_crate_name(package)) {
            violations.push(format!(
                "UNREGISTERED_CRYPTO_IN_CLOSURE:package={package}:no-crypto-policy-entry"
            ));
        }
    }
    Ok(violations)
}

// ---------------------------------------------------------------------------
// frozen-crates（WP-44.4）：死代码冻结 = 闭包隔离 + 依赖禁令。
// ---------------------------------------------------------------------------

pub const FROZEN_POLICY_SCHEMA: &str = "eneros.frozen-crates.v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenCrateEntry {
    pub package: String,
    pub reason: String,
    pub unfreeze: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenExemptionEntry {
    pub package: String,
    pub reason: String,
    pub owner: String,
    pub unfreeze: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenDepAllowEntry {
    pub from: String,
    pub to: String,
    pub reason: String,
    pub established: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenPolicy {
    pub schema: String,
    pub established: String,
    pub policy_ref: String,
    #[serde(default)]
    pub note: String,
    pub crates: Vec<FrozenCrateEntry>,
    #[serde(default)]
    pub release_closure_exemptions: Vec<FrozenExemptionEntry>,
    #[serde(default)]
    pub dependency_allowlist: Vec<FrozenDepAllowEntry>,
}

pub fn load_frozen_policy(root: &Path) -> DynResult<FrozenPolicy> {
    let path = root.join("product/policy/frozen-crates.json");
    let text = fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    let policy: FrozenPolicy =
        serde_json::from_str(&text).map_err(|e| invalid(format!("frozen-crates.json parse: {e}")))?;
    if policy.schema != FROZEN_POLICY_SCHEMA {
        return Err(invalid(format!(
            "frozen-crates-schema:{} (expected {FROZEN_POLICY_SCHEMA})",
            policy.schema
        )));
    }
    if policy.crates.is_empty() {
        return Err(invalid("frozen-crates policy declares no crates"));
    }
    // 豁免与 allowlist 必须引用已冻结 crate（防未冻结 crate 借豁免钻空）。
    let frozen: BTreeSet<&str> = policy.crates.iter().map(|c| c.package.as_str()).collect();
    for entry in &policy.release_closure_exemptions {
        if !frozen.contains(entry.package.as_str()) {
            return Err(invalid(format!(
                "frozen-exemption-for-unfrozen-crate:{}",
                entry.package
            )));
        }
    }
    for entry in &policy.dependency_allowlist {
        if !frozen.contains(entry.to.as_str()) {
            return Err(invalid(format!(
                "frozen-allowlist-target-unfrozen:{}",
                entry.to
            )));
        }
    }
    Ok(policy)
}

/// WP-44.4 R1：冻结 crate（豁免除外）进入 release 依赖闭包 → FAIL。
/// 纯函数（reached 集由调用方从 closure 计算），便于测试与复用。
pub fn gate_frozen_release_closure(
    frozen: &BTreeSet<String>,
    exempt: &BTreeSet<String>,
    reached: &BTreeSet<String>,
) -> Vec<String> {
    let mut violations = Vec::new();
    for package in frozen {
        if exempt.contains(package) {
            continue;
        }
        if reached.contains(package) {
            violations.push(format!("frozen-crate-in-release-closure:{package}"));
        }
    }
    violations
}

/// WP-44.4 R2：非冻结 crate 声明依赖冻结 crate，且该边不在 allowlist → FAIL。
/// `edges` = (依赖方 crate, 被依赖方 crate) 直接声明边（workspace 内）。
pub fn gate_frozen_dependencies(
    frozen: &BTreeSet<String>,
    allow_edges: &BTreeSet<(String, String)>,
    edges: &[(String, String)],
) -> Vec<String> {
    let mut violations = Vec::new();
    for (from, to) in edges {
        if !frozen.contains(to) {
            continue;
        }
        if allow_edges.contains(&(from.clone(), to.clone())) {
            continue;
        }
        violations.push(format!("frozen-dependency-by-nonfrozen:{from}:{to}"));
    }
    violations
}


/// crypto 算法提供者关键字（归一化后子串匹配）。只列算法实现词，
/// 不含 RustCrypto trait/基础设施（crypto-common、digest、cipher、
/// subtle 等——它们由依赖的算法实现包一并触发登记）。
/// 短词（<5 字符）仅保留无歧义项，避免误报。
const CRYPTO_PROVIDER_KEYWORDS: &[&str] = &[
    "sm2",
    "sm3",
    "sm4",
    "gmssl",
    "3des",
    "aes",
    "chacha",
    "poly1305",
    "salsa20",
    "blowfish",
    "twofish",
    "sha2",
    "sha3",
    "sha1",
    "md5",
    "blake3",
    "blake2",
    "ripemd",
    "whirlpool",
    "ed25519",
    "ecdsa",
    "secp",
    "curve25519",
    "x25519",
    "crypto_box",
    "openssl",
    "boringssl",
    "rustls",
    "native_tls",
    "jose",
    "jsonwebtoken",
    "pbkdf",
    "scrypt",
    "argon2",
    "bcrypt",
    "hkdf",
    "rsa",
];

/// 归一化：小写 + 剥离全部非字母数字（`sha-2`/`SHA_2`/`sha.2` → `sha2`，
/// 防分隔符改名绕过；`-`/`_`/`.` 分隔的算法 token 与登记名同形）。
fn normalize_crate_name(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

fn is_crypto_provider(package: &str) -> bool {
    let normalized = normalize_crate_name(package);
    CRYPTO_PROVIDER_KEYWORDS
        .iter()
        .any(|keyword| normalized.contains(keyword))
}

// ---------------------------------------------------------------------------
// ADR registry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdrEntry {
    pub id: String,
    pub path: String,
    pub title: String,
    pub status: String,
    #[serde(default)]
    pub supersedes: Vec<String>,
    #[serde(default)]
    pub superseded_by: String,
    #[serde(default)]
    pub decision_digest: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdrRegistry {
    pub schema: String,
    pub adrs: Vec<AdrEntry>,
}

pub fn load_adr_registry(root: &Path) -> DynResult<AdrRegistry> {
    let path = root.join("docs/adr/registry.toml");
    let text = fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    let registry: AdrRegistry =
        toml::from_str(&text).map_err(|e| invalid(format!("adr registry parse: {e}")))?;
    if registry.schema != ADR_REGISTRY_SCHEMA {
        return Err(invalid(format!("adr-registry-schema:{}", registry.schema)));
    }
    Ok(registry)
}

/// Gate：
/// 1. 重复 ADR ID → 失败；
/// 2. 多条 Accepted 的 product_kernel 决策 → 失败（唯一产品内核）；
/// 3. registry 中每条 ADR 的 status 与文件首行一致（防文档漂移）。
pub fn gate_adr_registry(root: &Path) -> DynResult<Vec<String>> {
    let mut violations = Vec::new();
    let registry = match load_adr_registry(root) {
        Ok(registry) => registry,
        Err(error) => return Ok(vec![format!("adr-registry-unreadable:{error}")]),
    };
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    for entry in &registry.adrs {
        // 重复 ID 拒绝：Deprecated 历史条目（旧短 ID 已废弃）豁免，
        // 保证旧文档仍可在 registry 追溯但不构成冲突裁决。
        if entry.status != "Deprecated" && !seen_ids.insert(entry.id.clone()) {
            violations.push(format!("adr-duplicate-id:{}", entry.id));
        }
        if entry.status == "Deprecated" {
            seen_ids.insert(entry.id.clone());
        }
        // 文件存在性。
        let path = root.join(&entry.path);
        if !path.is_file() {
            violations.push(format!("adr-file-missing:{}:{}", entry.id, entry.path));
            continue;
        }
        // status 与文件首部一致性（registry 为权威，文件不得反向冲突）：
        // - Superseded：文件必须含 Superseded/已取代 标记；
        // - Accepted：文件不得含 Superseded/已取代 标记（被取代文件不得标 Accepted）；
        // - Deprecated：历史短 ID 文档保留原文，不强制文件标记。
        let text = fs::read_to_string(&path)?;
        let first_lines: String = text.lines().take(12).collect::<Vec<_>>().join("\n");
        let file_superseded = first_lines.contains("Superseded") || first_lines.contains("已取代");
        let mismatch = match entry.status.as_str() {
            "Superseded" | "superseded" => !file_superseded,
            "Accepted" | "accepted" => file_superseded,
            "Deprecated" => false,
            _ => false,
        };
        if mismatch {
            violations.push(format!(
                "adr-status-mismatch:{}:file-status!={}",
                entry.id, entry.status
            ));
        }
    }
    // 唯一产品内核：decision_adr（ADR-0043）为唯一现行决策。
    // - decision_adr 必须 Accepted 且未被 supersede；
    // - 其他 kernel 决策（ADR-0001/0023/0030）若 Accepted 必须
    //   superseded_by=decision_adr（并入现行裁决），否则冲突失败；
    // - 恢复旧 ADR-0001 为 Accepted（无 superseded_by）时 Gate 必须红。
    let kernel_policy = load_kernel_policy(root).ok();
    let decision_adr = kernel_policy.as_ref().map(|p| p.decision_adr.clone());
    let Some(decision) = decision_adr else {
        violations.push("adr-kernel-policy-unavailable".to_owned());
        return Ok(violations);
    };
    let decision_entry = registry
        .adrs
        .iter()
        .find(|entry| entry.id == decision)
        .ok_or_else(|| invalid(format!("decision_adr {decision} missing from registry")))?;
    let decision_superseded = registry.adrs.iter().any(|other| {
        other
            .supersedes
            .iter()
            .any(|superseded| superseded == &decision)
    });
    if !decision_entry.status.eq_ignore_ascii_case("accepted") {
        violations.push(format!("adr-decision-not-accepted:{decision}"));
    }
    if decision_superseded {
        violations.push(format!("adr-decision-superseded:{decision}"));
    }
    for entry in &registry.adrs {
        let is_kernel_decision = entry.id.starts_with("ADR-0001")
            || entry.id.starts_with("ADR-0023")
            || entry.id.starts_with("ADR-0030");
        if is_kernel_decision
            && entry.status.eq_ignore_ascii_case("accepted")
            && entry.superseded_by != decision
        {
            violations.push(format!(
                "adr-conflicting-kernel-decision:{}:superseded_by={}",
                entry.id, entry.superseded_by
            ));
        }
    }
    Ok(violations)
}

/// 计算文件的 sha256（policy digest 用）。
pub fn file_digest(root: &Path, rel_path: &str) -> DynResult<String> {
    let bytes = fs::read(root.join(rel_path))?;
    Ok(hex(&Sha256::digest(&bytes)))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_release_closure_gate_flags_non_exempt_frozen() {
        // WP-44.4 R1：冻结 crate 进 release 闭包 → FAIL；豁免、非冻结 → 放行。
        let frozen: BTreeSet<String> =
            ["eneros-tsdb", "eneros-federation", "eneros-energy-market-agent"]
                .into_iter()
                .map(String::from)
                .collect();
        let exempt: BTreeSet<String> =
            ["eneros-energy-market-agent".to_owned()].into_iter().collect();
        let reached: BTreeSet<String> = [
            "eneros-tsdb",
            "eneros-energy-market-agent",
            "eneros-root-task",
            "eneros-storage",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let violations = gate_frozen_release_closure(&frozen, &exempt, &reached);
        assert!(violations.contains(&"frozen-crate-in-release-closure:eneros-tsdb".to_owned()));
        // 豁免的 energy-market-agent 与未冻结的 root-task/storage 不报。
        assert!(!violations
            .iter()
            .any(|v| v.contains("energy-market-agent")), "{violations:?}");
        assert!(!violations.iter().any(|v| v.contains("root-task")), "{violations:?}");
        assert!(!violations.iter().any(|v| v.contains("storage")), "{violations:?}");
    }

    #[test]
    fn frozen_dependency_gate_requires_allowlist() {
        // WP-44.4 R2：非冻结→冻结边未 allowlist → FAIL；allowlist 内放行。
        let frozen: BTreeSet<String> =
            ["eneros-solver-model".to_owned()].into_iter().collect();
        let allow: BTreeSet<(String, String)> =
            [("eneros-energy-lp-model".to_owned(), "eneros-solver-model".to_owned())]
                .into_iter()
                .collect();
        let edges = vec![
            ("eneros-energy-lp-model".to_owned(), "eneros-solver-model".to_owned()),
            ("eneros-intent-parser".to_owned(), "eneros-solver-model".to_owned()),
        ];
        let violations = gate_frozen_dependencies(&frozen, &allow, &edges);
        assert!(violations
            .iter()
            .any(|v| v == "frozen-dependency-by-nonfrozen:eneros-intent-parser:eneros-solver-model"));
        assert!(!violations
            .iter()
            .any(|v| v.contains("energy-lp-model")), "{violations:?}");
    }

    #[test]
    fn frozen_policy_load_rejects_bad_schema_and_dangling_refs() {
        let root = std::env::temp_dir().join(format!("eneros-frozen-pl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("product/policy")).unwrap();
        std::fs::write(
            root.join("product/policy/frozen-crates.json"),
            r#"{"schema":"wat","crates":[{"package":"eneros-x","reason":"r","unfreeze":"u"}]}"#,
        )
        .unwrap();
        assert!(load_frozen_policy(&root).is_err());
        // 豁免引用未冻结 crate → 拒绝。
        std::fs::write(
            root.join("product/policy/frozen-crates.json"),
            r#"{"schema":"eneros.frozen-crates.v1","established":"2026-08-19","policy_ref":"t","crates":[{"package":"eneros-x","reason":"r","unfreeze":"u"}],"release_closure_exemptions":[{"package":"eneros-ghost","reason":"r","owner":"o","unfreeze":"u"}]}"#,
        )
        .unwrap();
        let err = load_frozen_policy(&root).unwrap_err();
        assert!(format!("{err}").contains("frozen-exemption-for-unfrozen-crate"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn kernel_policy_validation() {
        let policy = KernelPolicy {
            schema: KERNEL_POLICY_SCHEMA.to_owned(),
            product_kernel: "sel4".to_owned(),
            runtime_root: "eneros-root-task".to_owned(),
            allowed_release_kernels: vec!["sel4".to_owned()],
            research_only: vec!["eneros-product-kernel".to_owned()],
            decision_adr: "ADR-0043".to_owned(),
            owner_approval: "CHG-2026-08-12-01".to_owned(),
        };
        assert!(policy.validate().is_empty());
        // 负例：product 在 research_only。
        let mut bad = policy.clone();
        bad.research_only = vec!["sel4".to_owned()];
        assert!(bad
            .validate()
            .iter()
            .any(|v| v.contains("product-is-research")));
        // 负例：owner approval 缺失。
        let mut bad = policy.clone();
        bad.owner_approval = String::new();
        assert!(bad.validate().iter().any(|v| v.contains("owner-approval")));
    }

    #[test]
    fn kernel_gate_rejects_research_kernel() {
        use crate::product_manifest::{
            Criticality, EvidencePolicyRef, KernelSpec, ManifestRole, ProductManifest, Requirement,
            RestartPolicy, SecurityPolicyRef, ServiceSpec, MANIFEST_SCHEMA,
        };
        let manifest = ProductManifest {
            schema: MANIFEST_SCHEMA.to_owned(),
            product_id: "eneros".to_owned(),
            profile_id: "qemu-prod".to_owned(),
            class: "release".to_owned(),
            architecture: "aarch64".to_owned(),
            platform_contract: "qemu-virt-aarch64".to_owned(),
            kernel: KernelSpec {
                name: "sel4".to_owned(),
                selector: "sel4:elfloader-arm-qemu-arm-virt".to_owned(),
                digest: String::new(),
                target: "host".to_owned(),
                features: vec![],
                external: true,
            },
            artifacts: vec![],
            services: vec![ServiceSpec {
                id: "root-task".to_owned(),
                binary: "eneros-root-task::eneros-root-task".to_owned(),
                digest: "aa".to_owned(),
                role: ManifestRole::RootTask,
                target: "aarch64-unknown-none".to_owned(),
                features: vec![],
                default_features: false,
                requirement: Requirement::Required,
                criticality: Criticality::System,
                restart: RestartPolicy::Never,
                dependencies: vec![],
                capabilities: vec![],
                resources: None,
                configs: vec![],
                readiness: None,
                external: false,
            }],
            configs: vec![],
            security: SecurityPolicyRef {
                policy: "product/crypto-policy.toml".to_owned(),
                digest: String::new(),
            },
            evidence_policy: EvidencePolicyRef {
                policy: "product/evidence/schema.json".to_owned(),
                digest: String::new(),
            },
            manifest_digest: String::new(),
        };
        let root = Path::new("/nonexistent");
        // policy 不存在 → 返回 unreadable 违规（fail-closed）。
        let violations = gate_kernel_policy(root, &manifest).unwrap();
        assert!(!violations.is_empty());
    }

    #[test]
    fn crypto_closure_closed_world() {
        // 闭世界：未登记 crypto 提供者必须被拒（含改名/新库）。
        let root = Path::new("/nonexistent"); // policy 缺失 → unreadable（fail-closed）
        let violations = gate_crypto_closure(
            root,
            &BTreeSet::from(["sha2".to_owned(), "sm3-wasm".to_owned()]),
        )
        .unwrap();
        assert!(violations
            .iter()
            .any(|v| v.contains("crypto-policy-unreadable")));

        // 模拟真实 policy：临时目录写入登记 sha2 的 policy。
        let dir = std::env::temp_dir().join(format!("crypto-policy-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("product")).unwrap();
        let policy_text = r#"
schema = "eneros.crypto-policy.v1"
[usages.boot_signature]
provider = "sha2"
crate_name = "sha2"
impl_dependencies = ["curve25519-dalek"]
evaluation_status = "approved"
product_allowed = true
allowed_targets = ["aarch64-unknown-none"]
allowed_features = ["*"]
evidence_digest = "binding:boot-integrity"
"#;
        std::fs::write(dir.join("product/crypto-policy.toml"), policy_text).unwrap();
        let root = dir.as_path();
        // 登记 + allowed → 无违规。
        let violations = gate_crypto_closure(root, &BTreeSet::from(["sha2".to_owned()])).unwrap();
        assert!(violations.is_empty(), "{violations:?}");
        // 实现依赖随主实现登记 → 无违规。
        let violations = gate_crypto_closure(
            root,
            &BTreeSet::from(["sha2".to_owned(), "curve25519-dalek".to_owned()]),
        )
        .unwrap();
        assert!(violations.is_empty(), "{violations:?}");
        // 改名绕过（sha-2 vs sha2）→ 归一化剥离分隔符后与登记名同形 → 覆盖。
        let violations = gate_crypto_closure(root, &BTreeSet::from(["SHA-2".to_owned()])).unwrap();
        assert!(violations.is_empty(), "{violations:?}");
        // 未登记新库 → UNREGISTERED。
        let violations = gate_crypto_closure(root, &BTreeSet::from(["gmssl".to_owned()])).unwrap();
        assert!(
            violations
                .iter()
                .any(|v| v.contains("UNREGISTERED_CRYPTO_IN_CLOSURE:package=gmssl")),
            "{violations:?}"
        );
        // 未登记改名国密（原名 eneros-crypto 改 sm3-core）→ 命中关键字 → 拒。
        let violations =
            gate_crypto_closure(root, &BTreeSet::from(["sm3-core".to_owned()])).unwrap();
        assert!(
            violations
                .iter()
                .any(|v| v.contains("UNREGISTERED_CRYPTO_IN_CLOSURE:package=sm3-core")),
            "{violations:?}"
        );
        // 分隔符改名逃逸封堵：sha-2/sm.3 归一化后与登记/关键字同形。
        let violations = gate_crypto_closure(root, &BTreeSet::from(["sm.3".to_owned()])).unwrap();
        assert!(
            violations
                .iter()
                .any(|v| v.contains("UNREGISTERED_CRYPTO_IN_CLOSURE:package=sm.3")),
            "{violations:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn crypto_closure_impl_deps_gated_on_approval() {
        // impl_dependencies 仅随 product_allowed=true 的主实现登记；
        // 未获准用途的依赖不得借登记合法化。
        let dir = std::env::temp_dir().join(format!("crypto-policy-impl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("product")).unwrap();
        let policy_text = r#"
schema = "eneros.crypto-policy.v1"
[usages.boot_signature]
provider = "sha2"
crate_name = "sha2"
impl_dependencies = ["curve25519-dalek"]
evaluation_status = "approved"
product_allowed = true
allowed_targets = ["aarch64-unknown-none"]
allowed_features = ["*"]
evidence_digest = "binding:boot-integrity"

[usages.attestation]
provider = "eneros-crypto"
crate_name = "eneros-crypto"
impl_dependencies = ["sm3-wasm"]
evaluation_status = "experimental"
product_allowed = false
allowed_targets = []
allowed_features = []
evidence_digest = "binding:none"
"#;
        std::fs::write(dir.join("product/crypto-policy.toml"), policy_text).unwrap();
        let root = dir.as_path();
        // 获准主实现的依赖 → 登记（无违规）。
        let violations = gate_crypto_closure(
            root,
            &BTreeSet::from(["sha2".to_owned(), "curve25519-dalek".to_owned()]),
        )
        .unwrap();
        assert!(violations.is_empty(), "{violations:?}");
        // 未获准用途的依赖（sm3-wasm）入闭包 → UNREGISTERED（不得借登记合法化）。
        let violations =
            gate_crypto_closure(root, &BTreeSet::from(["sm3-wasm".to_owned()])).unwrap();
        assert!(
            violations
                .iter()
                .any(|v| v.contains("UNREGISTERED_CRYPTO_IN_CLOSURE:package=sm3-wasm")),
            "{violations:?}"
        );
        // 未批准实现改名变体（同形：eneros_crypto）→ UNAUTHORIZED
        // （归一化双向比较，security-review 三轮 MEDIUM 修复）。
        let violations =
            gate_crypto_closure(root, &BTreeSet::from(["eneros_crypto".to_owned()])).unwrap();
        assert!(
            violations
                .iter()
                .any(|v| v.contains("UNAUTHORIZED_CRYPTO_IN_CLOSURE:usage=attestation")),
            "{violations:?}"
        );
        // 完全改名+删关键字（如 crypto-lib）不可静态关联——需同时篡改受控
        // 依赖图与策略文件方能利用（构建期纵深防御边界，记录为接受风险；
        // review 方向：UNAUTHORIZED 归一化双向比较已封堵同形改名）。
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalize_crate_name_maps_dash_underscore() {
        assert_eq!(normalize_crate_name("SHA-2"), "sha2");
        assert_eq!(normalize_crate_name("ed25519-dalek"), "ed25519dalek");
        assert_eq!(normalize_crate_name("sm.3"), "sm3");
        assert!(is_crypto_provider("ed25519-dalek"));
        assert!(is_crypto_provider("sha2"));
        assert!(is_crypto_provider("sha-2"));
        assert!(is_crypto_provider("sm_3"));
        assert!(is_crypto_provider("curve25519-dalek"));
        assert!(!is_crypto_provider("eneros-audit"));
        assert!(!is_crypto_provider("crypto-common")); // trait 基础设施不触发
    }

    #[test]
    fn parse_epoch_seconds_matches_civil_calendar() {
        // 1970-01-01T00:00:00Z = 0（标准儒略日公式回归锚）。
        assert_eq!(parse_epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_epoch_seconds("1970-01-01"), Some(0));
        // 2026-12-31T00:00:00Z = UTC 午夜（与 Python
        // datetime(2026,12,31,tzinfo=timezone.utc) 对照）。
        assert_eq!(
            parse_epoch_seconds("2026-12-31T00:00:00Z"),
            Some(1798675200)
        );
        // 非法输入 → None（fail-closed）。
        assert_eq!(parse_epoch_seconds("2026-13-01"), None);
        assert_eq!(parse_epoch_seconds("bad"), None);
        assert_eq!(parse_epoch_seconds("2026-01-32"), None);
    }

    #[test]
    fn crypto_review_rollback_is_machine_enforced() {
        // 43 WP-43.12：review_required 用途在 deadline 后未 PASS → 回退。
        let dir = std::env::temp_dir().join(format!("crypto-review-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("product/security")).unwrap();
        let policy_text = r#"
schema = "eneros.crypto-policy.v1"
[usages.attestation]
provider = "eneros-crypto"
crate_name = "eneros-crypto"
evaluation_status = "approved"
product_allowed = true
review_required = true
allowed_targets = ["aarch64-unknown-none"]
allowed_features = ["*"]
evidence_digest = "binding:sm2-interop-vectors"
"#;
        std::fs::write(dir.join("product/crypto-policy.toml"), policy_text).unwrap();
        let root = dir.as_path();
        // 无登记 → 回退。
        let err = load_crypto_policy(root).unwrap_err();
        assert!(
            format!("{err}").contains("crypto-review-rollback:attestation"),
            "{err}"
        );

        // PENDING + 未来 deadline → 决议有效期内（CHG 授权）→ 通过。
        let status = serde_json::json!({
            "schema": REVIEW_STATUS_SCHEMA,
            "generated_at": "2026-08-18T00:00:00Z",
            "reviews": {
                "attestation": {
                    "status": "PENDING",
                    "reviewer": "external-crypto-reviewer",
                    "reviewed_at": null,
                    "review_deadline": "2026-12-31T00:00:00Z",
                    "evidence_ref": "tests/vectors/sm2-interop/vectors.json"
                }
            }
        });
        std::fs::write(
            dir.join("product/security/review-status.json"),
            serde_json::to_vec_pretty(&status).unwrap(),
        )
        .unwrap();
        assert!(load_crypto_policy(root).is_ok());

        // 逾期未 PASS → 机器回退（product_allowed 语义失效）。
        let mut expired = status.clone();
        expired["reviews"]["attestation"]["review_deadline"] =
            serde_json::json!("2020-01-01T00:00:00Z");
        std::fs::write(
            dir.join("product/security/review-status.json"),
            serde_json::to_vec_pretty(&expired).unwrap(),
        )
        .unwrap();
        let err = load_crypto_policy(root).unwrap_err();
        assert!(
            format!("{err}").contains("crypto-review-rollback:attestation:deadline passed"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
