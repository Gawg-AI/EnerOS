//! 70.12/70.13/70.14（方向 6）：板级事实、bring-up 记录、能力矩阵与技术
//! 资格记录的机器 gate。
//!
//! # 与 `catalog` 的分工
//! `catalog` 管"目录/矩阵/资格状态"（43 卷既有语义，本卷只加硬门）；
//! 本模块管**方向 6 的原始工件**：
//! - `board fact-validate`：冻结记录 v2 账本判据（族约束/内存重叠/硬门能力）
//!   —— 判据实现单源在 `eneros-board-facts` crate（目标侧 root 用同一份）；
//! - `board bring-up check`：E70-D 切片——实板 bring-up 记录的字段与
//!   硬件 scope 合法性（QEMU/合成**不得**冒填实板证据）；
//! - `board matrix`：WP-70.13.5 —— 从目录 + 冻结记录派生"精确能力表"，
//!   并把"枚举到硬件但无驱动 / 驱动就绪但无设备合同"分成不同状态；
//! - `board qualify`：WP-70.14.5 —— 技术资格记录（scope/required case/
//!   影响映射/复验撤销规则）的读写与判据。
//!
//! 全部命令 fail-closed：缺工件 → 非零退出 + 机器可读 reason。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{hex, invalid, read_json, DynResult};

/// bring-up 记录 schema（E70-D 切片；蓝图 §7 证据字段的结构化载体）。
pub const BRINGUP_SCHEMA: &str = "eneros.board-bringup.v1";
/// 技术资格记录 schema（WP-70.14.5）。
pub const QUALIFICATION_SCHEMA: &str = "eneros.board-qualification.v1";
/// 精确能力表 schema（WP-70.13.5）。
pub const CAPABILITY_MATRIX_SCHEMA: &str = "eneros.board-capability-matrix.v1";
/// 硬件 scope 的合法取值（其它一律拒绝——QEMU/合成不得冒充实板）。
pub const HARDWARE_SCOPES: [&str; 3] = ["hw-bringup", "hw-hil", "hw-soak"];
/// 非硬件 scope（允许存在，但不得用于实板资格）。
pub const NON_HARDWARE_SCOPES: [&str; 4] = ["qemu", "synthetic", "host", "ci"];

// ============================================================
// 数据模型
// ============================================================

/// 冻结记录里的结构化事实项（v2）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactEntry {
    pub subsystem: String,
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub unit: String,
    pub verified: bool,
    /// `repo` | `vendor-doc` | `hardware`。
    pub hardware_scope: String,
    pub source: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub digest: Option<String>,
}

/// 缺失登记项（v2 结构化形态：族 + 说明）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissingEntry {
    pub subsystem: String,
    pub note: String,
}

/// 实板 bring-up 记录（E70-D）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BringUpRecord {
    pub schema: String,
    pub sku_id: String,
    pub board_revision: String,
    /// 实板序列号（未绑定序列号的结果不得归档为实板 PASS）。
    pub serial: String,
    /// 仪器/试验台标识（观察面独立性；空 = 由被测系统自报，拒绝）。
    pub rig_id: String,
    /// 证据 scope：`hw-*` 为真实硬件；其余不得用于实板资格。
    pub scope: String,
    /// 冻结记录摘要（必须与当前记录一致）。
    pub fact_record_digest: String,
    /// 产物绑定：角色 → sha256。
    pub artifacts: BTreeMap<String, String>,
    /// 必需服务就绪列表（启动后 required 不齐不得进入控制就绪）。
    pub required_services: Vec<String>,
    /// 原始证据文件（相对仓库根）。
    pub raw_evidence: Vec<String>,
    /// 执行/审核角色分离登记。
    pub operators: Operators,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operators {
    /// 负责真实设备事实的人（要求非作者复现时为复现人）。
    pub device_facts_by: String,
    /// 执行 bring-up 的人。
    pub bringup_by: String,
    /// 独立审核人（不得与 bringup_by 相同）。
    pub reviewed_by: String,
}

/// 影响映射项（WP-70.14.5：组件改动 → 受影响场景）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactBinding {
    /// `hardware` | `firmware` | `driver` | `protocol` | `config`。
    pub component: String,
    /// 组件摘要（sha256；变化即触发复验）。
    pub digest: String,
    /// 该组件影响到的场景 ID 列表。
    pub scenarios: Vec<String>,
}

/// 技术资格记录（WP-70.14.5）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationRecord {
    pub schema: String,
    pub candidate_id: String,
    pub sku_id: String,
    pub board_revision: String,
    /// 资格 scope（`hw-hil`/`hw-soak`；`l4` 由 catalog gate 复核）。
    pub scope: String,
    /// 功能/性能/恢复覆盖的用例（required case 绑定）。
    pub required_cases: Vec<CaseBinding>,
    /// 运行记录（evidence 相对路径 + run_id）。
    pub runs: Vec<RunBinding>,
    /// 组件影响映射（复验裁剪依据）。
    pub impacts: Vec<ImpactBinding>,
    /// 冻结记录摘要（必须与当前 `product/skus/<sku>.json` 的结构账本摘要
    /// 一致；换板/换记录即失效——资格不可跨组合继承）。
    pub fact_record_digest: String,
    /// 明确不支持的功能（精确能力表口径）。
    pub unsupported: Vec<String>,
    /// 限制与外部待证。
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseBinding {
    pub case_id: String,
    /// 该用例归属的 required 场景（CORE-70-Rxx 或卷内 WP 判据）。
    pub requirement: String,
    /// 证据相对路径。
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunBinding {
    pub run_id: String,
    pub scenario: String,
    /// `hw-hil` | `hw-soak` | `qemu` | …（与 scope 同域）。
    pub scope: String,
    pub started_at: String,
    pub ended_at: String,
    pub manifest: String,
}

// ============================================================
// 命令分发
// ============================================================

pub fn command_board(root: &Path, args: &[String]) -> DynResult<()> {
    match args.split_first() {
        None => Err(invalid(
            "board requires a subcommand: fact-validate|bring-up|matrix|qualify",
        )),
        Some((cmd, tail)) => match cmd.as_str() {
            "fact-validate" => command_fact_validate(root, tail),
            "bring-up" => command_bring_up(root, tail),
            "matrix" => command_matrix(root, tail),
            "qualify" => command_qualify(root, tail),
            "help" | "--help" | "-h" => {
                println!(
                    "board subcommands: fact-validate|bring-up|matrix|qualify\n\
                     \x20 fact-validate [--sku <id>]  冻结记录账本判据（v2）\n\
                     \x20 bring-up check <record.json>  实板 bring-up 记录判据\n\
                     \x20 matrix [--sku <id>] [--out <path>]  精确能力表\n\
                     \x20 qualify check <record.json> | verify <record.json>  技术资格记录"
                );
                Ok(())
            }
            other => Err(invalid(format!("unknown board subcommand '{other}'"))),
        },
    }
}

// ============================================================
// fact-validate：冻结记录 v2 账本判据
// ============================================================

/// 冻结记录的 v2 视图（v1 记录没有这些字段 → `entries` 为空）。
/// 冻结记录的 v2 视图。
///
/// 读取走 [`read_freeze_view`]（先按原始 JSON 判形态再严格解析）——v1 记录
/// 没有 `entries`/`structured_missing` 字段，直接 `deny_unknown_fields` 解析
/// 会把 v1 记录整体拒绝（迁移期不可接受）；但 v2 记录里出现未知字段必须
/// 拒绝（防拼写错被静默忽略）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardFactFreezeV2View {
    pub schema: String,
    pub sku_id: String,
    /// 冻结时刻（v1/v2 共有；证据归档字段，不参与判据）。
    #[serde(default)]
    pub frozen_at: String,
    /// 冻结基线 commit（v1/v2 共有；证据归档字段，不参与判据）。
    #[serde(default)]
    pub source_commit: String,
    #[serde(default)]
    pub revision: String,
    #[serde(default)]
    pub ledger_version: u32,
    #[serde(default)]
    pub entries: Vec<FactEntry>,
    /// v2 结构化缺失登记（族 + 说明）；v1 形态由 [`read_freeze_view`] 归一。
    #[serde(default)]
    pub structured_missing: Vec<MissingEntry>,
    /// v1 形态的自由文本缺失清单（v2 记录也允许保留人读说明）。
    #[serde(default)]
    pub missing: Vec<String>,
}

/// 读取冻结记录的 v2 视图（catalog validate/gate 复用同一解析）。
///
/// 形态判定（先判再解析，不靠"猜字段"）：
/// - 原始 JSON 无 `entries` → v1 记录：整体转为 v1 视图，`entries` 为空；
/// - 有 `entries` → 严格解析 v2（未知字段即拒绝）。
pub fn read_freeze_view(path: &Path) -> DynResult<BoardFactFreezeV2View> {
    let raw = read_json::<serde_json::Value>(path)?;
    let has_entries = raw.get("entries").is_some();
    if !has_entries {
        let schema = raw
            .get("schema")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned();
        let sku_id = raw
            .get("sku_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned();
        let missing: Vec<String> = raw
            .get("missing")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        return Ok(BoardFactFreezeV2View {
            schema,
            sku_id,
            frozen_at: raw
                .get("frozen_at")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            source_commit: raw
                .get("source_commit")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_owned(),
            revision: String::new(),
            ledger_version: 1,
            entries: Vec::new(),
            structured_missing: Vec::new(),
            missing,
        });
    }
    let view: BoardFactFreezeV2View = serde_json::from_value(raw)
        .map_err(|e| invalid(format!("freeze record {}: {e}", path.display())))?;
    Ok(view)
}

fn command_fact_validate(root: &Path, args: &[String]) -> DynResult<()> {
    let mut sku: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sku" => {
                sku = Some(
                    it.next()
                        .ok_or_else(|| invalid("--sku requires a value"))?
                        .clone(),
                )
            }
            other => {
                return Err(invalid(format!(
                    "unknown board fact-validate arg '{other}'"
                )))
            }
        }
    }
    let catalog = read_json::<serde_json::Value>(&root.join("product/compatibility-catalog.json"))?;
    let skus: Vec<(String, String)> = catalog
        .get("skus")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid("catalog has no skus[]"))?
        .iter()
        .filter_map(|s| {
            let id = s.get("sku_id")?.as_str()?.to_owned();
            let kind = s
                .get("kind")
                .and_then(|k| k.as_str())
                .unwrap_or("development")
                .to_owned();
            Some((id, kind))
        })
        .collect();
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for (id, kind) in &skus {
        if let Some(only) = &sku {
            if only != id {
                continue;
            }
        }
        checked += 1;
        let path = root.join(format!("product/skus/{id}.json"));
        if !path.is_file() {
            failures.push(format!("sku-fact-record-missing:{id}"));
            continue;
        }
        let view = read_freeze_view(&path)?;
        // 能力覆盖硬门只对 `qualified` 生效（见 validate_freeze_v2_qualified）：
        // 本命令判的是**记录形态与族一致性**，不冒充资格裁决。
        let mut f = validate_freeze_v2_qualified(&view, id, kind, false);
        failures.append(&mut f);
    }
    if checked == 0 {
        return Err(invalid("board fact-validate matched no SKU"));
    }
    if failures.is_empty() {
        println!("BOARD-FACT-VALIDATE PASS skus={checked}");
        Ok(())
    } else {
        Err(invalid(format!(
            "BOARD-FACT-VALIDATE FAIL ({} violation(s)): {}",
            failures.len(),
            failures.join("; ")
        )))
    }
}

/// 从视图提取（事实, 缺失登记）模型对（供判据/摘要复用；未知族在此具名拒绝）。
fn extract_ledger_parts(
    view: &BoardFactFreezeV2View,
    catalog_sku: &str,
) -> (
    Vec<eneros_board_facts::fact::Fact>,
    Vec<(eneros_board_facts::capability::Subsystem, String)>,
    Vec<String>,
) {
    use eneros_board_facts::capability::Subsystem;
    use eneros_board_facts::fact::{Fact, HardwareScope};

    let mut failures = Vec::new();
    let mut facts: Vec<Fact> = Vec::new();
    for e in &view.entries {
        let Some(sub) = Subsystem::parse(&e.subsystem) else {
            failures.push(format!(
                "ledger-unknown-subsystem:{}:{}",
                catalog_sku, e.subsystem
            ));
            continue;
        };
        let Some(scope) = HardwareScope::parse(&e.hardware_scope) else {
            failures.push(format!(
                "fact-scope-unknown:{}:{}#{}:{}",
                catalog_sku, e.subsystem, e.key, e.hardware_scope
            ));
            continue;
        };
        facts.push(Fact {
            subsystem: sub,
            key: e.key.clone(),
            value: e.value.clone(),
            unit: e.unit.clone(),
            verified: e.verified,
            hardware_scope: scope,
            source: e.source.clone(),
            evidence: e.evidence.clone(),
            digest: e.digest.clone(),
        });
    }
    let mut missing: Vec<(Subsystem, String)> = Vec::new();
    for m in &view.structured_missing {
        match Subsystem::parse(&m.subsystem) {
            Some(s) => missing.push((s, m.note.clone())),
            None => failures.push(format!(
                "ledger-unknown-subsystem:{}:{}",
                catalog_sku, m.subsystem
            )),
        }
    }
    (facts, missing, failures)
}

/// 冻结记录 v2 判据（单源 = `eneros-board-facts`；此处只做 JSON ↔ 模型桥接）。
///
/// `qualified=true` 时才追加能力覆盖硬门：未资格化的候选板卡允许能力族尚未
/// 冻结（必须已在 `structured_missing` 逐族登记），但**要出资格就必须全数
/// 验证**（蓝图 C4–C12：无介质/无 TRNG/无防回滚/无收发器不得出产品资格）。
pub fn validate_freeze_v2_qualified(
    view: &BoardFactFreezeV2View,
    catalog_sku: &str,
    catalog_kind: &str,
    qualified: bool,
) -> Vec<String> {
    use eneros_board_facts::capability::SkuKind;
    use eneros_board_facts::ledger::BoardLedger;

    let mut failures = Vec::new();
    if view.sku_id != catalog_sku {
        failures.push(format!("sku-fact-id-mismatch:{catalog_sku}"));
    }
    if view.schema != "eneros.board-fact-freeze.v1" && view.schema != "eneros.board-fact-freeze.v2"
    {
        failures.push(format!("sku-fact-schema:{}:{}", catalog_sku, view.schema));
    }
    if view.entries.is_empty() {
        // v1 记录：存在性够用（诚实分级：以 v1 覆盖度为准，不出具 v2 判据）。
        // 但**产品**板卡必须给出结构化账本——否则硬件事实无法被机器核对。
        if catalog_kind == "product" {
            failures.push(format!("sku-fact-ledger-required:{catalog_sku}"));
        }
        if view.frozen_at.trim().is_empty() || view.source_commit.trim().is_empty() {
            // 冻结时刻与基线 commit 是"这份记录何时、基于哪个代码身份取得"的
            // 最小可核对字段（v2 记录必须给）。
            failures.push(format!("sku-fact-header-incomplete:{catalog_sku}"));
        }
        return failures;
    }
    if view.ledger_version != 0 && view.ledger_version != 2 {
        failures.push(format!(
            "sku-fact-ledger-version:{}:{}",
            catalog_sku, view.ledger_version
        ));
    }
    if view.frozen_at.trim().is_empty() || view.source_commit.trim().is_empty() {
        failures.push(format!("sku-fact-header-incomplete:{catalog_sku}"));
    }
    if view.missing.is_empty() && view.structured_missing.is_empty() {
        // 两份缺失清单都空 → 该组合**声明**全部事实已冻结；此时能力覆盖
        // 硬门必然生效（`qualified` 路径），此处仅作形态提示，避免"空清单
        // 被当成完整"的误读。
        failures.push(format!("sku-fact-no-missing-ledger:{catalog_sku}"));
    }
    let (facts, missing, mut extra) = extract_ledger_parts(view, catalog_sku);
    failures.append(&mut extra);
    let kind = SkuKind::parse(catalog_kind).unwrap_or(SkuKind::Development);
    let ledger = match BoardLedger::new(
        &view.sku_id,
        &view.revision,
        if view.entries.is_empty() { 1 } else { 2 },
        facts,
        missing,
    ) {
        Ok(l) => l,
        Err(e) => {
            failures.push(format!("{}:{}", e.reason(), catalog_sku));
            return failures;
        }
    };
    failures.extend(ledger.validate(kind).violations);
    if qualified {
        failures.extend(ledger.capability_coverage(kind));
    }
    failures
        .into_iter()
        .map(|v| format!("{v}:{catalog_sku}"))
        .collect()
}

/// 兼容入口（未资格化口径：只判记录形态与族一致性，不判能力覆盖）。
///
/// 供 catalog 侧与测试复用；`catalog validate` 走
/// [`validate_freeze_v2_qualified`]（按 SKU 状态决定是否加能力硬门）。
#[allow(dead_code)]
pub fn validate_freeze_v2(
    view: &BoardFactFreezeV2View,
    catalog_sku: &str,
    catalog_kind: &str,
) -> Vec<String> {
    validate_freeze_v2_qualified(view, catalog_sku, catalog_kind, false)
}

// ============================================================
// bring-up：实板记录判据（E70-D）
// ============================================================

fn command_bring_up(root: &Path, args: &[String]) -> DynResult<()> {
    match args.split_first() {
        Some((cmd, tail)) if cmd == "check" => {
            let record_rel = tail
                .first()
                .ok_or_else(|| invalid("bring-up check requires <record.json>"))?;
            let path = root.join(record_rel);
            if !path.is_file() {
                return Err(invalid(format!(
                    "BOARD-BRINGUP FAIL: record missing: {record_rel}"
                )));
            }
            let record = read_json::<BringUpRecord>(&path)?;
            let failures = validate_bringup(root, &record, record_rel)?;
            if failures.is_empty() {
                println!("BOARD-BRINGUP PASS record={record_rel}");
                Ok(())
            } else {
                Err(invalid(format!(
                    "BOARD-BRINGUP FAIL ({} violation(s)): {}",
                    failures.len(),
                    failures.join("; ")
                )))
            }
        }
        _ => Err(invalid("board bring-up requires: check <record.json>")),
    }
}

/// 实板 bring-up 记录判据（全部 fail-closed）。
pub fn validate_bringup(
    root: &Path,
    record: &BringUpRecord,
    record_rel: &str,
) -> DynResult<Vec<String>> {
    let mut v = Vec::new();
    if record.schema != BRINGUP_SCHEMA {
        v.push(format!("bringup-schema:{}", record.schema));
    }
    if record.sku_id.trim().is_empty() {
        v.push("bringup-sku-empty".to_owned());
    }
    if record.board_revision.trim().is_empty() {
        v.push("bringup-revision-empty".to_owned());
    }
    // 序列号必须绑定到具体一台实物：未绑定不得归档实板 PASS。
    if record.serial.trim().is_empty() || record.serial.eq_ignore_ascii_case("unknown") {
        v.push("bringup-serial-unbound".to_owned());
    }
    if record.rig_id.trim().is_empty() {
        v.push("bringup-rig-unbound".to_owned());
    }
    // scope 合法性：hw-* 才具硬件资格；qemu/synthetic/host/ci 一律拒绝
    // （"QEMU 冒填实板"是本切片的直接负例）。
    if !HARDWARE_SCOPES.contains(&record.scope.as_str()) {
        if NON_HARDWARE_SCOPES.contains(&record.scope.as_str()) {
            v.push(format!("bringup-non-hardware-scope:{}", record.scope));
        } else {
            v.push(format!("bringup-scope-unknown:{}", record.scope));
        }
    }
    if record.operators.bringup_by.trim().is_empty()
        || record.operators.device_facts_by.trim().is_empty()
    {
        v.push("bringup-operator-missing".to_owned());
    }
    if record.operators.reviewed_by.trim().is_empty()
        || record.operators.reviewed_by == record.operators.bringup_by
    {
        // 审核与执行不得同一人（蓝图 §7「不同角色」）。
        v.push("bringup-reviewer-not-independent".to_owned());
    }
    // 冻结记录绑定：摘要必须与当前 SKU 记录一致。
    let freeze_rel = format!("product/skus/{}.json", record.sku_id);
    let freeze_path = root.join(&freeze_rel);
    if !freeze_path.is_file() {
        v.push(format!("bringup-freeze-missing:{}", record.sku_id));
    } else {
        let digest = freeze_record_digest(root, &record.sku_id)?;
        if record.fact_record_digest.is_empty() {
            v.push("bringup-freeze-digest-empty".to_owned());
        } else if record.fact_record_digest != digest {
            v.push(format!(
                "bringup-freeze-digest-stale:{}!={}",
                short(&record.fact_record_digest),
                short(&digest)
            ));
        }
    }
    // 产物绑定：至少要有启动链与 root 产物摘要，且摘要形态合法。
    if record.artifacts.is_empty() {
        v.push("bringup-artifacts-empty".to_owned());
    }
    for (role, digest) in &record.artifacts {
        if role.trim().is_empty() {
            v.push("bringup-artifact-role-empty".to_owned());
        }
        if !is_sha256(digest) {
            v.push(format!("bringup-artifact-digest-bad:{role}"));
        }
    }
    if !record.artifacts.contains_key("boot_chain") {
        v.push("bringup-artifact-missing:boot_chain".to_owned());
    }
    if !record.artifacts.contains_key("root_task") {
        v.push("bringup-artifact-missing:root_task".to_owned());
    }
    // 必需服务：不允许空列表（"全部就绪"必须有名单）。
    if record.required_services.is_empty() {
        v.push("bringup-required-services-empty".to_owned());
    }
    // 原始证据文件必须存在（缺一 required 原始文件 = 失败）。
    if record.raw_evidence.is_empty() {
        v.push("bringup-raw-evidence-empty".to_owned());
    }
    for rel in &record.raw_evidence {
        if !root.join(rel).is_file() {
            v.push(format!("bringup-raw-evidence-missing:{rel}"));
        }
    }
    if v.is_empty() {
        println!(
            "BOARD-BRINGUP-BINDING record={record_rel} sku={} serial={} rig={}",
            record.sku_id, record.serial, record.rig_id
        );
    }
    Ok(v)
}

/// 冻结记录的规范摘要（与 `eneros-board-facts::digest::ledger_digest` 同源；
/// v1 记录退化为"文件字节摘要"，但**必须**显式标注口径差异）。
pub fn freeze_record_digest(root: &Path, sku_id: &str) -> DynResult<String> {
    let path = root.join(format!("product/skus/{sku_id}.json"));
    let view = read_freeze_view(&path)?;
    if view.entries.is_empty() {
        // v1：无结构化账本 → 用文件字节摘要，并要求调用方按 "v1:" 前缀识别。
        let bytes = fs::read(&path)?;
        return Ok(format!("v1:{}", hex(&Sha256::digest(&bytes))));
    }
    use eneros_board_facts::digest::ledger_digest;
    let (facts, _missing, _failures) = extract_ledger_parts(&view, &view.sku_id);
    Ok(ledger_digest(&view.sku_id, &view.revision, &facts))
}

fn short(s: &str) -> String {
    s.chars().take(12).collect()
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

// ============================================================
// matrix：精确能力表（WP-70.13.5）
// ============================================================

fn command_matrix(root: &Path, args: &[String]) -> DynResult<()> {
    let mut sku: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sku" => sku = Some(it.next().ok_or_else(|| invalid("--sku"))?.clone()),
            "--out" => out = Some(root.join(it.next().ok_or_else(|| invalid("--out"))?)),
            other => return Err(invalid(format!("unknown board matrix arg '{other}'"))),
        }
    }
    let catalog = read_json::<serde_json::Value>(&root.join("product/compatibility-catalog.json"))?;
    let matrix: Vec<serde_json::Value> = catalog
        .get("skus")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid("catalog has no skus[]"))?
        .iter()
        .filter(|s| match &sku {
            Some(only) => s.get("sku_id").and_then(|x| x.as_str()) == Some(only.as_str()),
            None => true,
        })
        .cloned()
        .collect();
    if matrix.is_empty() {
        return Err(invalid("board matrix matched no SKU"));
    }
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for entry in &matrix {
        let sku_id = entry
            .get("sku_id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_owned();
        let kind = entry
            .get("kind")
            .and_then(|x| x.as_str())
            .unwrap_or("development");
        let drivers: BTreeSet<String> = entry
            .get("drivers")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let protocols: Vec<String> = entry
            .get("protocols")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let freeze_rel = format!("product/skus/{sku_id}.json");
        let freeze = read_freeze_view(&root.join(&freeze_rel)).ok();
        let declared_missing: BTreeSet<String> = freeze
            .as_ref()
            .map(|f| {
                f.structured_missing
                    .iter()
                    .map(|m| m.subsystem.clone())
                    .collect()
            })
            .unwrap_or_default();
        let fact_families: BTreeSet<String> = freeze
            .as_ref()
            .map(|f| f.entries.iter().map(|e| e.subsystem.clone()).collect())
            .unwrap_or_default();
        for proto in &protocols {
            // 三态区分（不许统一"在线成功"）：驱动缺 / 设备合同缺 / 已资格。
            let driver_present = drivers.iter().any(|d| d.contains(proto));
            let state = if !driver_present {
                "driver-missing"
            } else if kind == "development" {
                "development-only"
            } else {
                "contract-required"
            };
            rows.push(serde_json::json!({
                "sku_id": sku_id,
                "kind": kind,
                "protocol": proto,
                "driver_present": driver_present,
                "state": state,
                "fact_families": fact_families.iter().cloned().collect::<Vec<_>>(),
                "declared_missing": declared_missing.iter().cloned().collect::<Vec<_>>(),
                "qualification": if kind == "development" { "not-applicable" } else { "unqualified" },
            }));
        }
        // 无协议的 SKU（真实板卡未接通协议）也要有一行，避免"零行=通过"。
        if protocols.is_empty() {
            rows.push(serde_json::json!({
                "sku_id": sku_id,
                "kind": kind,
                "protocol": "",
                "driver_present": false,
                "state": if kind == "product" { "no-device-contract" } else { "development-only" },
                "fact_families": fact_families.iter().cloned().collect::<Vec<_>>(),
                "declared_missing": declared_missing.iter().cloned().collect::<Vec<_>>(),
                "qualification": if kind == "development" { "not-applicable" } else { "unqualified" },
            }));
        }
    }
    let report = serde_json::json!({
        "schema": CAPABILITY_MATRIX_SCHEMA,
        "generated_by": "cargo xtask board matrix",
        "rows": rows,
    });
    match out {
        Some(path) => {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
            println!(
                "BOARD-MATRIX wrote {} rows to {}",
                report["rows"].as_array().map(|r| r.len()).unwrap_or(0),
                path.display()
            );
        }
        None => println!("{}", serde_json::to_string_pretty(&report)?),
    }
    Ok(())
}

// ============================================================
// qualify：技术资格记录（WP-70.14.5）
// ============================================================

fn command_qualify(root: &Path, args: &[String]) -> DynResult<()> {
    match args.split_first() {
        Some((cmd, tail)) if cmd == "check" || cmd == "verify" => {
            let rel = tail
                .first()
                .ok_or_else(|| invalid("board qualify check requires <record.json>"))?;
            let path = root.join(rel);
            if !path.is_file() {
                return Err(invalid(format!(
                    "BOARD-QUALIFY FAIL: record missing: {rel}"
                )));
            }
            let record = read_json::<QualificationRecord>(&path)?;
            let failures = validate_qualification(root, &record, cmd == "verify")?;
            if failures.is_empty() {
                println!("BOARD-QUALIFY PASS record={rel} sku={}", record.sku_id);
                Ok(())
            } else {
                Err(invalid(format!(
                    "BOARD-QUALIFY FAIL ({} violation(s)): {}",
                    failures.len(),
                    failures.join("; ")
                )))
            }
        }
        _ => Err(invalid(
            "board qualify requires: check|verify <record.json>",
        )),
    }
}

/// 技术资格记录判据。
///
/// `verify`（复验）模式额外做影响映射核对：任一组件摘要与当前值不符 →
/// 受影响场景撤销（**撤销当前适用状态**，历史记录保留）。
pub fn validate_qualification(
    root: &Path,
    record: &QualificationRecord,
    verify_impacts: bool,
) -> DynResult<Vec<String>> {
    let mut v = Vec::new();
    if record.schema != QUALIFICATION_SCHEMA {
        v.push(format!("qualify-schema:{}", record.schema));
    }
    if record.candidate_id.trim().is_empty() {
        v.push("qualify-candidate-empty".to_owned());
    }
    if !HARDWARE_SCOPES.contains(&record.scope.as_str()) {
        v.push(format!("qualify-scope-not-hardware:{}", record.scope));
    }
    if record.required_cases.is_empty() {
        // 空 required 集 = "零用例通过"（不得当成通过）。
        v.push("qualify-required-cases-empty".to_owned());
    }
    let mut seen_cases: BTreeSet<&str> = BTreeSet::new();
    for c in &record.required_cases {
        if c.case_id.trim().is_empty() || c.requirement.trim().is_empty() {
            v.push("qualify-case-incomplete".to_owned());
            continue;
        }
        if !seen_cases.insert(c.case_id.as_str()) {
            v.push(format!("qualify-case-duplicate:{}", c.case_id));
        }
        let case_check = qualify_evidence_check(root, &c.evidence, "case");
        if !case_check.is_empty() {
            v.push(case_check);
        }
    }
    if record.runs.is_empty() {
        v.push("qualify-runs-empty".to_owned());
    }
    let mut required_manifest = false;
    for r in &record.runs {
        if r.run_id.trim().is_empty() || r.scenario.trim().is_empty() {
            v.push("qualify-run-incomplete".to_owned());
        }
        if !HARDWARE_SCOPES.contains(&r.scope.as_str()) {
            v.push(format!("qualify-run-not-hardware:{}", r.scope));
        }
        if r.started_at.trim().is_empty() || r.ended_at.trim().is_empty() {
            v.push(format!("qualify-run-time-missing:{}", r.run_id));
        }
        if r.manifest.trim().is_empty() || !root.join(&r.manifest).is_file() {
            v.push(format!("qualify-run-manifest-missing:{}", r.manifest));
        } else {
            required_manifest = true;
        }
    }
    if !required_manifest {
        v.push("qualify-no-run-manifest".to_owned());
    }
    // 影响映射：组件/摘要形态 + 场景非空。
    let mut seen_components: BTreeSet<&str> = BTreeSet::new();
    for i in &record.impacts {
        if !["hardware", "firmware", "driver", "protocol", "config"].contains(&i.component.as_str())
        {
            v.push(format!("qualify-impact-component:{}", i.component));
        }
        if !is_sha256(&i.digest) {
            v.push(format!("qualify-impact-digest:{}", i.component));
        }
        if i.scenarios.is_empty() {
            v.push(format!("qualify-impact-no-scenario:{}", i.component));
        }
        if !seen_components.insert(i.component.as_str()) {
            v.push(format!("qualify-impact-duplicate:{}", i.component));
        }
    }
    if record.impacts.is_empty() {
        v.push("qualify-impacts-empty".to_owned());
    }
    // 与冻结记录/目录绑定（摘要必须指向**当前**记录）。
    let freeze_rel = format!("product/skus/{}.json", record.sku_id);
    if !root.join(&freeze_rel).is_file() {
        v.push(format!("qualify-freeze-missing:{}", record.sku_id));
    } else {
        let current = freeze_record_digest(root, &record.sku_id)?;
        if record.fact_record_digest.trim().is_empty() {
            v.push("qualify-freeze-digest-empty".to_owned());
        } else if record.fact_record_digest != current {
            v.push(format!(
                "qualify-freeze-digest-stale:{}!={}",
                short(&record.fact_record_digest),
                short(&current)
            ));
        }
        let view = read_freeze_view(&root.join(&freeze_rel))?;
        if !view.revision.is_empty()
            && !record.board_revision.is_empty()
            && view.revision != record.board_revision
        {
            v.push(format!(
                "qualify-revision-mismatch:{}!={}",
                view.revision, record.board_revision
            ));
        }
    }
    // 复验：影响映射的实时核对（撤销受影响场景，而不是重写历史）。
    if verify_impacts {
        for i in &record.impacts {
            match current_impact_digest(root, i)? {
                None => v.push(format!("qualify-impact-unverifiable:{}", i.component)),
                Some(actual) if actual != i.digest => v.push(format!(
                    "qualify-impact-stale:{}:{}:revoked={}",
                    i.component,
                    short(&i.digest),
                    i.scenarios.join("|")
                )),
                Some(_) => {}
            }
        }
    }
    Ok(v)
}

fn qualify_evidence_check(root: &Path, rel: &str, kind: &str) -> String {
    if rel.trim().is_empty() {
        return format!("qualify-{kind}-evidence-empty");
    }
    if !root.join(rel).is_file() {
        return format!("qualify-{kind}-evidence-missing:{rel}");
    }
    String::new()
}

/// 组件当前摘要（复验口径）。
///
/// 只有能**从仓库事实直接算出**的组件才参与自动复验；硬件/固件/驱动/协议
/// 的事实源不在本仓库（厂商手册、实板、外部对端），返回 `None` 并要求人工
/// 裁决——**不得**默认"未变"（蓝图 §6：复验裁剪必须依据合同影响，不能用
/// "只改几行"豁免）。
fn current_impact_digest(root: &Path, i: &ImpactBinding) -> DynResult<Option<String>> {
    match i.component.as_str() {
        "config" => {
            // 配置 = configs/ 下全部文件的排序字节摘要（可算且稳定）。
            let dir = root.join("configs");
            if !dir.is_dir() {
                return Ok(None);
            }
            let mut hasher = Sha256::new();
            let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            files.sort();
            for p in files {
                hasher.update(
                    p.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                        .as_bytes(),
                );
                if let Ok(bytes) = fs::read(&p) {
                    hasher.update(&bytes);
                }
            }
            Ok(Some(hex(&hasher.finalize())))
        }
        _ => Ok(None),
    }
}

/// 资格记录里是否把非硬件 scope 混入（供 catalog gate 复核调用）。
pub fn has_non_hardware_run(record: &QualificationRecord) -> bool {
    record
        .runs
        .iter()
        .any(|r| !HARDWARE_SCOPES.contains(&r.scope.as_str()))
}

/// 供 catalog gate 复用的资格记录读取（缺文件 → 空列表）。
pub fn load_qualification_records(root: &Path) -> DynResult<Vec<(String, QualificationRecord)>> {
    let dir = root.join("product/skus");
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().ends_with(".qualification.json"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    for p in files {
        let rec = read_json::<QualificationRecord>(&p)?;
        out.push((p.to_string_lossy().to_string(), rec));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eneros-board-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("product/skus")).unwrap();
        fs::create_dir_all(dir.join("evidence")).unwrap();
        dir
    }

    fn write_freeze(
        root: &Path,
        sku: &str,
        entries: serde_json::Value,
        missing: serde_json::Value,
    ) {
        let body = serde_json::json!({
            "schema": "eneros.board-fact-freeze.v2",
            "sku_id": sku,
            "frozen_at": "2026-09-01T00:00:00Z",
            "source_commit": "deadbeef",
            "revision": "rev-A",
            "ledger_version": 2,
            "entries": entries,
            "missing": [],
            "structured_missing": missing,
        });
        fs::write(
            root.join(format!("product/skus/{sku}.json")),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    fn entry(
        subsystem: &str,
        key: &str,
        value: &str,
        unit: &str,
        verified: bool,
    ) -> serde_json::Value {
        serde_json::json!({
            "subsystem": subsystem,
            "key": key,
            "value": value,
            "unit": unit,
            "verified": verified,
            "hardware_scope": "vendor-doc",
            "source": "TRM §1",
            "evidence": "",
            "digest": null,
        })
    }

    fn view_of(root: &Path, sku: &str) -> BoardFactFreezeV2View {
        read_freeze_view(&root.join(format!("product/skus/{sku}.json"))).unwrap()
    }

    #[test]
    fn v2_freeze_validates_and_reports_overlap() {
        let root = tmp_root("v2");
        let entries = serde_json::json!([
            entry("soc", "compatible", "acme,soc", "none", true),
            entry("cpu", "cores", "8", "count", true),
            entry("ram", "base[0]", "0x40000000", "addr", true),
            entry("ram", "size[0]", "1024", "MiB", true),
            entry("ram", "base[1]", "0x40000000", "addr", true),
            entry("ram", "size[1]", "64", "MiB", true),
            entry("timer", "frequency", "50000000", "Hz", true),
            entry("gic", "version", "gicv3", "none", true),
            entry("boot-chain", "uefi", "vendor-uefi", "none", true),
        ]);
        write_freeze(&root, "acme-board", entries, serde_json::json!([]));
        let failures =
            validate_freeze_v2(&view_of(&root, "acme-board"), "acme-board", "development");
        assert!(
            failures.iter().any(|f| f.starts_with("ram-overlap")),
            "unexpected: {failures:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn v2_freeze_product_requires_hard_gates() {
        let root = tmp_root("gate");
        let entries = serde_json::json!([
            entry("soc", "compatible", "acme,soc", "none", true),
            entry("cpu", "cores", "8", "count", true),
            entry("ram", "base[0]", "0x40000000", "addr", true),
            entry("ram", "size[0]", "1024", "MiB", true),
            entry("timer", "frequency", "50000000", "Hz", true),
            entry("gic", "version", "gicv3", "none", true),
            entry("boot-chain", "uefi", "vendor-uefi", "none", true),
        ]);
        write_freeze(
            &root,
            "acme-board",
            entries,
            serde_json::json!([{"subsystem": "storage", "note": "无介质事实"}]),
        );
        let failures = validate_freeze_v2_qualified(
            &view_of(&root, "acme-board"),
            "acme-board",
            "product",
            true,
        );
        for expected in [
            "capability-unverified:storage:C4/C5",
            "capability-unverified:trng:C10",
            "capability-unverified:rollback-counter:C12",
            "capability-unverified:bus:C8/C9",
            "capability-unverified:network-phy:C6",
        ] {
            assert!(
                failures.iter().any(|f| f.starts_with(expected)),
                "missing {expected} in {failures:?}"
            );
        }
        // 同一记录在 development 口径下可以只是 RAM/GIC 等基础族齐备 → 通过。
        let dev = validate_freeze_v2(&view_of(&root, "acme-board"), "acme-board", "development");
        assert!(dev.is_empty(), "unexpected: {dev:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn v1_freeze_is_tolerated_but_product_needs_ledger() {
        let root = tmp_root("v1");
        let body = serde_json::json!({
            "schema": "eneros.board-fact-freeze.v1",
            "sku_id": "legacy",
            "frozen_at": "2026-09-01T00:00:00Z",
            "source_commit": "deadbeef",
            "facts": {"soc": {"value": "x", "verified": true, "source": "repo"}},
            "missing": ["storage/nvme：无介质事实"],
        });
        fs::write(
            root.join("product/skus/legacy.json"),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();
        let view = view_of(&root, "legacy");
        assert!(validate_freeze_v2(&view, "legacy", "development").is_empty());
        assert_eq!(
            validate_freeze_v2(&view, "legacy", "product"),
            vec!["sku-fact-ledger-required:legacy".to_string()]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn bringup_record_rejects_qemu_scope_and_unbound_serial() {
        let root = tmp_root("bringup");
        let mut record = BringUpRecord {
            schema: BRINGUP_SCHEMA.to_owned(),
            sku_id: "acme-board".to_owned(),
            board_revision: "rev-A".to_owned(),
            serial: "SN-0001".to_owned(),
            rig_id: "rig-1".to_owned(),
            scope: "qemu".to_owned(),
            fact_record_digest: "x".repeat(64),
            artifacts: BTreeMap::from([
                ("boot_chain".to_owned(), "a".repeat(64)),
                ("root_task".to_owned(), "b".repeat(64)),
            ]),
            required_services: vec!["root-task".to_owned()],
            raw_evidence: vec!["evidence/missing.log".to_owned()],
            operators: Operators {
                device_facts_by: "alice".to_owned(),
                bringup_by: "alice".to_owned(),
                reviewed_by: "alice".to_owned(),
            },
        };
        let v = validate_bringup(&root, &record, "x.json").unwrap();
        assert!(
            v.contains(&"bringup-non-hardware-scope:qemu".to_string()),
            "{v:?}"
        );
        assert!(
            v.contains(&"bringup-reviewer-not-independent".to_string()),
            "{v:?}"
        );
        assert!(v
            .iter()
            .any(|x| x.starts_with("bringup-raw-evidence-missing")));
        assert!(v.iter().any(|x| x.starts_with("bringup-freeze-missing")));
        // 绑好序列号 + 硬件 scope + 独立审核 + 真证据 + 现行摘要 → 通过。
        record.scope = "hw-bringup".to_owned();
        record.operators.reviewed_by = "bob".to_owned();
        record.raw_evidence = vec!["evidence/b0-boot.log".to_owned()];
        fs::write(root.join("evidence/b0-boot.log"), b"boot log\n").unwrap();
        write_freeze(
            &root,
            "acme-board",
            serde_json::json!([entry("soc", "compatible", "acme,soc", "none", true)]),
            serde_json::json!([]),
        );
        record.fact_record_digest = freeze_record_digest(&root, "acme-board").unwrap();
        let v = validate_bringup(&root, &record, "x.json").unwrap();
        assert!(v.is_empty(), "unexpected: {v:?}");
        // 摘要过期（记录换了 → 摘要不匹配）→ 拒绝。
        write_freeze(
            &root,
            "acme-board",
            serde_json::json!([entry("soc", "compatible", "acme,soc2", "none", true)]),
            serde_json::json!([]),
        );
        let v = validate_bringup(&root, &record, "x.json").unwrap();
        assert!(
            v.iter()
                .any(|x| x.starts_with("bringup-freeze-digest-stale")),
            "{v:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qualification_binding_and_impact_revocation() {
        let root = tmp_root("qualbind");
        write_freeze(
            &root,
            "acme-board",
            serde_json::json!([entry("soc", "compatible", "acme,soc", "none", true)]),
            serde_json::json!([]),
        );
        fs::write(root.join("evidence/case.json"), b"{}").unwrap();
        fs::create_dir_all(root.join("configs")).unwrap();
        fs::write(
            root.join("configs/points.toml"),
            b"a = 1
",
        )
        .unwrap();
        let mut hasher = Sha256::new();
        hasher.update(b"points.toml");
        hasher.update(
            b"a = 1
",
        );
        let config_digest = hex(&hasher.finalize());
        let manifest_rel = "evidence/run-manifest.json";
        fs::write(root.join(manifest_rel), b"{}").unwrap();
        let mut record = QualificationRecord {
            schema: QUALIFICATION_SCHEMA.to_owned(),
            candidate_id: "cand-1".to_owned(),
            sku_id: "acme-board".to_owned(),
            board_revision: "rev-A".to_owned(),
            scope: "hw-hil".to_owned(),
            required_cases: vec![CaseBinding {
                case_id: "CORE-70-R11".to_owned(),
                requirement: "CORE-70-R11".to_owned(),
                evidence: "evidence/case.json".to_owned(),
            }],
            runs: vec![RunBinding {
                run_id: "run-1".to_owned(),
                scenario: "hil-full-plan".to_owned(),
                scope: "hw-hil".to_owned(),
                started_at: "2026-09-01T00:00:00Z".to_owned(),
                ended_at: "2026-09-02T00:00:00Z".to_owned(),
                manifest: manifest_rel.to_owned(),
            }],
            impacts: vec![ImpactBinding {
                component: "config".to_owned(),
                digest: config_digest.clone(),
                scenarios: vec!["hil-full-plan".to_owned()],
            }],
            fact_record_digest: freeze_record_digest(&root, "acme-board").unwrap(),
            unsupported: vec!["can: 无收发器".to_owned()],
            limitations: vec!["EMC 外部待证".to_owned()],
        };
        let baseline = validate_qualification(&root, &record, true).unwrap();
        assert!(baseline.is_empty(), "baseline violations: {baseline:?}");
        // 影响映射过期（configs/ 改了）→ 撤销受影响场景。
        fs::write(
            root.join("configs/points.toml"),
            b"a = 2
",
        )
        .unwrap();
        let v = validate_qualification(&root, &record, true).unwrap();
        assert!(
            v.iter()
                .any(|x| x.starts_with("qualify-impact-stale:config")),
            "{v:?}"
        );
        // 冻结记录换了（板卡修订/事实变化）→ 资格摘要失效。
        fs::write(
            root.join("configs/points.toml"),
            b"a = 1
",
        )
        .unwrap();
        write_freeze(
            &root,
            "acme-board",
            serde_json::json!([entry("soc", "compatible", "acme,soc2", "none", true)]),
            serde_json::json!([]),
        );
        let v = validate_qualification(&root, &record, true).unwrap();
        assert!(
            v.iter()
                .any(|x| x.starts_with("qualify-freeze-digest-stale")),
            "{v:?}"
        );
        // 人工可核对的组件（hardware/driver/protocol/firmware）不得默认"未变"。
        record.impacts.push(ImpactBinding {
            component: "driver".to_owned(),
            digest: "c".repeat(64),
            scenarios: vec!["hil-full-plan".to_owned()],
        });
        let v = validate_qualification(&root, &record, true).unwrap();
        assert!(
            v.contains(&"qualify-impact-unverifiable:driver".to_string()),
            "{v:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn qualification_rejects_empty_and_non_hardware() {
        let root = tmp_root("qualify");
        let record = QualificationRecord {
            schema: QUALIFICATION_SCHEMA.to_owned(),
            candidate_id: "cand-1".to_owned(),
            sku_id: "acme-board".to_owned(),
            board_revision: "rev-A".to_owned(),
            scope: "qemu".to_owned(),
            required_cases: vec![],
            runs: vec![],
            impacts: vec![],
            fact_record_digest: String::new(),
            unsupported: vec![],
            limitations: vec![],
        };
        let v = validate_qualification(&root, &record, false).unwrap();
        for expected in [
            "qualify-scope-not-hardware:qemu",
            "qualify-required-cases-empty",
            "qualify-runs-empty",
            "qualify-impacts-empty",
            "qualify-freeze-missing:acme-board",
        ] {
            assert!(
                v.contains(&expected.to_string()),
                "missing {expected} in {v:?}"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }
}
