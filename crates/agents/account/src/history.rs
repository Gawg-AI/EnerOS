//! 交易历史查询

use alloc::vec::Vec;

use crate::types::LedgerEntry;

/// 按账户和时间范围查询交易历史
///
/// 返回 account 匹配且 `start <= at <= end` 的条目引用列表。
pub fn query_history(
    entries: &[LedgerEntry],
    account: u64,
    start: u64,
    end: u64,
) -> Vec<&LedgerEntry> {
    entries
        .iter()
        .filter(|e| e.account == account && e.at >= start && e.at <= end)
        .collect()
}

/// 获取指定账户最近 N 条交易记录
///
/// 按条目在列表中的顺序取最后 N 条（假设列表按时间递增排列）。
pub fn recent_entries(entries: &[LedgerEntry], account: u64, n: usize) -> Vec<&LedgerEntry> {
    let filtered: Vec<&LedgerEntry> = entries.iter().filter(|e| e.account == account).collect();
    let len = filtered.len();
    if n >= len {
        filtered
    } else {
        filtered[len - n..].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::create_entry;
    use crate::types::FlowDir;

    fn make_entries() -> Vec<LedgerEntry> {
        let mut entries = Vec::new();
        for i in 0..10u64 {
            let mut e = create_entry(1, FlowDir::Credit, 10.0, 5.0, None, "test");
            e.entry_id = i + 1;
            e.at = 1000 + i * 100;
            entries.push(e);
        }
        // 添加另一个账户的条目
        let mut e2 = create_entry(2, FlowDir::Debit, 5.0, 2.5, None, "other");
        e2.entry_id = 11;
        e2.at = 1500;
        entries.push(e2);
        entries
    }

    #[test]
    fn test_query_history_full_range() {
        let entries = make_entries();
        let result = query_history(&entries, 1, 0, 9999);
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn test_query_history_partial_range() {
        let entries = make_entries();
        // at 范围: 1000, 1100, 1200, ..., 1900
        let result = query_history(&entries, 1, 1200, 1500);
        assert_eq!(result.len(), 4); // 1200, 1300, 1400, 1500
    }

    #[test]
    fn test_query_history_filters_account() {
        let entries = make_entries();
        let result = query_history(&entries, 2, 0, 9999);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].account, 2);
    }

    #[test]
    fn test_query_history_no_match() {
        let entries = make_entries();
        let result = query_history(&entries, 99, 0, 9999);
        assert!(result.is_empty());
    }

    #[test]
    fn test_recent_entries() {
        let entries = make_entries();
        let result = recent_entries(&entries, 1, 3);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].entry_id, 8);
        assert_eq!(result[1].entry_id, 9);
        assert_eq!(result[2].entry_id, 10);
    }

    #[test]
    fn test_recent_entries_more_than_available() {
        let entries = make_entries();
        let result = recent_entries(&entries, 2, 100);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_recent_entries_empty() {
        let entries = make_entries();
        let result = recent_entries(&entries, 99, 5);
        assert!(result.is_empty());
    }
}
