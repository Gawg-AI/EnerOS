//! 审计日志（防篡改 + HMAC 签名 + 链式哈希 + 独立存储 + 轮转 + 365 天保留）
//!
//! 审计日志与普通日志分离，存储在独立目录，每条日志带 HMAC-SHA256 签名
//! 防止篡改。采用链式哈希（每条记录的 prev_hash 指向前一条的 SHA256），
//! 检测删除/插入攻击。保留 365 天，支持按大小轮转和查询 API。

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
#[cfg(target_os = "linux")]
use sha2::Digest;
#[cfg(target_os = "linux")]
use std::io::Write;
use std::path::{Path, PathBuf};

type HmacSha256 = Hmac<Sha256>;

/// 审计日志错误
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("hmac error: {0}")]
    Hmac(String),
    #[error("unsupported platform")]
    UnsupportedPlatform,
}

/// 审计操作类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Login,
    Logout,
    ConfigChange,
    AgentControl,
    PermissionChange,
    Update,
    Emergency,
    CommandExec,
    DataAccess,
    Other,
}

impl AuditAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditAction::Login => "login",
            AuditAction::Logout => "logout",
            AuditAction::ConfigChange => "config_change",
            AuditAction::AgentControl => "agent_control",
            AuditAction::PermissionChange => "permission_change",
            AuditAction::Update => "update",
            AuditAction::Emergency => "emergency",
            AuditAction::CommandExec => "command_exec",
            AuditAction::DataAccess => "data_access",
            AuditAction::Other => "other",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "login" => Some(Self::Login),
            "logout" => Some(Self::Logout),
            "config_change" => Some(Self::ConfigChange),
            "agent_control" => Some(Self::AgentControl),
            "permission_change" => Some(Self::PermissionChange),
            "update" => Some(Self::Update),
            "emergency" => Some(Self::Emergency),
            "command_exec" => Some(Self::CommandExec),
            "data_access" => Some(Self::DataAccess),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

/// 审计结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditResult {
    Success,
    Failure,
    Denied,
}

/// 审计日志条目（带 HMAC 签名 + 链式哈希）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub action: AuditAction,
    pub actor: String,
    pub target: String,
    pub result: AuditResult,
    #[serde(default)]
    pub source_ip: Option<String>,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub prev_hash: String,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub signature: String,
}

fn default_schema_version() -> u32 {
    1
}

impl AuditEntry {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        seq: u64,
        action: AuditAction,
        actor: impl Into<String>,
        target: impl Into<String>,
        result: AuditResult,
        source_ip: Option<&str>,
        detail: &str,
        prev_hash: &str,
        secret: &[u8],
    ) -> Result<Self, AuditError> {
        Self::new_with_timestamp(
            seq, Utc::now(), action, actor, target, result, source_ip, detail, prev_hash, secret,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_timestamp(
        seq: u64,
        timestamp: DateTime<Utc>,
        action: AuditAction,
        actor: impl Into<String>,
        target: impl Into<String>,
        result: AuditResult,
        source_ip: Option<&str>,
        detail: &str,
        prev_hash: &str,
        secret: &[u8],
    ) -> Result<Self, AuditError> {
        if secret.is_empty() {
            return Err(AuditError::Config("hmac secret must not be empty".into()));
        }
        let entry = Self {
            seq,
            timestamp,
            action,
            actor: actor.into(),
            target: target.into(),
            result,
            source_ip: source_ip.map(Into::into),
            detail: detail.to_string(),
            prev_hash: prev_hash.to_string(),
            schema_version: 1,
            signature: String::new(),
        };
        let mut entry = entry;
        entry.signature = entry.compute_signature(secret)?;
        Ok(entry)
    }

    fn signing_payload(&self) -> String {
        format!(
            "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
            self.seq,
            self.timestamp.timestamp_micros(),
            self.action.as_str(),
            self.actor,
            self.target,
            self.result_str(),
            self.source_ip.as_deref().unwrap_or(""),
            self.detail,
            self.prev_hash,
            self.schema_version,
        )
    }

    fn result_str(&self) -> &'static str {
        match self.result {
            AuditResult::Success => "success",
            AuditResult::Failure => "failure",
            AuditResult::Denied => "denied",
        }
    }

    fn compute_signature(&self, secret: &[u8]) -> Result<String, AuditError> {
        let mut mac =
            HmacSha256::new_from_slice(secret).map_err(|e| AuditError::Hmac(e.to_string()))?;
        mac.update(self.signing_payload().as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes().as_slice()))
    }

    pub fn verify(&self, secret: &[u8]) -> Result<bool, AuditError> {
        let mut mac =
            HmacSha256::new_from_slice(secret).map_err(|e| AuditError::Hmac(e.to_string()))?;
        mac.update(self.signing_payload().as_bytes());
        let expected_bytes =
            hex_decode(&self.signature).map_err(|e| AuditError::Config(e.to_string()))?;
        Ok(mac.verify_slice(&expected_bytes).is_ok())
    }

    pub fn to_jsonl(&self) -> Result<String, AuditError> {
        serde_json::to_string(self).map_err(|e| AuditError::Config(e.to_string()))
    }

    pub fn from_jsonl(line: &str) -> Result<Self, AuditError> {
        serde_json::from_str(line).map_err(|e| AuditError::Config(e.to_string()))
    }
}

/// 完整性违规类型
#[derive(Debug, Clone, PartialEq)]
pub enum ViolationType {
    SignatureMismatch,
    SeqGap { expected: u64, actual: u64 },
    HashChainBroken { expected: String, actual: String },
    Unparseable,
}

/// 完整性违规记录
#[derive(Debug, Clone)]
pub struct IntegrityViolation {
    pub seq: u64,
    pub line_number: usize,
    pub violation_type: ViolationType,
    pub detail: String,
}

mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(target_os = "linux")]
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize().as_slice())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    #[serde(default = "default_audit_dir")]
    pub audit_dir: PathBuf,
    pub hmac_secret: String,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_max_size_bytes")]
    pub max_size_bytes: u64,
    #[serde(default)]
    pub forward_enabled: bool,
    #[serde(default)]
    pub forward_target: Option<String>,
}

fn default_audit_dir() -> PathBuf {
    PathBuf::from("/var/log/eneros/audit")
}
fn default_retention_days() -> u32 {
    365
}
fn default_max_size_bytes() -> u64 {
    100 * 1024 * 1024
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            audit_dir: default_audit_dir(),
            hmac_secret: String::new(),
            retention_days: 365,
            max_size_bytes: default_max_size_bytes(),
            forward_enabled: false,
            forward_target: None,
        }
    }
}

#[allow(dead_code)]
struct AuditState {
    seq_counter: u64,
    last_hash: String,
    initialized: bool,
}

pub struct AuditLogger {
    config: AuditConfig,
    #[allow(dead_code)]
    secret: Vec<u8>,
    #[allow(dead_code)]
    state: parking_lot::Mutex<AuditState>,
}

impl AuditLogger {
    pub fn load(path: &Path) -> Result<Self, AuditError> {
        let content = std::fs::read_to_string(path)?;
        let config: AuditConfig =
            toml::from_str(&content).map_err(|e| AuditError::Config(e.to_string()))?;
        Self::new(config)
    }

    pub fn new(config: AuditConfig) -> Result<Self, AuditError> {
        if config.hmac_secret.is_empty() {
            return Err(AuditError::Config(
                "hmac_secret must not be empty — configure a strong secret (>=32 bytes hex)".into(),
            ));
        }
        let secret = hex_decode(&config.hmac_secret)
            .map_err(|e| AuditError::Config(format!("hmac_secret decode: {e}")))?;
        Ok(Self {
            config,
            secret,
            state: parking_lot::Mutex::new(AuditState {
                seq_counter: 0,
                last_hash: String::new(),
                initialized: false,
            }),
        })
    }

    pub fn config(&self) -> &AuditConfig {
        &self.config
    }

    #[cfg(target_os = "linux")]
    pub fn log(
        &self,
        action: AuditAction,
        actor: &str,
        target: &str,
        result: AuditResult,
        source_ip: Option<&str>,
        detail: &str,
    ) -> Result<AuditEntry, AuditError> {
        let mut state = self.state.lock();

        if !state.initialized {
            let (max_seq, last_hash) = self.recover_state()?;
            state.seq_counter = max_seq;
            state.last_hash = last_hash;
            state.initialized = true;
        }

        state.seq_counter += 1;
        let seq = state.seq_counter;
        let prev_hash = state.last_hash.clone();

        let entry = AuditEntry::new(
            seq, action, actor, target, result, source_ip, detail, &prev_hash, &self.secret,
        )?;

        let log_file = self.config.audit_dir.join("audit.log");
        if let Some(parent) = log_file.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let jsonl = entry.to_jsonl()?;
        {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_file)?;
            writeln!(file, "{}", jsonl)?;
            file.sync_all()?;
        }

        state.last_hash = sha256_hex(jsonl.as_bytes());

        let file_size = std::fs::metadata(&log_file).map(|m| m.len()).unwrap_or(0);
        if file_size >= self.config.max_size_bytes {
            let ts = Utc::now().format("%Y%m%d_%H%M%S").to_string();
            let rotated = self.config.audit_dir.join(format!("audit.log.{}", ts));
            let _ = std::fs::rename(&log_file, &rotated);
        }

        if let Err(e) = self.cleanup_old_files() {
            tracing::warn!("审计日志清理失败: {e}");
        }

        Ok(entry)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn log(
        &self,
        _action: AuditAction,
        _actor: &str,
        _target: &str,
        _result: AuditResult,
        _source_ip: Option<&str>,
        _detail: &str,
    ) -> Result<AuditEntry, AuditError> {
        Err(AuditError::UnsupportedPlatform)
    }

    #[cfg(target_os = "linux")]
    fn recover_state(&self) -> Result<(u64, String), AuditError> {
        let log_file = self.config.audit_dir.join("audit.log");
        if !log_file.exists() {
            return Ok((0, String::new()));
        }
        let content = std::fs::read_to_string(&log_file)?;
        let mut max_seq = 0u64;
        let mut last_hash = String::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = AuditEntry::from_jsonl(line) {
                if entry.seq > max_seq {
                    max_seq = entry.seq;
                }
                last_hash = sha256_hex(line.as_bytes());
            }
        }
        Ok((max_seq, last_hash))
    }

    #[cfg(target_os = "linux")]
    fn recover_max_seq(&self) -> Result<u64, AuditError> {
        Ok(self.recover_state()?.0)
    }

    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_arguments)]
    pub fn query(
        &self,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        action_filter: Option<&AuditAction>,
        actor_filter: Option<&str>,
        result_filter: Option<&AuditResult>,
        target_filter: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<AuditEntry>, AuditError> {
        let log_file = self.config.audit_dir.join("audit.log");
        if !log_file.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&log_file)?;
        let mut results = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = AuditEntry::from_jsonl(line) {
                if let Some(s) = start {
                    if entry.timestamp < s {
                        continue;
                    }
                }
                if let Some(e) = end {
                    if entry.timestamp > e {
                        continue;
                    }
                }
                if let Some(a) = action_filter {
                    if &entry.action != a {
                        continue;
                    }
                }
                if let Some(af) = actor_filter {
                    if entry.actor != af {
                        continue;
                    }
                }
                if let Some(r) = result_filter {
                    if &entry.result != r {
                        continue;
                    }
                }
                if let Some(t) = target_filter {
                    if entry.target != t {
                        continue;
                    }
                }
                results.push(entry);
                if let Some(lim) = limit {
                    if results.len() >= lim {
                        break;
                    }
                }
            }
        }

        Ok(results)
    }

    #[cfg(not(target_os = "linux"))]
    #[allow(clippy::too_many_arguments)]
    pub fn query(
        &self,
        _start: Option<DateTime<Utc>>,
        _end: Option<DateTime<Utc>>,
        _action_filter: Option<&AuditAction>,
        _actor_filter: Option<&str>,
        _result_filter: Option<&AuditResult>,
        _target_filter: Option<&str>,
        _limit: Option<usize>,
    ) -> Result<Vec<AuditEntry>, AuditError> {
        Err(AuditError::UnsupportedPlatform)
    }

    #[cfg(target_os = "linux")]
    pub fn verify_integrity(&self) -> Result<Vec<IntegrityViolation>, AuditError> {
        let log_file = self.config.audit_dir.join("audit.log");
        if !log_file.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&log_file)?;
        let mut violations = Vec::new();
        let mut expected_seq: u64 = 1;
        let mut prev_line_hash = String::new();

        for (idx, line) in content.lines().enumerate() {
            let line_number = idx + 1;
            if line.trim().is_empty() {
                continue;
            }
            match AuditEntry::from_jsonl(line) {
                Ok(entry) => {
                    if entry.seq != expected_seq {
                        violations.push(IntegrityViolation {
                            seq: entry.seq,
                            line_number,
                            violation_type: ViolationType::SeqGap {
                                expected: expected_seq,
                                actual: entry.seq,
                            },
                            detail: format!("expected seq {}, got {}", expected_seq, entry.seq),
                        });
                    }

                    if entry.prev_hash != prev_line_hash {
                        violations.push(IntegrityViolation {
                            seq: entry.seq,
                            line_number,
                            violation_type: ViolationType::HashChainBroken {
                                expected: prev_line_hash.clone(),
                                actual: entry.prev_hash.clone(),
                            },
                            detail: format!(
                                "prev_hash mismatch: expected {}, got {}",
                                prev_line_hash, entry.prev_hash
                            ),
                        });
                    }

                    if !entry.verify(&self.secret)? {
                        violations.push(IntegrityViolation {
                            seq: entry.seq,
                            line_number,
                            violation_type: ViolationType::SignatureMismatch,
                            detail: format!(
                                "HMAC signature verification failed for seq {}",
                                entry.seq
                            ),
                        });
                    }

                    prev_line_hash = sha256_hex(line.as_bytes());
                    expected_seq = entry.seq + 1;
                }
                Err(_) => {
                    violations.push(IntegrityViolation {
                        seq: 0,
                        line_number,
                        violation_type: ViolationType::Unparseable,
                        detail: format!("line {} cannot be parsed", line_number),
                    });
                }
            }
        }

        Ok(violations)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn verify_integrity(&self) -> Result<Vec<IntegrityViolation>, AuditError> {
        Err(AuditError::UnsupportedPlatform)
    }

    #[cfg(target_os = "linux")]
    fn cleanup_old_files(&self) -> Result<(), AuditError> {
        let parent = self.config.audit_dir.as_path();
        let retention = self.config.retention_days;

        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();

            if !name.starts_with("audit.log.") {
                continue;
            }

            let ts_part = name.strip_prefix("audit.log.").unwrap_or("");
            if ts_part.len() >= 15 {
                if let Ok(file_time) =
                    chrono::NaiveDateTime::parse_from_str(&ts_part[..15], "%Y%m%d_%H%M%S")
                {
                    let file_age = Utc::now().naive_utc() - file_time;
                    if file_age.num_days() >= retention as i64 {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }

        Ok(())
    }

    pub fn flush(&self) -> Result<(), AuditError> {
        Ok(())
    }

    pub fn reload(path: &Path) -> Result<Self, AuditError> {
        Self::load(path)
    }
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd length".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_secret() -> Vec<u8> {
        b"test-audit-secret-key-32bytes!!".to_vec()
    }

    #[cfg(target_os = "linux")]
    fn test_secret_hex() -> String {
        hex::encode(b"test-secret-key-32-bytes-long!!")
    }

    #[cfg(target_os = "linux")]
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(target_os = "linux")]
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[cfg(target_os = "linux")]
    fn make_temp_dir() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir()
            .join(format!("eneros-audit-test-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(target_os = "linux")]
    fn make_test_config(audit_dir: &Path) -> AuditConfig {
        AuditConfig {
            audit_dir: audit_dir.to_path_buf(),
            hmac_secret: test_secret_hex(),
            max_size_bytes: 100 * 1024 * 1024,
            ..Default::default()
        }
    }

    #[test]
    fn test_audit_entry_new_and_verify() {
        let secret = test_secret();
        let entry = AuditEntry::new(
            1,
            AuditAction::Login,
            "admin",
            "system",
            AuditResult::Success,
            None,
            "",
            "",
            &secret,
        )
        .unwrap();

        assert!(!entry.signature.is_empty());
        assert!(entry.verify(&secret).unwrap());
    }

    #[test]
    fn test_audit_entry_tamper_detection() {
        let secret = test_secret();
        let mut entry = AuditEntry::new(
            1,
            AuditAction::ConfigChange,
            "admin",
            "/etc/eneros/network.toml",
            AuditResult::Success,
            None,
            "",
            "",
            &secret,
        )
        .unwrap();

        entry.actor = "hacker".to_string();
        assert!(!entry.verify(&secret).unwrap());
    }

    #[test]
    fn test_audit_entry_tamper_source_ip_detail() {
        let secret = test_secret();
        let mut entry = AuditEntry::new(
            5,
            AuditAction::AgentControl,
            "admin",
            "agent://powerflow",
            AuditResult::Success,
            Some("192.168.1.50"),
            "agent started",
            "",
            &secret,
        )
        .unwrap();

        assert!(entry.verify(&secret).unwrap());
        entry.source_ip = Some("10.0.0.99".to_string());
        assert!(!entry.verify(&secret).unwrap());

        let mut entry2 = AuditEntry::new(
            5,
            AuditAction::AgentControl,
            "admin",
            "agent://powerflow",
            AuditResult::Success,
            Some("192.168.1.50"),
            "agent started",
            "",
            &secret,
        )
        .unwrap();
        entry2.detail = "tampered".to_string();
        assert!(!entry2.verify(&secret).unwrap());
    }

    #[test]
    fn test_audit_entry_jsonl_roundtrip() {
        let secret = test_secret();
        let entry = AuditEntry::new(
            42,
            AuditAction::AgentControl,
            "agent-001",
            "agent://powerflow",
            AuditResult::Success,
            Some("192.168.1.50"),
            "agent started",
            "abc123",
            &secret,
        )
        .unwrap();

        let jsonl = entry.to_jsonl().unwrap();
        let decoded = AuditEntry::from_jsonl(&jsonl).unwrap();

        assert_eq!(decoded.seq, 42);
        assert_eq!(decoded.actor, "agent-001");
        assert_eq!(decoded.target, "agent://powerflow");
        assert_eq!(decoded.source_ip, Some("192.168.1.50".to_string()));
        assert_eq!(decoded.detail, "agent started");
        assert_eq!(decoded.prev_hash, "abc123");
        assert_eq!(decoded.schema_version, 1);
        assert!(decoded.verify(&secret).unwrap());
    }

    #[test]
    fn test_audit_actions() {
        assert_eq!(AuditAction::Login.as_str(), "login");
        assert_eq!(AuditAction::ConfigChange.as_str(), "config_change");
        assert_eq!(AuditAction::Emergency.as_str(), "emergency");
    }

    #[test]
    fn test_audit_config_default() {
        let config = AuditConfig::default();
        assert_eq!(config.audit_dir, PathBuf::from("/var/log/eneros/audit"));
        assert_eq!(config.retention_days, 365);
        assert_eq!(config.max_size_bytes, 100 * 1024 * 1024);
        assert!(!config.forward_enabled);
    }

    #[test]
    fn test_audit_config_parse() {
        let toml_str = r#"
hmac_secret = "deadbeef"
retention_days = 730
max_size_bytes = 524288000
forward_enabled = true
forward_target = "10.0.0.1:6514"
"#;
        let config: AuditConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hmac_secret, "deadbeef");
        assert_eq!(config.retention_days, 730);
        assert_eq!(config.max_size_bytes, 524288000);
        assert!(config.forward_enabled);
        assert_eq!(config.forward_target, Some("10.0.0.1:6514".to_string()));
    }

    #[test]
    fn test_hex_encode_decode() {
        let original = b"hello world";
        let encoded = hex::encode(original);
        let decoded = hex_decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_hex_decode_odd_length() {
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn test_signing_payload_consistency() {
        let secret = test_secret();
        let entry = AuditEntry::new(
            1,
            AuditAction::Login,
            "admin",
            "system",
            AuditResult::Success,
            None,
            "",
            "",
            &secret,
        )
        .unwrap();

        let entry2 = AuditEntry::new(
            1,
            AuditAction::Login,
            "admin",
            "system",
            AuditResult::Success,
            None,
            "",
            "",
            &secret,
        )
        .unwrap();

        assert_eq!(entry.seq, entry2.seq);
        assert_eq!(entry.actor, entry2.actor);
    }

    #[test]
    fn test_audit_logger_new() {
        let config = AuditConfig {
            hmac_secret: hex::encode(b"my-secret-key-32-bytes-long!!!"),
            ..Default::default()
        };
        let logger = AuditLogger::new(config).unwrap();
        let state = logger.state.lock();
        assert_eq!(state.seq_counter, 0);
        assert!(!state.initialized);
        drop(state);
        assert!(!logger.secret.is_empty());
    }

    #[test]
    fn test_audit_logger_empty_secret_rejected() {
        let config = AuditConfig::default();
        let result = AuditLogger::new(config);
        assert!(matches!(result, Err(AuditError::Config(_))));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_log_unsupported() {
        let config = AuditConfig {
            hmac_secret: hex::encode(b"my-secret-key-32-bytes-long!!!"),
            ..Default::default()
        };
        let logger = AuditLogger::new(config).unwrap();
        let result = logger.log(
            AuditAction::Login,
            "admin",
            "system",
            AuditResult::Success,
            None,
            "",
        );
        assert!(matches!(result, Err(AuditError::UnsupportedPlatform)));
    }

    #[test]
    fn test_command_exec_action() {
        assert_eq!(AuditAction::CommandExec.as_str(), "command_exec");
        assert_eq!(AuditAction::DataAccess.as_str(), "data_access");
        assert_eq!(
            AuditAction::from_str("command_exec"),
            Some(AuditAction::CommandExec)
        );
        assert_eq!(
            AuditAction::from_str("data_access"),
            Some(AuditAction::DataAccess)
        );
        assert_eq!(AuditAction::from_str("invalid"), None);
    }

    #[test]
    fn test_signature_no_collision() {
        let secret = test_secret();
        let ts = Utc::now();
        let e1 = AuditEntry::new_with_timestamp(
            1,
            ts,
            AuditAction::Login,
            "a|b",
            "c",
            AuditResult::Success,
            None,
            "",
            "",
            &secret,
        )
        .unwrap();
        let e2 = AuditEntry::new_with_timestamp(
            1,
            ts,
            AuditAction::Login,
            "a",
            "b|c",
            AuditResult::Success,
            None,
            "",
            "",
            &secret,
        )
        .unwrap();
        assert_ne!(e1.signature, e2.signature);
    }

    #[test]
    fn test_new_with_timestamp() {
        let secret = test_secret();
        let ts = Utc::now();
        let entry = AuditEntry::new_with_timestamp(
            10,
            ts,
            AuditAction::Login,
            "admin",
            "system",
            AuditResult::Success,
            None,
            "",
            "",
            &secret,
        )
        .unwrap();
        assert_eq!(entry.timestamp, ts);
        assert_eq!(entry.seq, 10);
        assert!(entry.verify(&secret).unwrap());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_log_with_source_ip_and_detail() {
        let dir = make_temp_dir();
        let config = make_test_config(&dir);
        let logger = AuditLogger::new(config).unwrap();

        let entry = logger
            .log(
                AuditAction::Login,
                "admin",
                "system",
                AuditResult::Success,
                Some("192.168.1.100"),
                "login from console",
            )
            .unwrap();

        assert_eq!(entry.source_ip, Some("192.168.1.100".to_string()));
        assert_eq!(entry.detail, "login from console");
        assert_eq!(entry.seq, 1);
        assert_eq!(entry.prev_hash, "");
        assert_eq!(entry.schema_version, 1);

        let log_file = dir.join("audit.log");
        let content = std::fs::read_to_string(&log_file).unwrap();
        let parsed = AuditEntry::from_jsonl(content.lines().next().unwrap()).unwrap();
        assert_eq!(parsed.source_ip, Some("192.168.1.100".to_string()));
        assert_eq!(parsed.detail, "login from console");
        assert!(parsed.verify(&test_secret()).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_audit_log_rotation() {
        let dir = make_temp_dir();
        let config = AuditConfig {
            audit_dir: dir.clone(),
            hmac_secret: test_secret_hex(),
            max_size_bytes: 500,
            ..Default::default()
        };
        let logger = AuditLogger::new(config).unwrap();

        for i in 0..10 {
            logger
                .log(
                    AuditAction::Login,
                    "admin",
                    "system",
                    AuditResult::Success,
                    None,
                    &format!("rotation test entry {}", i),
                )
                .unwrap();
        }

        let has_rotated = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("audit.log."));
        assert!(has_rotated, "rotation should have created archive files");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_chain_hash_tamper_detection() {
        let dir = make_temp_dir();
        let config = make_test_config(&dir);
        let logger = AuditLogger::new(config).unwrap();

        logger
            .log(AuditAction::Login, "admin", "system", AuditResult::Success, None, "e1")
            .unwrap();
        logger
            .log(AuditAction::Login, "admin", "system", AuditResult::Success, None, "e2")
            .unwrap();
        logger
            .log(AuditAction::Login, "admin", "system", AuditResult::Success, None, "e3")
            .unwrap();

        let log_file = dir.join("audit.log");
        let content = std::fs::read_to_string(&log_file).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        let tampered = format!("{}\n{}\n", lines[0], lines[2]);
        std::fs::write(&log_file, tampered).unwrap();

        let violations = logger.verify_integrity().unwrap();
        assert!(
            !violations.is_empty(),
            "should detect tampering after deletion"
        );

        let has_seq_gap = violations.iter().any(|v| {
            matches!(
                v.violation_type,
                ViolationType::SeqGap {
                    expected: 2,
                    actual: 3
                }
            )
        });
        let has_chain_broken = violations
            .iter()
            .any(|v| matches!(v.violation_type, ViolationType::HashChainBroken { .. }));
        assert!(has_seq_gap, "should detect seq gap");
        assert!(has_chain_broken, "should detect broken hash chain");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_recover_max_seq_error() {
        let dir = make_temp_dir();
        let log_file = dir.join("audit.log");
        std::fs::write(&log_file, b"\xff\xfe invalid bytes").unwrap();

        let config = make_test_config(&dir);
        let logger = AuditLogger::new(config).unwrap();

        let result = logger.recover_max_seq();
        assert!(result.is_err(), "corrupted file should return error");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_query_with_filters() {
        let dir = make_temp_dir();
        let config = make_test_config(&dir);
        let logger = AuditLogger::new(config).unwrap();

        logger
            .log(AuditAction::Login, "admin", "system", AuditResult::Success, None, "e1")
            .unwrap();
        logger
            .log(AuditAction::ConfigChange, "admin", "/etc/config", AuditResult::Success, None, "e2")
            .unwrap();
        logger
            .log(AuditAction::Login, "user2", "system", AuditResult::Failure, None, "e3")
            .unwrap();

        let results = logger
            .query(None, None, Some(&AuditAction::Login), None, None, None, None)
            .unwrap();
        assert_eq!(results.len(), 2);

        let results = logger
            .query(None, None, None, Some("admin"), None, None, None)
            .unwrap();
        assert_eq!(results.len(), 2);

        let results = logger
            .query(None, None, None, None, Some(&AuditResult::Failure), None, None)
            .unwrap();
        assert_eq!(results.len(), 1);

        let results = logger
            .query(None, None, None, None, None, Some("system"), None)
            .unwrap();
        assert_eq!(results.len(), 2);

        let results = logger
            .query(None, None, None, None, None, None, Some(1))
            .unwrap();
        assert_eq!(results.len(), 1);

        let results = logger
            .query(
                None,
                None,
                Some(&AuditAction::Login),
                Some("admin"),
                Some(&AuditResult::Success),
                Some("system"),
                None,
            )
            .unwrap();
        assert_eq!(results.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_concurrent_log_safety() {
        use std::sync::Arc;
        use std::thread;

        let dir = make_temp_dir();
        let config = make_test_config(&dir);
        let logger = Arc::new(AuditLogger::new(config).unwrap());

        let mut handles = Vec::new();
        for i in 0..4 {
            let logger = Arc::clone(&logger);
            handles.push(thread::spawn(move || {
                for j in 0..10 {
                    let _ = logger.log(
                        AuditAction::Login,
                        &format!("user{}", i),
                        "system",
                        AuditResult::Success,
                        None,
                        &format!("concurrent {}-{}", i, j),
                    );
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let log_file = dir.join("audit.log");
        let content = std::fs::read_to_string(&log_file).unwrap();
        let count = content.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(count, 40);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_verify_integrity_clean() {
        let dir = make_temp_dir();
        let config = make_test_config(&dir);
        let logger = AuditLogger::new(config).unwrap();

        for i in 0..3 {
            logger
                .log(
                    AuditAction::Login,
                    "admin",
                    "system",
                    AuditResult::Success,
                    None,
                    &format!("clean entry {}", i),
                )
                .unwrap();
        }

        let violations = logger.verify_integrity().unwrap();
        assert!(
            violations.is_empty(),
            "clean log should have no violations: {:?}",
            violations
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_flush() {
        let dir = make_temp_dir();
        let config = make_test_config(&dir);
        let logger = AuditLogger::new(config).unwrap();

        logger
            .log(
                AuditAction::Login,
                "admin",
                "system",
                AuditResult::Success,
                None,
                "before flush",
            )
            .unwrap();
        assert!(logger.flush().is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
