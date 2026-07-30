//! 能源账户核心数据类型

use alloc::string::String;

/// 资金流向
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowDir {
    /// 收入（余额增加）
    Credit,
    /// 支出（余额减少）
    Debit,
}

/// 能源账户
#[derive(Debug, Clone, PartialEq)]
pub struct EnergyAccount {
    /// 账户唯一标识
    pub account_id: u64,
    /// 所有者 Agent ID
    pub owner: u64,
    /// 电量余额 (kWh)
    pub balance_kwh: f64,
    /// 资金余额 (元)
    pub balance_yuan: f64,
    /// 创建时间戳
    pub created_at: u64,
    /// 是否冻结
    pub frozen: bool,
}

/// 账本条目（一笔收支记录）
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerEntry {
    /// 条目唯一标识
    pub entry_id: u64,
    /// 所属账户 ID
    pub account: u64,
    /// 资金方向
    pub direction: FlowDir,
    /// 电量 (kWh)
    pub amount_kwh: f64,
    /// 金额 (元)
    pub amount_yuan: f64,
    /// 关联交易 ID
    pub ref_deal: Option<u64>,
    /// 时间戳
    pub at: u64,
    /// 备注
    pub memo: String,
}

/// 冻结记录
#[derive(Debug, Clone, PartialEq)]
pub struct FrozenAmount {
    /// 冻结唯一标识
    pub freeze_id: u64,
    /// 所属账户 ID
    pub account: u64,
    /// 冻结金额 (元)
    pub amount_yuan: f64,
    /// 冻结时间戳
    pub at: u64,
    /// 是否已释放
    pub released: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flow_dir_equality() {
        assert_eq!(FlowDir::Credit, FlowDir::Credit);
        assert_eq!(FlowDir::Debit, FlowDir::Debit);
        assert_ne!(FlowDir::Credit, FlowDir::Debit);
    }

    #[test]
    fn test_energy_account_default_values() {
        let acct = EnergyAccount {
            account_id: 1,
            owner: 100,
            balance_kwh: 0.0,
            balance_yuan: 0.0,
            created_at: 1000,
            frozen: false,
        };
        assert_eq!(acct.account_id, 1);
        assert_eq!(acct.owner, 100);
        assert_eq!(acct.balance_kwh, 0.0);
        assert_eq!(acct.balance_yuan, 0.0);
        assert!(!acct.frozen);
    }

    #[test]
    fn test_ledger_entry_creation() {
        let entry = LedgerEntry {
            entry_id: 1,
            account: 1,
            direction: FlowDir::Credit,
            amount_kwh: 50.0,
            amount_yuan: 25.0,
            ref_deal: Some(42),
            at: 2000,
            memo: String::from("sell energy"),
        };
        assert_eq!(entry.direction, FlowDir::Credit);
        assert_eq!(entry.ref_deal, Some(42));
        assert_eq!(entry.memo, "sell energy");
    }

    #[test]
    fn test_frozen_amount_creation() {
        let frozen = FrozenAmount {
            freeze_id: 1,
            account: 1,
            amount_yuan: 100.0,
            at: 3000,
            released: false,
        };
        assert_eq!(frozen.amount_yuan, 100.0);
        assert!(!frozen.released);
    }
}
