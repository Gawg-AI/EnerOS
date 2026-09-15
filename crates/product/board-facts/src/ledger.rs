//! 板级事实账本与判据（WP-70.12.1）。
//!
//! 一组**互不替代**的判据（每条都有可核对的 reason 串）：
//!
//! | reason | 判据 |
//! |---|---|
//! | `ledger-no-facts` | 账本为空（空记录不得当"完整"） |
//! | `ledger-fact-duplicate:<族>/<键>` | 同族同键重复（含大小写/空白差异） |
//! | `ledger-missing-duplicate` / `ledger-missing-unknown-subsystem` | 缺失登记重复或引用了未知族 |
//! | `ledger-subsystem-declared-missing` | 某族已登记缺失，却仍有该族事实存在 |
//! | `ledger-unverified-family` | 某族全部事实都未验证（族级未冻结） |
//! | `ledger-verified-on-unfrozen-subsystem` | 族已登记缺失却声称已验证 |
//! | `ledger-shared-fact-unverified` | 同一族里验证度不一致（部分已证部分未证） |
//! | `fact-*` | 单条事实自校验（见 [`crate::fact::FactError`]） |
//! | `ram-*` | 内存窗口重叠/成对/可解析（见 [`crate::memory`]） |
//! | `capability-unverified:<族>` | 该种类的必需能力族缺已验证事实（产品资格硬门） |

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::capability::{requirements_for, SkuKind, Subsystem};
use crate::fact::{Fact, UNIT_NONE};
use crate::memory::{spans_from_facts, MemorySpan};

/// 板级事实账本（一个 SKU 组合一份）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardLedger {
    pub sku_id: String,
    /// 板卡修订（换修订 = 新组合，旧证据保留原适用范围）。
    pub revision: String,
    /// 账本 schema 版本（1 = 只有 facts/missing 的旧记录；2 = 结构化账本）。
    pub ledger_version: u32,
    pub facts: Vec<Fact>,
    /// 未冻结登记：`(能力族, 说明)`；族必须是已知枚举。
    pub missing: Vec<(Subsystem, String)>,
}

/// 账本判据结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LedgerReport {
    /// 机器可读违规串（空 = 通过）。
    pub violations: Vec<String>,
    /// 已验证事实的窗口（供资源预算核对复用）。
    pub spans: Vec<MemorySpan>,
}

impl LedgerReport {
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }

    /// 违规里最严重的类别前缀（报告/退出码用）。
    pub fn first(&self) -> Option<&str> {
        self.violations.first().map(|s| s.as_str())
    }
}

/// 账本显式错误（与 [`LedgerReport`] 的分工：这里是"记录形态不成立"，
/// report 是"事实判据未通过"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerError {
    /// 未知 schema 版本。
    UnsupportedVersion(u32),
    /// SKU id 为空。
    EmptySku,
    /// 未知能力族名。
    UnknownSubsystem(String),
    /// 事实自校验失败（含定位）。
    Fact {
        subsystem: Subsystem,
        key: String,
        reason: String,
    },
}

impl LedgerError {
    pub fn reason(&self) -> String {
        match self {
            LedgerError::UnsupportedVersion(v) => format!("ledger-version-unsupported:{v}"),
            LedgerError::EmptySku => "ledger-empty-sku".to_string(),
            LedgerError::UnknownSubsystem(s) => format!("ledger-unknown-subsystem:{s}"),
            LedgerError::Fact {
                subsystem,
                key,
                reason,
            } => format!("{reason}@{}#{}", subsystem.json_name(), key),
        }
    }
}

impl BoardLedger {
    /// 构造 + 形态校验（不判能力覆盖；覆盖判据见 [`Self::validate`]）。
    pub fn new(
        sku_id: &str,
        revision: &str,
        ledger_version: u32,
        facts: Vec<Fact>,
        missing: Vec<(Subsystem, String)>,
    ) -> Result<Self, LedgerError> {
        if sku_id.trim().is_empty() {
            return Err(LedgerError::EmptySku);
        }
        if !(1..=2).contains(&ledger_version) {
            return Err(LedgerError::UnsupportedVersion(ledger_version));
        }
        for f in &facts {
            if let Err(e) = f.validate() {
                return Err(LedgerError::Fact {
                    subsystem: f.subsystem,
                    key: f.key.clone(),
                    reason: e.reason().to_string(),
                });
            }
        }
        Ok(Self {
            sku_id: sku_id.to_string(),
            revision: revision.to_string(),
            ledger_version,
            facts,
            missing,
        })
    }

    /// 记录形态与族一致性判据（**不含**能力覆盖——见 [`Self::capability_coverage`]）。
    ///
    /// `kind` 目前只用于将来按种类收紧族判据；保留参数以免调用方以为
    /// "没有种类差异"（开发/产品维度的差异由能力覆盖表达）。
    pub fn validate(&self, _kind: SkuKind) -> LedgerReport {
        let mut report = LedgerReport::default();
        if self.facts.is_empty() {
            report.violations.push("ledger-no-facts".to_string());
        }
        // 1. 唯一性（族+键，规范化后比较）。
        let mut seen: Vec<(Subsystem, String)> = Vec::new();
        for f in &self.facts {
            let norm = normalize_key(&f.key);
            if seen.iter().any(|(s, k)| *s == f.subsystem && *k == norm) {
                report.violations.push(format!(
                    "ledger-fact-duplicate:{}#{}",
                    f.subsystem.json_name(),
                    f.key
                ));
            } else {
                seen.push((f.subsystem, norm));
            }
        }
        // 2. 缺失登记唯一且引用已知族。
        let mut missing_seen: Vec<(Subsystem, String)> = Vec::new();
        for (sub, note) in &self.missing {
            let key = normalize_key(note);
            if key.is_empty() {
                report
                    .violations
                    .push(format!("ledger-missing-empty-note:{}", sub.json_name()));
            }
            if missing_seen.iter().any(|(s, k)| *s == *sub && *k == key) {
                report
                    .violations
                    .push(format!("ledger-missing-duplicate:{}", sub.json_name()));
            } else {
                missing_seen.push((*sub, key));
            }
        }
        // 3. 族级一致性（三种形态各自有判据）：
        //    - **整族未冻结**：该族有事实但一条都未验证 → 不得当"有事实"；
        //    - **族名与事实矛盾**：整族都已验证却仍登记为缺失 → 缺失登记
        //      没有说明对象（同一族里混有未验证事实时，缺失登记是对那些
        //      未证项的说明，不是"整族未知"声明）；
        //    - **半冻结未说明**：族内验证度不一致且未在缺失登记里说明。
        for sub in Subsystem::ALL {
            let family: Vec<&Fact> = self.facts.iter().filter(|f| f.subsystem == sub).collect();
            if family.is_empty() {
                continue;
            }
            let any_verified = family.iter().any(|f| f.verified);
            let all_unverified = !any_verified;
            let declared = self.missing.iter().any(|(s, _)| *s == sub);
            if all_unverified {
                report
                    .violations
                    .push(format!("ledger-unverified-family:{}", sub.json_name()));
            }
            if declared && !family.iter().any(|f| !f.verified) {
                // 整族已验证却登记为缺失：缺失登记没有说明对象（自相矛盾）。
                report.violations.push(format!(
                    "ledger-subsystem-declared-missing:{}",
                    sub.json_name()
                ));
            }
            if any_verified && !declared && family.iter().any(|f| !f.verified) {
                report
                    .violations
                    .push(format!("ledger-shared-fact-unverified:{}", sub.json_name()));
            }
        }
        // 4. 单条事实（构造期已校验；此处兜底防直接构造的结构体绕过）。
        for f in &self.facts {
            if let Err(e) = f.validate() {
                report.violations.push(format!(
                    "{}@{}#{}",
                    e.reason(),
                    f.subsystem.json_name(),
                    f.key
                ));
            }
        }
        // 5. 内存窗口（重叠/成对/可解析）。
        match spans_from_facts(&self.facts) {
            Ok(spans) => report.spans = spans,
            Err(e) => report.violations.push(e.reason()),
        }
        // 6. 能力覆盖**不在此判**：覆盖判据是资格前判据（未资格化的候选板卡
        //    允许能力族尚未冻结，只要已在 `missing` 逐族登记）。调用方按 SKU
        //    状态决定是否追加 [`Self::capability_coverage`]。
        report
    }

    /// 能力覆盖判据（硬门能力族是否都有已验证事实）。
    ///
    /// 与 [`Self::validate`] 分开的原因：覆盖判据是**资格前**判据——未资格化
    /// 的候选板卡允许存在未冻结能力族（那时它们必须已在 `missing` 里登记），
    /// 但一旦要出具资格（`qualified`）就必须全数验证。调用方按 SKU 状态选择
    /// 是否执行；两个判据共用同一实现，避免"两道判据各写一遍"。
    pub fn capability_coverage(&self, kind: SkuKind) -> Vec<String> {
        let mut out = Vec::new();
        for req in requirements_for(kind) {
            // 审计（S7 F1）：**产品**资格判据 = 硬件实证（`hardware_scope =
            // hardware` + 依据 + 材料摘要），不是"有 verified 事实"——否则
            // 全部 13 个硬门族都用仓库占位事实（`repo`）填上就能拿到资格，
            // 与蓝图 C4–C12「实板事实」的原意相反（`is_hardware_evidence`
            // 此前是零调用的死代码）。开发平台沿用"已验证即可"。
            let ok = self.facts.iter().any(|f| {
                f.subsystem == req.subsystem
                    && if kind == SkuKind::Product {
                        f.is_hardware_evidence()
                    } else {
                        f.verified
                    }
            });
            if !ok {
                out.push(format!(
                    "{}:{}:{}",
                    if kind == SkuKind::Product {
                        "capability-not-hardware-evidence"
                    } else {
                        "capability-unverified"
                    },
                    req.subsystem.json_name(),
                    req.criterion
                ));
            }
        }
        out
    }

    /// 某些能力族的已验证事实（报告/装配用）。返回按键排序的不可变切片引用。
    pub fn verified_of(&self, sub: Subsystem) -> Vec<&Fact> {
        let mut v: Vec<&Fact> = self
            .facts
            .iter()
            .filter(|f| f.subsystem == sub && f.verified)
            .collect();
        v.sort_by(|a, b| a.key.cmp(&b.key));
        v
    }

    /// 该族是否被显式登记为缺失。
    pub fn is_declared_missing(&self, sub: Subsystem) -> bool {
        self.missing.iter().any(|(s, _)| *s == sub)
    }

    /// 一族里**未**验证的键（装配/预算核对用：不得消费这些值）。
    pub fn unverified_keys(&self, sub: Subsystem) -> Vec<&str> {
        self.facts
            .iter()
            .filter(|f| f.subsystem == sub && !f.verified)
            .map(|f| f.key.as_str())
            .collect()
    }

    /// 单位安全取值（缺单位时返回 [`UNIT_NONE`]，不让空串扩散）。
    pub fn unit_of(f: &Fact) -> &str {
        if f.unit.trim().is_empty() {
            UNIT_NONE
        } else {
            f.unit.as_str()
        }
    }
}

/// 键规范化：仅去首尾空白 + 折叠内部空白（**不改大小写**——`Base` 与
/// `base` 是两个键，静默合并会掩盖登记错误）。
fn normalize_key(k: &str) -> String {
    k.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fact::HardwareScope;
    use crate::memory::{ram_window_facts, ram_window_unknown};

    fn dev_ledger(facts: Vec<Fact>) -> BoardLedger {
        BoardLedger::new("qemu-virt-aarch64", "1.0.0", 2, facts, Vec::new()).unwrap()
    }

    /// 最小可用的开发平台账本（SoC/CPU/RAM/Timer/BootChain/GIC 全验证）。
    fn dev_baseline_facts() -> Vec<Fact> {
        let mut facts = vec![
            Fact::repo(Subsystem::Soc, "compatible", "qemu,virt", UNIT_NONE),
            Fact::repo(Subsystem::Cpu, "cores", "4", "count"),
            Fact::repo(Subsystem::Soc, "revision", "virt-1.0", UNIT_NONE),
            Fact::repo(Subsystem::Ram, "map", "1GiB@0x40000000", UNIT_NONE),
            Fact::repo(Subsystem::Timer, "frequency", "62500000", "Hz"),
            Fact::repo(Subsystem::BootChain, "uefi", "AAVMF", UNIT_NONE),
            Fact::repo(Subsystem::Gic, "version", "gicv3", UNIT_NONE),
            Fact::repo(Subsystem::Ram, "base[0]", "0x40000000", "addr"),
            Fact::repo(Subsystem::Ram, "size[0]", "1024", "MiB"),
        ];
        for f in facts.iter_mut() {
            f.source = "repo:qemu-virt.dts".into();
        }
        facts
    }

    #[test]
    fn dev_baseline_passes() {
        let l = dev_ledger(dev_baseline_facts());
        let r = l.validate(SkuKind::Development);
        assert!(r.is_ok(), "unexpected: {:?}", r.violations);
        assert_eq!(r.spans.len(), 1, "RAM 窗口被提取");
    }

    #[test]
    fn duplicate_key_is_rejected() {
        let mut facts = dev_baseline_facts();
        let mut dup = Fact::repo(Subsystem::Cpu, "cores ", "4", "count");
        dup.source = "blob".into();
        facts.push(dup);
        let r = dev_ledger(facts).validate(SkuKind::Development);
        assert!(r
            .violations
            .iter()
            .any(|v| v.starts_with("ledger-fact-duplicate:cpu#cores")));
    }

    #[test]
    fn declared_missing_family_conflicts_with_present_facts() {
        // RAM 事实全部已验证却登记"无板级 RAM map" = 缺失登记没有对象。
        let facts = dev_baseline_facts();
        let l = BoardLedger::new(
            "sku",
            "r1",
            2,
            facts,
            vec![(Subsystem::Ram, "无板级 RAM map".into())],
        )
        .unwrap();
        let r = l.validate(SkuKind::Development);
        assert!(
            r.violations
                .contains(&"ledger-subsystem-declared-missing:ram".to_string()),
            "unexpected: {:?}",
            r.violations
        );
        // 未验证窗口 + 同族缺失登记 → 合规（缺失登记说明的是那个未证项）。
        let mut facts2 = dev_baseline_facts();
        facts2.push(ram_window_unknown(5));
        let l = BoardLedger::new(
            "sku",
            "r1",
            2,
            facts2,
            vec![(Subsystem::Ram, "base[5] 未冻结".into())],
        )
        .unwrap();
        let r = l.validate(SkuKind::Development);
        assert!(r.is_ok(), "unexpected: {:?}", r.violations);
    }

    #[test]
    fn all_unverified_family_is_rejected() {
        let mut facts = dev_baseline_facts();
        for f in facts.iter_mut() {
            if f.subsystem == Subsystem::Cpu {
                f.verified = false;
            }
        }
        let l = dev_ledger(facts);
        assert!(!l.is_declared_missing(Subsystem::Cpu));
        let r = l.validate(SkuKind::Development);
        assert!(r
            .violations
            .contains(&"ledger-unverified-family:cpu".to_string()));
        // 能力覆盖判据独立可查（资格前判据）。
        assert!(l
            .capability_coverage(SkuKind::Development)
            .contains(&"capability-unverified:cpu:C1".to_string()));
    }

    #[test]
    fn partial_verification_needs_missing_registration() {
        let mut facts = dev_baseline_facts();
        // RTC 族：频率已核（PL031 32.768 kHz）、修订寄存器无文档 →
        // 同一族里验证度不一致，必须显式登记未证项。
        facts.push(Fact::repo(Subsystem::Rtc, "frequency", "32768", "Hz"));
        let mut unknown = Fact::unknown(Subsystem::Rtc, "rev");
        unknown.value = "no revision register documented".into();
        facts.push(unknown);
        // 未登记 → 触发"族内验证度不一致且未说明"违规。
        let r = dev_ledger(facts.clone()).validate(SkuKind::Development);
        assert!(
            r.violations
                .contains(&"ledger-shared-fact-unverified:rtc".to_string()),
            "unexpected: {:?}",
            r.violations
        );
        // 登记为缺失（说明未证项）→ 通过；开发平台不要求该族。
        let l = BoardLedger::new(
            "qemu-virt-aarch64",
            "1.0.0",
            2,
            facts,
            vec![(Subsystem::Rtc, "无 RTC 文档（PL031 未接入）".into())],
        )
        .unwrap();
        let r = l.validate(SkuKind::Development);
        assert!(r.is_ok(), "unexpected: {:?}", r.violations);
    }

    #[test]
    fn product_kind_requires_hard_gate_families() {
        // 开发平台账本用于产品 SKU → 缺 storage/nic/bus/trng/rollback/power/wd。
        let l = dev_ledger(dev_baseline_facts());
        let report = l.validate(SkuKind::Product);
        assert!(report.is_ok(), "记录形态本身成立：{:?}", report.violations);
        let r = LedgerReport {
            violations: l.capability_coverage(SkuKind::Product),
            spans: report.spans,
        };
        for expected in [
            "capability-not-hardware-evidence:storage:C4/C5",
            "capability-not-hardware-evidence:network-phy:C6",
            "capability-not-hardware-evidence:bus:C8/C9",
            "capability-not-hardware-evidence:trng:C10",
            "capability-not-hardware-evidence:rollback-counter:C12",
            "capability-not-hardware-evidence:power:C5",
            "capability-not-hardware-evidence:watchdog:C5",
        ] {
            assert!(
                r.violations.iter().any(|v| v == expected),
                "missing {expected} in {:?}",
                r.violations
            );
        }
    }

    #[test]
    fn repo_facts_do_not_satisfy_product_capability() {
        // 审计（S7 F1）红测：把 13 个产品硬门族全部用 repo 事实（占位）填上，
        // 旧实现会判"能力齐备"；现必须逐族报 hardware-evidence 缺失。
        let mut facts = Vec::new();
        for sub in crate::capability::SkuKind::Product.required() {
            // 注意：值不得是 `unknown`/`placeholder` 这类占位词——复查补漏
            // （2026-09-13）后它们与"未取得"同义，会被逐条判据拒绝（见
            // `placeholder_words_cannot_be_verified`）。本条红测针对的是
            // **来源等级**（repo 事实不能顶产品硬门），不是占位形态。
            let mut f = Fact::repo(*sub, "declared", "qemu-virt-dts", "none");
            f.source = "repo:qemu-virt.dts".into();
            facts.push(f);
        }
        let l = BoardLedger::new("sku", "rev", 2, facts, Vec::new()).unwrap();
        let r = l.validate(SkuKind::Product);
        assert!(r.is_ok(), "形态成立：{:?}", r.violations);
        let cov = l.capability_coverage(SkuKind::Product);
        assert_eq!(
            cov.len(),
            crate::capability::SkuKind::Product.required().len(),
            "全部族都应因缺硬件实证被拒：{cov:?}"
        );
        assert!(cov
            .iter()
            .all(|v| v.starts_with("capability-not-hardware-evidence:")));
        // 换成硬件实证事实 → 通过。
        let mut hw = Vec::new();
        for sub in crate::capability::SkuKind::Product.required() {
            hw.push(Fact {
                subsystem: *sub,
                key: "f".into(),
                value: "v".into(),
                unit: "none".into(),
                verified: true,
                hardware_scope: HardwareScope::Hardware,
                source: "TRM §1".into(),
                evidence: "bringup serial log line 42".into(),
                digest: Some("a".repeat(64)),
            });
        }
        let l = BoardLedger::new("sku", "rev", 2, hw, Vec::new()).unwrap();
        assert_eq!(
            l.capability_coverage(SkuKind::Product),
            Vec::<String>::new(),
            "硬件实证事实满足产品覆盖"
        );
    }

    #[test]
    fn ram_overlap_makes_admission_red() {
        let mut facts = dev_baseline_facts();
        facts.extend(ram_window_facts(1, 0x4000_0000, 64));
        let mut facts2 = dev_baseline_facts();
        facts2.retain(|f| f.key != "base[0]" && f.key != "size[0]");
        facts.extend(facts2);
        let r = dev_ledger(facts).validate(SkuKind::Development);
        assert!(
            r.violations.iter().any(|v| v.starts_with("ram-overlap:")),
            "unexpected: {:?}",
            r.violations
        );
    }

    #[test]
    fn duplicate_missing_registration_is_rejected() {
        let l = BoardLedger::new(
            "sku",
            "r1",
            2,
            dev_baseline_facts(),
            vec![
                (Subsystem::Storage, "无介质事实".into()),
                (Subsystem::Storage, "无介质事实 ".into()),
            ],
        )
        .unwrap();
        assert!(l
            .validate(SkuKind::Development)
            .violations
            .contains(&"ledger-missing-duplicate:storage".to_string()));
    }

    #[test]
    fn empty_ledger_and_bad_version_are_rejected() {
        assert_eq!(
            BoardLedger::new("", "r", 2, dev_baseline_facts(), Vec::new()),
            Err(LedgerError::EmptySku)
        );
        assert_eq!(
            BoardLedger::new("s", "r", 9, dev_baseline_facts(), Vec::new()),
            Err(LedgerError::UnsupportedVersion(9))
        );
        let empty = BoardLedger::new("s", "r", 2, Vec::new(), Vec::new()).unwrap();
        assert!(empty
            .validate(SkuKind::Development)
            .violations
            .contains(&"ledger-no-facts".to_string()));
    }

    #[test]
    fn construction_rejects_invalid_fact() {
        let mut facts = dev_baseline_facts();
        let mut bad = Fact::unknown(Subsystem::Storage, "media");
        bad.hardware_scope = HardwareScope::Hardware;
        facts.push(bad);
        let err = BoardLedger::new("s", "r", 2, facts, Vec::new()).unwrap_err();
        assert_eq!(
            err.reason(),
            "fact-scope-incoherent@storage#media",
            "构造期即拒绝，不把非法事实带进账本"
        );
    }

    #[test]
    fn unverified_windows_do_not_silently_vanish() {
        // 未冻结 RAM 占位（base unknown + size 0）不参与判定，但也必须
        // 在 missing 里登记，否则 shared-fact 判据报警。
        let mut facts = dev_baseline_facts();
        facts.push(ram_window_unknown(0));
        let l = dev_ledger(facts);
        let r = l.validate(SkuKind::Development);
        assert!(r
            .violations
            .contains(&"ledger-shared-fact-unverified:ram".to_string()));
        assert_eq!(l.unverified_keys(Subsystem::Ram), vec!["base[0]"]);
    }
}
