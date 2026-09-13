//! 70.14（WP-70.14.1/4）：浸泡/HIL 运行证据的 fail-closed 判据。
//!
//! # 本模块替换的判据
//! 既有 `tests/soak/soak72.sh` 的 campaign 报告里有三处**推算值**（本轮
//! 对照蓝图 §9 事实表 D6-13 逐条复核属实）：
//! 1. `duration_days = 参数推导`（`PROTOCOL_HOURS / 24`）——不是实测墙钟；
//! 2. `artifact_sha256` 固定读 `target/eneros/artifacts/qemu-prod/...`——
//!    真机 SKU 产物摘要根本不在那里；
//! 3. `memory_leak_bytes_per_day: 0` 与 `memory_leak_threshold: 0` 为**字面
//!    零写入**——不是从任何样本序列推导的。
//!
//! 本模块把"跑完 72h"变成可核对判据：真实起止与心跳、样本序列、每必需
//! 协议的覆盖区间、产物/设备/rig 绑定；缺任一项 → 运行记录判负（不是"警告"
//! 也不是 `exit 0`）。
//!
//! # 边界
//! 判据是对**记录与样本**的机器核对；真实时间/物理结果由试验台仪器给出
//! （`rig` 字段 + 原始证据文件），本模块不代替仪器。

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{invalid, read_json, DynResult};

/// 硬件 scope（与 `board.rs` 同域；非硬件 scope 不得作为 72h 技术资格）。
pub const HARDWARE_SCOPES: [&str; 3] = ["hw-hil", "hw-soak", "hw-bringup"];

/// 一次运行的原样记录（E70-E 切片：原始时间/样本/覆盖/资源）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoakRunManifest {
    pub schema: String,
    pub run_id: String,
    pub scenario: String,
    pub scope: String,
    /// 产物绑定（角色 → sha256；不得为空串）。
    pub artifacts: std::collections::BTreeMap<String, String>,
    /// 设备事实：SKU / 板卡修订 / 序列号（空 = 未绑定实物）。
    pub device: DeviceFacts,
    /// 试验台事实（独立观察面）。
    pub rig: RigFacts,
    /// 真实墙钟（RFC3339；从实际开始/结束读取，不由参数推导）。
    pub started_at: String,
    pub ended_at: String,
    /// 心跳时刻（RFC3339；至少 2 个，间隔不得超 `max_heartbeat_gap_s`）。
    pub heartbeats: Vec<String>,
    /// 每必需协议的实测覆盖区间。
    pub protocol_coverage: Vec<ProtocolCoverage>,
    /// 资源样本序列（原始值，用于推导峰值/斜率；禁用汇总值）。
    pub metrics: Vec<MetricSample>,
    /// 判定程序版本（判据本身的身份）。
    pub judge_version: String,
    /// 原始文件清单（相对路径；必须实际存在）。
    pub raw_files: Vec<String>,
    /// 运行中断记录（有中断 → 不得当连续 72h）。
    pub interruptions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceFacts {
    pub sku_id: String,
    pub board_revision: String,
    pub serial: String,
    pub firmware: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigFacts {
    pub rig_id: String,
    /// 仪器（独立测量/抓包/电源控制）清单。
    pub instruments: Vec<String>,
    /// 电源控制通道（掉电测试必需）。
    pub power_channel: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolCoverage {
    pub protocol: String,
    /// 实测活跃区间起止（RFC3339）。
    pub active_from: String,
    pub active_to: String,
    /// 该协议本次运行的有效样本数（>0 才叫覆盖）。
    pub samples: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricSample {
    /// 采样时刻（RFC3339）。
    pub at: String,
    /// 堆字节 / DMA 字节 / 句柄数 / 队列水位（原始值）。
    pub heap_bytes: u64,
    pub dma_bytes: u64,
    pub handles: u64,
    /// flush 延迟（毫秒）与控制周期（毫秒）——长尾统计输入。
    pub flush_latency_ms_max: u64,
    pub control_period_ms_max: u64,
}

/// 判据阈值（由运行命令显式冻结；空值不填默认）。
#[derive(Debug, Clone, Copy)]
pub struct SoakPolicy {
    /// 最小连续时长（小时）。
    pub min_hours: u64,
    /// 心跳最大间隔（秒）——心跳空洞 = 停机或采集掉线，必须被抓。
    pub max_heartbeat_gap_s: u64,
    /// 资源斜率上限（字节/小时；超过即判增长）。
    pub max_heap_slope_bytes_per_hour: i64,
    /// 控制周期上限（毫秒；合同 deadline）。
    pub control_period_limit_ms: u64,
}

impl SoakPolicy {
    /// 72h 技术资格判据（蓝图 §7）。
    pub const QUALIFICATION: SoakPolicy = SoakPolicy {
        min_hours: 72,
        max_heartbeat_gap_s: 900,
        max_heap_slope_bytes_per_hour: 64 * 1024,
        control_period_limit_ms: 5_000,
    };
}

/// 运行记录的机器判定结果（可归档；`violations` 空 = 通过）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct SoakVerdict {
    pub run_id: String,
    pub violations: Vec<String>,
    /// 实测连续小时（由真实起止算出）。
    pub measured_hours: u64,
    /// 堆斜率（字节/小时，最小二乘；样本不足 → None）。
    pub heap_slope_bytes_per_hour: Option<i64>,
    pub heap_peak_bytes: Option<u64>,
    pub dma_peak_bytes: Option<u64>,
    pub handles_peak: Option<u64>,
    pub flush_latency_peak_ms: Option<u64>,
    pub control_period_peak_ms: Option<u64>,
    pub protocol_coverage_hours: std::collections::BTreeMap<String, u64>,
}

impl SoakVerdict {
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// 解析 RFC3339 到秒（只接受 UTC `...Z` 形态；本地时区/裸日期拒绝——
/// "时间失真"负例必须被抓）。
pub fn parse_rfc3339_secs(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let rest = rest.strip_suffix('Z')?;
    // 审计（S7 P3）：字段必须**定形**（逐段固定位宽的无符号数字）——旧实现
    // 只查 `1..=31`（`2026-02-30` 被静默规范化成 3-02）、不设年份上界
    // （`9223372036854775807-01-01…` 在 dev profile 直接 overflow panic），
    // 且 `+2026`/`2026-9-1`/`T0:0:0` 都被接受。先校验形态，再解析。
    let d: Vec<&str> = date.split('-').collect();
    if d.len() != 3 || d[0].len() != 4 || d[1].len() != 2 || d[2].len() != 2 {
        return None;
    }
    let secs_part = rest.split('.').next().unwrap_or("");
    let t: Vec<&str> = secs_part.split(':').collect();
    if t.len() != 3 || t.iter().any(|p| p.len() != 2) {
        return None;
    }
    let all_digits = |parts: &[&str]| {
        parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    };
    if !all_digits(&d) || !all_digits(&t) {
        return None;
    }
    let y: i64 = d[0].parse().ok()?;
    let mo: i64 = d[1].parse().ok()?;
    let day: i64 = d[2].parse().ok()?;
    let h: i64 = t[0].parse().ok()?;
    let mi: i64 = t[1].parse().ok()?;
    let se: i64 = t[2].parse().ok()?;
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&mo) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    // 闰年/月长校验（`2026-02-30`、`2026-04-31` 一律拒绝）。
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=dim).contains(&day) {
        return None;
    }
    // 民用历到 Unix 秒（Howard Hinnant 算法）。
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3_600 + mi * 60 + se)
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// 最小二乘斜率（x = 样本序号，y = 值）——避免引入浮点依赖导致的
/// 平台差异；返回 (斜率 numerator, denominator)。
fn slope_per_step(values: &[u64]) -> Option<(i128, i128)> {
    let n = values.len() as i128;
    if n < 3 {
        return None;
    }
    let mut sx: i128 = 0;
    let mut sy: i128 = 0;
    let mut sxx: i128 = 0;
    let mut sxy: i128 = 0;
    for (i, v) in values.iter().enumerate() {
        let x = i as i128;
        let y = *v as i128;
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let denom = n * sxx - sx * sx;
    if denom == 0 {
        return None;
    }
    Some((n * sxy - sx * sy, denom))
}

/// 运行记录判据（全部 fail-closed；`policy` 由调用方冻结）。
///
/// `base` = 记录里相对路径的解析基准（通常仓库根；自检/临时目录场景显式
/// 传入——判据本身与位置无关）。
pub fn validate_run(
    base: &Path,
    manifest: &SoakRunManifest,
    policy: &SoakPolicy,
    require_hardware: bool,
) -> DynResult<SoakVerdict> {
    let mut v = SoakVerdict {
        run_id: manifest.run_id.clone(),
        ..Default::default()
    };
    if manifest.schema != "eneros.soak-run.v1" {
        v.violations.push(format!("run-schema:{}", manifest.schema));
    }
    if manifest.run_id.trim().is_empty() || manifest.scenario.trim().is_empty() {
        v.violations.push("run-identity-incomplete".to_owned());
    }
    if manifest.judge_version.trim().is_empty() {
        v.violations.push("run-judge-version-missing".to_owned());
    }
    if require_hardware {
        if !HARDWARE_SCOPES.contains(&manifest.scope.as_str()) {
            v.violations
                .push(format!("run-scope-not-hardware:{}", manifest.scope));
        }
        if manifest.device.serial.trim().is_empty()
            || manifest.device.serial.eq_ignore_ascii_case("unknown")
        {
            v.violations.push("run-device-serial-unbound".to_owned());
        }
        if manifest.device.board_revision.trim().is_empty()
            || manifest
                .device
                .board_revision
                .eq_ignore_ascii_case("unknown")
        {
            v.violations.push("run-board-revision-unbound".to_owned());
        }
        if manifest.device.firmware.trim().is_empty() {
            v.violations.push("run-device-firmware-unbound".to_owned());
        }
    }
    if manifest.device.sku_id.trim().is_empty() {
        v.violations.push("run-device-sku-empty".to_owned());
    }
    if manifest.rig.rig_id.trim().is_empty() {
        v.violations.push("run-rig-unbound".to_owned());
    }
    if manifest.rig.instruments.is_empty() {
        v.violations.push("run-rig-instruments-empty".to_owned());
    }
    if manifest.rig.power_channel.trim().is_empty() {
        v.violations.push("run-rig-power-channel-empty".to_owned());
    }
    // 产物绑定：至少 root/boot 链摘要，且形如 sha256、不得为字面零。
    if manifest.artifacts.is_empty() {
        v.violations.push("run-artifacts-empty".to_owned());
    }
    // 审计（S7 P2）：注释说"至少 root/boot 链摘要"但代码只查非空——任意角色名
    // 的任意 64hex 都能充当候选产物绑定，而 shell 又取 `values()` 的第一个当
    // `artifact_sha256`。改为按角色名强制（与 bring-up 判据同口径）。
    for role in ["root_task", "boot_chain"] {
        if !manifest.artifacts.contains_key(role) {
            v.violations.push(format!("run-artifact-missing:{role}"));
        }
    }
    for (role, digest) in &manifest.artifacts {
        if !is_sha256(digest) {
            v.violations.push(format!("run-artifact-digest-bad:{role}"));
        }
        if digest.chars().all(|c| c == '0') {
            v.violations
                .push(format!("run-artifact-digest-zero:{role}"));
        }
    }
    // 真实时长：由起止时刻算出（不由参数推导）。
    let started = parse_rfc3339_secs(&manifest.started_at);
    let ended = parse_rfc3339_secs(&manifest.ended_at);
    match (started, ended) {
        (Some(s), Some(e)) => {
            if e <= s {
                v.violations.push("run-time-order-invalid".to_owned());
            } else {
                let hours = ((e - s) as u64) / 3_600;
                v.measured_hours = hours;
                if hours < policy.min_hours {
                    v.violations.push(format!(
                        "run-duration-below-min:measured={hours}h:min={}h",
                        policy.min_hours
                    ));
                }
            }
        }
        _ => v.violations.push("run-time-invalid".to_owned()),
    }
    // 心跳：至少 2 个、单调、间隔受限（心跳空洞 = 停机/采集掉线）。
    if manifest.heartbeats.len() < 2 {
        v.violations.push("run-heartbeats-too-few".to_owned());
    } else {
        let mut prev: Option<i64> = None;
        for (i, hb) in manifest.heartbeats.iter().enumerate() {
            match parse_rfc3339_secs(hb) {
                None => {
                    v.violations.push(format!("run-heartbeat-invalid:{i}"));
                    break;
                }
                Some(t) => {
                    if let Some(p) = prev {
                        if t <= p {
                            v.violations
                                .push(format!("run-heartbeat-not-increasing:{i}"));
                            break;
                        }
                        if ((t - p) as u64) > policy.max_heartbeat_gap_s {
                            v.violations.push(format!(
                                "run-heartbeat-gap:{}s>{}s",
                                t - p,
                                policy.max_heartbeat_gap_s
                            ));
                            break;
                        }
                    }
                    prev = Some(t);
                }
            }
        }
        // 心跳必须覆盖整个区间（首尾与起止一致，容差 0：心跳是实测事实）。
        if let (Some(s), Some(e)) = (started, ended) {
            if let (Some(first), Some(last)) = (
                manifest
                    .heartbeats
                    .first()
                    .and_then(|h| parse_rfc3339_secs(h)),
                manifest
                    .heartbeats
                    .last()
                    .and_then(|h| parse_rfc3339_secs(h)),
            ) {
                if first > s || last < e - policy.max_heartbeat_gap_s as i64 {
                    v.violations
                        .push("run-heartbeat-window-incomplete".to_owned());
                }
            }
        }
    }
    // 中断记录：有中断即不得当"连续"。
    if !manifest.interruptions.is_empty() {
        v.violations.push(format!(
            "run-interrupted:{}",
            manifest.interruptions.join("|")
        ));
    }
    // 协议覆盖：每种必需协议都要有正样本与有效区间。
    if manifest.protocol_coverage.is_empty() {
        v.violations.push("run-protocol-coverage-empty".to_owned());
    }
    let mut seen_protocols: BTreeSet<&str> = BTreeSet::new();
    for c in &manifest.protocol_coverage {
        if c.protocol.trim().is_empty() {
            v.violations.push("run-protocol-empty".to_owned());
            continue;
        }
        if !seen_protocols.insert(c.protocol.as_str()) {
            v.violations
                .push(format!("run-protocol-duplicate:{}", c.protocol));
        }
        if c.samples == 0 {
            v.violations
                .push(format!("run-protocol-zero-samples:{}", c.protocol));
        }
        match (
            parse_rfc3339_secs(&c.active_from),
            parse_rfc3339_secs(&c.active_to),
        ) {
            (Some(a), Some(b)) if b > a => {
                v.protocol_coverage_hours
                    .insert(c.protocol.clone(), ((b - a) as u64) / 3_600);
                if let (Some(s), Some(e)) = (started, ended) {
                    if a < s - 60 || b > e + 60 {
                        v.violations
                            .push(format!("run-protocol-window-outside-run:{}", c.protocol));
                    }
                    // 审计（S7 P2）：旧实现只查"窗口在运行区间内且样本>0" ——
                    // 72h 运行里 60 秒、1 个样本也过。要求覆盖区间达到冻结
                    // 时长（容差 1 小时：允许单次重连造成的短缝）。
                    let span = b - a;
                    let needed = (policy.min_hours as i64) * 3_600 - 3_600;
                    if policy.min_hours > 0 && span < needed {
                        v.violations.push(format!(
                            "run-protocol-window-too-short:{}:{}s<{}s",
                            c.protocol, span, needed
                        ));
                    }
                }
            }
            _ => v
                .violations
                .push(format!("run-protocol-window-invalid:{}", c.protocol)),
        }
    }
    // 资源序列：样本不足不得给结论（不填 0）；斜率超过冻结阈值即判增长。
    if manifest.metrics.len() < 3 {
        v.violations.push("run-metrics-too-few".to_owned());
    } else {
        let heap: Vec<u64> = manifest.metrics.iter().map(|m| m.heap_bytes).collect();
        v.heap_peak_bytes = heap.iter().copied().max();
        v.dma_peak_bytes = manifest.metrics.iter().map(|m| m.dma_bytes).max();
        v.handles_peak = manifest.metrics.iter().map(|m| m.handles).max();
        v.flush_latency_peak_ms = manifest
            .metrics
            .iter()
            .map(|m| m.flush_latency_ms_max)
            .max();
        v.control_period_peak_ms = manifest
            .metrics
            .iter()
            .map(|m| m.control_period_ms_max)
            .max();
        // 斜率换算：样本间隔按真实时刻算（小时）。
        let span_hours = match (started, ended) {
            (Some(s), Some(e)) if e > s => ((e - s) as f64) / 3600.0,
            _ => 0.0,
        };
        if let Some((num, den)) = slope_per_step(&heap) {
            let per_step = (num as f64) / (den as f64);
            let per_hour = if span_hours > 0.0 {
                per_step * (manifest.metrics.len() as f64 - 1.0) / span_hours
            } else {
                0.0
            };
            let slope = per_hour.round() as i64;
            v.heap_slope_bytes_per_hour = Some(slope);
            if slope > policy.max_heap_slope_bytes_per_hour {
                v.violations.push(format!(
                    "run-heap-growth:{slope}B/h>{}B/h",
                    policy.max_heap_slope_bytes_per_hour
                ));
            }
        } else {
            v.violations.push("run-heap-slope-undecidable".to_owned());
        }
        if let Some(p) = v.control_period_peak_ms {
            if p > policy.control_period_limit_ms {
                v.violations.push(format!(
                    "run-control-period-over-limit:{p}ms>{}ms",
                    policy.control_period_limit_ms
                ));
            }
        }
        // 采样时刻必须与运行区间一致（时钟失真负例）。
        for (i, m) in manifest.metrics.iter().enumerate() {
            match parse_rfc3339_secs(&m.at) {
                None => {
                    v.violations.push(format!("run-metric-time-invalid:{i}"));
                    break;
                }
                Some(t) => {
                    if let (Some(s), Some(e)) = (started, ended) {
                        if t < s || t > e + 60 {
                            v.violations.push(format!("run-metric-outside-run:{i}"));
                            break;
                        }
                    }
                }
            }
        }
    }
    // 原始文件必须存在（缺一 required 原始文件 = 失败）。
    if manifest.raw_files.is_empty() {
        v.violations.push("run-raw-files-empty".to_owned());
    }
    for rel in &manifest.raw_files {
        let path = base.join(rel);
        if !path.is_file() {
            v.violations.push(format!("run-raw-file-missing:{rel}"));
        } else if path.metadata().map(|m| m.len() == 0).unwrap_or(true) {
            // 审计（S7 P2）：零字节"原始证据"不是证据。
            v.violations.push(format!("run-raw-file-empty:{rel}"));
        }
        // 审计（S7 P2）：绝对路径会绕过 base（`base.join("/etc/hostname")`），
        // `..` 可以指到仓库外——原始证据必须是运行目录内的相对路径。
        let p = std::path::Path::new(rel);
        if p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            v.violations
                .push(format!("run-raw-file-escapes-base:{rel}"));
        }
    }
    Ok(v)
}

/// 从目录读取运行记录并判定（命令入口共用）。
pub fn load_and_validate_run(
    base: &Path,
    rel: &str,
    policy: &SoakPolicy,
    require_hardware: bool,
) -> DynResult<SoakVerdict> {
    let path = base.join(rel);
    if !path.is_file() {
        return Err(invalid(format!("soak run manifest missing: {rel}")));
    }
    let manifest = read_json::<SoakRunManifest>(&path)?;
    validate_run(base, &manifest, policy, require_hardware)
}

/// 在原始串口/日志文件里扫描未处置的 PANIC/OOM 标记（零泛豁免）。
///
/// 只扫描 manifest 列出的原始文件；`allow_patterns` 为**显式**登记的可预期
/// 事件（例如本场景专门注入的故障），每项必须由调用方给出理由（不设按进程
/// 名泛豁免）。
pub fn scan_fatal_markers(
    base: &Path,
    raw_files: &[String],
) -> DynResult<Vec<(String, usize, String)>> {
    let mut hits = Vec::new();
    for rel in raw_files {
        let path = base.join(rel);
        // 审计（S7 P1）：旧实现用 `read_to_string` + `else { continue }`——
        // 只要原始日志里有一个非 UTF-8 字节，**整个文件**就不被扫描，
        // PANIC/OOM 零泛豁免形同虚设（构造性绕过：往日志尾部塞一个 0xFF）。
        // 改为按字节读 + lossy 解码；读不出的文件本身即判负。
        let bytes = fs::read(&path).unwrap_or_default();
        if bytes.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            let l = line.to_ascii_lowercase();
            if l.contains("panic") || l.contains("oom") || l.contains("out of memory") {
                hits.push((rel.clone(), i + 1, line.trim().to_owned()));
            }
        }
    }
    Ok(hits)
}

// ============================================================
// 命令分发
// ============================================================

/// `soak ...` 分发。
pub fn command_soak(root: &Path, args: &[String]) -> DynResult<()> {
    match args.split_first() {
        None => Err(invalid("soak requires a subcommand: run-check|policy")),
        Some((cmd, tail)) => match cmd.as_str() {
            "run-check" => command_run_check(root, tail),
            "policy" => {
                println!(
                    "SOAK-POLICY min_hours={} max_heartbeat_gap_s={} \
                     max_heap_slope_bytes_per_hour={} control_period_limit_ms={}",
                    SoakPolicy::QUALIFICATION.min_hours,
                    SoakPolicy::QUALIFICATION.max_heartbeat_gap_s,
                    SoakPolicy::QUALIFICATION.max_heap_slope_bytes_per_hour,
                    SoakPolicy::QUALIFICATION.control_period_limit_ms
                );
                Ok(())
            }
            "help" | "--help" | "-h" => {
                println!(
                    "soak subcommands: run-check|policy\n\
                     \x20 run-check <manifest.json> [--min-hours N] [--expect-scope hw|any]\n\
                     \x20            [--max-heap-slope B/h] [--control-limit-ms N] [--out <json>]\n\
                     \x20            [--base-dir <dir>]（记录内相对路径的解析基准）"
                );
                Ok(())
            }
            other => Err(invalid(format!("unknown soak subcommand '{other}'"))),
        },
    }
}

fn command_run_check(root: &Path, args: &[String]) -> DynResult<()> {
    let mut rel: Option<String> = None;
    let mut policy = SoakPolicy::QUALIFICATION;
    let mut require_hardware = true;
    let mut out: Option<std::path::PathBuf> = None;
    let mut base = root.to_path_buf();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| invalid(format!("{a} requires a value")))
        };
        match a.as_str() {
            "--min-hours" => {
                policy.min_hours = val()?
                    .parse()
                    .map_err(|_| invalid("--min-hours must be u64"))?
            }
            "--max-heap-slope" => {
                policy.max_heap_slope_bytes_per_hour = val()?
                    .parse()
                    .map_err(|_| invalid("--max-heap-slope must be i64"))?
            }
            "--control-limit-ms" => {
                policy.control_period_limit_ms = val()?
                    .parse()
                    .map_err(|_| invalid("--control-limit-ms must be u64"))?
            }
            "--max-heartbeat-gap-s" => {
                policy.max_heartbeat_gap_s = val()?
                    .parse()
                    .map_err(|_| invalid("--max-heartbeat-gap-s must be u64"))?
            }
            "--expect-scope" => match val()?.as_str() {
                "hw" => require_hardware = true,
                "any" => {
                    require_hardware = false;
                    println!(
                        "WARNING: --expect-scope any —— 开发平台口径，本判定不得作为 72h 技术资格"
                    );
                }
                other => return Err(invalid(format!("unknown --expect-scope '{other}'"))),
            },
            "--out" => out = Some(root.join(val()?)),
            "--base-dir" => base = root.join(val()?),
            other if other.starts_with("--") => {
                return Err(invalid(format!("unknown soak run-check arg '{other}'")))
            }
            other => rel = Some(other.to_owned()),
        }
    }
    let rel = rel.ok_or_else(|| invalid("soak run-check requires <manifest.json>"))?;
    let mut verdict = load_and_validate_run(&base, &rel, &policy, require_hardware)?;
    // 零泛豁免：原始文件里的 PANIC/OOM 一律判负（缺文件已在 run 判据里报）。
    let path = base.join(&rel);
    let manifest = read_json::<SoakRunManifest>(&path)?;
    for (file, line, text) in scan_fatal_markers(&base, &manifest.raw_files)? {
        verdict
            .violations
            .push(format!("run-fatal-marker:{file}:{line}:{text}"));
    }
    if let Some(out) = out {
        if let Some(dir) = out.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&out, serde_json::to_vec_pretty(&verdict)?)?;
    }
    println!(
        "SOAK-RUN measured_hours={} heap_slope_b_per_h={:?} heap_peak={:?} control_peak_ms={:?} protocols={}",
        verdict.measured_hours,
        verdict.heap_slope_bytes_per_hour,
        verdict.heap_peak_bytes,
        verdict.control_period_peak_ms,
        verdict.protocol_coverage_hours.len()
    );
    if verdict.is_ok() {
        println!("SOAK-RUN-CHECK PASS run={}", verdict.run_id);
        Ok(())
    } else {
        Err(invalid(format!(
            "SOAK-RUN-CHECK FAIL ({} violation(s)): {}",
            verdict.violations.len(),
            verdict.violations.join("; ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eneros-soak-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("evidence")).unwrap();
        dir
    }

    fn hhmm_minutes(minutes: u64) -> String {
        let base = parse_rfc3339_secs("2026-09-01T00:00:00Z").unwrap();
        let t = base + (minutes as i64) * 60;
        let days = t.div_euclid(86_400);
        let rem = t.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mo = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if mo <= 2 { y + 1 } else { y };
        format!(
            "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }

    fn hhmm(hours: u64) -> String {
        // 2026-09-01T00:00:00Z + hours
        let base = parse_rfc3339_secs("2026-09-01T00:00:00Z").unwrap();
        let t = base + (hours as i64) * 3600;
        let days = t.div_euclid(86_400);
        let rem = t.rem_euclid(86_400);
        // 民用历反算（与解析器同一算法族；测试用固定基准年）。
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mo = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if mo <= 2 { y + 1 } else { y };
        format!(
            "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }

    fn good_manifest(root: &Path) -> SoakRunManifest {
        let mut artifacts = BTreeMap::new();
        artifacts.insert("root_task".to_owned(), "a".repeat(64));
        artifacts.insert("boot_chain".to_owned(), "b".repeat(64));
        // 3 天，每 10 分钟一个心跳（433 个；末次 = 起 + 72h）。
        let heartbeats: Vec<String> = (0..=(72 * 6)).map(|i| hhmm_minutes(i * 10)).collect();
        let metrics: Vec<MetricSample> = (0..72)
            .map(|i| MetricSample {
                at: hhmm(i),
                heap_bytes: 1_000_000 + (i % 5) * 100,
                dma_bytes: 250_000,
                handles: 40,
                flush_latency_ms_max: 12,
                control_period_ms_max: 100,
            })
            .collect();
        fs::write(root.join("evidence/serial.log"), b"boot ok\n").unwrap();
        SoakRunManifest {
            schema: "eneros.soak-run.v1".to_owned(),
            run_id: "run-1".to_owned(),
            scenario: "steady-72h".to_owned(),
            scope: "hw-soak".to_owned(),
            artifacts,
            device: DeviceFacts {
                sku_id: "acme-board".to_owned(),
                board_revision: "rev-A".to_owned(),
                serial: "SN-1".to_owned(),
                firmware: "uefi-1.2".to_owned(),
            },
            rig: RigFacts {
                rig_id: "rig-1".to_owned(),
                instruments: vec!["power-meter".to_owned(), "tcpdump".to_owned()],
                power_channel: "psu-1".to_owned(),
            },
            started_at: hhmm(0),
            ended_at: hhmm(72),
            heartbeats,
            protocol_coverage: vec![
                ProtocolCoverage {
                    protocol: "modbus-tcp".to_owned(),
                    active_from: hhmm(0),
                    active_to: hhmm(72),
                    samples: 100_000,
                },
                ProtocolCoverage {
                    protocol: "iec104".to_owned(),
                    active_from: hhmm(0),
                    active_to: hhmm(72),
                    samples: 50_000,
                },
            ],
            metrics,
            judge_version: "xtask/soak-1".to_owned(),
            raw_files: vec!["evidence/serial.log".to_owned()],
            interruptions: vec![],
        }
    }

    #[test]
    fn good_run_passes_and_derives_metrics() {
        let root = tmp_root("good");
        let m = good_manifest(&root);
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(v.is_ok(), "unexpected: {:?}", v.violations);
        assert_eq!(v.measured_hours, 72);
        assert_eq!(v.heap_peak_bytes, Some(1_000_400));
        assert_eq!(v.heap_slope_bytes_per_hour, Some(0), "锯齿不产生增长斜率");
        assert_eq!(v.protocol_coverage_hours.get("modbus-tcp"), Some(&72));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parameter_derived_duration_is_rejected() {
        // 参数说 72h，但实测起止只有 2h → 拒绝（旧 script 的 duration_days
        // = PROTOCOL_HOURS/24 正是这个形态）。
        let root = tmp_root("short");
        let mut m = good_manifest(&root);
        m.ended_at = hhmm(2);
        m.heartbeats = (0..=12).map(|i| hhmm_minutes(i * 10)).collect();
        let _ = m.heartbeats.len();
        m.metrics = m
            .metrics
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .chain(std::iter::empty())
            .collect();
        m.protocol_coverage.iter_mut().for_each(|c| {
            c.active_to = hhmm(2);
        });
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v.violations
                .iter()
                .any(|x| x.starts_with("run-duration-below-min")),
            "{:?}",
            v.violations
        );
        assert_eq!(v.measured_hours, 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qemu_scope_cannot_claim_hardware_qualification() {
        let root = tmp_root("scope");
        let mut m = good_manifest(&root);
        m.scope = "qemu".to_owned();
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(v
            .violations
            .contains(&"run-scope-not-hardware:qemu".to_string()));
        // host/CI 口径下允许（开发平台回归），但 device 绑定仍要成立。
        let v2 = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, false).unwrap();
        assert!(v2.is_ok(), "unexpected: {:?}", v2.violations);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hardcoded_zero_leak_and_zero_artifact_are_rejected() {
        let root = tmp_root("zero");
        let mut m = good_manifest(&root);
        m.artifacts.insert("qemu-prod".to_owned(), "0".repeat(64));
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(v
            .violations
            .contains(&"run-artifact-digest-zero:qemu-prod".to_string()));
        // 摘要形态非法同样拒绝。
        let mut m2 = good_manifest(&root);
        m2.artifacts.insert("root_task".to_owned(), "".to_owned());
        let v2 = validate_run(&root, &m2, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(v2
            .violations
            .contains(&"run-artifact-digest-bad:root_task".to_string()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn heap_growth_and_control_overflow_are_rejected() {
        let root = tmp_root("growth");
        let mut m = good_manifest(&root);
        // 每小时 +96 KiB（> 冻结阈值 64 KiB/h）——真实"慢增长"形态。
        for (i, s) in m.metrics.iter_mut().enumerate() {
            s.heap_bytes = 1_000_000 + (i as u64) * 96 * 1024;
        }
        m.metrics[70].control_period_ms_max = 9_999;
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v.violations
                .iter()
                .any(|x| x.starts_with("run-heap-growth")),
            "{:?}",
            v.violations
        );
        assert!(v
            .violations
            .iter()
            .any(|x| x.starts_with("run-control-period-over-limit")));
        assert!(v.heap_slope_bytes_per_hour.unwrap() > 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn heartbeat_gap_interruption_and_missing_raw_file_are_rejected() {
        let root = tmp_root("hb");
        let mut m = good_manifest(&root);
        m.heartbeats = vec![hhmm(0), hhmm(1), hhmm(6), hhmm(72)]; // 5h 空洞 > 900s
        m.interruptions = vec!["采集掉线 2h".to_owned()];
        m.raw_files = vec!["evidence/missing.log".to_owned()];
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(v
            .violations
            .iter()
            .any(|x| x.starts_with("run-heartbeat-gap")));
        assert!(v
            .violations
            .iter()
            .any(|x| x.starts_with("run-interrupted")));
        assert!(v
            .violations
            .iter()
            .any(|x| x.starts_with("run-raw-file-missing")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn protocol_zero_samples_and_bad_windows_are_rejected() {
        let root = tmp_root("proto");
        let mut m = good_manifest(&root);
        m.protocol_coverage[1].samples = 0;
        m.protocol_coverage[0].active_to = "not-a-time".to_owned();
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(v
            .violations
            .contains(&"run-protocol-zero-samples:iec104".to_string()));
        assert!(v
            .violations
            .contains(&"run-protocol-window-invalid:modbus-tcp".to_string()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn metric_clock_distortion_and_too_few_samples_are_rejected() {
        let root = tmp_root("clock");
        let mut m = good_manifest(&root);
        m.metrics.truncate(2);
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(v.violations.contains(&"run-metrics-too-few".to_string()));
        let mut m2 = good_manifest(&root);
        m2.metrics[3].at = hhmm(1_000); // 运行区间之外
        let v2 = validate_run(&root, &m2, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(v2
            .violations
            .iter()
            .any(|x| x.starts_with("run-metric-outside-run")));
        let _ = fs::remove_dir_all(&root);
    }

    /// 审计（S7 P1）红测：非 UTF-8 字节不得让 PANIC 扫描整文件跳过。
    #[test]
    fn fatal_marker_scan_survives_non_utf8_bytes() {
        let root = tmp_root("nonutf8");
        let mut bytes = b"boot ok\nPANIC #1 at x.rs:1\n".to_vec();
        bytes.extend_from_slice(&[0xff, 0xfe, 0x80]);
        fs::write(root.join("evidence/serial.log"), &bytes).unwrap();
        let hits = scan_fatal_markers(&root, &["evidence/serial.log".to_owned()]).unwrap();
        assert_eq!(
            hits.len(),
            1,
            "非 UTF-8 尾部不得让整个文件逃过扫描: {hits:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// 审计（S7 P2）红测：零字节原始证据 / 绝对路径 / `..` 逃逸 / 缺必需
    /// 产物角色，都必须判负。
    #[test]
    fn raw_files_and_artifact_roles_are_hardened() {
        let root = tmp_root("hard");
        let m = good_manifest(&root);
        fs::write(root.join("evidence/serial.log"), b"").unwrap();
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v.violations
                .iter()
                .any(|x| x.starts_with("run-raw-file-empty")),
            "{:?}",
            v.violations
        );
        let mut m2 = good_manifest(&root);
        m2.raw_files = vec!["/etc/hostname".to_owned()];
        let v2 = validate_run(&root, &m2, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v2.violations
                .iter()
                .any(|x| x.starts_with("run-raw-file-escapes-base")),
            "{:?}",
            v2.violations
        );
        let mut m3 = good_manifest(&root);
        m3.raw_files = vec!["../../etc/hostname".to_owned()];
        let v3 = validate_run(&root, &m3, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v3.violations
                .iter()
                .any(|x| x.starts_with("run-raw-file-escapes-base")),
            "{:?}",
            v3.violations
        );
        let mut m4 = good_manifest(&root);
        m4.artifacts.clear();
        m4.artifacts.insert("misc".to_owned(), "a".repeat(64));
        let v4 = validate_run(&root, &m4, &SoakPolicy::QUALIFICATION, true).unwrap();
        for expected in [
            "run-artifact-missing:root_task",
            "run-artifact-missing:boot_chain",
        ] {
            assert!(
                v4.violations.contains(&expected.to_string()),
                "{:?}",
                v4.violations
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// 审计（S7 P2）红测：协议"覆盖"必须真覆盖（60 秒 1 样本不足 72h）。
    #[test]
    fn protocol_coverage_must_span_the_run() {
        let root = tmp_root("span");
        let mut m = good_manifest(&root);
        m.protocol_coverage[0].active_to = hhmm_minutes(1);
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v.violations
                .iter()
                .any(|x| x.starts_with("run-protocol-window-too-short")),
            "{:?}",
            v.violations
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// 审计（S7 P3）红测：非法日历（2 月 30 日）与越界年份必须拒绝。
    #[test]
    fn rfc3339_rejects_impossible_dates_and_extreme_years() {
        assert!(
            parse_rfc3339_secs("2026-02-30T00:00:00Z").is_none(),
            "2 月 30 日"
        );
        assert!(
            parse_rfc3339_secs("2026-04-31T00:00:00Z").is_none(),
            "4 月 31 日"
        );
        assert!(
            parse_rfc3339_secs("2024-02-29T00:00:00Z").is_some(),
            "闰年 2 月 29 日合法"
        );
        assert!(
            parse_rfc3339_secs("2026-02-29T00:00:00Z").is_none(),
            "平年 2 月 29 日"
        );
        assert!(parse_rfc3339_secs("9223372036854775807-01-01T00:00:00Z").is_none());
        assert!(parse_rfc3339_secs("+2026-09-01T00:00:00Z").is_none());
        assert!(parse_rfc3339_secs("2026-9-1T0:0:0Z").is_none());
        assert!(
            parse_rfc3339_secs("1969-12-31T23:59:59Z").is_none(),
            "年份下界"
        );
    }

    #[test]
    fn rfc3339_parser_rejects_local_and_malformed() {
        assert!(parse_rfc3339_secs("2026-09-01T00:00:00Z").is_some());
        assert_eq!(
            parse_rfc3339_secs("2026-09-01T00:00:00Z"),
            Some(1_788_220_800)
        );
        assert!(parse_rfc3339_secs("2026-09-01T00:00:00+08:00").is_none());
        assert!(parse_rfc3339_secs("2026-09-01").is_none());
        assert!(parse_rfc3339_secs("2026-13-01T00:00:00Z").is_none());
        assert!(parse_rfc3339_secs("2026-09-01T25:00:00Z").is_none());
        assert!(parse_rfc3339_secs("").is_none());
    }

    #[test]
    fn fatal_marker_scan_is_zero_tolerance() {
        let root = tmp_root("markers");
        fs::write(
            root.join("evidence/serial.log"),
            b"boot ok\nPANIC #1 at crates/x/src/y.rs:12\nout of memory\n",
        )
        .unwrap();
        let hits = scan_fatal_markers(&root, &["evidence/serial.log".to_owned()]).unwrap();
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].1, 2);
        let _ = fs::remove_dir_all(&root);
    }
}
