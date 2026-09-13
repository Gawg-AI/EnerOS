//! 单条板级事实的模型与自校验（WP-70.12.1）。
//!
//! 与 `xtask/src/catalog.rs::BoardFact` v1 的差异（全部为**加法**）：
//! `subsystem`（能力族）、`unit`（单位，禁裸数字）、`hardware_scope`
//! （事实来源等级）、`evidence`（实证摘要）、`digest`（宣称硬件实证时的
//! 材料摘要）。v1 记录缺这些字段仍可读取（见 `ledger` 的兼容路径）。

use alloc::string::String;

use crate::capability::Subsystem;

/// 无单位事实（拓扑/标识/布尔类）的显式标记——不用空串，避免"忘了填"。
pub const UNIT_NONE: &str = "none";

/// 事实来源等级。**决定该事实能否作为硬件实证依据**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareScope {
    /// 仓库内材料（dts 占位、PlatformConfig 常量、代码路径）：可证"代码
    /// 里这么写"，**不可**证硬件如此。
    Repo,
    /// 厂商文档（数据手册/TRM/原理图/errata）：可证"文档如此"，仍需实板
    /// 复核（地址/容量可能随修订变化）。
    VendorDoc,
    /// 实板实测（bring-up 串口/FDT/仪器）：唯一可支撑产品资格的事实等级。
    Hardware,
}

impl HardwareScope {
    pub fn json_name(self) -> &'static str {
        match self {
            HardwareScope::Repo => "repo",
            HardwareScope::VendorDoc => "vendor-doc",
            HardwareScope::Hardware => "hardware",
        }
    }

    pub fn parse(name: &str) -> Option<HardwareScope> {
        match name {
            "repo" => Some(HardwareScope::Repo),
            "vendor-doc" => Some(HardwareScope::VendorDoc),
            "hardware" => Some(HardwareScope::Hardware),
            _ => None,
        }
    }

    /// 是否构成硬件实证。
    pub fn is_hardware_hard(self) -> bool {
        matches!(self, HardwareScope::Hardware)
    }
}

/// 事实自校验错误（机器可读：`reason` 即判据编号）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactError {
    /// 值为空。
    EmptyValue,
    /// 值里含 BLOCKED/未知措辞却声明 verified。
    ClaimsBlockedYetVerified,
    /// `verified=true` 但来源为空。
    VerifiedWithoutSource,
    /// 单位为空。
    EmptyUnit,
    /// 值疑似裸数字但单位缺失（`none` 只允许非数值事实）。
    NumericValueWithoutUnit,
    /// 来源等级缺失或非法（`hardware_scope` 与 `verified` 必须自洽）。
    IncoherentScope,
    /// 宣称硬件实证但没有材料摘要。
    HardwareWithoutDigest,
    /// 摘要形态非法（非 64 位小写 hex）。
    BadDigest,
    /// 宣称硬件实证但没有实测依据。
    HardwareWithoutEvidence,
}

impl FactError {
    /// 判据编号（报告/测试断言用；同时是 [`Fact`] 拒绝的可核对理由）。
    pub fn reason(self) -> &'static str {
        match self {
            FactError::EmptyValue => "fact-empty-value",
            FactError::ClaimsBlockedYetVerified => "fact-blocked-yet-verified",
            FactError::VerifiedWithoutSource => "fact-verified-without-source",
            FactError::EmptyUnit => "fact-empty-unit",
            FactError::NumericValueWithoutUnit => "fact-numeric-without-unit",
            FactError::IncoherentScope => "fact-scope-incoherent",
            FactError::HardwareWithoutDigest => "fact-hardware-without-digest",
            FactError::BadDigest => "fact-bad-digest",
            FactError::HardwareWithoutEvidence => "fact-hardware-without-evidence",
        }
    }
}

/// 一条板级事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    /// 能力族。
    pub subsystem: Subsystem,
    /// 事实键（同一族内唯一，如 `base_addr`、`capacity`、`revision`）。
    pub key: String,
    /// 事实值（字符串形态：地址用 `0x...`、容量用十进制 + 单位）。
    pub value: String,
    /// 单位（[`UNIT_NONE`] = 非数值事实）。
    pub unit: String,
    /// 是否已冻结（`false` = 记录在案但未确认；不得当作事实消费）。
    pub verified: bool,
    /// 来源等级。
    pub hardware_scope: HardwareScope,
    /// 来源（手册页码/实测命令/仓库路径）。`verified=true` 时不得为空。
    pub source: String,
    /// 实测依据（串口行/FDT 摘要/仪器记录）。
    pub evidence: String,
    /// 材料摘要（`hardware_scope=hardware` 时必填；64 位小写 hex）。
    pub digest: Option<String>,
}

impl Fact {
    /// 构造一条已验证的仓库事实（测试与迁移常用）。
    pub fn repo(subsystem: Subsystem, key: &str, value: &str, unit: &str) -> Self {
        Self {
            subsystem,
            key: String::from(key),
            value: String::from(value),
            unit: String::from(unit),
            verified: true,
            hardware_scope: HardwareScope::Repo,
            source: String::from("repo"),
            evidence: String::new(),
            digest: None,
        }
    }

    /// 构造一条未验证事实（登记在案，不构成事实）。
    pub fn unknown(subsystem: Subsystem, key: &str) -> Self {
        Self {
            subsystem,
            key: String::from(key),
            value: String::from("unknown"),
            unit: String::from(UNIT_NONE),
            verified: false,
            hardware_scope: HardwareScope::Repo,
            source: String::from("BLOCKED: 未取得"),
            evidence: String::new(),
            digest: None,
        }
    }

    /// 判据 —— 逐项自校验（顺序即报告顺序）。
    pub fn validate(&self) -> Result<(), FactError> {
        if self.value.trim().is_empty() {
            return Err(FactError::EmptyValue);
        }
        if self.unit.trim().is_empty() {
            return Err(FactError::EmptyUnit);
        }
        if self.verified {
            if self.source.trim().is_empty() {
                return Err(FactError::VerifiedWithoutSource);
            }
            // 值 / 来源 / 依据 三处任一出现未取得措辞都拒绝——"文件存在"
            // 或"来源待核"不得当硬件 verified（蓝图 §4 复核纪律）。
            if looks_blocked(&self.value)
                || looks_blocked(&self.source)
                || looks_blocked(&self.evidence)
            {
                return Err(FactError::ClaimsBlockedYetVerified);
            }
        } else if self.hardware_scope.is_hardware_hard() {
            // 未验证事实不得自称硬件实证。
            return Err(FactError::IncoherentScope);
        }
        if self.unit == UNIT_NONE && looks_numeric(&self.value) {
            return Err(FactError::NumericValueWithoutUnit);
        }
        if self.hardware_scope.is_hardware_hard() {
            if self.evidence.trim().is_empty() {
                return Err(FactError::HardwareWithoutEvidence);
            }
            match self.digest.as_deref() {
                None => return Err(FactError::HardwareWithoutDigest),
                Some(d) if !is_sha256_hex(d) => return Err(FactError::BadDigest),
                Some(_) => {}
            }
        } else if let Some(d) = self.digest.as_deref() {
            // 摘要一旦出现就必须是合法形态（防止占位假摘要）。
            if !d.is_empty() && !is_sha256_hex(d) {
                return Err(FactError::BadDigest);
            }
        }
        Ok(())
    }

    /// 是否可被当作硬件实证消费（产品资格判据）。
    pub fn is_hardware_evidence(&self) -> bool {
        self.verified && self.hardware_scope.is_hardware_hard() && self.validate().is_ok()
    }

    /// 事实标识（报告/错误定位）。
    pub fn id(&self) -> (Subsystem, &str) {
        (self.subsystem, self.key.as_str())
    }
}

/// BLOCKED/未知措辞检测——README 里的重点：**不得把"文件存在"当硬件
/// verified**。值或依据里出现未取得措辞却声明 verified 即拒绝。
fn looks_blocked(text: &str) -> bool {
    const MARKERS: [&str; 7] = [
        "blocked",
        "BLOCKED",
        "未知",
        "未取得",
        "未冻结",
        "待核",
        "pending",
    ];
    MARKERS.iter().any(|m| text.contains(m))
}

/// 裸数字检测：纯数值（含 `0x` 前缀、小数、千分位）且无单位。
fn looks_numeric(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return false;
    }
    let body = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X"));
    match body {
        Some(hex) => !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()),
        None => {
            let mut seen_digit = false;
            for c in v.chars() {
                if c.is_ascii_digit() {
                    seen_digit = true;
                } else if c == '.' {
                } else {
                    return false;
                }
            }
            seen_digit
        }
    }
}

/// 64 位小写 hex 摘要。
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hardware_fact() -> Fact {
        Fact {
            subsystem: Subsystem::Trng,
            key: "base_addr".into(),
            value: "0x2800_1000".into(),
            unit: "addr".into(),
            verified: true,
            hardware_scope: HardwareScope::Hardware,
            source: "D2000 TRM §12.4 表 12-3 + bring-up 实测".into(),
            evidence: "b3-entropy.log health=PASS samples=8".into(),
            digest: Some("a".repeat(64)),
        }
    }

    #[test]
    fn hardware_fact_requires_evidence_and_digest() {
        let mut f = hardware_fact();
        assert!(f.validate().is_ok());
        f.digest = None;
        assert_eq!(f.validate(), Err(FactError::HardwareWithoutDigest));
        f.digest = Some("z".repeat(64));
        assert_eq!(f.validate(), Err(FactError::BadDigest));
        f.digest = Some("a".repeat(64));
        f.evidence = String::new();
        assert_eq!(f.validate(), Err(FactError::HardwareWithoutEvidence));
    }

    #[test]
    fn unverified_fact_may_not_claim_hardware_scope() {
        let mut f = Fact::unknown(Subsystem::Trng, "base_addr");
        assert!(f.validate().is_ok(), "未验证事实合法（登记在案）");
        f.hardware_scope = HardwareScope::Hardware;
        assert_eq!(f.validate(), Err(FactError::IncoherentScope));
        assert!(!f.is_hardware_evidence());
    }

    #[test]
    fn blocked_wording_cannot_be_verified() {
        // 旧记录里的原句：verified=true + "BLOCKED：缺硬件与 BSP 文档"
        // 旧记录原句形态：value 带未取得措辞却宣称已验证。
        let mut g = Fact::repo(Subsystem::Ram, "map", "未知（无板级 RAM map）", UNIT_NONE);
        assert_eq!(g.validate(), Err(FactError::ClaimsBlockedYetVerified));
        // 来源行带 BLOCKED 同样拒绝（verified=true + "BLOCKED：缺硬件"）。
        g.value = "2 GiB @ 0x0".into();
        g.unit = "MiB".into();
        g.source = "BLOCKED：缺硬件与 BSP 文档".into();
        assert_eq!(g.validate(), Err(FactError::ClaimsBlockedYetVerified));
        g.source = "v2.3 手册 §4.2".into();
        assert!(g.validate().is_ok());
        // 依据行里带未取得措辞同样拒绝（防"来源待核"当已验证）。
        let mut h = Fact::repo(Subsystem::Soc, "compatible", "arm,virt", UNIT_NONE);
        h.evidence = "待核：官方手册未取得".into();
        assert_eq!(h.validate(), Err(FactError::ClaimsBlockedYetVerified));
        // verified=false 时同样的措辞是合法登记。
        g.verified = false;
        g.value = "未知（无板级 RAM map）".into();
        g.source = "BLOCKED：缺硬件".into();
        assert!(g.validate().is_ok());
    }

    #[test]
    fn numeric_value_needs_unit() {
        let mut f = Fact::repo(Subsystem::Timer, "frequency", "62500000", UNIT_NONE);
        assert_eq!(f.validate(), Err(FactError::NumericValueWithoutUnit));
        f.unit = "Hz".into();
        assert!(f.validate().is_ok());
        // 非数值事实允许 none。
        let name = Fact::repo(Subsystem::Soc, "compatible", "phytium,d2000", UNIT_NONE);
        assert!(name.validate().is_ok());
    }

    #[test]
    fn verified_needs_source_and_unit() {
        let mut f = Fact::repo(Subsystem::Cpu, "cores", "8", "count");
        f.source = String::new();
        assert_eq!(f.validate(), Err(FactError::VerifiedWithoutSource));
        f.source = "v1.0 手册".into();
        f.unit = String::new();
        assert_eq!(f.validate(), Err(FactError::EmptyUnit));
        f.unit = "count".into();
        f.value = String::new();
        assert_eq!(f.validate(), Err(FactError::EmptyValue));
    }

    #[test]
    fn sha256_shape_is_strict() {
        assert!(is_sha256_hex(&"0".repeat(64)));
        assert!(!is_sha256_hex(&"0".repeat(63)));
        assert!(
            !is_sha256_hex(&"A".repeat(64)),
            "大写不接受（摘要单源小写）"
        );
        assert!(!is_sha256_hex("dev"));
    }
}
