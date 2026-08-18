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
// 33 GV-05：soak-run artifact_sha256 实算（sha2 为 xtask 既有依赖）。
use sha2::{Digest, Sha256};

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
        invalid("qa requires a subcommand: boot-reset|soak|perf-keygen|perf-gate|trace|proof|validation")
    })?;
    match mode.as_str() {
        "boot-reset" => command_boot_reset(root, tail),
        "soak" => command_soak(root, tail),
        "soak-run" => command_soak_run(root, tail),
        "soak-report" => command_soak_report(root, tail),
        "interop-report" => command_interop_report(root, tail),
        "kat-report" => command_kat_report(root, tail),
        "compliance-report" => command_compliance_report(root, tail),
        "sign-evidence" => command_sign_evidence(root, tail),
        "verify-evidence-sig" => command_verify_evidence_sig(root, tail),
        "g7-audit" => command_g7_audit(root, tail),
        "perf-baseline" => command_perf_baseline(root, tail),
        "perf-measure" => command_perf_measure(root, tail),
        "perf-keygen" => command_perf_keygen(root, tail),
        "gate" => super::gate::command_gate(root, tail),
        "perf-gate" => command_perf_gate(root, tail),
        "trace" => command_trace(root, tail),
        "proof" => command_proof(root, tail),
        "validation" => command_validation(root, tail),
        "gng-version" => command_gng_version(root, tail),
        other => Err(invalid(format!("unknown qa subcommand '{other}'"))),
    }
}

// ============================================================
// GNG-VERSION: Go/No-Go 闸门与 workspace 版本一致性（蓝图/20 R1-1）
// 规则：任一 G 项非 PASS → workspace 版本必须带 `-rc.` 后缀；
// 全部 PASS 才允许写发布版（如 1.0.0）。禁止手工绕过——只读文件真实内容。
// ============================================================

/// 读取 docs/v1.0.0-go-no-go.md，解析 G1..G7 每项闸门状态（真实表格内容）。
/// 返回 gate -> is_pass；文件缺失/无状态行即 Err（fail-closed）。
fn parse_go_no_go_statuses(root: &Path) -> DynResult<BTreeMap<String, bool>> {
    let path = root.join("docs/v1.0.0-go-no-go.md");
    let text = fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    let mut statuses = BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("| G") {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('|').map(str::trim).collect();
        // 表格行格式：| G1 | 闸门项 | 判据 | 状态 | 证据 | → cols[1]=闸门号, cols[4]=状态
        let gate = cols.get(1).copied().unwrap_or("");
        if gate.len() == 2
            && gate.starts_with('G')
            && gate.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
        {
            let status = cols.get(4).copied().unwrap_or("");
            statuses.insert(gate.to_string(), status.contains("PASS"));
        }
    }
    if statuses.is_empty() {
        return Err(invalid(
            "go-no-go 文档中未解析到任何 G 项状态行（表格结构异常）",
        ));
    }
    Ok(statuses)
}

/// 读取根 Cargo.toml `[workspace.package]` 段的真实 version 字段值。
fn workspace_version(root: &Path) -> DynResult<String> {
    let path = root.join("Cargo.toml");
    let text = fs::read_to_string(&path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    let mut in_workspace_package = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_workspace_package = t == "[workspace.package]";
            continue;
        }
        if in_workspace_package {
            if let Some((key, value)) = t.split_once('=') {
                if key.trim() == "version" {
                    let v = value.trim().trim_matches('"').trim_matches('\'');
                    if !v.is_empty() {
                        return Ok(v.to_string());
                    }
                }
            }
        }
    }
    Err(invalid(
        "Cargo.toml 中未找到 [workspace.package] version 字段",
    ))
}

/// 版本闸门一致性校验（44-00 §3，取代 蓝图/20 R1-1）：
/// 未过终验不得挂 rc/1.0 标记；1.x+ 只有全 PASS 才允许；0.x 合法（fail-closed）。
fn command_gng_version(root: &Path, _args: &[String]) -> DynResult<()> {
    let statuses = parse_go_no_go_statuses(root)?;
    let version = workspace_version(root)?;
    let non_pass: Vec<&str> = statuses
        .iter()
        .filter(|(_, &p)| !p)
        .map(|(g, _)| g.as_str())
        .collect();
    let all_pass = non_pass.is_empty();
    // 44-00 §3（取代 蓝图/20 R1-1）：未过 GO 终验不得挂 rc/1.0 标记。
    if version.contains('-') {
        return Err(invalid(format!(
            "version-gate-consistency FAIL：workspace 版本 '{version}' 带预发布标记。\
             44-00 §3：未过 GO 终验不得挂 rc/1.0 标记；终验通过后直接 1.0.0"
        )));
    }
    let major: u64 = version
        .split('.')
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| {
            invalid(format!(
                "version-gate-consistency FAIL：workspace 版本 '{version}' 非 x.y.z 语义化格式"
            ))
        })?;
    if !all_pass && major >= 1 {
        return Err(invalid(format!(
            "version-gate-consistency FAIL：Go/No-Go 非 PASS 项 [{}]，但 workspace 版本 '{version}' 为 1.x+。\
             44-00 §3：冲刺期必须 0.x.y，只有全 PASS 才允许 1.0+",
            non_pass.join(", ")
        )));
    }
    println!("GNG-VERSION-PASS version={version} all_pass={all_pass} scheme=0x-44-00-s3");
    Ok(())
}

/// 计算字节流 sha256（十六进制，机器可重算证据）。
fn sha256_bytes(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct SoakCampaign {
    schema: String,
    campaign_id: String,
    artifact_sha256: String,
    /// 42 candidate lineage 绑定：当前候选血缘，防止旧证据跨候选复用。
    #[serde(default)]
    candidate_id: String,
    #[serde(default)]
    source_commit: String,
    #[serde(default)]
    dirty_tree_digest: String,
    #[serde(default)]
    product_manifest_digest: String,
    #[serde(default)]
    profile_digest: String,
    #[serde(default)]
    toolchain_digest: String,
    #[serde(default)]
    platform_inventory_digest: String,
    duration_days: u64,
    /// v0.253.0（D1）：最短浸泡天数（默认 3=72h，对应 14 号文 D1；
    /// 旧证据未声明时按 3 天计）。
    #[serde(default = "default_min_soak_days")]
    min_duration_days: u64,
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
    /// 42 WP-42.10：工作负载覆盖（真实执行的能力清单；空 = 无负载）。
    #[serde(default)]
    workload_coverage: Vec<String>,
    /// 42 WP-42.10：真实事务/错误/恢复计数（不得写死 0——0 只允许在
    /// 真为零时出现，且与 cycles 一致）。
    #[serde(default)]
    transactions_total: u64,
    #[serde(default)]
    errors_total: u64,
    #[serde(default)]
    recoveries_total: u64,
    /// 42 WP-42.2：故障统计（注入/恢复/注入确认）。
    #[serde(default)]
    fault_stats: Option<SoakFaultStats>,
}

/// 42 WP-42.2：浸泡故障统计——injected 为真实注入且经确认的数量，
/// recovered 为按期望 oracle 恢复的数量；未注入时必须如实声明
/// injection_confirmed=false（QEMU 适配器无故障注入能力）。
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct SoakFaultStats {
    injected: u64,
    recovered: u64,
    injection_confirmed: bool,
}

fn default_min_soak_days() -> u64 {
    3 // 72h（14 号文 D1）
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct SoakRestart {
    restart_id: u64,
    at_secs: u64,
    /// 根因分类（空 = 未知重启，违规）.
    root_cause: Option<String>,
}

/// `qa soak-run --cycles N [--image <path> --vars-template <path>] [--out <json>]`
///
/// v0.253.0（D1a）：QEMU 浸泡循环——黑盒复用 `xtask run qemu-prod`（同一
/// 可执行入口，CI 行为一致），连续 N 次冷启动：
/// - 每次启动断言 PASS（失败即记录 restart 并继续，直至 --max-fail 上限）；
/// - 收集每次启动耗时（秒）与启动时间分布；
/// - 产出 QA-018 `eneros.soak-campaign.v1` 证据 JSON（供 `qa soak` 门禁消费）。
///
/// 无硬件环境的 QEMU 侧浸泡基线；真机 72h 浸泡由 `tests/soak/` 脚本集执行
/// （硬件就绪时），本命令是 CI 可自动跑的浸泡冒烟。
///
/// `qa soak-report [--bundle <evidence-bundle-dir>] [--out <json>]`
///
/// v0.255.0（D1c）：从浸泡证据包生成 D1 验收报告——六类验收项逐项判定：
/// - `PASS`：证据文件存在（boot-chain/power-cut/pcap/point-consistency/
///   schedule/blocked-events/secure-boot/ota-rollback/attestation/exported/
///   symbolize-report）；
/// - `BLOCKED`：证据缺失且缺失清单注明硬件/外设依赖（真机浸泡未执行）；
/// - 报告含每项证据的 sha256（来自 MANIFEST，防篡改引用）。
///
/// 产出 schema=eneros.d1-soak-report.v1，供 D1 验收与阶段 D 出口评审。
fn command_soak_report(root: &Path, args: &[String]) -> DynResult<()> {
    #[derive(Serialize)]
    struct D1Report {
        schema: &'static str,
        campaign: D1CampaignSummary,
        categories: Vec<D1Category>,
        verdict: &'static str,
    }
    #[derive(Serialize)]
    struct D1CampaignSummary {
        present: bool,
        campaign_id: Option<String>,
        duration_days: Option<u64>,
    }
    #[derive(Serialize)]
    struct D1Category {
        name: &'static str,
        evidence: Vec<D1EvidenceItem>,
        status: &'static str,
    }
    #[derive(Serialize)]
    struct D1EvidenceItem {
        file: String,
        present: bool,
        sha256: Option<String>,
        note: &'static str,
    }

    let mut bundle_dir = root.join("target/soak/evidence-bundle");
    let mut out_path = root.join("target/soak/d1-soak-report.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--bundle" => bundle_dir = root.join(val()?),
            "--out" => out_path = root.join(val()?),
            other => return Err(invalid(format!("unknown soak-report arg '{other}'"))),
        }
    }

    // 读取 MANIFEST（sha256 清单）→ 文件哈希索引。
    let manifest_path = bundle_dir.join("MANIFEST.sha256");
    let mut hashes: std::collections::BTreeMap<String, String> = Default::default();
    if let Ok(text) = fs::read_to_string(&manifest_path) {
        for line in text.lines() {
            if let Some((hash, name)) = line.split_once("  ") {
                hashes.insert(
                    name.trim_start_matches("./").to_owned(),
                    hash.trim().to_owned(),
                );
            }
        }
    }

    let evidence = |f: &str, note: &'static str| D1EvidenceItem {
        file: f.to_owned(),
        present: hashes.contains_key(f),
        sha256: hashes.get(f).cloned(),
        note,
    };

    // 六类验收项（14 号文 D1）。
    let categories = vec![
        D1Category {
            name: "boot_chain",
            evidence: vec![
                evidence("boot-chain.log", "串口日志：100 次冷启动零失败"),
                evidence("boot-times.csv", "启动时间分布（avg/min/max）"),
            ],
            status: "assessment",
        },
        D1Category {
            name: "storage",
            evidence: vec![
                evidence("power-cut.log", "随机断电 50 次注入记录"),
                evidence("recovery-check.log", "断电恢复校验（数据可读+校验和一致）"),
            ],
            status: "assessment",
        },
        D1Category {
            name: "protocol",
            evidence: vec![evidence(
                "point-consistency.json",
                "点表一致性报告（72h 无失联）",
            )],
            status: "assessment",
        },
        D1Category {
            name: "decision_chain",
            evidence: vec![
                evidence("schedule.log", "真实 HiGHS 日前调度日志"),
                evidence("blocked-events.json", "safety Fatal 阻断事件记录"),
            ],
            status: "assessment",
        },
        D1Category {
            name: "security_chain",
            evidence: vec![
                evidence("secure-boot.log", "安全启动验签日志"),
                evidence("ota-rollback.log", "OTA-A/B 升级+回滚日志"),
                evidence("attestation.log", "远程证明日志"),
            ],
            status: "assessment",
        },
        D1Category {
            name: "ops",
            evidence: vec![evidence("symbolize-report.json", "crash-dump 符号化报告")],
            status: "assessment",
        },
    ];

    // 判定：某类全部证据 present → PASS；部分/全缺 → BLOCKED（缺失清单
    // 存在即说明硬件/外设依赖未满足）。
    let missing_report = bundle_dir.join("missing-evidence.txt");
    let hardware_blocked = missing_report.exists();
    let mut all_pass = true;
    let mut categories = categories;
    for cat in &mut categories {
        let present = cat.evidence.iter().filter(|e| e.present).count();
        cat.status = if present == cat.evidence.len() {
            "PASS"
        } else {
            all_pass = false;
            if hardware_blocked {
                "BLOCKED"
            } else {
                "PARTIAL"
            }
        };
    }

    // campaign 摘要（soak-campaign.json 存在则带出）。
    let campaign_path = root.join("target/soak/soak-campaign.json");
    let campaign = if campaign_path.exists() {
        if let Ok(c) = read_json::<serde_json::Value>(&campaign_path) {
            D1CampaignSummary {
                present: true,
                campaign_id: c
                    .get("campaign_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_owned()),
                duration_days: c.get("duration_days").and_then(|v| v.as_u64()),
            }
        } else {
            D1CampaignSummary {
                present: true,
                campaign_id: None,
                duration_days: None,
            }
        }
    } else {
        D1CampaignSummary {
            present: false,
            campaign_id: None,
            duration_days: None,
        }
    };

    let verdict = if all_pass {
        "D1-PASS"
    } else if hardware_blocked {
        "D1-BLOCKED-HARDWARE"
    } else {
        "D1-PARTIAL"
    };

    let report = D1Report {
        schema: "eneros.d1-soak-report.v1",
        campaign,
        categories,
        verdict,
    };
    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(&out_path, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("SOAK-REPORT verdict={verdict} → {}", out_path.display());
    // 报告生成本身必须成功；verdict 由验收方判定（BLOCKED 不是生成失败）。
    Ok(())
}

/// `qa interop-report [--dir <interop-out>] [--out <json>]`
///
/// v0.256.0（D2）：从四类对端互操作结果（skip/结果 JSON）生成互操作矩阵
/// 报告（schema=eneros.interop-matrix.v1）——每个对端逐用例判定：
/// - `PASS`：对端地址注入且用例执行通过（真实对端接入后）；
/// - `SKIP`：无对端配置（产出 skip 证据，矩阵报告标注待执行）；
/// - 报告引用各对端 skip/结果 JSON，供 D2 验收与阶段 D 出口评审。
fn command_interop_report(root: &Path, args: &[String]) -> DynResult<()> {
    #[derive(Serialize)]
    struct InteropMatrix {
        schema: &'static str,
        peers: Vec<PeerRow>,
        verdict: &'static str,
        /// 蓝图 36 WP-36.8：非 D2-PASS 时 non_release=true。
        non_release: bool,
        /// 42 candidate lineage 绑定。
        #[serde(default)]
        candidate_id: String,
        #[serde(default)]
        source_commit: String,
        #[serde(default)]
        dirty_tree_digest: String,
        #[serde(default)]
        product_manifest_digest: String,
        #[serde(default)]
        profile_digest: String,
        #[serde(default)]
        toolchain_digest: String,
        #[serde(default)]
        platform_inventory_digest: String,
    }
    #[derive(Serialize)]
    struct PeerRow {
        peer: &'static str,
        status: &'static str,
        /// WP-26.1：证据类别——VERIFIED（真实执行）/ PENDING（skip 证据，
        /// 不计入通过，发布判定阻断）。
        classification: &'static str,
        cases: Vec<String>,
        evidence: String,
        required_peer: String,
    }

    let mut dir = root.join("target/interop");
    let mut out_path = root.join("target/interop/interop-matrix.json");
    let mut profile_id = "qemu-prod".to_owned();
    // 蓝图 36 WP-36.8：信息报告模式（--allow-non-pass）——矩阵 verdict
    // 非 D2-PASS 时写 non_release=true，不得被 release evidence gate 消费。
    let (allow_non_pass, rest) = crate::verdict::take_allow_non_pass(args);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--dir" => dir = root.join(val()?),
            "--out" => out_path = root.join(val()?),
            "--profile" => profile_id = val()?.to_owned(),
            other => return Err(invalid(format!("unknown interop-report arg '{other}'"))),
        }
    }

    let peers: [(&str, &str, Vec<&str>); 4] = [
        (
            "iec104",
            "iec104-skip.json",
            vec!["总召唤", "SBO", "时标 SOE", "时钟同步", "突变上送"],
        ),
        (
            "modbus",
            "modbus-skip.json",
            vec!["全功能码", "异常码", "边界地址", "长时轮询"],
        ),
        (
            "mms",
            "mms-skip.json",
            vec!["关联", "读写", "报告订阅", "GOOSE 订阅"],
        ),
        (
            "mqtt",
            "mqtt-skip.json",
            vec!["TLS", "QoS0/1/2", "断链重连"],
        ),
    ];

    let mut rows = Vec::new();
    let mut all_verified = true;
    for (peer, skip_file, cases) in peers {
        let skip_path = dir.join(skip_file);
        let result_path = dir.join(format!("{peer}-result.json"));
        // 读取 skip 证据里的 required_peer（无则 unknown）。
        let required_peer = read_json::<serde_json::Value>(&skip_path)
            .ok()
            .and_then(|v| v.get("required_peer").cloned())
            .and_then(|v| v.as_str().map(|s| s.to_owned()))
            .unwrap_or_else(|| "unknown".to_owned());
        // 蓝图 36 WP-36.10 #2：缺 skip 文件不能推导 VERIFIED；必须有
        // result JSON（verdict=PASS、peer identity、oracle）才 VERIFIED，
        // 否则一律 PENDING（非 PASS）。
        let mut verified = false;
        let mut detail = format!("{}/{}", dir.display(), skip_file);
        if let Ok(result) = read_json::<serde_json::Value>(&result_path) {
            let has_verdict_pass = result
                .get("verdict")
                .and_then(|v| v.as_str())
                .is_some_and(|v| v.eq_ignore_ascii_case("PASS"));
            let has_peer = result
                .get("peer_identity")
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.is_empty());
            let has_oracle = result
                .get("oracle")
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.is_empty());
            if has_verdict_pass && has_peer && has_oracle {
                verified = true;
                detail = result_path.to_string_lossy().replace('\\', "/");
            }
        }
        let status = if verified {
            "PASS"
        } else {
            all_verified = false;
            if skip_path.exists() {
                "SKIP"
            } else {
                "PENDING"
            }
        };
        let required = if verified {
            "real peer (result JSON with verdict=PASS + peer_identity + oracle)".to_owned()
        } else {
            required_peer
        };
        rows.push(PeerRow {
            peer,
            status,
            classification: if verified { "VERIFIED" } else { "PENDING" },
            cases: cases.iter().map(|s| s.to_string()).collect(),
            evidence: detail,
            required_peer: required,
        });
    }

    let verdict = if all_verified {
        "D2-PASS"
    } else {
        "D2-PENDING-REAL-PEERS"
    };
    let lineage = if root.join(".git").exists() {
        Some(super::evidence_chain::compute_lineage(
            root,
            &profile_id,
            None,
        )?)
    } else {
        None
    };
    let report = InteropMatrix {
        schema: "eneros.interop-matrix.v1",
        peers: rows,
        verdict,
        non_release: !all_verified,
        candidate_id: lineage
            .as_ref()
            .map(|l| l.candidate_id.clone())
            .unwrap_or_default(),
        source_commit: lineage
            .as_ref()
            .map(|l| l.source_commit.clone())
            .unwrap_or_default(),
        dirty_tree_digest: lineage
            .as_ref()
            .map(|l| l.dirty_tree_digest.clone())
            .unwrap_or_default(),
        product_manifest_digest: lineage
            .as_ref()
            .map(|l| l.product_manifest_digest.clone())
            .unwrap_or_default(),
        profile_digest: lineage
            .as_ref()
            .map(|l| l.profile_digest.clone())
            .unwrap_or_default(),
        toolchain_digest: lineage
            .as_ref()
            .map(|l| l.toolchain_digest.clone())
            .unwrap_or_default(),
        platform_inventory_digest: lineage
            .as_ref()
            .map(|l| l.platform_inventory_digest.clone())
            .unwrap_or_default(),
    };
    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(&out_path, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("INTEROP-MATRIX verdict={verdict} → {}", out_path.display());
    // 36 收尾（review should-fix）：interop 矩阵非 D2-PASS 默认非零，
    // 仅显式 --allow-non-pass 生成信息报告（non_release=true 已写）。
    if !all_verified && !allow_non_pass {
        return Err(invalid(format!(
            "INTEROP-MATRIX remains {verdict} (only D2-PASS releases; use --allow-non-pass for info report)"
        )));
    }
    Ok(())
}

/// `qa kat-report [--out <json>]`
///
/// v0.258.0（D3b）：密码 KAT 证据报告——运行 SM2/SM3/SM4/常数时间
/// KAT 测试（eneros-crypto 的 cry001/sm2_kat/sm3_kat/sm4_kat），
/// 汇总通过/失败为评审证据 JSON（schema=eneros.crypto-kat.v1），
/// 供 D3 密码学独立评审引用（product_allowed=false 解除的前置证据）。
fn command_kat_report(root: &Path, args: &[String]) -> DynResult<()> {
    #[derive(Serialize)]
    struct KatReport {
        schema: &'static str,
        suites: Vec<KatSuite>,
        verdict: &'static str,
    }
    #[derive(Serialize)]
    struct KatSuite {
        name: &'static str,
        test_file: &'static str,
        pass: bool,
        note: &'static str,
    }

    let mut out_path = root.join("target/kat-report.json");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out_path = root.join(val()?),
            other => return Err(invalid(format!("unknown kat-report arg '{other}'"))),
        }
    }

    // 运行 KAT 测试（cargo test，过滤对应 test binary）。
    let run = |filter: &str| -> bool {
        let status = std::process::Command::new("cargo")
            .args(["test", "-p", "eneros-crypto", "--test", filter, "--quiet"])
            .current_dir(root)
            .status();
        matches!(status, Ok(s) if s.success())
    };

    let suites = vec![
        KatSuite {
            name: "SM2 (GB/T 32918.2/4-2017)",
            test_file: "tests/sm2_kat.rs",
            pass: run("sm2_kat"),
            note: "签名/加密 KAT + 确定性签名可复现",
        },
        KatSuite {
            name: "SM3 (GB/T 32905)",
            test_file: "tests/sm3_kat.rs",
            pass: run("sm3_kat"),
            note: "标准向量 + 长度边界",
        },
        KatSuite {
            name: "SM4 (GB/T 32907)",
            test_file: "tests/sm4_kat.rs",
            pass: run("sm4_kat"),
            note: "ECB/CBC 标准向量",
        },
        KatSuite {
            name: "常数时间（CRY-001）",
            test_file: "tests/cry001_constant_time.rs",
            pass: run("cry001_constant_time"),
            note: "模逆/点乘时序 + KAT 对拍",
        },
    ];

    let all_pass = suites.iter().all(|s| s.pass);
    let report = KatReport {
        schema: "eneros.crypto-kat.v1",
        suites,
        verdict: if all_pass { "KAT-ALL-PASS" } else { "KAT-FAIL" },
    };
    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(&out_path, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!(
        "KAT-REPORT verdict={} → {}",
        report.verdict,
        out_path.display()
    );
    if !all_pass {
        return Err(invalid(
            "crypto KAT suite has failures — block product_allowed release",
        ));
    }
    Ok(())
}

/// `qa compliance-report [--out <json>] [--sign-key <path>]`
///
/// v0.259.0（D4）：合规证据汇总流水线——运行以下**真实重算**并汇总为
/// 合规报告（schema=eneros.compliance-report.v1）：
/// 1. 信创/等保动态探测（eneros-compliance Phase3ExitVerification，
///    P0-13：无动态证据标 Pending，不硬编码 Passed）；
/// 2. 密码 KAT 结果（qa kat-report 的 KAT-ALL-PASS）；
/// 3. QA-017~022 门禁证据文件存在性 + 校验（evidence 目录/JSON）；
/// 4. 全报告 sha256 摘要 + 可选 ed25519 签名（--sign-key）。
///
/// 供 D4「每项证据机器可重算」验收（14 号文 G5）与合规评审引用。
fn command_compliance_report(root: &Path, args: &[String]) -> DynResult<()> {
    #[derive(Serialize)]
    struct ComplianceReport {
        schema: &'static str,
        generated_at: String,
        xinchuang: Vec<ComplianceItem>,
        dengbao: Vec<ComplianceItem>,
        kat_verdict: String,
        qa_gates: Vec<QaGateRow>,
        /// 34 GV-14：字段拆分——代码自测结果（动态探测 recompute）。
        self_test_status: String,
        /// 34 GV-14：字段拆分——外部认证状态（人工/流程写入并签名，
        /// PENDING|SUBMITTED|CERTIFIED；与 self_test 不得混淆）。
        external_certification: &'static str,
        sha256: String,
        signature: String,
        verdict: &'static str,
    }
    #[derive(Serialize)]
    struct ComplianceItem {
        name: String,
        status: String,
        evidence: String,
    }
    #[derive(Serialize)]
    struct QaGateRow {
        gate: &'static str,
        evidence_file: String,
        present: bool,
        /// 34 GV-07：侧车签名（<file>.sig）经 ed25519 验证为真。
        signed: bool,
    }

    use sha2::{Digest, Sha256};

    let mut out_path = root.join("target/compliance-report.json");
    let mut sign_key: Option<std::path::PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out_path = root.join(val()?),
            "--sign-key" => sign_key = Some(root.join(val()?)),
            other => return Err(invalid(format!("unknown compliance-report arg '{other}'"))),
        }
    }

    // 34 GV-07：签名强制——无 --sign-key 即 Err（旧实现仅 WARN，
    // 证据可无签名产出 = 防篡改未达成）。
    let key_path = sign_key.ok_or_else(|| {
        invalid("compliance-report requires --sign-key（34 GV-07：证据签名强制，dev key 入仓先例 product/security/dev-signing.key）")
    })?;
    let key_bytes = fs::read(&key_path).map_err(|e| invalid(format!("read sign key: {e}")))?;
    let keypair = ed25519_dalek::SigningKey::from_bytes(
        &key_bytes[..32]
            .try_into()
            .map_err(|_| invalid("key must be 32 bytes"))?,
    );
    let verify_key = keypair.verifying_key();

    // 1. 信创/等保动态探测（真实 recompute，非硬编码）。
    let mut xinchuang_items = Vec::new();
    let mut dengbao_items = Vec::new();
    // 直接运行 compliance crate 的探测逻辑（host 可测）。
    if let Ok(output) = std::process::Command::new("cargo")
        .args([
            "test",
            "-p",
            "eneros-compliance",
            "--lib",
            "--",
            "--list",
            "--format",
            "terse",
        ])
        .current_dir(root)
        .output()
    {
        // 探测 crate 可编译可测（列表成功 = 探测面可用）。
        let _ = output;
    }
    // 通过 cargo test 实际运行探测（phase3_exit_check 测试会执行 recompute）。
    let compliance_pass = std::process::Command::new("cargo")
        .args(["test", "-p", "eneros-compliance", "--lib"])
        .current_dir(root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    xinchuang_items.push(ComplianceItem {
        name: "信创认证动态探测".into(),
        status: if compliance_pass { "PASS" } else { "FAIL" }.into(),
        evidence: "cargo test -p eneros-compliance（Phase3ExitVerification recompute）".into(),
    });
    dengbao_items.push(ComplianceItem {
        name: "等保 2.0 三级动态探测".into(),
        status: if compliance_pass { "PASS" } else { "FAIL" }.into(),
        evidence: "cargo test -p eneros-compliance（P0-13 无证据标 Pending）".into(),
    });

    // 2. KAT 证据（复用 kat-report 产物）。
    let kat_path = root.join("target/kat-report.json");
    let kat_verdict = read_json::<serde_json::Value>(&kat_path)
        .ok()
        .and_then(|v| v.get("verdict").cloned())
        .and_then(|v| v.as_str().map(|s| s.to_owned()))
        .unwrap_or_else(|| "KAT-NOT-RUN".to_owned());

    // 3. QA-017~022 门禁证据文件（34 GV-07：存在性 + 侧车签名验证，
    // 旧实现仅文件存在性检查）。
    let evidence_dir = root.join("target/eneros/evidence/g9");
    let qa_gates = vec![
        ("QA-017", "qa_017.json"),
        ("QA-018", "qa_018.json"),
        ("QA-019", "qa_019.json"),
        ("QA-020", "qa_020.json"),
        ("QA-021", "qa_021.json"),
        ("QA-022", "qa_022.json"),
    ]
    .into_iter()
    .map(|(gate, f)| {
        let fp = evidence_dir.join(f);
        let sigp = std::path::PathBuf::from(format!("{}.sig", fp.display()));
        QaGateRow {
            gate,
            evidence_file: f.to_owned(),
            present: fp.exists(),
            signed: fp.exists() && verify_evidence_sig(&fp, &sigp, &verify_key),
        }
    })
    .collect::<Vec<_>>();

    // 4. 摘要 + 签名（34 GV-07：签名强制；generated_at 取系统
    // RFC3339，禁止空串占位）。
    let report = ComplianceReport {
        schema: "eneros.compliance-report.v1",
        generated_at: rfc3339_now(),
        xinchuang: xinchuang_items,
        dengbao: dengbao_items,
        kat_verdict,
        qa_gates,
        // 34 GV-14：判定字段拆分——旧实现合规判定仅凭单测退出码
        // 混同“自测通过”与“已认证”；自测状态如实上报，外部
        // 认证未经人工/流程写入恒为 PENDING（fail-closed）。
        self_test_status: if compliance_pass { "PASS" } else { "FAIL" }.to_owned(),
        external_certification: "PENDING",
        sha256: String::new(),
        signature: String::new(),
        verdict: "assessment",
    };
    let json = serde_json::to_vec_pretty(&report)?;
    let digest = Sha256::digest(&json);
    let digest_hex = hex_encode(&digest);
    let mut report = report;
    report.sha256 = digest_hex;

    // 签名在带摘要的 JSON 上（34 GV-07：key 已在入口强制，必签）。
    let json_signed = serde_json::to_vec_pretty(&report)?;
    let signature = {
        use ed25519_dalek::Signer;
        hex_encode(&keypair.sign(&json_signed).to_bytes())
    };
    report.signature = signature;

    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(&out_path, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("COMPLIANCE-REPORT → {}", out_path.display());
    Ok(())
}

/// 34 GV-07：RFC3339 UTC 时间戳（系统时钟；禁止空串占位）。
/// 42 WP-42.12：perf 运行 id——时间纳秒派生（唯一、不可复用）。
pub(crate) fn perf_run_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("perf-run-{nanos:x}")
}

pub(crate) fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // 自 1970-01-01 的天数 → 公历日期（civil-from-days 算法）。
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    if mth <= 2 {
        y += 1;
    }
    format!("{y:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// 34 GV-07：证据侧车签名验证 `verify_evidence_sig(path, sig_path, pubkey)`。
///
/// 侧车签名文件内容 = `hex(ed25519(文件字节))`；文件缺失/签名长度
/// 非法/验签失败一律 false（fail-closed，不静默放行）。
pub fn verify_evidence_sig(
    path: &Path,
    sig_path: &Path,
    pubkey: &ed25519_dalek::VerifyingKey,
) -> bool {
    let (Ok(raw), Ok(sig_hex)) = (fs::read(path), fs::read_to_string(sig_path)) else {
        return false;
    };
    let sig_bytes = hex_decode(sig_hex.trim());
    let Ok(sig): Result<[u8; 64], _> = sig_bytes.try_into() else {
        return false;
    };
    use ed25519_dalek::Verifier;
    pubkey
        .verify(&raw, &ed25519_dalek::Signature::from_bytes(&sig))
        .is_ok()
}

/// `qa sign-evidence <path> --sign-key <key>`
///
/// 34 GV-07：为证据文件生成侧车签名 `<path>.sig`（既有证据一次性
/// 补签迁移用；dev key 入仓先例 product/security/dev-signing.key）。
/// 输出同时打印公钥 hex（供 verify-evidence-sig 使用）。
fn command_sign_evidence(root: &Path, args: &[String]) -> DynResult<()> {
    let mut path: Option<std::path::PathBuf> = None;
    let mut sign_key: Option<std::path::PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--sign-key" => sign_key = Some(root.join(val()?)),
            other if other.starts_with("--") => {
                return Err(invalid(format!("unknown sign-evidence arg '{other}'")))
            }
            other => path = Some(root.join(other)),
        }
    }
    let path = path.ok_or_else(|| invalid("sign-evidence requires <path>"))?;
    let key_path = sign_key.ok_or_else(|| invalid("sign-evidence requires --sign-key"))?;
    let raw = fs::read(&path).map_err(|e| invalid(format!("read {}: {e}", path.display())))?;
    let key_bytes = fs::read(&key_path).map_err(|e| invalid(format!("read sign key: {e}")))?;
    let keypair = ed25519_dalek::SigningKey::from_bytes(
        &key_bytes[..32]
            .try_into()
            .map_err(|_| invalid("key must be 32 bytes"))?,
    );
    use ed25519_dalek::Signer;
    let sig = hex_encode(&keypair.sign(&raw).to_bytes());
    let sig_path = std::path::PathBuf::from(format!("{}.sig", path.display()));
    fs::write(&sig_path, format!("{sig}\n"))?;
    println!("SIGN-EVIDENCE → {}", sig_path.display());
    println!("pubkey={}", hex_encode(keypair.verifying_key().as_bytes()));
    Ok(())
}

/// `qa verify-evidence-sig <path> --pub-key <hex>`
///
/// 34 GV-07：证据侧车签名验证（fail-closed：缺失/不匹配 → exit 非 0）。
fn command_verify_evidence_sig(root: &Path, args: &[String]) -> DynResult<()> {
    let mut path: Option<std::path::PathBuf> = None;
    let mut pub_key: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--pub-key" => pub_key = Some(val()?.clone()),
            other if other.starts_with("--") => {
                return Err(invalid(format!(
                    "unknown verify-evidence-sig arg '{other}'"
                )))
            }
            other => path = Some(root.join(other)),
        }
    }
    let path = path.ok_or_else(|| invalid("verify-evidence-sig requires <path>"))?;
    let pk_hex = pub_key.ok_or_else(|| invalid("verify-evidence-sig requires --pub-key <hex>"))?;
    let pk_bytes: [u8; 32] = hex_decode(pk_hex.trim())
        .try_into()
        .map_err(|_| invalid("pub-key must be 32-byte ed25519 pk (hex)"))?;
    let pk = ed25519_dalek::VerifyingKey::from_bytes(&pk_bytes)
        .map_err(|e| invalid(format!("bad pubkey: {e}")))?;
    let sig_path = std::path::PathBuf::from(format!("{}.sig", path.display()));
    if verify_evidence_sig(&path, &sig_path, &pk) {
        println!(
            "VERIFY-EVIDENCE-SIG → OK（{} ← {}）",
            path.display(),
            sig_path.display()
        );
        Ok(())
    } else {
        Err(invalid(format!(
            "verify-evidence-sig: 签名缺失或验证失败（{} / {}）",
            path.display(),
            sig_path.display()
        )))
    }
}

/// `qa g7-audit [--out <json>]`
///
/// v1.0.0（授权闸门 G7）：宣称一致性审计——扫描发布闭包中可能被文档
/// 宣称为真实能力的 mock/占位/模拟实现，输出审计报告
/// （schema=eneros.g7-consistency.v1）：
/// - 生产路径 mock 引用（cfg(test) 外）→ 违规；
/// - 文档宣称能力 vs 代码实现状态逐项对照（预置清单）；
/// - 无 Mock 能力被宣称为真实能力 → G7-PASS。
///
/// 按 Cargo.toml 的 `name` 字段查找 crate 目录（crate 名 ≠ 目录名）。
fn find_crate_dir(root: &Path, crate_name: &str) -> Option<std::path::PathBuf> {
    let crates_dir = root.join("crates");
    let mut stack: Vec<std::path::PathBuf> = vec![crates_dir];
    while let Some(dir) = stack.pop() {
        if dir.ends_with("target") || dir.ends_with("fuzz") {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            }
        }
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() {
            if let Ok(text) = fs::read_to_string(&manifest) {
                if text.lines().any(|l| {
                    l.trim() == format!("name = \"{crate_name}\"")
                        || l.trim() == format!("name = '{crate_name}'")
                }) {
                    return Some(dir);
                }
            }
        }
    }
    None
}

/// 递归收集 crates/** 下每个 crate（含 Cargo.toml 的目录）的 src/**/*.rs
/// 文件（排除 target/、fuzz/）。FIX-BUGS-0810/WP-26.1：旧实现仅扫一层
/// `crates/<x>/src/lib.rs`——全仓 crate 均位于两层路径，扫描零命中。
pub(crate) fn collect_crate_lib_files(crates_dir: &Path) -> Vec<(String, Vec<std::path::PathBuf>)> {
    let mut out: Vec<(String, Vec<std::path::PathBuf>)> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![crates_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir.ends_with("target") || dir.ends_with("fuzz") {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            }
        }
        // 本目录是 crate 根（含 Cargo.toml）→ 收集 src/**/*.rs。
        if dir.join("Cargo.toml").is_file() {
            let name = dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let src = dir.join("src");
            if src.is_dir() {
                let mut files = Vec::new();
                let mut fstack: Vec<std::path::PathBuf> = vec![src];
                while let Some(fd) = fstack.pop() {
                    let Ok(fe) = fs::read_dir(&fd) else { continue };
                    for f in fe.flatten() {
                        let fp = f.path();
                        if fp.is_dir() {
                            fstack.push(fp);
                        } else if fp.extension().map(|e| e == "rs").unwrap_or(false) {
                            files.push(fp);
                        }
                    }
                }
                files.sort();
                out.push((name, files));
            }
        }
    }
    out.sort();
    out
}

/// 检测一个文件里未门控的 mock 引用：
/// 1. lib.rs 顶层（非缩进）`use ...::mock` 且无 cfg 前缀（旧语义）；
/// 2. 任意 src 文件顶层 `pub mod mock` 且无 cfg(any(test, feature="mock")) 门
///    （WP-26.1：扩展检测面，与 WP-26.2 门禁共用）。
pub(crate) fn mock_leak_lines(path: &std::path::Path, _is_lib: bool) -> Vec<(usize, String)> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        // 缩进行 = 模块（mod tests 等）内部——不算顶层泄漏。
        if line.starts_with(char::is_whitespace) {
            continue;
        }
        let t = line.trim_start();
        // 34 GV-08（复验）：use-mock 检查对所有 .rs 生效（旧实现仅 lib.rs）。
        if t.starts_with("use ") && t.contains("::mock") && !protected_by_test_cfg(&lines, i) {
            hits.push((i + 1, t.to_owned()));
        }
        // 34 GV-08（复验）：mod 匹配含 pub(crate)/pub(super) 可见性变体
        //（旧实现仅 `pub mod mock`）。
        let is_mock_mod = t == "pub mod mock"
            || t.starts_with("pub mod mock;")
            || t.starts_with("pub(crate) mod mock")
            || t.starts_with("pub(super) mod mock")
            || t.starts_with("pub(crate) mod mock;")
            || t.starts_with("pub(super) mod mock;");
        if is_mock_mod && !protected_by_test_cfg(&lines, i) {
            hits.push((i + 1, t.to_owned()));
        }
    }
    hits
}

/// 34 GV-08（复验）：cfg 保护窗口——向上找最近属性行，须含 `test`/`mock`
/// feature 才算豁免（旧实现仅查上方 1-2 行是否 `#[cfg(`，不验 feature 内容、
/// 窗口过短）。
fn protected_by_test_cfg(lines: &[&str], i: usize) -> bool {
    for p in (0..i).rev() {
        let pt = lines[p].trim_start();
        if pt.is_empty() || pt.starts_with("//") || pt.starts_with("/*") {
            continue; // 空行/注释不打断窗口
        }
        if pt.starts_with("#[") {
            return pt.contains("test") || pt.contains("mock");
        }
        // 非属性代码行 → 属性不紧邻，窗口中断。
        break;
    }
    false
}

fn command_g7_audit(root: &Path, args: &[String]) -> DynResult<()> {
    #[derive(Serialize)]
    struct G7Report {
        schema: &'static str,
        production_mock_refs: Vec<String>,
        claimed_capabilities: Vec<serde_json::Value>,
        verdict: &'static str,
    }
    #[derive(Serialize)]
    struct CapabilityRow {
        id: &'static str,
        name: &'static str,
        detector: CapabilityDetector,
    }

    /// 能力检测器（WP-26.1：status 由 detector 运行时求值，禁止字面量）。
    #[derive(Serialize, Clone, Copy)]
    enum CapabilityDetector {
        /// crate 的 src/**/*.rs 中存在测试（test_name_substr 为空 = 任意
        /// `#[cfg(test)] mod tests`；非空 = 测试名包含该子串）。
        TestPresence {
            crate_name: &'static str,
            test_name_substr: &'static str,
        },
        /// 证据文件存在（可选 JSON 字段校验）。
        EvidenceFile {
            path: &'static str,
            required_fields: &'static [&'static str],
        },
    }

    impl CapabilityDetector {
        /// 运行时求值（蓝图 36 WP-36.9 止血）：
        /// TestPresence / EvidenceFile 只证明"存在"，不证明产品闭环
        /// （manifest service + production call path + evidence oracle），
        /// 因此只能产生 PENDING——永远不能 VERIFIED。
        /// 返回 (status, found)。
        fn evaluate(self, root: &Path) -> (&'static str, bool) {
            match self {
                CapabilityDetector::TestPresence {
                    crate_name,
                    test_name_substr,
                } => {
                    // crate 名 ≠ 目录名（如 eneros-crypto 位于
                    // crates/security/crypto）：按 Cargo.toml 的 name 匹配。
                    let Some(crate_dir) = find_crate_dir(root, crate_name) else {
                        return ("PENDING", false);
                    };
                    let mut found = false;
                    let mut files = Vec::new();
                    let mut stack = vec![crate_dir.join("src")];
                    while let Some(d) = stack.pop() {
                        let Ok(entries) = fs::read_dir(&d) else {
                            continue;
                        };
                        for e in entries.flatten() {
                            let p = e.path();
                            if p.is_dir() {
                                stack.push(p);
                            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                                files.push(p);
                            }
                        }
                    }
                    for f in files {
                        let Ok(text) = fs::read_to_string(&f) else {
                            continue;
                        };
                        if text.contains("#[cfg(test)]")
                            && (test_name_substr.is_empty() || text.contains(test_name_substr))
                        {
                            found = true;
                            break;
                        }
                    }
                    ("PENDING", found)
                }
                CapabilityDetector::EvidenceFile { path, .. } => {
                    ("PENDING", root.join(path).is_file())
                }
            }
        }
    }

    let mut out_path = root.join("target/g7-report.json");
    let (allow_non_pass, rest) = crate::verdict::take_allow_non_pass(args);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out_path = root.join(val()?),
            other => return Err(invalid(format!("unknown g7-audit arg '{other}'"))),
        }
    }

    // 1. 生产路径 mock 引用扫描（WP-26.1：递归全量——crates/** 两层 crate
    //    全部命中；lib.rs 顶层 `use ...::mock` 无 cfg 门 + 任意 src 文件
    //    `pub mod mock` 无 cfg(any(test, feature="mock")) 门）。
    let crates_dir = root.join("crates");
    let mut real_violations = Vec::new();
    for (crate_name, files) in collect_crate_lib_files(&crates_dir) {
        for file in files {
            let is_lib = file.file_name().map(|n| n == "lib.rs").unwrap_or(false);
            for (line, text) in mock_leak_lines(&file, is_lib) {
                real_violations.push(format!(
                    "{}:{}:{} (crate={})",
                    file.display(),
                    line,
                    text,
                    crate_name
                ));
            }
        }
    }
    real_violations.sort();
    real_violations.dedup();

    // 2. 宣称能力 vs 实现（WP-26.1：detector 运行时求值，禁止字面量 status）。
    //    status 由 detector 求值：VERIFIED / UNVERIFIED（计入失败）/ PENDING。
    let claimed = vec![
        CapabilityRow {
            id: "cap-secure-boot",
            name: "安全启动（SM2 验签）",
            detector: CapabilityDetector::TestPresence {
                crate_name: "eneros-secure-boot",
                test_name_substr: "",
            },
        },
        CapabilityRow {
            id: "cap-ota-ab",
            name: "OTA A/B 升级+回滚",
            detector: CapabilityDetector::TestPresence {
                crate_name: "eneros-ota-ab",
                test_name_substr: "",
            },
        },
        CapabilityRow {
            id: "cap-sm-crypto",
            name: "国密 SM2/3/4",
            detector: CapabilityDetector::TestPresence {
                crate_name: "eneros-crypto",
                test_name_substr: "test_",
            },
        },
        CapabilityRow {
            id: "cap-protocols",
            name: "IEC104/Modbus/IEC61850 协议",
            detector: CapabilityDetector::TestPresence {
                crate_name: "eneros-iec104-master",
                test_name_substr: "contract",
            },
        },
        CapabilityRow {
            id: "cap-ai-chain",
            name: "AI 决策链（LLM+MILP）",
            detector: CapabilityDetector::TestPresence {
                crate_name: "eneros-dual-brain",
                test_name_substr: "",
            },
        },
        CapabilityRow {
            id: "cap-attestation",
            name: "远程证明",
            detector: CapabilityDetector::EvidenceFile {
                path: "docs/security/attestation-design.md",
                required_fields: &[],
            },
        },
    ];

    let mut statuses = Vec::new();
    for row in &claimed {
        statuses.push((row.id, row.detector.evaluate(root)));
    }
    // 蓝图 36 WP-36.9 止血：TestPresence/EvidenceFile 只能 PENDING，
    // 不再产生 VERIFIED（假 PASS 路径封死）；完整产品图验证在 42 实现。
    // 报告行：detector 求值结果入 status（全部 PENDING 或缺失）。
    let capability_rows: Vec<serde_json::Value> = claimed
        .iter()
        .zip(statuses.iter())
        .map(|(row, (st, found))| {
            serde_json::json!({
                "id": row.id,
                "name": row.name,
                "detector": row.detector,
                "status": st,
                "evidence_present": found,
            })
        })
        .collect();
    let verdict = "G7-REVIEW";
    let report = G7Report {
        schema: "eneros.g7-consistency.v1",
        production_mock_refs: real_violations,
        claimed_capabilities: capability_rows,
        verdict,
    };
    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(&out_path, serde_json::to_vec_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("G7-AUDIT verdict={verdict} → {}", out_path.display());
    // 蓝图 36 WP-36.9：verdict 非 PASS（当前所有能力均为 PENDING）时
    // 默认非零退出；仅显式 --allow-non-pass 可生成信息报告（non_release）。
    if !allow_non_pass {
        return Err(invalid(
            "G7-AUDIT: capabilities lack product closure (manifest service + production call path + evidence oracle) — PENDING, not PASS",
        ));
    }
    Ok(())
}

fn command_soak_run(root: &Path, args: &[String]) -> DynResult<()> {
    use std::time::Instant;

    let mut cycles: usize = 10;
    let mut max_fail: usize = 1;
    let mut profile_id = "qemu-prod".to_owned();
    let mut image: Option<std::path::PathBuf> = None;
    let mut vars_template: Option<std::path::PathBuf> = None;
    let mut out_path = root.join("target/soak-campaign.json");
    // 33 GV-05：最短浸泡天数可配（默认 3=72h 14 号文 D1；CI 短周期显式
    // 传 0，禁止静默）。
    let mut min_duration_days: u64 = 3;
    // 33 GV-05：内存泄漏阈值（字节/天）——实测 RSS 回归斜率与之比较。
    let mut leak_threshold: u64 = 64 * 1024 * 1024;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--cycles" => {
                cycles = val()?
                    .parse()
                    .map_err(|_| invalid("--cycles must be usize"))?
            }
            "--max-fail" => {
                max_fail = val()?
                    .parse()
                    .map_err(|_| invalid("--max-fail must be usize"))?
            }
            "--profile" => profile_id = val()?.to_owned(),
            "--image" => image = Some(std::path::PathBuf::from(val()?)),
            "--vars-template" => vars_template = Some(std::path::PathBuf::from(val()?)),
            "--out" => out_path = root.join(val()?),
            "--min-duration-days" => {
                min_duration_days = val()?
                    .parse()
                    .map_err(|_| invalid("--min-duration-days must be u64"))?;
                if min_duration_days == 0 {
                    println!(
                        "WARNING: --min-duration-days=0 —— CI 短周期浸泡冒烟，\
                         非 14 号文 D1 72h 浸泡证据（显式标注，不得静默）"
                    );
                }
            }
            "--leak-threshold" => {
                leak_threshold = val()?
                    .parse()
                    .map_err(|_| invalid("--leak-threshold must be u64 (bytes/day)"))?;
            }
            other => return Err(invalid(format!("unknown soak-run arg '{other}'"))),
        }
    }
    if cycles == 0 {
        return Err(invalid("soak-run --cycles must be > 0"));
    }
    // 33 GV-05：artifact_sha256 实算需要产物路径（qemu-prod --image 本
    // 就强制），缺省显式报错而非带病运行。
    let image =
        image.ok_or_else(|| invalid("soak-run requires --image <path> (for artifact_sha256)"))?;

    // 1. 构造 qemu-prod 命令行（复用同一入口）。
    // 42（Round 131）：直接复用当前 xtask 可执行文件（`current_exe`），
    // 不再每轮 `cargo xtask` 重编译——消除外部源文件同步/改动导致的
    // 中途编译失败（实测 1000-cycle 在第 232/233 轮因源码被外部瞬时
    // 改写、`cargo xtask` 编译失败 exit 101 而中止；产品与 harness 本身
    // 无故障），同时显著降低每轮开销。
    let mut cmd =
        std::process::Command::new(std::env::current_exe().unwrap_or_else(|_| "cargo".into()));
    cmd.arg("run").arg("qemu-prod");
    cmd.arg("--image").arg(&image);
    if let Some(vt) = &vars_template {
        cmd.arg("--vars-template").arg(vt);
    }
    cmd.arg("--expect").arg("PASS");
    // 42：QEMU 受控退出 + 每轮重置 BOOTCTRL，使 soak-run 可重复执行，
    // 不再把 timeout 当 PASS。
    if !cfg!(windows) {
        cmd.arg("--qmp-shutdown").arg("--reset-bootctrl");
    }

    // 2. 循环启动；每轮结束后采样宿主 RSS（/proc/self/status VmRSS）与
    // 累计耗时，供泄漏回归（33 GV-05）。
    let mut restarts: Vec<SoakRestart> = Vec::new();
    let mut boot_secs: Vec<u64> = Vec::new();
    let mut rss_samples: Vec<(u64, u64)> = Vec::new(); // (累计秒, rss 字节)
    let mut failures: usize = 0;
    let mut elapsed_total: u64 = 0;
    // 42 WP-42.10：真实事务/错误/恢复统计——恢复 = 失败后的后续 cycle
    // 成功（故障注入后按 oracle 恢复的对偶；QEMU 适配器无注入时计数为
    // 引导失败的自动恢复）。
    let mut transactions_total: u64 = 0;
    let mut errors_total: u64 = 0;
    let mut recoveries_total: u64 = 0;
    let mut pending_recovery: bool = false;
    for cycle in 0..cycles {
        let start = Instant::now();
        let status = cmd.output();
        let elapsed = start.elapsed().as_secs();
        elapsed_total += elapsed;
        boot_secs.push(elapsed.max(1));
        match status {
            Ok(out) if out.status.success() => {
                println!("soak cycle {}/{} PASS in {}s", cycle + 1, cycles, elapsed);
                transactions_total += 1;
                if pending_recovery {
                    recoveries_total += 1;
                    pending_recovery = false;
                }
            }
            Ok(out) => {
                failures += 1;
                errors_total += 1;
                pending_recovery = true;
                let stderr = String::from_utf8_lossy(&out.stderr);
                println!(
                    "soak cycle {}/{} FAIL rc={} ({})",
                    cycle + 1,
                    cycles,
                    out.status,
                    stderr.lines().next().unwrap_or("no stderr")
                );
                restarts.push(SoakRestart {
                    restart_id: cycle as u64 + 1,
                    at_secs: elapsed,
                    root_cause: Some("qemu-prod-expect-failed".to_owned()),
                });
            }
            Err(e) => {
                failures += 1;
                errors_total += 1;
                pending_recovery = true;
                println!("soak cycle {}/{} SPAWN-ERR: {e}", cycle + 1, cycles);
                restarts.push(SoakRestart {
                    restart_id: cycle as u64 + 1,
                    at_secs: elapsed,
                    root_cause: Some(format!("spawn-error:{e}")),
                });
            }
        }
        if let Some(rss) = vm_rss_bytes() {
            rss_samples.push((elapsed_total, rss));
        }
        if failures > max_fail {
            return Err(invalid(format!(
                "soak-run aborted: {failures} failures exceed --max-fail {max_fail}"
            )));
        }
    }

    // 3. 汇总启动时间分布 + 写 boot-times.csv（14 号文 D1 证据形式）。
    let total: u64 = boot_secs.iter().sum();
    let avg = total / cycles as u64;
    let min = boot_secs.iter().copied().min().unwrap_or(0);
    let max = boot_secs.iter().copied().max().unwrap_or(0);
    println!("soak boot-time: avg={avg}s min={min}s max={max}s over {cycles} cycles");
    let csv_path = out_path.with_file_name("boot-times.csv");
    let csv: String = boot_secs
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{},{}\n", i + 1, s))
        .collect();
    fs::write(&csv_path, csv)?;

    // 4. 33 GV-05：artifact_sha256 实算（SHA-256——与验证判据 sha256sum
    // 一致；sha2 为 xtask 既有依赖）。
    let artifact_bytes = fs::read(&image).map_err(|e| {
        invalid(format!(
            "soak-run cannot read --image {}: {e}",
            image.display()
        ))
    })?;
    let artifact_sha256 = hex_encode(&Sha256::digest(&artifact_bytes));
    println!(
        "soak artifact sha256={artifact_sha256} ({})",
        image.display()
    );

    // 42：候选血缘绑定——soak 证据必须指向当前候选，禁止旧 evidence 复用。
    // 非 git 临时目录（单元测试）不伪造血缘，字段留空；正式运行必须 .git 存在。
    let lineage = if root.join(".git").exists() {
        Some(super::evidence_chain::compute_lineage(
            root,
            &profile_id,
            Some(&image),
        )?)
    } else {
        None
    };

    // 5. 33 GV-05：宿主 RSS 采样序列线性回归 → 泄漏率（字节/天）。
    // 采样不足（非 Linux /proc 缺失或样本 <2）→ 0 并显式警告（不静默）。
    let memory_leak_bytes_per_day = if rss_samples.len() >= 2 {
        linear_slope_per_day(&rss_samples)
    } else {
        println!(
            "WARNING: RSS 采样不足（{} 样本），memory_leak 记 0——非静默",
            rss_samples.len()
        );
        0
    };
    println!(
        "soak memory-leak regression: {} bytes/day (threshold {})",
        memory_leak_bytes_per_day, leak_threshold
    );

    // 6. 产出 QA-018 证据 JSON。
    let campaign = SoakCampaign {
        schema: "eneros.soak-campaign.v1".into(),
        campaign_id: format!("qemu-soak-{cycles}x-{avg}s"),
        artifact_sha256,
        candidate_id: lineage
            .as_ref()
            .map(|l| l.candidate_id.clone())
            .unwrap_or_default(),
        source_commit: lineage
            .as_ref()
            .map(|l| l.source_commit.clone())
            .unwrap_or_default(),
        dirty_tree_digest: lineage
            .as_ref()
            .map(|l| l.dirty_tree_digest.clone())
            .unwrap_or_default(),
        product_manifest_digest: lineage
            .as_ref()
            .map(|l| l.product_manifest_digest.clone())
            .unwrap_or_default(),
        profile_digest: lineage
            .as_ref()
            .map(|l| l.profile_digest.clone())
            .unwrap_or_default(),
        toolchain_digest: lineage
            .as_ref()
            .map(|l| l.toolchain_digest.clone())
            .unwrap_or_default(),
        platform_inventory_digest: lineage
            .as_ref()
            .map(|l| l.platform_inventory_digest.clone())
            .unwrap_or_default(),
        // duration 以实际秒数为准（33 GV-05：门禁阈值经 --min-duration-days
        // 可配，短周期 CI 显式 0）。
        duration_days: ((total as f64 / 86400.0).max(0.0)) as u64,
        min_duration_days,
        memory_leak_bytes_per_day,
        memory_leak_threshold: leak_threshold,
        // QEMU 浸泡仅验证引导链（无数据负载写入），数据腐化检测不适用
        //（真机 72h 浸泡脚本负责数据面）——false 为如实声明（33 GV-05）。
        data_corruption: false,
        restarts,
        metric_samples: cycles as u64,
        // 42 WP-42.10：真实统计——事务=成功引导数，错误=失败引导数，
        // 恢复=失败后成功数；QEMU 适配器无故障注入，如实声明。
        workload_coverage: vec!["boot-chain".to_owned()],
        transactions_total,
        errors_total,
        recoveries_total,
        fault_stats: Some(SoakFaultStats {
            injected: 0,
            recovered: 0,
            injection_confirmed: false,
        }),
    };
    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(&out_path, serde_json::to_vec_pretty(&campaign)?)?;
    println!("SOAK-RUN evidence written to {}", out_path.display());

    if failures > 0 {
        return Err(invalid(format!(
            "soak-run completed with {failures} failed cycles"
        )));
    }
    Ok(())
}

/// 宿主 RSS（字节）：/proc/self/status VmRSS（33 GV-05 泄漏回归采样源）。
/// 非 Linux 或无 /proc 返回 None。
fn vm_rss_bytes() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

/// RSS 采样序列（x=累计秒, y=字节）线性回归斜率 → 字节/天。
/// 负斜率（RSS 下降）钳为 0；样本 <2 或分母为 0 返回 0。
fn linear_slope_per_day(samples: &[(u64, u64)]) -> u64 {
    let n = samples.len();
    if n < 2 {
        return 0;
    }
    let (sum_x, sum_y, sum_xy, sum_xx) =
        samples
            .iter()
            .fold((0f64, 0f64, 0f64, 0f64), |(sx, sy, sxy, sxx), &(x, y)| {
                let (x, y) = (x as f64, y as f64);
                (sx + x, sy + y, sxy + x * y, sxx + x * x)
            });
    let denom = n as f64 * sum_xx - sum_x * sum_x;
    if denom.abs() < f64::EPSILON {
        return 0;
    }
    let slope_per_sec = (n as f64 * sum_xy - sum_x * sum_y) / denom;
    if slope_per_sec <= 0.0 {
        return 0;
    }
    (slope_per_sec * 86400.0) as u64
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
    // 33 GV-05：artifact_sha256 必须实算（自证链——硬编码空串不得过门）。
    if campaign.artifact_sha256.is_empty() {
        report.violations.push("missing-artifact-sha256".to_owned());
    }
    if campaign.duration_days < campaign.min_duration_days {
        report.violations.push(format!(
            "insufficient-duration:{}d<{}d",
            campaign.duration_days, campaign.min_duration_days
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct PerfResults {
    schema: String,
    baseline_sha256: String,
    candidate_sha256: String,
    /// 42 WP-42.12：测量时刻与本次运行 id（candidate 必须不同于 baseline
    /// ——measured_at/run_id 不得复用，防止旧结果冒充新测量）。
    #[serde(default)]
    measured_at: String,
    #[serde(default)]
    run_id: String,
    /// 生成候选报告时从冻结 baseline 复制的测量时刻（空 = 首建基线）。
    #[serde(default)]
    baseline_measured_at: String,
    /// 42 candidate lineage 绑定。
    #[serde(default)]
    candidate_id: String,
    #[serde(default)]
    source_commit: String,
    #[serde(default)]
    dirty_tree_digest: String,
    #[serde(default)]
    product_manifest_digest: String,
    #[serde(default)]
    profile_digest: String,
    #[serde(default)]
    toolchain_digest: String,
    #[serde(default)]
    platform_inventory_digest: String,
    metrics: Vec<PerfMetric>,
}

#[derive(Debug, Deserialize, Serialize)]
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
    /// 29 GV-03：指标方向（higher_is_better 语义）。缺省按指标名白名单
    /// 推断（`default_higher_is_better`），兼容旧基线 JSON。
    #[serde(default)]
    direction: Option<MetricDirection>,
    /// 42 WP-42.12：采样样本（ops/s 每 rep，排序存储）。
    #[serde(default)]
    samples: Vec<u64>,
    /// 42 WP-42.12：百分位与置信区间（95%）。
    #[serde(default)]
    p50: u64,
    #[serde(default)]
    p95: u64,
    #[serde(default)]
    ci_low: u64,
    #[serde(default)]
    ci_high: u64,
    /// 42 WP-42.12：warmup 迭代次数（不计入样本）。
    #[serde(default)]
    warmup_ops: u64,
    /// 42 WP-42.12：可选区间 SLO（上下双界；任一存在即按 range 校验）。
    #[serde(default)]
    slo_lower_bound: Option<u64>,
    #[serde(default)]
    slo_upper_bound: Option<u64>,
}

/// 指标方向：吞吐类（frames/s、parses/s）越高越好；延迟/耗时类越低越好。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum MetricDirection {
    HigherIsBetter,
    LowerIsBetter,
}

/// 指标方向白名单（缺省推断）：现存 5 项基线全部为吞吐类。
fn default_higher_is_better(metric_id: &str) -> bool {
    matches!(
        metric_id,
        "iec104_encode" | "ipc_wire_roundtrip" | "modbus_rtu_frame" | "elf_parse" | "gguf_parse"
    )
}

/// 29 GV-03：回归判定（方向感知）。GV-03 修复前方向反了——`measured >
/// baseline` 才计 delta，真实退化（measured 下降）静默放行。
///
/// - higher_is_better（吞吐）：measured < baseline * (1 - max_pct%) → 退化；
/// - lower_is_better（延迟）：measured > baseline * (1 + max_pct%) → 退化。
fn regressed(
    measured: f64,
    baseline: f64,
    higher_is_better: bool,
    max_regression_pct: f64,
) -> bool {
    if baseline <= 0.0 {
        return false;
    }
    let tolerance = max_regression_pct / 100.0;
    if higher_is_better {
        measured < baseline * (1.0 - tolerance)
    } else {
        measured > baseline * (1.0 + tolerance)
    }
}

// ============================================================
// QA-019b: performance baseline generation (v0.251.0, C-251)
// ============================================================
/// `qa perf-baseline [--out <path>]`：真实测量 host 可测的 5 类指标并
/// 生成性能基线（QA-019 数据源——替代自证 JSON）。
///
/// 测量项（host 实测，非自证）：
/// - iec104_encode：APDU I 帧编解码吞吐（帧/s）
/// - ipc_wire_roundtrip：El0Frame 编解码吞吐（帧/s）
/// - modbus_rtu_frame：Modbus 帧解析吞吐（帧/s）
/// - elf_parse：ELF 解析吞吐（帧/s）
/// - gguf_parse：GGUF 头解析吞吐（帧/s）
///
/// 基线用 ed25519 私钥签名（`--sign-key <path>`，无 key 时仅生成未签名
/// 基线并警告）；`perf-gate` 校验签名后执行 SLO/回归门禁。
fn command_perf_baseline(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out_path = root.join("target/perf-results.json");
    let mut sign_key: Option<std::path::PathBuf> = None;
    let mut slo_threshold: u64 = 0;
    let mut tolerance_pct: u64 = 10;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => {
                out_path = root.join(
                    it.next()
                        .ok_or_else(|| invalid("perf-baseline --out requires <path>"))?,
                );
            }
            "--sign-key" => {
                sign_key = Some(
                    root.join(
                        it.next()
                            .ok_or_else(|| invalid("perf-baseline --sign-key requires <path>"))?,
                    ),
                );
            }
            "--slo-threshold" => {
                slo_threshold = it
                    .next()
                    .ok_or_else(|| invalid("perf-baseline --slo-threshold requires <u64>"))?
                    .parse()
                    .map_err(|_| invalid("perf-baseline --slo-threshold must be u64"))?;
                if slo_threshold == 0 {
                    return Err(invalid(
                        "perf-baseline --slo-threshold must be > 0 to freeze an SLO",
                    ));
                }
            }
            "--tolerance-pct" => {
                tolerance_pct = it
                    .next()
                    .ok_or_else(|| invalid("perf-baseline --tolerance-pct requires <u64>"))?
                    .parse()
                    .map_err(|_| invalid("perf-baseline --tolerance-pct must be u64"))?;
                if tolerance_pct == 0 {
                    return Err(invalid("perf-baseline --tolerance-pct must be > 0"));
                }
            }
            other => return Err(invalid(format!("unknown perf-baseline arg '{other}'"))),
        }
    }

    let metrics = measure_perf_metrics(slo_threshold, tolerance_pct, None);

    // 2. 序列化 + 摘要。
    let mut results = PerfResults {
        schema: "eneros.perf-regression.v1".into(),
        baseline_sha256: String::new(),
        candidate_sha256: String::new(),
        // 42 WP-42.12：基线本身也是真实测量（时刻/run_id 唯一）。
        measured_at: rfc3339_now(),
        run_id: perf_run_id(),
        baseline_measured_at: String::new(),
        candidate_id: String::new(),
        source_commit: String::new(),
        dirty_tree_digest: String::new(),
        product_manifest_digest: String::new(),
        profile_digest: String::new(),
        toolchain_digest: String::new(),
        platform_inventory_digest: String::new(),
        metrics,
    };
    if root.join(".git").exists() {
        let lineage = super::evidence_chain::compute_lineage(root, "qemu-prod", None)?;
        results.candidate_id = lineage.candidate_id;
        results.source_commit = lineage.source_commit;
        results.dirty_tree_digest = lineage.dirty_tree_digest;
        results.product_manifest_digest = lineage.product_manifest_digest;
        results.profile_digest = lineage.profile_digest;
        results.toolchain_digest = lineage.toolchain_digest;
        results.platform_inventory_digest = lineage.platform_inventory_digest;
    }
    let json = serde_json::to_vec_pretty(&results)?;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(&json);
    let digest_hex = hex_encode(&digest);
    results.baseline_sha256 = digest_hex.clone();
    let json_signed = serde_json::to_vec_pretty(&results)?;

    // 3. 可选签名。
    let signature = match &sign_key {
        Some(key_path) => {
            let key_bytes =
                fs::read(key_path).map_err(|e| invalid(format!("read sign key: {e}")))?;
            let keypair = ed25519_dalek::SigningKey::from_bytes(
                &key_bytes[..32]
                    .try_into()
                    .map_err(|_| invalid("key must be 32 bytes"))?,
            );
            use ed25519_dalek::Signer;
            let sig = keypair.sign(&json_signed);
            hex_encode(&sig.to_bytes())
        }
        None => {
            println!("WARN: perf-baseline unsigned (pass --sign-key for tamper protection)");
            String::new()
        }
    };

    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(&out_path, &json_signed)?;
    let sig_path = out_path.with_extension("json.sig");
    fs::write(&sig_path, &signature)?;

    println!(
        "PERF-BASELINE schema=eneros.perf-regression.v1 metrics={} sha256={} signed={} slo={}",
        results.metrics.len(),
        digest_hex,
        !signature.is_empty(),
        slo_threshold
    );
    for m in &results.metrics {
        println!(
            "  {} = {} {} (tolerance {}%)",
            m.metric_id, m.measured, m.unit, m.regression_tolerance_pct
        );
    }
    Ok(())
}

/// `qa perf-measure --baseline <baseline.json> [--out <candidate.json>] [--sign-key <key>]`
///
/// 42：候选性能测量。读取冻结 baseline 的 SLO/基线值/方向，只更新实测值，
/// 生成 `candidate_sha256` 非空且不等于 baseline 的候选报告，供
/// `perf-gate` 做外部基线回归。禁止候选自建 baseline。
fn command_perf_measure(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out_path = root.join("target/perf-candidate.json");
    let mut baseline_path: Option<std::path::PathBuf> = None;
    let mut sign_key: Option<std::path::PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--baseline" => {
                baseline_path = Some(
                    root.join(
                        it.next()
                            .ok_or_else(|| invalid("perf-measure --baseline requires <path>"))?,
                    ),
                );
            }
            "--out" => {
                out_path = root.join(
                    it.next()
                        .ok_or_else(|| invalid("perf-measure --out requires <path>"))?,
                );
            }
            "--sign-key" => {
                sign_key = Some(
                    root.join(
                        it.next()
                            .ok_or_else(|| invalid("perf-measure --sign-key requires <path>"))?,
                    ),
                );
            }
            other => return Err(invalid(format!("unknown perf-measure arg '{other}'"))),
        }
    }
    let baseline_path =
        baseline_path.ok_or_else(|| invalid("perf-measure requires --baseline <baseline.json>"))?;
    let baseline: PerfResults = read_json(&baseline_path)?;
    if baseline.schema != "eneros.perf-regression.v1" {
        return Err(invalid("perf-measure baseline schema mismatch"));
    }
    if baseline.baseline_sha256.is_empty() {
        return Err(invalid("perf-measure baseline has no baseline_sha256"));
    }
    const REQUIRED_PERF_METRICS: [&str; 5] = [
        "iec104_encode",
        "ipc_wire_roundtrip",
        "modbus_rtu_frame",
        "elf_parse",
        "gguf_parse",
    ];
    for id in REQUIRED_PERF_METRICS {
        let Some(metric) = baseline.metrics.iter().find(|m| m.metric_id == id) else {
            return Err(invalid(format!(
                "perf-measure baseline missing required metric {id}"
            )));
        };
        if metric.slo_threshold == 0 {
            return Err(invalid(format!(
                "perf-measure baseline has unfrozen SLO for {id} (slo_threshold=0); freeze baseline first"
            )));
        }
    }

    let metrics = measure_perf_metrics(0, 0, Some(&baseline));
    let mut results = PerfResults {
        schema: "eneros.perf-regression.v1".into(),
        baseline_sha256: baseline.baseline_sha256.clone(),
        candidate_sha256: String::new(),
        // 42 WP-42.12：候选测量的时刻/run_id 必须不同于基线（门禁校验）。
        measured_at: rfc3339_now(),
        run_id: perf_run_id(),
        baseline_measured_at: baseline.measured_at.clone(),
        candidate_id: String::new(),
        source_commit: String::new(),
        dirty_tree_digest: String::new(),
        product_manifest_digest: String::new(),
        profile_digest: String::new(),
        toolchain_digest: String::new(),
        platform_inventory_digest: String::new(),
        metrics,
    };
    if root.join(".git").exists() {
        let lineage = super::evidence_chain::compute_lineage(root, "qemu-prod", None)?;
        results.candidate_id = lineage.candidate_id;
        results.source_commit = lineage.source_commit;
        results.dirty_tree_digest = lineage.dirty_tree_digest;
        results.product_manifest_digest = lineage.product_manifest_digest;
        results.profile_digest = lineage.profile_digest;
        results.toolchain_digest = lineage.toolchain_digest;
        results.platform_inventory_digest = lineage.platform_inventory_digest;
    }
    let json = serde_json::to_vec_pretty(&results)?;
    use sha2::{Digest, Sha256};
    let digest_hex = hex_encode(&Sha256::digest(&json));
    results.candidate_sha256 = digest_hex.clone();
    let json_signed = serde_json::to_vec_pretty(&results)?;

    let signature = match &sign_key {
        Some(key_path) => {
            let key_bytes =
                fs::read(key_path).map_err(|e| invalid(format!("read sign key: {e}")))?;
            let keypair = ed25519_dalek::SigningKey::from_bytes(
                &key_bytes[..32]
                    .try_into()
                    .map_err(|_| invalid("key must be 32 bytes"))?,
            );
            use ed25519_dalek::Signer;
            let sig = keypair.sign(&json_signed);
            hex_encode(&sig.to_bytes())
        }
        None => {
            println!("WARN: perf-measure unsigned (pass --sign-key for tamper protection)");
            String::new()
        }
    };

    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(&out_path, &json_signed)?;
    let sig_path = out_path.with_extension("json.sig");
    fs::write(&sig_path, &signature)?;
    println!(
        "PERF-MEASURE schema=eneros.perf-regression.v1 metrics={} baseline={} candidate={} signed={}",
        results.metrics.len(),
        results.baseline_sha256,
        digest_hex,
        !signature.is_empty()
    );
    for m in &results.metrics {
        println!(
            "  {} = {} {} (baseline={} slo={})",
            m.metric_id, m.measured, m.unit, m.baseline_value, m.slo_threshold
        );
    }
    Ok(())
}

/// 真实测量当前 host 的 5 个性能指标。`baseline` 为 `None` 时生成初始
/// baseline（`baseline_value` = 本次 measured）；为 `Some` 时生成候选
/// 报告（`slo_threshold`/`baseline_value`/`direction`/`unit` 继承冻结
/// baseline，仅 `measured` 为本次实测）。
fn measure_perf_metrics(
    slo_threshold: u64,
    default_tolerance_pct: u64,
    baseline: Option<&PerfResults>,
) -> Vec<PerfMetric> {
    use std::time::Instant;
    // 42 WP-42.12：采样式测量——1 次 warmup（不计入样本）+ `reps` 次
    // 计时样本，产出 percentile/置信区间；单次计时受调度噪声支配，
    // 不可作为冻结 SLO 依据。
    fn bench_ops_samples<F: FnMut()>(ops: usize, reps: usize, mut f: F) -> Vec<f64> {
        let _ = Instant::now();
        for _ in 0..ops {
            f();
        }
        let mut samples = Vec::with_capacity(reps);
        for _ in 0..reps {
            let start = Instant::now();
            for _ in 0..ops {
                f();
            }
            let secs = start.elapsed().as_secs_f64();
            samples.push(ops as f64 / secs);
        }
        samples
    }

    /// (mean, p50, p95, ci_low, ci_high)；样本 <2 时 CI 记 0（非静默，
    /// 由调用方按样本数解释）。
    fn stat_summary(samples: &[f64]) -> (u64, u64, u64, u64, u64) {
        let n = samples.len() as f64;
        if n == 0.0 {
            return (0, 0, 0, 0, 0);
        }
        let mean = samples.iter().sum::<f64>() / n;
        let mut sorted: Vec<u64> = samples.iter().map(|s| *s as u64).collect();
        sorted.sort_unstable();
        let p50 = sorted.get(sorted.len() / 2).copied().unwrap_or(0);
        let p95_idx = ((sorted.len().saturating_sub(1)) as f64 * 0.95) as usize;
        let p95 = sorted.get(p95_idx).copied().unwrap_or(0);
        let variance = samples.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
        let std_dev = variance.sqrt();
        let ci = 1.96 * std_dev / n.sqrt();
        (
            mean as u64,
            p50,
            p95,
            (mean - ci).max(0.0) as u64,
            (mean + ci) as u64,
        )
    }

    /// 组装指标：`measured`=均值；samples 排序存储；range（上下双界）
    /// 为可选冻结区间。
    #[allow(clippy::too_many_arguments)] // 44.2：perf 指标组装助手（10 参结构化值，重构代价>收益，登记）
    fn metric(
        metric_id: &str,
        samples: Vec<f64>,
        slo: u64,
        base: u64,
        tol: u64,
        crit: bool,
        unit: &str,
        dir: Option<MetricDirection>,
        lower: Option<u64>,
        upper: Option<u64>,
    ) -> PerfMetric {
        let (measured, p50, p95, ci_low, ci_high) = stat_summary(&samples);
        let mut sorted: Vec<u64> = samples.iter().map(|v| *v as u64).collect();
        sorted.sort_unstable();
        PerfMetric {
            metric_id: metric_id.into(),
            slo_threshold: slo,
            measured,
            regression_tolerance_pct: tol,
            baseline_value: base,
            critical_path: crit,
            unit: unit.into(),
            direction: dir,
            samples: sorted,
            p50,
            p95,
            ci_low,
            ci_high,
            warmup_ops: 1,
            slo_lower_bound: lower,
            slo_upper_bound: upper,
        }
    }

    // iec104 APDU I 帧编解码。
    let asdu = eneros_iec104_slave::asdu::Asdu {
        type_id: eneros_iec104_slave::asdu::TypeId::MeasuredValueFloat,
        cause_of_tx: eneros_iec104_slave::asdu::Cot::Periodic,
        common_addr: 1,
        ioas: vec![],
    };
    let iec104_apdu = eneros_iec104_slave::apdu::Apdu::i_format(1, 1, asdu);
    let iec104_bytes = iec104_apdu.encode().unwrap();
    let iec104_samples = bench_ops_samples(50_000, 5, || {
        let _ = eneros_iec104_slave::apdu::Apdu::decode(&iec104_bytes);
    });

    // ipc-wire 帧编解码。
    let wire_frame = eneros_ipc_wire::el0::encode_frame(
        eneros_ipc_wire::el0::El0FrameType::Report,
        b"payload-32-bytes-aaaaaaaaaaaaaaaa",
    );
    let wire_samples = bench_ops_samples(50_000, 5, || {
        let _ = eneros_ipc_wire::el0::decode_frame(&wire_frame);
    });

    // Modbus RTU 帧解析（构造含 CRC 的合法帧）。
    let mut modbus_frame = vec![0x01u8, 0x03, 0x00, 0x00, 0x00, 0x02];
    let crc = modbus_crc16(&modbus_frame);
    modbus_frame.push((crc & 0xFF) as u8);
    modbus_frame.push((crc >> 8) as u8);
    let modbus_samples = bench_ops_samples(50_000, 5, || {
        let _ = eneros_modbus_rtu::frame::ModbusFrame::decode(&modbus_frame);
    });

    // ELF 解析（构造最小合法 ELF64）。
    let elf_bytes = minimal_elf64();
    let elf_samples = bench_ops_samples(20_000, 5, || {
        let _ = eneros_elf_loader::LoadedElf::parse(&elf_bytes);
    });

    // GGUF 头解析。
    let gguf_bytes = minimal_gguf();
    let gguf_samples = bench_ops_samples(50_000, 5, || {
        let _ = eneros_gguf_loader::header::GgufHeader::parse(&gguf_bytes);
    });

    let s = |id: &str| -> Option<&PerfMetric> {
        baseline.and_then(|r| r.metrics.iter().find(|m| m.metric_id == id))
    };
    let t = |id: &str,
             fallback_critical: bool,
             fallback_unit: &str|
     -> (u64, u64, u64, bool, String, Option<MetricDirection>) {
        let bm = s(id);
        (
            if slo_threshold != 0 {
                slo_threshold
            } else {
                bm.map(|m| m.slo_threshold).unwrap_or(0)
            },
            bm.map(|m| m.baseline_value).unwrap_or(0),
            bm.map(|m| m.regression_tolerance_pct)
                .unwrap_or(default_tolerance_pct),
            bm.map(|m| m.critical_path).unwrap_or(fallback_critical),
            bm.map(|m| m.unit.as_str())
                .unwrap_or(fallback_unit)
                .to_owned(),
            bm.and_then(|m| m.direction)
                .or(Some(MetricDirection::HigherIsBetter)),
        )
    };
    // range（可选上下双界）从冻结 baseline 继承。
    let bounds = |id: &str| -> (Option<u64>, Option<u64>) {
        s(id)
            .map(|m| (m.slo_lower_bound, m.slo_upper_bound))
            .unwrap_or((None, None))
    };

    let (iec104_slo, iec104_base, iec104_tol, iec104_crit, iec104_unit, iec104_dir) =
        t("iec104_encode", false, "frames/s");
    let (wire_slo, wire_base, wire_tol, wire_crit, wire_unit, wire_dir) =
        t("ipc_wire_roundtrip", true, "frames/s");
    let (modbus_slo, modbus_base, modbus_tol, modbus_crit, modbus_unit, modbus_dir) =
        t("modbus_rtu_frame", false, "frames/s");
    let (elf_slo, elf_base, elf_tol, elf_crit, elf_unit, elf_dir) =
        t("elf_parse", true, "parses/s");
    let (gguf_slo, gguf_base, gguf_tol, gguf_crit, gguf_unit, gguf_dir) =
        t("gguf_parse", false, "parses/s");

    let (iec104_lo, iec104_hi) = bounds("iec104_encode");
    let (wire_lo, wire_hi) = bounds("ipc_wire_roundtrip");
    let (modbus_lo, modbus_hi) = bounds("modbus_rtu_frame");
    let (elf_lo, elf_hi) = bounds("elf_parse");
    let (gguf_lo, gguf_hi) = bounds("gguf_parse");

    let base_of = |measured: &[f64]| -> u64 {
        let (mean, _, _, _, _) = stat_summary(measured);
        mean
    };
    // baseline 生成时 baseline_value = 本次实测均值（先算，避免移动）。
    let iec104_measured = base_of(&iec104_samples);
    let wire_measured = base_of(&wire_samples);
    let modbus_measured = base_of(&modbus_samples);
    let elf_measured = base_of(&elf_samples);
    let gguf_measured = base_of(&gguf_samples);
    vec![
        metric(
            "iec104_encode",
            iec104_samples,
            iec104_slo,
            if baseline.is_some() {
                iec104_base
            } else {
                iec104_measured
            },
            iec104_tol,
            iec104_crit,
            &iec104_unit,
            iec104_dir,
            iec104_lo,
            iec104_hi,
        ),
        metric(
            "ipc_wire_roundtrip",
            wire_samples,
            wire_slo,
            if baseline.is_some() {
                wire_base
            } else {
                wire_measured
            },
            wire_tol,
            wire_crit,
            &wire_unit,
            wire_dir,
            wire_lo,
            wire_hi,
        ),
        metric(
            "modbus_rtu_frame",
            modbus_samples,
            modbus_slo,
            if baseline.is_some() {
                modbus_base
            } else {
                modbus_measured
            },
            modbus_tol,
            modbus_crit,
            &modbus_unit,
            modbus_dir,
            modbus_lo,
            modbus_hi,
        ),
        metric(
            "elf_parse",
            elf_samples,
            elf_slo,
            if baseline.is_some() {
                elf_base
            } else {
                elf_measured
            },
            elf_tol,
            elf_crit,
            &elf_unit,
            elf_dir,
            elf_lo,
            elf_hi,
        ),
        metric(
            "gguf_parse",
            gguf_samples,
            gguf_slo,
            if baseline.is_some() {
                gguf_base
            } else {
                gguf_measured
            },
            gguf_tol,
            gguf_crit,
            &gguf_unit,
            gguf_dir,
            gguf_lo,
            gguf_hi,
        ),
    ]
}

/// Modbus CRC16（与 modbus-rtu crate 同算法，构造合法帧用）。
fn modbus_crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// 最小合法 ELF64（aarch64 ET_DYN）。
fn minimal_elf64() -> Vec<u8> {
    let mut elf = vec![0u8; 64];
    elf[0..4].copy_from_slice(b"\x7fELF");
    elf[4] = 2; // ELFCLASS64
    elf[5] = 1; // little-endian
    elf[6] = 1; // EV_CURRENT
    elf[16..18].copy_from_slice(&0x3u16.to_le_bytes()); // ET_DYN
    elf[18..20].copy_from_slice(&0xB7u16.to_le_bytes()); // AArch64
    elf[20..24].copy_from_slice(&1u32.to_le_bytes()); // EV_CURRENT
    elf[40..48].copy_from_slice(&0x1000u64.to_le_bytes()); // entry
    elf[56..64].copy_from_slice(&64u64.to_le_bytes()); // phoff
                                                       // 一个 PT_LOAD 段（R|X，非 W——满足 W^X）。
                                                       // ELF64 Phdr: type(0,4) flags(4,8) offset(8,16) vaddr(16,24)
                                                       //             paddr(24,32) filesz(32,40) memsz(40,48) align(48,56)
    let mut ph = vec![0u8; 56];
    ph[0..4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    ph[4..8].copy_from_slice(&5u32.to_le_bytes()); // flags R|X
    ph[8..16].copy_from_slice(&0x1000u64.to_le_bytes()); // offset
    ph[16..24].copy_from_slice(&0x1000u64.to_le_bytes()); // vaddr
    ph[24..32].copy_from_slice(&0x1000u64.to_le_bytes()); // paddr
    ph[32..40].copy_from_slice(&1u64.to_le_bytes()); // filesz
    ph[40..48].copy_from_slice(&1u64.to_le_bytes()); // memsz
    ph[48..56].copy_from_slice(&0x1000u64.to_le_bytes()); // align
    elf.extend_from_slice(&ph);
    elf
}

/// 最小合法 GGUF 头。
fn minimal_gguf() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"GGUF");
    buf.extend_from_slice(&3u32.to_le_bytes()); // version
    buf.extend_from_slice(&0u64.to_le_bytes()); // tensor_count
    buf.extend_from_slice(&0u64.to_le_bytes()); // metadata_kv_count
    buf
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn hex_decode(s: &str) -> Vec<u8> {
    if s.len() % 2 != 0 {
        return Vec::new();
    }
    (0..s.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// `qa perf-keygen --out <privkey-file> [--pubkey-out <hex-file>]`
///
/// WP-26.1：生成 CI 用的临时 ed25519 签名密钥对（私钥 32 字节 raw seed；
/// 公钥 32 字节 hex 写入 `--pubkey-out`）。CI 无持久密钥的现实下，现场
/// 生成密钥 → perf-baseline --sign-key 签名 → perf-gate 用同一公钥验证，
/// 使签名链真实存在（不再 rm -f 绕过）。
fn command_perf_keygen(root: &Path, args: &[String]) -> DynResult<()> {
    let mut out: Option<std::path::PathBuf> = None;
    let mut pubkey_out: Option<std::path::PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--out" => out = Some(root.join(val()?)),
            "--pubkey-out" => pubkey_out = Some(root.join(val()?)),
            other => return Err(invalid(format!("unknown perf-keygen arg '{other}'"))),
        }
    }
    let out = out.ok_or_else(|| invalid("perf-keygen requires --out <file>"))?;
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| invalid(format!("getrandom: {e}")))?;
    let keypair = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pubkey = keypair.verifying_key().to_bytes();
    fs::write(&out, seed)?;
    let pk_note = match &pubkey_out {
        Some(pk) => {
            fs::write(pk, hex_encode(&pubkey))?;
            pk.display().to_string()
        }
        None => "not written".to_owned(),
    };
    println!("PERF-KEYGEN → {} (pubkey {pk_note})", out.display());
    Ok(())
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

    // 蓝图 36 WP-36.10 #8：perf 不允许 candidate 创建自己的 baseline；
    // SLO=0（未冻结）或 baseline==candidate（自基线）→ 失败。
    if results.baseline_sha256.is_empty() {
        report.violations.push("baseline-missing".to_owned());
    }
    if results.baseline_sha256 == results.candidate_sha256 && !results.baseline_sha256.is_empty() {
        report
            .violations
            .push("self-baseline:candidate-created-its-own-baseline".to_owned());
    }
    // 42 WP-42.12：measured_at/run_id 必须存在且不同于基线测量时刻
    // （旧结果冒充新测量 = 自证伪造）。
    if results.measured_at.is_empty() {
        report.violations.push("measured-at-missing".to_owned());
    }
    if results.run_id.is_empty() {
        report.violations.push("run-id-missing".to_owned());
    }
    if !results.baseline_measured_at.is_empty()
        && results.baseline_measured_at == results.measured_at
    {
        report
            .violations
            .push("measured-at-same-as-baseline".to_owned());
    }
    for metric in &results.metrics {
        if metric.slo_threshold == 0 {
            report
                .violations
                .push(format!("slo-unfrozen:{}", metric.metric_id));
        }
    }

    // v0.251.0（C-251）：基线签名防篡改——若存在 <results>.sig（由
    // perf-baseline --sign-key 生成），用 CI 注入的公钥验证；签名缺失
    // 或无效 → 门禁失败（防篡改要求，13 号文 QA-021 基线签名）。
    // WP-26.1：签名文件缺失 → 该项 FAIL（fail-closed）——旧行为（CI 先
    // rm -f *.sig 再校验）主动绕过签名链，已删除。
    let sig_path = path.with_extension("json.sig");
    if !sig_path.exists() {
        report.violations.push("signature-missing".to_owned());
    }
    if sig_path.exists() {
        let sig_hex = fs::read_to_string(&sig_path).unwrap_or_default();
        let sig_bytes = hex_decode(sig_hex.trim());
        let ok = match std::env::var("ENEROS_PERF_VERIFY_PUBKEY") {
            Ok(pk_hex) => {
                let pk_bytes: [u8; 32] = match hex_decode(pk_hex.trim()).try_into() {
                    Ok(b) => b,
                    Err(_) => {
                        report
                            .violations
                            .push("signature-verify:bad-pubkey".to_owned());
                        return Err(invalid("bad verify pubkey (need 32-byte ed25519 pk)"));
                    }
                };
                if sig_bytes.len() == 64 {
                    use ed25519_dalek::Verifier;
                    let sig: [u8; 64] = match sig_bytes.try_into() {
                        Ok(s) => s,
                        Err(_) => return Err(invalid("bad signature length")),
                    };
                    let pk = ed25519_dalek::VerifyingKey::from_bytes(&pk_bytes);
                    let sig = ed25519_dalek::Signature::from_bytes(&sig);
                    let raw = fs::read(&path).unwrap_or_default();
                    pk.map(|p| p.verify(&raw, &sig).is_ok()).unwrap_or(false)
                } else {
                    false
                }
            }
            _ => {
                // 无公钥环境：签名存在但无法验证 → 违规（34 GV-11：
                // 移除 perf 逃生门（旧实现可
                // 静默跳过未验证签名，证据链断裂）。
                report
                    .violations
                    .push("signature-unverifiable:set-ENEROS_PERF_VERIFY_PUBKEY".to_owned());
                true
            }
        };
        if !ok {
            report.violations.push("signature-invalid".to_owned());
        }
    }
    if results.metrics.is_empty() {
        report.violations.push("no-metrics".to_owned());
    }

    let mut regressed_count = 0usize;
    let mut slo_violations = 0usize;
    for metric in &results.metrics {
        // 关键路径：SLO 必须按方向解释。slo_threshold=0 表示未冻结，
        // 跳过——首次基线不设 SLO，人工冻结后生效。
        // - higher_is_better（吞吐/速率）：实测低于下限 → 违规；
        // - lower_is_better（延迟/耗时）：实测超过上限 → 违规。
        if metric.critical_path && metric.slo_threshold > 0 {
            let higher_is_better = metric
                .direction
                .map(|d| d == MetricDirection::HigherIsBetter)
                .unwrap_or_else(|| default_higher_is_better(&metric.metric_id));
            let slo_bad = if higher_is_better {
                metric.measured < metric.slo_threshold
            } else {
                metric.measured > metric.slo_threshold
            };
            if slo_bad {
                let op = if higher_is_better { "<" } else { ">" };
                report.violations.push(format!(
                    "slo-violation:{}:{} {op} {}",
                    metric.metric_id, metric.measured, metric.slo_threshold
                ));
                slo_violations += 1;
            }
        }
        // 回归检查：实测相对基线退化不得超过容忍度（29 GV-03：方向感知
        // 判定——旧逻辑 measured>baseline 才计 delta，吞吐退化静默放行）。
        if metric.baseline_value > 0 {
            let higher_is_better = metric
                .direction
                .map(|d| d == MetricDirection::HigherIsBetter)
                .unwrap_or_else(|| default_higher_is_better(&metric.metric_id));
            if regressed(
                metric.measured as f64,
                metric.baseline_value as f64,
                higher_is_better,
                metric.regression_tolerance_pct as f64,
            ) {
                let delta_pct = if higher_is_better {
                    (metric.baseline_value.saturating_sub(metric.measured) * 100)
                        / metric.baseline_value
                } else {
                    (metric.measured.saturating_sub(metric.baseline_value) * 100)
                        / metric.baseline_value
                };
                report.violations.push(format!(
                    "regression:{}:{}%>{}%（{}）",
                    metric.metric_id,
                    delta_pct,
                    metric.regression_tolerance_pct,
                    if higher_is_better {
                        "吞吐退化"
                    } else {
                        "延迟恶化"
                    }
                ));
                regressed_count += 1;
            }
        }
        // 42 WP-42.12：可选区间 SLO（range，上下双界）——任一界存在即校验。
        match (metric.slo_lower_bound, metric.slo_upper_bound) {
            (Some(lo), Some(hi)) => {
                if metric.measured < lo || metric.measured > hi {
                    report.violations.push(format!(
                        "range-violation:{}:measured={} out-of-bounds=[{},{}]",
                        metric.metric_id, metric.measured, lo, hi
                    ));
                    regressed_count += 1;
                }
            }
            (Some(lo), None) => {
                if metric.measured < lo {
                    report.violations.push(format!(
                        "range-violation:{}:measured={} < lower={}",
                        metric.metric_id, metric.measured, lo
                    ));
                    regressed_count += 1;
                }
            }
            (None, Some(hi)) => {
                if metric.measured > hi {
                    report.violations.push(format!(
                        "range-violation:{}:measured={} > upper={}",
                        metric.metric_id, metric.measured, hi
                    ));
                    regressed_count += 1;
                }
            }
            (None, None) => {}
        }
    }

    report
        .summary
        .insert("metric_count", results.metrics.len().to_string());
    report
        .summary
        .insert("slo_violations", slo_violations.to_string());
    report
        .summary
        .insert("regressions", regressed_count.to_string());
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
    // v0.242.0（B6）：显式降级模式 `qa proof --degrade`——本机无
    // Isabelle 环境时生成降级声明证据（TCB 形式化进行中，非
    // admitted-proof）；默认仍 fail-closed（存在 sorry 即 FAIL）。
    let degrade = args.iter().any(|a| a == "--degrade");
    let formal_dir = match args.first().map(String::as_str) {
        None | Some("--degrade") => root.join("formal/Isabelle"),
        Some(p) => root.join(p),
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

    // v0.242.0（B6）：机器可重算证据——理论文件 sha256 清单。
    // 重算脚本：`cargo xtask qa proof` 重跑本门禁，产出相同哈希即
    // 证明证据未被篡改（QA-017~022 自证 JSON 门禁升级）。
    let mut manifest = String::new();
    if let Ok(entries) = fs::read_dir(&formal_dir) {
        let mut files: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "thy"))
            .map(|e| e.path())
            .collect();
        files.sort();
        for f in files {
            if let Ok(content) = fs::read(&f) {
                let digest = sha256_bytes(&content);
                let name = f
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                manifest.push_str(&format!("{name}:{digest}\n"));
            }
        }
    }
    report.summary.insert("theory_sha256_manifest", manifest);
    report
        .summary
        .insert("recompute_command", "cargo xtask qa proof".to_string());

    if report.violations.is_empty() && thy_count > 0 {
        report.pass();
    }

    // 降级声明（B6 → 34 GV-06）：生成证据 JSON 并标注降级状态，结果记
    // DEGRADED（**非 PASS**——旧实现 `report.pass()` 把 sorry 证明强改
    // PASS，是证据伪造开关；finalize 对非 PASS 返回 Err，qa 汇总按
    // 非 PASS 计，go-no-go 如实降级）。
    if degrade {
        report.summary.insert("degraded", String::from("true"));
        report.summary.insert(
            "degradation_note",
            String::from("TCB formalisation in progress; not an admitted-proof gate (no Isabelle environment). Re-run with real Isabelle for full proof scan."),
        );
        // 34 GV-06（复验）：无条件置 DEGRADED——degrade 模式下门禁按定义未
        // 完整运行（无 Isabelle 环境），干净扫描亦不得输出 PASS（旧实现仅
        // 在非 PASS 时改标，sorry-free 树在 --degrade 下仍谎报 PASS）。
        report.result = "DEGRADED";
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

    /// v0.254.0（D1b）：soak-run 产出 QA-018 证据 + boot-times.csv
    /// （用 sleep 模拟启动耗时，验证参数校验与产物结构）。
    #[test]
    fn t5_soak_run_produces_evidence() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let out = root.join("soak-campaign.json");
        // 33 GV-05：--image 强制（artifact_sha256 实算需要产物）；用
        // 一个真实小文件模拟镜像产物。
        let image = root.join("fake-image.raw");
        fs::write(&image, b"fake qemu image payload 1234567890").expect("write image");
        // cycles=1、max-fail=1：本机无 QEMU 时命令可能 spawn 失败——
        // 但证据文件仍应产出（restarts 记录 spawn-error）。
        let r = command_soak_run(
            root,
            &[
                "--cycles".into(),
                "1".into(),
                "--max-fail".into(),
                "5".into(),
                "--image".into(),
                image.to_string_lossy().into(),
                "--min-duration-days".into(),
                "0".into(),
                "--out".into(),
                out.to_string_lossy().into(),
            ],
        );
        // 无论 QEMU 是否可用，campaign JSON 必须产出。
        assert!(out.exists(), "soak-campaign.json must be written");
        let parsed: SoakCampaign = read_json(&out).expect("campaign parses");
        assert_eq!(parsed.schema, "eneros.soak-campaign.v1");
        assert_eq!(parsed.metric_samples, 1);
        // 33 GV-05：artifact_sha256 实算非空且等于镜像 sha256sum。
        let want = hex_encode(&Sha256::digest(b"fake qemu image payload 1234567890"));
        assert_eq!(parsed.artifact_sha256, want, "artifact_sha256 必须实算");
        assert_eq!(
            parsed.min_duration_days, 0,
            "--min-duration-days=0 显式传入"
        );
        // boot-times.csv 必须产出（14 号文 D1 启动时间分布证据）。
        let csv = out.with_file_name("boot-times.csv");
        assert!(csv.exists(), "boot-times.csv must be written");
        // 命令本身：QEMU 缺失时 spawn 失败应报错（fail-closed），
        // 而非静默通过。
        if r.is_ok() {
            // 环境有 QEMU 且启动成功——合法。
        } else {
            // 无 QEMU 环境：错误信息应包含原因（spawn/expect）。
            let msg = format!("{:?}", r);
            assert!(
                msg.contains("spawn") || msg.contains("qemu") || msg.contains("failed"),
                "failure reason must be explicit: {msg}"
            );
        }
    }

    /// 33 GV-05：soak-run 缺 --image → 显式报错（不再带病产出空哈希）。
    #[test]
    fn t5b_soak_run_requires_image() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let r = command_soak_run(
            root,
            &[
                "--cycles".into(),
                "1".into(),
                "--max-fail".into(),
                "5".into(),
            ],
        );
        let msg = format!("{:?}", r);
        assert!(r.is_err(), "缺 --image 必须报错");
        assert!(msg.contains("--image"), "错误信息须指出 --image: {msg}");
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

    /// 33 GV-05：artifact_sha256 空串（旧 soak-run 硬编码产物）→ 门禁必拒。
    #[test]
    fn t3c_soak_missing_artifact_sha_rejected() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let campaign = serde_json::json!({
            "schema": "eneros.soak-campaign.v1",
            "campaign_id": "soak-empty-hash",
            "artifact_sha256": "",
            "duration_days": 30,
            "min_duration_days": 3,
            "memory_leak_bytes_per_day": 0,
            "memory_leak_threshold": 0,
            "data_corruption": false,
            "restarts": [],
            "metric_samples": 100,
        });
        let p = root.join("soak.json");
        write_json(&p, &campaign);
        let r = command_soak(root, &[p.to_string_lossy().into()]);
        assert!(r.is_err(), "空 artifact_sha256 必须拒绝");
        // 门禁错误为通用串（"QA-018 gate did not pass"），违规明细在
        // QaReport violations 内；此处以拒绝本身为判据。
    }

    /// v0.253.0（D1）：72h（3 天）浸泡证据可通过门禁（14 号文 D1 标准）。
    #[test]
    fn t3b_soak_72h_pass() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let campaign = serde_json::json!({
            "schema": "eneros.soak-campaign.v1",
            "campaign_id": "soak-72h",
            "artifact_sha256": "abc",
            "duration_days": 3,
            "min_duration_days": 3,
            "memory_leak_bytes_per_day": 0,
            "memory_leak_threshold": 0,
            "data_corruption": false,
            "restarts": [],
            "metric_samples": 100,
        });
        let p = root.join("soak.json");
        write_json(&p, &campaign);
        assert!(command_soak(root, &[p.to_string_lossy().into()]).is_ok());
    }

    /// v0.253.0（D1）：72h 浸泡不足（2 天）必须拒绝。
    #[test]
    fn t4b_soak_72h_insufficient_rejected() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let campaign = serde_json::json!({
            "schema": "eneros.soak-campaign.v1",
            "campaign_id": "soak-48h",
            "artifact_sha256": "abc",
            "duration_days": 2,
            "min_duration_days": 3,
            "memory_leak_bytes_per_day": 0,
            "memory_leak_threshold": 0,
            "data_corruption": false,
            "restarts": [],
            "metric_samples": 50,
        });
        let p = root.join("soak.json");
        write_json(&p, &campaign);
        assert!(command_soak(root, &[p.to_string_lossy().into()]).is_err());
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
            "measured_at": "2026-08-17T00:00:00Z",
            "run_id": "perf-run-test1",
            "baseline_measured_at": "",
            "candidate_sha256": "def",
            "metrics": [
                {"metric_id": "boot-time", "slo_threshold": 5000, "measured": 4000, "regression_tolerance_pct": 5, "baseline_value": 3900, "critical_path": true, "unit": "ms"},
                {"metric_id": "ipc-latency", "slo_threshold": 100, "measured": 80, "regression_tolerance_pct": 10, "baseline_value": 79, "critical_path": true, "unit": "us"},
            ],
        });
        let p = root.join("perf.json");
        write_json(&p, &results);
        // WP-26.1：签名缺失 → FAIL（fail-closed；证伪旧行为——旧代码
        // 无签名文件时直接评估 SLO/回归，且 CI 先 rm -f *.sig 绕过）。
        let err = command_perf_gate(root, &[p.to_string_lossy().into()]);
        assert!(err.is_err(), "签名缺失必须 FAIL（fail-closed）");
    }

    /// WP-26.1：签名闭环——perf-keygen → perf-baseline --sign-key →
    /// perf-gate（同一公钥验证）通过（证伪"签名链被绕过"的旧行为）。
    #[test]
    fn t5b_perf_gate_signature_chain() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let results = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "measured_at": "2026-08-17T00:00:00Z",
            "run_id": "perf-run-test2",
            "baseline_measured_at": "",
            "candidate_sha256": "def",
            "metrics": [
                {"metric_id": "boot-time", "slo_threshold": 5000, "measured": 4000, "regression_tolerance_pct": 5, "baseline_value": 3900, "critical_path": true, "unit": "ms"},
            ],
        });
        // 29 GV-03：统一走 perf_gate_signed（静态 Mutex 串行化环境变量临界区，
        // 与 t6b/t6c/t6d 并行无竞争）。签名存在且有效必须通过门禁。
        let r = perf_gate_signed(root, &results);
        assert!(r.is_ok(), "签名存在且有效必须通过门禁: {r:?}");
    }

    #[test]
    fn t6_perf_gate_fail_slo() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let results = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "measured_at": "2026-08-17T00:00:00Z",
            "run_id": "perf-run-test3",
            "baseline_measured_at": "",
            "candidate_sha256": "def",
            "metrics": [
                {"metric_id": "boot-time", "slo_threshold": 5000, "measured": 6000, "regression_tolerance_pct": 5, "baseline_value": 4000, "critical_path": true, "unit": "ms"},
            ],
        });
        let p = root.join("perf.json");
        write_json(&p, &results);
        assert!(command_perf_gate(root, &[p.to_string_lossy().into()]).is_err());
    }

    /// 签名 + 门禁闭环 helper（29 GV-03 新增用例共用）。
    /// 环境变量（ENEROS_PERF_VERIFY_PUBKEY）在并行测试间竞争——用静态
    /// Mutex 串行化 set_var→gate→remove_var 临界区。
    fn perf_gate_signed(root: &Path, results: &serde_json::Value) -> Result<(), String> {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let key = root.join("perf-key.bin");
        let pk = root.join("perf-pk.hex");
        command_perf_keygen(
            root,
            &[
                "--out".into(),
                key.to_string_lossy().into(),
                "--pubkey-out".into(),
                pk.to_string_lossy().into(),
            ],
        )
        .map_err(|e| e.to_string())?;
        let p = root.join("perf.json");
        write_json(&p, results);
        let json_signed = serde_json::to_vec_pretty(results).unwrap();
        let key_bytes = std::fs::read(&key).unwrap();
        let keypair = ed25519_dalek::SigningKey::from_bytes(&key_bytes[..32].try_into().unwrap());
        use ed25519_dalek::Signer;
        let sig = hex_encode(&keypair.sign(&json_signed).to_bytes());
        std::fs::write(p.with_extension("json.sig"), sig).unwrap();
        unsafe {
            std::env::set_var(
                "ENEROS_PERF_VERIFY_PUBKEY",
                std::fs::read_to_string(&pk).unwrap().trim(),
            );
        }
        let r = command_perf_gate(root, &[p.to_string_lossy().into()]);
        unsafe {
            std::env::remove_var("ENEROS_PERF_VERIFY_PUBKEY");
        }
        r.map_err(|e| e.to_string())
    }

    /// 29 GV-03 判据负例：吞吐类（higher_is_better）实测 = 基线 × 0.5
    /// → 必拦（旧逻辑方向反，此处真实退化曾被静默放行）。
    #[test]
    fn t6b_perf_gate_throughput_regression_caught() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let results = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "measured_at": "2026-08-17T00:00:00Z",
            "run_id": "perf-run-test4",
            "baseline_measured_at": "",
            "candidate_sha256": "def",
            "metrics": [
                {"metric_id": "iec104_encode", "slo_threshold": 100000, "measured": 50000, "regression_tolerance_pct": 10, "baseline_value": 100000, "critical_path": false, "unit": "frames/s", "direction": "higher_is_better"},
            ],
        });
        let r = perf_gate_signed(root, &results);
        assert!(r.is_err(), "吞吐退化 50% 必须被拦: {r:?}");
    }

    /// 29 GV-03 正例：吞吐类实测 = 基线 × 2（性能提高）→ 不误伤。
    /// 蓝图 36 WP-36.10 #8：SLO 必须冻结（>0）且 baseline != candidate。
    #[test]
    fn t6c_perf_gate_throughput_improvement_passes() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let results = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "measured_at": "2026-08-17T00:00:00Z",
            "run_id": "perf-run-test5",
            "baseline_measured_at": "",
            "candidate_sha256": "def",
            "metrics": [
                {"metric_id": "iec104_encode", "slo_threshold": 100000, "measured": 200000, "regression_tolerance_pct": 10, "baseline_value": 100000, "critical_path": false, "unit": "frames/s", "direction": "higher_is_better"},
            ],
        });
        let r = perf_gate_signed(root, &results);
        assert!(r.is_ok(), "吞吐提高不得误拦: {r:?}");
    }

    /// 蓝图 36 WP-36.10 #8：SLO=0（未冻结）→ 必须失败；自基线 → 必须失败。
    #[test]
    fn t36_perf_gate_rejects_unfrozen_slo_and_self_baseline() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let results = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "measured_at": "2026-08-17T00:00:00Z",
            "run_id": "perf-run-test6",
            "baseline_measured_at": "",
            "candidate_sha256": "def",
            "metrics": [
                {"metric_id": "iec104_encode", "slo_threshold": 0, "measured": 200000, "regression_tolerance_pct": 10, "baseline_value": 100000, "critical_path": false, "unit": "frames/s", "direction": "higher_is_better"},
            ],
        });
        let r = perf_gate_signed(root, &results);
        assert!(
            r.is_err(),
            "SLO=0（未冻结）必须失败（36 WP-36.10 #8）: {r:?}"
        );

        let results2 = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "same",
            "candidate_sha256": "same",
            "metrics": [
                {"metric_id": "iec104_encode", "slo_threshold": 100000, "measured": 200000, "regression_tolerance_pct": 10, "baseline_value": 100000, "critical_path": false, "unit": "frames/s", "direction": "higher_is_better"},
            ],
        });
        let r2 = perf_gate_signed(root, &results2);
        assert!(
            r2.is_err(),
            "candidate 自建 baseline 必须失败（36 WP-36.10 #8）: {r2:?}"
        );
    }

    /// 29 GV-03：延迟类（lower_is_better）缺省白名单外推断——实测超基线
    /// 15%（tolerance 5%）→ 必拦。
    #[test]
    fn t6d_perf_gate_latency_default_direction_caught() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let results = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "measured_at": "2026-08-17T00:00:00Z",
            "run_id": "perf-run-test7",
            "baseline_measured_at": "",
            "candidate_sha256": "def",
            "metrics": [
                {"metric_id": "boot-time", "slo_threshold": 0, "measured": 4600, "regression_tolerance_pct": 5, "baseline_value": 4000, "critical_path": true, "unit": "ms"},
            ],
        });
        let r = perf_gate_signed(root, &results);
        assert!(
            r.is_err(),
            "延迟恶化 15% 必须被拦（缺省 direction 推断）: {r:?}"
        );
    }

    #[test]
    fn t37_perf_measure_rejects_unfrozen_baseline() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let baseline = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "measured_at": "2026-08-17T00:00:00Z",
            "run_id": "perf-run-test8",
            "baseline_measured_at": "",
            "candidate_sha256": "",
            "metrics": [
                {"metric_id": "iec104_encode", "slo_threshold": 0, "measured": 100, "regression_tolerance_pct": 10, "baseline_value": 100, "critical_path": false, "unit": "frames/s", "direction": "higher_is_better"},
            ],
        });
        let p = root.join("baseline.json");
        write_json(&p, &baseline);
        let r = command_perf_measure(root, &["--baseline".into(), p.to_string_lossy().into()]);
        assert!(r.is_err(), "unfrozen baseline must be rejected: {r:?}");
    }

    #[test]
    fn t38_perf_measure_binds_to_frozen_baseline() {
        let dir = tempdir();
        let root = Path::new(&dir);
        let baseline = serde_json::json!({
            "schema": "eneros.perf-regression.v1",
            "baseline_sha256": "abc",
            "measured_at": "2026-08-17T00:00:00Z",
            "run_id": "perf-run-test9",
            "baseline_measured_at": "",
            "candidate_sha256": "",
            "metrics": [
                {"metric_id": "iec104_encode", "slo_threshold": 100000, "measured": 100, "regression_tolerance_pct": 10, "baseline_value": 100000, "critical_path": false, "unit": "frames/s", "direction": "higher_is_better"},
                {"metric_id": "ipc_wire_roundtrip", "slo_threshold": 100000, "measured": 100, "regression_tolerance_pct": 10, "baseline_value": 100000, "critical_path": true, "unit": "frames/s", "direction": "higher_is_better"},
                {"metric_id": "modbus_rtu_frame", "slo_threshold": 100000, "measured": 100, "regression_tolerance_pct": 10, "baseline_value": 100000, "critical_path": false, "unit": "frames/s", "direction": "higher_is_better"},
                {"metric_id": "elf_parse", "slo_threshold": 100000, "measured": 100, "regression_tolerance_pct": 10, "baseline_value": 100000, "critical_path": true, "unit": "parses/s", "direction": "higher_is_better"},
                {"metric_id": "gguf_parse", "slo_threshold": 100000, "measured": 100, "regression_tolerance_pct": 10, "baseline_value": 100000, "critical_path": false, "unit": "parses/s", "direction": "higher_is_better"},
            ],
        });
        let p = root.join("baseline.json");
        write_json(&p, &baseline);
        let r = command_perf_measure(root, &["--baseline".into(), p.to_string_lossy().into()]);
        assert!(
            r.is_ok(),
            "frozen baseline must allow candidate measurement: {r:?}"
        );
        let out = root.join("target/perf-candidate.json");
        let candidate: PerfResults = read_json(&out).expect("candidate parses");
        assert_eq!(candidate.baseline_sha256, "abc");
        assert!(!candidate.candidate_sha256.is_empty());
        assert!(candidate.metrics.iter().all(|m| m.slo_threshold > 0));
        assert!(candidate.metrics.iter().all(|m| m.baseline_value > 0));
        assert_ne!(candidate.baseline_sha256, candidate.candidate_sha256);
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

    /// 34 GV-06（复验回归）：--degrade 必须无条件输出 DEGRADED（非 PASS）——
    /// 旧实现仅非 PASS 时改标，sorry-free 树在 --degrade 下谎报 PASS。
    #[test]
    fn t9b_degrade_emits_degraded() {
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
        // --degrade → DEGRADED 非 PASS → finalize 报错（exit != 0）。
        assert!(command_proof(root, &["--degrade".to_string()]).is_err());
        // 落盘报告 result 必须为 DEGRADED。
        let path = root.join("target/eneros/evidence/g9/qa_021.json");
        let rep: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            rep["result"].as_str(),
            Some("DEGRADED"),
            "--degrade 必须产出 DEGRADED"
        );
        // 干净树 + --degrade 也必须 DEGRADED（不得谎报 PASS）。
        fs::write(
            formal.join("Test.thy"),
            "theory Test imports Main begin\nend",
        )
        .unwrap();
        assert!(command_proof(root, &["--degrade".to_string()]).is_err());
        let rep2: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            rep2["result"].as_str(),
            Some("DEGRADED"),
            "干净树 + --degrade 仍须 DEGRADED"
        );
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
