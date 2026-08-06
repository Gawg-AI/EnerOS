//! 账本条目辅助函数

use alloc::string::String;

use crate::types::{FlowDir, LedgerEntry, MilliYuan};

/// 创建账本条目（entry_id 由调用方或 Store 分配，此处设为 0）
///
/// ARC-003：`yuan` 为元（f64）→ 内部按舍入规范转毫元定点。
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
        amount_yuan: MilliYuan::from_yuan_f64(yuan),
        ref_deal: deal,
        at: 0,
        memo: String::from(memo),
    }
}

/// 统计条目列表中的总 Credit 与总 Debit（按 yuan 计）
///
/// 返回 (total_credit_yuan, total_debit_yuan)
pub fn entry_summary(entries: &[LedgerEntry]) -> (MilliYuan, MilliYuan) {
    let mut credit = MilliYuan(0);
    let mut debit = MilliYuan(0);
    for e in entries {
        match e.direction {
            FlowDir::Credit => {
                if let Some(v) = credit.checked_add(e.amount_yuan) {
                    credit = v;
                }
            }
            FlowDir::Debit => {
                if let Some(v) = debit.checked_add(e.amount_yuan) {
                    debit = v;
                }
            }
        }
    }
    (credit, debit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MilliYuan;

    #[test]
    fn test_create_entry() {
        let entry = create_entry(1, FlowDir::Credit, 50.0, 25.0, Some(42), "sell energy");
        assert_eq!(entry.account, 1);
        assert_eq!(entry.direction, FlowDir::Credit);
        assert_eq!(entry.amount_kwh, 50.0);
        assert_eq!(entry.amount_yuan, MilliYuan(25_000), "25 元 → 25000 毫元");
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
        assert_eq!(credit, MilliYuan::ZERO);
        assert_eq!(debit, MilliYuan::ZERO);
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
        assert_eq!(credit, MilliYuan(60_000), "50+10 元 = 60 元");
        assert_eq!(debit, MilliYuan(20_000), "15+5 元 = 20 元");
    }
}
