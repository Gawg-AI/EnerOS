//! Release 依赖闭包真实性 Gate（蓝图 36 WP-36.6）。
//!
//! 对 ProductManifest 每个二进制执行 `cargo tree -e normal,build`（目标
//! target 过滤），从依赖树计算每个 package 的 feature 激活与来源路径。
//! deny class：mock/fake/stub/simulated/null/test-support/host-file/host-os。
//! 允许项必须来自精确 allowlist（带到期时间、owner、理由）。
//!
//! 说明：cargo metadata 的 `node.features` 受 workspace feature unification
//! 污染（dev 边/其它 target 的启用会全局出现），且 dep edge 不携带 features；
//! 因此本 Gate 以 `cargo tree -e normal,build` 的精确 release 视图为准，
//! dev 依赖与 host 测试 feature 不进入 AArch64 release 闭包判定。
//!
//! 错误格式（蓝图 36 要求）：
//! ```text
//! RELEASE_FEATURE_DENIED
//! binary=svc-protocol-modbus-rtu
//! package=eneros-rs485
//! feature=mock
//! path=svc -> eneros-modbus-rtu -> eneros-rs485[mock]
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::invalid;
use super::product_deriver::split_selector;
use super::product_manifest::ProductManifest;
use super::{policy, DynResult};

/// deny class 关键词（feature 名或 package 名命中即拒绝，除非 allowlist）。
pub const DENY_FEATURE_PATTERNS: &[&str] = &[
    "mock",
    "fake",
    "stub",
    "simulated",
    "sim",
    "null",
    "test-support",
    "test_support",
    "host-file",
    "host_file",
    "host-os",
    "host_os",
    "insecure-test",
    "test-rng",
];

/// 精确 allowlist（36 的合法豁免必须：到期时间、owner、理由、非 release class）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // 44.2：JSON 模型字段；reason 由历史豁免记录保留，当前读路径未消费
pub struct ClosureAllowEntry {
    pub package: String,
    pub feature: String,
    pub expires: String,
    pub owner: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureAllowlist {
    pub schema: String,
    /// 空 allowlist = 零豁免（产品闭包零 mock 泄漏的目标状态）。
    #[serde(default)]
    pub entries: Vec<ClosureAllowEntry>,
}

pub fn load_allowlist(root: &Path) -> DynResult<ClosureAllowlist> {
    let path = root.join("product/closure/feature-allowlist.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    let list: ClosureAllowlist =
        toml::from_str(&text).map_err(|e| invalid(format!("allowlist parse: {e}")))?;
    if list.schema != "eneros.closure-allowlist.v1" {
        return Err(invalid(format!("closure-allowlist-schema:{}", list.schema)));
    }
    Ok(list)
}

#[derive(Debug, Clone, Serialize)]
pub struct ClosureReport {
    pub schema: &'static str,
    pub binary: String,
    pub target: String,
    pub package: String,
    pub reached_packages: usize,
    pub denied: Vec<String>,
    pub violations: Vec<String>,
}

/// 解析 cargo tree 一行：`{depth}{package}|{features}` 或 `{depth}{package}|{features} (*)`。
/// 返回 (depth, package_name, features)。
fn parse_tree_line(line: &str) -> Option<(usize, String, Vec<String>)> {
    let line = line.trim_end();
    if line.is_empty() {
        return None;
    }
    let digits = line
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }
    let depth: usize = digits.parse().ok()?;
    let rest = &line[digits.len()..];
    let (package_part, features_part) = rest.split_once('|').unwrap_or((rest, ""));
    let package = package_part
        .split_whitespace()
        .next()
        .unwrap_or(package_part)
        .to_owned();
    // cargo tree 对重复展开行输出 `{f} (*)`（或单独 `(*)`），(*)
    // 是行级标记不是 feature，先剥离。
    let features_part = features_part.trim();
    let mut features_part = features_part.strip_suffix(" (*)").unwrap_or(features_part);
    if let Some(rest) = features_part.strip_suffix("(*)") {
        features_part = rest;
    }
    let features_part = features_part.trim();
    let features = if features_part.is_empty() {
        Vec::new()
    } else {
        features_part
            .split(',')
            .map(|f| f.trim().to_owned())
            .filter(|f| !f.is_empty())
            .collect::<Vec<_>>()
    };
    Some((depth, package, features))
}

/// 对每个 service 的 target 执行 `cargo tree -e normal,build`，
/// 计算 reachable package 的 feature 激活路径并检查 deny class。
/// 返回 (报告, 全部到达的 package 名集合[供 crypto closure 用])。
pub fn gate_resolved_closure(
    root: &Path,
    manifest: &ProductManifest,
) -> DynResult<(Vec<ClosureReport>, BTreeSet<String>)> {
    let allowlist = load_allowlist(root)?;
    let mut reports = Vec::new();
    let mut all_packages: BTreeSet<String> = BTreeSet::new();

    let mut by_target: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for service in &manifest.services {
        if service.external {
            continue;
        }
        let (package, _) = split_selector(&service.binary);
        let target = if service.target.is_empty() {
            "host".to_owned()
        } else {
            service.target.clone()
        };
        by_target.entry(target).or_default().push(package);
    }

    for (target, packages) in by_target {
        for package_name in &packages {
            let mut report = ClosureReport {
                schema: "eneros.closure-report.v1",
                binary: package_name.clone(),
                target: target.clone(),
                package: package_name.clone(),
                reached_packages: 0,
                denied: Vec::new(),
                violations: Vec::new(),
            };
            let output = run_cargo_tree(root, &target, package_name)?;
            // depth 栈重建依赖路径。
            let mut stack: Vec<(usize, String)> = Vec::new();
            let mut reached: BTreeSet<String> = BTreeSet::new();
            for line in output.lines() {
                let Some((depth, dep_name, features)) = parse_tree_line(line) else {
                    continue;
                };
                while stack.last().is_some_and(|(d, _)| *d >= depth) {
                    stack.pop();
                }
                let path_prefix = if stack.is_empty() {
                    package_name.clone()
                } else {
                    let mut parts: Vec<String> =
                        stack.iter().map(|(_, name)| name.clone()).collect();
                    parts.push(dep_name.clone());
                    parts.join(" -> ")
                };
                stack.push((depth, dep_name.clone()));
                reached.insert(dep_name.clone());
                all_packages.insert(dep_name.clone());
                // deny feature 检查（含 root 自身声明的 features）。
                for feature in &features {
                    if is_denied(&dep_name, feature) && !allowlisted(&allowlist, &dep_name, feature)
                    {
                        report.denied.push(format!(
                            "RELEASE_FEATURE_DENIED\nbinary={package_name}\npackage={dep_name}\nfeature={feature}\npath={path_prefix}[{feature}]"
                        ));
                    }
                }
                // package 名 deny class。
                if is_denied(&dep_name, "") {
                    report.violations.push(format!(
                        "RELEASE_PACKAGE_DENIED:binary={package_name}:package={dep_name}"
                    ));
                }
            }
            report.reached_packages = reached.len();
            reports.push(report);
        }
    }
    Ok((reports, all_packages))
}

fn is_denied(package: &str, feature: &str) -> bool {
    let package_lower = package.to_ascii_lowercase();
    let feature_lower = feature.to_ascii_lowercase();
    DENY_FEATURE_PATTERNS.iter().any(|pattern| {
        let pattern = pattern.to_ascii_lowercase();
        // package 名命中仅限明确的测试/mock 词（sim/null 等模糊词只查 feature）。
        (pattern != "sim" && pattern != "null" && package_lower.contains(&pattern))
            || (!feature.is_empty() && feature_lower.contains(&pattern))
    })
}

fn allowlisted(allowlist: &ClosureAllowlist, package: &str, feature: &str) -> bool {
    let today = current_utc_date();
    allowlist.entries.iter().any(|entry| {
        entry.package == package
            && (entry.feature == "*" || entry.feature == feature)
            && entry.expires.as_str() > today.as_str()
            && !entry.owner.is_empty()
    })
}

/// 当前 UTC 日期（YYYY-MM-DD）。allowlist 过期判定必须用真实时钟，
/// 不能硬编码审计基线日（36 收尾复验修复：硬编码基线日使 expires 字段
/// 永不生效，豁免条目无限期合法）。
fn current_utc_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    // Howard Hinnant civil_from_days。
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y0 = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y0 + 1 } else { y0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn run_cargo_tree(root: &Path, target: &str, package: &str) -> DynResult<String> {
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .args(["tree", "--locked"])
        .args(["--target", target])
        .args(["-p", package])
        .args(["-e", "normal,build"])
        .args(["--prefix", "depth"])
        .args(["--format", "{p}|{f}"]);
    let output = command.output()?;
    if !output.status.success() {
        return Err(invalid(format!(
            "cargo tree failed for {package}@{target}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// 摘要写入 evidence（不能只报命中词，必须含路径与 digest）。
pub fn closure_report_digest(reports: &[ClosureReport]) -> String {
    let json = serde_json::to_vec(reports).unwrap_or_default();
    hex(&Sha256::digest(&json))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

/// 汇总 crypto closure 检查（product_allowed=false 禁止到达产品 ELF）。
pub fn crypto_closure_violations(
    root: &Path,
    packages: &BTreeSet<String>,
) -> DynResult<Vec<String>> {
    policy::gate_crypto_closure(root, packages)
}

/// `closure gate --profile <id>`：resolved feature graph Gate（WP-36.6）。
/// 额外执行 crypto closure（WP-36.2：product_allowed=false 禁止到达产品 ELF）。
pub fn command_closure(root: &Path, args: &[String]) -> DynResult<()> {
    let mut profile_id = String::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "gate" => {}
            "--profile" => {
                profile_id = it
                    .next()
                    .ok_or_else(|| invalid("--profile requires a value"))?
                    .clone()
            }
            other => return Err(invalid(format!("unknown closure arg '{other}'"))),
        }
    }
    if profile_id.is_empty() {
        return Err(invalid("closure gate requires --profile <id>"));
    }
    let manifest = super::product_manifest::load_normalized(root, &profile_id)?;
    let (reports, packages) = gate_resolved_closure(root, &manifest)?;
    let mut violations: Vec<String> = Vec::new();
    let mut denied_total = 0usize;
    for report in &reports {
        denied_total += report.denied.len();
        violations.extend(report.denied.iter().cloned());
        violations.extend(report.violations.iter().cloned());
    }
    let crypto = crypto_closure_violations(root, &packages)?;
    violations.extend(crypto.iter().cloned());
    let digest = closure_report_digest(&reports);
    println!(
        "CLOSURE-GATE profile={profile_id} binaries={} reached_packages={} denied={} crypto_violations={} digest={digest}",
        reports.len(),
        packages.len(),
        denied_total,
        crypto.len(),
    );
    if !violations.is_empty() {
        for v in &violations {
            eprintln!("{v}");
        }
        return Err(invalid(format!(
            "CLOSURE-GATE FAIL: {} violation(s)",
            violations.len()
        )));
    }
    println!("CLOSURE-GATE PASS");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_class_detection() {
        assert!(is_denied("eneros-rs485", "mock"));
        assert!(is_denied("eneros-storage", "mock"));
        assert!(is_denied("anything", "test-support"));
        assert!(is_denied("anything", "host-file"));
        assert!(is_denied("eneros-rs485", "simulated"));
        // feature 为空时只检查 package 名中明确带 mock/fake 的。
        assert!(!is_denied("eneros-simulink", ""));
        assert!(!is_denied("eneros-rs485", ""));
    }

    #[test]
    fn allowlist_requires_expiry_and_owner() {
        let list = ClosureAllowlist {
            schema: "eneros.closure-allowlist.v1".to_owned(),
            entries: vec![
                ClosureAllowEntry {
                    package: "eneros-rs485".to_owned(),
                    feature: "mock".to_owned(),
                    expires: "2099-01-01".to_owned(),
                    owner: "crypto-review".to_owned(),
                    reason: "test".to_owned(),
                },
                ClosureAllowEntry {
                    package: "eneros-rs485".to_owned(),
                    feature: "mock".to_owned(),
                    expires: "2000-01-01".to_owned(),
                    owner: "expired".to_owned(),
                    reason: "test".to_owned(),
                },
            ],
        };
        assert!(allowlisted(&list, "eneros-rs485", "mock"));
        // 过期条目不生效（与真实当前日期比较）。
        let list2 = ClosureAllowlist {
            schema: "eneros.closure-allowlist.v1".to_owned(),
            entries: vec![ClosureAllowEntry {
                package: "eneros-rs485".to_owned(),
                feature: "mock".to_owned(),
                expires: "2000-01-01".to_owned(),
                owner: "x".to_owned(),
                reason: "test".to_owned(),
            }],
        };
        assert!(!allowlisted(&list2, "eneros-rs485", "mock"));
    }

    #[test]
    fn current_utc_date_is_well_formed() {
        let date = current_utc_date();
        assert_eq!(date.len(), 10);
        assert_eq!(&date[4..5], "-");
        assert_eq!(&date[7..8], "-");
        //  sane range：不早于 2026（本仓审计基线年）。
        assert!(date.as_str() >= "2026-01-01", "date={date}");
    }

    #[test]
    fn parse_tree_line_formats() {
        let (depth, name, features) = parse_tree_line("1eneros-audit v1.0.0-rc.0 (path)|").unwrap();
        assert_eq!(depth, 1);
        assert_eq!(name, "eneros-audit");
        assert!(features.is_empty());
        let (depth, name, features) =
            parse_tree_line("2sha2 v0.10.9|default,core-api (*)").unwrap();
        assert_eq!(depth, 2);
        assert_eq!(name, "sha2");
        assert_eq!(features, vec!["default".to_owned(), "core-api".to_owned()]);
        assert!(parse_tree_line("").is_none());
        assert!(parse_tree_line("not a line").is_none());
    }
}
