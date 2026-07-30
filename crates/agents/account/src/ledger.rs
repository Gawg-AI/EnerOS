//! 账户存储与 AccountManager 实现

use alloc::vec::Vec;

use crate::types::{EnergyAccount, FlowDir, FrozenAmount, LedgerEntry};
use crate::{AccountManager, EconError};

/// 内存账户存储
#[derive(Debug, Default)]
pub struct AccountStore {
    /// 所有账户
    pub accounts: Vec<EnergyAccount>,
    /// 所有账本条目
    pub entries: Vec<LedgerEntry>,
    /// 所有冻结记录
    pub freezes: Vec<FrozenAmount>,
    /// 下一个账户 ID
    pub next_account_id: u64,
    /// 下一个条目 ID
    pub next_entry_id: u64,
    /// 下一个冻结 ID
    pub next_freeze_id: u64,
}

impl AccountStore {
    /// 创建空的账户存储
    pub fn new() -> Self {
        Self {
            accounts: Vec::new(),
            entries: Vec::new(),
            freezes: Vec::new(),
            next_account_id: 1,
            next_entry_id: 1,
            next_freeze_id: 1,
        }
    }

    /// 获取所有账本条目
    pub fn all_entries(&self) -> &[LedgerEntry] {
        &self.entries
    }

    fn find_account(&self, account_id: u64) -> Option<usize> {
        self.accounts
            .iter()
            .position(|a| a.account_id == account_id)
    }

    /// 计算账户已冻结但未释放的总金额
    fn frozen_total(&self, account_id: u64) -> f64 {
        self.freezes
            .iter()
            .filter(|f| f.account == account_id && !f.released)
            .map(|f| f.amount_yuan)
            .sum()
    }
}

impl AccountManager for AccountStore {
    fn open(&mut self, owner: u64) -> Result<EnergyAccount, EconError> {
        // 检查是否已存在同一 owner 的账户
        if self.accounts.iter().any(|a| a.owner == owner) {
            return Err(EconError::DuplicateAccount);
        }

        let acct = EnergyAccount {
            account_id: self.next_account_id,
            owner,
            balance_kwh: 0.0,
            balance_yuan: 0.0,
            created_at: 0,
            frozen: false,
        };
        self.next_account_id += 1;
        self.accounts.push(acct.clone());
        Ok(acct)
    }

    fn balance(&self, account_id: u64) -> Result<(f64, f64), EconError> {
        let idx = self
            .find_account(account_id)
            .ok_or(EconError::AccountNotFound)?;
        let acct = &self.accounts[idx];
        Ok((acct.balance_kwh, acct.balance_yuan))
    }

    fn post(&mut self, entry: LedgerEntry) -> Result<(), EconError> {
        let idx = self
            .find_account(entry.account)
            .ok_or(EconError::AccountNotFound)?;

        if entry.amount_kwh < 0.0 || entry.amount_yuan < 0.0 {
            return Err(EconError::InvalidEntry);
        }

        if self.accounts[idx].frozen {
            return Err(EconError::Frozen);
        }

        // 预计算冻结总额，避免后续借用冲突
        let frozen_total = self.frozen_total(entry.account);

        let acct = &mut self.accounts[idx];
        match entry.direction {
            FlowDir::Credit => {
                acct.balance_kwh += entry.amount_kwh;
                acct.balance_yuan += entry.amount_yuan;
            }
            FlowDir::Debit => {
                let available_yuan = acct.balance_yuan - frozen_total;
                if acct.balance_kwh < entry.amount_kwh || available_yuan < entry.amount_yuan {
                    return Err(EconError::InsufficientBalance);
                }
                acct.balance_kwh -= entry.amount_kwh;
                acct.balance_yuan -= entry.amount_yuan;
            }
        }

        self.entries.push(entry);
        Ok(())
    }

    fn freeze(&mut self, account_id: u64, amount: f64) -> Result<u64, EconError> {
        let idx = self
            .find_account(account_id)
            .ok_or(EconError::AccountNotFound)?;

        if amount <= 0.0 {
            return Err(EconError::InvalidEntry);
        }

        let acct = &self.accounts[idx];
        if acct.frozen {
            return Err(EconError::Frozen);
        }

        // 可用余额 = 总余额 - 已冻结金额
        let frozen_total = self.frozen_total(account_id);
        let available = acct.balance_yuan - frozen_total;
        if available < amount {
            return Err(EconError::InsufficientBalance);
        }

        let freeze_id = self.next_freeze_id;
        self.next_freeze_id += 1;

        self.freezes.push(FrozenAmount {
            freeze_id,
            account: account_id,
            amount_yuan: amount,
            at: 0,
            released: false,
        });

        Ok(freeze_id)
    }

    fn unfreeze(&mut self, freeze_id: u64) -> Result<(), EconError> {
        let idx = self
            .freezes
            .iter()
            .position(|f| f.freeze_id == freeze_id)
            .ok_or(EconError::AccountNotFound)?;

        if self.freezes[idx].released {
            return Err(EconError::AlreadyReleased);
        }

        self.freezes[idx].released = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::create_entry;

    #[test]
    fn test_open_account() {
        let mut store = AccountStore::new();
        let acct = store.open(100).unwrap();
        assert_eq!(acct.account_id, 1);
        assert_eq!(acct.owner, 100);
        assert_eq!(acct.balance_kwh, 0.0);
        assert_eq!(acct.balance_yuan, 0.0);
        assert!(!acct.frozen);
    }

    #[test]
    fn test_duplicate_account() {
        let mut store = AccountStore::new();
        store.open(100).unwrap();
        let result = store.open(100);
        assert_eq!(result, Err(EconError::DuplicateAccount));
    }

    #[test]
    fn test_credit() {
        let mut store = AccountStore::new();
        store.open(100).unwrap();

        let entry = create_entry(1, FlowDir::Credit, 50.0, 25.0, Some(1), "sell");
        store.post(entry).unwrap();

        let (kwh, yuan) = store.balance(1).unwrap();
        assert_eq!(kwh, 50.0);
        assert_eq!(yuan, 25.0);
    }

    #[test]
    fn test_debit() {
        let mut store = AccountStore::new();
        store.open(100).unwrap();

        let credit = create_entry(1, FlowDir::Credit, 100.0, 50.0, None, "deposit");
        store.post(credit).unwrap();

        let debit = create_entry(1, FlowDir::Debit, 30.0, 15.0, None, "buy");
        store.post(debit).unwrap();

        let (kwh, yuan) = store.balance(1).unwrap();
        assert_eq!(kwh, 70.0);
        assert_eq!(yuan, 35.0);
    }

    #[test]
    fn test_insufficient_balance() {
        let mut store = AccountStore::new();
        store.open(100).unwrap();

        let credit = create_entry(1, FlowDir::Credit, 10.0, 5.0, None, "small");
        store.post(credit).unwrap();

        let debit = create_entry(1, FlowDir::Debit, 20.0, 10.0, None, "too much");
        let result = store.post(debit);
        assert_eq!(result, Err(EconError::InsufficientBalance));
    }

    #[test]
    fn test_freeze_and_unfreeze() {
        let mut store = AccountStore::new();
        store.open(100).unwrap();

        let credit = create_entry(1, FlowDir::Credit, 100.0, 100.0, None, "deposit");
        store.post(credit).unwrap();

        let freeze_id = store.freeze(1, 60.0).unwrap();
        assert_eq!(freeze_id, 1);

        // 冻结后，可用余额不足以支持大额支出
        let debit = create_entry(1, FlowDir::Debit, 0.0, 50.0, None, "spend");
        let result = store.post(debit);
        assert_eq!(result, Err(EconError::InsufficientBalance));

        // 解冻后可以支出
        store.unfreeze(freeze_id).unwrap();
        let debit2 = create_entry(1, FlowDir::Debit, 0.0, 50.0, None, "spend after unfreeze");
        store.post(debit2).unwrap();

        let (_, yuan) = store.balance(1).unwrap();
        assert_eq!(yuan, 50.0);
    }

    #[test]
    fn test_double_unfreeze() {
        let mut store = AccountStore::new();
        store.open(100).unwrap();

        let credit = create_entry(1, FlowDir::Credit, 100.0, 100.0, None, "deposit");
        store.post(credit).unwrap();

        let freeze_id = store.freeze(1, 30.0).unwrap();
        store.unfreeze(freeze_id).unwrap();

        let result = store.unfreeze(freeze_id);
        assert_eq!(result, Err(EconError::AlreadyReleased));
    }

    #[test]
    fn test_balance_account_not_found() {
        let store = AccountStore::new();
        let result = store.balance(999);
        assert_eq!(result, Err(EconError::AccountNotFound));
    }

    #[test]
    fn test_post_to_frozen_account() {
        let mut store = AccountStore::new();
        store.open(100).unwrap();
        store.accounts[0].frozen = true;

        let entry = create_entry(1, FlowDir::Credit, 10.0, 5.0, None, "test");
        let result = store.post(entry);
        assert_eq!(result, Err(EconError::Frozen));
    }
}
