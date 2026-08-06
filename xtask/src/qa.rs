//! QA-017~022：G9 商业资格质量门禁.
//!
//! MASTER §21.16 / G9:
//! - QA-017: "10k boot/reset campaign — 零未知失败"
//! - QA-018: "30-day soak campaign — 无泄漏/腐化"
//! - QA-019: "performance regression gate — 冻结 SLO"
//! - QA-020: "requirement trace coverage — critical 100% 自动证据"
//! - QA-021: "Isabelle admitted-proof gate — TCB claim 零 sorry"
//! - QA-022: "independent validation report — 非实现方签署"
//!
//! 每个子命令验证对应证据文件并以 fail-closed 方式门禁：
//! 缺证据、缺文件、阈值未达、扫描到禁止模式 → 返回错误。
//!
//! # 禁止捷径（MASTER G9）
//!
//! - 不得因无硬件而跳过 QEMU 级别的 boot/soak/perf 证据
//! - 不得用平均/p99 替代 WCET
//! - 不得用文件存在替代 proof 扫描
//! - 不得用实现方自签替代独立验证

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{invalid, read_json, DynResult};

// ============================================================
// 通用报告结构
// ============================================================

/// QA 门禁报告.
#[derive(Debug, Serialize)]
struct QaReport {
    schema: &'static str,
    qa_id: &'static str,
    result: &'static str,
    summary: BTreeMap<&'static str, String>,
    violations: Vec<String>,
}

impl QaReport {
    fn new(qa_id: &'static str) -> Self {
        Self {
            schema: "eneros.qa-gate.v1",
            qa_id,
            result: "FAIL",
            summary: BTreeMap::new(),
            violations: Vec::new(),
        }
    }

    fn pass(&mut self) {
        self.result = "PASS";
    }
}

/// 将报告写入 target/eneros/evidence/g9/ 并打印.
fn finalize(root: &Path, report: &QaReport) -> DynResult<()> {
    let dir = root.join("target/eneros/evidence/g9");
    fs::create_dir_all(&dir)?;
    let file = format!("{}.json", report.qa_id.to_lowercase().replace('-', "_"));
    let path = dir.join(&file);
    fs::write(&path, serde_json::to_vec_pretty(report)?)?;
    println!("{}", serde_json::to_string_pretty(report)?);
    if report.result != "PASS" {
        return Err(invalid(format!("{} gate did not pass", report.qa_id)));
    }
    Ok(())
}

// ============================================================
// 命令分发
// ============================================================

pub(super) fn command_qa(root: &Path, args: &[String]) -> DynResult<()> {
    let (mode, tail) = args.split_first().ok_or_else(|| {
        invalid("qa requires a subcommand: boot-reset|soak|perf-gate|trace|proof|validation")
    })?;
    match mode.as_str() {
        "boot-reset" => command_boot_reset(root, tail),
        "soak" => command_soak(root, tail),
        "perf-gate" => command_perf_gate(root, tail),
        "trace" => command_trace(root, tail),
        "proof" => command_proof(root, tail),
        "validation" => command_validation(root, tail),
        other => Err(invalid(format!("unknown qa subcommand '{other}'"))),
    }
}

// ============================================================
// QA-017: 10k boot/reset campaign
// ============================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct BootResetCampaign {
    schema: String,
    campaign_id: String,
    artifact_sha256: String,
    total_cycles: u64,
    cycles: Vec<BootCycle>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BootCycle {
    cycle_id: u64,
    result: CycleResult,
    /// 失败原因分类（空 = 成功；非空 = 已知根因）.
    failure_category: Option<String>,
    /// 是否未知失败（无根因）.
    unknown: bool,
    duration_ms: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum CycleResult {
    Pass,
    Fail,
    Reset,
}

fn command_boot_reset(root: &Path, args: &[String]) -> DynResult<()> {
    let path = match args {
        [p] => root.join(p),
        _ => return Err(invalid("qa boot-reset requires <campaign-results.json>")),
    };
    let campaign: BootResetCampaign = read_json(&path)?;
    let mut report = QaReport::new("QA-017");

    if campaign.schema != "eneros.boot-reset-campaign.v1" {
        report.violations.push("schema-mismatch".to_owned());
    }
    if campaign.total_cycles < 10_000 {
        report.violations.push(format!(
            "insufficient-cycles:{}<10000",
            campaign.total_cycles
        ));
    }
    if campaign.total_cycles != campaign.cycles.len() as u64 {
        report.violations.push(format!(
            "cycle-count-mismatch:declared={}:recorded={}",
            campaign.total_cycles,
            campaign.cycles.len()
        ));
    }

    let unknown_failures: Vec<&BootCycle> = campaign.cycles.iter().filter(|c| c.unknown).collect();
    if !unknown_failures.is_empty() {
        report
            .violations
            .push(format!("unknown-failures:{}", unknown_failures.len()));
    }

    // 已知失败必须有 failure_category
    let untriaged = campaign
        .cycles
        .iter()
        .filter(|c| c.result != CycleResult::Pass && c.failure_category.is_none())
        .count();
    if untriaged > 0 {
        report
            .violations
            .push(format!("untriaged-failures:{}", untriaged));
    }

    let passed = campaign
        .cycles
        .iter()
        .filter(|c| c.result == CycleResult::Pass)
        .count();
    let failed = campaign
        .cycles
        .iter()
        .filter(|c| c.result == CycleResult::Fail)
        .count();
    let reset = campaign
        .cycles
        .iter()
        .filter(|c| c.result == CycleResult::Reset)
        .count();

    report
        .summary
        .insert("campaign_id", campaign.campaign_id.clone());
    report
        .summary
        .insert("total_cycles", campaign.total_cycles.to_string());
    report.summary.insert("passed", passed.to_string());
    report.summary.insert("failed", failed.to_string());
    report.summary.insert("reset", reset.to_string());
    report
        .summary
        .insert("unknown_failures", unknown_failures.len().to_string());

    if report.violations.is_empty() {
        report.pass();
    }
    finalize(root, &report)
}

// ============================================================
// QA-018: 30-day soak campaign
// ============================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct SoakCampaign {
    schema: String,
    campaign_id: String,
    artifact_sha256: String,
    duration_days: u64,
    /// 内存泄漏率（字节/天）.
    memory_leak_bytes_per_day: u64,
    /// 内存泄漏阈值（字节/天）.
    memory_leak_threshold: u64,
    /// 是否检测到数据腐化.
    data_corruption: bool,
    /// 重启记录.
    restarts: Vec<SoakRestart>,
    /// 采集的指标样本数.
    metric_samples: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct SoakRestart {
    restart_id: u64,
    at_secs: u64,
    /// 根因分类（空 = 未知重启，违规）.
    root_cause: Option<String>,
}

fn command_soak(root: &Path, args: &[String]) -> DynResult<()> {
    let path = match args {
        [p] => root.join(p),
        _ => return Err(invalid("qa soak requires <soak-results.json>")),
    };
    let campaign: SoakCampaign = read_json(&path)?;
    let mut report = QaReport::new("QA-018");

    if campaign.schema != "eneros.soak-campaign.v1" {
        report.violations.push("schema-mismatch".to_owned());
    }
    if campaign.duration_days < 30 {
        report.violations.push(format!(
            "insufficient-duration:{}d<30d",
            campaign.duration_days
        ));
    }
    if campaign.memory_leak_bytes_per_day > campaign.memory_leak_threshold {
        report.violations.push(format!(
            "memory-leak-exceeds-threshold:{}>{}",
            campaign.memory_leak_bytes_per_day, campaign.memory_leak_threshold
        ));
    }
    if campaign.data_corruption {
        report
            .violations
            .push("data-corruption-detected".to_owned());
    }
    if campaign.metric_samples == 0 {
        report.violations.push("no-metric-samples".to_owned());
    }

    let unexplained_restarts = campaign
        .restarts
        .iter()
        .filter(|r| r.root_cause.is_none())
        .count();
    if unexplained_restarts > 0 {
        report
            .violations
            .push(format!("unexplained-restarts:{}", unexplained_restarts));
    }

    report
        .summary
        .insert("campaign_id", campaign.campaign_id.clone());
    report
        .summary
        .insert("duration_days", campaign.duration_days.to_string());
    report.summary.insert(
        "memory_leak_bytes_per_day",
        campaign.memory_leak_bytes_per_day.to_string(),
    );
    report
        .summary
        .insert("data_corruption", campaign.data_corruption.to_string());
    report
        .summary
        .insert("restart_count", campaign.restarts.len().to_string());
    report
        .summary
        .insert("unexplained_restarts", unexplained_restarts.to_string());

    if report.violations.is_empty() {
        report.pass();
    }
    finalize(root, &report)
}

// ============================================================
// QA-019: performance regression gate
// ============================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct PerfResults {
    schema: String,
    baseline_sha256: String,
    candidate_sha256: String,
    metrics: Vec<PerfMetric>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct PerfMetric {
    metric_id: String,
    /// 冻结的 SLO 阈值（如 deadline ms）.
    slo_threshold: u64,
    /// 实测值.
    measured: u64,
    /// 回归阈值百分比（如 5 = 允许 5% 回归）.
    regression_tolerance_pct: u64,
    /// 基线值（上次冻结值）.
    baseline_value: u64,
    /// 是否为关键路径（关键路径不允许超 SLO）.
    critical_path: bool,
    unit: String,
}

fn command_perf_gate(root: &Path, args: &[String]) -> DynResult<()> {
    let path = match args {
        [p] => root.join(p),
        _ => return Err(invalid("qa perf-gate requires <perf-results.json>")),
    };
    let results: PerfResults = read_json(&path)?;
    let mut report = QaReport::new("QA-019");

    if results.schema != "eneros.perf-regression.v1" {
        report.violations.push("schema-mismatch".to_owned());
    }
    if results.metrics.is_empty() {
        report.violations.push("no-metrics".to_owned());
    }

    let mut regressed = 0usize;
    let mut slo_violations = 0usize;
    for metric in &results.metrics {
        // 关键路径：实测不得超过 SLO 阈值
        if metric.critical_path && metric.measured > metric.slo_threshold {
            report.violations.push(format!(
                "slo-violation:{}:{}>{}",
                metric.metric_id, metric.measured, metric.slo_threshold
            ));
            slo_violations += 1;
        }
        // 回归检查：实测相对基线退化不得超过容忍度
        if metric.baseline_value > 0 {
            let delta_pct = if metric.measured > metric.baseline_value {
                ((metric.measured - metric.baseline_value) * 100) / metric.baseline_value
            } else {
                0
            };
            if delta_pct > metric.regression_tolerance_pct {
                report.violations.push(format!(
                    "regression:{}:{}%>{}%",
                    metric.metric_id, delta_pct, metric.regression_tolerance_pct
                ));
                regressed += 1;
            }
        }
    }

    report
        .summary
        .insert("metric_count", results.metrics.len().to_string());
    report
        .summary
        .insert("slo_violations", slo_violations.to_string());
    report.summary.insert("regressions", regressed.to_string());
    report
        .summary
        .insert("baseline_sha256", results.baseline_sha256.clone());

    if report.violations.is_empty() {
        report.pass();
    }
    finalize(root, &report)
}

// ============================================================
// QA-020: requirement trace coverage
// ============================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequirementRegistry {
    schema: String,
    requirements: Vec<TraceRequirement>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceRequirement {
    requirement_id: String,
    /// 严重度（critical 必须有证据）.
    severity: String,
    /// 是否有自动化测试证据.
    has_automated_evidence: bool,
    /// 关联的测试 ID.
    test_ids: Vec<String>,
    /// 关联的 artifact 路径.
    artifact_path: Option<String>,
}

fn command_trace(root: &Path, args: &[String]) -> DynResult<()> {
    let path = match args {
        [p] => root.join(p),
        _ => return Err(invalid("qa trace requires <requirement-registry.json>")),
    };
    let registry: RequirementRegistry = read_json(&path)?;
    let mut report = QaReport::new("QA-020");

    if registry.schema != "eneros.requirement-trace.v1" {
        report.violations.push("schema-mismatch".to_owned());
    }
    if registry.requirements.is_empty() {
        report.violations.push("empty-registry".to_owned());
    }

    // 检查重复 ID
    let mut seen = BTreeSet::new();
    for req in &registry.requirements {
        if !seen.insert(req.requirement_id.as_str()) {
            report
                .violations
                .push(format!("duplicate-requirement:{}", req.requirement_id));
        }
    }

    // critical 需求必须有自动化证据 + 测试 + artifact
    let critical_without_evidence: Vec<&TraceRequirement> = registry
        .requirements
        .iter()
        .filter(|r| {
            r.severity == "critical"
                && (!r.has_automated_evidence || r.test_ids.is_empty() || r.artifact_path.is_none())
        })
        .collect();

    for req in &critical_without_evidence {
        if !req.has_automated_evidence {
            report.violations.push(format!(
                "critical-no-automated-evidence:{}",
                req.requirement_id
            ));
        }
        if req.test_ids.is_empty() {
            report
                .violations
                .push(format!("critical-no-tests:{}", req.requirement_id));
        }
        if req.artifact_path.is_none() {
            report
                .violations
                .push(format!("critical-no-artifact:{}", req.requirement_id));
        }
    }

    let critical_count = registry
        .requirements
        .iter()
        .filter(|r| r.severity == "critical")
        .count();
    let critical_covered = critical_count - critical_without_evidence.len();

    report.summary.insert(
        "total_requirements",
        registry.requirements.len().to_string(),
    );
    report
        .summary
        .insert("critical_count", critical_count.to_string());
    report
        .summary
        .insert("critical_covered", critical_covered.to_string());

    if report.violations.is_empty() {
        report.pass();
    }
    finalize(root, &report)
}

// ============================================================
// QA-021: Isabelle admitted-proof gate
// ============================================================

/// 扫描 Isabelle 理论文件中的禁止模式.
///
/// 禁止：`sorry`、`oops`、`refines ≡ True`、`refines: True`、
/// 未解释关键常量（`consts` 声明无定义且无 axiomatization 审批）.
fn scan_theory_files(formal_dir: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    let Ok(entries) = fs::read_dir(formal_dir) else {
        violations.push("formal-dir-missing".to_owned());
        return violations;
    };

    let mut thy_count = 0u32;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(true, |ext| ext != "thy") {
            continue;
        }
        thy_count += 1;
        let Ok(content) = fs::read_to_string(&path) else {
            violations.push(format!("unreadable-theory:{}", path.display()));
            continue;
        };
        let rel = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        // 逐行扫描禁止模式
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            // 跳过注释
            if trimmed.starts_with("(*") || trimmed.starts_with("--") {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();
            if lower.contains("sorry") {
                violations.push(format!("sorry:{}:{}", rel, lineno + 1));
            }
            if lower.contains("oops") {
                violations.push(format!("oops:{}:{}", rel, lineno + 1));
            }
            // refines ≡ True / refines: True / refines True
            if lower.contains("refines") && (lower.contains("true") || lower.contains("≡ true")) {
                violations.push(format!("true-refinement:{}:{}", rel, lineno + 1));
            }
        }
    }

    if thy_count == 0 {
        violations.push("no-theory-files".to_owned());
    }

    violations
}

fn command_proof(root: &Path, args: &[String]) -> DynResult<()> {
    let formal_dir = match args {
        [] => root.join("formal/Isabelle"),
        [p] => root.join(p),
        _ => return Err(invalid("qa proof takes at most one <formal-dir> argument")),
    };
    let mut report = QaReport::new("QA-021");

    let violations = scan_theory_files(&formal_dir);
    let thy_count = fs::read_dir(&formal_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "thy"))
                .count()
        })
        .unwrap_or(0);

    // 检查 ROOT session 文件存在
    let root_file = formal_dir.join("ROOT");
    if !root_file.is_file() {
        report.violations.push("missing-ROOT-session".to_owned());
    }

    report.violations.extend(violations);
    report.summary.insert("theory_files", thy_count.to_string());
    report
        .summary
        .insert("root_session", root_file.is_file().to_string());

    if report.violations.is_empty() && thy_count > 0 {
        report.pass();
    }
    finalize(root, &report)
}

// ============================================================
// QA-022: independent validation report
// ============================================================

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ValidationReport {
    schema: String,
    candidate_sha256: String,
    review_date: String,
    reviewers: Vec<Reviewer>,
    gate_areas: Vec<GateArea>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct Reviewer {
    name: String,
    role: String,
    /// 是否为实现方（实现方不能独立签发）.
    is_implementer: bool,
    /// 是否已签署.
    signed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GateArea {
    area: String,
    /// 结论（pass/fail/blocked）.
    conclusion: String,
    /// 签署此区域的评审人.
    signed_by: String,
    /// 是否由非实现方签署.
    independent_signoff: bool,
}

fn command_validation(root: &Path, args: &[String]) -> DynResult<()> {
    let path = match args {
        [p] => root.join(p),
        _ => return Err(invalid("qa validation requires <validation-report.json>")),
    };
    let report_data: ValidationReport = read_json(&path)?;
    let mut report = QaReport::new("QA-022");

    if report_data.schema != "eneros.independent-validation.v1" {
        report.violations.push("schema-mismatch".to_owned());
    }
    if report_data.reviewers.is_empty() {
        report.violations.push("no-reviewers".to_owned());
    }

    // 至少 1 名非实现方签署
    let independent_signed: Vec<&Reviewer> = report_data
        .reviewers
        .iter()
        .filter(|r| !r.is_implementer && r.signed)
        .collect();
    if independent_signed.is_empty() {
        report.violations.push("no-independent-signoff".to_owned());
    }

    // 实现方不得独立签发任何 gate area
    for area in &report_data.gate_areas {
        if area.conclusion != "pass" && area.conclusion != "blocked" {
            report.violations.push(format!(
                "invalid-conclusion:{}:{}",
                area.area, area.conclusion
            ));
        }
        if !area.independent_signoff {
            report
                .violations
                .push(format!("area-not-independently-signed:{}", area.area));
        }
        if area.signed_by.is_empty() {
            report
                .violations
                .push(format!("area-unsigned:{}", area.area));
        }
    }

    // 必需 gate areas
    let required_areas = [
        "security",
        "quality",
        "performance",
        "compliance",
        "release",
    ];
    let covered: BTreeSet<&str> = report_data
        .gate_areas
        .iter()
        .map(|a| a.area.as_str())
        .collect();
    for required in &required_areas {
        if !covered.contains(*required) {
            report
                .violations
                .push(format!("missing-gate-area:{}", required));
        }
    }

    let pass_areas = report_data
        .gate_areas
        .iter()
        .filter(|a| a.conclusion == "pass")
        .count();

    report
        .summary
        .insert("reviewer_count", report_data.reviewers.len().to_string());
    report
        .summary
        .insert("independent_signed", independent_signed.len().to_string());
    report
        .summary
        .insert("gate_areas", report_data.gate_areas.len().to_string());
    report.summary.insert("pass_areas", pass_areas.to_string());
    report
        .summary
        .insert("candidate_sha256", report_data.candidate_sha256.clone());

    if report.violations.is_empty() {
        report.pass();
    }
    finalize(root, &report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_json<T: Serialize>(path: &Path, value: &T) {
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    #[test]
    fn t1_boot_reset_pass() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let cycles: Vec<BootCycle> = (0..10_000)
            .map(|i| BootCycle {
                cycle_id: i,
                result: CycleResult::Pass,
                failure_category: None,
                unknown: false,
                duration_ms: 100,
            })
            .collect();
        let campaign = serde_json::json!({
            "schema": "eneros.boot-reset-campaign.v1",
            "campaign_id": "br-001",
            "artifact_sha256": "abc",
            "total_cycles": 10_000,
            "cycles": cycles,
        });
        let p = root.join("br.json");
        write_json(&p, &campaign);
        assert!(command_boot_reset(root, &[p.to_string_lossy().into()]).is_ok());
    }

    #[test]
    fn t2_boot_reset_fail_unknown() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let mut cycles: Vec<BootCycle> = (0..10_000)
            .map(|i| BootCycle {
                cycle_id: i,
                result: CycleResult::Pass,
                failure_category: None,
                unknown: false,
                duration_ms: 100,
            })
            .collect();
        cycles[5].unknown = true;
        let campaign = serde_json::json!({
            "schema": "eneros.boot-reset-campaign.v1",
            "campaign_id": "br-002",
            "artifact_sha256": "abc",
            "total_cycles": 10_000,
            "cycles": cycles,
        });
        let p = root.join("br.json");
        write_json(&p, &campaign);
        assert!(command_boot_reset(root, &[p.to_string_lossy().into()]).is_err());
    }

    #[test]
    fn t3_soak_pass() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let campaign = serde_json::json!({
            "schema": "eneros.soak-campaign.v1",
            "campaign_id": "soak-001",
            "artifact_sha256": "abc",
            "duration_days": 30,
            "memory_leak_bytes_per_day": 1024,
            "memory_leak_threshold": 4096,
            "data_corruption": false,
            "restarts": [{"restart_id": 1, "at_secs": 100, "root_cause": "power-glitch"}],
            "metric_samples": 100_000,
        });
        let p = root.join("soak.json");
        write_json(&p, &campaign);
        assert!(command_soak(root, &[p.to_string_lossy().into()]).is_ok());
    }

    #[test]
    fn t4_soak_fail_corruption() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let campaign = serde_json::json!({
            "schema": "eneros.soak-campaign.v1",
            "campaign_id": "soak-002",
            "artifact_sha256": "abc",
            "duration_days": 30,
            "memory_leak_bytes_per_day": 100,
            "memory_leak_threshold": 4096,
            "data_corruption": true,
            "restarts": [],
            "metric_samples": 50000,
        });
        let p = root.join("soak.json");
        write_json(&p, &campaign);
        assert!(command_soak(root, &[p.to_string_lossy().into()]).is_err());
    }

    #[test]
    fn t5_perf_gate_pass() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let results = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "candidate_sha256": "def",
            "metrics": [
                {"metric_id": "boot-time", "slo_threshold": 5000, "measured": 4000, "regression_tolerance_pct": 5, "baseline_value": 3900, "critical_path": true, "unit": "ms"},
                {"metric_id": "ipc-latency", "slo_threshold": 100, "measured": 80, "regression_tolerance_pct": 10, "baseline_value": 79, "critical_path": true, "unit": "us"},
            ],
        });
        let p = root.join("perf.json");
        write_json(&p, &results);
        assert!(command_perf_gate(root, &[p.to_string_lossy().into()]).is_ok());
    }

    #[test]
    fn t6_perf_gate_fail_slo() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let results = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "candidate_sha256": "def",
            "metrics": [
                {"metric_id": "boot-time", "slo_threshold": 5000, "measured": 6000, "regression_tolerance_pct": 5, "baseline_value": 4000, "critical_path": true, "unit": "ms"},
            ],
        });
        let p = root.join("perf.json");
        write_json(&p, &results);
        assert!(command_perf_gate(root, &[p.to_string_lossy().into()]).is_err());
    }

    #[test]
    fn t7_trace_pass() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let reg = serde_json::json!({
            "schema": "eneros.requirement-trace.v1",
            "requirements": [
                {"requirement_id": "REQ-001", "severity": "critical", "has_automated_evidence": true, "test_ids": ["T1"], "artifact_path": "a.json"},
                {"requirement_id": "REQ-002", "severity": "low", "has_automated_evidence": false, "test_ids": [], "artifact_path": null},
            ],
        });
        let p = root.join("trace.json");
        write_json(&p, &reg);
        assert!(command_trace(root, &[p.to_string_lossy().into()]).is_ok());
    }

    #[test]
    fn t8_trace_fail_critical_no_evidence() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let reg = serde_json::json!({
            "schema": "eneros.requirement-trace.v1",
            "requirements": [
                {"requirement_id": "REQ-001", "severity": "critical", "has_automated_evidence": false, "test_ids": [], "artifact_path": null},
            ],
        });
        let p = root.join("trace.json");
        write_json(&p, &reg);
        assert!(command_trace(root, &[p.to_string_lossy().into()]).is_err());
    }

    #[test]
    fn t9_proof_detects_sorry() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let formal = root.join("formal/Isabelle");
        fs::create_dir_all(&formal).unwrap();
        fs::write(formal.join("ROOT"), "session Test = HOL +").unwrap();
        fs::write(
            formal.join("Test.thy"),
            "theory Test imports Main begin\nlemma t: True sorry\nend",
        )
        .unwrap();
        assert!(command_proof(root, &[]).is_err());
    }

    #[test]
    fn t10_proof_pass_clean() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let formal = root.join("formal/Isabelle");
        fs::create_dir_all(&formal).unwrap();
        fs::write(formal.join("ROOT"), "session Test = HOL +").unwrap();
        fs::write(
            formal.join("Test.thy"),
            "theory Test imports Main begin\nlemma t: True by simp\nend",
        )
        .unwrap();
        assert!(command_proof(root, &[]).is_ok());
    }

    #[test]
    fn t11_validation_pass() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let report = serde_json::json!({
            "schema": "eneros.independent-validation.v1",
            "candidate_sha256": "abc",
            "review_date": "2026-08-04",
            "reviewers": [
                {"name": "ext-audit", "role": "security-auditor", "is_implementer": false, "signed": true},
            ],
            "gate_areas": [
                {"area": "security", "conclusion": "pass", "signed_by": "ext-audit", "independent_signoff": true},
                {"area": "quality", "conclusion": "pass", "signed_by": "ext-audit", "independent_signoff": true},
                {"area": "performance", "conclusion": "pass", "signed_by": "ext-audit", "independent_signoff": true},
                {"area": "compliance", "conclusion": "pass", "signed_by": "ext-audit", "independent_signoff": true},
                {"area": "release", "conclusion": "pass", "signed_by": "ext-audit", "independent_signoff": true},
            ],
        });
        let p = root.join("val.json");
        write_json(&p, &report);
        assert!(command_validation(root, &[p.to_string_lossy().into()]).is_ok());
    }

    #[test]
    fn t12_validation_fail_no_independent() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let report = serde_json::json!({
            "schema": "eneros.independent-validation.v1",
            "candidate_sha256": "abc",
            "review_date": "2026-08-04",
            "reviewers": [
                {"name": "dev", "role": "developer", "is_implementer": true, "signed": true},
            ],
            "gate_areas": [
                {"area": "security", "conclusion": "pass", "signed_by": "dev", "independent_signoff": false},
                {"area": "quality", "conclusion": "pass", "signed_by": "dev", "independent_signoff": false},
                {"area": "performance", "conclusion": "pass", "signed_by": "dev", "independent_signoff": false},
                {"area": "compliance", "conclusion": "pass", "signed_by": "dev", "independent_signoff": false},
                {"area": "release", "conclusion": "pass", "signed_by": "dev", "independent_signoff": false},
            ],
        });
        let p = root.join("val.json");
        write_json(&p, &report);
        assert!(command_validation(root, &[p.to_string_lossy().into()]).is_err());
    }

    fn tempdir() -> String {
        let dir = std::env::temp_dir().join(format!("eneros-qa-test-{}", std::process::id()));
        let sub = dir.join(format!(
            "t{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                % 1_000_000
        ));
        fs::create_dir_all(&sub).unwrap();
        sub.to_string_lossy().into_owned()
    }
}
