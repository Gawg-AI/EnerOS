//! 由唯一 ProductManifest 派生 build plan / init bundle / Binary BOM / SBOM scope
//! （蓝图 36 WP-36.4）。所有输出携带相同 manifest_digest/profile_digest。
//!
//! 反解析 Gate：对最终 raw/ISO/update/recovery 逐层反解析，bundle 条目与
//! Binary BOM 一一比对；多余二进制也视为失败（防止 hidden payload）。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::invalid;
use super::product_manifest::{parse_init_bundle, ManifestRole, ProductManifest};
#[cfg(test)]
use super::product_manifest::{Requirement, MANIFEST_SCHEMA};
use super::{read_json, DynResult};

/// init bundle 条目（与 image_factory::compose_init_bundle 的模块格式一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BundleModule {
    pub kind: u32,
    pub flags: u32,
    pub name: String,
    pub package: String,
    pub bin: String,
    pub target: String,
    pub external: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildPlan {
    pub schema: &'static str,
    pub manifest_digest: String,
    pub profile_digest: String,
    pub source_commit: String,
    pub steps: Vec<BuildStep>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildStep {
    pub service: String,
    pub role: String,
    pub package: String,
    pub bin: String,
    pub target: String,
    pub features: Vec<String>,
    pub default_features: bool,
    pub external: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitBundle {
    pub schema: &'static str,
    pub manifest_digest: String,
    pub entries: Vec<BundleModule>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BinaryBom {
    pub schema: &'static str,
    pub manifest_digest: String,
    pub profile_digest: String,
    pub source_commit: String,
    pub binaries: Vec<BomEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomEntry {
    pub service: String,
    pub role: String,
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub in_bundle: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SbomScope {
    pub schema: &'static str,
    pub manifest_digest: String,
    pub profile_digest: String,
    pub source_commit: String,
    pub packages: Vec<String>,
}

/// 拆分 `package::bin` selector。外部 selector（sel4: / external:）无 package::bin。
pub fn split_selector(selector: &str) -> (String, String) {
    selector
        .split_once("::")
        .map(|(p, b)| (p.to_owned(), b.to_owned()))
        .unwrap_or_else(|| (selector.to_owned(), selector.to_owned()))
}

/// 派生 build plan：每个服务一个构建步骤。
pub fn build_plan(manifest: &ProductManifest) -> BuildPlan {
    let mut steps = Vec::new();
    for service in &manifest.services {
        if service.external {
            steps.push(BuildStep {
                service: service.id.clone(),
                role: service.role.as_str().to_owned(),
                package: service.binary.clone(),
                bin: String::new(),
                target: service.target.clone(),
                features: service.features.clone(),
                default_features: service.default_features,
                external: true,
            });
            continue;
        }
        let (package, bin) = split_selector(&service.binary);
        steps.push(BuildStep {
            service: service.id.clone(),
            role: service.role.as_str().to_owned(),
            package,
            bin,
            target: service.target.clone(),
            features: service.features.clone(),
            default_features: service.default_features,
            external: false,
        });
    }
    BuildPlan {
        schema: "eneros.build-plan.v2",
        manifest_digest: manifest.manifest_digest.clone(),
        profile_digest: String::new(),
        source_commit: String::new(),
        steps,
    }
}

/// 派生 init bundle 条目（root + required + optional，按声明顺序）。
/// kind=优先级（1=root, 2=required, 3=optional），flags=criticality 位。
pub fn init_bundle(manifest: &ProductManifest) -> Result<InitBundle, String> {
    let mut entries = Vec::new();
    for service in &manifest.services {
        if !service.role.is_bundled() {
            continue;
        }
        let kind = match service.role {
            ManifestRole::RootTask => 1u32,
            ManifestRole::RequiredService => 2u32,
            ManifestRole::OptionalService => 3u32,
            _ => continue,
        };
        let mut flags = match service.criticality {
            super::product_manifest::Criticality::System => 31u32,
            super::product_manifest::Criticality::Control => 25u32,
            super::product_manifest::Criticality::Data => 20u32,
            super::product_manifest::Criticality::Support => 15u32,
        };
        // WP-37.2：能力位图（flags 高 16 位；与 runtime bundle.rs CAP_*
        // 对应——manifest 校验层已拒绝未知名，此处遇 None 为防御性失败）。
        for cap in &service.capabilities {
            let bit = super::product_manifest::capability_bit(&cap.name)
                .ok_or_else(|| format!("unknown-capability:{}:{}", service.id, cap.name))?;
            flags |= bit;
        }
        let (package, bin) = split_selector(&service.binary);
        entries.push(BundleModule {
            kind,
            flags,
            name: service.id.clone(),
            package,
            bin,
            target: service.target.clone(),
            external: service.external,
        });
    }
    Ok(InitBundle {
        schema: "eneros.init-bundle.v3",
        manifest_digest: manifest.manifest_digest.clone(),
        entries,
    })
}

/// 派生 Binary BOM：构建产物目录中的 ELF + sha256。
/// 反解析 Gate 会用它校验介质中的 bundle 条目。
pub fn binary_bom(
    root: &Path,
    manifest: &ProductManifest,
    built_dir: &Path,
) -> DynResult<BinaryBom> {
    let bound_violations = manifest.validate_bound();
    if !bound_violations.is_empty() {
        return Err(invalid(format!(
            "manifest bound validation failed (build binding required): {}",
            bound_violations.join("; ")
        )));
    }
    let mut binaries = Vec::new();
    for service in &manifest.services {
        if service.external {
            binaries.push(BomEntry {
                service: service.id.clone(),
                role: service.role.as_str().to_owned(),
                path: service.binary.clone(),
                sha256: service.digest.clone(),
                size_bytes: 0,
                in_bundle: service.role.is_bundled(),
            });
            continue;
        }
        let (package, bin) = split_selector(&service.binary);
        let _ = package;
        // 产物布局：target/<target>/release/<bin>（与 image_factory 一致）。
        let target_dir = if service.target.is_empty() {
            built_dir.join("release")
        } else {
            built_dir.join(&service.target).join("release")
        };
        let mut elf_path = target_dir.join(&bin);
        if !elf_path.is_file() {
            // UEFI target 产物带 .efi 后缀（boot-manager/installer）。
            let efi_candidate = target_dir.join(format!("{bin}.efi"));
            if efi_candidate.is_file() {
                elf_path = efi_candidate;
            } else {
                return Err(invalid(format!(
                    "bom-elf-missing:{}:{}",
                    service.id,
                    elf_path.display()
                )));
            }
        }
        let bytes = fs::read(&elf_path)?;
        binaries.push(BomEntry {
            service: service.id.clone(),
            role: service.role.as_str().to_owned(),
            path: elf_path.to_string_lossy().replace('\\', "/"),
            sha256: hex(&Sha256::digest(&bytes)),
            size_bytes: u64::try_from(bytes.len())?,
            in_bundle: service.role.is_bundled(),
        });
    }
    let _ = root;
    Ok(BinaryBom {
        schema: "eneros.binary-bom.v2",
        manifest_digest: manifest.manifest_digest.clone(),
        profile_digest: String::new(),
        source_commit: String::new(),
        binaries,
    })
}

/// SBOM scope：manifest 声明的 cargo package 集合（真实 closure 由
/// cargo metadata resolved graph 提供，见 closure.rs）。
pub fn sbom_scope(manifest: &ProductManifest) -> SbomScope {
    let mut packages = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for service in &manifest.services {
        if service.external {
            continue;
        }
        let (package, _) = split_selector(&service.binary);
        if seen.insert(package.clone()) {
            packages.push(package);
        }
    }
    packages.sort();
    SbomScope {
        schema: "eneros.sbom-scope.v2",
        manifest_digest: manifest.manifest_digest.clone(),
        // WP-36.4：所有派生物携带相同 profile_digest/source_commit；
        // 由 write_derivatives 回填真实值（与 build-plan/binary-bom 一致）。
        profile_digest: String::new(),
        source_commit: String::new(),
        packages,
    }
}

/// 反解析 Gate：校验介质中的 init bundle 条目与 manifest 期望一致。
/// - 每个 manifest bundle 服务必须出现在介质中且 digest 一致；
/// - 介质中多余条目（hidden payload）也判失败；
/// - required 集合与 manifest 完全一致；
/// - manifest 已绑定 service digest 时（bind_digests 后），介质条目 digest
///   必须与 manifest 声明一致（WP-36.4 “与 Binary BOM 一一比对”；36 收尾
///   复验修复：原实现只查 digest 非全零，不做交叉比对）。
pub fn verify_media_bundle(
    manifest: &ProductManifest,
    bundle_bytes: &[u8],
) -> DynResult<Vec<String>> {
    let mut violations = Vec::new();
    let entries = parse_init_bundle(bundle_bytes)?;
    let expected = init_bundle(manifest).expect("capabilities 合法");
    // manifest → 期望 map：name → (kind, flags, bound digest)。
    let mut expected_map: BTreeMap<&str, (&BundleModule, Option<&str>)> = BTreeMap::new();
    for module in &expected.entries {
        let bound = manifest
            .services
            .iter()
            .find(|s| s.id == module.name)
            .map(|s| s.digest.as_str())
            .filter(|d| !d.is_empty());
        expected_map.insert(module.name.as_str(), (module, bound));
    }
    // 介质条目 map。
    let mut media_map: BTreeMap<String, (u32, u32, String)> = BTreeMap::new();
    for (kind, flags, name, digest) in &entries {
        media_map.insert(name.clone(), (*kind, *flags, digest.clone()));
    }
    // 1. required 集合一致：manifest 声明的每个 bundle 服务必须在介质中。
    for module in &expected.entries {
        if let Some((kind, flags, _)) = media_map.get(&module.name) {
            if *kind != module.kind {
                violations.push(format!(
                    "bundle-kind-mismatch:{}:{}!={}",
                    module.name, kind, module.kind
                ));
            }
            if *flags != module.flags {
                violations.push(format!(
                    "bundle-flags-mismatch:{}:{}!={}",
                    module.name, flags, module.flags
                ));
            }
        } else {
            violations.push(format!("bundle-missing:{}", module.name));
        }
    }
    // 2. 多余条目（hidden payload）。
    for (name, (kind, _, _)) in &media_map {
        if !expected_map.contains_key(name.as_str()) {
            violations.push(format!("bundle-extra:{name}:kind={kind}"));
        }
    }
    // 3. media 中每个条目的 digest 必须是真实 64-hex（非空/非全零）——
    //    媒体侧真实绑定；并与 manifest 已绑定的 service digest 交叉比对
    //    （未绑定时 digest 为空则跳过比对，由 validate_bound 在 BOM 阶段兜底）。
    for (name, (_, _, digest)) in &media_map {
        if digest.len() != 64 || digest.chars().all(|c| c == '0') {
            violations.push(format!("bundle-zero-digest:{name}"));
        }
        if let Some((_, Some(bound))) = expected_map.get(name.as_str()) {
            if bound != digest {
                violations.push(format!("bundle-digest-mismatch:{name}"));
            }
        }
    }
    // 4. root-task 必须存在。
    if !media_map.contains_key("root-task") {
        violations.push("bundle-missing:root-task".to_owned());
    }
    Ok(violations)
}

/// 派生物身份回填：build-plan/binary-bom/sbom-scope 三个派生物统一携带
/// 相同的 profile_digest/source_commit（WP-36.4 要求所有输出同源绑定；
/// sbom-scope 曾漏回填，本函数保证三个派生物一致，防回归）。
pub fn fill_derivative_identity(
    plan: &mut BuildPlan,
    bom: &mut BinaryBom,
    scope: &mut SbomScope,
    profile_digest: &str,
    source_commit: &str,
) {
    plan.profile_digest = profile_digest.to_owned();
    plan.source_commit = source_commit.to_owned();
    bom.profile_digest = profile_digest.to_owned();
    bom.source_commit = source_commit.to_owned();
    scope.profile_digest = profile_digest.to_owned();
    scope.source_commit = source_commit.to_owned();
}

/// 写派生物 JSON（target/eneros/manifest/<profile_id>/）。
/// WP-36.4：所有输出携带相同 manifest_digest/profile_digest/source_commit。
pub fn write_derivatives(
    root: &Path,
    manifest: &ProductManifest,
    built_dir: &Path,
) -> DynResult<()> {
    let dir = super::product_manifest::manifest_dir(root, &manifest.profile_id);
    fs::create_dir_all(&dir)?;
    let profile_digest = profile_digest(root, &manifest.profile_id)?;
    let source_commit = source_commit(root)?;
    let mut plan = build_plan(manifest);
    let bundle = init_bundle(manifest).expect("capabilities 合法");
    let mut bom = binary_bom(root, manifest, built_dir)?;
    let mut scope = sbom_scope(manifest);
    fill_derivative_identity(
        &mut plan,
        &mut bom,
        &mut scope,
        &profile_digest,
        &source_commit,
    );
    fs::write(
        dir.join("build-plan.json"),
        serde_json::to_vec_pretty(&plan)?,
    )?;
    fs::write(
        dir.join("init-bundle.json"),
        serde_json::to_vec_pretty(&bundle)?,
    )?;
    fs::write(
        dir.join("binary-bom.json"),
        serde_json::to_vec_pretty(&bom)?,
    )?;
    fs::write(
        dir.join("sbom-scope.json"),
        serde_json::to_vec_pretty(&scope)?,
    )?;
    println!(
        "PRODUCT-DERIVED plan/bundle/bom/sbom-scope → {}/",
        dir.display()
    );
    Ok(())
}

/// 从 raw 镜像提取 init bundle 并反解析（QEMU 布局：BOOTCTRL 分区
/// boot-manager 后跟 kernel + bundle；由 image_factory 布局常量决定偏移）。
/// 为保持单一事实，此函数只支持从已生成的 bundle 文件（target 产物）验证；
/// 介质级反解析由 image_factory 在构建完成后立即调用 verify_media_bundle。
pub fn verify_bundle_file(root: &Path, manifest: &ProductManifest) -> DynResult<()> {
    let path =
        super::product_manifest::manifest_dir(root, &manifest.profile_id).join("init-bundle.bin");
    if !path.is_file() {
        return Err(invalid(format!(
            "bundle file missing for reverse-parse: {}",
            path.display()
        )));
    }
    let bytes = fs::read(&path)?;
    let violations = verify_media_bundle(manifest, &bytes)?;
    if !violations.is_empty() {
        return Err(invalid(format!(
            "REVERSE-PARSE-GATE FAIL: {}",
            violations.join("; ")
        )));
    }
    let entries = parse_init_bundle(&bytes)?;
    println!(
        "REVERSE-PARSE-GATE PASS entries={} (root={})",
        entries.len(),
        entries.iter().any(|(_, _, name, _)| name == "root-task")
    );
    Ok(())
}

/// 加载已有派生 JSON 的辅助。
#[allow(dead_code)] // 44.2：派生 JSON 载入辅助，当前无调用面（44.4 冻结登记）
pub fn load_derived<T: for<'de> serde::Deserialize<'de>>(
    root: &Path,
    profile_id: &str,
    file: &str,
) -> DynResult<T> {
    read_json(&super::product_manifest::manifest_dir(root, profile_id).join(file))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

pub fn profile_digest(root: &Path, profile_id: &str) -> DynResult<String> {
    let path = root
        .join("product/profiles")
        .join(format!("{profile_id}.json"));
    let bytes = fs::read(&path)?;
    Ok(hex(&Sha256::digest(&bytes)))
}

pub fn source_commit(root: &Path) -> DynResult<String> {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Err(invalid("git rev-parse HEAD failed"));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product_manifest::{Criticality, KernelSpec, RestartPolicy, ServiceSpec};

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
                digest: String::new(),
                target: "host".to_owned(),
                features: vec![],
                external: true,
            },
            artifacts: vec![],
            services: vec![
                ServiceSpec {
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
                },
                ServiceSpec {
                    id: "svc-hello".to_owned(),
                    binary: "eneros-services::svc-hello".to_owned(),
                    digest: "bb".to_owned(),
                    role: ManifestRole::RequiredService,
                    target: "aarch64-unknown-none".to_owned(),
                    features: vec![],
                    default_features: false,
                    requirement: Requirement::Required,
                    criticality: Criticality::Support,
                    restart: RestartPolicy::Always,
                    dependencies: vec![],
                    capabilities: vec![],
                    resources: None,
                    configs: vec![],
                    readiness: None,
                    external: false,
                },
            ],
            configs: vec![],
            security: crate::product_manifest::SecurityPolicyRef {
                policy: "product/crypto-policy.toml".to_owned(),
                digest: String::new(),
            },
            evidence_policy: crate::product_manifest::EvidencePolicyRef {
                policy: "product/evidence/schema.json".to_owned(),
                digest: String::new(),
            },
            manifest_digest: String::new(),
        }
    }

    #[test]
    fn init_bundle_derives_kind_by_role() {
        let manifest = sample_manifest();
        let bundle = init_bundle(&manifest).expect("capabilities 合法");
        assert_eq!(bundle.entries.len(), 2);
        assert_eq!(bundle.entries[0].kind, 1);
        assert_eq!(bundle.entries[0].name, "root-task");
        assert_eq!(bundle.entries[1].kind, 2);
    }

    /// WP-37.2：manifest capabilities → bundle flags 高 16 位（能力位图）；
    /// 未知名能力构建失败（fail-closed）。
    #[test]
    fn init_bundle_carries_capability_bits() {
        let mut manifest = sample_manifest();
        // sample_manifest 的服务加能力声明。
        for service in &mut manifest.services {
            service.capabilities = vec![
                crate::product_manifest::CapabilitySpec {
                    name: "ep-send".to_string(),
                    required: true,
                },
                crate::product_manifest::CapabilitySpec {
                    name: "ep-recv".to_string(),
                    required: true,
                },
            ];
        }
        let bundle = init_bundle(&manifest).expect("capabilities 合法");
        for entry in &bundle.entries {
            // ep-send=1<<17 | ep-recv=1<<18 → 高 16 位 = 0x60000。
            assert_eq!(
                entry.flags & 0xFFFF_0000,
                (1 << 17) | (1 << 18),
                "能力位图必须进入 flags 高 16 位: {:08x}",
                entry.flags
            );
            // criticality 低 16 位保持。
            assert!(entry.flags & 0xFFFF != 0);
        }
        // 未知名能力 → 构建失败。
        let mut bad = sample_manifest();
        bad.services[0].capabilities = vec![crate::product_manifest::CapabilitySpec {
            name: "not-a-capability".to_string(),
            required: true,
        }];
        assert!(
            init_bundle(&bad).is_err(),
            "未知名能力必须拒绝（fail-closed）"
        );
    }

    #[test]
    fn build_plan_covers_all_services() {
        let manifest = sample_manifest();
        let plan = build_plan(&manifest);
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].package, "eneros-root-task");
        assert_eq!(plan.steps[0].bin, "eneros-root-task");
    }

    #[test]
    fn verify_media_rejects_extra_module() {
        let manifest = sample_manifest();
        // 构造与 manifest 期望一致 + 一个多余模块的 bundle。
        let mut bytes = vec![0u8; 64 + 96 * 3];
        bytes[..8].copy_from_slice(b"ENRINIT2");
        bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&64u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&3u32.to_le_bytes());
        for (i, (kind, flags, name)) in [
            (1u32, 31u32, "root-task"),
            (2, 15, "svc-hello"),
            (2, 15, "hidden-payload"),
        ]
        .into_iter()
        .enumerate()
        {
            let entry = 64 + i * 96;
            bytes[entry..entry + 4].copy_from_slice(&kind.to_le_bytes());
            bytes[entry + 4..entry + 8].copy_from_slice(&flags.to_le_bytes());
            bytes[entry + 24..entry + 24 + name.len()].copy_from_slice(name.as_bytes());
        }
        let violations = verify_media_bundle(&manifest, &bytes).unwrap();
        assert!(violations.iter().any(|v| v.starts_with("bundle-extra")));
    }

    #[test]
    fn verify_media_rejects_missing_required() {
        let manifest = sample_manifest();
        let mut bytes = vec![0u8; 160];
        bytes[..8].copy_from_slice(b"ENRINIT2");
        bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&64u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&1u32.to_le_bytes());
        let entry = 64;
        bytes[entry..entry + 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[entry + 4..entry + 8].copy_from_slice(&31u32.to_le_bytes());
        bytes[entry + 24..entry + 24 + 9].copy_from_slice(b"root-task");
        let violations = verify_media_bundle(&manifest, &bytes).unwrap();
        assert!(violations
            .iter()
            .any(|v| v.starts_with("bundle-missing:svc-hello")));
    }

    /// 构造与 manifest 双服务对应、内容全空的合法 bundle（条目 offset/len=0）。
    fn bundle_with_zero_len_entries() -> Vec<u8> {
        let mut bytes = vec![0u8; 64 + 96 * 2];
        bytes[..8].copy_from_slice(b"ENRINIT2");
        bytes[8..10].copy_from_slice(&2u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&64u16.to_le_bytes());
        bytes[24..28].copy_from_slice(&2u32.to_le_bytes());
        for (i, (kind, flags, name)) in [(1u32, 31u32, "root-task"), (2, 15, "svc-hello")]
            .into_iter()
            .enumerate()
        {
            let entry = 64 + i * 96;
            bytes[entry..entry + 4].copy_from_slice(&kind.to_le_bytes());
            bytes[entry + 4..entry + 8].copy_from_slice(&flags.to_le_bytes());
            bytes[entry + 24..entry + 24 + name.len()].copy_from_slice(name.as_bytes());
        }
        bytes
    }

    #[test]
    fn verify_media_digest_cross_check() {
        // sha256(空字节)：介质条目 offset/len=0 时的真实 digest。
        let empty_sha = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let bytes = bundle_with_zero_len_entries();
        // 绑定一致 digest → 无 digest 违规（36 收尾复验：交叉比对不误报）。
        let mut manifest = sample_manifest();
        manifest.services[0].digest = empty_sha.to_owned();
        manifest.services[1].digest = empty_sha.to_owned();
        let violations = verify_media_bundle(&manifest, &bytes).unwrap();
        assert!(
            !violations.iter().any(|v| v.contains("digest")),
            "matching digests must not be flagged: {violations:?}"
        );
        // 绑定不一致 digest → bundle-digest-mismatch（hidden-payload/篡改介质）。
        manifest.services[0].digest = "11".repeat(32);
        let violations = verify_media_bundle(&manifest, &bytes).unwrap();
        assert!(
            violations
                .iter()
                .any(|v| v == "bundle-digest-mismatch:root-task"),
            "digest mismatch must be rejected: {violations:?}"
        );
    }

    #[test]
    fn derivative_identity_filled_for_plan_bom_and_sbom_scope() {
        // WP-36.4：build-plan/binary-bom/sbom-scope 三个派生物必须统一
        // 携带相同 profile_digest/source_commit；sbom-scope 曾漏回填，
        // 此测试保证三者的身份字段始终一致（防回归）。
        let mut plan = build_plan(&sample_manifest());
        let mut bom = BinaryBom {
            schema: "eneros.binary-bom.v2",
            manifest_digest: "m".to_owned(),
            profile_digest: String::new(),
            source_commit: String::new(),
            binaries: vec![],
        };
        let mut scope = sbom_scope(&sample_manifest());
        fill_derivative_identity(&mut plan, &mut bom, &mut scope, "pd-abc", "sc-473b1e2");
        for (name, p, s) in [
            ("build-plan", &plan.profile_digest, &plan.source_commit),
            ("binary-bom", &bom.profile_digest, &bom.source_commit),
            ("sbom-scope", &scope.profile_digest, &scope.source_commit),
        ] {
            assert_eq!(p, "pd-abc", "{name} profile_digest must be filled");
            assert_eq!(s, "sc-473b1e2", "{name} source_commit must be filled");
        }
    }
}
