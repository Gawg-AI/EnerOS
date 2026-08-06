use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{invalid, read_json, DynResult};

const POLICY_SCHEMA: &str = "eneros.release-key-policy.v1";

#[derive(Debug, Deserialize)]
struct KeyPolicy {
    schema: String,
    private_keys_in_repository: bool,
    roles: Vec<KeyRole>,
    known_test_public_key_sha256: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct KeyRole {
    role: String,
    public_certificate_sha256: Option<String>,
    x509_fingerprint_sha256: Option<String>,
    public_key_sha256: Option<String>,
    key_id_hex: Option<String>,
}

#[derive(Debug, Serialize)]
struct TrustReport {
    schema: &'static str,
    result: &'static str,
    release_public_key_sha256: String,
    release_key_id_hex: String,
    uefi_certificate_pem_sha256: String,
    uefi_certificate_der_sha256: String,
    repository_private_key_candidates: Vec<String>,
    policy_binding: &'static str,
    test_trust_rejection: &'static str,
    violations: Vec<String>,
}

pub(super) fn command_audit_trust(root: &Path, args: &[String]) -> DynResult<()> {
    let mut public_key = None;
    let mut certificate = None;
    let mut fault = None;
    let mut index = 0usize;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| invalid(format!("{flag} requires a value")))?;
        match flag.as_str() {
            "--release-public-key" => public_key = Some(PathBuf::from(value)),
            "--uefi-cert" => certificate = Some(value.clone()),
            "--fault" => fault = Some(value.clone()),
            _ => return Err(invalid(format!("unknown audit trust option '{flag}'"))),
        }
        index += 2;
    }
    let public_key = public_key.ok_or_else(|| invalid("--release-public-key is mandatory"))?;
    let certificate = certificate.ok_or_else(|| invalid("--uefi-cert is mandatory"))?;
    let public_key = target_input(root, &public_key, "release public key")?;
    let mut key_bytes = fs::read(public_key)?;
    let mut candidates = scan_private_keys(root)?;
    match fault.as_deref() {
        None => {}
        Some("known-test-key") => key_bytes = vec![0x42; 32],
        Some("wrong-cert-digest") => {}
        Some("private-key-candidate") => candidates.push("fault-injected/private.seed".to_owned()),
        Some(other) => return Err(invalid(format!("unknown trust fault '{other}'"))),
    }
    let policy = load_policy(root)?;
    let key_sha = hex(&Sha256::digest(&key_bytes));
    let key_id = key_sha
        .get(..32)
        .ok_or_else(|| invalid("release public key digest is truncated"))?
        .to_owned();
    let cert_sha = if fault.as_deref() == Some("wrong-cert-digest") {
        "00".repeat(32)
    } else {
        certificate_sha256(&certificate)?
    };
    let release_role = role(&policy, "release-manifest-release-candidate")?;
    let uefi_role = role(&policy, "uefi-db-release-candidate")?;
    let mut violations = Vec::new();
    if key_bytes.len() != 32
        || release_role.public_key_sha256.as_deref() != Some(key_sha.as_str())
        || release_role.key_id_hex.as_deref() != Some(key_id.as_str())
    {
        violations.push("release-public-key-policy-mismatch".to_owned());
    }
    if policy
        .known_test_public_key_sha256
        .iter()
        .any(|digest| digest == &key_sha)
    {
        violations.push("known-test-public-key".to_owned());
    }
    if uefi_role.public_certificate_sha256.as_deref() != Some(cert_sha.as_str()) {
        violations.push("uefi-certificate-policy-mismatch".to_owned());
    }
    if !candidates.is_empty() || policy.private_keys_in_repository {
        violations.push("repository-private-key".to_owned());
    }
    let report = TrustReport {
        schema: "eneros.trust-audit.v1",
        result: if violations.is_empty() {
            "PASS"
        } else {
            "FAIL"
        },
        release_public_key_sha256: key_sha,
        release_key_id_hex: key_id,
        uefi_certificate_pem_sha256: cert_sha,
        uefi_certificate_der_sha256: uefi_role
            .x509_fingerprint_sha256
            .clone()
            .ok_or_else(|| invalid("UEFI role lacks DER fingerprint"))?,
        repository_private_key_candidates: candidates,
        policy_binding: if violations
            .iter()
            .any(|item| item.contains("policy-mismatch"))
        {
            "FAIL"
        } else {
            "PASS"
        },
        test_trust_rejection: if violations
            .iter()
            .any(|item| item == "known-test-public-key")
        {
            "FAIL"
        } else {
            "PASS"
        },
        violations,
    };
    let directory = root.join("target/eneros/evidence/SEC-005");
    fs::create_dir_all(&directory)?;
    let output = if fault.is_some() {
        directory.join(format!(
            "trust-report-fault-{}.json",
            fault.as_deref().unwrap_or("unknown")
        ))
    } else {
        directory.join("trust-report.json")
    };
    fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report.result != "PASS" {
        return Err(invalid("release trust audit failed"));
    }
    Ok(())
}

pub(super) fn enforce_release_public_key(root: &Path, bytes: &[u8]) -> DynResult<()> {
    let policy = load_policy(root)?;
    let digest = hex(&Sha256::digest(bytes));
    if bytes.len() != 32
        || policy
            .known_test_public_key_sha256
            .iter()
            .any(|known| known == &digest)
        || role(&policy, "release-manifest-release-candidate")?
            .public_key_sha256
            .as_deref()
            != Some(digest.as_str())
    {
        return Err(invalid(
            "release public key is not the production-class policy root",
        ));
    }
    Ok(())
}

pub(super) fn enforce_uefi_certificate(root: &Path, certificate: &str) -> DynResult<()> {
    let policy = load_policy(root)?;
    let digest = certificate_sha256(certificate)?;
    if role(&policy, "uefi-db-release-candidate")?
        .public_certificate_sha256
        .as_deref()
        != Some(digest.as_str())
    {
        return Err(invalid(
            "UEFI certificate is not the production-class policy root",
        ));
    }
    Ok(())
}

fn load_policy(root: &Path) -> DynResult<KeyPolicy> {
    let policy: KeyPolicy = read_json(&root.join("product/security/key-policy.json"))?;
    if policy.schema != POLICY_SCHEMA {
        return Err(invalid("unsupported release key policy schema"));
    }
    Ok(policy)
}

fn role<'a>(policy: &'a KeyPolicy, name: &str) -> DynResult<&'a KeyRole> {
    policy
        .roles
        .iter()
        .find(|role| role.role == name)
        .ok_or_else(|| invalid(format!("release key policy lacks role '{name}'")))
}

fn certificate_sha256(certificate: &str) -> DynResult<String> {
    if cfg!(windows) {
        let output = Command::new("wsl.exe")
            .args(["--exec", "sha256sum", "--", certificate])
            .output()?;
        if !output.status.success() {
            return Err(invalid(format!(
                "UEFI certificate hash failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let digest = String::from_utf8(output.stdout)?
            .split_whitespace()
            .next()
            .ok_or_else(|| invalid("sha256sum returned no certificate digest"))?
            .to_ascii_lowercase();
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid("sha256sum returned a malformed certificate digest"));
        }
        Ok(digest)
    } else {
        Ok(hex(&Sha256::digest(fs::read(certificate)?)))
    }
}

fn scan_private_keys(root: &Path) -> DynResult<Vec<String>> {
    let mut candidates = Vec::new();
    scan_directory(root, root, &mut candidates)?;
    candidates.sort();
    candidates.dedup();
    Ok(candidates)
}

fn scan_directory(root: &Path, directory: &Path, candidates: &mut Vec<String>) -> DynResult<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if entry.file_type()?.is_dir() {
            if name == ".git" || name == "target" {
                continue;
            }
            scan_directory(root, &path, candidates)?;
            continue;
        }
        if !entry.file_type()?.is_file() {
            continue;
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let suspicious_extension =
            matches!(extension.as_str(), "key" | "p12" | "pfx" | "seed" | "pk8");
        let bytes = fs::read(&path)?;
        let suspicious_pem = [
            "PRIVATE KEY-----",
            "RSA PRIVATE KEY-----",
            "EC PRIVATE KEY-----",
            "OPENSSH PRIVATE KEY-----",
            "ENCRYPTED PRIVATE KEY-----",
        ]
        .iter()
        .map(|suffix| ["-----BEGIN ", suffix].concat())
        .any(|needle| {
            bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        });
        if suspicious_extension || suspicious_pem {
            candidates.push(
                path.strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn target_input(root: &Path, path: &Path, label: &str) -> DynResult<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
    .canonicalize()?;
    let target = root.join("target").canonicalize()?;
    if !candidate.starts_with(&target) || !candidate.is_file() {
        return Err(invalid(format!(
            "{label} must be an existing file below target"
        )));
    }
    Ok(candidate)
}

fn hex(bytes: &[u8]) -> String {
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
    fn known_test_public_fingerprint_is_stable() {
        assert_eq!(
            hex(&Sha256::digest([0x42; 32])),
            "425ed4e4a36b30ea21b90e21c712c649e8214c29b7eaf68089d1039c6e55384c"
        );
    }
}
