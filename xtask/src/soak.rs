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

/// 资源样本必须覆盖的最小运行占比（%）。
///
/// 复查修复（2026-09-13）："至少 3 个样本"没有覆盖要求 ⇒ 把全部样本堆在
/// 运行尾部的 1 小时内，1 MB/h 的真实增长会被斜率换算稀释成 13.7 KB/h 并
/// PASS（实测）。75% 是"样本必须跟着整段运行走"的结构性下限，不假设采样
/// 周期（真机采样周期由试验台脚本决定，判据只要求别把样本挤在一角）。
pub const MIN_METRIC_SPAN_PCT: i64 = 75;

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
    /// 记录的设备事实（SKU/修订）——资格判据用它核对"同一块板"。
    pub device_sku_id: String,
    pub device_board_revision: String,
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
    // 复查修复（2026-09-13）：小数秒必须**定形**——旧实现
    // `rest.split('.').next()` 把 `.` 之后的内容整段丢弃且不校验，
    // `2026-09-01T00:00:00.garbageZ` 被当成合法时刻（实测 PASS）。
    // 只接受 1..=9 位数字的小数秒（丢弃其值，判据只到秒）。
    let secs_part = match rest.split_once('.') {
        Some((head, frac)) => {
            if frac.is_empty() || frac.len() > 9 || !frac.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            head
        }
        None => rest,
    };
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
    // 复查修复：`se == 60`（闰秒）在本算法里只是"多 60 秒"，且证据时刻不会
    // 恰好落在闰秒上——直接拒绝而非静默折叠（旧实现 `se > 60` 放行）。
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&mo) || h > 23 || mi > 59 || se > 59 {
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

/// 摘要的短展示（证据/违规串；不参与判定）。
fn short16(s: &str) -> String {
    s.chars().take(16).collect()
}

/// 字节摘要 → 小写 hex（与 `board.rs` 同口径；判定只比较 hex 串）。
fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// 最小二乘斜率（x = 样本真实时刻相对运行起点的**分钟数**，y = 值）→
/// 换算成"值/小时"的**整数值**。整数定点，避免浮点平台差异。
///
/// 复查修复（2026-09-13）：旧实现 x = **样本序号**，再把 `per_step` 乘
/// `(n-1)` 除以**运行全程小时数**——样本集中在运行尾部 1 小时时，1 MB/h
/// 的真实增长被稀释成 13.7 KB/h 并 PASS（实测）。斜率必须来自真实时刻。
fn slope_per_hour(points: &[(i64, u64)]) -> Option<i64> {
    let n = points.len() as i128;
    if n < 3 {
        return None;
    }
    let x0 = points.first()?.0;
    let mut sx: i128 = 0;
    let mut sy: i128 = 0;
    let mut sxx: i128 = 0;
    let mut sxy: i128 = 0;
    for (t, v) in points {
        // 秒 → 分钟（整数除法）：72h ≈ 4320 分钟，i128 无溢出风险，同时
        // 保住小斜率的分辨率（按秒会让 `num*3600/den` 先丢整数位）。
        let x = ((*t - x0) / 60) as i128;
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
    Some(((n * sxy - sx * sy) * 60 / denom) as i64)
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
        device_sku_id: manifest.device.sku_id.clone(),
        device_board_revision: manifest.device.board_revision.clone(),
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
    // 复查修复（2026-09-13）：产物条目是**路径**，判据必须 `sha256(文件) ==
    // 记录里的摘要`——旧实现只查"值是否为 64 位 hex"，摘要与任何产物都没有
    // 关系（实测：磁盘上不存在任何产物、摘要随便填，仍然 PASS）。
    for (role, spec) in &manifest.artifacts {
        if spec.trim().is_empty() {
            v.violations.push(format!("run-artifact-digest-bad:{role}"));
            continue;
        }
        let (declared, rel) = match spec.split_once('@') {
            Some((d, p)) => (d.trim(), p.trim()),
            None => {
                v.violations.push(format!(
                    "run-artifact-unbound:{role}:必须为 <sha256>@<相对路径>（值不与任何产物绑定时判据无从核对）"
                ));
                continue;
            }
        };
        if !is_sha256(declared) {
            v.violations.push(format!("run-artifact-digest-bad:{role}"));
            continue;
        }
        if declared.chars().all(|c| c == '0') {
            v.violations
                .push(format!("run-artifact-digest-zero:{role}"));
            continue;
        }
        if let Err(reason) = check_rel_path(rel) {
            v.violations
                .push(format!("run-artifact-escapes-base:{role}:{reason}"));
            continue;
        }
        let path = base.join(rel);
        match fs::read(&path) {
            Err(e) => v
                .violations
                .push(format!("run-artifact-missing:{role}:{rel}:{e}")),
            Ok(bytes) => {
                let actual = hex(&eneros_board_facts::digest::sha256(&bytes));
                if actual != declared {
                    v.violations.push(format!(
                        "run-artifact-digest-mismatch:{role}:{}!={}",
                        short16(declared),
                        short16(&actual)
                    ));
                }
            }
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
                    // 复查修复（2026-09-13）：`min_hours as i64 * 3600` 在
                    // `--min-hours 9223372036854775807` 下直接 panic（实测
                    // `attempt to multiply with overflow`）；`min_hours=0`
                    // 时 `needed` 变成负数使整条判据恒真。改为 checked 乘法
                    // + 只在 min_hours > 0 时判定（QUALIFICATION 恒为 72）。
                    let needed = (policy.min_hours as i64)
                        .checked_mul(3_600)
                        .and_then(|x| x.checked_sub(3_600))
                        .unwrap_or(i64::MAX);
                    if policy.min_hours > 0 && span < needed {
                        v.violations.push(format!(
                            "run-protocol-window-too-short:{}:{}s<{}s",
                            c.protocol, span, needed
                        ));
                    }
                    // 复查修复（2026-09-13）：覆盖判据还要有**样本密度**——
                    // 「71 小时的窗口 + 1 个样本」在旧实现里等于全量覆盖
                    // （实测 PASS）。口径：覆盖窗口内至少每 30 分钟 1 个样本，
                    // 且绝对下限 3 个（采样周期由试验台脚本决定，判据只排除
                    // "整段运行只报了几次"的形态）。
                    let min_samples = ((span / 1_800) as u64).max(3);
                    if c.samples < min_samples {
                        v.violations.push(format!(
                            "run-protocol-samples-too-few:{}:{}<{}",
                            c.protocol, c.samples, min_samples
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
        v.heap_peak_bytes = manifest.metrics.iter().map(|m| m.heap_bytes).max();
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
        // 采样时刻必须与运行区间一致（时钟失真负例），且**严格递增**——
        // 旧实现只查"在窗口内"，样本列表顺序即回归顺序：同一组值重排即可
        // 改变斜率结论（复查 2026-09-13）。
        let mut stamped: Vec<(i64, u64)> = Vec::with_capacity(manifest.metrics.len());
        let mut time_ok = true;
        for (i, m) in manifest.metrics.iter().enumerate() {
            match parse_rfc3339_secs(&m.at) {
                None => {
                    v.violations.push(format!("run-metric-time-invalid:{i}"));
                    time_ok = false;
                    break;
                }
                Some(t) => {
                    if let (Some(s), Some(e)) = (started, ended) {
                        if t < s || t > e + 60 {
                            v.violations.push(format!("run-metric-outside-run:{i}"));
                            time_ok = false;
                            break;
                        }
                    }
                    if let Some((pt, _)) = stamped.last() {
                        if t <= *pt {
                            // 重复时刻让最小二乘的分母退化/顺序敏感。
                            v.violations
                                .push(format!("run-metric-time-not-increasing:{i}"));
                            time_ok = false;
                            break;
                        }
                    }
                    stamped.push((t, m.heap_bytes));
                }
            }
        }
        // 样本必须覆盖运行全程的主要部分：旧实现只要求"3 个样本"，把全部
        // 样本堆在运行尾部的 1 小时内即可让斜率被稀释（实测 1 MB/h → 13.7 KB/h
        // PASS）。覆盖判据与时间基斜率（上面）互为独立防线。
        if time_ok && stamped.len() >= 3 {
            if let (Some(s), Some(e)) = (started, ended) {
                let span = stamped.last().expect("non-empty").0 - stamped[0].0;
                let run = e - s;
                let needed = run * MIN_METRIC_SPAN_PCT / 100;
                if run > 0 && span < needed {
                    v.violations.push(format!(
                        "run-metrics-span-too-short:{span}s<{needed}s({MIN_METRIC_SPAN_PCT}% of {run}s)"
                    ));
                }
            }
        }
        // 斜率：单位 = 值/小时（整数定点）。
        match slope_per_hour(&stamped) {
            Some(slope) => {
                v.heap_slope_bytes_per_hour = Some(slope);
                if slope > policy.max_heap_slope_bytes_per_hour {
                    v.violations.push(format!(
                        "run-heap-growth:{slope}B/h>{}B/h",
                        policy.max_heap_slope_bytes_per_hour
                    ));
                }
            }
            None => {
                // 时间基最小二乘不可判（<3 样本或时刻重合）——不得当"零增长"。
                if time_ok {
                    v.violations.push("run-heap-slope-undecidable".to_owned());
                }
            }
        }
        if let Some(p) = v.control_period_peak_ms {
            if p > policy.control_period_limit_ms {
                v.violations.push(format!(
                    "run-control-period-over-limit:{p}ms>{}ms",
                    policy.control_period_limit_ms
                ));
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
        if let Err(reason) = check_rel_path(rel) {
            v.violations
                .push(format!("run-raw-file-escapes-base:{rel}:{reason}"));
        }
    }
    Ok(v)
}

/// 证据路径判据（**单源**：soak / bring-up / 资格记录共用）。
///
/// 拒绝绝对路径与 `..` 逃逸——`base.join("/etc/hostname")` 会替换整个路径，
/// `base.join("../x")` 会指到仓库外，两者都让"证据在仓库内"的判据失效。
pub fn check_rel_path(rel: &str) -> Result<(), &'static str> {
    if rel.trim().is_empty() {
        return Err("empty");
    }
    let p = std::path::Path::new(rel);
    if p.is_absolute() {
        return Err("absolute");
    }
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("parent-dir");
    }
    Ok(())
}

/// 从目录读取运行记录并判定（命令入口共用）。
///
/// 70.14 复查修复：**零泛豁免的 PANIC/OOM 扫描并入本函数**（单源）。
/// 旧实现只在 `command_run_check` 的第二步做扫描，而其它消费者
/// （`board qualify check/verify` 走本函数）拿到的 verdict **不含** fatal
/// 违规 ⇒ 一份原始日志里带未处置 PANIC 的 run 可以支撑 72h 技术资格
/// （S7-4 的红测只覆盖了 `scope=qemu` 这类 validate 层违规）。扫描并入
/// 后：任何"这份 run 是否成立"的判定都含零泛豁免面。
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
    let mut verdict = validate_run(base, &manifest, policy, require_hardware)?;
    for (file, line, text) in scan_fatal_markers(base, &manifest.raw_files)? {
        verdict
            .violations
            .push(format!("run-fatal-marker:{file}:{line}:{text}"));
    }
    Ok(verdict)
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
        // 改为按字节读 + lossy 解码。
        //
        // 复查修复（2026-09-13）：`unwrap_or_default()` 把**读失败**折叠成
        // "空文件 → continue"——注释声称"读不出的文件本身即判负"，实现却
        // 静默跳过（EACCES/EIO/EISDIR 都变成零违规）。读失败必须判负。
        let bytes = fs::read(&path).map_err(|e| {
            invalid(format!(
                "run-fatal-scan-unreadable:{rel}:{e}（读不出的原始证据不得当'无可疑'）"
            ))
        })?;
        if bytes.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (i, line) in text.lines().enumerate() {
            let l = line.to_ascii_lowercase();
            // 70.14 复查修复：**按词边界匹配**。旧实现是裸子串
            // `l.contains("oom")`——真实浸泡/环境日志里的
            // `ambient room temp=25.0C` 命中 `room`（内含 "oom"）⇒ 诚实运行
            // 被 `run-fatal-marker` 整场判负（72h 长跑必撞的假阳性）。
            // 词边界保留全部真实形态：`PANIC` / `panicked at` / `OOM` /
            // `oom-kill` / `oom_reaper`（非字母数字即分词）。
            if fatal_marker_in(&l) {
                hits.push((rel.clone(), i + 1, line.trim().to_owned()));
            }
        }
    }
    Ok(hits)
}

/// 单个（已小写的）行是否含未处置的 PANIC/OOM 致命标记。
///
/// 语义：`panic` / `panicked` / `oom` 必须是**独立词**（避免 `room`、
/// `bloom`、`looming` 之类子串误伤）；`out of memory` 与分配失败签名
/// `memory allocation of` 按短语匹配（无同形误伤）。
///
/// 分配失败签名与 70.x 门判据器 G2 同口径（69.5 的真实失败正是
/// `memory allocation of 114688 bytes failed`）——"零泛豁免"必须同时覆盖
/// panic 与 OOM 两条腿，否则浸泡面会比门窗更宽。
pub fn fatal_marker_in(lower_line: &str) -> bool {
    if lower_line.contains("out of memory") || lower_line.contains("memory allocation of") {
        return true;
    }
    lower_line
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| t == "panic" || t == "panicked" || t == "oom")
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
                let v: u64 = val()?
                    .parse()
                    .map_err(|_| invalid("--min-hours must be u64"))?;
                // 复查修复（2026-09-13）：判据阈值只能**调高**不能调低——
                // `--min-hours 0` 会把"72h 技术资格"降成"任何时长"，且判据
                // 产物里不记录用了哪套阈值（REG-70.14-JUDGE-POLICY-OVERRIDE
                // 的同一性质）。放宽是发布面策略签名问题，不允许由调用方
                // 静默完成；本入口只接受"更严"。
                if v < SoakPolicy::QUALIFICATION.min_hours {
                    return Err(invalid(format!(
                        "--min-hours {v} 低于资格判据下限 {}h（放宽阈值属发布策略，不得由调用方静默完成）",
                        SoakPolicy::QUALIFICATION.min_hours
                    )));
                }
                policy.min_hours = v;
            }
            "--max-heap-slope" => {
                let v: i64 = val()?
                    .parse()
                    .map_err(|_| invalid("--max-heap-slope must be i64"))?;
                if v > SoakPolicy::QUALIFICATION.max_heap_slope_bytes_per_hour {
                    return Err(invalid(format!(
                        "--max-heap-slope {v} 高于资格判据上限 {} B/h（放宽阈值属发布策略，不得由调用方静默完成）",
                        SoakPolicy::QUALIFICATION.max_heap_slope_bytes_per_hour
                    )));
                }
                policy.max_heap_slope_bytes_per_hour = v;
            }
            "--control-limit-ms" => {
                let v: u64 = val()?
                    .parse()
                    .map_err(|_| invalid("--control-limit-ms must be u64"))?;
                // 70.14 复查补齐：同一性质的两条阈值此前**没有**单向闸门
                // （只锁了 min-hours / max-heap-slope）——`--control-limit-ms
                // 999999999` 直接关掉控制周期判据，且判据产物不记录策略。
                if v > SoakPolicy::QUALIFICATION.control_period_limit_ms {
                    return Err(invalid(format!(
                        "--control-limit-ms {v} 高于资格判据上限 {}ms（放宽阈值属发布策略，不得由调用方静默完成）",
                        SoakPolicy::QUALIFICATION.control_period_limit_ms
                    )));
                }
                policy.control_period_limit_ms = v;
            }
            "--max-heartbeat-gap-s" => {
                let v: u64 = val()?
                    .parse()
                    .map_err(|_| invalid("--max-heartbeat-gap-s must be u64"))?;
                // 同上：`--max-heartbeat-gap-s 999999999` 同时关掉"心跳空洞"
                // 与"心跳覆盖全窗"两条判据。
                if v > SoakPolicy::QUALIFICATION.max_heartbeat_gap_s {
                    return Err(invalid(format!(
                        "--max-heartbeat-gap-s {v} 高于资格判据上限 {}s（放宽阈值属发布策略，不得由调用方静默完成）",
                        SoakPolicy::QUALIFICATION.max_heartbeat_gap_s
                    )));
                }
                policy.max_heartbeat_gap_s = v;
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
    // fatal 扫描已在 `load_and_validate_run` 内（单源：资格/能力表等消费者同样
    // 拿到零泛豁免违规；此处不得再扫第二遍）。
    let verdict = load_and_validate_run(&base, &rel, &policy, require_hardware)?;
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

    /// 写出一对"真实"产物文件并返回 `<sha256>@<相对路径>` 绑定串
    /// （复查 2026-09-13：产物条目必须与盘上文件的可算摘要绑定）。
    fn artifact_binding(root: &Path, rel: &str, content: &[u8]) -> String {
        let path = root.join(rel);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(&path, content).unwrap();
        let digest = hex(&eneros_board_facts::digest::sha256(content));
        format!("{digest}@{rel}")
    }

    fn good_manifest(root: &Path) -> SoakRunManifest {
        let mut artifacts = BTreeMap::new();
        artifacts.insert(
            "root_task".to_owned(),
            artifact_binding(root, "artifacts/root-task.bin", b"root-task-image"),
        );
        artifacts.insert(
            "boot_chain".to_owned(),
            artifact_binding(root, "artifacts/boot-chain.bin", b"boot-chain-image"),
        );
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
        m.artifacts.insert(
            "qemu-prod".to_owned(),
            format!("{}@artifacts/x.bin", "0".repeat(64)),
        );
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
        // 复查修复（2026-09-13）红测：摘要必须与盘上文件实算值一致——
        // 只给摘要（无路径）= 无从核对 ⇒ 判负；摘要与内容不符 ⇒ 判负。
        let mut m3 = good_manifest(&root);
        m3.artifacts.insert("root_task".to_owned(), "a".repeat(64));
        let v3 = validate_run(&root, &m3, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v3.violations
                .iter()
                .any(|x| x.starts_with("run-artifact-unbound:root_task")),
            "{:?}",
            v3.violations
        );
        let mut m4 = good_manifest(&root);
        let wrong = format!("{}@artifacts/root-task.bin", "c".repeat(64));
        m4.artifacts.insert("root_task".to_owned(), wrong);
        let v4 = validate_run(&root, &m4, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v4.violations
                .iter()
                .any(|x| x.starts_with("run-artifact-digest-mismatch:root_task")),
            "{:?}",
            v4.violations
        );
        // 产物文件缺失（记录声称有、盘上没有）⇒ 判负。
        let mut m5 = good_manifest(&root);
        let missing = format!("{}@artifacts/absent.bin", "d".repeat(64));
        m5.artifacts.insert("root_task".to_owned(), missing);
        let v5 = validate_run(&root, &m5, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v5.violations
                .iter()
                .any(|x| x.starts_with("run-artifact-missing:root_task")),
            "{:?}",
            v5.violations
        );
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

    /// 复查修复（2026-09-13 第二轮）红测：**产物绑定串**的路径同样必须被
    /// 路径判据把守——旧判据只查摘要形态，`../` 与绝对路径的产物路径从不
    /// 被判（`base.join("/etc/hostname")` 替换整个路径 ⇒ 摘要核对指向仓库
    /// 外的文件）。`check_rel_path` 是 soak / bring-up / 资格三处共用的单源。
    #[test]
    fn artifact_binding_paths_are_guarded_like_raw_files() {
        assert_eq!(check_rel_path(""), Err("empty"));
        assert_eq!(check_rel_path("   "), Err("empty"));
        assert_eq!(check_rel_path("/etc/hostname"), Err("absolute"));
        assert_eq!(check_rel_path("../outside.bin"), Err("parent-dir"));
        assert_eq!(check_rel_path("evidence/../../x"), Err("parent-dir"));
        assert!(check_rel_path("evidence/70/70.14/run/artifacts/root.bin").is_ok());
        assert!(check_rel_path("./artifacts/root.bin").is_ok());

        let root = tmp_root("artpath");
        let mut m = good_manifest(&root);
        m.artifacts.insert(
            "root_task".to_owned(),
            format!("{}@/etc/hostname", "a".repeat(64)),
        );
        let v = validate_run(&root, &m, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v.violations
                .iter()
                .any(|x| x.starts_with("run-artifact-escapes-base:root_task:absolute")),
            "{:?}",
            v.violations
        );
        let mut m2 = good_manifest(&root);
        m2.artifacts.insert(
            "root_task".to_owned(),
            format!("{}@../outside.bin", "b".repeat(64)),
        );
        let v2 = validate_run(&root, &m2, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v2.violations
                .iter()
                .any(|x| x.starts_with("run-artifact-escapes-base:root_task:parent-dir")),
            "{:?}",
            v2.violations
        );
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
        // 复查修复（2026-09-13）红测：小数秒必须定形、闰秒拒绝。
        assert!(
            parse_rfc3339_secs("2026-09-01T00:00:00.garbageZ").is_none(),
            "非法小数位不得被整段丢弃"
        );
        assert!(
            parse_rfc3339_secs("2026-09-01T00:00:00.Z").is_none(),
            "空小数位"
        );
        assert!(
            parse_rfc3339_secs("2026-09-01T00:00:00.1234567890Z").is_none(),
            "小数位超过 9 位"
        );
        assert_eq!(
            parse_rfc3339_secs("2026-09-01T00:00:00.123Z"),
            parse_rfc3339_secs("2026-09-01T00:00:00Z"),
            "合法小数秒按秒截断"
        );
        assert!(
            parse_rfc3339_secs("2026-09-01T00:00:60Z").is_none(),
            "闰秒不静默折叠成下一分钟"
        );
    }

    /// 复查修复（2026-09-13）红测：不可读的原始证据不得当"无可疑"。
    #[test]
    fn fatal_scan_fails_closed_on_unreadable_evidence() {
        let root = tmp_root("scan-unreadable");
        // 目录：`is_file()` 为假（走缺失分支）；这里直接对**不存在的**路径
        // 断言 Err（旧实现 `unwrap_or_default()` 会把任何读失败折叠成空 ⇒ Ok([])）。
        let err = scan_fatal_markers(&root, &["evidence/absent.log".to_owned()]).unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("run-fatal-scan-unreadable"),
            "读不出的原始证据必须判负：{msg}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// 70.14 复查红测：致命标记扫描**按词边界**，不按裸子串。
    ///
    /// 缺陷形态（旧实现）：`l.contains("oom")`——真实 72h 浸泡/环境日志里
    /// `ambient room temp=25.0C` 的 `room` 就含 `oom`，诚实运行被
    /// `run-fatal-marker` 整场判负（"下一次实弹必撞"的假阳性）；同时
    /// `bloom`/`looming` 同理。零泛豁免语义必须保留（真 PANIC/OOM 行照命中）。
    #[test]
    fn fatal_marker_matching_is_word_bounded_not_substring() {
        // 不得误伤（词边界）：room / bloom / looming / boomtown。
        for ok in [
            "[sensor] ambient room temp=25.0C humidity=40%",
            "[orch] bloom filter rebuilt",
            "forecast: looming degradation acceptable",
            "site=boomtown relay closed",
        ] {
            assert!(
                !fatal_marker_in(&ok.to_ascii_lowercase()),
                "合法行不得被判致命：{ok}"
            );
        }
        // 必须命中（真实形态，零泛豁免不变）。
        for bad in [
            "PANIC: panicked at crates/x/src/lib.rs:12",
            "[svc-x] panicked at 'called Result::unwrap()'",
            "memory allocation of 114688 bytes failed",
            "Out of memory: Killed process 123",
            "kernel: oom-kill:constraint=CONSTRAINT_NONE",
            "oom_reaper: reaped process 42",
        ] {
            assert!(
                fatal_marker_in(&bad.to_ascii_lowercase()),
                "致命行必须命中：{bad}"
            );
        }
    }

    /// 70.14 复查红测：**资格消费者拿到的 verdict 必须含零泛豁免违规**。
    ///
    /// 缺陷形态（旧实现）：fatal 扫描只在 `command_run_check` 的第二步执行，
    /// 而 `board qualify check/verify` 直接调 `load_and_validate_run` ⇒ 一份
    /// 原始日志里带未处置 PANIC 的 run 可以支撑 72h 技术资格（与"任何未处置
    /// PANIC/OOM 使整场失败"直接冲突）。扫描并入 `load_and_validate_run` 后，
    /// 所有消费者（含资格）都拿到 `run-fatal-marker:*`。
    #[test]
    fn fatal_markers_are_part_of_every_run_consumer_verdict() {
        let root = tmp_root("fatal-in-load");
        let m = good_manifest(&root);
        let rel = "evidence/run.json";
        // 原始日志里写入一条真实 PANIC（其余字段全合规）。
        fs::write(
            root.join("evidence/serial.log"),
            "boot ok\nPANIC #1 at x.rs:1\n",
        )
        .unwrap();
        fs::write(root.join(rel), serde_json::to_vec_pretty(&m).unwrap()).unwrap();
        let v = load_and_validate_run(&root, rel, &SoakPolicy::QUALIFICATION, true).unwrap();
        assert!(
            v.violations
                .iter()
                .any(|x| x.starts_with("run-fatal-marker:")),
            "加载判据（资格/能力表消费者共用）必须含 PANIC 违规：{:?}",
            v.violations
        );
        let _ = fs::remove_dir_all(&root);
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
