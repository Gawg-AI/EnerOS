//! ProductManifest v2：正式制品与服务装配的唯一事实（蓝图 36 WP-36.3/36.5）。
//!
//! 核心不变量：一个 release profile → 一个规范化 ProductManifest →
//! 同一对象生成 build plan、Binary BOM、init bundle、SBOM scope 与 Gate expectation。
//! 任何派生物不得维护独立硬编码服务列表。
//!
//! 严格性：未知字段（serde deny_unknown_fields）、重复 service ID、
//! 循环依赖、缺 required service、零 digest、全零 key 一律失败。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{invalid, read_json, DynResult, ProductProfile};

pub const MANIFEST_SCHEMA: &str = "eneros.product-manifest.v2";
pub const PROFILE_SCHEMA_V2: &str = "eneros.product-profile.v2";
pub const SERVICE_KIND_REGISTRY_SCHEMA: &str = "eneros.service-kind-registry.v1";

/// 唯一角色集合（蓝图 36 §3.3）：boot-manager、kernel、root-task、
/// required-service、optional-service、recovery、installer、factory。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManifestRole {
    BootManager,
    Kernel,
    RootTask,
    RequiredService,
    OptionalService,
    Recovery,
    Installer,
    Factory,
}

impl ManifestRole {
    pub const ALL: [&'static str; 8] = [
        "boot-manager",
        "kernel",
        "root-task",
        "required-service",
        "optional-service",
        "recovery",
        "installer",
        "factory",
    ];

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "boot-manager" => Some(Self::BootManager),
            "kernel" => Some(Self::Kernel),
            "root-task" => Some(Self::RootTask),
            "required-service" => Some(Self::RequiredService),
            "optional-service" => Some(Self::OptionalService),
            "recovery" => Some(Self::Recovery),
            "installer" => Some(Self::Installer),
            "factory" => Some(Self::Factory),
            _ => None,
        }
    }

    /// 该角色是否进入 init bundle（root + 服务类）。
    pub fn is_bundled(self) -> bool {
        matches!(
            self,
            Self::RootTask | Self::RequiredService | Self::OptionalService
        )
    }

    #[allow(dead_code)] // 44.2：角色判定工具，当前调用面未接线（44.4 冻结登记）
    pub fn is_service(self) -> bool {
        matches!(
            self,
            Self::RootTask | Self::RequiredService | Self::OptionalService
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BootManager => "boot-manager",
            Self::Kernel => "kernel",
            Self::RootTask => "root-task",
            Self::RequiredService => "required-service",
            Self::OptionalService => "optional-service",
            Self::Recovery => "recovery",
            Self::Installer => "installer",
            Self::Factory => "factory",
        }
    }
}

/// Requirement：Required | Optional（禁止默认把 malformed service 变 optional）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Requirement {
    #[default]
    Required,
    Optional,
}

/// Criticality 等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Criticality {
    #[default]
    System,
    Control,
    Data,
    Support,
}

/// 重启策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    #[default]
    Always,
    OnFailure,
    Never,
}

/// 就绪契约：只有显式声明 ready 语义的服务才能宣称健康。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessContract {
    pub probe: String,
    pub timeout_ms: u64,
    pub required_ack: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySpec {
    pub name: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudget {
    pub stack_pages: u32,
    pub ipc_buffers: u32,
    pub priority: u32,
}

/// 服务声明。binary 用 `package::bin` selector 或外部路径（external=true）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSpec {
    pub id: String,
    pub binary: String,
    /// 空字符串表示 digest 由构建后计算（normalized manifest 中必填）。
    #[serde(default)]
    pub digest: String,
    pub role: ManifestRole,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub default_features: bool,
    #[serde(default)]
    pub requirement: Requirement,
    #[serde(default)]
    pub criticality: Criticality,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<CapabilitySpec>,
    #[serde(default)]
    pub resources: Option<ResourceBudget>,
    #[serde(default)]
    pub configs: Vec<String>,
    #[serde(default)]
    pub readiness: Option<ReadinessContract>,
    /// 外部预构建 artifact（如 seL4 elfloader），不参与 cargo 构建。
    #[serde(default)]
    pub external: bool,
}

impl ServiceSpec {
    pub fn is_required(&self) -> bool {
        self.requirement == Requirement::Required
    }
}

/// Kernel 声明：必须属于 kernel-policy allowlist（由 policy gate 复核）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KernelSpec {
    pub name: String,
    pub selector: String,
    #[serde(default)]
    pub digest: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSpec {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSpec {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub digest: String,
    /// 消费该配置的服务 id；空 = 无 consumer（release 阻断）。
    #[serde(default)]
    pub consumer: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityPolicyRef {
    pub policy: String,
    #[serde(default)]
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidencePolicyRef {
    pub policy: String,
    #[serde(default)]
    pub digest: String,
}

/// 规范化 ProductManifest v2（唯一可签名输入）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProductManifest {
    pub schema: String,
    pub product_id: String,
    pub profile_id: String,
    pub class: String,
    pub architecture: String,
    pub platform_contract: String,
    pub kernel: KernelSpec,
    pub artifacts: Vec<ArtifactSpec>,
    pub services: Vec<ServiceSpec>,
    pub configs: Vec<ConfigSpec>,
    pub security: SecurityPolicyRef,
    pub evidence_policy: EvidencePolicyRef,
    /// sha256(规范化 manifest 内容，不含本字段)。
    #[serde(default)]
    pub manifest_digest: String,
}

impl ProductManifest {
    /// 计算 manifest_digest（规范化排序序列化，不含 digest 自身）。
    pub fn compute_digest(&self) -> DynResult<String> {
        let mut clone = self.clone();
        clone.manifest_digest.clear();
        clone.services.sort_by(|a, b| a.id.cmp(&b.id));
        clone.artifacts.sort_by(|a, b| a.id.cmp(&b.id));
        clone.configs.sort_by(|a, b| a.id.cmp(&b.id));
        let json = serde_json::to_vec(&clone)?;
        Ok(hex(&Sha256::digest(&json)))
    }

    /// 深度校验：重复 ID、循环依赖、required 集合、零 digest、selector 语法。
    pub fn validate(&self) -> DynResult<Vec<String>> {
        let mut violations = Vec::new();
        if self.schema != MANIFEST_SCHEMA {
            violations.push(format!("manifest-schema:{}", self.schema));
        }
        if self.class != "release" && self.class != "recovery" && self.class != "factory" {
            violations.push(format!("manifest-class:{}", self.class));
        }
        if self.architecture != "aarch64" {
            violations.push(format!("manifest-architecture:{}", self.architecture));
        }
        // 重复 service ID。
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for service in &self.services {
            if !seen.insert(&service.id) {
                violations.push(format!("duplicate-service:{}", service.id));
            }
        }
        // selector 语法：package::bin 或外部路径。
        for service in &self.services {
            if !service.external
                && !service
                    .binary
                    .split_once("::")
                    .is_some_and(|(p, b)| !p.is_empty() && !b.is_empty())
            {
                violations.push(format!("selector-syntax:{}:{}", service.id, service.binary));
            }
        }
        // 依赖环检测（36 收尾复验修复）：全图 DFS——旧实现只沿每个服务的
        // 第一条依赖边遍历，第二条及之后的边构成的环（含跨起点的环）会漏检；
        // 蓝图 36 WP-36.3 要求“循环依赖一律失败”。
        let ids: BTreeSet<&str> = self.services.iter().map(|s| s.id.as_str()).collect();
        for service in &self.services {
            for dependency in &service.dependencies {
                if !ids.contains(dependency.as_str()) {
                    violations.push(format!("unknown-dependency:{}:{}", service.id, dependency));
                }
            }
        }
        let mut done: BTreeSet<String> = BTreeSet::new();
        for service in &self.services {
            if done.contains(&service.id) {
                continue;
            }
            let mut path: Vec<String> = Vec::new();
            if self.detect_dependency_cycle(&service.id, &mut path, &mut done, &mut violations) {
                violations.push(format!("dependency-cycle:{}", path.join("->")));
            }
        }
        // root-task 必须存在且 required；至少一个 required 服务。
        let root = self
            .services
            .iter()
            .find(|s| s.role == ManifestRole::RootTask);
        if root.is_none() {
            violations.push("missing-root-task".to_owned());
        } else if !root.is_some_and(|s| s.is_required()) {
            violations.push("root-task-not-required".to_owned());
        }
        if !self
            .services
            .iter()
            .any(|s| s.role == ManifestRole::RequiredService && s.is_required())
        {
            violations.push("no-required-service".to_owned());
        }
        // 蓝图 40 §7：required service 基线——删除任一基线服务（或降级为
        // optional）36/37 Gate 必须阻断（机器可验证，不依赖自述）。
        for baseline in REQUIRED_SERVICE_BASELINE {
            let ok = self
                .services
                .iter()
                .any(|s| s.id == *baseline && s.is_required());
            if !ok {
                violations.push(format!("required-service-baseline-missing:{baseline}"));
            }
        }
        // kernel 全零 key / 空 digest 检查（外部 kernel 允许空 digest 时
        // 由 policy gate 绑定外部 artifact hash；这里禁止全零 64 hex）。
        if !self.kernel.digest.is_empty() && self.kernel.digest.chars().all(|c| c == '0') {
            violations.push("all-zero-kernel-digest".to_owned());
        }
        // configs 无 consumer 阻断（release 语义：active 配置必须有 typed consumer）。
        for config in &self.configs {
            if config.consumer.is_empty() {
                violations.push(format!("config-no-consumer:{}", config.id));
            }
        }
        // WP-37.2：能力名必须登记（runtime 不认识的能力 = 授权漂移）。
        violations.extend(validate_capabilities(self));
        Ok(violations)
    }

    /// 依赖环 DFS（沿全部依赖边；`done` 记录已完成且无环的节点）。
    /// 返回 true 时 path 首尾为同一节点，构成完整环路径。
    #[allow(clippy::only_used_in_recursion)] // 44.2：violations 由调用方在循环闭合后登记，函数内仅穿参
    fn detect_dependency_cycle(
        &self,
        id: &str,
        path: &mut Vec<String>,
        done: &mut BTreeSet<String>,
        violations: &mut Vec<String>,
    ) -> bool {
        if path.iter().any(|p| p == id) {
            path.push(id.to_owned());
            return true;
        }
        if done.contains(id) {
            return false;
        }
        let Some(service) = self.services.iter().find(|s| s.id == id) else {
            return false; // 未知依赖由 unknown-dependency 检查报告
        };
        path.push(id.to_owned());
        for dependency in &service.dependencies {
            if self.detect_dependency_cycle(dependency, path, done, violations) {
                return true;
            }
        }
        path.pop();
        done.insert(id.to_owned());
        false
    }

    /// 构建后回填 digest（服务 ELF sha256 + kernel sha256），重算 manifest_digest。
    /// 返回更新后的自身（不可变语义便于链式调用）。
    pub fn bind_digests(
        &mut self,
        service_digests: &BTreeMap<String, String>,
        kernel_digest: &str,
    ) -> DynResult<()> {
        for service in &mut self.services {
            if let Some(digest) = service_digests.get(&service.id) {
                service.digest = digest.clone();
            }
        }
        if !kernel_digest.is_empty() {
            self.kernel.digest = kernel_digest.to_owned();
        }
        self.manifest_digest = self.compute_digest()?;
        Ok(())
    }

    /// 构建绑定校验：进入 init bundle 的 required 服务 digest 必须非空
    /// （zero digest 失败）。boot-manager/installer 等非 bundle 角色由
    /// Binary BOM 构建后绑定。
    pub fn validate_bound(&self) -> Vec<String> {
        let mut violations = Vec::new();
        for service in &self.services {
            if service.role.is_bundled() && service.is_required() && service.digest.is_empty() {
                violations.push(format!("zero-digest:{}", service.id));
            }
        }
        if self.kernel.digest.is_empty() {
            violations.push("zero-kernel-digest".to_owned());
        }
        violations
    }

    /// 归一化排序后序列化（确定性输出）。
    pub fn normalized_json(&self) -> DynResult<Vec<u8>> {
        let mut clone = self.clone();
        clone.services.sort_by(|a, b| a.id.cmp(&b.id));
        clone.artifacts.sort_by(|a, b| a.id.cmp(&b.id));
        clone.configs.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(serde_json::to_vec_pretty(&clone)?)
    }
}

/// 从 profile（v1 或 v2）规范化生成 ProductManifest。
///
/// - v2 profile：直接读取 kernel/services/configs/security_ref/evidence_ref；
///   同时兼容 v1 的 binaries 字段（转换为 service 声明）。
/// - v1 profile：binaries 全部转 service（boot-manager/kernel/installer 等
///   角色按 ManifestRole 映射），root/services 集合保持 v1 语义。
pub fn normalize_profile(root: &Path, profile: &ProductProfile) -> DynResult<ProductManifest> {
    let profile_path = root
        .join("product/profiles")
        .join(format!("{}.json", profile.id));
    let raw: serde_json::Value = read_json(&profile_path)?;
    let schema = raw
        .get("schema")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    // kernel：v2 显式 kernel 字段，否则从 binaries 找 role=kernel。
    let kernel = match raw.get("kernel") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| invalid(format!("profile kernel field: {e}")))?,
        None => {
            let bin = raw
                .get("binaries")
                .and_then(|b| b.as_array())
                .and_then(|list| {
                    list.iter()
                        .find(|item| item.get("role").and_then(|r| r.as_str()) == Some("kernel"))
                })
                .ok_or_else(|| invalid("profile has no kernel binary"))?;
            let selector = bin
                .get("selector")
                .and_then(|s| s.as_str())
                .ok_or_else(|| invalid("kernel selector missing"))?;
            // selector 支持 package::bin（cargo）与 external:name（seL4 等）。
            let (name, _) = selector
                .split_once("::")
                .or_else(|| selector.split_once(':'))
                .ok_or_else(|| invalid("kernel selector must be package::bin or external:name"))?;
            let mut features = Vec::new();
            if let Some(list) = bin.get("features").and_then(|f| f.as_array()) {
                for f in list {
                    if let Some(f) = f.as_str() {
                        features.push(f.to_owned());
                    }
                }
            }
            KernelSpec {
                name: name.to_owned(),
                selector: selector.to_owned(),
                digest: String::new(),
                target: bin
                    .get("target")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                features,
                external: selector.starts_with("sel4:") || selector.starts_with("external:"),
            }
        }
    };

    // services：v2 显式 services + binaries（不含 kernel 的其余角色）。
    let mut services: Vec<ServiceSpec> = Vec::new();
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();
    if let Some(list) = raw.get("services").and_then(|s| s.as_array()) {
        for value in list {
            let service: ServiceSpec = serde_json::from_value(value.clone())
                .map_err(|e| invalid(format!("profile service: {e}")))?;
            if !seen_ids.insert(service.id.clone()) {
                return Err(invalid(format!(
                    "duplicate service id in profile: {}",
                    service.id
                )));
            }
            services.push(service);
        }
    }
    if let Some(list) = raw.get("binaries").and_then(|b| b.as_array()) {
        for value in list {
            let Some(role_text) = value.get("role").and_then(|r| r.as_str()) else {
                return Err(invalid("binary without role"));
            };
            if role_text == "kernel" {
                continue;
            }
            let Some(role) = ManifestRole::parse(role_text) else {
                return Err(invalid(format!(
                    "schema-unknown-role:{role_text} (allowed: {})",
                    ManifestRole::ALL.join(",")
                )));
            };
            let selector = value
                .get("selector")
                .and_then(|s| s.as_str())
                .ok_or_else(|| invalid("binary selector missing"))?;
            let id = match role {
                ManifestRole::BootManager => "boot-manager".to_owned(),
                ManifestRole::Installer => "installer".to_owned(),
                ManifestRole::Recovery => "recovery".to_owned(),
                ManifestRole::Factory => "factory".to_owned(),
                ManifestRole::RootTask => "root-task".to_owned(),
                _ => {
                    let (_, bin) = selector.split_once("::").unwrap_or((selector, selector));
                    bin.to_owned()
                }
            };
            if seen_ids.insert(id.clone()) {
                let mut features = Vec::new();
                if let Some(list) = value.get("features").and_then(|f| f.as_array()) {
                    for f in list {
                        if let Some(f) = f.as_str() {
                            features.push(f.to_owned());
                        }
                    }
                }
                services.push(ServiceSpec {
                    id,
                    binary: selector.to_owned(),
                    digest: String::new(),
                    role,
                    target: value
                        .get("target")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    features,
                    default_features: value
                        .get("default_features")
                        .and_then(|d| d.as_bool())
                        .unwrap_or(false),
                    requirement: if role == ManifestRole::OptionalService {
                        Requirement::Optional
                    } else {
                        Requirement::Required
                    },
                    criticality: Criticality::Support,
                    restart: RestartPolicy::Never,
                    dependencies: Vec::new(),
                    capabilities: Vec::new(),
                    resources: None,
                    configs: Vec::new(),
                    readiness: None,
                    external: selector.starts_with("sel4:") || selector.starts_with("external:"),
                });
            }
        }
    }

    // configs：v2 显式声明。
    let mut configs: Vec<ConfigSpec> = Vec::new();
    if let Some(list) = raw.get("configs").and_then(|c| c.as_array()) {
        for value in list {
            let config: ConfigSpec = serde_json::from_value(value.clone())
                .map_err(|e| invalid(format!("profile config: {e}")))?;
            configs.push(config);
        }
    }

    let security = match raw.get("security_policy_ref") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| invalid(format!("security_policy_ref: {e}")))?,
        None => SecurityPolicyRef {
            policy: "product/crypto-policy.toml".to_owned(),
            digest: String::new(),
        },
    };
    let evidence_policy = match raw.get("evidence_policy_ref") {
        Some(value) => serde_json::from_value(value.clone())
            .map_err(|e| invalid(format!("evidence_policy_ref: {e}")))?,
        None => EvidencePolicyRef {
            policy: "product/evidence/schema.json".to_owned(),
            digest: String::new(),
        },
    };

    let mut artifacts = Vec::new();
    if let Some(list) = raw.get("artifacts").and_then(|a| a.as_array()) {
        for value in list {
            if let Some(kind) = value.as_str() {
                artifacts.push(ArtifactSpec {
                    id: kind.to_owned(),
                    kind: kind.to_owned(),
                    path: String::new(),
                    digest: String::new(),
                });
            }
        }
    }

    let mut manifest = ProductManifest {
        schema: MANIFEST_SCHEMA.to_owned(),
        product_id: "eneros".to_owned(),
        profile_id: profile.id.clone(),
        class: profile.class.policy_name().to_owned(),
        architecture: "aarch64".to_owned(),
        platform_contract: profile.platform.clone(),
        kernel,
        artifacts,
        services,
        configs,
        security,
        evidence_policy,
        manifest_digest: String::new(),
    };
    manifest.manifest_digest = manifest.compute_digest()?;
    let violations = manifest.validate()?;
    if !violations.is_empty() {
        return Err(invalid(format!(
            "manifest validation failed: {}",
            violations.join("; ")
        )));
    }
    let _ = schema; // v1/v2 兼容；schema 校验由 profile load 层完成
    Ok(manifest)
}

/// 派生物目录：target/eneros/manifest/<profile_id>/。
/// 按 profile 隔离，避免多 profile 交替 normalize/derive/image 互相覆盖
/// （36 收尾：board-prod derive 曾覆盖 qemu-prod normalized manifest）。
pub fn manifest_dir(root: &Path, profile_id: &str) -> PathBuf {
    root.join("target/eneros/manifest").join(profile_id)
}

/// 写 product-manifest.normalized.json（唯一可签名输入）。
pub fn write_normalized(root: &Path, manifest: &ProductManifest) -> DynResult<PathBuf> {
    let dir = manifest_dir(root, &manifest.profile_id);
    fs::create_dir_all(&dir)?;
    let path = dir.join("product-manifest.normalized.json");
    fs::write(&path, manifest.normalized_json()?)?;
    println!(
        "PRODUCT-MANIFEST profile={} digest={} → {}",
        manifest.profile_id,
        manifest.manifest_digest,
        path.display()
    );
    Ok(path)
}

/// 加载已生成的 normalized manifest（用于派生与 Gate）。
pub fn load_normalized(root: &Path, profile_id: &str) -> DynResult<ProductManifest> {
    // 36 收尾（security review）：CLI profile_id 必须先消毒再拼路径，
    // 防路径穿越（closure gate / manifest derive / verify-bundle 共用此入口）。
    if !crate::is_profile_id(profile_id) {
        return Err(invalid(format!("invalid profile id '{profile_id}'")));
    }
    let path = manifest_dir(root, profile_id).join("product-manifest.normalized.json");
    let manifest: ProductManifest = read_json(&path)?;
    if manifest.profile_id != profile_id {
        return Err(invalid(format!(
            "normalized manifest profile mismatch: {} != {}",
            manifest.profile_id, profile_id
        )));
    }
    let recomputed = manifest.compute_digest()?;
    if recomputed != manifest.manifest_digest {
        return Err(invalid("normalized manifest digest mismatch"));
    }
    Ok(manifest)
}

/// 蓝图 40 §7：required service 基线（唯一产品控制闭环装配集合）。
/// 删除/降级任一基线服务 → `validate()` 阻断（36/37 Gate）。
/// 注：svc-ai-worker 若产品 capability 明确 out-of-profile（蓝图 40 §7），
/// 须由产品负责人签署基线调整——本基线随当前产品声明。
pub const REQUIRED_SERVICE_BASELINE: &[&str] = &[
    "root-task",
    "svc-telemetry",
    "svc-energy-agent",
    "svc-ai-worker",
    "svc-control-dispatch",
    "svc-device-agent",
];

/// ServiceKind registry（蓝图 36 WP-36.5）：registry 只定义标识与 capability，
/// 不把未实现服务写入 release profile。
pub const SERVICE_KIND_REGISTRY: &[(&str, &str)] = &[
    // runtime
    ("runtime.root", "root supervisor"),
    ("runtime.config", "signed runtime configuration"),
    ("runtime.audit", "audit chain persistence"),
    // driver
    ("driver.console", "console/serial service"),
    ("driver.entropy", "entropy provider service"),
    ("driver.block", "block storage service"),
    ("driver.network", "network service"),
    ("driver.bus", "field bus service"),
    ("driver.time", "time synchronization service"),
    ("driver.watchdog", "watchdog service"),
    // protocol
    ("protocol.modbus-rtu", "Modbus RTU master/slave"),
    ("protocol.modbus-tcp", "Modbus TCP"),
    ("protocol.iec104", "IEC 60870-5-104"),
    ("protocol.mms", "MMS"),
    ("protocol.goose", "GOOSE"),
    ("protocol.sv", "Sampled Values"),
    ("protocol.mqtt", "MQTT client"),
    // control
    ("control.telemetry", "telemetry pipeline"),
    ("control.energy-agent", "energy agent"),
    ("control.ai-worker", "AI worker"),
    ("control.device-agent", "device agent"),
    ("control.dispatch", "control dispatch (唯一控制写出口)"),
    // business
    ("business.market", "energy market"),
    ("business.settlement", "settlement"),
    ("business.account", "account"),
    ("business.ota", "system OTA"),
    ("business.model-ota", "model OTA"),
    ("business.federation", "federation"),
    ("business.cloud-sync", "cloud sync"),
    // ops
    ("ops.mgmt", "management API"),
    ("ops.metrics", "metrics"),
    ("ops.logging", "logging"),
    ("ops.tracing", "tracing"),
    ("ops.dashboard", "dashboard"),
    ("ops.alert", "alerting"),
];

pub fn write_service_kind_registry(root: &Path) -> DynResult<PathBuf> {
    let path = root.join("product/service-kind-registry.json");
    let value = serde_json::json!({
        "schema": SERVICE_KIND_REGISTRY_SCHEMA,
        "kinds": SERVICE_KIND_REGISTRY.iter().map(|(id, capability)| {
            serde_json::json!({ "id": id, "capability": capability, "status": "declared" })
        }).collect::<Vec<_>>(),
    });
    fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
    Ok(path)
}

/// 校验 profile 声明的 service kind 全部来自 registry（未实现服务不得入 release）。
/// 生效点：37+ 新服务接入门禁（届时由新 owner 接线）。36 不强制——当前正式
/// profile 的服务集（root/svc-*）早于 registry 建立，立即强制会阻断现有装配；
/// registry 的约束方向是“未登记的新 kind 不得进入 release profile”。
#[allow(dead_code)] // 44.2：服务种类校验，当前调用面未接线（44.4 冻结登记）
pub fn validate_service_kinds(manifest: &ProductManifest) -> Vec<String> {
    let mut violations = Vec::new();
    let known: BTreeSet<&str> = SERVICE_KIND_REGISTRY.iter().map(|(id, _)| *id).collect();
    for service in &manifest.services {
        if !known.contains(service.id.as_str()) {
            // 允许 boot-manager/installer/recovery/factory 等装配角色（非 service kind）。
            if service.role.is_service() {
                violations.push(format!("unregistered-service-kind:{}", service.id));
            }
        }
    }
    violations
}

/// WP-37.2：能力名注册表（与 runtime `bundle.rs::CAPABILITY_NAMES` 对应；
/// 双侧必须一致——新增能力须同时登记，未知名在 manifest 校验层拒绝）。
/// 位值与 runtime `CAP_*` 常量一致（1<<16 起，flags 高 16 位）。
pub fn capability_bit(name: &str) -> Option<u32> {
    match name {
        "supervisor" => Some(1 << 16),
        "ep-send" => Some(1 << 17),
        "ep-recv" => Some(1 << 18),
        "device-io" => Some(1 << 19),
        _ => None,
    }
}

/// WP-37.2：manifest 能力名校验（fail-closed）——未知名能力不得进入
/// release（runtime 不认识的能力 = 授权漂移）。
pub fn validate_capabilities(manifest: &ProductManifest) -> Vec<String> {
    let mut violations = Vec::new();
    for service in &manifest.services {
        for cap in &service.capabilities {
            if capability_bit(&cap.name).is_none() {
                violations.push(format!("unknown-capability:{}:{}", service.id, cap.name));
            }
        }
    }
    violations
}

/// 解析 init bundle（ENRINIT2）条目：与 image_factory 打包格式一致。
/// 返回 (kind, flags, name, sha256)。
pub fn parse_init_bundle(bytes: &[u8]) -> DynResult<Vec<(u32, u32, String, String)>> {
    if bytes.len() < 64 || &bytes[..8] != b"ENRINIT2" {
        return Err(invalid("not an ENRINIT2 init bundle"));
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    // WP-37.2 wire v3：接受 v2/v3（v3 的 flags 高 16 位为能力位图）。
    if version != 3 && version != 2 {
        return Err(invalid(format!("init bundle version {version} != 2/3")));
    }
    let header_len = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
    if header_len != 64 {
        return Err(invalid("init bundle header len != 64"));
    }
    let entry_len = 96usize;
    let count = usize::try_from(u32::from_le_bytes(bytes[24..28].try_into()?))?;
    if bytes.len() < header_len + entry_len * count {
        return Err(invalid("init bundle truncated table"));
    }
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let entry = header_len + index * entry_len;
        let kind = u32::from_le_bytes(bytes[entry..entry + 4].try_into()?);
        let flags = u32::from_le_bytes(bytes[entry + 4..entry + 8].try_into()?);
        let offset = usize::try_from(u64::from_le_bytes(bytes[entry + 8..entry + 16].try_into()?))?;
        let len = usize::try_from(u64::from_le_bytes(
            bytes[entry + 16..entry + 24].try_into()?,
        ))?;
        let name_end = entry
            + 24
            + bytes[entry + 24..entry + 56]
                .iter()
                .position(|b| *b == 0)
                .unwrap_or(32);
        let name = String::from_utf8_lossy(&bytes[entry + 24..name_end]).to_string();
        if offset + len > bytes.len() {
            return Err(invalid(format!("init bundle entry {name} out of bounds")));
        }
        entries.push((
            kind,
            flags,
            name,
            hex(&Sha256::digest(&bytes[offset..offset + len])),
        ));
    }
    Ok(entries)
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

    fn sample_manifest() -> ProductManifest {
        ProductManifest {
            schema: MANIFEST_SCHEMA.to_owned(),
            product_id: "eneros".to_owned(),
            profile_id: "qemu-prod".to_owned(),
            class: "release".to_owned(),
            architecture: "aarch64".to_owned(),
            platform_contract: "qemu-virt-aarch64".to_owned(),
            kernel: KernelSpec {
                name: "sel4".to_owned(),
                selector: "sel4:elfloader-arm-qemu-arm-virt".to_owned(),
                digest: "deadbeef".to_owned(),
                target: "host".to_owned(),
                features: vec![],
                external: true,
            },
            artifacts: vec![],
            services: vec![
                ServiceSpec {
                    id: "root-task".to_owned(),
                    binary: "eneros-root-task::eneros-root-task".to_owned(),
                    digest: "aabb".to_owned(),
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
                },
                ServiceSpec {
                    id: "svc-hello".to_owned(),
                    binary: "eneros-services::svc-hello".to_owned(),
                    digest: "ccdd".to_owned(),
                    role: ManifestRole::RequiredService,
                    target: "aarch64-unknown-none".to_owned(),
                    features: vec![],
                    default_features: false,
                    requirement: Requirement::Required,
                    criticality: Criticality::Support,
                    restart: RestartPolicy::Always,
                    dependencies: vec!["root-task".to_owned()],
                    capabilities: vec![],
                    resources: None,
                    configs: vec![],
                    readiness: None,
                    external: false,
                },
                // 蓝图 40 §7 required-service 基线（sample 必须包含——否则
                // required-service-baseline-missing 违规）。
                ServiceSpec {
                    id: "svc-telemetry".to_owned(),
                    binary: "eneros-control-services::svc-telemetry".to_owned(),
                    digest: "a1a1".to_owned(),
                    role: ManifestRole::RequiredService,
                    target: "aarch64-unknown-none".to_owned(),
                    features: vec![],
                    default_features: false,
                    requirement: Requirement::Required,
                    criticality: Criticality::Control,
                    restart: RestartPolicy::Always,
                    dependencies: vec!["root-task".to_owned()],
                    capabilities: vec![],
                    resources: None,
                    configs: vec![],
                    readiness: None,
                    external: false,
                },
                ServiceSpec {
                    id: "svc-energy-agent".to_owned(),
                    binary: "eneros-control-services::svc-energy-agent".to_owned(),
                    digest: "b2b2".to_owned(),
                    role: ManifestRole::RequiredService,
                    target: "aarch64-unknown-none".to_owned(),
                    features: vec![],
                    default_features: false,
                    requirement: Requirement::Required,
                    criticality: Criticality::Control,
                    restart: RestartPolicy::Always,
                    dependencies: vec!["svc-telemetry".to_owned()],
                    capabilities: vec![],
                    resources: None,
                    configs: vec![],
                    readiness: None,
                    external: false,
                },
                ServiceSpec {
                    id: "svc-ai-worker".to_owned(),
                    binary: "eneros-control-services::svc-ai-worker".to_owned(),
                    digest: "c3c3".to_owned(),
                    role: ManifestRole::RequiredService,
                    target: "aarch64-unknown-none".to_owned(),
                    features: vec![],
                    default_features: false,
                    requirement: Requirement::Required,
                    criticality: Criticality::Control,
                    restart: RestartPolicy::Always,
                    dependencies: vec!["svc-telemetry".to_owned()],
                    capabilities: vec![],
                    resources: None,
                    configs: vec![],
                    readiness: None,
                    external: false,
                },
                ServiceSpec {
                    id: "svc-control-dispatch".to_owned(),
                    binary: "eneros-control-services::svc-control-dispatch".to_owned(),
                    digest: "d4d4".to_owned(),
                    role: ManifestRole::RequiredService,
                    target: "aarch64-unknown-none".to_owned(),
                    features: vec![],
                    default_features: false,
                    requirement: Requirement::Required,
                    criticality: Criticality::Control,
                    restart: RestartPolicy::Always,
                    dependencies: vec!["svc-device-agent".to_owned()],
                    capabilities: vec![],
                    resources: None,
                    configs: vec![],
                    readiness: None,
                    external: false,
                },
                ServiceSpec {
                    id: "svc-device-agent".to_owned(),
                    binary: "eneros-control-services::svc-device-agent".to_owned(),
                    digest: "e5e5".to_owned(),
                    role: ManifestRole::RequiredService,
                    target: "aarch64-unknown-none".to_owned(),
                    features: vec![],
                    default_features: false,
                    requirement: Requirement::Required,
                    criticality: Criticality::Control,
                    restart: RestartPolicy::Always,
                    dependencies: vec!["root-task".to_owned()],
                    capabilities: vec![],
                    resources: None,
                    configs: vec![],
                    readiness: None,
                    external: false,
                },
            ],
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
        }
    }

    #[test]
    fn valid_manifest_passes_validation() {
        let manifest = sample_manifest();
        assert!(manifest.validate().unwrap().is_empty());
    }

    #[test]
    fn duplicate_service_is_rejected() {
        let mut manifest = sample_manifest();
        let dup = manifest.services[1].clone();
        manifest.services.push(dup);
        let violations = manifest.validate().unwrap();
        assert!(violations
            .iter()
            .any(|v| v.starts_with("duplicate-service")));
    }

    #[test]
    fn dependency_cycle_is_rejected() {
        let mut manifest = sample_manifest();
        manifest.services[1].dependencies[0] = "svc-hello".to_owned();
        let violations = manifest.validate().unwrap();
        assert!(violations.iter().any(|v| v.starts_with("dependency-cycle")));
    }

    #[test]
    fn dependency_cycle_on_non_first_edge_is_rejected() {
        // 36 收尾复验：旧实现只沿第一条依赖边遍历，此环（经第二条边）漏检。
        // 拓扑：root-task -deps[svc-hello, svc-pi]-> svc-hello；svc-hello -> root-task。
        let mut manifest = sample_manifest();
        manifest.services[0].dependencies = vec!["svc-hello".to_owned(), "svc-pi".to_owned()];
        manifest.services.push(ServiceSpec {
            id: "svc-pi".to_owned(),
            binary: "eneros-services::svc-pi".to_owned(),
            digest: "eeff".to_owned(),
            role: ManifestRole::RequiredService,
            target: "aarch64-unknown-none".to_owned(),
            features: vec![],
            default_features: false,
            requirement: Requirement::Required,
            criticality: Criticality::Support,
            restart: RestartPolicy::Always,
            dependencies: vec!["root-task".to_owned()],
            capabilities: vec![],
            resources: None,
            configs: vec![],
            readiness: None,
            external: false,
        });
        let violations = manifest.validate().unwrap();
        assert!(
            violations.iter().any(|v| v.starts_with("dependency-cycle")),
            "cycle via non-first dependency edge must be rejected: {violations:?}"
        );
        // 无环依赖不误报：解除 svc-hello→root-task 回边，变单向链。
        manifest.services[1].dependencies = Vec::new();
        manifest.services[2].dependencies = vec!["svc-hello".to_owned()];
        manifest.services[0].dependencies = vec!["svc-hello".to_owned()];
        let violations = manifest.validate().unwrap();
        assert!(
            !violations.iter().any(|v| v.starts_with("dependency-cycle")),
            "acyclic dependencies must not be flagged: {violations:?}"
        );
    }

    #[test]
    fn missing_root_task_is_rejected() {
        let mut manifest = sample_manifest();
        manifest.services.remove(0);
        let violations = manifest.validate().unwrap();
        assert!(violations.iter().any(|v| v == "missing-root-task"));
    }

    #[test]
    fn unknown_role_is_rejected() {
        assert_eq!(
            ManifestRole::parse("installer"),
            Some(ManifestRole::Installer)
        );
        assert_eq!(ManifestRole::parse("root"), None);
        assert_eq!(
            ManifestRole::parse("required-service"),
            Some(ManifestRole::RequiredService)
        );
    }

    #[test]
    fn digest_is_deterministic_and_self_consistent() {
        let manifest = sample_manifest();
        let first = manifest.compute_digest().unwrap();
        let second = manifest.compute_digest().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn parse_bundle_roundtrip() {
        // 手工构造最小 ENRINIT2（2 个条目）。
        let mut bytes = vec![0u8; 64 + 96 * 2];
        bytes[..8].copy_from_slice(b"ENRINIT2");
        bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&64u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&2u32.to_le_bytes());
        bytes[32..64].fill(0xAB);
        for (i, (kind, flags, name)) in [(1u32, 31u32, "root-task"), (2, 20, "svc-hello")]
            .into_iter()
            .enumerate()
        {
            let entry = 64 + i * 96;
            bytes[entry..entry + 4].copy_from_slice(&kind.to_le_bytes());
            bytes[entry + 4..entry + 8].copy_from_slice(&flags.to_le_bytes());
            bytes[entry + 24..entry + 24 + name.len()].copy_from_slice(name.as_bytes());
        }
        let entries = parse_init_bundle(&bytes).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].2, "root-task");
        assert_eq!(entries[1].2, "svc-hello");
    }

    #[test]
    fn parse_bundle_rejects_truncated() {
        assert!(parse_init_bundle(b"ENRINIT2").is_err());
        assert!(parse_init_bundle(&[]).is_err());
        assert!(parse_init_bundle(&[0u8; 64]).is_err());
    }

    #[test]
    fn registry_covers_planned_kinds() {
        for (id, _) in SERVICE_KIND_REGISTRY {
            let parts: Vec<&str> = id.split('.').collect();
            assert_eq!(parts.len(), 2, "kind id {id} must be domain.name");
        }
    }

    #[test]
    fn manifest_dir_isolates_profiles() {
        // 36 收尾：多 profile 交替 normalize/derive/image 不得互相覆盖。
        let root = std::env::temp_dir().join(format!("eneros-36-isolate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut m = sample_manifest();
        m.profile_id = "qemu-prod".to_owned();
        m.manifest_digest = m.compute_digest().unwrap();
        let p1 = write_normalized(&root, &m).unwrap();
        m.profile_id = "board-prod".to_owned();
        m.manifest_digest = m.compute_digest().unwrap();
        let p2 = write_normalized(&root, &m).unwrap();
        assert_ne!(
            p1, p2,
            "profiles must not share the normalized manifest file"
        );
        assert!(p1.ends_with("qemu-prod/product-manifest.normalized.json"));
        assert!(p2.ends_with("board-prod/product-manifest.normalized.json"));
        // 各自内容可独立加载且 profile 匹配。
        let loaded_q = load_normalized(&root, "qemu-prod").unwrap();
        assert_eq!(loaded_q.profile_id, "qemu-prod");
        let loaded_b = load_normalized(&root, "board-prod").unwrap();
        assert_eq!(loaded_b.profile_id, "board-prod");
        let _ = std::fs::remove_dir_all(&root);
    }
}
