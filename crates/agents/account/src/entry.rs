//! 账本条目辅助函数

use alloc::string::String;

use crate::types::{FlowDir, LedgerEntry};

/// 创建账本条目（entry_id 由调用方或 Store 分配，此处设为 0）
pub fn create_entry(
    account: u64,
    dir: FlowDir,
    kwh: f64,
    yuan: f64,
    deal: Option<u64>,
    memo: &str,
) -> LedgerEntry {
    LedgerEntry {
        entry_id: 0,
        account,
        direction: dir,
        amount_kwh: kwh,
        amount_yuan: yuan,
        ref_deal: deal,
        at: 0,
        memo: String::from(memo),
    }
}

/// 统计条目列表中的总 Credit 与总 Debit（按 yuan 计）
///
/// 返回 (total_credit_yuan, total_debit_yuan)
pub fn entry_summary(entries: &[LedgerEntry]) -> (f64, f64) {
    let mut credit = 0.0;
    let mut debit = 0.0;
    for e in entries {
        match e.direction {
            FlowDir::Credit => credit += e.amount_yuan,
            FlowDir::Debit => debit += e.amount_yuan,
        }
    }
    (credit, debit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_entry() {
        let entry = create_entry(1, FlowDir::Credit, 50.0, 25.0, Some(42), "sell energy");
        assert_eq!(entry.account, 1);
        assert_eq!(entry.direction, FlowDir::Credit);
        assert_eq!(entry.amount_kwh, 50.0);
        assert_eq!(entry.amount_yuan, 25.0);
        assert_eq!(entry.ref_deal, Some(42));
        assert_eq!(entry.memo, "sell energy");
        assert_eq!(entry.entry_id, 0);
    }

    #[test]
    fn test_create_entry_no_deal() {
        let entry = create_entry(2, FlowDir::Debit, 10.0, 5.0, None, "buy");
        assert_eq!(entry.ref_deal, None);
        assert_eq!(entry.direction, FlowDir::Debit);
    }

    #[test]
    fn test_entry_summary_empty() {
        let (credit, debit) = entry_summary(&[]);
        assert_eq!(credit, 0.0);
        assert_eq!(debit, 0.0);
    }

    #[test]
    fn test_entry_summary_mixed() {
        let entries = vec![
            create_entry(1, FlowDir::Credit, 100.0, 50.0, None, "c1"),
            create_entry(1, FlowDir::Debit, 30.0, 15.0, None, "d1"),
            create_entry(1, FlowDir::Credit, 20.0, 10.0, None, "c2"),
            create_entry(1, FlowDir::Debit, 10.0, 5.0, None, "d2"),
        ];
        let (credit, debit) = entry_summary(&entries);
        assert_eq!(credit, 60.0);
        assert_eq!(debit, 20.0);
    }
}
