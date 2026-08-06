//! 模型白名单：以 SM3 哈希为键的模型登记与查询.
//!
//! 白名单条目记录模型的标识、版本、SM3 哈希、登记时间、登记人与有效期。
//! 严格模式（`strict_mode = true`）下，未登记或已过期的模型哈希将被
//! [`crate::ModelGuard`] 拒绝加载；宽松模式下白名单不参与判定（仅签名验证）。

use alloc::collections::BTreeMap;
use alloc::string::String;

/// 白名单条目（一个已登记模型的元数据）.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WhitelistEntry {
    /// 模型标识（如 "llm-7b-int4" / "solver-lp-v2"）.
    pub model_id: String,
    /// 模型版本（如 "1.3.0"）.
    pub version: String,
    /// 模型数据的 SM3 哈希（白名单键，GB/T 32905）.
    pub hash: [u8; 32],
    /// 登记时间（Unix 时间戳）.
    pub added_at: u64,
    /// 登记人标识（审计可读用途）.
    pub added_by: String,
    /// 有效期截止（Unix 时间戳；`expiry < now` 视为过期）.
    pub expiry: u64,
}

/// 模型白名单（SM3 哈希 → 条目）.
///
/// no_std 偏差：以 `BTreeMap<[u8; 32], WhitelistEntry>` 替代蓝图 HashMap，
/// 键为模型 SM3 哈希，同一哈希重复 `add` 覆盖旧条目。
pub struct ModelWhitelist {
    /// 严格模式开关：`true` 时未登记哈希拒绝加载；`false` 时白名单仅作记录，
    /// 加载判定只依赖签名验证（宽松语义由 [`crate::ModelGuard`] 落实）.
    pub strict_mode: bool,
    /// 条目存储（SM3 哈希 → 条目）.
    entries: BTreeMap<[u8; 32], WhitelistEntry>,
}

impl ModelWhitelist {
    /// 创建空白名单.
    ///
    /// `strict_mode`：严格模式开关（见字段文档）。
    pub fn new(strict_mode: bool) -> Self {
        Self {
            strict_mode,
            entries: BTreeMap::new(),
        }
    }

    /// 登记条目（以 `entry.hash` 为键插入；同哈希覆盖旧条目）.
    pub fn add(&mut self, entry: WhitelistEntry) {
        self.entries.insert(entry.hash, entry);
    }

    /// 按哈希移除条目（不存在时静默忽略）.
    pub fn remove(&mut self, hash: &[u8; 32]) {
        self.entries.remove(hash);
    }

    /// 按哈希查询是否已登记（不检查有效期；过期判定由调用方按 `expiry` 与
    /// 外部 `now` 比较）.
    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.entries.contains_key(hash)
    }

    /// 按哈希取条目引用（未登记返回 `None`）.
    pub fn get(&self, hash: &[u8; 32]) -> Option<&WhitelistEntry> {
        self.entries.get(hash)
    }

    /// 已登记条目数.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 白名单是否为空.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ============================================================
// 单元测试 WL1~WL5（测试模块内允许 std）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试条目（哈希全 0xAA，有效期 2_000_000_000）.
    fn make_entry() -> WhitelistEntry {
        WhitelistEntry {
            model_id: String::from("llm-7b-int4"),
            version: String::from("1.0.0"),
            hash: [0xAA; 32],
            added_at: 1_700_000_000,
            added_by: String::from("ops-admin"),
            expiry: 2_000_000_000,
        }
    }

    /// WL1：add 后 contains/get 命中，且读回字段与写入一致.
    #[test]
    fn wl1_add_then_contains_and_get() {
        let mut wl = ModelWhitelist::new(true);
        let entry = make_entry();
        wl.add(entry.clone());

        assert!(wl.contains(&entry.hash), "add 后 contains 必须命中");
        let got = wl.get(&entry.hash).expect("add 后 get 必须命中");
        assert_eq!(got.model_id, "llm-7b-int4");
        assert_eq!(got.version, "1.0.0");
        assert_eq!(got.hash, [0xAA; 32]);
        assert_eq!(got.added_at, 1_700_000_000);
        assert_eq!(got.added_by, "ops-admin");
        assert_eq!(got.expiry, 2_000_000_000);
        assert_eq!(wl.len(), 1);
        assert!(!wl.is_empty());
    }

    /// WL2：remove 后 contains 为 false，len 归零.
    #[test]
    fn wl2_remove_then_absent() {
        let mut wl = ModelWhitelist::new(true);
        let entry = make_entry();
        wl.add(entry.clone());
        wl.remove(&entry.hash);

        assert!(!wl.contains(&entry.hash), "remove 后 contains 必须为 false");
        assert!(wl.get(&entry.hash).is_none(), "remove 后 get 必须为 None");
        assert_eq!(wl.len(), 0);
        assert!(wl.is_empty());
    }

    /// WL3：strict_mode=true 且白名单为空时，任意哈希 contains 均为 false
    /// （严格模式语义：空白名单 = 拒绝一切未登记模型）.
    #[test]
    fn wl3_strict_empty_whitelist_denies_all() {
        let wl = ModelWhitelist::new(true);
        assert!(wl.strict_mode, "strict_mode 标志必须为 true");
        assert!(!wl.contains(&[0x00; 32]), "空白名单不得命中全零哈希");
        assert!(!wl.contains(&[0xAA; 32]), "空白名单不得命中任意哈希");
        assert!(!wl.contains(&[0xFF; 32]), "空白名单不得命中任意哈希");
    }

    /// WL4：过期语义——`entry.expiry < now` 判定成立（直接比较字段，
    /// 验证白名单条目携带的有效期可支撑 ModelGuard 过期拒绝）.
    #[test]
    fn wl4_expiry_semantics() {
        let entry = make_entry(); // expiry = 2_000_000_000
        let now_before: u64 = 1_900_000_000;
        let now_after: u64 = 2_000_000_001;
        let now_equal: u64 = 2_000_000_000;

        assert!(entry.expiry >= now_before, "now 未达有效期不得判过期");
        assert!(entry.expiry < now_after, "now 超过有效期必须判过期");
        assert!(
            entry.expiry >= now_equal,
            "expiry == now 不判过期（判定为严格小于）"
        );
    }

    /// WL5：strict_mode=false 宽松语义——标志可读写，白名单本身不拦截
    /// （宽松模式下 contains 不参与加载判定，属 ModelGuard 行为）.
    #[test]
    fn wl5_non_strict_mode_flag() {
        let mut wl = ModelWhitelist::new(false);
        assert!(!wl.strict_mode, "strict_mode 标志必须为 false");

        // 宽松模式下未登记哈希 contains 仍为 false（contains 是事实查询，
        // 与模式无关；模式只影响 ModelGuard 是否采纳该结果）
        assert!(!wl.contains(&[0x42; 32]));

        // 标志可切换：运行期收紧为严格模式
        wl.strict_mode = true;
        assert!(wl.strict_mode, "strict_mode 标志必须可写");
    }
}
