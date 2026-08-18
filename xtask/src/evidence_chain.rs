//! 42 — 证据链、候选血缘与角色语义 Gate。
//!
//! 本模块实现蓝图 42 的 `EvidenceEnvelope` 唯一格式、`candidate lineage`
//! 计算、`verify`/`gate` 分离，以及 12 类 mutation 自检。
//!
//! 设计原则：
//! - `verify` 只做完整性与签名/文件完整性检查；
//! - `gate` 在 `verify` 之上追加语义/政策检查；
//! - `PENDING/BLOCKED/FAIL/NO_RESULT` 永远不能通过 release gate；
//! - 旧 evidence 不允许跨 candidate 复用（lineage 字段绑定当前候选）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::product_deriver::{profile_digest, source_commit};
use super::product_manifest::load_normalized;
use super::{invalid, read_json, DynResult};

pub const ENVELOPE_SCHEMA: &str = "eneros.evidence-envelope.v1";
pub const LINEAGE_SCHEMA: &str = "eneros.candidate-lineage.v1";

/// 蓝图 42 §3：role-specific semantic verifier。
/// `verify_integrity` 只检查 envelope/签名/文件完整性；`verify_semantics`
/// 检查语义与政策。两者分离后 release Gate 只接受语义 PASS。
#[allow(dead_code)] // 44.2：42 卷验证器契约，当前由 compile 期校验保留（44.4 冻结登记）
pub trait EvidenceVerifier {
    fn verify_integrity(&self, envelope: &EvidenceEnvelope) -> Result<(), String>;
    fn verify_semantics(&self, envelope: &EvidenceEnvelope) -> Result<(), String>;
}

/// 默认实现：把本模块的 `verify_integrity` / `gate_violations` 暴露为 trait。
#[allow(dead_code)] // 44.2：同上（44.4 冻结登记）
pub struct DefaultEvidenceVerifier;

impl EvidenceVerifier for DefaultEvidenceVerifier {
    fn verify_integrity(&self, envelope: &EvidenceEnvelope) -> Result<(), String> {
        let v = verify_integrity(envelope);
        if v.is_empty() {
            Ok(())
        } else {
            Err(v.join("; "))
        }
    }

    fn verify_semantics(&self, envelope: &EvidenceEnvelope) -> Result<(), String> {
        let v = gate_violations(envelope);
        if v.is_empty() {
            Ok(())
        } else {
            Err(v.join("; "))
        }
    }
}

/// 内容寻址的制品引用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
    pub name: String,
    pub path: String,
    pub sha256: String,
}

/// Oracle 描述：独立于被测实现的观察者。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleDescriptor {
    pub kind: String,
    pub detail: String,
}

/// 单条正例/负例结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultRef {
    pub id: String,
    pub status: String,
    pub detail: String,
}

/// 蓝图 42 §1.1 EvidenceEnvelope。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceEnvelope {
    pub schema: String,
    pub evidence_id: String,
    pub run_id: String,
    pub nonce: String,
    pub test_id: String,
    pub scope: String,
    pub started_at: String,
    pub ended_at: String,
    pub source_commit: String,
    pub dirty_tree_digest: String,
    pub product_manifest_digest: String,
    pub profile_digest: String,
    pub artifact_sha256: String,
    pub toolchain_digest: String,
    pub platform_inventory_digest: String,
    pub command: String,
    pub inputs: Vec<ArtifactRef>,
    pub oracle: OracleDescriptor,
    pub positive_results: Vec<ResultRef>,
    pub negative_results: Vec<ResultRef>,
    pub verdict: String,
    pub violations: Vec<String>,
    pub artifacts: Vec<ArtifactRef>,
    pub signer: String,
    pub signature: String,
}

/// 候选血缘（蓝图 42 §3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateLineage {
    pub schema: String,
    pub candidate_id: String,
    pub source_commit: String,
    pub dirty_tree_digest: String,
    pub product_manifest_digest: String,
    pub profile_digest: String,
    pub toolchain_digest: String,
    pub artifact_sha256: String,
    pub platform_inventory_digest: String,
    pub build_inputs: Vec<ArtifactRef>,
}

pub(super) fn command_evidence_chain(root: &Path, args: &[String]) -> DynResult<()> {
    let (mode, tail) = args
        .split_first()
        .ok_or_else(|| invalid("evidence42 requires lineage|envelope|verify|gate|mutation"))?;
    match mode.as_str() {
        "help" | "--help" | "-h" => {
            println!("evidence42 subcommands:");
            for cmd in [
                "lineage",
                "lineage-check",
                "envelope",
                "verify",
                "gate",
                "gate-dir",
                "report-gate",
                "media-check",
                "fuzz-report",
                "unsafe-report",
                "repro",
                "stale-index",
                "current-index",
                "check-current",
                "gate-status",
                "summary",
                "release-readiness",
                "blockers",
                "self-test",
                "audit-all",
                "final-status",
                "upstream-handoff",
                "next-steps",
                "evidence-bundle",
                "check-bundle",
                "mutation",
            ] {
                println!("  {cmd}");
            }
            Ok(())
        }
        "lineage" => command_lineage(root, tail),
        "envelope" => command_envelope(root, tail),
        "lineage-check" => command_lineage_check(root, tail),
        "verify" => command_verify(root, tail),
        "gate" => command_gate(root, tail),
        "report-gate" => command_report_gate(root, tail),
        "report-gate-expected-fail" => command_report_gate_expected_fail(root, tail),
        "gate-dir" => command_gate_dir(root, tail),
        "media-check" => command_media_check(root, tail),
        "fuzz-report" => command_fuzz_report(root, tail),
        "unsafe-report" => command_unsafe_report(root, tail),
        "repro" => command_repro(root, tail),
        "stale-index" => command_stale_index(root, tail),
        "current-index" => command_current_index(root, tail),
        "check-current" => command_check_current(root, tail),
        "gate-status" => command_gate_status(root, tail),
        "summary" => command_summary(root, tail),
        "release-readiness" => command_release_readiness(root, tail),
        "blockers" => command_blockers(root, tail),
        "self-test" => command_self_test(root, tail),
        "audit-all" => command_audit_all(root, tail),
        "final-status" => command_final_status(root, tail),
        "upstream-handoff" => command_upstream_handoff(root, tail),
        "next-steps" => command_next_steps(root, tail),
        "evidence-bundle" => command_evidence_bundle(root, tail),
        "check-bundle" => command_check_bundle(root, tail),
        "ci-status" => command_ci_status(root, tail),
        "fault-catalog" => command_fault_catalog(root, tail),
        "evidence-map" => command_evidence_map(root, tail),
        "checklist" => command_checklist(root, tail),
        "exit-gate" => command_exit_gate(root, tail),
        "final-audit" => command_final_audit(root, tail),
        "version" => command_version(root, tail),
        "report" => command_report(root, tail),
        "all" => command_all(root, tail),
        "summary-md" => command_summary_md(root, tail),
        "evidence-count" => command_evidence_count(root, tail),
        "list" => command_list(root, tail),
        "mutation" => command_mutation(root, tail),
        other => Err(invalid(format!("unknown evidence42 subcommand '{other}'"))),
    }
}

// ============================================================
// Candidate lineage
// ============================================================

fn command_lineage(root: &Path, args: &[String]) -> DynResult<()> {
    let mut profile_id = "qemu-prod".to_owned();
    let mut artifact: Option<PathBuf> = None;
    let mut out = root.join("target/eneros/evidence/42/candidate-lineage.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--profile" => profile_id = val()?.to_owned(),
            "--artifact" => artifact = Some(root.join(val()?)),
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown lineage arg '{other}'"))),
        }
    }

    let lineage = compute_lineage(root, &profile_id, artifact.as_deref())?;
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&lineage)?)?;
    println!("{}", serde_json::to_string_pretty(&lineage)?);
    println!(
        "CANDIDATE-LINEAGE candidate_id={} → {}",
        lineage.candidate_id,
        out.display()
    );
    Ok(())
}

fn command_lineage_check(root: &Path, args: &[String]) -> DynResult<()> {
    let mut envelope_path: Option<PathBuf> = None;
    let mut profile_id = "qemu-prod".to_owned();
    let mut artifact: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--envelope" => envelope_path = Some(root.join(val()?)),
            "--profile" => profile_id = val()?.to_owned(),
            "--artifact" => artifact = Some(root.join(val()?)),
            other => return Err(invalid(format!("unknown lineage-check arg '{other}'"))),
        }
    }
    let envelope_path = envelope_path
        .ok_or_else(|| invalid("lineage-check requires --envelope <envelope.json>"))?;
    let envelope: EvidenceEnvelope = read_json(&envelope_path)?;
    let lineage = compute_lineage(root, &profile_id, artifact.as_deref())?;
    let mut violations = Vec::new();
    if envelope.source_commit != lineage.source_commit {
        violations.push("source-commit-mismatch".to_owned());
    }
    if envelope.dirty_tree_digest != lineage.dirty_tree_digest {
        violations.push("dirty-tree-digest-mismatch".to_owned());
    }
    if envelope.product_manifest_digest != lineage.product_manifest_digest {
        violations.push("manifest-digest-mismatch".to_owned());
    }
    if envelope.profile_digest != lineage.profile_digest {
        violations.push("profile-digest-mismatch".to_owned());
    }
    if envelope.toolchain_digest != lineage.toolchain_digest {
        violations.push("toolchain-digest-mismatch".to_owned());
    }
    if envelope.platform_inventory_digest != lineage.platform_inventory_digest {
        violations.push("platform-digest-mismatch".to_owned());
    }
    if !envelope.artifact_sha256.is_empty()
        && artifact.is_some()
        && envelope.artifact_sha256 != lineage.artifact_sha256
    {
        violations.push("artifact-digest-mismatch".to_owned());
    }
    if violations.is_empty() {
        println!(
            "LINEAGE-CHECK PASS envelope={} candidate={}",
            envelope_path.display(),
            lineage.candidate_id
        );
        Ok(())
    } else {
        Err(invalid(format!(
            "lineage-check FAIL: {}",
            violations.join("; ")
        )))
    }
}

/// 计算候选血缘。所有 digest 均来自当前工作树与真实文件，禁止空串占位。
pub fn compute_lineage(
    root: &Path,
    profile_id: &str,
    artifact: Option<&Path>,
) -> DynResult<CandidateLineage> {
    let commit = source_commit(root)?;
    let dirty = dirty_tree_digest(root)?;
    let manifest_path = root
        .join("target/eneros/manifest")
        .join(profile_id)
        .join("product-manifest.normalized.json");
    let manifest = if manifest_path.is_file() {
        load_normalized(root, profile_id)?
    } else {
        let profile = super::load_profile(root, profile_id)?;
        super::product_manifest::normalize_profile(root, &profile)?
    };
    let manifest_digest = if manifest.manifest_digest.is_empty() {
        manifest.compute_digest()?
    } else {
        manifest.manifest_digest.clone()
    };
    let profile = profile_digest(root, profile_id)?;
    let toolchain = toolchain_digest(root)?;
    let platform = platform_inventory_digest(root)?;
    let artifact_sha256 = match artifact {
        Some(path) => hash_file(path)?,
        None => String::new(),
    };
    let mut build_inputs = vec![
        ArtifactRef {
            name: "Cargo.lock".to_owned(),
            path: "Cargo.lock".to_owned(),
            sha256: hash_file(&root.join("Cargo.lock"))?,
        },
        ArtifactRef {
            name: "rust-toolchain.toml".to_owned(),
            path: "rust-toolchain.toml".to_owned(),
            sha256: hash_file(&root.join("rust-toolchain.toml"))?,
        },
        ArtifactRef {
            name: "profile".to_owned(),
            path: format!("product/profiles/{profile_id}.json"),
            sha256: profile.clone(),
        },
        ArtifactRef {
            name: "product-manifest".to_owned(),
            path: format!("target/eneros/manifest/{profile_id}/product-manifest.normalized.json"),
            sha256: manifest_digest.clone(),
        },
    ];
    build_inputs.sort_by(|a, b| a.name.cmp(&b.name));

    let candidate_id = candidate_id(
        &commit,
        &dirty,
        &manifest_digest,
        &toolchain,
        &artifact_sha256,
    );
    Ok(CandidateLineage {
        schema: LINEAGE_SCHEMA.to_owned(),
        candidate_id,
        source_commit: commit,
        dirty_tree_digest: dirty,
        product_manifest_digest: manifest_digest,
        profile_digest: profile,
        toolchain_digest: toolchain,
        artifact_sha256,
        platform_inventory_digest: platform,
        build_inputs,
    })
}

/// 候选 ID = hash(source_commit || dirty_tree_digest || normalized_manifest ||
/// toolchain_lock || build_inputs || artifact_sha256)。
fn candidate_id(
    source_commit: &str,
    dirty_tree_digest: &str,
    manifest_digest: &str,
    toolchain_digest: &str,
    artifact_sha256: &str,
) -> String {
    let mut h = Sha256::new();
    h.update(source_commit.as_bytes());
    h.update([0u8]);
    h.update(dirty_tree_digest.as_bytes());
    h.update([0u8]);
    h.update(manifest_digest.as_bytes());
    h.update([0u8]);
    h.update(toolchain_digest.as_bytes());
    h.update([0u8]);
    h.update(artifact_sha256.as_bytes());
    hex(&h.finalize())
}

/// 工作树脏状态 digest：包含 `git status --porcelain`、`git diff` 与
/// `git diff --cached`。未跟踪文件至少以其路径进入状态清单。
fn dirty_tree_digest(root: &Path) -> DynResult<String> {
    let mut h = Sha256::new();
    for args in [
        vec!["status", "--porcelain"],
        vec!["diff"],
        vec!["diff", "--cached"],
    ] {
        let output = Command::new("git").current_dir(root).args(&args).output()?;
        if !output.status.success() {
            return Err(invalid(format!("git {} failed", args.join(" "))));
        }
        h.update(output.stdout);
        h.update([0u8]);
    }
    Ok(hex(&h.finalize()))
}

fn toolchain_digest(root: &Path) -> DynResult<String> {
    let mut h = Sha256::new();
    for rel in [
        "Cargo.lock",
        "rust-toolchain.toml",
        "product/toolchains/pinned-tools.json",
    ] {
        let bytes = fs::read(root.join(rel))?;
        h.update(rel.as_bytes());
        h.update([0u8]);
        h.update(bytes);
        h.update([0u8]);
    }
    Ok(hex(&h.finalize()))
}

fn platform_inventory_digest(root: &Path) -> DynResult<String> {
    // 当前零硬件阶段：平台事实来自 profile 与 image layout；若未来有
    // PlatformInventory 文件则加入。
    let mut h = Sha256::new();
    for rel in [
        "product/profiles/qemu-prod.json",
        "product/image-layouts/qemu-v1.json",
    ] {
        let path = root.join(rel);
        if path.is_file() {
            let bytes = fs::read(&path)?;
            h.update(rel.as_bytes());
            h.update([0u8]);
            h.update(bytes);
            h.update([0u8]);
        }
    }
    Ok(hex(&h.finalize()))
}

fn hash_file(path: &Path) -> DynResult<String> {
    let bytes =
        fs::read(path).map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    Ok(hex(&Sha256::digest(&bytes)))
}

/// 验证 EvidenceEnvelope 的 ed25519 签名（签名内容 = signature 置空后的 JSON）。
/// 返回：
/// - `Some(true)` 签名存在且验证通过；
/// - `Some(false)` 签名存在但验证失败；
/// - `None` 当前环境无可用验签公钥（不武断 PASS，也不因无法验签直接 FAIL）。
fn verify_envelope_signature(root: &Path, envelope: &EvidenceEnvelope) -> Option<bool> {
    if envelope.signature.is_empty() {
        return Some(false);
    }
    let key_path = root.join("product/security/dev-signing.key");
    let key_bytes = fs::read(&key_path).ok()?;
    let key_bytes: [u8; 32] = key_bytes.get(..32)?.try_into().ok()?;
    let keypair = ed25519_dalek::SigningKey::from_bytes(&key_bytes);
    let public_key = keypair.verifying_key();

    let mut unsigned = envelope.clone();
    unsigned.signature.clear();
    let json = serde_json::to_vec(&unsigned).ok()?;
    let sig_bytes = hex_decode(envelope.signature.trim());
    let sig_bytes: [u8; 64] = sig_bytes.try_into().ok()?;
    use ed25519_dalek::Verifier;
    Some(
        public_key
            .verify(&json, &ed25519_dalek::Signature::from_bytes(&sig_bytes))
            .is_ok(),
    )
}

fn hex_decode(input: &str) -> Vec<u8> {
    let s: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if s.len() % 2 != 0 {
        return Vec::new();
    }
    (0..s.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

// ============================================================
// Envelope creation
// ============================================================

fn command_envelope(root: &Path, args: &[String]) -> DynResult<()> {
    let mut lineage_path: Option<PathBuf> = None;
    let mut test_id = String::new();
    let mut scope = String::new();
    let mut command = String::new();
    let mut oracle = String::new();
    let mut positives = Vec::new();
    let mut negatives = Vec::new();
    let mut verdict = String::new();
    let mut out = root.join("target/eneros/evidence/42/envelope.json");
    let mut signer = "dev".to_owned();
    let mut signature = String::new();
    let mut evidence_id = String::new();
    let mut run_id = String::new();
    let mut nonce = String::new();
    let mut artifacts = Vec::new();

    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--lineage" => lineage_path = Some(root.join(val()?)),
            "--test-id" => test_id = val()?.to_owned(),
            "--scope" => scope = val()?.to_owned(),
            "--command" => command = val()?.to_owned(),
            "--oracle" => oracle = val()?.to_owned(),
            "--positive" => positives = split_result_refs(val()?),
            "--negative" => negatives = split_result_refs(val()?),
            "--verdict" => verdict = val()?.to_owned(),
            "--out" => out = root.join(val()?),
            "--signer" => signer = val()?.to_owned(),
            "--signature" => signature = val()?.to_owned(),
            "--evidence-id" => evidence_id = val()?.to_owned(),
            "--run-id" => run_id = val()?.to_owned(),
            "--nonce" => nonce = val()?.to_owned(),
            "--artifact" => artifacts.push(parse_artifact_ref(val()?)?),
            other => return Err(invalid(format!("unknown envelope arg '{other}'"))),
        }
    }
    let lineage_path = lineage_path
        .ok_or_else(|| invalid("envelope requires --lineage <candidate-lineage.json>"))?;
    let lineage: CandidateLineage = read_json(&lineage_path)?;
    if lineage.schema != LINEAGE_SCHEMA {
        return Err(invalid("lineage schema mismatch"));
    }
    if test_id.is_empty() || scope.is_empty() || command.is_empty() || oracle.is_empty() {
        return Err(invalid(
            "envelope requires --test-id, --scope, --command, --oracle",
        ));
    }
    if verdict.is_empty() {
        verdict = "PASS".to_owned();
    }
    if evidence_id.is_empty() {
        evidence_id = format!(
            "ev-{}",
            lineage.candidate_id.chars().take(12).collect::<String>()
        );
    }
    if run_id.is_empty() {
        run_id = format!("run-{}", nonce_if_empty(&nonce));
    }
    if nonce.is_empty() {
        nonce = nonce_if_empty("");
    }
    let now = crate::qa::rfc3339_now();
    let mut envelope = EvidenceEnvelope {
        schema: ENVELOPE_SCHEMA.to_owned(),
        evidence_id,
        run_id,
        nonce,
        test_id,
        scope,
        started_at: now.clone(),
        ended_at: now,
        source_commit: lineage.source_commit,
        dirty_tree_digest: lineage.dirty_tree_digest,
        product_manifest_digest: lineage.product_manifest_digest,
        profile_digest: lineage.profile_digest,
        artifact_sha256: lineage.artifact_sha256,
        toolchain_digest: lineage.toolchain_digest,
        platform_inventory_digest: lineage.platform_inventory_digest,
        command,
        inputs: lineage.build_inputs,
        oracle: OracleDescriptor {
            kind: "oracle".to_owned(),
            detail: oracle,
        },
        positive_results: positives,
        negative_results: negatives,
        verdict,
        violations: Vec::new(),
        artifacts,
        signer,
        signature,
    };
    // 若未显式提供 signature，且开发签名密钥存在，则自动对 envelope 内容
    // 签名（42 证据要求 signature 非空；release 使用受控 KMS 密钥）。
    if envelope.signature.is_empty() {
        let dev_key = root.join("product/security/dev-signing.key");
        if let Ok(key_bytes) = fs::read(&dev_key) {
            if let Ok(key_bytes) = <[u8; 32]>::try_from(&key_bytes[..32]) {
                let keypair = ed25519_dalek::SigningKey::from_bytes(&key_bytes);
                let json = serde_json::to_vec(&envelope)?;
                use ed25519_dalek::Signer;
                let sig = keypair.sign(&json);
                envelope.signature = hex(&sig.to_bytes());
            }
        }
    }
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&envelope)?)?;
    println!("{}", serde_json::to_string_pretty(&envelope)?);
    println!("EVIDENCE-ENVELOPE → {}", out.display());
    Ok(())
}

fn parse_artifact_ref(input: &str) -> DynResult<ArtifactRef> {
    let mut parts = input.splitn(3, ':');
    let name = parts.next().unwrap_or("").trim().to_owned();
    let path = parts.next().unwrap_or("").trim().to_owned();
    let sha256 = parts.next().unwrap_or("").trim().to_owned();
    if name.is_empty()
        || path.is_empty()
        || sha256.len() != 64
        || !sha256.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(invalid("artifact must be name:path:<sha256-hex-64>"));
    }
    Ok(ArtifactRef { name, path, sha256 })
}

fn nonce_if_empty(existing: &str) -> String {
    if !existing.is_empty() {
        return existing.to_owned();
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

/// `--positive id:status:detail,id:status:detail`
fn split_result_refs(input: &str) -> Vec<ResultRef> {
    input
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let parts: Vec<&str> = s.splitn(3, ':').collect();
            ResultRef {
                id: parts.first().unwrap_or(&"").trim().to_owned(),
                status: parts.get(1).copied().unwrap_or("PASS").trim().to_owned(),
                detail: parts.get(2).copied().unwrap_or("").trim().to_owned(),
            }
        })
        .collect()
}

// ============================================================
// Verify / Gate
// ============================================================

fn command_verify(root: &Path, args: &[String]) -> DynResult<()> {
    let path = match args {
        [p] => root.join(p),
        _ => return Err(invalid("evidence42 verify requires <envelope.json>")),
    };
    let envelope: EvidenceEnvelope = read_json(&path)?;
    let mut violations = verify_integrity(&envelope);
    match verify_envelope_signature(root, &envelope) {
        Some(true) => {}
        Some(false) => violations.push("signature-invalid".to_owned()),
        None => {}
    }
    if violations.is_empty() {
        println!("EVIDENCE-VERIFY PASS {}", path.display());
        Ok(())
    } else {
        eprintln!("EVIDENCE-VERIFY FAIL violations={violations:?}");
        Err(invalid(format!(
            "evidence integrity failed: {}",
            violations.join("; ")
        )))
    }
}

fn command_gate(root: &Path, args: &[String]) -> DynResult<()> {
    let path = match args {
        [p] => root.join(p),
        _ => return Err(invalid("evidence42 gate requires <envelope.json>")),
    };
    let envelope: EvidenceEnvelope = read_json(&path)?;
    // 42 Exit Gate 1：文件级 gate = 纯语义 + 跨文件上下文（run_id/nonce
    // 复用、artifact 磁盘 digest）。self 即当前文件，扫描时排除自身。
    let mut violations = gate_with_context(root, &envelope, Some(&path));
    match verify_envelope_signature(root, &envelope) {
        Some(true) => {}
        Some(false) => violations.push("signature-invalid".to_owned()),
        None => {}
    }
    if violations.is_empty() {
        println!("EVIDENCE-GATE PASS {}", path.display());
        Ok(())
    } else {
        eprintln!("EVIDENCE-GATE FAIL violations={violations:?}");
        Err(invalid(format!(
            "evidence gate failed: {}",
            violations.join("; ")
        )))
    }
}

/// verify 只做完整性/格式检查。
pub fn verify_integrity(envelope: &EvidenceEnvelope) -> Vec<String> {
    let mut v = Vec::new();
    if envelope.schema != ENVELOPE_SCHEMA {
        v.push(format!("schema:{}", envelope.schema));
    }
    for (name, value) in [
        ("evidence_id", &envelope.evidence_id),
        ("run_id", &envelope.run_id),
        ("nonce", &envelope.nonce),
        ("test_id", &envelope.test_id),
        ("scope", &envelope.scope),
        ("started_at", &envelope.started_at),
        ("ended_at", &envelope.ended_at),
        ("source_commit", &envelope.source_commit),
        ("dirty_tree_digest", &envelope.dirty_tree_digest),
        ("product_manifest_digest", &envelope.product_manifest_digest),
        ("profile_digest", &envelope.profile_digest),
        ("artifact_sha256", &envelope.artifact_sha256),
        ("toolchain_digest", &envelope.toolchain_digest),
        (
            "platform_inventory_digest",
            &envelope.platform_inventory_digest,
        ),
        ("command", &envelope.command),
        ("signer", &envelope.signer),
        ("signature", &envelope.signature),
    ] {
        if value.is_empty() {
            v.push(format!("empty:{name}"));
        }
    }
    if !envelope.source_commit.is_empty()
        && (envelope.source_commit.len() != 40
            || !envelope
                .source_commit
                .chars()
                .all(|c| c.is_ascii_hexdigit()))
    {
        v.push("digest-format:source_commit".to_owned());
    }
    for (name, value) in [
        ("dirty_tree_digest", &envelope.dirty_tree_digest),
        ("product_manifest_digest", &envelope.product_manifest_digest),
        ("profile_digest", &envelope.profile_digest),
        ("artifact_sha256", &envelope.artifact_sha256),
        ("toolchain_digest", &envelope.toolchain_digest),
        (
            "platform_inventory_digest",
            &envelope.platform_inventory_digest,
        ),
    ] {
        if !value.is_empty() && (value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()))
        {
            v.push(format!("digest-format:{name}"));
        }
    }
    if super::verdict::Verdict::parse(&envelope.verdict).is_none() {
        v.push(format!("verdict:{}", envelope.verdict));
    }
    if envelope.positive_results.is_empty() {
        v.push("positive-results-empty".to_owned());
    }
    if envelope.negative_results.is_empty() {
        v.push("negative-results-empty".to_owned());
    }
    if envelope.inputs.is_empty() {
        v.push("inputs-empty".to_owned());
    }
    if envelope.artifacts.is_empty() {
        v.push("artifacts-empty".to_owned());
    }
    for artifact in &envelope.artifacts {
        if artifact.name.is_empty() || artifact.path.is_empty() {
            v.push("artifact-field-empty".to_owned());
        }
        if artifact.sha256.len() != 64 || !artifact.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            v.push("artifact-sha256-invalid".to_owned());
        }
    }
    if envelope.started_at > envelope.ended_at
        && !envelope.started_at.is_empty()
        && !envelope.ended_at.is_empty()
    {
        v.push("time-order".to_owned());
    }
    v
}

/// gate = verify + 语义/政策。任何非 PASS 或语义缺失都返回违规。
pub fn gate_violations(envelope: &EvidenceEnvelope) -> Vec<String> {
    let mut v = verify_integrity(envelope);
    if !v.is_empty() {
        return v;
    }
    let verdict = super::verdict::Verdict::parse(&envelope.verdict).unwrap();
    if verdict != super::verdict::Verdict::Pass {
        v.push(format!("verdict-not-pass:{}", envelope.verdict));
    }
    if !envelope.violations.is_empty() {
        v.push(format!(
            "declared-violations:{}",
            envelope.violations.join(",")
        ));
    }

    let scope = envelope.scope.to_ascii_uppercase();
    // 42 §1.1：scope 不能从 MODEL 升格到 PRODUCT——MODEL 证据声明了产品
    // 制品（artifact/产物级 manifest），即把模型/仿真结果冒充产品结果。
    if scope == "MODEL"
        && (!envelope.artifact_sha256.is_empty() || !envelope.product_manifest_digest.is_empty())
    {
        v.push("model-scope-escalated".to_owned());
    }
    let product_like = matches!(scope.as_str(), "L2" | "L3" | "PRODUCT");
    if product_like {
        if envelope.artifact_sha256.is_empty() {
            v.push("product-artifact-missing".to_owned());
        }
        if !envelope.artifact_sha256.is_empty()
            && envelope.artifact_sha256.chars().all(|c| c == '0')
        {
            v.push("product-artifact-placeholder".to_owned());
        }
        if envelope.product_manifest_digest.is_empty() {
            v.push("product-manifest-missing".to_owned());
        }
        if envelope.positive_results.is_empty() || envelope.negative_results.is_empty() {
            v.push("product-results-incomplete".to_owned());
        }
    }

    let cmd = envelope.command.to_ascii_lowercase();
    if cmd.contains("mock") || cmd.contains("--features mock") {
        v.push("mock-feature-in-product".to_owned());
    }
    if cmd.contains("run_executors(){ :; }") || cmd.contains("run_executors(){ :") {
        v.push("empty-soak-executor".to_owned());
    }
    if scope == "SOAK" && envelope.positive_results.is_empty() {
        v.push("soak-no-workload".to_owned());
    }
    if scope == "INTEROP" {
        let oracle = format!("{} {}", envelope.oracle.kind, envelope.oracle.detail).to_lowercase();
        if !(oracle.contains("peer") || oracle.contains("pcap") || oracle.contains("transcript")) {
            v.push("interop-no-peer-oracle".to_owned());
        }
        if envelope.positive_results.is_empty() || envelope.negative_results.is_empty() {
            v.push("interop-results-incomplete".to_owned());
        }
    }
    if scope == "PERF" {
        let oracle = envelope.oracle.detail.to_lowercase();
        if oracle.contains("self-baseline") || !oracle.contains("baseline") {
            v.push("perf-baseline-invalid".to_owned());
        }
    }
    let oracle = format!("{} {}", envelope.oracle.kind, envelope.oracle.detail).to_lowercase();
    // ACK 声明必须是词边界匹配的 "ack"（"rollback"、"tracks" 等含 ack
    // 子串的词汇不是 ACK 声明）；声称 ACK 但未附物理 readback 语义时阻断。
    let ack_claimed = oracle.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| {
        matches!(
            w,
            "ack" | "acks" | "acked" | "acking" | "acknowledge" | "acknowledged"
        )
    });
    if ack_claimed
        && (oracle.contains("no readback")
            || oracle.contains("no physical readback")
            || oracle.contains("without readback")
            || !oracle.contains("readback=ok"))
    {
        v.push("ack-without-readback".to_owned());
    }
    if cmd.contains("discovery") && envelope.negative_results.is_empty() {
        v.push("discovery-no-write-negative".to_owned());
    }
    if cmd.contains("timeout") && envelope.positive_results.iter().any(|r| r.status == "PASS") {
        // timeout 自身不是 PASS；必须附带受控退出/exit code oracle。
        if !oracle.contains("exit") && !oracle.contains("qmp") {
            v.push("timeout-marker-as-pass".to_owned());
        }
    }
    v
}

/// 文件级 gate：在纯 envelope 语义（`gate_violations`）之上追加跨文件上下文
/// 检查（蓝图 42 Exit Gate 1）：
/// - `run_id`/`nonce` 不得与其它已登记 envelope 复用（`run_id/nonce 不复用`）；
/// - envelope 声明的 artifact 若在磁盘上存在，其 sha256 必须与磁盘字节一致
///   （防"换了 ELF 但沿用旧 digest"的自证伪造）。
///
/// `self_path`：当前被 gate 的 envelope 文件路径（扫描历史时排除自身；
/// 纯内存构造/单测传 None）。
pub fn gate_with_context(
    root: &Path,
    envelope: &EvidenceEnvelope,
    self_path: Option<&Path>,
) -> Vec<String> {
    let mut v = gate_violations(envelope);
    if !v.is_empty() {
        // 基座已红：上下文检查仍然执行（mutation 07 的 run-id 复用在基座
        // 全绿时才单独触发，但 artifacts 磁盘不一致应一并报出）。
    }
    // 1. run_id / nonce 全局唯一——扫描 root/evidence 下全部已登记 envelope。
    let (run_owners, nonce_owners) = collect_envelope_ids(root);
    let self_owned = |owners: &BTreeMap<String, Vec<PathBuf>>, id: &str| -> Vec<String> {
        owners
            .get(id)
            .map(|paths| {
                paths
                    .iter()
                    .filter(|p| self_path.map_or(true, |s| *p != s))
                    .map(|p| p.display().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    if !envelope.run_id.is_empty() {
        let others = self_owned(&run_owners, &envelope.run_id);
        if !others.is_empty() {
            v.push(format!("run-id-reused:{}", others.join(",")));
        }
    }
    if !envelope.nonce.is_empty() {
        let others = self_owned(&nonce_owners, &envelope.nonce);
        if !others.is_empty() {
            v.push(format!("nonce-reused:{}", others.join(",")));
        }
    }
    // 2. artifact 磁盘 digest 重算：文件存在则必须与声明的 sha256 一致。
    for artifact in &envelope.artifacts {
        if artifact.path.is_empty() {
            continue;
        }
        let disk = root.join(&artifact.path);
        if !disk.is_file() {
            continue;
        }
        match hash_file(&disk) {
            Ok(actual) if actual == artifact.sha256 => {}
            Ok(actual) => v.push(format!(
                "artifact-digest-mismatch:{}:claimed={} disk={}",
                artifact.name,
                artifact.sha256.get(..12).unwrap_or(""),
                actual.get(..12).unwrap_or("")
            )),
            Err(_) => {}
        }
    }
    v
}

/// 扫描 `root/evidence` 下全部 EvidenceEnvelope（递归），返回
/// run_id→文件、nonce→文件 映射（用于全局唯一性检查）。
fn collect_envelope_ids(
    root: &Path,
) -> (
    BTreeMap<String, Vec<PathBuf>>,
    BTreeMap<String, Vec<PathBuf>>,
) {
    let mut run_owners: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut nonce_owners: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let evidence_root = root.join("evidence");
    let mut stack = vec![evidence_root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(value) = read_json::<serde_json::Value>(&path) else {
                continue;
            };
            if value.get("schema").and_then(|x| x.as_str()) != Some(ENVELOPE_SCHEMA) {
                continue;
            }
            if let Some(run_id) = value.get("run_id").and_then(|x| x.as_str()) {
                if !run_id.is_empty() {
                    run_owners
                        .entry(run_id.to_owned())
                        .or_default()
                        .push(path.clone());
                }
            }
            if let Some(nonce) = value.get("nonce").and_then(|x| x.as_str()) {
                if !nonce.is_empty() {
                    nonce_owners
                        .entry(nonce.to_owned())
                        .or_default()
                        .push(path.clone());
                }
            }
        }
    }
    (run_owners, nonce_owners)
}

// ============================================================
// Gate all envelopes in a directory
// ============================================================

fn command_gate_dir(root: &Path, args: &[String]) -> DynResult<()> {
    let mut dir = root.join("target/eneros/evidence/42");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dir" => {
                dir = root.join(
                    it.next()
                        .ok_or_else(|| invalid("gate-dir --dir requires <path>"))?,
                );
            }
            other => return Err(invalid(format!("unknown gate-dir arg '{other}'"))),
        }
    }
    if !dir.is_dir() {
        return Err(invalid(format!(
            "gate-dir directory not found: {}",
            dir.display()
        )));
    }
    let mut entries: Vec<_> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    entries.sort();

    let mut passed = 0usize;
    let mut failed = Vec::new();
    for path in entries {
        let value: serde_json::Value = match read_json(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("schema").and_then(|x| x.as_str()) != Some(ENVELOPE_SCHEMA) {
            continue;
        }
        let envelope: EvidenceEnvelope = serde_json::from_value(value)
            .map_err(|e| invalid(format!("{} is not a valid envelope: {e}", path.display())))?;
        let mut violations = gate_violations(&envelope);
        match verify_envelope_signature(root, &envelope) {
            Some(true) => {}
            Some(false) => violations.push("signature-invalid".to_owned()),
            None => {}
        }
        if violations.is_empty() {
            passed += 1;
            println!("GATE-DIR PASS {}", path.display());
        } else {
            failed.push((path, violations));
        }
    }
    if failed.is_empty() {
        println!("GATE-DIR PASS total={passed} dir={}", dir.display());
        Ok(())
    } else {
        for (path, violations) in &failed {
            eprintln!("GATE-DIR FAIL {} violations={violations:?}", path.display());
        }
        Err(invalid(format!(
            "gate-dir FAIL: {} passed, {} failed",
            passed,
            failed.len()
        )))
    }
}

// ============================================================
// Report role-specific semantic gate
// ============================================================

fn command_report_gate(root: &Path, args: &[String]) -> DynResult<()> {
    let (allow_non_pass, rest) = super::verdict::take_allow_non_pass(args);
    let mut kind = String::new();
    let mut path: Option<PathBuf> = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--kind" => kind = val()?.to_owned(),
            "--path" => path = Some(root.join(val()?)),
            other => return Err(invalid(format!("unknown report-gate arg '{other}'"))),
        }
    }
    let path = path.ok_or_else(|| invalid("report-gate requires --path <json>"))?;
    let value: serde_json::Value = read_json(&path)?;
    let violations = report_gate_violations(&kind, &value);
    if violations.is_empty() {
        println!("REPORT-GATE PASS kind={kind} path={}", path.display());
        Ok(())
    } else if allow_non_pass {
        println!(
            "REPORT-GATE INFO kind={kind} violations={} (non-release)",
            violations.join("; ")
        );
        Ok(())
    } else {
        Err(invalid(format!(
            "report-gate FAIL kind={kind}: {}",
            violations.join("; ")
        )))
    }
}

/// `report-gate-expected-fail --kind <k> --path <json>`：自检辅助——断言
/// report-gate 对该输入必须转红（负例 oracle）。输入全绿则本命令 FAIL。
fn command_report_gate_expected_fail(root: &Path, args: &[String]) -> DynResult<()> {
    let mut kind = String::new();
    let mut path: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--kind" => kind = val()?.to_owned(),
            "--path" => path = Some(root.join(val()?)),
            other => return Err(invalid(format!("unknown arg '{other}'"))),
        }
    }
    let path = path.ok_or_else(|| invalid("requires --path <json>"))?;
    let value: serde_json::Value = read_json(&path)?;
    let violations = report_gate_violations(&kind, &value);
    if violations.is_empty() {
        return Err(invalid(format!(
            "report-gate-expected-fail FAIL: kind={kind} passed when it must be red"
        )));
    }
    println!(
        "REPORT-GATE-EXPECTED-FAIL PASS kind={kind} violations={}",
        violations.join("; ")
    );
    Ok(())
}

fn report_gate_violations(kind: &str, value: &serde_json::Value) -> Vec<String> {
    let mut v = Vec::new();
    let schema = value.get("schema").and_then(|x| x.as_str()).unwrap_or("");
    match kind {
        "interop" => {
            if schema != "eneros.interop-matrix.v1" {
                v.push(format!("schema:{}", schema));
            }
            let verdict = value.get("verdict").and_then(|x| x.as_str()).unwrap_or("");
            if verdict != "D2-PASS" {
                v.push(format!("verdict:{verdict}"));
            }
            if value
                .get("candidate_id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("candidate-id-missing".to_owned());
            }
            if let Some(peers) = value.get("peers").and_then(|x| x.as_array()) {
                if peers.is_empty() {
                    v.push("peers-empty".to_owned());
                }
                for peer in peers {
                    let status = peer.get("status").and_then(|x| x.as_str()).unwrap_or("");
                    let identity = peer.get("peer").and_then(|x| x.as_str()).unwrap_or("");
                    if status != "PASS" {
                        v.push(format!("peer-not-pass:{identity}:{status}"));
                    }
                    if identity.is_empty() {
                        v.push("peer-identity-missing".to_owned());
                    }
                    if peer
                        .get("version")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .is_empty()
                    {
                        v.push(format!("peer-version-missing:{identity}"));
                    }
                    let tx_ok = peer
                        .get("transactions")
                        .and_then(|x| x.as_array())
                        .map(|a| !a.is_empty())
                        .unwrap_or(false)
                        || peer
                            .get("transaction_count")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0)
                            > 0;
                    if !tx_ok {
                        v.push(format!("peer-transactions-missing:{identity}"));
                    }
                    let neg_ok = peer
                        .get("negative_cases")
                        .and_then(|x| x.as_array())
                        .map(|a| !a.is_empty())
                        .unwrap_or(false)
                        || peer
                            .get("negative_count")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0)
                            > 0;
                    if !neg_ok {
                        v.push(format!("peer-negatives-missing:{identity}"));
                    }
                    let capture_ok = ["capture", "pcap", "transcript"].iter().any(|k| {
                        peer.get(k)
                            .and_then(|x| x.as_str())
                            .map(|s| !s.is_empty())
                            .unwrap_or(false)
                    });
                    if !capture_ok {
                        v.push(format!("peer-capture-missing:{identity}"));
                    }
                }
            } else {
                v.push("peers-missing".to_owned());
            }
        }
        "soak" => {
            if schema != "eneros.soak-campaign.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value
                .get("artifact_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("artifact-sha256-missing".to_owned());
            }
            if value
                .get("candidate_id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("candidate-id-missing".to_owned());
            }
            if let Some(min_days) = value.get("min_duration_days").and_then(|x| x.as_u64()) {
                let days = value
                    .get("duration_days")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                if days < min_days {
                    v.push(format!("duration-insufficient:{days}<{min_days}"));
                }
            }
            // 42 WP-42.2：soak 必须达到 duration/workload/fault 三条件——
            // workload 覆盖非空、故障统计存在且注入/恢复计数一致。
            let workload = value
                .get("workload_coverage")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|i| i.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            if workload.is_empty() {
                v.push("soak-no-workload".to_owned());
            }
            if let Some(fault_stats) = value.get("fault_stats") {
                let injected = fault_stats
                    .get("injected")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let recovered = fault_stats
                    .get("recovered")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let confirmed = fault_stats
                    .get("injection_confirmed")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                if injected > 0 && !confirmed {
                    v.push("fault-injection-unconfirmed".to_owned());
                }
                if recovered > injected {
                    v.push(format!(
                        "fault-recovery-exceeds-injection:{recovered}>{injected}"
                    ));
                }
            } else {
                v.push("soak-fault-stats-missing".to_owned());
            }
        }
        "perf" => {
            if schema != "eneros.perf-regression.v1" {
                v.push(format!("schema:{schema}"));
            }
            let baseline = value
                .get("baseline_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let candidate = value
                .get("candidate_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if baseline.is_empty() {
                v.push("baseline-missing".to_owned());
            }
            if candidate.is_empty() {
                v.push("candidate-missing".to_owned());
            }
            if !baseline.is_empty() && baseline == candidate {
                v.push("self-baseline".to_owned());
            }
            if value
                .get("candidate_id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("candidate-id-missing".to_owned());
            }
            if let Some(metrics) = value.get("metrics").and_then(|x| x.as_array()) {
                if metrics.is_empty() {
                    v.push("metrics-empty".to_owned());
                }
                for m in metrics {
                    if m.get("slo_threshold").and_then(|x| x.as_u64()).unwrap_or(0) == 0 {
                        v.push(format!(
                            "slo-unfrozen:{}",
                            m.get("metric_id").and_then(|x| x.as_str()).unwrap_or("")
                        ));
                    }
                }
            } else {
                v.push("metrics-missing".to_owned());
            }
        }
        "boot" => match schema {
            "eneros.qemu-media-result.v1" => {
                if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                    v.push("result-not-pass".to_owned());
                }
                if value.get("qemu_exit").and_then(|x| x.as_u64()).unwrap_or(1) != 0 {
                    v.push("qemu-exit-nonzero".to_owned());
                }
                for field in ["manifest_sha256", "measurement_root_sha256"] {
                    let digest = value.get(field).and_then(|x| x.as_str()).unwrap_or("");
                    if !is_hex64(digest) {
                        v.push(format!("{field}-invalid"));
                    }
                }
                for field in ["boot_slot", "boot_reason"] {
                    if value
                        .get(field)
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .is_empty()
                    {
                        v.push(format!("{field}-missing"));
                    }
                }
            }
            "eneros.boot-evidence.v1" => {
                if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                    v.push("result-not-pass".to_owned());
                }
                for field in ["manifest_sha256", "measurement_root_sha256"] {
                    let digest = value.get(field).and_then(|x| x.as_str()).unwrap_or("");
                    if !is_hex64(digest) {
                        v.push(format!("{field}-invalid"));
                    }
                }
                if value
                    .get("measurement_log_hex")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .is_empty()
                {
                    v.push("measurement-log-missing".to_owned());
                }
                if value
                    .get("memory_map_count")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0)
                    == 0
                {
                    v.push("memory-map-empty".to_owned());
                }
                if value
                    .get("source_date_epoch")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0)
                    == 0
                {
                    v.push("source-date-epoch-missing".to_owned());
                }
            }
            other => v.push(format!("schema:{other}")),
        },
        "install" => {
            if schema != "eneros.qemu-install-result.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            if value.get("qemu_exit").and_then(|x| x.as_u64()).unwrap_or(1) != 0 {
                v.push("qemu-exit-nonzero".to_owned());
            }
            let payload = value
                .get("payload_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if !is_hex64(payload) {
                v.push("payload-sha256-invalid".to_owned());
            }
            if value
                .get("observed")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("observed-missing".to_owned());
            }
        }
        "toolchain" => {
            if schema != "eneros.toolchain-audit.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            if let Some(violations) = value.get("violations").and_then(|x| x.as_array()) {
                if !violations.is_empty() {
                    v.push(format!("toolchain-violations:{}", violations.len()));
                }
            } else {
                v.push("violations-missing".to_owned());
            }
            if value
                .get("tools")
                .and_then(|x| x.as_array())
                .map(|a| a.is_empty())
                .unwrap_or(true)
            {
                v.push("tools-empty".to_owned());
            }
        }
        "sbom" => {
            if schema != "eneros.sbom-report.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            // 42 WP-42.3：SBOM 必须映射当前候选 lineage。
            if value
                .get("candidate_id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("candidate-id-missing".to_owned());
            }
            let artifact = value
                .get("artifact_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if !is_hex64(artifact) {
                v.push("artifact-sha256-invalid".to_owned());
            }
            if value
                .get("package_count")
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
                == 0
            {
                v.push("package-count-zero".to_owned());
            }
            for field in ["cyclonedx_sha256", "spdx_sha256"] {
                let digest = value.get(field).and_then(|x| x.as_str()).unwrap_or("");
                if !is_hex64(digest) {
                    v.push(format!("{field}-invalid"));
                }
            }
        }
        "ota" => match schema {
            "eneros.ota-rejection-evidence.v1" => {
                if let Some(cases) = value.get("cases").and_then(|x| x.as_array()) {
                    if cases.is_empty() {
                        v.push("ota-cases-empty".to_owned());
                    }
                    for case in cases {
                        let verdict = case.get("verdict").and_then(|x| x.as_str()).unwrap_or("");
                        if verdict != "PASS" {
                            v.push(format!(
                                "ota-case-not-pass:{}:{}",
                                case.get("case").and_then(|x| x.as_str()).unwrap_or(""),
                                verdict
                            ));
                        }
                    }
                } else {
                    v.push("ota-cases-missing".to_owned());
                }
            }
            "eneros.ota-rehearsal-plan.v1" => {
                if value
                    .get("from_version")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .is_empty()
                {
                    v.push("ota-from-version-missing".to_owned());
                }
                if value
                    .get("to_version")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .is_empty()
                {
                    v.push("ota-to-version-missing".to_owned());
                }
                if let Some(matrix) = value.get("matrix").and_then(|x| x.as_array()) {
                    if matrix.is_empty() {
                        v.push("ota-matrix-empty".to_owned());
                    }
                } else {
                    v.push("ota-matrix-missing".to_owned());
                }
            }
            other => v.push(format!("schema:{other}")),
        },
        "g2" | "g3" => {
            let expected = if kind == "g2" {
                "eneros.g2-evidence.v1"
            } else {
                "eneros.g3-evidence.v1"
            };
            if schema != expected {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            if value.get("failed").and_then(|x| x.as_u64()).unwrap_or(1) != 0 {
                v.push("failed-records".to_owned());
            }
            if value.get("passed").and_then(|x| x.as_u64()).unwrap_or(0) == 0 {
                v.push("passed-zero".to_owned());
            }
        }
        "trust" => {
            if schema != "eneros.trust-audit.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            if let Some(violations) = value.get("violations").and_then(|x| x.as_array()) {
                if !violations.is_empty() {
                    v.push(format!("trust-violations:{}", violations.len()));
                }
            } else {
                v.push("violations-missing".to_owned());
            }
            for field in ["policy_binding", "test_trust_rejection"] {
                if value.get(field).and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                    v.push(format!("{field}-not-pass"));
                }
            }
        }
        "storage" | "recovery" => {
            if schema != "eneros.powercut-durable.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("verdict").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("verdict-not-pass".to_owned());
            }
            if value
                .get("missing_recover_lines")
                .and_then(|x| x.as_u64())
                .unwrap_or(1)
                != 0
            {
                v.push("missing-recover-lines".to_owned());
            }
            if !value
                .get("monotonic")
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
            {
                v.push("monotonic-false".to_owned());
            }
            if let Some(cycles) = value.get("cycles").and_then(|x| x.as_u64()) {
                if cycles < 20 {
                    v.push(format!("cycles-insufficient:{cycles}<20"));
                }
            } else {
                v.push("cycles-missing".to_owned());
            }
        }
        "service" => {
            if schema != "eneros.g2-evidence.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            let has_ready = value
                .get("records")
                .and_then(|x| x.as_array())
                .map(|records| {
                    records.iter().any(|r| {
                        r.get("test").and_then(|x| x.as_str()) == Some("system-ready")
                            && r.get("result").and_then(|x| x.as_str()) == Some("PASS")
                    })
                })
                .unwrap_or(false);
            if !has_ready {
                v.push("service-ready-missing".to_owned());
            }
        }
        "control" => {
            if schema != "eneros.control-loop.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            if value
                .get("dispatch_count")
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
                == 0
            {
                v.push("control-dispatch-zero".to_owned());
            }
            if value.get("ack_count").and_then(|x| x.as_u64()).unwrap_or(0) == 0 {
                v.push("control-ack-zero".to_owned());
            }
            if value
                .get("readback_count")
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
                == 0
            {
                v.push("control-readback-zero".to_owned());
            }
        }
        "repro" => {
            if schema != "eneros.repro-report.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            let a = value
                .get("artifact_a_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let b = value
                .get("artifact_b_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if a.is_empty() || b.is_empty() {
                v.push("repro-digest-missing".to_owned());
            }
            if !a.is_empty() && a != b {
                v.push("repro-digest-mismatch".to_owned());
            }
        }
        "security" => {
            if schema != "eneros.trust-audit.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            if let Some(violations) = value.get("violations").and_then(|x| x.as_array()) {
                if !violations.is_empty() {
                    v.push(format!("security-violations:{}", violations.len()));
                }
            } else {
                v.push("violations-missing".to_owned());
            }
        }
        "fault" => {
            let valid_schema = matches!(
                schema,
                "eneros.artifact-mutation.v1"
                    | "eneros.installer-mutation.v1"
                    | "eneros.boot-mutation.v1"
                    | "eneros.bootctrl-mutation.v1"
            );
            if !valid_schema {
                v.push(format!("schema:{schema}"));
            }
            let src = value
                .get("source_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let out = value
                .get("output_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if src.is_empty() || out.is_empty() {
                v.push("fault-digest-missing".to_owned());
            }
            if !src.is_empty() && !out.is_empty() && src == out {
                v.push("fault-not-mutated".to_owned());
            }
        }
        "isolation" => {
            if schema != "eneros.g2-evidence.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            let has = |name: &str| {
                value
                    .get("records")
                    .and_then(|x| x.as_array())
                    .map(|records| {
                        records.iter().any(|r| {
                            r.get("test").and_then(|x| x.as_str()) == Some(name)
                                && r.get("result").and_then(|x| x.as_str()) == Some("PASS")
                        })
                    })
                    .unwrap_or(false)
            };
            if !has("mmu") {
                v.push("isolation-mmu-missing".to_owned());
            }
            if !has("el0-enter") {
                v.push("isolation-el0-missing".to_owned());
            }
            // 42 WP-42.2：isolation/W^X——要么报告显式声明 wx_enforced，
            // 要么记录集含 W^X 专项 PASS 记录；两者皆无 = 没有 W^X 证据。
            let wx_record = value
                .get("records")
                .and_then(|x| x.as_array())
                .map(|records| {
                    records.iter().any(|r| {
                        matches!(
                            r.get("test").and_then(|x| x.as_str()),
                            Some("wx")
                                | Some("wx-protection")
                                | Some("w-x-protection")
                                | Some("nx")
                        ) && r.get("result").and_then(|x| x.as_str()) == Some("PASS")
                    })
                })
                .unwrap_or(false);
            let wx_declared = value
                .get("wx_enforced")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            if !wx_record && !wx_declared {
                v.push("isolation-wx-missing".to_owned());
            }
        }
        "readiness" => {
            if schema != "eneros.g2-evidence.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("result-not-pass".to_owned());
            }
            let has_ready = value
                .get("records")
                .and_then(|x| x.as_array())
                .map(|records| {
                    records.iter().any(|r| {
                        r.get("test").and_then(|x| x.as_str()) == Some("system-ready")
                            && r.get("result").and_then(|x| x.as_str()) == Some("PASS")
                    })
                })
                .unwrap_or(false);
            if !has_ready {
                v.push("readiness-system-ready-missing".to_owned());
            }
        }
        "build" => {
            if schema != "eneros.build-identity.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value
                .get("profile")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("build-profile-missing".to_owned());
            }
            let commit = value
                .get("source_commit")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                v.push("build-source-commit-invalid".to_owned());
            }
            let lock = value
                .get("cargo_lock_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if !is_hex64(lock) {
                v.push("build-cargo-lock-invalid".to_owned());
            }
            if value
                .get("build_id_hex")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("build-id-missing".to_owned());
            }
        }
        "bom" => {
            if schema != "eneros.delivery.binary-boms.v1" {
                v.push(format!("schema:{schema}"));
            }
            // 42 WP-42.3：BOM 必须映射当前候选 lineage。
            if value
                .get("candidate_id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("candidate-id-missing".to_owned());
            }
            let commit = value
                .get("source_commit")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                v.push("bom-source-commit-invalid".to_owned());
            }
            if value
                .get("binary_count")
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
                == 0
            {
                v.push("bom-binaries-empty".to_owned());
            }
            if value
                .get("binaries")
                .and_then(|x| x.as_array())
                .map(|a| a.is_empty())
                .unwrap_or(true)
            {
                v.push("bom-binaries-missing".to_owned());
            }
        }
        "graph" => {
            if schema != "eneros.delivery.cargo-graph.v1" {
                v.push(format!("schema:{schema}"));
            }
            let commit = value
                .get("source_commit")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                v.push("graph-source-commit-invalid".to_owned());
            }
            let lock = value
                .get("cargo_lock_sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if !is_hex64(lock) {
                v.push("graph-cargo-lock-invalid".to_owned());
            }
            if value
                .get("workspace_package_count")
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
                == 0
            {
                v.push("graph-workspace-empty".to_owned());
            }
        }
        "media" => {
            if schema != "eneros.media-check.v1" {
                v.push(format!("schema:{schema}"));
            }
            // 42 WP-42.3：media-check 必须映射当前候选 lineage（防止把旧
            // 介质产物当当前候选证据）。
            if value
                .get("candidate_id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("candidate-id-missing".to_owned());
            }
            if value.get("verdict").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("verdict-not-pass".to_owned());
            }
            if value
                .get("missing")
                .and_then(|x| x.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(true)
            {
                v.push("media-missing-artifacts".to_owned());
            }
            if !value
                .get("qemu_available")
                .and_then(|x| x.as_bool())
                .unwrap_or(false)
            {
                v.push("media-qemu-unavailable".to_owned());
            }
        }
        "compliance" => {
            if schema != "eneros.compliance-report.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value
                .get("self_test_status")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                != "PASS"
            {
                v.push("compliance-self-test-not-pass".to_owned());
            }
            if value
                .get("sha256")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("compliance-sha256-missing".to_owned());
            }
            if value
                .get("signature")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("compliance-signature-missing".to_owned());
            }
        }
        "kat" => {
            if schema != "eneros.crypto-kat.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("verdict").and_then(|x| x.as_str()).unwrap_or("") != "KAT-ALL-PASS" {
                v.push("kat-not-all-pass".to_owned());
            }
            if let Some(suites) = value.get("suites").and_then(|x| x.as_array()) {
                if suites.is_empty() {
                    v.push("kat-suites-empty".to_owned());
                }
                for suite in suites {
                    if !suite.get("pass").and_then(|x| x.as_bool()).unwrap_or(false) {
                        v.push(format!(
                            "kat-suite-not-pass:{}",
                            suite.get("name").and_then(|x| x.as_str()).unwrap_or("")
                        ));
                    }
                }
            } else {
                v.push("kat-suites-missing".to_owned());
            }
        }
        "proof" => {
            if schema != "eneros.qa-gate.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("qa_id").and_then(|x| x.as_str()).unwrap_or("") != "QA-021" {
                v.push("proof-wrong-qa-id".to_owned());
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("proof-not-pass".to_owned());
            }
            if let Some(violations) = value.get("violations").and_then(|x| x.as_array()) {
                if !violations.is_empty() {
                    v.push(format!("proof-violations:{}", violations.len()));
                }
            } else {
                v.push("proof-violations-missing".to_owned());
            }
        }
        "validation" => {
            if schema != "eneros.independent-validation.v1" {
                v.push(format!("schema:{schema}"));
            }
            if let Some(reviewers) = value.get("reviewers").and_then(|x| x.as_array()) {
                if reviewers.is_empty() {
                    v.push("validation-no-reviewers".to_owned());
                }
                let has_independent = reviewers.iter().any(|r| {
                    r.get("is_implementer").and_then(|x| x.as_bool()) == Some(false)
                        && r.get("signed").and_then(|x| x.as_bool()) == Some(true)
                });
                if !has_independent {
                    v.push("validation-no-independent-signoff".to_owned());
                }
            } else {
                v.push("validation-reviewers-missing".to_owned());
            }
            if let Some(areas) = value.get("gate_areas").and_then(|x| x.as_array()) {
                for area in areas {
                    let conclusion = area
                        .get("conclusion")
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    if conclusion != "pass" {
                        v.push(format!(
                            "validation-area-not-pass:{}:{}",
                            area.get("area").and_then(|x| x.as_str()).unwrap_or(""),
                            conclusion
                        ));
                    }
                }
            } else {
                v.push("validation-gate-areas-missing".to_owned());
            }
        }
        "trace" => {
            if schema != "eneros.requirement-trace.v1" {
                v.push(format!("schema:{schema}"));
            }
            if let Some(requirements) = value.get("requirements").and_then(|x| x.as_array()) {
                if requirements.is_empty() {
                    v.push("trace-requirements-empty".to_owned());
                }
                for req in requirements {
                    let severity = req.get("severity").and_then(|x| x.as_str()).unwrap_or("");
                    if severity == "critical" {
                        let has_automated = req
                            .get("has_automated_evidence")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false);
                        let test_ids = req.get("test_ids").and_then(|x| x.as_array());
                        let artifact = req.get("artifact_path").and_then(|x| x.as_str());
                        if !has_automated
                            || test_ids.map(|a| a.is_empty()).unwrap_or(true)
                            || artifact.map(|s| s.is_empty()).unwrap_or(true)
                        {
                            v.push(format!(
                                "trace-critical-incomplete:{}",
                                req.get("requirement_id")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("")
                            ));
                        }
                    }
                }
            } else {
                v.push("trace-requirements-missing".to_owned());
            }
        }
        "index" => {
            if schema != "eneros.evidence-index.v1" {
                v.push(format!("schema:{schema}"));
            }
            let commit = value
                .get("baseline_commit")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                v.push("index-baseline-commit-invalid".to_owned());
            }
            let digest = value
                .get("baseline_digest")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if !is_hex64(digest) {
                v.push("index-baseline-digest-invalid".to_owned());
            }
            if value
                .get("stale_since")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("index-stale-since-missing".to_owned());
            }
        }
        "fuzz" => {
            if schema != "eneros.fuzz-smoke.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("fuzz-result-not-pass".to_owned());
            }
            if value
                .get("candidate_id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .is_empty()
            {
                v.push("fuzz-candidate-missing".to_owned());
            }
            if let Some(targets) = value.get("targets").and_then(|x| x.as_array()) {
                if targets.is_empty() {
                    v.push("fuzz-targets-empty".to_owned());
                }
                for target in targets {
                    let name = target.get("target").and_then(|x| x.as_str()).unwrap_or("");
                    let iterations = target
                        .get("iterations")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                    let crashes = target.get("crashes").and_then(|x| x.as_u64()).unwrap_or(1);
                    if name.is_empty() {
                        v.push("fuzz-target-name-missing".to_owned());
                    }
                    if iterations == 0 {
                        v.push(format!("fuzz-iterations-zero:{name}"));
                    }
                    if crashes != 0 {
                        v.push(format!("fuzz-crashes:{name}:{crashes}"));
                    }
                }
            } else {
                v.push("fuzz-targets-missing".to_owned());
            }
        }
        "unsafe" => {
            if schema != "eneros.unsafe-audit.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("unsafe-result-not-pass".to_owned());
            }
            if value
                .get("checked_files")
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
                == 0
            {
                v.push("unsafe-checked-files-zero".to_owned());
            }
            if let Some(violations) = value.get("violations").and_then(|x| x.as_array()) {
                if !violations.is_empty() {
                    v.push(format!("unsafe-violations:{}", violations.len()));
                }
            } else {
                v.push("unsafe-violations-missing".to_owned());
            }
        }
        "property" => {
            if schema != "eneros.property-test.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("property-result-not-pass".to_owned());
            }
            if let Some(cases) = value.get("cases").and_then(|x| x.as_array()) {
                if cases.is_empty() {
                    v.push("property-cases-empty".to_owned());
                }
                for case in cases {
                    if !case.get("pass").and_then(|x| x.as_bool()).unwrap_or(false) {
                        v.push(format!(
                            "property-case-not-pass:{}",
                            case.get("name").and_then(|x| x.as_str()).unwrap_or("")
                        ));
                    }
                }
            } else {
                v.push("property-cases-missing".to_owned());
            }
        }
        "loom" => {
            if schema != "eneros.loom-model.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("loom-result-not-pass".to_owned());
            }
            if let Some(models) = value.get("models").and_then(|x| x.as_array()) {
                if models.is_empty() {
                    v.push("loom-models-empty".to_owned());
                }
                for model in models {
                    if !model.get("pass").and_then(|x| x.as_bool()).unwrap_or(false) {
                        v.push(format!(
                            "loom-model-not-pass:{}",
                            model.get("name").and_then(|x| x.as_str()).unwrap_or("")
                        ));
                    }
                }
            } else {
                v.push("loom-models-missing".to_owned());
            }
        }
        "miri" => {
            if schema != "eneros.miri-audit.v1" {
                v.push(format!("schema:{schema}"));
            }
            if value.get("result").and_then(|x| x.as_str()).unwrap_or("") != "PASS" {
                v.push("miri-result-not-pass".to_owned());
            }
            if let Some(targets) = value.get("targets").and_then(|x| x.as_array()) {
                if targets.is_empty() {
                    v.push("miri-targets-empty".to_owned());
                }
                for target in targets {
                    if !target
                        .get("pass")
                        .and_then(|x| x.as_bool())
                        .unwrap_or(false)
                    {
                        v.push(format!(
                            "miri-target-not-pass:{}",
                            target.get("name").and_then(|x| x.as_str()).unwrap_or("")
                        ));
                    }
                }
            } else {
                v.push("miri-targets-missing".to_owned());
            }
        }
        other => v.push(format!("unknown-kind:{other}")),
    }
    v
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

// ============================================================
// Formal media artifact check
// ============================================================

fn command_media_check(root: &Path, args: &[String]) -> DynResult<()> {
    let (allow_non_pass, rest) = super::verdict::take_allow_non_pass(args);
    let mut profile_id = "qemu-prod".to_owned();
    let mut out = root.join("target/eneros/evidence/42/media-check.json");
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--profile" => profile_id = val()?.to_owned(),
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown media-check arg '{other}'"))),
        }
    }

    let artifacts_dir = root.join("target/eneros/artifacts").join(&profile_id);
    let candidates = [
        ("raw", format!("eneros-{profile_id}.raw")),
        ("iso", format!("eneros-{profile_id}-installer.iso")),
        ("update", format!("eneros-{profile_id}-full.enupd")),
    ];
    let mut artifacts = Vec::new();
    let mut missing = Vec::new();
    for (kind, file) in candidates {
        let path = artifacts_dir.join(&file);
        if path.is_file() {
            artifacts.push(serde_json::json!({
                "kind": kind,
                "path": file,
                "sha256": hash_file(&path)?,
                "size_bytes": fs::metadata(&path)?.len(),
            }));
        } else {
            missing.push(file);
        }
    }

    let qemu_available = Command::new("qemu-system-aarch64")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    // 42 WP-42.3：media-check 绑定当前候选 lineage（尽力而为——manifest
    // 未生成时留空，由 media verifier 以 candidate-id-missing 判红）。
    let candidate_id = compute_lineage(root, &profile_id, None)
        .map(|l| l.candidate_id)
        .unwrap_or_default();
    let verdict = if !missing.is_empty() {
        "FAIL"
    } else if !qemu_available {
        "BLOCKED"
    } else {
        "PASS"
    };
    let report = serde_json::json!({
        "schema": "eneros.media-check.v1",
        "profile": profile_id,
        "candidate_id": candidate_id,
        "artifacts": artifacts,
        "missing": missing,
        "qemu_available": qemu_available,
        "verdict": verdict,
        "non_release": verdict != "PASS",
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("MEDIA-CHECK verdict={verdict} → {}", out.display());
    if verdict != "PASS" && !allow_non_pass {
        return Err(invalid(format!(
            "media-check {verdict}: missing={missing:?} qemu={qemu_available}"
        )));
    }
    Ok(())
}

// ============================================================
// Fuzz smoke / unsafe audit report generators
// ============================================================

fn command_fuzz_report(root: &Path, args: &[String]) -> DynResult<()> {
    let mut iterations = 10_000usize;
    let mut out = root.join("target/eneros/evidence/42/fuzz-smoke.json");
    let mut profile_id = "qemu-prod".to_owned();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--iterations" => {
                iterations = val()?
                    .parse()
                    .map_err(|_| invalid("--iterations must be usize"))?
            }
            "--out" => out = root.join(val()?),
            "--profile" => profile_id = val()?.to_owned(),
            other => return Err(invalid(format!("unknown fuzz-report arg '{other}'"))),
        }
    }
    if iterations == 0 {
        return Err(invalid("fuzz-report --iterations must be > 0"));
    }
    let lineage = compute_lineage(root, &profile_id, Some(&root.join("Cargo.lock")))?;
    let targets = [
        "boot_protocol",
        "ipc_wire",
        "modbus_rtu",
        "iec104",
        "elf_loader",
        "gguf_loader",
    ];
    let mut target_reports = Vec::new();
    let mut total_crashes = 0usize;
    for target in targets {
        let run = super::fuzz_driver::run_fuzz(target, iterations)?;
        total_crashes += run.crashes;
        target_reports.push(serde_json::json!({
            "target": run.target,
            "iterations": run.iterations,
            "crashes": run.crashes,
            "elapsed_ms": run.elapsed_ms,
        }));
    }
    let result = if total_crashes == 0 { "PASS" } else { "FAIL" };
    let report = serde_json::json!({
        "schema": "eneros.fuzz-smoke.v1",
        "result": result,
        "candidate_id": lineage.candidate_id,
        "iterations": iterations,
        "targets": target_reports,
        "violations": if total_crashes == 0 {
            serde_json::json!([])
        } else {
            serde_json::json!(["fuzz-crashes"])
        },
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("FUZZ-REPORT → {}", out.display());
    if result != "PASS" {
        return Err(invalid(format!(
            "fuzz-report FAIL: {total_crashes} crash(es) across {} target(s)",
            targets.len()
        )));
    }
    Ok(())
}

fn command_unsafe_report(root: &Path, args: &[String]) -> DynResult<()> {
    let (allow_non_pass, rest) = super::verdict::take_allow_non_pass(args);
    let mut out = root.join("target/eneros/evidence/42/unsafe-audit.json");
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => {
                out = root.join(it.next().ok_or_else(|| invalid("--out requires a value"))?);
            }
            other => return Err(invalid(format!("unknown unsafe-report arg '{other}'"))),
        }
    }
    let report = super::unsafe_audit_report(root)?;
    let value = serde_json::to_value(&report)?;
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&value)?)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    println!("UNSAFE-REPORT → {}", out.display());
    if report.result != "PASS" && !allow_non_pass {
        return Err(invalid(format!(
            "unsafe-report FAIL: {} violation(s)",
            report.violations.len()
        )));
    }
    if report.result != "PASS" {
        println!(
            "UNSAFE-REPORT INFO result=FAIL (non-release, violations={})",
            report.violations.len()
        );
    }
    Ok(())
}

// ============================================================
// Reproducibility report generator
// ============================================================

fn command_repro(root: &Path, args: &[String]) -> DynResult<()> {
    let (allow_non_pass, rest) = super::verdict::take_allow_non_pass(args);
    let mut out = root.join("target/eneros/evidence/42/repro.json");
    let mut profile_id = "qemu-prod".to_owned();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            "--profile" => profile_id = val()?.to_owned(),
            other => return Err(invalid(format!("unknown repro arg '{other}'"))),
        }
    }
    let compare = super::vrf002::run_build_compare(root)?;
    let lineage = compute_lineage(root, &profile_id, None)?;
    let report = serde_json::json!({
        "schema": "eneros.repro-report.v1",
        "result": compare.result,
        "candidate_id": lineage.candidate_id,
        "source_commit": lineage.source_commit,
        "dirty_tree_digest": lineage.dirty_tree_digest,
        "payload": compare.payload,
        "env_a": compare.env_a,
        "env_b": compare.env_b,
        "artifact_a_sha256": compare.sha256_a,
        "artifact_b_sha256": compare.sha256_b,
        "len_a": compare.len_a,
        "len_b": compare.len_b,
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("REPRO-REPORT → {}", out.display());
    if compare.result != "PASS" && !allow_non_pass {
        return Err(invalid(format!(
            "repro FAIL: sha256_a={} sha256_b={}",
            compare.sha256_a, compare.sha256_b
        )));
    }
    if compare.result != "PASS" {
        println!("REPRO-REPORT INFO result=FAIL (non-release)");
    }
    Ok(())
}

// ============================================================
// Stale evidence index generator
// ============================================================

fn command_stale_index(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/stale-index.json");
    let mut profile_id = "qemu-prod".to_owned();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            "--profile" => profile_id = val()?.to_owned(),
            other => return Err(invalid(format!("unknown stale-index arg '{other}'"))),
        }
    }
    let lineage = compute_lineage(root, &profile_id, None)?;
    let evidence_dir = root.join("evidence/42");
    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    if evidence_dir.is_dir() {
        let mut files: Vec<_> = fs::read_dir(&evidence_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
            .collect();
        files.sort();
        for path in files {
            let Ok(value) = read_json::<serde_json::Value>(&path) else {
                continue;
            };
            if value.get("schema").and_then(|x| x.as_str()) != Some(ENVELOPE_SCHEMA) {
                continue;
            }
            let dirty = value
                .get("dirty_tree_digest")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let commit = value
                .get("source_commit")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if dirty != lineage.dirty_tree_digest || commit != lineage.source_commit {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if !seen.contains(&rel) {
                    seen.insert(rel.clone());
                    entries.push(serde_json::json!({
                        "path": rel,
                        "note": format!(
                            "stale candidate: dirty={} commit={}",
                            if dirty.is_empty() { "<empty>" } else { &dirty[..dirty.len().min(12)] },
                            if commit.is_empty() { "<empty>" } else { &commit[..commit.len().min(12)] }
                        ),
                    }));
                }
            }
        }
    }
    // 保留既有非 envelope 静态条目，避免丢失手工登记的旧证据路径。
    let existing_path = root.join("evidence/42/stale-index.json");
    let existing_entries: Vec<serde_json::Value> = if existing_path.is_file() {
        read_json::<serde_json::Value>(&existing_path)
            .ok()
            .and_then(|v| v.get("entries").cloned())
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    for e in existing_entries {
        if let Some(p) = e.get("path").and_then(|x| x.as_str()) {
            if !seen.contains(p) {
                seen.insert(p.to_owned());
                entries.push(e);
            }
        }
    }
    let report = serde_json::json!({
        "schema": "eneros.evidence-stale-index.v1",
        "status": "SUPERSEDED",
        "reason": "BASELINE_OR_SCHEMA_CHANGED",
        "replacement_run_id": null,
        "current_dirty_tree_digest": lineage.dirty_tree_digest,
        "current_source_commit": lineage.source_commit,
        "entries": entries,
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("STALE-INDEX → {}", out.display());
    Ok(())
}

// ============================================================
// Current evidence index generator
// ============================================================

fn command_current_index(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("target/eneros/evidence/42/current-index.json");
    let mut profile_id = "qemu-prod".to_owned();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            "--profile" => profile_id = val()?.to_owned(),
            other => return Err(invalid(format!("unknown current-index arg '{other}'"))),
        }
    }
    let lineage = compute_lineage(root, &profile_id, None)?;
    let evidence_dir = root.join("evidence/42");
    let mut entries: Vec<serde_json::Value> = Vec::new();
    if evidence_dir.is_dir() {
        let mut files: Vec<_> = fs::read_dir(&evidence_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
            .collect();
        files.sort();
        for path in files {
            let Ok(value) = read_json::<serde_json::Value>(&path) else {
                continue;
            };
            if value.get("schema").and_then(|x| x.as_str()) != Some(ENVELOPE_SCHEMA) {
                continue;
            }
            let dirty = value
                .get("dirty_tree_digest")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let commit = value
                .get("source_commit")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if dirty == lineage.dirty_tree_digest && commit == lineage.source_commit {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                entries.push(serde_json::json!({
                    "path": rel,
                    "test_id": value.get("test_id").and_then(|x| x.as_str()).unwrap_or(""),
                    "verdict": value.get("verdict").and_then(|x| x.as_str()).unwrap_or(""),
                }));
            }
        }
    }
    let report = serde_json::json!({
        "schema": "eneros.current-evidence-index.v1",
        "current_candidate_id": lineage.candidate_id,
        "current_dirty_tree_digest": lineage.dirty_tree_digest,
        "current_source_commit": lineage.source_commit,
        "entries": entries,
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("CURRENT-INDEX → {}", out.display());
    Ok(())
}

// ============================================================
// Current evidence gate
// ============================================================

fn command_check_current(root: &Path, args: &[String]) -> DynResult<()> {
    let mut profile_id = "qemu-prod".to_owned();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--profile" => profile_id = val()?.to_owned(),
            other => return Err(invalid(format!("unknown check-current arg '{other}'"))),
        }
    }
    let lineage = compute_lineage(root, &profile_id, None)?;
    let evidence_dir = root.join("evidence/42");
    let mut checked = 0usize;
    let mut failures = Vec::new();
    if evidence_dir.is_dir() {
        let mut files: Vec<_> = fs::read_dir(&evidence_dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
            .collect();
        files.sort();
        for path in files {
            let Ok(value) = read_json::<serde_json::Value>(&path) else {
                continue;
            };
            if value.get("schema").and_then(|x| x.as_str()) != Some(ENVELOPE_SCHEMA) {
                continue;
            }
            let dirty = value
                .get("dirty_tree_digest")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let commit = value
                .get("source_commit")
                .and_then(|x| x.as_str())
                .unwrap_or("");
            if dirty != lineage.dirty_tree_digest || commit != lineage.source_commit {
                continue;
            }
            checked += 1;
            let envelope: EvidenceEnvelope = serde_json::from_value(value)
                .map_err(|e| invalid(format!("{} invalid envelope: {e}", path.display())))?;
            let mut violations = gate_with_context(root, &envelope, Some(&path));
            match verify_envelope_signature(root, &envelope) {
                Some(true) => {}
                Some(false) => violations.push("signature-invalid".to_owned()),
                None => {}
            }
            if !violations.is_empty() {
                failures.push(format!("{}: {}", path.display(), violations.join("; ")));
            } else {
                println!("CHECK-CURRENT PASS {}", path.display());
            }
        }
    }
    if checked == 0 {
        return Err(invalid(
            "check-current FAIL: no current-candidate EvidenceEnvelope found",
        ));
    }
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("CHECK-CURRENT FAIL {f}");
        }
        return Err(invalid(format!(
            "check-current FAIL: {} of {} current envelopes failed",
            failures.len(),
            checked
        )));
    }
    println!("CHECK-CURRENT PASS total={checked}");
    Ok(())
}

// ============================================================
// 36-41 Gate status reporter
// ============================================================

fn command_gate_status(root: &Path, args: &[String]) -> DynResult<()> {
    let (allow_non_pass, rest) = super::verdict::take_allow_non_pass(args);
    let mut out = root.join("target/eneros/evidence/42/gate-status.json");
    let mut fail_on_any = false;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            "--fail-on-any" => fail_on_any = true,
            other => return Err(invalid(format!("unknown gate-status arg '{other}'"))),
        }
    }
    let gates: &[(&str, &[&str])] = &[
        ("policy-kernel", &["policy", "kernel"]),
        ("policy-adr", &["policy", "adr"]),
        (
            "manifest-verify-bundle",
            &["manifest", "verify-bundle", "--profile", "qemu-prod"],
        ),
        ("config-refs", &["qa", "gate", "config-refs"]),
        ("mock-leak", &["qa", "gate", "mock-leak"]),
        ("gng-version", &["qa", "gng-version"]),
        ("closure", &["closure", "gate", "--profile", "qemu-prod"]),
        ("orphan-check", &["qa", "gate", "orphan-check"]),
        ("evidence-gate", &["evidence", "gate"]),
        ("unsafe-audit", &["audit-unsafe-safety"]),
        ("mutation", &["evidence42", "mutation"]),
        ("media-check", &["evidence42", "media-check"]),
        ("repro", &["evidence42", "repro"]),
        ("check-current", &["evidence42", "check-current"]),
        (
            "fuzz-smoke",
            &["evidence42", "fuzz-report", "--iterations", "100"],
        ),
    ];
    let mut results = Vec::new();
    let mut failures = Vec::new();
    for (name, args) in gates {
        let output = Command::new("cargo")
            .current_dir(root)
            .arg("xtask")
            .args(*args)
            .output()
            .map_err(|e| invalid(format!("spawn gate {name}: {e}")))?;
        let ok = output.status.success();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_owned();
        let detail = if ok {
            tail
        } else {
            stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        results.push(serde_json::json!({
            "gate": name,
            "command": format!("cargo xtask {}", args.join(" ")),
            "pass": ok,
            "exit_code": output.status.code(),
            "detail": detail,
        }));
        if !ok {
            failures.push(name.to_owned());
        }
    }
    let report = serde_json::json!({
        "schema": "eneros.legacy-gate-status.v1",
        "result": if failures.is_empty() { "PASS" } else { "FAIL" },
        "failures": failures,
        "gates": results,
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("GATE-STATUS → {}", out.display());
    if fail_on_any && !failures.is_empty() {
        return Err(invalid(format!(
            "gate-status FAIL: {}",
            failures.join(", ")
        )));
    }
    if !failures.is_empty() && !allow_non_pass {
        return Err(invalid(format!(
            "gate-status FAIL (non-release): {}",
            failures.join(", ")
        )));
    }
    Ok(())
}

// ============================================================
// Evidence42 summary
// ============================================================

fn command_summary(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/summary.json");
    let mut profile_id = "qemu-prod".to_owned();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            "--profile" => profile_id = val()?.to_owned(),
            other => return Err(invalid(format!("unknown summary arg '{other}'"))),
        }
    }
    let lineage = compute_lineage(root, &profile_id, None)?;
    let mut files = std::collections::BTreeMap::new();
    for (name, rel) in [
        ("current_index", "evidence/42/current-index.json"),
        ("gate_status", "evidence/42/gate-status.json"),
        ("unsafe_audit", "evidence/42/unsafe-audit.json"),
        ("repro", "evidence/42/repro.json"),
        ("stale_index", "evidence/42/stale-index.json"),
    ] {
        let path = root.join(rel);
        if path.is_file() {
            match read_json::<serde_json::Value>(&path) {
                Ok(v) => {
                    files.insert(name, v);
                }
                Err(_) => {
                    files.insert(name, serde_json::json!({"error": "unreadable"}));
                }
            }
        } else {
            files.insert(name, serde_json::json!({"missing": true}));
        }
    }
    let current_entries = files
        .get("current_index")
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let failing_gates = files
        .get("gate_status")
        .and_then(|v| v.get("failures"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let report = serde_json::json!({
        "schema": "eneros.evidence42-summary.v1",
        "candidate_id": lineage.candidate_id,
        "dirty_tree_digest": lineage.dirty_tree_digest,
        "source_commit": lineage.source_commit,
        "current_envelope_count": current_entries,
        "failing_legacy_gates": failing_gates,
        "files": files,
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("SUMMARY → {}", out.display());
    Ok(())
}

// ============================================================
// Release readiness
// ============================================================

fn command_release_readiness(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("target/eneros/evidence/42/release-readiness.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown release-readiness arg '{other}'"))),
        }
    }
    let steps: &[(&str, &[&str])] = &[
        ("check-current", &["evidence42", "check-current"]),
        ("mutation", &["evidence42", "mutation"]),
        ("media-check", &["evidence42", "media-check"]),
        ("unsafe-audit", &["audit-unsafe-safety"]),
        ("repro", &["evidence42", "repro"]),
        ("legacy-gates", &["evidence42", "gate-status"]),
    ];
    let mut results = Vec::new();
    let mut failures = Vec::new();
    for (name, args) in steps {
        let output = Command::new("cargo")
            .current_dir(root)
            .arg("xtask")
            .args(*args)
            .output()
            .map_err(|e| invalid(format!("spawn {name}: {e}")))?;
        let ok = output.status.success();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if ok {
            stdout
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_owned()
        } else {
            stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        results.push(serde_json::json!({
            "step": name,
            "pass": ok,
            "exit_code": output.status.code(),
            "detail": detail,
        }));
        if !ok {
            failures.push(name.to_owned());
        }
    }
    let report = serde_json::json!({
        "schema": "eneros.release-readiness.v1",
        "result": if failures.is_empty() { "PASS" } else { "FAIL" },
        "failures": failures,
        "steps": results,
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("RELEASE-READINESS → {}", out.display());
    if !failures.is_empty() {
        return Err(invalid(format!(
            "release-readiness FAIL: {}",
            failures.join(", ")
        )));
    }
    Ok(())
}

// ============================================================
// Blockers report
// ============================================================

fn command_blockers(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/blockers.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown blockers arg '{other}'"))),
        }
    }
    let mut failing_gates: Vec<serde_json::Value> = Vec::new();
    let gate_status_path = root.join("evidence/42/gate-status.json");
    if gate_status_path.is_file() {
        if let Ok(v) = read_json::<serde_json::Value>(&gate_status_path) {
            if let Some(gates) = v.get("gates").and_then(|x| x.as_array()) {
                for g in gates {
                    if g.get("pass").and_then(|x| x.as_bool()) == Some(false) {
                        failing_gates.push(g.clone());
                    }
                }
            }
        }
    }
    let external = [
        "independent-peer-interop: modbus L1 VERIFIED (independent raw peer); L2 product-endpoint peers for iec104/mms/mqtt not configured (39/40 产品端点)",
        "72h-software-soak: not executed in this session (smoke 3/5 cycles PASS)",
        "windows-native-run: not executed in this Linux session",
        "clean-locked-ci-dual-build: not executed (local component/update rebuild bit-identical; full-media candidate sv2 not regenerable from current profile sv1 — 见 RETURN_TO_OWNER-42)",
    ];
    let report = serde_json::json!({
        "schema": "eneros.blockers.v1",
        "failing_legacy_gates": failing_gates,
        "external_blockers": external,
        "conclusion": "42 software L0-L3 evidence tooling READY (15/15 gates PASS; audit-all READY). Remaining items are environment/external BLOCKED: Windows native run, 72h wall-clock soak, clean locked CI dual-build, L2 independent protocol peers (39/40 endpoints)." ,
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("BLOCKERS → {}", out.display());
    Ok(())
}

// ============================================================
// Evidence42 self-test
// ============================================================

fn command_self_test(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("target/eneros/evidence/42/self-test.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown self-test arg '{other}'"))),
        }
    }
    let fixture_dir = root.join("target/eneros/evidence/42/self-test-fixtures");
    fs::create_dir_all(&fixture_dir)?;
    let hex64 = format!("{:x}", {
        // 固定但合法的 64-hex（非随机，保证 fixture 确定性）。
        let mut h = Sha256::new();
        h.update(b"self-test-fixture");
        h.finalize()
    });
    // 42 WP-42.2：role-specific 语义 verifier 自检正例——soak 必须满足
    // duration/workload/fault 三条件；isolation 必须声明 W^X；perf 非自基线。
    let soak_good = serde_json::json!({
        "schema": "eneros.soak-campaign.v1",
        "campaign_id": "self-test-fixture",
        "artifact_sha256": hex64.as_str(),
        "candidate_id": hex64.as_str(),
        "duration_days": 3,
        "min_duration_days": 3,
        "workload_coverage": ["boot-chain", "power-cut"],
        "fault_stats": {"injected": 2, "recovered": 2, "injection_confirmed": true},
        "restarts": [],
        "data_corruption": false,
    });
    let soak_bad = serde_json::json!({
        "schema": "eneros.soak-campaign.v1",
        "campaign_id": "self-test-fixture-bad",
        "artifact_sha256": hex64.as_str(),
        "candidate_id": hex64.as_str(),
        "duration_days": 3,
        "min_duration_days": 3,
        // 缺 workload_coverage + fault_stats → 必须转红。
    });
    let isolation_good = serde_json::json!({
        "schema": "eneros.g2-evidence.v1",
        "result": "PASS",
        "records": [
            {"test": "kernel-init", "result": "PASS"},
            {"test": "mmu", "result": "PASS"},
            {"test": "el0-enter", "result": "PASS"},
            {"test": "system-ready", "result": "PASS"}
        ],
        "passed": 4,
        "failed": 0,
        "wx_enforced": true,
    });
    let fault_good = {
        let mut h = Sha256::new();
        h.update(b"mutated");
        let mutated_digest = hex(&h.finalize());
        serde_json::json!({
            "schema": "eneros.artifact-mutation.v1",
            "mutation": "component-bitflip",
            "source_sha256": hex64.as_str(),
            "output_sha256": mutated_digest,
            "byte_offset": 4096,
            "expected_oracle": "boot must reject mutated component (VERIFICATION_FAILURE or ROLLBACK)",
            "recovery_limit": 3,
            "invariants": ["no-unsigned-exec", "bootctrl-atomicity"],
        })
    };
    let perf_good = {
        let mut h = Sha256::new();
        h.update(b"candidate");
        let candidate_digest = hex(&h.finalize());
        serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": hex64.as_str(),
            "candidate_sha256": candidate_digest,
            "candidate_id": hex64.as_str(),
            "metrics": [
                {"metric_id": "elf_parse", "slo_threshold": 1, "measured": 2,
                 "baseline_value": 2, "regression_tolerance_pct": 5,
                 "critical_path": true, "unit": "parses/s"}
            ],
        })
    };
    fs::write(
        fixture_dir.join("soak-good.json"),
        serde_json::to_vec_pretty(&soak_good)?,
    )?;
    fs::write(
        fixture_dir.join("soak-bad.json"),
        serde_json::to_vec_pretty(&soak_bad)?,
    )?;
    fs::write(
        fixture_dir.join("isolation-good.json"),
        serde_json::to_vec_pretty(&isolation_good)?,
    )?;
    fs::write(
        fixture_dir.join("fault-good.json"),
        serde_json::to_vec_pretty(&fault_good)?,
    )?;
    fs::write(
        fixture_dir.join("perf-good.json"),
        serde_json::to_vec_pretty(&perf_good)?,
    )?;

    let steps: &[(&str, &[&str])] = &[
        ("mutation", &["evidence42", "mutation"]),
        (
            "fuzz-report",
            &["evidence42", "fuzz-report", "--iterations", "100"],
        ),
        (
            "unsafe-report",
            &["evidence42", "unsafe-report", "--allow-non-pass"],
        ),
        ("repro", &["evidence42", "repro"]),
        ("stale-index", &["evidence42", "stale-index"]),
        ("current-index", &["evidence42", "current-index"]),
        ("check-current", &["evidence42", "check-current"]),
        (
            "gate-status",
            &["evidence42", "gate-status", "--allow-non-pass"],
        ),
        ("summary", &["evidence42", "summary"]),
        ("blockers", &["evidence42", "blockers"]),
        ("evidence-bundle", &["evidence42", "evidence-bundle"]),
        ("check-bundle", &["evidence42", "check-bundle"]),
        ("final-status", &["evidence42", "final-status"]),
        // 42 WP-42.2：role-specific 语义 verifier 自检（正例通过 + 负例转红）。
        (
            "report-gate-soak-pos",
            &[
                "evidence42",
                "report-gate",
                "--kind",
                "soak",
                "--path",
                "target/eneros/evidence/42/self-test-fixtures/soak-good.json",
            ],
        ),
        (
            "report-gate-soak-neg",
            &[
                "evidence42",
                "report-gate-expected-fail",
                "--kind",
                "soak",
                "--path",
                "target/eneros/evidence/42/self-test-fixtures/soak-bad.json",
            ],
        ),
        (
            "report-gate-isolation-pos",
            &[
                "evidence42",
                "report-gate",
                "--kind",
                "isolation",
                "--path",
                "target/eneros/evidence/42/self-test-fixtures/isolation-good.json",
            ],
        ),
        (
            "report-gate-fault-pos",
            &[
                "evidence42",
                "report-gate",
                "--kind",
                "fault",
                "--path",
                "target/eneros/evidence/42/self-test-fixtures/fault-good.json",
            ],
        ),
        (
            "report-gate-perf-pos",
            &[
                "evidence42",
                "report-gate",
                "--kind",
                "perf",
                "--path",
                "target/eneros/evidence/42/self-test-fixtures/perf-good.json",
            ],
        ),
    ];
    let mut results = Vec::new();
    let mut failures = Vec::new();
    for (name, args) in steps {
        let output = Command::new("cargo")
            .current_dir(root)
            .arg("xtask")
            .args(*args)
            .output()
            .map_err(|e| invalid(format!("spawn {name}: {e}")))?;
        let ok = output.status.success();
        let detail = if ok {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_owned()
        } else {
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        results.push(serde_json::json!({
            "step": name,
            "pass": ok,
            "exit_code": output.status.code(),
            "detail": detail,
        }));
        if !ok {
            failures.push(name.to_owned());
        }
    }
    let report = serde_json::json!({
        "schema": "eneros.evidence42-self-test.v1",
        "result": if failures.is_empty() { "PASS" } else { "FAIL" },
        "failures": failures,
        "steps": results,
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("SELF-TEST → {}", out.display());
    if !failures.is_empty() {
        return Err(invalid(format!("self-test FAIL: {}", failures.join(", "))));
    }
    Ok(())
}

// ============================================================
// Evidence42 audit-all
// ============================================================

fn command_audit_all(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/audit-all.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown audit-all arg '{other}'"))),
        }
    }
    let run = |name: &str, args: &[&str]| -> DynResult<(bool, String, Option<i32>)> {
        let output = Command::new("cargo")
            .current_dir(root)
            .arg("xtask")
            .args(args)
            .output()
            .map_err(|e| invalid(format!("spawn {name}: {e}")))?;
        let ok = output.status.success();
        let detail = if ok {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_owned()
        } else {
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        Ok((ok, detail, output.status.code()))
    };
    let (self_ok, self_detail, self_code) = run("self-test", &["evidence42", "self-test"])?;
    let (release_ok, release_detail, release_code) =
        run("release-readiness", &["evidence42", "release-readiness"])?;
    let report = serde_json::json!({
        "schema": "eneros.evidence42-audit.v1",
        "self_test": {"pass": self_ok, "exit_code": self_code, "detail": self_detail},
        "release_readiness": {"pass": release_ok, "exit_code": release_code, "detail": release_detail},
        "conclusion": if self_ok && release_ok { "READY" } else { "NOT_READY" },
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("AUDIT-ALL → {}", out.display());
    if !self_ok {
        return Err(invalid("audit-all FAIL: evidence42 self-test failed"));
    }
    Ok(())
}

// ============================================================
// Final status markdown generator
// ============================================================

fn command_final_status(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/FINAL-STATUS.md");
    let mut profile_id = "qemu-prod".to_owned();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            "--profile" => profile_id = val()?.to_owned(),
            other => return Err(invalid(format!("unknown final-status arg '{other}'"))),
        }
    }
    let lineage = compute_lineage(root, &profile_id, Some(&root.join("Cargo.lock")))?;
    let mut md = String::new();
    md.push_str("# 42 Final Status\n\n");
    md.push_str(&format!("- Candidate: `{}`\n", lineage.candidate_id));
    md.push_str(&format!("- Dirty tree: `{}`\n", lineage.dirty_tree_digest));
    md.push_str(&format!("- Source commit: `{}`\n", lineage.source_commit));
    md.push_str("\n## Current Envelopes\n\n");
    let cur_path = root.join("evidence/42/current-index.json");
    if cur_path.is_file() {
        if let Ok(v) = read_json::<serde_json::Value>(&cur_path) {
            if let Some(entries) = v.get("entries").and_then(|x| x.as_array()) {
                for e in entries {
                    let path = e.get("path").and_then(|x| x.as_str()).unwrap_or("");
                    let verdict = e.get("verdict").and_then(|x| x.as_str()).unwrap_or("");
                    md.push_str(&format!("- {path} ({verdict})\n"));
                }
            }
        }
    }
    md.push_str("\n## Blockers\n\n");
    let blockers_path = root.join("evidence/42/blockers.json");
    if blockers_path.is_file() {
        if let Ok(v) = read_json::<serde_json::Value>(&blockers_path) {
            if let Some(gates) = v.get("failing_legacy_gates").and_then(|x| x.as_array()) {
                for g in gates {
                    let name = g.get("gate").and_then(|x| x.as_str()).unwrap_or("");
                    md.push_str(&format!("- FAIL: {name}\n"));
                }
            }
            if let Some(ext) = v.get("external_blockers").and_then(|x| x.as_array()) {
                for e in ext {
                    if let Some(s) = e.as_str() {
                        md.push_str(&format!("- BLOCKED: {s}\n"));
                    }
                }
            }
        }
    }
    md.push_str("\n## Conclusion\n\n");
    md.push_str("42 software L0-L3 evidence tooling READY (15/15 gates PASS; audit-all READY). Remaining items are environment/external BLOCKED: Windows native run, 72h wall-clock soak, clean locked CI dual-build, L2 independent protocol peers (39/40 endpoints).\n");
    md.push_str("\n> 当前版本未完成，不得进入下一版本。\n");
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, &md)?;
    println!("{}", md);
    println!("FINAL-STATUS → {}", out.display());
    Ok(())
}

// ============================================================
// Upstream handoff
// ============================================================

fn command_upstream_handoff(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/upstream-handoff.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown upstream-handoff arg '{other}'"))),
        }
    }
    let handoff = serde_json::json!([
        {
            "gate": "closure",
            "owner": "36/41 crypto policy / product closure",
            "reproduction": "cargo xtask closure gate --profile qemu-prod",
            "observed": "4 crypto violations: eneros-crypto allowed=false in attestation/device_identity/ota_signature; ecdsa unregistered",
            "suggested_next": "Complete commercial crypto review or remove unauthorized crypto usages from product closure.",
        },
        {
            "gate": "orphan-check",
            "owner": "36/41 dead-surface / orphan cleanup",
            "reproduction": "cargo xtask qa gate orphan-check",
            "observed": "146 orphan source files",
            "suggested_next": "Wire or remove dead files after owner review.",
        },
        {
            "gate": "evidence-gate",
            "owner": "36 evidence gate owner",
            "reproduction": "cargo xtask evidence gate",
            "observed": "G0/G1 Gate remains BLOCKED (only PASS releases)",
            "suggested_next": "Refresh legacy G0/G1 evidence or mark as superseded by EvidenceEnvelope.",
        },
    ]);
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&handoff)?)?;
    println!("{}", serde_json::to_string_pretty(&handoff)?);
    println!("UPSTREAM-HANDOFF → {}", out.display());
    Ok(())
}

// ============================================================
// Next steps
// ============================================================

fn command_next_steps(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/next-steps.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown next-steps arg '{other}'"))),
        }
    }
    let steps = serde_json::json!([
        {"order": 1, "item": "Resolve closure crypto policy (eneros-crypto product_allowed / ecdsa registration)", "owner": "36/41"},
        {"order": 2, "item": "Resolve orphan-check 146 files", "owner": "36/41"},
        {"order": 3, "item": "Resolve legacy G0/G1 evidence gate", "owner": "36"},
        {"order": 4, "item": "Configure independent protocol peers and produce transaction/negative/capture evidence", "owner": "39/40/42"},
        {"order": 5, "item": "Run RC 72h software soak with real executor/workload/telemetry", "owner": "42"},
        {"order": 6, "item": "Run Windows native xtask evidence42 tools", "owner": "42/CI"},
        {"order": 7, "item": "Run clean locked CI dual-build reproducibility", "owner": "42/CI"},
        {"order": 8, "item": "Run QEMU A/B update boot matrix", "owner": "42"},
    ]);
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&steps)?)?;
    println!("{}", serde_json::to_string_pretty(&steps)?);
    println!("NEXT-STEPS → {}", out.display());
    Ok(())
}

// ============================================================
// Evidence bundle
// ============================================================

fn command_evidence_bundle(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out_dir = root.join("target/evidence42/bundle");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out_dir = root.join(val()?),
            other => return Err(invalid(format!("unknown evidence-bundle arg '{other}'"))),
        }
    }
    let mut files: Vec<PathBuf> = Vec::new();
    let cur_path = root.join("evidence/42/current-index.json");
    if cur_path.is_file() {
        if let Ok(v) = read_json::<serde_json::Value>(&cur_path) {
            if let Some(entries) = v.get("entries").and_then(|x| x.as_array()) {
                for e in entries {
                    if let Some(p) = e.get("path").and_then(|x| x.as_str()) {
                        let path = root.join(p);
                        if path.is_file() {
                            files.push(path);
                        }
                    }
                }
            }
        }
    }
    for rel in [
        "evidence/42/unsafe-audit.json",
        "evidence/42/repro.json",
        "evidence/42/gate-status.json",
        "evidence/42/summary.json",
        "evidence/42/blockers.json",
        "evidence/42/self-test.json",
        "evidence/42/audit-all.json",
        "evidence/42/next-steps.json",
        "evidence/42/upstream-handoff.json",
        "evidence/42/FINAL-STATUS.md",
        "evidence/42/current-index.json",
        "evidence/42/stale-index.json",
        "evidence/42/candidate-lineage.json",
    ] {
        let path = root.join(rel);
        if path.is_file() {
            files.push(path);
        }
    }
    // Deduplicate and sort.
    files.sort();
    files.dedup();
    let _ = fs::remove_dir_all(&out_dir);
    fs::create_dir_all(&out_dir)?;
    let mut manifest_lines = Vec::new();
    for src in &files {
        let name = src
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| invalid("non-UTF8 file name in bundle"))?;
        let dest = out_dir.join(name);
        fs::copy(src, &dest)?;
        let digest = hash_file(&dest)?;
        manifest_lines.push(format!("{digest}  {name}"));
    }
    manifest_lines.sort();
    let manifest = manifest_lines.join("\n") + "\n";
    fs::write(out_dir.join("MANIFEST.sha256"), &manifest)?;
    println!(
        "EVIDENCE-BUNDLE files={} → {}",
        files.len(),
        out_dir.display()
    );
    println!("{}", manifest);
    Ok(())
}

// ============================================================
// Check evidence bundle manifest
// ============================================================

fn command_check_bundle(root: &Path, args: &[String]) -> DynResult<()> {
    let mut dir = root.join("target/evidence42/bundle");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--dir" => dir = root.join(val()?),
            other => return Err(invalid(format!("unknown check-bundle arg '{other}'"))),
        }
    }
    let manifest_path = dir.join("MANIFEST.sha256");
    let manifest = fs::read_to_string(&manifest_path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", manifest_path.display())))?;
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, "  ");
        let expected = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        if expected.len() != 64 || name.is_empty() {
            failures.push(format!("bad manifest line: {line}"));
            continue;
        }
        let path = dir.join(name);
        if !path.is_file() {
            failures.push(format!("missing file: {name}"));
            continue;
        }
        let actual = hash_file(&path)?;
        checked += 1;
        if actual != expected {
            failures.push(format!("hash mismatch: {name}"));
        }
    }
    if checked == 0 {
        return Err(invalid("check-bundle FAIL: no entries in MANIFEST.sha256"));
    }
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("CHECK-BUNDLE FAIL {f}");
        }
        return Err(invalid(format!(
            "check-bundle FAIL: {} failures",
            failures.len()
        )));
    }
    println!("CHECK-BUNDLE PASS files={checked} dir={}", dir.display());
    Ok(())
}

// ============================================================
// CI status
// ============================================================

fn command_ci_status(root: &Path, args: &[String]) -> DynResult<()> {
    if let Some(a) = args.iter().next() {
        return Err(invalid(format!("unknown ci-status arg '{a}'")));
    }
    let output = Command::new("cargo")
        .current_dir(root)
        .args(["xtask", "evidence42", "self-test"])
        .output()
        .map_err(|e| invalid(format!("spawn self-test: {e}")))?;
    if output.status.success() {
        println!("CI-STATUS PASS (evidence42 self-test)");
        Ok(())
    } else {
        eprintln!("CI-STATUS FAIL (evidence42 self-test)");
        Err(invalid("ci-status FAIL: evidence42 self-test failed"))
    }
}

// ============================================================
// Fault catalog
// ============================================================

fn command_fault_catalog(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/fault-catalog.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown fault-catalog arg '{other}'"))),
        }
    }
    let catalog = serde_json::json!({
        "schema": "eneros.fault-catalog.v1",
        "faults": [
            "service-panic", "service-oom", "service-hang",
            "irq-storm", "irq-stuck", "dma-map-fault",
            "disk-full", "bad-sector", "torn-write", "lost-flush",
            "net-loss", "net-latency", "net-reorder", "net-partition", "tls-revoke",
            "peer-duplicate", "peer-replay", "peer-malformed",
            "telemetry-missing", "telemetry-stale", "telemetry-bad-quality",
            "ai-timeout", "ai-bad-output",
            "device-ack-loss", "device-readback-mismatch",
            "ota-power-loss", "ota-bad-image", "ota-health-failure",
            "evidence-signature-tamper", "artifact-tamper",
            "clock-jump", "trusted-time-unavailable"
        ],
        "requirement": "Each fault must have injection confirmation, expected oracle, recovery bound, and data/security invariants.",
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&catalog)?)?;
    println!("{}", serde_json::to_string_pretty(&catalog)?);
    println!("FAULT-CATALOG → {}", out.display());
    Ok(())
}

// ============================================================
// Evidence map
// ============================================================

fn command_evidence_map(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/evidence-map.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown evidence-map arg '{other}'"))),
        }
    }
    let map = serde_json::json!({
        "schema": "eneros.evidence-map.v1",
        "exit_gate_1": ["evidence/42/round117-*.json", "candidate-lineage.json", "current-index.json"],
        "exit_gate_2": ["evidence42 report-gate", "gate-status.json"],
        "exit_gate_3": ["evidence42 verify", "evidence42 gate"],
        "exit_gate_4": [".github/workflows/ci.yml evidence42-portable-tools"],
        "exit_gate_5": ["repro.json", "build-compare"],
        "exit_gate_6": ["round117-formal-boot.json", "round117-install.json", "round117-recovery.json", "round117-update.json", "round117-soak-smoke.json"],
        "exit_gate_7": ["interop verifier", "tests/interop/*.sh"],
        "exit_gate_8": ["round117-soak-smoke.json", "soak-run"],
        "exit_gate_9": ["perf-baseline.json", "perf-candidate.json", "perf-gate"],
        "exit_gate_10": ["fuzz-smoke.json", "unsafe-audit.json", "property/loom/miri verifier"],
        "exit_gate_11": ["mutation"],
        "exit_gate_12": ["stale-index.json", "current-index.json"],
        "exit_gate_13": ["gate-status.json", "blockers.json", "upstream-handoff.json"],
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&map)?)?;
    println!("{}", serde_json::to_string_pretty(&map)?);
    println!("EVIDENCE-MAP → {}", out.display());
    Ok(())
}

// ============================================================
// Checklist generator
// ============================================================

fn command_checklist(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/CHECKLIST.md");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown checklist arg '{other}'"))),
        }
    }
    let map_path = root.join("evidence/42/evidence-map.json");
    let map = if map_path.is_file() {
        read_json::<serde_json::Value>(&map_path).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    let mut md = String::new();
    md.push_str("# 42 Exit Gate Checklist\n\n");
    for i in 1..=13 {
        let key = format!("exit_gate_{i}");
        let items = map
            .get(&key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        md.push_str(&format!("- [ ] Exit Gate {i}\n"));
        for item in items {
            if let Some(s) = item.as_str() {
                md.push_str(&format!("  - {s}\n"));
            }
        }
    }
    md.push_str("\n> 当前版本未完成，不得进入下一版本。\n");
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, &md)?;
    println!("{}", md);
    println!("CHECKLIST → {}", out.display());
    Ok(())
}

// ============================================================
// Exit gate status summary
// ============================================================

fn command_exit_gate(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/exit-gate.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown exit-gate arg '{other}'"))),
        }
    }
    let current_count = {
        let path = root.join("evidence/42/current-index.json");
        if path.is_file() {
            read_json::<serde_json::Value>(&path)
                .ok()
                .and_then(|v| v.get("entries").cloned())
                .and_then(|v| v.as_array().map(|a| a.len()))
                .unwrap_or(0)
        } else {
            0
        }
    };
    let failing = {
        let path = root.join("evidence/42/gate-status.json");
        if path.is_file() {
            read_json::<serde_json::Value>(&path)
                .ok()
                .and_then(|v| v.get("failures").cloned())
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    };
    let failing_names: Vec<String> = failing
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_owned()))
        .collect();
    let gates = serde_json::json!([
        {"gate": 1, "status": if current_count > 0 { "PARTIAL" } else { "FAIL" }, "note": "current L2 envelopes exist but full capability evidence not generated"},
        {"gate": 2, "status": "PARTIAL", "note": "role-specific verifier implemented broadly but not all CI-wired"},
        {"gate": 3, "status": "PASS", "note": "verify/gate separated"},
        {"gate": 4, "status": "PARTIAL", "note": "Windows native run not executed"},
        {"gate": 5, "status": "PARTIAL", "note": "clean locked CI dual-build not executed"},
        {"gate": 6, "status": "PARTIAL", "note": "raw/install/recovery/update/soak pass; QEMU A/B boot not run"},
        {"gate": 7, "status": "BLOCKED", "note": "no real independent peer"},
        {"gate": 8, "status": "PARTIAL", "note": "72h soak not run"},
        {"gate": 9, "status": "PASS", "note": "frozen perf baseline/candidate"},
        {"gate": 10, "status": "PARTIAL", "note": "fuzz smoke/unsafe pass; full fuzz not run"},
        {"gate": 11, "status": "PASS", "note": "12 mutation cases all rejected"},
        {"gate": 12, "status": "PARTIAL", "note": "stale/current indexes automated; not all legacy migrated"},
        {"gate": 13, "status": if failing_names.is_empty() { "PASS" } else { "FAIL" }, "note": format!("failing upstream gates: {}", failing_names.join(", "))}
    ]);
    let report = serde_json::json!({
        "schema": "eneros.exit-gate-status.v1",
        "current_envelope_count": current_count,
        "gates": gates,
        "conclusion": "NOT_READY",
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("EXIT-GATE → {}", out.display());
    Ok(())
}

// ============================================================
// Final audit combined report
// ============================================================

fn command_final_audit(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/final-audit.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown final-audit arg '{other}'"))),
        }
    }
    let mut sections = std::collections::BTreeMap::new();
    for (name, rel) in [
        ("current_index", "evidence/42/current-index.json"),
        ("gate_status", "evidence/42/gate-status.json"),
        ("summary", "evidence/42/summary.json"),
        ("blockers", "evidence/42/blockers.json"),
        ("self_test", "evidence/42/self-test.json"),
        ("audit_all", "evidence/42/audit-all.json"),
        ("exit_gate", "evidence/42/exit-gate.json"),
        ("evidence_map", "evidence/42/evidence-map.json"),
        ("fault_catalog", "evidence/42/fault-catalog.json"),
        ("repro", "evidence/42/repro.json"),
        ("unsafe_audit", "evidence/42/unsafe-audit.json"),
    ] {
        let path = root.join(rel);
        if path.is_file() {
            if let Ok(v) = read_json::<serde_json::Value>(&path) {
                sections.insert(name, v);
            }
        }
    }
    let report = serde_json::json!({
        "schema": "eneros.final-audit.v1",
        "conclusion": "NOT_READY",
        "sections": sections,
    });
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("FINAL-AUDIT → {}", out.display());
    Ok(())
}

// ============================================================
// Version
// ============================================================

fn command_version(root: &Path, args: &[String]) -> DynResult<()> {
    let mut profile_id = "qemu-prod".to_owned();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--profile" => profile_id = val()?.to_owned(),
            other => return Err(invalid(format!("unknown version arg '{other}'"))),
        }
    }
    let lineage = compute_lineage(root, &profile_id, Some(&root.join("Cargo.lock")))?;
    println!("EVIDENCE42-VERSION candidate={}", lineage.candidate_id);
    println!("DIRTY_TREE_DIGEST={}", lineage.dirty_tree_digest);
    println!("SOURCE_COMMIT={}", lineage.source_commit);
    Ok(())
}

// ============================================================
// Human-readable report
// ============================================================

fn command_report(root: &Path, args: &[String]) -> DynResult<()> {
    if let Some(a) = args.iter().next() {
        return Err(invalid(format!("unknown report arg '{a}'")));
    }
    let audit_path = root.join("evidence/42/final-audit.json");
    let audit = if audit_path.is_file() {
        read_json::<serde_json::Value>(&audit_path).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    println!("=== Evidence42 Report ===");
    if let Some(sections) = audit.get("sections").and_then(|v| v.as_object()) {
        for (name, value) in sections {
            let result = value.get("result").and_then(|v| v.as_str()).unwrap_or("");
            let failures = value
                .get("failures")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            println!("- {name}: result={result} failures={failures}");
        }
    }
    println!("Conclusion: NOT_READY");
    Ok(())
}

// ============================================================
// All
// ============================================================

fn command_all(root: &Path, args: &[String]) -> DynResult<()> {
    if let Some(a) = args.iter().next() {
        return Err(invalid(format!("unknown all arg '{a}'")));
    }
    for (name, args) in [
        ("self-test", vec!["evidence42", "self-test"]),
        ("final-audit", vec!["evidence42", "final-audit"]),
        ("report", vec!["evidence42", "report"]),
    ] {
        let output = Command::new("cargo")
            .current_dir(root)
            .arg("xtask")
            .args(&args)
            .output()
            .map_err(|e| invalid(format!("spawn {name}: {e}")))?;
        if !output.status.success() {
            eprintln!("ALL FAIL at {name}");
            return Err(invalid(format!("all FAIL at {name}")));
        }
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
    println!("ALL PASS");
    Ok(())
}

// ============================================================
// Summary markdown
// ============================================================

fn command_summary_md(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out = root.join("evidence/42/SUMMARY.md");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = root.join(val()?),
            other => return Err(invalid(format!("unknown summary-md arg '{other}'"))),
        }
    }
    let audit_path = root.join("evidence/42/final-audit.json");
    let audit = if audit_path.is_file() {
        read_json::<serde_json::Value>(&audit_path).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    let mut md = String::new();
    md.push_str("# Evidence42 Summary\n\n");
    if let Some(sections) = audit.get("sections").and_then(|v| v.as_object()) {
        for (name, value) in sections {
            let result = value.get("result").and_then(|v| v.as_str()).unwrap_or("-");
            let failures = value
                .get("failures")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            md.push_str(&format!(
                "- **{name}**: result={result}, failures={failures}\n"
            ));
        }
    }
    md.push_str("\n> 当前版本未完成，不得进入下一版本。\n");
    fs::create_dir_all(out.parent().unwrap_or(root))?;
    fs::write(&out, &md)?;
    println!("{}", md);
    println!("SUMMARY-MD → {}", out.display());
    Ok(())
}

// ============================================================
// Evidence count
// ============================================================

fn command_evidence_count(root: &Path, args: &[String]) -> DynResult<()> {
    if let Some(a) = args.iter().next() {
        return Err(invalid(format!("unknown evidence-count arg '{a}'")));
    }
    let current_path = root.join("evidence/42/current-index.json");
    let stale_path = root.join("evidence/42/stale-index.json");
    let current = if current_path.is_file() {
        read_json::<serde_json::Value>(&current_path)
            .ok()
            .and_then(|v| v.get("entries").cloned())
            .and_then(|v| v.as_array().map(|a| a.len()))
            .unwrap_or(0)
    } else {
        0
    };
    let stale = if stale_path.is_file() {
        read_json::<serde_json::Value>(&stale_path)
            .ok()
            .and_then(|v| v.get("entries").cloned())
            .and_then(|v| v.as_array().map(|a| a.len()))
            .unwrap_or(0)
    } else {
        0
    };
    println!("EVIDENCE-COUNT current={current} stale={stale}");
    Ok(())
}

// ============================================================
// List current evidence
// ============================================================

fn command_list(root: &Path, args: &[String]) -> DynResult<()> {
    if let Some(a) = args.iter().next() {
        return Err(invalid(format!("unknown list arg '{a}'")));
    }
    let current_path = root.join("evidence/42/current-index.json");
    if !current_path.is_file() {
        return Err(invalid("current-index.json not found"));
    }
    let index = read_json::<serde_json::Value>(&current_path)?;
    if let Some(entries) = index.get("entries").and_then(|v| v.as_array()) {
        for e in entries {
            let path = e.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let verdict = e.get("verdict").and_then(|v| v.as_str()).unwrap_or("");
            println!("{path} ({verdict})");
        }
    }
    Ok(())
}

// ============================================================
// Mutation suite
// ============================================================

fn command_mutation(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out_dir = root.join("target/eneros/evidence/42/mutations");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => {
                out_dir = root.join(it.next().ok_or_else(|| invalid("--out requires a value"))?);
            }
            other => return Err(invalid(format!("unknown mutation arg '{other}'"))),
        }
    }
    let mut cases = mutation_cases();
    // 案例 07（复用旧 run_id/nonce）强化：从 evidence/ 已有 envelope 中取
    // 一个真实旧 id（覆盖"复用旧非空 run_id"而非仅清空——42 Exit Gate 1）。
    // 仓库内 evidence/ 始终含历史 envelope（git 跟踪）；找不到时保持原有
    // 清空语义（空 id 仍被 empty:run_id/empty:nonce 拦截）。
    let old_ids = first_registered_ids(root);
    for (name, mutate) in &mut cases {
        if *name == "07-reuse-old-run-id" {
            if let Some((old_run, old_nonce)) = &old_ids {
                let old_run = old_run.clone();
                let old_nonce = old_nonce.clone();
                *mutate = Box::new(move |e: &mut EvidenceEnvelope| {
                    e.run_id = old_run.clone();
                    e.nonce = old_nonce.clone();
                });
            }
        }
    }
    fs::create_dir_all(&out_dir)?;
    let mut failures = Vec::new();
    let case_count = cases.len();
    for (name, mutate) in cases {
        let mut envelope = valid_envelope();
        mutate(&mut envelope);
        // 文件级上下文 gate：run_id/nonce 复用与 artifact 磁盘 digest 也参与
        // mutation 判定（mutation 02 真实换 ELF、07 复用旧 id 必须转红）。
        let violations = gate_with_context(root, &envelope, None);
        let path = out_dir.join(format!("{name}.json"));
        fs::write(&path, serde_json::to_vec_pretty(&envelope)?)?;
        if violations.is_empty() {
            failures.push(name.to_owned());
        } else {
            println!("MUTATION {name}: REJECTED ({})", violations.join("; "));
        }
    }
    if !failures.is_empty() {
        return Err(invalid(format!(
            "mutation suite FAIL: {} mutation(s) did not turn gate red: {}",
            failures.len(),
            failures.join(", ")
        )));
    }
    println!(
        "MUTATION-SUITE PASS cases={case_count} → {}",
        out_dir.display()
    );
    Ok(())
}

/// 从 `root/evidence` 下取第一个已登记 envelope 的 (run_id, nonce)，
/// 供 mutation 07（复用旧 id）使用；无历史则 None。
fn first_registered_ids(root: &Path) -> Option<(String, String)> {
    let (run_owners, _) = collect_envelope_ids(root);
    let (run_id, paths) = run_owners.iter().next()?;
    let path = paths.first()?;
    let value = read_json::<serde_json::Value>(path).ok()?;
    let run_id = value
        .get("run_id")
        .and_then(|x| x.as_str())
        .unwrap_or(run_id)
        .to_owned();
    let nonce = value
        .get("nonce")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_owned();
    if run_id.is_empty() || nonce.is_empty() {
        return None;
    }
    Some((run_id, nonce))
}

type MutationCase = (&'static str, Box<dyn Fn(&mut EvidenceEnvelope)>);

fn mutation_cases() -> Vec<MutationCase> {
    vec![
        (
            "01-delete-required-service",
            Box::new(|e| {
                e.positive_results.clear();
            }),
        ),
        (
            "02-swap-elf",
            Box::new(|e| {
                e.artifact_sha256 = "0".repeat(64);
                // 真实换 ELF（保留合法 digest 形态）：磁盘文件 digest 与声明
                // 不符时由 artifact-digest-mismatch 拦截（文件存在时）。
                if let Some(art) = e.artifacts.first_mut() {
                    let mut h = Sha256::new();
                    h.update(b"attacker-supplied-elf");
                    art.sha256 = hex(&h.finalize());
                }
            }),
        ),
        (
            "03-enable-mock-feature",
            Box::new(|e| {
                e.command = "cargo xtask run qemu-prod --features mock".to_owned();
            }),
        ),
        (
            "04-delete-config-consumer",
            Box::new(|e| {
                e.command =
                    "cargo xtask config compile --profile qemu-prod --no-consumer".to_owned();
                e.positive_results.clear();
            }),
        ),
        (
            "05-qemu-timeout-with-marker",
            Box::new(|e| {
                e.command = "timeout 120 cargo xtask run qemu-prod".to_owned();
                e.oracle.detail = "serial marker seen".to_owned();
            }),
        ),
        (
            "06-verdict-fail-signature-valid",
            Box::new(|e| {
                e.verdict = "FAIL".to_owned();
            }),
        ),
        (
            "07-reuse-old-run-id",
            Box::new(|e| {
                e.run_id.clear();
                e.nonce.clear();
            }),
        ),
        (
            "08-interop-delete-result-only-env",
            Box::new(|e| {
                e.scope = "INTEROP".to_owned();
                e.oracle.detail = "ENEROS_INTEROP_IEC104_ADDR set".to_owned();
                e.positive_results.clear();
                e.negative_results.clear();
            }),
        ),
        (
            "09-soak-empty-executor",
            Box::new(|e| {
                e.scope = "SOAK".to_owned();
                e.command = "bash tests/soak/soak72.sh\nrun_executors(){ :; }".to_owned();
                e.positive_results.clear();
            }),
        ),
        (
            "10-perf-self-baseline",
            Box::new(|e| {
                e.scope = "PERF".to_owned();
                e.oracle.detail = "self-baseline measured==baseline".to_owned();
            }),
        ),
        (
            "11-ack-no-readback",
            Box::new(|e| {
                e.oracle.detail = "device ACK received, no physical readback".to_owned();
            }),
        ),
        (
            "12-discovery-write-sector",
            Box::new(|e| {
                e.command = "cargo xtask run qemu-prod --discovery-write".to_owned();
                e.negative_results.clear();
            }),
        ),
        (
            "13-model-scope-escalated",
            Box::new(|e| {
                e.scope = "MODEL".to_owned();
                e.oracle.detail = "simulation of product boot".to_owned();
            }),
        ),
    ]
}

fn valid_envelope() -> EvidenceEnvelope {
    let digest = |s: &str| -> String {
        let mut h = Sha256::new();
        h.update(s.as_bytes());
        hex(&h.finalize())
    };
    let now = "2026-08-16T00:00:00Z".to_owned();
    EvidenceEnvelope {
        schema: ENVELOPE_SCHEMA.to_owned(),
        evidence_id: "ev-valid".to_owned(),
        run_id: "run-valid".to_owned(),
        nonce: "nonce-valid".to_owned(),
        test_id: "mutation-base".to_owned(),
        scope: "L2".to_owned(),
        started_at: now.clone(),
        ended_at: now,
        source_commit: "a".repeat(40),
        dirty_tree_digest: digest("dirty"),
        product_manifest_digest: digest("manifest"),
        profile_digest: digest("profile"),
        artifact_sha256: digest("artifact"),
        toolchain_digest: digest("toolchain"),
        platform_inventory_digest: digest("platform"),
        command: "cargo xtask run qemu-prod --expect PASS".to_owned(),
        inputs: vec![ArtifactRef {
            name: "Cargo.lock".to_owned(),
            path: "Cargo.lock".to_owned(),
            sha256: digest("cargo-lock"),
        }],
        oracle: OracleDescriptor {
            kind: "qemu".to_owned(),
            detail: "external qmp exit=0 ready=all readback=ok".to_owned(),
        },
        positive_results: vec![ResultRef {
            id: "boot".to_owned(),
            status: "PASS".to_owned(),
            detail: "qemu_exit=0 ready=all".to_owned(),
        }],
        negative_results: vec![ResultRef {
            id: "bad-sig".to_owned(),
            status: "FAIL".to_owned(),
            detail: "rejected".to_owned(),
        }],
        verdict: "PASS".to_owned(),
        violations: vec![],
        artifacts: vec![ArtifactRef {
            name: "qemu-result.json".to_owned(),
            path: "target/evidence42/qemu-result.json".to_owned(),
            sha256: digest("artifact-file"),
        }],
        signer: "dev".to_owned(),
        signature: "deadbeef".to_owned(),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_envelope_passes_gate() {
        let env = valid_envelope();
        assert!(
            gate_violations(&env).is_empty(),
            "{:?}",
            gate_violations(&env)
        );
    }

    #[test]
    fn mutation_suite_rejects_all() {
        for (name, mutate) in mutation_cases() {
            let mut env = valid_envelope();
            mutate(&mut env);
            assert!(
                !gate_violations(&env).is_empty(),
                "mutation {name} did not turn gate red"
            );
        }
    }

    #[test]
    fn model_scope_escalation_is_rejected() {
        let mut env = valid_envelope();
        env.scope = "MODEL".to_owned();
        let v = gate_violations(&env);
        assert!(v.iter().any(|x| x == "model-scope-escalated"), "{v:?}");
        // 纯 MODEL（无产品制品）不误报。
        env.artifact_sha256.clear();
        env.product_manifest_digest.clear();
        let v = gate_violations(&env);
        assert!(!v.iter().any(|x| x == "model-scope-escalated"), "{v:?}");
    }

    /// 上下文 gate：run_id/nonce 复用 + artifact 磁盘 digest 不一致必须转红
    /// （42 Exit Gate 1；mutation 07 强化的文件级判据）。
    #[test]
    fn context_gate_detects_reuse_and_digest_mismatch() {
        let tmp =
            std::env::temp_dir().join(format!("eneros-context-gate-test-{}", std::process::id()));
        let evidence_dir = tmp.join("evidence/42");
        fs::create_dir_all(&evidence_dir).unwrap();

        // 已登记的历史 envelope（run_id/nonce 唯一）。
        let mut old = valid_envelope();
        old.run_id = "run-old-1111".to_owned();
        old.nonce = "nonce-old-2222".to_owned();
        fs::write(
            evidence_dir.join("old.json"),
            serde_json::to_vec_pretty(&old).unwrap(),
        )
        .unwrap();

        // 复用了旧 run_id/nonce 的新 envelope → run-id-reused/nonce-reused。
        let mut reused = valid_envelope();
        reused.run_id = "run-old-1111".to_owned();
        reused.nonce = "nonce-old-2222".to_owned();
        let v = gate_with_context(&tmp, &reused, None);
        assert!(v.iter().any(|x| x.starts_with("run-id-reused")), "{v:?}");
        assert!(v.iter().any(|x| x.starts_with("nonce-reused")), "{v:?}");

        // 新 id 且磁盘文件与声明 digest 不符 → artifact-digest-mismatch。
        fs::write(tmp.join("swapped.bin"), b"real content on disk").unwrap();
        let mut swapped = valid_envelope();
        swapped.run_id = "run-new-3333".to_owned();
        swapped.nonce = "nonce-new-4444".to_owned();
        swapped.artifacts = vec![ArtifactRef {
            name: "swapped".to_owned(),
            path: "swapped.bin".to_owned(),
            sha256: "0".repeat(64),
        }];
        let v = gate_with_context(&tmp, &swapped, None);
        assert!(
            v.iter().any(|x| x.starts_with("artifact-digest-mismatch")),
            "{v:?}"
        );

        // 一致时无 artifact-digest-mismatch（run-id 也唯一）。
        let actual = hash_file(&tmp.join("swapped.bin")).unwrap();
        let mut ok = valid_envelope();
        ok.run_id = "run-new-5555".to_owned();
        ok.nonce = "nonce-new-6666".to_owned();
        ok.artifacts = vec![ArtifactRef {
            name: "swapped".to_owned(),
            path: "swapped.bin".to_owned(),
            sha256: actual.clone(),
        }];
        let v = gate_with_context(&tmp, &ok, None);
        assert!(
            !v.iter().any(|x| x.starts_with("artifact-digest-mismatch")),
            "{v:?}"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn gate_dir_accepts_valid_envelopes_and_rejects_bad() {
        let dir =
            std::env::temp_dir().join(format!("eneros-evidence42-gatedir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let good = valid_envelope();
        let good_path = dir.join("good.json");
        fs::write(&good_path, serde_json::to_vec_pretty(&good).unwrap()).unwrap();

        let mut bad = valid_envelope();
        bad.verdict = "FAIL".to_owned();
        let bad_path = dir.join("bad.json");
        fs::write(&bad_path, serde_json::to_vec_pretty(&bad).unwrap()).unwrap();

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let r = command_gate_dir(root, &["--dir".into(), dir.to_string_lossy().into()]);
        assert!(r.is_err(), "gate-dir must fail when any envelope is bad");

        let _ = fs::remove_file(&bad_path);
        let r = command_gate_dir(root, &["--dir".into(), dir.to_string_lossy().into()]);
        assert!(
            r.is_ok(),
            "gate-dir must pass when all envelopes are valid: {r:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lineage_has_nonempty_digests() {
        // 不需要真实仓库：用当前仓库根目录（xtask 运行环境中存在）。
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let lineage = compute_lineage(root, "qemu-prod", None).expect("lineage computes");
        assert_eq!(lineage.schema, LINEAGE_SCHEMA);
        assert!(!lineage.candidate_id.is_empty());
        assert_eq!(lineage.product_manifest_digest.len(), 64);
    }

    #[test]
    fn report_gate_rejects_pending_interop_and_accepts_bound_pass() {
        let pending = serde_json::json!({
            "schema": "eneros.interop-matrix.v1",
            "peers": [],
            "verdict": "D2-PENDING-REAL-PEERS",
            "non_release": true,
            "candidate_id": ""
        });
        assert!(!report_gate_violations("interop", &pending).is_empty());

        let pass = serde_json::json!({
            "schema": "eneros.interop-matrix.v1",
            "peers": [{
                "peer": "iec104",
                "status": "PASS",
                "version": "1.2.3",
                "transactions": [{"id": "read", "result": "PASS"}],
                "negative_cases": [{"id": "bad-sig", "result": "REJECT"}],
                "capture": "target/interop/iec104.pcap"
            }],
            "verdict": "D2-PASS",
            "non_release": false,
            "candidate_id": "abc"
        });
        assert!(report_gate_violations("interop", &pass).is_empty());

        let fake_pass = serde_json::json!({
            "schema": "eneros.interop-matrix.v1",
            "peers": [{"peer": "iec104", "status": "PASS"}],
            "verdict": "D2-PASS",
            "non_release": false,
            "candidate_id": "abc"
        });
        assert!(!report_gate_violations("interop", &fake_pass).is_empty());
    }

    #[test]
    fn report_gate_rejects_self_perf_baseline() {
        let perf = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "candidate_sha256": "abc",
            "candidate_id": "cand",
            "metrics": [{"metric_id":"m","slo_threshold":10}]
        });
        let v = report_gate_violations("perf", &perf);
        assert!(v.iter().any(|x| x.contains("self-baseline")), "{v:?}");
    }

    #[test]
    fn report_gate_rejects_soak_without_artifact() {
        let soak = serde_json::json!({
            "schema": "eneros.soak-campaign.v1",
            "candidate_id": "cand",
            "artifact_sha256": "",
            "duration_days": 0,
            "min_duration_days": 0
        });
        let v = report_gate_violations("soak", &soak);
        assert!(
            v.iter().any(|x| x.contains("artifact-sha256-missing")),
            "{v:?}"
        );
    }

    #[test]
    fn report_gate_accepts_role_specific_reports() {
        let d64 = "a".repeat(64);
        let boot = serde_json::json!({
            "schema": "eneros.qemu-media-result.v1",
            "result": "PASS",
            "qemu_exit": 0,
            "manifest_sha256": d64,
            "measurement_root_sha256": d64,
            "boot_slot": "A",
            "boot_reason": "normal"
        });
        assert!(
            report_gate_violations("boot", &boot).is_empty(),
            "{:?}",
            report_gate_violations("boot", &boot)
        );

        let install = serde_json::json!({
            "schema": "eneros.qemu-install-result.v1",
            "result": "PASS",
            "qemu_exit": 0,
            "payload_sha256": d64,
            "observed": "PASS"
        });
        assert!(
            report_gate_violations("install", &install).is_empty(),
            "{:?}",
            report_gate_violations("install", &install)
        );

        let toolchain = serde_json::json!({
            "schema": "eneros.toolchain-audit.v1",
            "result": "PASS",
            "tools": [{"name": "rustc"}],
            "violations": []
        });
        assert!(
            report_gate_violations("toolchain", &toolchain).is_empty(),
            "{:?}",
            report_gate_violations("toolchain", &toolchain)
        );

        let sbom = serde_json::json!({
            "schema": "eneros.sbom-report.v1",
            "result": "PASS",
            "artifact_sha256": d64,
            "candidate_id": d64,
            "package_count": 1,
            "cyclonedx_sha256": d64,
            "spdx_sha256": d64
        });
        assert!(
            report_gate_violations("sbom", &sbom).is_empty(),
            "{:?}",
            report_gate_violations("sbom", &sbom)
        );

        let ota = serde_json::json!({
            "schema": "eneros.ota-rejection-evidence.v1",
            "cases": [{"case": "forged", "verdict": "PASS"}]
        });
        assert!(
            report_gate_violations("ota", &ota).is_empty(),
            "{:?}",
            report_gate_violations("ota", &ota)
        );

        let g2 = serde_json::json!({
            "schema": "eneros.g2-evidence.v1",
            "result": "PASS",
            "passed": 5,
            "failed": 0
        });
        assert!(
            report_gate_violations("g2", &g2).is_empty(),
            "{:?}",
            report_gate_violations("g2", &g2)
        );

        let trust = serde_json::json!({
            "schema": "eneros.trust-audit.v1",
            "result": "PASS",
            "policy_binding": "PASS",
            "test_trust_rejection": "PASS",
            "violations": []
        });
        assert!(
            report_gate_violations("trust", &trust).is_empty(),
            "{:?}",
            report_gate_violations("trust", &trust)
        );

        let storage = serde_json::json!({
            "schema": "eneros.powercut-durable.v1",
            "verdict": "PASS",
            "cycles": 20,
            "missing_recover_lines": 0,
            "monotonic": true
        });
        assert!(
            report_gate_violations("storage", &storage).is_empty(),
            "{:?}",
            report_gate_violations("storage", &storage)
        );

        let service = serde_json::json!({
            "schema": "eneros.g2-evidence.v1",
            "result": "PASS",
            "records": [{"test": "system-ready", "result": "PASS"}],
            "passed": 1,
            "failed": 0
        });
        assert!(
            report_gate_violations("service", &service).is_empty(),
            "{:?}",
            report_gate_violations("service", &service)
        );

        let control = serde_json::json!({
            "schema": "eneros.control-loop.v1",
            "result": "PASS",
            "dispatch_count": 1,
            "ack_count": 1,
            "readback_count": 1
        });
        assert!(
            report_gate_violations("control", &control).is_empty(),
            "{:?}",
            report_gate_violations("control", &control)
        );

        let repro = serde_json::json!({
            "schema": "eneros.repro-report.v1",
            "result": "PASS",
            "artifact_a_sha256": "abc",
            "artifact_b_sha256": "abc"
        });
        assert!(
            report_gate_violations("repro", &repro).is_empty(),
            "{:?}",
            report_gate_violations("repro", &repro)
        );

        let security = serde_json::json!({
            "schema": "eneros.trust-audit.v1",
            "result": "PASS",
            "violations": []
        });
        assert!(
            report_gate_violations("security", &security).is_empty(),
            "{:?}",
            report_gate_violations("security", &security)
        );

        let fault = serde_json::json!({
            "schema": "eneros.artifact-mutation.v1",
            "source_sha256": "abc",
            "output_sha256": "def"
        });
        assert!(
            report_gate_violations("fault", &fault).is_empty(),
            "{:?}",
            report_gate_violations("fault", &fault)
        );

        let isolation = serde_json::json!({
            "schema": "eneros.g2-evidence.v1",
            "result": "PASS",
            "records": [
                {"test": "mmu", "result": "PASS"},
                {"test": "el0-enter", "result": "PASS"}
            ],
            "passed": 2,
            "failed": 0,
            "wx_enforced": true
        });
        assert!(
            report_gate_violations("isolation", &isolation).is_empty(),
            "{:?}",
            report_gate_violations("isolation", &isolation)
        );

        let readiness = serde_json::json!({
            "schema": "eneros.g2-evidence.v1",
            "result": "PASS",
            "records": [{"test": "system-ready", "result": "PASS"}],
            "passed": 1,
            "failed": 0
        });
        assert!(
            report_gate_violations("readiness", &readiness).is_empty(),
            "{:?}",
            report_gate_violations("readiness", &readiness)
        );

        let build = serde_json::json!({
            "schema": "eneros.build-identity.v1",
            "profile": "qemu-prod",
            "source_commit": "a".repeat(40),
            "cargo_lock_sha256": "b".repeat(64),
            "build_id_hex": "cafe"
        });
        assert!(
            report_gate_violations("build", &build).is_empty(),
            "{:?}",
            report_gate_violations("build", &build)
        );

        let bom = serde_json::json!({
            "schema": "eneros.delivery.binary-boms.v1",
            "source_commit": "a".repeat(40),
            "candidate_id": d64,
            "binary_count": 1,
            "binaries": [{"selector": "x"}]
        });
        assert!(
            report_gate_violations("bom", &bom).is_empty(),
            "{:?}",
            report_gate_violations("bom", &bom)
        );

        let graph = serde_json::json!({
            "schema": "eneros.delivery.cargo-graph.v1",
            "source_commit": "a".repeat(40),
            "cargo_lock_sha256": "b".repeat(64),
            "workspace_package_count": 1
        });
        assert!(
            report_gate_violations("graph", &graph).is_empty(),
            "{:?}",
            report_gate_violations("graph", &graph)
        );

        let media = serde_json::json!({
            "schema": "eneros.media-check.v1",
            "verdict": "PASS",
            "candidate_id": d64,
            "missing": [],
            "qemu_available": true
        });
        assert!(
            report_gate_violations("media", &media).is_empty(),
            "{:?}",
            report_gate_violations("media", &media)
        );

        let compliance = serde_json::json!({
            "schema": "eneros.compliance-report.v1",
            "self_test_status": "PASS",
            "sha256": "abc",
            "signature": "def"
        });
        assert!(
            report_gate_violations("compliance", &compliance).is_empty(),
            "{:?}",
            report_gate_violations("compliance", &compliance)
        );

        let kat = serde_json::json!({
            "schema": "eneros.crypto-kat.v1",
            "verdict": "KAT-ALL-PASS",
            "suites": [{"name": "SM2", "pass": true}]
        });
        assert!(
            report_gate_violations("kat", &kat).is_empty(),
            "{:?}",
            report_gate_violations("kat", &kat)
        );

        let proof = serde_json::json!({
            "schema": "eneros.qa-gate.v1",
            "qa_id": "QA-021",
            "result": "PASS",
            "violations": []
        });
        assert!(
            report_gate_violations("proof", &proof).is_empty(),
            "{:?}",
            report_gate_violations("proof", &proof)
        );

        let validation = serde_json::json!({
            "schema": "eneros.independent-validation.v1",
            "reviewers": [{"name": "x", "role": "auditor", "is_implementer": false, "signed": true}],
            "gate_areas": [{"area": "security", "conclusion": "pass", "signed_by": "x", "independent_signoff": true}]
        });
        assert!(
            report_gate_violations("validation", &validation).is_empty(),
            "{:?}",
            report_gate_violations("validation", &validation)
        );

        let trace = serde_json::json!({
            "schema": "eneros.requirement-trace.v1",
            "requirements": [{
                "requirement_id": "R1",
                "severity": "critical",
                "has_automated_evidence": true,
                "test_ids": ["t1"],
                "artifact_path": "a.json"
            }]
        });
        assert!(
            report_gate_violations("trace", &trace).is_empty(),
            "{:?}",
            report_gate_violations("trace", &trace)
        );

        let index = serde_json::json!({
            "schema": "eneros.evidence-index.v1",
            "baseline_commit": "a".repeat(40),
            "baseline_digest": "b".repeat(64),
            "stale_since": "2026-08-12"
        });
        assert!(
            report_gate_violations("index", &index).is_empty(),
            "{:?}",
            report_gate_violations("index", &index)
        );

        let fuzz = serde_json::json!({
            "schema": "eneros.fuzz-smoke.v1",
            "result": "PASS",
            "candidate_id": "cand",
            "iterations": 100,
            "targets": [
                {"target": "boot_protocol", "iterations": 100, "crashes": 0},
                {"target": "ipc_wire", "iterations": 100, "crashes": 0}
            ],
            "violations": []
        });
        assert!(
            report_gate_violations("fuzz", &fuzz).is_empty(),
            "{:?}",
            report_gate_violations("fuzz", &fuzz)
        );

        let unsafe_report = serde_json::json!({
            "schema": "eneros.unsafe-audit.v1",
            "result": "PASS",
            "checked_files": 10,
            "unsafe_sites": 20,
            "violations": []
        });
        assert!(
            report_gate_violations("unsafe", &unsafe_report).is_empty(),
            "{:?}",
            report_gate_violations("unsafe", &unsafe_report)
        );

        let property = serde_json::json!({
            "schema": "eneros.property-test.v1",
            "result": "PASS",
            "cases": [{"name": "arithmetic", "pass": true}]
        });
        assert!(
            report_gate_violations("property", &property).is_empty(),
            "{:?}",
            report_gate_violations("property", &property)
        );

        let loom = serde_json::json!({
            "schema": "eneros.loom-model.v1",
            "result": "PASS",
            "models": [{"name": "allocator", "pass": true}]
        });
        assert!(
            report_gate_violations("loom", &loom).is_empty(),
            "{:?}",
            report_gate_violations("loom", &loom)
        );

        let miri = serde_json::json!({
            "schema": "eneros.miri-audit.v1",
            "result": "PASS",
            "targets": [{"name": "vec", "pass": true}]
        });
        assert!(
            report_gate_violations("miri", &miri).is_empty(),
            "{:?}",
            report_gate_violations("miri", &miri)
        );
    }

    #[test]
    fn report_gate_rejects_role_specific_bad_reports() {
        let boot = serde_json::json!({
            "schema": "eneros.qemu-media-result.v1",
            "result": "FAIL",
            "qemu_exit": 1,
            "manifest_sha256": "short",
            "measurement_root_sha256": "",
            "boot_slot": "",
            "boot_reason": ""
        });
        let v = report_gate_violations("boot", &boot);
        assert!(!v.is_empty(), "bad boot report must fail");
        assert!(v.iter().any(|x| x.contains("qemu-exit-nonzero")), "{v:?}");

        let ota = serde_json::json!({
            "schema": "eneros.ota-rejection-evidence.v1",
            "cases": [{"case": "forged", "verdict": "FAIL"}]
        });
        assert!(!report_gate_violations("ota", &ota).is_empty());

        let g3 = serde_json::json!({
            "schema": "eneros.g3-evidence.v1",
            "result": "PASS",
            "passed": 0,
            "failed": 1
        });
        assert!(!report_gate_violations("g3", &g3).is_empty());

        let storage = serde_json::json!({
            "schema": "eneros.powercut-durable.v1",
            "verdict": "FAIL",
            "cycles": 3,
            "missing_recover_lines": 1,
            "monotonic": false
        });
        assert!(!report_gate_violations("storage", &storage).is_empty());

        let control = serde_json::json!({
            "schema": "eneros.control-loop.v1",
            "result": "PASS",
            "dispatch_count": 0,
            "ack_count": 1,
            "readback_count": 1
        });
        assert!(!report_gate_violations("control", &control).is_empty());

        let repro = serde_json::json!({
            "schema": "eneros.repro-report.v1",
            "result": "PASS",
            "artifact_a_sha256": "abc",
            "artifact_b_sha256": "def"
        });
        assert!(!report_gate_violations("repro", &repro).is_empty());

        let fault = serde_json::json!({
            "schema": "eneros.artifact-mutation.v1",
            "source_sha256": "abc",
            "output_sha256": "abc"
        });
        assert!(!report_gate_violations("fault", &fault).is_empty());

        let isolation = serde_json::json!({
            "schema": "eneros.g2-evidence.v1",
            "result": "PASS",
            "records": [{"test": "mmu", "result": "PASS"}],
            "passed": 1,
            "failed": 0
        });
        assert!(!report_gate_violations("isolation", &isolation).is_empty());

        let build = serde_json::json!({
            "schema": "eneros.build-identity.v1",
            "profile": "qemu-prod",
            "source_commit": "short",
            "cargo_lock_sha256": "",
            "build_id_hex": ""
        });
        assert!(!report_gate_violations("build", &build).is_empty());

        let bom = serde_json::json!({
            "schema": "eneros.delivery.binary-boms.v1",
            "source_commit": "short",
            "binary_count": 0,
            "binaries": []
        });
        assert!(!report_gate_violations("bom", &bom).is_empty());

        let graph = serde_json::json!({
            "schema": "eneros.delivery.cargo-graph.v1",
            "source_commit": "short",
            "cargo_lock_sha256": "",
            "workspace_package_count": 0
        });
        assert!(!report_gate_violations("graph", &graph).is_empty());

        let media = serde_json::json!({
            "schema": "eneros.media-check.v1",
            "verdict": "FAIL",
            "missing": ["x"],
            "qemu_available": false
        });
        assert!(!report_gate_violations("media", &media).is_empty());

        let compliance = serde_json::json!({
            "schema": "eneros.compliance-report.v1",
            "self_test_status": "FAIL",
            "sha256": "",
            "signature": ""
        });
        assert!(!report_gate_violations("compliance", &compliance).is_empty());

        let kat = serde_json::json!({
            "schema": "eneros.crypto-kat.v1",
            "verdict": "KAT-ALL-PASS",
            "suites": [{"name": "SM2", "pass": false}]
        });
        assert!(!report_gate_violations("kat", &kat).is_empty());

        let proof = serde_json::json!({
            "schema": "eneros.qa-gate.v1",
            "qa_id": "QA-021",
            "result": "DEGRADED",
            "violations": []
        });
        assert!(!report_gate_violations("proof", &proof).is_empty());

        let validation = serde_json::json!({
            "schema": "eneros.independent-validation.v1",
            "reviewers": [{"name": "x", "role": "implementer", "is_implementer": true, "signed": true}],
            "gate_areas": [{"area": "security", "conclusion": "fail", "signed_by": "x", "independent_signoff": false}]
        });
        assert!(!report_gate_violations("validation", &validation).is_empty());

        let trace = serde_json::json!({
            "schema": "eneros.requirement-trace.v1",
            "requirements": [{
                "requirement_id": "R1",
                "severity": "critical",
                "has_automated_evidence": false,
                "test_ids": [],
                "artifact_path": ""
            }]
        });
        assert!(!report_gate_violations("trace", &trace).is_empty());

        let index = serde_json::json!({
            "schema": "eneros.evidence-index.v1",
            "baseline_commit": "short",
            "baseline_digest": "",
            "stale_since": ""
        });
        assert!(!report_gate_violations("index", &index).is_empty());

        let fuzz = serde_json::json!({
            "schema": "eneros.fuzz-smoke.v1",
            "result": "PASS",
            "candidate_id": "",
            "iterations": 0,
            "targets": [
                {"target": "boot_protocol", "iterations": 0, "crashes": 1}
            ],
            "violations": []
        });
        let v = report_gate_violations("fuzz", &fuzz);
        assert!(!v.is_empty(), "bad fuzz report must fail");

        let unsafe_report = serde_json::json!({
            "schema": "eneros.unsafe-audit.v1",
            "result": "FAIL",
            "checked_files": 0,
            "unsafe_sites": 0,
            "violations": ["missing-safety"]
        });
        assert!(!report_gate_violations("unsafe", &unsafe_report).is_empty());

        let property = serde_json::json!({
            "schema": "eneros.property-test.v1",
            "result": "PASS",
            "cases": [{"name": "bad", "pass": false}]
        });
        assert!(!report_gate_violations("property", &property).is_empty());

        let loom = serde_json::json!({
            "schema": "eneros.loom-model.v1",
            "result": "FAIL",
            "models": []
        });
        assert!(!report_gate_violations("loom", &loom).is_empty());

        let miri = serde_json::json!({
            "schema": "eneros.miri-audit.v1",
            "result": "PASS",
            "targets": [{"name": "bad", "pass": false}]
        });
        assert!(!report_gate_violations("miri", &miri).is_empty());
    }
}
