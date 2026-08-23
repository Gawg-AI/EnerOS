use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::product_manifest::ServiceSpec;

mod artifact_inspector;
mod catalog;
mod closure;
mod config_compile;
mod evidence;
mod evidence_chain;
mod factory;
mod fault_factory;
mod flasher;
mod fuzz_driver;
mod gate;
mod harness;
mod image_factory;
mod keys;
mod policy;
mod product_deriver;
mod product_manifest;
mod qa;
mod qemu_harness;
mod sbom;
mod symbolize;
mod test_results;
mod toolchain;
mod trust;
mod update_package;
mod verdict;
mod versioning;
mod vrf002;

type DynResult<T> = Result<T, Box<dyn Error>>;
struct ClosureRecord {
    name: String,
    version: String,
    license: String,
    source: String,
    manifest_path: String,
    workspace_member: bool,
    resolved_features: BTreeSet<String>,
}

const PROFILE_SCHEMA: &str = "eneros.product-profile.v1";
const POLICY_SCHEMA: &str = "eneros.forbidden-policy.v1";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // 44.2：serde 反序列化数据模型，字段由 profile JSON 驱动
struct ProductProfile {
    schema: String,
    id: String,
    class: ProfileClass,
    platform: String,
    target: String,
    image_layout: Option<String>,
    artifacts: Vec<String>,
    binaries: Vec<ProfileBinary>,
    signing: SigningPolicy,
    security: SecurityPolicy,
    // 蓝图 36 profile v2：显式 kernel/services/configs/策略引用。
    #[serde(default)]
    kernel: Option<serde_json::Value>,
    #[serde(default)]
    services: Option<serde_json::Value>,
    #[serde(default)]
    configs: Option<serde_json::Value>,
    #[serde(default)]
    security_policy_ref: Option<serde_json::Value>,
    #[serde(default)]
    evidence_policy_ref: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ProfileClass {
    Test,
    Development,
    Release,
    Recovery,
    Factory,
}

impl ProfileClass {
    fn is_release(self) -> bool {
        matches!(self, Self::Release | Self::Recovery | Self::Factory)
    }

    fn policy_name(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Development => "development",
            Self::Release => "release",
            Self::Recovery => "recovery",
            Self::Factory => "factory",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileBinary {
    selector: String,
    role: String,
    target: Option<String>,
    default_features: bool,
    features: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SigningPolicy {
    required: bool,
    key_source: String,
    allow_test_keys: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecurityPolicy {
    allow_mock: bool,
    allow_unsigned: bool,
    minimum_security_version: u32,
    entropy_sources: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForbiddenPolicy {
    schema: String,
    release_classes: Vec<String>,
    packages: Vec<String>,
    feature_patterns: Vec<String>,
    path_fragments: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageLayout {
    schema: String,
    id: String,
    disk_size_bytes: u64,
    sector_size: u32,
    alignment_bytes: u64,
    disk_guid: String,
    partitions: Vec<LayoutPartition>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayoutPartition {
    role: String,
    name: String,
    type_guid: String,
    size_bytes: u64,
    attributes: u64,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    workspace_members: Vec<String>,
    resolve: CargoResolve,
}

#[derive(Debug, Deserialize)]
struct CargoResolve {
    nodes: Vec<CargoNode>,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    id: String,
    name: String,
    version: String,
    license: Option<String>,
    source: Option<String>,
    manifest_path: PathBuf,
    features: BTreeMap<String, Vec<String>>,
    targets: Vec<CargoTarget>,
}

#[derive(Debug, Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CargoNode {
    id: String,
    features: Vec<String>,
    deps: Vec<CargoDependency>,
}

#[derive(Debug, Deserialize)]
struct CargoDependency {
    pkg: String,
}

#[derive(Debug, Serialize)]
struct AuditReport {
    schema: &'static str,
    result: &'static str,
    profile: String,
    profile_class: String,
    binary_count: usize,
    closure_package_count: usize,
    selected_binaries: Vec<AuditBinaryEvidence>,
    closure_packages: Vec<AuditClosurePackage>,
    forbidden_policy_sha256: String,
    security_policy: AuditSecurityPolicy,
    violations: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AuditBinaryEvidence {
    selector: String,
    target: String,
    default_features: bool,
    requested_features: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AuditClosurePackage {
    id: String,
    name: String,
    version: String,
    license: String,
    source: String,
    manifest_path: String,
    workspace_member: bool,
    resolved_features: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AuditSecurityPolicy {
    signing_required: bool,
    allow_test_keys: bool,
    allow_mock: bool,
    allow_unsigned: bool,
    minimum_security_version: u32,
    entropy_sources: Vec<String>,
}

struct ExactTreePackage {
    name: String,
    version: String,
    features: BTreeSet<String>,
}

#[derive(Debug, Serialize)]
struct LayoutAuditReport {
    schema: &'static str,
    result: &'static str,
    layout: String,
    partition_count: usize,
    disk_size_bytes: u64,
    allocated_bytes: u64,
    violations: Vec<String>,
}

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: FAIL: {error}");
            ExitCode::from(2)
        }
    }
}

fn real_main() -> DynResult<()> {
    let root = workspace_root()?;
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        print_help();
        return Err(invalid("a command is required"));
    };
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "help" | "--help" | "-h" => {
            ensure_empty(&rest, "help")?;
            print_help();
            Ok(())
        }
        "audit" => command_audit(&root, &rest),
        "audit-unsafe-safety" => audit_unsafe_safety(&root, &rest),
        "build" => command_build(&root, &rest),
        "test" => command_test(&root, &rest),
        "catalog" => catalog::command_catalog(&root, &rest),
        "fault" => fault_factory::command_fault(&root, &rest),
        "fuzz" => command_fuzz(&root, &rest),
        "build-compare" => vrf002::command_build_compare(&root),
        "bump-version" => versioning::command_bump_version(&root, &rest),
        "manifest" => command_manifest(&root, &rest),
        "policy" => command_policy(&root, &rest),
        "closure" => closure::command_closure(&root, &rest),
        "config" => command_config(&root, &rest),
        "image" => image_factory::command_image(&root, &rest),
        "run" => qemu_harness::command_run(&root, &rest),
        "inspect" => artifact_inspector::command_inspect(&root, &rest),
        "evidence" => evidence::command_evidence(&root, &rest),
        "evidence42" => evidence_chain::command_evidence_chain(&root, &rest),
        "flash" => flasher::command_flash(&root, &rest),
        "factory" => factory::command_factory(&root, &rest),
        "qa" => qa::command_qa(&root, &rest),
        "release" => sbom::command_release(&root, &rest),
        "symbolize" => symbolize::command_symbolize(&root, &rest),
        other => Err(invalid(format!("unknown command '{other}'"))),
    }
}

fn print_help() {
    println!(
        "EnerOS formal delivery entry\n\
         usage: cargo xtask <command>\n\
         \n\
         accepted now:\n\
           audit all|profile <id>|layout <id>\n\
           audit toolchain ...|trust ...\n\
           build <profile-id>|all\n\
           bump-version patch|minor|major|--set <x.y.z> [--from <ver>] [--dry-run] [--manifest-path <p>]\n\
           test unit|result <json>|gate-semantics|ci-policy\n\
           manifest normalize|derive|registry|verify-bundle --profile <id>\n\
           policy kernel|adr\n\
           closure gate --profile <id>\n\
           image raw|iso|recovery|vars|update ...\n\
           fault manifest|artifact|installer ...\n\
           run qemu-prod|qemu-install ...\n\
           inspect raw|recovery|installer|update ...\n\
           evidence verify|gate [--allow-non-pass] ...\n\
           evidence42 lineage|lineage-check|envelope|verify|gate|gate-dir|report-gate|media-check|fuzz-report|unsafe-report|repro|stale-index|mutation ...\n\
           catalog derive|sign|validate|gate|platform-gate ...\n\
           release sbom|verify ...\n\
           symbolize <crash-dump.bin> <elf-file>\n\
           flash list|verify-inventory|lab-create|authorize-file|write|birth-record|verify-birth|key-domains ...\n\
           qa boot-reset|soak|perf-baseline|perf-measure|perf-gate|trace|proof|validation|g7-audit|interop-report|gate orphan-check|config-refs|mock-leak|docs ...\n\
         \n\
         reserved fail-closed command:\n\
           image factory"
    );
}

/// `manifest normalize --profile <id>`：profile → 规范化 ProductManifest v2。
/// `manifest derive --profile <id>`：生成 build plan/BOM/bundle/SBOM scope。
/// `manifest registry`：写 ServiceKind registry。
/// `manifest verify-bundle --profile <id>`：反解析 Gate（bundle 与 manifest 比对）。
/// `config compile`：签名配置 bundle 生成（蓝图 41 WP-41.2）。
fn command_config(root: &Path, args: &[String]) -> DynResult<()> {
    let mut profile_id = "qemu-prod";
    let mut sign_key_hex: Option<String> = None;
    let mut root_key_hex: Option<String> = None;
    // 子命令形态：`config compile [--profile ...]`。
    let args: Vec<String> = if args.first().map(|s| s.as_str()) == Some("compile") {
        args[1..].to_vec()
    } else {
        args.to_vec()
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--profile" => {
                i += 1;
                profile_id = args.get(i).ok_or_else(|| invalid("--profile 需要值"))?;
            }
            "--sign-key-hex" => {
                i += 1;
                sign_key_hex = Some(
                    args.get(i)
                        .ok_or_else(|| invalid("--sign-key-hex 需要值"))?
                        .clone(),
                );
            }
            "--root-key-hex" => {
                i += 1;
                root_key_hex = Some(
                    args.get(i)
                        .ok_or_else(|| invalid("--root-key-hex 需要值"))?
                        .clone(),
                );
            }
            other => return Err(invalid(format!("config: 未知参数 {other}"))),
        }
        i += 1;
    }
    config_compile::config_compile(
        root,
        profile_id,
        sign_key_hex.as_deref(),
        root_key_hex.as_deref(),
    )?;
    Ok(())
}

fn command_manifest(root: &Path, args: &[String]) -> DynResult<()> {
    let (mode, tail) = args
        .split_first()
        .ok_or_else(|| invalid("manifest requires normalize|derive|registry|verify-bundle"))?;
    match mode.as_str() {
        "normalize" => {
            let profile_id = take_profile_id(tail)?;
            let profile = load_profile(root, &profile_id)?;
            let manifest = product_manifest::normalize_profile(root, &profile)?;
            let path = product_manifest::write_normalized(root, &manifest)?;
            println!(
                "MANIFEST-NORMALIZE profile={profile_id} file={}",
                path.display()
            );
            // profile_digest 绑定。
            let digest = product_deriver::profile_digest(root, &profile_id)?;
            println!("PROFILE-DIGEST {digest}");
            Ok(())
        }
        "derive" => {
            let profile_id = take_profile_id(tail)?;
            let manifest = product_manifest::load_normalized(root, &profile_id)?;
            let built_dir = root.join("target");
            product_deriver::write_derivatives(root, &manifest, &built_dir)?;
            Ok(())
        }
        "registry" => {
            product_manifest::write_service_kind_registry(root)?;
            Ok(())
        }
        "verify-bundle" => {
            let profile_id = take_profile_id(tail)?;
            let manifest = product_manifest::load_normalized(root, &profile_id)?;
            product_deriver::verify_bundle_file(root, &manifest)
        }
        other => Err(invalid(format!("unknown manifest subcommand '{other}'"))),
    }
}

fn take_profile_id(args: &[String]) -> DynResult<String> {
    let mut it = args.iter();
    let mut profile_id = String::new();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--profile" => {
                profile_id = it
                    .next()
                    .ok_or_else(|| invalid("--profile requires a value"))?
                    .clone()
            }
            other => return Err(invalid(format!("unknown manifest arg '{other}'"))),
        }
    }
    if profile_id.is_empty() {
        return Err(invalid("manifest requires --profile <id>"));
    }
    Ok(profile_id)
}

/// `policy kernel|adr`：kernel-policy / ADR registry Gate（fail-closed）。
fn command_policy(root: &Path, args: &[String]) -> DynResult<()> {
    let (mode, _tail) = args
        .split_first()
        .ok_or_else(|| invalid("policy requires kernel|adr"))?;
    match mode.as_str() {
        "kernel" => {
            let violations = policy::load_kernel_policy(root)?.validate();
            if !violations.is_empty() {
                return Err(invalid(format!(
                    "KERNEL-POLICY-GATE FAIL: {}",
                    violations.join("; ")
                )));
            }
            println!(
                "KERNEL-POLICY-GATE PASS product_kernel={}",
                policy::load_kernel_policy(root)?.product_kernel
            );
            Ok(())
        }
        "adr" => {
            let violations = policy::gate_adr_registry(root)?;
            if !violations.is_empty() {
                return Err(invalid(format!(
                    "ADR-REGISTRY-GATE FAIL: {}",
                    violations.join("; ")
                )));
            }
            println!(
                "ADR-REGISTRY-GATE PASS entries={}",
                policy::load_adr_registry(root)?.adrs.len()
            );
            Ok(())
        }
        other => Err(invalid(format!("unknown policy subcommand '{other}'"))),
    }
}

fn command_fuzz(_root: &Path, args: &[String]) -> DynResult<()> {
    if args.len() != 2 {
        return Err("fuzz requires <target> <iterations>".into());
    }
    let target = args[0].clone();
    let iterations: usize = args[1].parse().map_err(|_| "iterations must be usize")?;
    fuzz_driver::command_fuzz(&target, iterations)
}

/// `audit-unsafe-safety`：全仓 unsafe 代码块 SAFETY 注释覆盖率门禁
/// （阶段 C v0.243.0 验收：unsafe 块 SAFETY 注释覆盖率 100%）。
///
/// 规则：
/// - 仅统计**真实代码**中的 `unsafe`（剔除 `//`、`/* */` 注释与字符串）；
/// - 每个 unsafe 行之前 8 行内必须存在 `SAFETY:` / `# Safety` / `# 安全`
///   注释（函数级 `# Safety` 文档覆盖函数体内全部 unsafe 行）；
/// - 文档注释/字符串中出现的 "unsafe" 字样（如"零 unsafe"声明）不计数。
fn audit_unsafe_safety(root: &Path, args: &[String]) -> DynResult<()> {
    ensure_empty(args, "audit-unsafe-safety")?;
    let report = unsafe_audit_report(root)?;
    if report.result != "PASS" {
        eprintln!(
            "audit-unsafe-safety: FAIL ({} 处 unsafe 缺少 SAFETY 注释)",
            report.violations.len()
        );
        for v in &report.violations {
            eprintln!("{v}");
        }
        return Err(invalid(format!(
            "unsafe SAFETY 注释覆盖率未达 100%（{} 处违规）",
            report.violations.len()
        )));
    }
    println!(
        "AUDIT-UNSAFE-SAFETY result=PASS files_with_unsafe={} unsafe_sites={}",
        report.checked_files, report.unsafe_sites
    );
    Ok(())
}

/// 结构化 unsafe/SAFETY 审计报告，供 `audit-unsafe-safety` 与 42
/// `evidence42 unsafe-report` 共用。
#[derive(Debug, serde::Serialize)]
pub(crate) struct UnsafeAuditReport {
    schema: &'static str,
    result: &'static str,
    checked_files: usize,
    unsafe_sites: usize,
    violations: Vec<String>,
}

/// 执行 unsafe/SAFETY 扫描并返回结构化报告（不直接打印/退出）。
pub(crate) fn unsafe_audit_report(root: &Path) -> DynResult<UnsafeAuditReport> {
    let crates_dir = root.join("crates");
    let mut violations: Vec<String> = Vec::new();
    let mut checked_files = 0usize;
    let mut checked_unsafe = 0usize;

    let mut files: Vec<PathBuf> = Vec::new();
    collect_rs_files(&crates_dir, &mut files)?;
    files.sort();

    for file in files {
        let text = fs::read_to_string(&file)?;
        let lines: Vec<&str> = text.lines().collect();
        let mut in_block_comment = false;
        let mut file_has_unsafe = false;
        let mut file_violations: Vec<String> = Vec::new();

        for (idx, line) in lines.iter().enumerate() {
            // 逐字节扫描，跳过注释与字符串中的内容（字节级安全，
            // 不按字符边界切片）。
            let bytes = line.as_bytes();
            let mut i = 0usize;
            let mut code_has_unsafe = false;
            while i < bytes.len() {
                if in_block_comment {
                    if bytes[i..].starts_with(b"*/") {
                        in_block_comment = false;
                        i += 2;
                    } else {
                        i += 1;
                    }
                } else if bytes[i..].starts_with(b"//") {
                    break;
                } else if bytes[i..].starts_with(b"/*") {
                    in_block_comment = true;
                    i += 2;
                } else if bytes[i] == b'"' {
                    // 跳过字符串字面量。
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == b'\\' {
                            i += 2;
                        } else if bytes[i] == b'"' {
                            i += 1;
                            break;
                        } else {
                            i += 1;
                        }
                    }
                } else if bytes[i..].starts_with(b"unsafe") {
                    let before_ok =
                        i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
                    let after_idx = i + 6;
                    let after_ok = after_idx >= bytes.len()
                        || !(bytes[after_idx].is_ascii_alphanumeric() || bytes[after_idx] == b'_');
                    if before_ok && after_ok {
                        code_has_unsafe = true;
                        break;
                    }
                    i += 6;
                } else {
                    i += 1;
                }
            }
            if !code_has_unsafe {
                continue;
            }
            file_has_unsafe = true;
            checked_unsafe += 1;

            // 检查前面 12 行内是否有 SAFETY 注释（覆盖函数级/块级注释）。
            let window_start = idx.saturating_sub(12);
            let covered = lines[window_start..=idx].iter().any(|l| {
                let t = l.trim();
                t.starts_with("// SAFETY")
                    || t.starts_with("//SAFETY")
                    || t.contains("SAFETY:")
                    || t.contains("# Safety")
                    || t.contains("# 安全")
            });
            if !covered {
                let snippet: String = line.trim().chars().take(60).collect();
                file_violations.push(format!(
                    "  {}:{}  `{}` 前 8 行内缺少 SAFETY 注释",
                    file.display(),
                    idx + 1,
                    snippet
                ));
            }
        }
        if file_has_unsafe {
            checked_files += 1;
        }
        violations.extend(file_violations);
    }

    Ok(UnsafeAuditReport {
        schema: "eneros.unsafe-audit.v1",
        result: if violations.is_empty() {
            "PASS"
        } else {
            "FAIL"
        },
        checked_files,
        unsafe_sites: checked_unsafe,
        violations,
    })
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> DynResult<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out)?;
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

fn command_audit(root: &Path, args: &[String]) -> DynResult<()> {
    let (mode, tail) = args
        .split_first()
        .ok_or_else(|| invalid("audit requires 'all' or 'profile <id>'"))?;
    match mode.as_str() {
        "all" => {
            ensure_empty(tail, "audit all")?;
            let profiles = profile_ids(root)?;
            for profile in profiles {
                let report = audit_profile(root, &profile, None)?;
                write_audit_report(root, &report)?;
                println!(
                    "AUDIT profile={} result={} binaries={} closure={}",
                    report.profile,
                    report.result,
                    report.binary_count,
                    report.closure_package_count
                );
                ensure_audit_pass(&report)?;
            }
            for layout in layout_ids(root)? {
                let report = audit_layout(root, &layout)?;
                write_layout_report(root, &report)?;
                println!(
                    "AUDIT layout={} result={} partitions={} allocated={}",
                    report.layout, report.result, report.partition_count, report.allocated_bytes
                );
                ensure_layout_audit_pass(&report)?;
            }
            Ok(())
        }
        "profile" => {
            let (profile_id, injection) = match tail {
                [profile_id] => (profile_id.as_str(), None),
                [profile_id, flag, package] if flag == "--fault-inject-package" => {
                    (profile_id.as_str(), Some(package.as_str()))
                }
                _ => {
                    return Err(invalid(
                        "audit profile requires <id> [--fault-inject-package <package>]",
                    ));
                }
            };
            let report = audit_profile(root, profile_id, injection)?;
            write_audit_report(root, &report)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            ensure_audit_pass(&report)
        }
        "layout" => {
            if tail.len() != 1 {
                return Err(invalid("audit layout requires exactly one layout id"));
            }
            let report = audit_layout(root, &tail[0])?;
            write_layout_report(root, &report)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            ensure_layout_audit_pass(&report)
        }
        "toolchain" => toolchain::command_audit_toolchain(root, tail),
        "trust" => trust::command_audit_trust(root, tail),
        _ => Err(invalid(format!("unknown audit mode '{mode}'"))),
    }
}

fn command_build(root: &Path, args: &[String]) -> DynResult<()> {
    if args.len() != 1 {
        return Err(invalid(
            "build requires exactly one explicit profile id or 'all'",
        ));
    }
    if args[0] == "all" {
        let profiles = profile_ids(root)?;
        if profiles.is_empty() {
            return Err(invalid("no product profiles are declared"));
        }
        for profile in &profiles {
            build_profile(root, profile)?;
        }
        println!("BUILD all result=PASS profiles={}", profiles.len());
        return Ok(());
    }
    build_profile(root, &args[0])
}

fn build_profile(root: &Path, id: &str) -> DynResult<()> {
    let profile = load_profile(root, id)?;
    let report = audit_profile(root, &profile.id, None)?;
    ensure_audit_pass(&report)?;
    for binary in &profile.binaries {
        build_binary(root, &profile, binary)?;
    }
    write_audit_report(root, &report)?;
    println!("BUILD profile={} result=PASS", profile.id);
    Ok(())
}

fn command_test(root: &Path, args: &[String]) -> DynResult<()> {
    match args {
        [suite] if suite == "unit" => {
            run_checked(Command::new(cargo_program()).current_dir(root).args([
                "test",
                "--locked",
                "-p",
                "eneros-xtask",
            ]))
        }
        [mode, path] if mode == "result" => test_results::command_result(Path::new(path)),
        [suite] if suite == "gate-semantics" => test_results::command_gate_semantics(),
        [suite] if suite == "ci-policy" => test_results::command_ci_policy(root),
        _ => Err(invalid(
            "test requires unit, result <json>, gate-semantics, or ci-policy",
        )),
    }
}

fn build_binary(root: &Path, profile: &ProductProfile, binary: &ProfileBinary) -> DynResult<()> {
    // seL4 迁移（ADR-0023）：sel4: 前缀表示内核/引导镜像由 third_party/sel4-build.sh
    // （CMake/ninja）构建，不在 cargo workspace 内，跳过 cargo 构建。
    if let Some(artifact) = binary.selector.strip_prefix("sel4:") {
        let path = root.join("target/sel4").join(artifact);
        if !path.is_file() {
            return Err(invalid(format!(
                "sel4 artifact missing: {}（请先运行 third_party/sel4-build.sh）",
                path.display()
            )));
        }
        return Ok(());
    }
    let (package, target) = split_selector(&binary.selector)?;
    let mut command = Command::new(cargo_program());
    command
        .current_dir(root)
        .env(
            "ENEROS_MINIMUM_SECURITY_VERSION",
            profile.security.minimum_security_version.to_string(),
        )
        .args([
            "build",
            "--locked",
            "--offline",
            "-p",
            package,
            "--bin",
            target,
        ]);
    let build_target = binary.target.as_deref().unwrap_or(&profile.target);
    if build_target != "host" {
        command.args(["--target", build_target]);
        // 无预编译 rust-std 的 tier-2/3 目标：从源码构建 core/alloc
        // （RUSTC_BOOTSTRAP=1 下 stable cargo 接受 -Z build-std）。
        command.args([
            "-Z",
            "build-std=core,alloc",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ]);
    }
    if profile.class.is_release() {
        command.arg("--release");
    }
    if !binary.default_features {
        command.arg("--no-default-features");
    }
    if !binary.features.is_empty() {
        command.args(["--features", &binary.features.join(",")]);
    }
    run_checked(&mut command)
}

fn audit_profile(
    root: &Path,
    id: &str,
    fault_inject_package: Option<&str>,
) -> DynResult<AuditReport> {
    let profile = load_profile(root, id)?;
    validate_profile(&profile)?;
    let policy_path = root.join("product/policy/forbidden.json");
    let policy_bytes = fs::read(&policy_path)?;
    let policy: ForbiddenPolicy = serde_json::from_slice(&policy_bytes)?;
    if policy.schema != POLICY_SCHEMA {
        return Err(invalid(format!(
            "unsupported forbidden policy schema '{}'",
            policy.schema
        )));
    }
    let policy_marks_release = policy
        .release_classes
        .iter()
        .any(|class| class == profile.class.policy_name());
    if policy_marks_release != profile.class.is_release() {
        return Err(invalid(format!(
            "profile class '{}' disagrees with forbidden policy release_classes",
            profile.class.policy_name()
        )));
    }
    let metadata = cargo_metadata(root)?;
    let workspace: BTreeSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();
    let package_by_id: BTreeMap<&str, &CargoPackage> = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    let node_by_id: BTreeMap<&str, &CargoNode> = metadata
        .resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut binary_roots: BTreeMap<String, &str> = BTreeMap::new();
    for package in &metadata.packages {
        if !workspace.contains(package.id.as_str()) {
            continue;
        }
        for target in &package.targets {
            if target.kind.iter().any(|kind| kind == "bin") {
                binary_roots.insert(
                    format!("{}::{}", package.name, target.name),
                    package.id.as_str(),
                );
            }
        }
    }

    let mut violations = Vec::new();
    // WP-44.3：release profile 服务清单 ⊆ service-kind-registry 产品 kinds
    // （测试探针等未登记服务进 release 即 FAIL；test/development 类豁免）。
    if profile.class.is_release() {
        if let Some(services_value) = &profile.services {
            let services: Vec<ServiceSpec> = serde_json::from_value(services_value.clone())
                .map_err(|e| invalid(format!("profile services parse: {e}")))?;
            violations.extend(product_manifest::validate_service_kinds(&services));
        }
    }
    let mut closure_records: BTreeMap<String, ClosureRecord> = BTreeMap::new();
    let canonical_root = root.canonicalize()?;
    for binary in &profile.binaries {
        let Some(root_id) = binary_roots.get(&binary.selector) else {
            if binary.selector.starts_with("sel4:") {
                // seL4 产物（ADR-0023）不在 cargo metadata；由 sel4-build.sh 产出。
                continue;
            }
            violations.push(format!("binary-not-in-cargo-metadata:{}", binary.selector));
            continue;
        };
        let package = package_by_id
            .get(root_id)
            .ok_or_else(|| invalid(format!("binary package missing: {}", binary.selector)))?;
        for feature in &binary.features {
            if !package.features.contains_key(feature) {
                violations.push(format!(
                    "undeclared-requested-feature:{}:{}",
                    binary.selector, feature
                ));
            }
        }
        let exact_catalog =
            cargo_metadata_for_binary(root, &package.manifest_path, &profile, binary)?;
        for exact in cargo_tree_for_binary(root, &package.manifest_path, &profile, binary)? {
            let matches = exact_catalog
                .packages
                .iter()
                .filter(|candidate| {
                    candidate.name == exact.name && candidate.version == exact.version
                })
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(invalid(format!(
                    "exact tree package identity is ambiguous: {}@{} matches={}",
                    exact.name,
                    exact.version,
                    matches.len()
                )));
            }
            record_audited_package(
                &canonical_root,
                &profile,
                &policy,
                matches[0],
                &exact.features,
                &mut closure_records,
                &mut violations,
            )?;
        }
    }

    if let Some(name) = fault_inject_package {
        let matches: Vec<&CargoPackage> = metadata
            .packages
            .iter()
            .filter(|package| package.name == name)
            .collect();
        if matches.len() != 1 {
            return Err(invalid(format!(
                "fault injection package must resolve uniquely, got {} matches for '{name}'",
                matches.len()
            )));
        }
        let mut injected_closure = BTreeSet::new();
        collect_closure(&matches[0].id, &node_by_id, &mut injected_closure)?;
        record_audited_closure(
            &canonical_root,
            &profile,
            &policy,
            &injected_closure,
            &package_by_id,
            &node_by_id,
            &mut closure_records,
            &mut violations,
        )?;
    }

    if profile.class.is_release() {
        for binary in &profile.binaries {
            for feature in &binary.features {
                if policy
                    .feature_patterns
                    .iter()
                    .any(|pattern| glob_matches(pattern, feature))
                {
                    violations.push(format!(
                        "forbidden-requested-feature:{}:{}",
                        binary.selector, feature
                    ));
                }
            }
        }
    }
    // WP-44.4：冻结 crate 门禁（release 类）——R1 冻结 crate 不得进入
    // release 依赖闭包（豁免除外）；R2 非冻结 crate 不得声明依赖冻结 crate
    // （preexisting 边在 allowlist 内豁免）。
    if profile.class.is_release() {
        if let Ok(frozen_policy) = policy::load_frozen_policy(root) {
            let frozen: BTreeSet<String> = frozen_policy
                .crates
                .iter()
                .map(|c| c.package.clone())
                .collect();
            let exempt: BTreeSet<String> = frozen_policy
                .release_closure_exemptions
                .iter()
                .map(|e| e.package.clone())
                .collect();
            let allow_edges: BTreeSet<(String, String)> = frozen_policy
                .dependency_allowlist
                .iter()
                .map(|e| (e.from.clone(), e.to.clone()))
                .collect();
            let reached: BTreeSet<String> = closure_records
                .values()
                .map(|record| record.name.clone())
                .collect();
            violations.extend(policy::gate_frozen_release_closure(
                &frozen, &exempt, &reached,
            ));
            let mut edges: Vec<(String, String)> = Vec::new();
            for node in &metadata.resolve.nodes {
                let Some(from_pkg) = package_by_id.get(node.id.as_str()) else {
                    continue;
                };
                if !workspace.contains(from_pkg.id.as_str()) || frozen.contains(&from_pkg.name) {
                    continue;
                }
                for dep in &node.deps {
                    let Some(to_pkg) = package_by_id.get(dep.pkg.as_str()) else {
                        continue;
                    };
                    if frozen.contains(&to_pkg.name) {
                        edges.push((from_pkg.name.clone(), to_pkg.name.clone()));
                    }
                }
            }
            violations.extend(policy::gate_frozen_dependencies(
                &frozen,
                &allow_edges,
                &edges,
            ));
        } else {
            violations.push("frozen-policy-unreadable".to_owned());
        }
    }
    violations.sort();
    violations.dedup();
    let result = if violations.is_empty() {
        "PASS"
    } else {
        "FAIL"
    };
    let closure_packages = closure_records
        .into_iter()
        .map(|(id, record)| AuditClosurePackage {
            id,
            name: record.name,
            version: record.version,
            license: record.license,
            source: record.source,
            manifest_path: record.manifest_path,
            workspace_member: record.workspace_member,
            resolved_features: record.resolved_features.into_iter().collect(),
        })
        .collect::<Vec<_>>();
    let report = AuditReport {
        schema: "eneros.xtask.profile-audit.v1",
        result,
        profile: profile.id,
        profile_class: format!("{:?}", profile.class).to_ascii_lowercase(),
        binary_count: profile.binaries.len(),
        closure_package_count: closure_packages.len(),
        selected_binaries: profile
            .binaries
            .iter()
            .map(|binary| AuditBinaryEvidence {
                selector: binary.selector.clone(),
                target: binary
                    .target
                    .clone()
                    .unwrap_or_else(|| profile.target.clone()),
                default_features: binary.default_features,
                requested_features: binary.features.clone(),
            })
            .collect(),
        closure_packages,
        forbidden_policy_sha256: hex(&Sha256::digest(&policy_bytes)),
        security_policy: AuditSecurityPolicy {
            signing_required: profile.signing.required,
            allow_test_keys: profile.signing.allow_test_keys,
            allow_mock: profile.security.allow_mock,
            allow_unsigned: profile.security.allow_unsigned,
            minimum_security_version: profile.security.minimum_security_version,
            entropy_sources: profile.security.entropy_sources.clone(),
        },
        violations,
    };
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn record_audited_closure(
    canonical_root: &Path,
    profile: &ProductProfile,
    policy: &ForbiddenPolicy,
    closure: &BTreeSet<String>,
    package_by_id: &BTreeMap<&str, &CargoPackage>,
    node_by_id: &BTreeMap<&str, &CargoNode>,
    records: &mut BTreeMap<String, ClosureRecord>,
    violations: &mut Vec<String>,
) -> DynResult<()> {
    for id in closure {
        let package = package_by_id
            .get(id.as_str())
            .ok_or_else(|| invalid(format!("closure package missing from metadata: {id}")))?;
        let node = node_by_id
            .get(id.as_str())
            .ok_or_else(|| invalid(format!("closure node missing: {id}")))?;
        record_audited_package(
            canonical_root,
            profile,
            policy,
            package,
            &node.features.iter().cloned().collect(),
            records,
            violations,
        )?;
    }
    Ok(())
}

fn record_audited_package(
    canonical_root: &Path,
    profile: &ProductProfile,
    policy: &ForbiddenPolicy,
    package: &CargoPackage,
    features: &BTreeSet<String>,
    records: &mut BTreeMap<String, ClosureRecord>,
    violations: &mut Vec<String>,
) -> DynResult<()> {
    let canonical_manifest = package.manifest_path.canonicalize()?;
    let relative_manifest = canonical_manifest.strip_prefix(canonical_root);
    let workspace_member = relative_manifest.is_ok();
    let manifest_path = relative_manifest
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| "external".to_owned());
    let license = package
        .license
        .clone()
        .unwrap_or_else(|| "NOASSERTION".to_owned());
    let source = package
        .source
        .clone()
        .unwrap_or_else(|| "workspace-or-path".to_owned());
    let record = records
        .entry(package.id.clone())
        .or_insert_with(|| ClosureRecord {
            name: package.name.clone(),
            version: package.version.clone(),
            license,
            source,
            manifest_path,
            workspace_member,
            resolved_features: BTreeSet::new(),
        });
    record.resolved_features.extend(features.iter().cloned());

    if profile.class.is_release() {
        if policy.packages.iter().any(|name| name == &package.name) {
            violations.push(format!("forbidden-package:{}", package.name));
        }
        let manifest = package.manifest_path.to_string_lossy().replace('\\', "/");
        for fragment in &policy.path_fragments {
            if manifest
                .to_ascii_lowercase()
                .contains(&fragment.to_ascii_lowercase())
            {
                violations.push(format!("forbidden-path:{}:{}", package.name, fragment));
            }
        }
        for feature in features {
            if policy
                .feature_patterns
                .iter()
                .any(|pattern| glob_matches(pattern, feature))
            {
                violations.push(format!(
                    "forbidden-resolved-feature:{}:{}",
                    package.name, feature
                ));
            }
        }
    }
    Ok(())
}

fn ensure_audit_pass(report: &AuditReport) -> DynResult<()> {
    if report.result == "PASS" {
        Ok(())
    } else {
        Err(invalid(format!(
            "profile '{}' failed: {}",
            report.profile,
            report.violations.join("; ")
        )))
    }
}

fn collect_closure(
    root: &str,
    nodes: &BTreeMap<&str, &CargoNode>,
    closure: &mut BTreeSet<String>,
) -> DynResult<()> {
    let mut pending = vec![root.to_owned()];
    while let Some(id) = pending.pop() {
        if !closure.insert(id.clone()) {
            continue;
        }
        let node = nodes
            .get(id.as_str())
            .ok_or_else(|| invalid(format!("dependency node is missing: {id}")))?;
        for dependency in &node.deps {
            pending.push(dependency.pkg.clone());
        }
    }
    Ok(())
}

fn cargo_metadata(root: &Path) -> DynResult<CargoMetadata> {
    let output = Command::new(cargo_program())
        .current_dir(root)
        .args(["metadata", "--locked", "--offline", "--format-version", "1"])
        .output()?;
    if !output.status.success() {
        return Err(invalid(format!(
            "cargo metadata --locked failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn cargo_metadata_for_binary(
    root: &Path,
    manifest_path: &Path,
    profile: &ProductProfile,
    binary: &ProfileBinary,
) -> DynResult<CargoMetadata> {
    let mut command = Command::new(cargo_program());
    command.current_dir(root).args([
        "metadata",
        "--locked",
        "--offline",
        "--format-version",
        "1",
        "--manifest-path",
    ]);
    command.arg(manifest_path);
    let target = binary.target.as_deref().unwrap_or(&profile.target);
    if target != "host" {
        command.args(["--filter-platform", target]);
    }
    if !binary.default_features {
        command.arg("--no-default-features");
    }
    if !binary.features.is_empty() {
        command.args(["--features", &binary.features.join(",")]);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(invalid(format!(
            "exact cargo metadata failed for {}: {}",
            binary.selector,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn cargo_tree_for_binary(
    root: &Path,
    manifest_path: &Path,
    profile: &ProductProfile,
    binary: &ProfileBinary,
) -> DynResult<Vec<ExactTreePackage>> {
    let (package, _) = split_selector(&binary.selector)?;
    let mut command = Command::new(cargo_program());
    command
        .current_dir(root)
        .args(["tree", "--locked", "--offline", "--manifest-path"]);
    command.arg(manifest_path);
    command.args([
        "-p",
        package,
        "-e",
        "normal,build",
        "--prefix",
        "none",
        "--charset",
        "ascii",
        "--format",
        "{p}|{f}",
    ]);
    let target = binary.target.as_deref().unwrap_or(&profile.target);
    if target != "host" {
        command.args(["--target", target]);
    }
    if !binary.default_features {
        command.arg("--no-default-features");
    }
    if !binary.features.is_empty() {
        command.args(["--features", &binary.features.join(",")]);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(invalid(format!(
            "exact cargo tree failed for {}: {}",
            binary.selector,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mut packages: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for line in String::from_utf8(output.stdout)?.lines() {
        let line = line.strip_suffix(" (*)").unwrap_or(line);
        let (identity, features) = line
            .split_once('|')
            .ok_or_else(|| invalid(format!("cargo tree line has no feature separator: {line}")))?;
        let mut words = identity.split_ascii_whitespace();
        let name = words
            .next()
            .ok_or_else(|| invalid("cargo tree package name is empty"))?;
        let version = words
            .next()
            .and_then(|word| word.strip_prefix('v'))
            .ok_or_else(|| invalid(format!("cargo tree version is invalid: {identity}")))?;
        let feature_set = packages
            .entry((name.to_owned(), version.to_owned()))
            .or_default();
        feature_set.extend(
            features
                .split(',')
                .filter(|feature| !feature.is_empty())
                .map(str::to_owned),
        );
    }
    if packages.is_empty() {
        return Err(invalid(format!(
            "exact cargo tree is empty for {}",
            binary.selector
        )));
    }
    Ok(packages
        .into_iter()
        .map(|((name, version), features)| ExactTreePackage {
            name,
            version,
            features,
        })
        .collect())
}

fn load_profile(root: &Path, id: &str) -> DynResult<ProductProfile> {
    if !is_profile_id(id) {
        return Err(invalid(format!("invalid profile id '{id}'")));
    }
    let path = root.join("product/profiles").join(format!("{id}.json"));
    if !path.is_file() {
        return Err(invalid(format!("unknown profile '{id}'")));
    }
    let profile: ProductProfile = read_json(&path)?;
    if profile.id != id {
        return Err(invalid(format!(
            "profile id '{}' does not match file id '{id}'",
            profile.id
        )));
    }
    Ok(profile)
}

fn validate_profile(profile: &ProductProfile) -> DynResult<()> {
    // 36：接受 v1 与 v2；v1 由 normalize_profile 按旧语义派生。
    if profile.schema != PROFILE_SCHEMA && profile.schema != product_manifest::PROFILE_SCHEMA_V2 {
        return Err(invalid(format!(
            "unsupported profile schema '{}' (expected v1 or v2)",
            profile.schema
        )));
    }
    if profile.platform.is_empty() || profile.target.is_empty() {
        return Err(invalid("profile platform and target must be explicit"));
    }
    if profile.artifacts.is_empty() || profile.binaries.is_empty() {
        return Err(invalid("profile artifacts and binaries must be non-empty"));
    }
    let needs_image_layout = profile.artifacts.iter().any(|artifact| {
        matches!(
            artifact.as_str(),
            "raw-gpt" | "installer-iso" | "recovery-image" | "firmware-bundle"
        )
    });
    if needs_image_layout && profile.image_layout.as_deref().unwrap_or("").is_empty() {
        return Err(invalid(
            "profile with media artifacts must name an explicit image_layout",
        ));
    }
    let mut selectors = BTreeSet::new();
    for binary in &profile.binaries {
        // seL4 迁移（ADR-0023）：sel4: 前缀的 selector 由 third_party/sel4-build.sh
        // 产出，不在 cargo workspace 内（audit_profile 见 554 行同款豁免）。
        if !binary.selector.starts_with("sel4:") {
            split_selector(&binary.selector)?;
        }
        if binary.role.is_empty() {
            return Err(invalid(format!(
                "binary role is empty: {}",
                binary.selector
            )));
        }
        if !selectors.insert(binary.selector.as_str()) {
            return Err(invalid(format!(
                "duplicate binary selector: {}",
                binary.selector
            )));
        }
        let feature_count = binary.features.iter().collect::<BTreeSet<_>>().len();
        if feature_count != binary.features.len() {
            return Err(invalid(format!(
                "duplicate binary feature: {}",
                binary.selector
            )));
        }
    }
    if profile.class.is_release() {
        if !profile.signing.required || profile.signing.key_source == "none" {
            return Err(invalid(
                "release profile must require an external signing source",
            ));
        }
        if profile.signing.allow_test_keys {
            return Err(invalid("release profile cannot allow test keys"));
        }
        if profile.security.allow_mock || profile.security.allow_unsigned {
            return Err(invalid(
                "release profile cannot allow Mock or unsigned fallback",
            ));
        }
        if profile.security.minimum_security_version == 0 {
            return Err(invalid("release profile security version must be non-zero"));
        }
        if profile.security.entropy_sources.is_empty() {
            return Err(invalid("release profile must declare an entropy source"));
        }
    }
    Ok(())
}

fn profile_ids(root: &Path) -> DynResult<Vec<String>> {
    let mut ids = Vec::new();
    for entry in fs::read_dir(root.join("product/profiles"))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension() == Some(OsStr::new("json"))
            && path.file_stem() != Some(OsStr::new("schema"))
        {
            ids.push(path.file_stem().unwrap().to_string_lossy().into_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

fn layout_ids(root: &Path) -> DynResult<Vec<String>> {
    let mut ids = Vec::new();
    for entry in fs::read_dir(root.join("product/image-layouts"))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension() == Some(OsStr::new("json"))
            && path.file_stem() != Some(OsStr::new("schema"))
        {
            ids.push(path.file_stem().unwrap().to_string_lossy().into_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

fn audit_layout(root: &Path, id: &str) -> DynResult<LayoutAuditReport> {
    if !is_profile_id(id) {
        return Err(invalid(format!("invalid layout id '{id}'")));
    }
    let path = root
        .join("product/image-layouts")
        .join(format!("{id}.json"));
    if !path.is_file() {
        return Err(invalid(format!("unknown image layout '{id}'")));
    }
    let layout: ImageLayout = read_json(&path)?;
    let mut violations = Vec::new();
    if layout.schema != "eneros.image-layout.v1" {
        violations.push(format!("unsupported-schema:{}", layout.schema));
    }
    if layout.id != id {
        violations.push(format!("layout-id-filename-mismatch:{}", layout.id));
    }
    if !matches!(layout.sector_size, 512 | 4096)
        || layout.disk_size_bytes % u64::from(layout.sector_size) != 0
    {
        violations.push("invalid-sector-geometry".to_owned());
    }
    if layout.alignment_bytes < 4096
        || layout.alignment_bytes > 2 * 1024 * 1024
        || !layout.alignment_bytes.is_power_of_two()
        || layout.alignment_bytes % u64::from(layout.sector_size) != 0
    {
        violations.push("invalid-alignment".to_owned());
    }
    if eneros_image_format::Guid::parse(&layout.disk_guid).is_err() {
        violations.push("invalid-disk-guid".to_owned());
    }
    let required_roles: BTreeSet<&str> = [
        "PLATFORM_BOOT",
        "BOOTCTRL",
        "BOOT_A",
        "BOOT_B",
        "SYS_A",
        "SYS_B",
        "RECOVERY",
        "CONFIG",
        "DATA",
        "LOGS",
        "MODELS",
    ]
    .into_iter()
    .collect();
    let mut roles = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut type_guids = BTreeSet::new();
    let mut allocated_bytes = 0u64;
    for partition in &layout.partitions {
        if !required_roles.contains(partition.role.as_str()) {
            violations.push(format!("unknown-role:{}", partition.role));
        }
        if !roles.insert(partition.role.as_str()) {
            violations.push(format!("duplicate-role:{}", partition.role));
        }
        if partition.name != partition.role {
            violations.push(format!(
                "name-role-mismatch:{}:{}",
                partition.role, partition.name
            ));
        }
        if !names.insert(partition.name.as_str()) {
            violations.push(format!("duplicate-name:{}", partition.name));
        }
        if eneros_image_format::Guid::parse(&partition.type_guid).is_err() {
            violations.push(format!("invalid-type-guid:{}", partition.role));
        }
        if !type_guids.insert(partition.type_guid.as_str()) {
            violations.push(format!("duplicate-type-guid:{}", partition.type_guid));
        }
        if partition.size_bytes == 0
            || partition.size_bytes % layout.alignment_bytes != 0
            || partition.size_bytes % u64::from(layout.sector_size) != 0
        {
            violations.push(format!("invalid-size-or-alignment:{}", partition.role));
        }
        if partition.attributes & !1 != 0 {
            violations.push(format!("unknown-partition-attributes:{}", partition.role));
        }
        allocated_bytes = match allocated_bytes.checked_add(partition.size_bytes) {
            Some(value) => value,
            None => {
                violations.push("partition-size-overflow".to_owned());
                u64::MAX
            }
        };
    }
    for role in required_roles.difference(&roles) {
        violations.push(format!("missing-role:{role}"));
    }
    let required_disk_bytes = layout
        .alignment_bytes
        .checked_add(allocated_bytes)
        .and_then(|value| value.checked_add(layout.alignment_bytes));
    if required_disk_bytes.map_or(true, |required| required > layout.disk_size_bytes) {
        violations.push("layout-exceeds-disk-capacity".to_owned());
    }
    violations.sort();
    violations.dedup();
    Ok(LayoutAuditReport {
        schema: "eneros.xtask.layout-audit.v1",
        result: if violations.is_empty() {
            "PASS"
        } else {
            "FAIL"
        },
        layout: layout.id,
        partition_count: layout.partitions.len(),
        disk_size_bytes: layout.disk_size_bytes,
        allocated_bytes,
        violations,
    })
}

fn ensure_layout_audit_pass(report: &LayoutAuditReport) -> DynResult<()> {
    if report.result == "PASS" {
        Ok(())
    } else {
        Err(invalid(format!(
            "layout '{}' failed: {}",
            report.layout,
            report.violations.join("; ")
        )))
    }
}

fn write_audit_report(root: &Path, report: &AuditReport) -> DynResult<()> {
    let directory = root.join("target/eneros/evidence/BASE-006");
    fs::create_dir_all(&directory)?;
    let bytes = serde_json::to_vec_pretty(report)?;
    fs::write(
        directory.join(format!("profile-{}.json", report.profile)),
        bytes,
    )?;
    Ok(())
}

fn write_layout_report(root: &Path, report: &LayoutAuditReport) -> DynResult<()> {
    let directory = root.join("target/eneros/evidence/IMG-001");
    fs::create_dir_all(&directory)?;
    let bytes = serde_json::to_vec_pretty(report)?;
    fs::write(
        directory.join(format!("layout-{}.json", report.layout)),
        bytes,
    )?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> DynResult<T> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn run_checked(command: &mut Command) -> DynResult<()> {
    eprintln!("+ {:?}", command);
    let status = command.status()?;
    if !status.success() {
        return Err(invalid(format!("child command failed with {status}")));
    }
    Ok(())
}

fn split_selector(selector: &str) -> DynResult<(&str, &str)> {
    let Some((package, target)) = selector.split_once("::") else {
        return Err(invalid(format!("invalid binary selector '{selector}'")));
    };
    if package.is_empty() || target.is_empty() || target.contains("::") {
        return Err(invalid(format!("invalid binary selector '{selector}'")));
    }
    Ok((package, target))
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    match pattern.split_once('*') {
        None => pattern == value,
        Some((prefix, suffix)) => value.starts_with(prefix) && value.ends_with(suffix),
    }
}

fn workspace_root() -> DynResult<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| invalid("xtask manifest has no workspace parent"))?;
    Ok(root.to_path_buf())
}

fn cargo_program() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

fn ensure_empty(args: &[String], command: &str) -> DynResult<()> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(invalid(format!("{command} takes no additional arguments")))
    }
}

fn is_profile_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && value.len() <= 32
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn invalid(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_release_json() -> &'static str {
        r#"{
          "schema":"eneros.product-profile.v1",
          "id":"qemu-prod",
          "class":"release",
          "platform":"qemu-virt-aarch64",
          "target":"aarch64-unknown-none",
          "image_layout":"qemu-v1",
          "artifacts":["raw-gpt"],
          "binaries":[{"selector":"p::b","role":"kernel","default_features":false,"features":[]}],
          "signing":{"required":true,"key_source":"external-file","allow_test_keys":false},
          "security":{"allow_mock":false,"allow_unsigned":false,"minimum_security_version":1,"entropy_sources":["virtio-rng"]}
        }"#
    }

    #[test]
    fn release_profile_with_sel4_selector_is_accepted() {
        // ADR-0023：sel4: 前缀 selector 由 third_party/sel4-build.sh 产出，
        // 不在 cargo workspace 内，validate_profile 必须豁免（P0-28 build all 前置）。
        let json = valid_release_json().replace(
            r#""selector":"p::b""#,
            r#""selector":"sel4:elfloader-arm-qemu-arm-virt""#,
        );
        let profile: ProductProfile = serde_json::from_str(&json).unwrap();
        validate_profile(&profile).unwrap();
    }

    #[test]
    fn release_profile_without_default_features_is_rejected() {
        // P0-28：profile binaries 必须显式声明 default_features（audit all 曾因此必失败）。
        let json = valid_release_json().replace(
            r#""selector":"p::b","role":"kernel","default_features":false,"features":[]"#,
            r#""selector":"p::b","role":"kernel","features":[]"#,
        );
        assert!(serde_json::from_str::<ProductProfile>(&json).is_err());
    }

    #[test]
    fn valid_release_profile_is_accepted() {
        let profile: ProductProfile = serde_json::from_str(valid_release_json()).unwrap();
        validate_profile(&profile).unwrap();
    }

    #[test]
    fn unknown_profile_field_is_rejected() {
        let invalid = valid_release_json().replace(
            "\"platform\":\"qemu-virt-aarch64\"",
            "\"platform\":\"qemu-virt-aarch64\",\"unexpected\":true",
        );
        assert!(serde_json::from_str::<ProductProfile>(&invalid).is_err());
    }

    #[test]
    fn release_mock_is_rejected() {
        let invalid = valid_release_json().replace("\"allow_mock\":false", "\"allow_mock\":true");
        let profile: ProductProfile = serde_json::from_str(&invalid).unwrap();
        assert!(validate_profile(&profile).is_err());
    }

    #[test]
    fn release_test_key_is_rejected() {
        let invalid =
            valid_release_json().replace("\"allow_test_keys\":false", "\"allow_test_keys\":true");
        let profile: ProductProfile = serde_json::from_str(&invalid).unwrap();
        assert!(validate_profile(&profile).is_err());
    }

    #[test]
    fn glob_policy_is_bounded() {
        assert!(glob_matches("mock-*", "mock-storage"));
        assert!(!glob_matches("mock-*", "production-mock-storage"));
        assert!(glob_matches("sel4-compat", "sel4-compat"));
    }
}
