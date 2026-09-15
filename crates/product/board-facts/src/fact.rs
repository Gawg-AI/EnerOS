//! 单条板级事实的模型与自校验（WP-70.12.1）。
//!
//! 与 `xtask/src/catalog.rs::BoardFact` v1 的差异（全部为**加法**）：
//! `subsystem`（能力族）、`unit`（单位，禁裸数字）、`hardware_scope`
//! （事实来源等级）、`evidence`（实证摘要）、`digest`（宣称硬件实证时的
//! 材料摘要）。v1 记录缺这些字段仍可读取（见 `ledger` 的兼容路径）。

use alloc::string::String;
use alloc::vec::Vec;

use crate::capability::Subsystem;

/// 无单位事实（拓扑/标识/布尔类）的显式标记——不用空串，避免"忘了填"。
pub const UNIT_NONE: &str = "none";

/// 硬件实证依据串的最小长度（审计 S7 F8：单字符证据不算依据）。
pub const MIN_HARDWARE_EVIDENCE_LEN: usize = 8;

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
    /// 事实键形态非法（空/含分隔符或控制字符）——摘要域边界依赖键的唯一性。
    BadKey,
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
            FactError::BadKey => "fact-bad-key",
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
        // 键形态（审计 S7 F6）：键进入摘要的字段标识——含 `=`/`:`/换行/控制
        // 字符会让字段边界产生歧义（两个不同记录撞同一摘要）。空键同理。
        if self.key.trim().is_empty()
            || self
                .key
                .chars()
                .any(|c| c == '=' || c == ':' || c == '\n' || c == '\r' || c.is_control())
        {
            return Err(FactError::BadKey);
        }
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
            // 审计（S7 F8）：证据串必须**可核对**——单字符/空白填充不算依据
            // （否则任意 64hex 摘要 + 一个字母即可宣称硬件实证）。
            if self.evidence.trim().len() < MIN_HARDWARE_EVIDENCE_LEN {
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
///
/// 匹配口径（70.14 复查第三轮收敛，三条独立规则）：
/// 1. **ASCII 标记按词边界**（`blocked`/`pending`）——裸子串 `contains` 会把
///    `unblocked after reset`、`impending degradation` 判成"未取得"（真实值
///    误伤）；CJK 标记（`未知`/`未取得`/`未冻结`/`待核`）无词边界概念，按子串。
/// 2. **占位词按词序列**：单句占位词按整词、多词短语按连续子序列（见
///    [`PLACEHOLDERS`]）；`na` 单列为**整串相等**才命中（见
///    [`PLACEHOLDERS_WHOLE_ONLY`]，避免 `Na-ion battery` 被误伤）。
/// 3. **全宽折叠 + 整串压缩相等**：全角字母折成 ASCII（`ｕｎｋｎｏｗｎ`）；
///    去掉分隔符后的**整串**与占位词压缩形相等（`un-known`、`notavailable`）
///    ——只做整串压缩比较，故 `unknown_ok` 之类带后缀的真实词不受影响。
fn looks_blocked(text: &str) -> bool {
    /// ASCII 标记：按词边界（避免 `unblocked`/`impending` 这类子串误伤）。
    const ASCII_MARKERS: [&str; 2] = ["blocked", "pending"];
    /// CJK 标记：无词边界概念，按子串。
    const CJK_MARKERS: [&str; 4] = ["未知", "未取得", "未冻结", "待核"];

    let folded = fold_width(text).trim().to_ascii_lowercase();
    let tokens = words(&folded);
    if ASCII_MARKERS.iter().any(|m| tokens.contains(m)) {
        return true;
    }
    if CJK_MARKERS.iter().any(|m| folded.contains(m)) {
        return true;
    }
    if PLACEHOLDERS
        .iter()
        .any(|p| contains_tokens(&tokens, &words(&fold_width(p).to_ascii_lowercase())))
    {
        return true;
    }
    if PLACEHOLDERS_WHOLE_ONLY
        .iter()
        .any(|p| tokens == words(&fold_width(p).to_ascii_lowercase()))
    {
        return true;
    }
    // 分隔符变体：整串压缩后与占位词压缩形相等。
    let compact: String = folded.chars().filter(|c| c.is_alphanumeric()).collect();
    PLACEHOLDERS
        .iter()
        .chain(PLACEHOLDERS_WHOLE_ONLY.iter())
        .any(|p| {
            let pc: String = fold_width(p)
                .to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            !pc.is_empty() && compact == pc
        })
}

/// 全宽 ASCII（U+FF01..U+FF5E）与表意空格折成半宽。
///
/// 旧实现只做 `to_ascii_lowercase()`——全角字母是**单 token**、不等于
/// `unknown`，于是 `value="ｕｎｋｎｏｗｎ"` + verified 通过逐条判据。
fn fold_width(text: &str) -> String {
    text.chars()
        .map(|c| {
            let cp = c as u32;
            if (0xFF01..=0xFF5E).contains(&cp) {
                char::from_u32(cp - 0xFEE0).unwrap_or(c)
            } else if cp == 0x3000 {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// 占位词表（`unknown`/`tbd` 等）。
///
/// 口径（两轮复查收敛）：**单句占位词**（`unknown`、`tbd`、`na`、`none`…）必须
/// 整串相等才算命中——按词边界匹配会在 `nand flash @ 0x10000000` 这类真实值上
/// 误伤；**多词占位短语**（`not available`、`to be determined`…）按连续词序列
/// 匹配，因为嵌入更长文本的多词短语同样是"未取得"（`status: not available`
/// 若只按整串相等就会静默放行，实测如此）。
///
/// `unknown` 的**词性子串**（`unknowable-margin`、`unknown_ok`）不算占位：
/// 前者是完整词 `unknowable`，后者是词 `unknown_ok`——两者都不等于 `unknown`
/// 这个整串占位值。
const PLACEHOLDERS: [&str; 21] = [
    "unknown",
    "tbd",
    "tba",
    "n/a",
    "placeholder",
    "todo",
    "none",
    "not available",
    "not applicable",
    "to be determined",
    "to be decided",
    "see datasheet",
    "待定",
    "待补",
    "未确定",
    "不适用",
    "未提供",
    "无数据",
    "未测量",
    "未测得",
    "见手册",
];

/// **仅在整串相等时**才算占位的词。
///
/// `na` 是 `n/a` 的无标点写法，但它同时是钠（Na）的化学式——储能域里
/// `Na-ion battery` / `Na-S` 是**真实技术名词**。旧实现按"任一 token 相等"
/// 匹配 ⇒ `value="Na-ion battery"` 被判 `ClaimsBlockedYetVerified`（真实值
/// 误伤）。`n/a` 本身仍按词序列匹配（含标点写法无歧义）。
const PLACEHOLDERS_WHOLE_ONLY: [&str; 1] = ["na"];

/// 小写文本 → 字母数字词序列（非字母数字字符即分隔符）。
fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// `phrase` 的词序列是否是 `tokens` 的一段**连续**子序列。
///
/// 用连续子序列而非整串相等，才能拦住 `status: not available`；用词序列而非
/// 裸子串匹配，才不会让 `not ready, but available` 这类真实值误伤（`not` 与
/// `available` 不连续）。单句占位词的词序列长度 1 ⇒ 退化为"按词边界整词匹配"。
fn contains_tokens(tokens: &[&str], phrase: &[&str]) -> bool {
    !phrase.is_empty()
        && phrase.len() <= tokens.len()
        && tokens.windows(phrase.len()).any(|w| w == phrase)
}

/// 裸数字检测（含 `0x` 前缀、小数、千分位/下划线分隔、正负号）——这些
/// 形态都是"量"，没有单位就不成立。
///
/// 审计（70.12 S7）修正：旧实现把 `_`/`,`/`+`/`-` 当"非数字"字符 → `"1_000"`
/// `"1,000"` `"-5"` `"0x2800_1000"` 一律漏判。
fn looks_numeric(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() {
        return false;
    }
    let v = v
        .strip_prefix('+')
        .or_else(|| v.strip_prefix('-'))
        .unwrap_or(v);
    let body = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X"));
    match body {
        Some(hex) => {
            let cleaned: String = hex.chars().filter(|c| *c != '_').collect();
            !cleaned.is_empty() && cleaned.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => {
            let mut seen_digit = false;
            for c in v.chars() {
                if c.is_ascii_digit() {
                    seen_digit = true;
                } else if c == '.' || c == '_' || c == ',' {
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

    /// 复查补漏（2026-09-13）红测：占位词不得冒充硬件实证。
    ///
    /// 旧实现 `looks_blocked` 只查 `blocked/未知/未取得/待核/pending`，
    /// 而 `Fact::unknown()` 的规范占位值恰是 `"unknown"` ⇒ 13 个产品硬门族
    /// 全填 `value="unknown"` 即可通过 `capability_coverage(Product)`
    /// （实测 `violations == []`），与蓝图 D6-02「不得把占位值提升为硬件已核」
    /// 直接矛盾。
    #[test]
    fn placeholder_words_cannot_be_verified() {
        for v in [
            "unknown",
            "UNKNOWN",
            "Unknown",
            "tbd",
            "TBD",
            "tba",
            "n/a",
            "N/A",
            "na",
            "placeholder",
            "TODO",
            "not available",
            "未知值 unknown",
        ] {
            let mut f = hardware_fact();
            f.value = v.to_string();
            assert_eq!(
                f.validate(),
                Err(FactError::ClaimsBlockedYetVerified),
                "占位值 {v:?} 不得声明 verified"
            );
            assert!(!f.is_hardware_evidence(), "占位值 {v:?} 不是硬件实证");
        }
        // 反向：措辞里含 `unknown` 子串的**正常值**不受影响（不误伤）。
        for v in ["unknowable-margin", "0x2800_1000", "8", "AAVMF"] {
            let mut f = hardware_fact();
            f.value = v.to_string();
            assert!(f.validate().is_ok(), "正常值 {v:?} 不应被占位判据误伤");
        }
        // `verified=false` 的登记仍合法（未取得本来就该这么记）。
        let mut u = Fact::unknown(Subsystem::Trng, "base_addr");
        assert!(u.validate().is_ok());
        u.value = "tbd".into();
        assert!(u.validate().is_ok(), "未验证事实里的占位词是合法登记");
    }

    /// 复查补漏（2026-09-13 第二轮）红测：**多词占位短语嵌在更长文本里**。
    ///
    /// 第一轮的实现只做「整串相等 ∨ 单词相等」，多词短语（`not available`）
    /// 因此只在**整串**或**独立单词**的形态下命中——实测
    /// `value="status: not available"` / `"data not available (待补)"` 判
    /// `Ok(())` 且 `is_hardware_evidence() == true`：占位短语加个前缀即可绕开
    /// 「未取得不得当已验证」。与 `Fact::unknown()` 的规范占位值同罪。
    #[test]
    fn multiword_placeholders_cannot_hide_inside_text() {
        for v in [
            "status: not available",
            "value not available",
            "not available yet",
            "data not available (待补)",
            "status: to be determined",
            "not applicable to this revision",
            "see datasheet for address",
        ] {
            let mut f = hardware_fact();
            f.value = v.to_string();
            assert_eq!(
                f.validate(),
                Err(FactError::ClaimsBlockedYetVerified),
                "嵌入文本的多词占位短语同样不得声明 verified：{v:?}"
            );
            assert!(!f.is_hardware_evidence(), "占位值 {v:?} 不是硬件实证");
        }
        // 来源/依据两处同样不得出现占位短语（判据是三处任一）。
        let mut f = hardware_fact();
        f.source = "vendor TRM: not available".into();
        assert_eq!(f.validate(), Err(FactError::ClaimsBlockedYetVerified));
        let mut f = hardware_fact();
        f.evidence = "bring-up: not applicable".into();
        assert_eq!(f.validate(), Err(FactError::ClaimsBlockedYetVerified));
        // 反向：占位**词**出现但不连续时不得误伤（词序列匹配，不是子串匹配）。
        for v in [
            "not ready, but available",
            "available: yes (not measured)",
            "datasheet rev 2 @ 0x1000",
        ] {
            let mut f = hardware_fact();
            f.value = v.to_string();
            assert!(f.validate().is_ok(), "真实值 {v:?} 不应被多词占位判据误伤");
        }
    }

    /// 70.14 复查第三轮红测：占位词的**剩余绕过形态**与**真实值误伤**。
    ///
    /// 绕过（旧实现全部通过 validate + `is_hardware_evidence()`）：
    /// `none`（旧表缺）、`不适用`/`未提供`/`无数据`（中文形态缺失）、
    /// `un-known`/`notavailable`（分词后拆碎）、全角 `ｕｎｋｎｏｗｎ`
    /// （`to_ascii_lowercase` 不折全宽）。
    /// 误伤（旧实现把真实值判成"未取得"）：`Na-ion battery`（`na` 按 token
    /// 命中钠的化学式）、`unblocked after reset` 与 `impending degradation`
    /// （`blocked`/`pending` 裸子串无词边界）。
    #[test]
    fn placeholder_bypasses_and_real_value_false_positives() {
        // 绕过形态：必须拒绝（verified 与占位语义互斥）。
        for v in [
            "none",
            "NONE",
            "不适用",
            "未提供",
            "无数据",
            "un-known",
            "notavailable",
            "\u{ff55}\u{ff4e}\u{ff4b}\u{ff4e}\u{ff4f}\u{ff57}\u{ff4e}",
        ] {
            let mut f = hardware_fact();
            f.value = v.to_string();
            assert_eq!(
                f.validate(),
                Err(FactError::ClaimsBlockedYetVerified),
                "占位形态 {v:?} 不得声明 verified"
            );
        }
        // 真实值：不得误伤。
        for v in [
            "Na-ion battery",
            "Na-S 电池",
            "nand flash @ 0x10000000",
            "unknowable-margin", // 单 token `unknowable` ≠ `unknown`
            "0x2800_1000",       // 裸数字带分隔符（曾漏判的形态）
        ] {
            let mut f = hardware_fact();
            f.value = v.to_string();
            assert!(
                f.validate().is_ok(),
                "真实值 {v:?} 不得被判占位/未取得（误伤）"
            );
        }
        // 来源/依据面的 ASCII 标记按词边界：`unblocked`/`impending` 是真实措辞。
        for src in [
            "unblocked after reset (rev 2)",
            "impending degradation margin",
        ] {
            let mut f = hardware_fact();
            f.source = src.to_string();
            assert!(f.validate().is_ok(), "来源 {src:?} 不得被裸子串误伤");
        }
        // 词边界不放松真判据：`blocked` / `pending` 作为词仍拒绝。
        for src in ["blocked: awaiting vendor TRM", "pending bring-up"] {
            let mut f = hardware_fact();
            f.source = src.to_string();
            assert_eq!(
                f.validate(),
                Err(FactError::ClaimsBlockedYetVerified),
                "带未取得措辞的来源 {src:?} 必须拒绝"
            );
        }
    }

    /// 70.14 复查红测：占位判据的两条边界的**分工**。
    ///
    /// - **整串**压缩相等（`un known` → `unknown`）⇒ 拒绝：分隔符不构成
    ///   "这是真实值"的理由。
    /// - **单 token** 与占位词相等（`unknown_ok` 含独立词 `unknown`）⇒ 拒绝：
    ///   独立出现的 `unknown` 就是"未取得"陈述（前缀/后缀不改变语义）。
    /// - **单 token 是更长真实词**（`unknowable`、`unknownable-margin`）⇒ 放行：
    ///   不是占位陈述。
    #[test]
    fn compact_form_matching_is_whole_string_only() {
        let mut f = hardware_fact();
        f.value = "un known".into();
        assert_eq!(
            f.validate(),
            Err(FactError::ClaimsBlockedYetVerified),
            "分隔符拆分的整串占位必须拒绝"
        );
        f.value = "unknown_ok".into();
        assert_eq!(
            f.validate(),
            Err(FactError::ClaimsBlockedYetVerified),
            "独立词 unknown 出现即未取得陈述（后缀不豁免）"
        );
        f.value = "unknown-ok".into();
        assert_eq!(
            f.validate(),
            Err(FactError::ClaimsBlockedYetVerified),
            "同上（分隔符形态）"
        );
        f.value = "unknowable-ok".into();
        assert!(
            f.validate().is_ok(),
            "单 token 是更长真实词（unknowable）⇒ 不是占位陈述"
        );
    }
}
