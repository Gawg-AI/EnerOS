//! 账户存储与 AccountManager 实现

use alloc::vec::Vec;

use crate::types::{MilliYuan, EnergyAccount, FlowDir, FrozenAmount, LedgerEntry};
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
    fn frozen_total(&self, account_id: u64) -> MilliYuan {
        let mut total = MilliYuan(0);
        for f in self.freezes.iter().filter(|f| f.account == account_id && !f.released) {
            if let Some(v) = total.checked_add(f.amount) {
                total = v;
            }
        }
        total
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
            balance_yuan: MilliYuan::ZERO,
            created_at: 0,
            frozen: false,
        };
        self.next_account_id += 1;
        self.accounts.push(acct.clone());
        Ok(acct)
    }

    fn balance(&self, account_id: u64) -> Result<(f64, MilliYuan), EconError> {
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

        if entry.amount_kwh < 0.0 || entry.amount_yuan.0 < 0 {
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
                acct.balance_yuan = acct
                    .balance_yuan
                    .checked_add(entry.amount_yuan)
                    .ok_or(EconError::InvalidEntry)?;
            }
            FlowDir::Debit => {
                let available_yuan = acct
                    .balance_yuan
                    .checked_sub(frozen_total)
                    .ok_or(EconError::InsufficientBalance)?;
                if acct.balance_kwh < entry.amount_kwh || available_yuan < entry.amount_yuan {
                    return Err(EconError::InsufficientBalance);
                }
                acct.balance_kwh -= entry.amount_kwh;
                acct.balance_yuan = acct
                    .balance_yuan
                    .checked_sub(entry.amount_yuan)
                    .ok_or(EconError::InsufficientBalance)?;
            }
        }

        self.entries.push(entry);
        Ok(())
    }

    fn freeze(&mut self, account_id: u64, amount: MilliYuan) -> Result<u64, EconError> {
        let idx = self
            .find_account(account_id)
            .ok_or(EconError::AccountNotFound)?;

        if amount.0 <= 0 {
            return Err(EconError::InvalidEntry);
        }

        let acct = &self.accounts[idx];
        if acct.frozen {
            return Err(EconError::Frozen);
        }

        // 可用余额 = 总余额 - 已冻结金额
        let frozen_total = self.frozen_total(account_id);
        let available = acct
            .balance_yuan
            .checked_sub(frozen_total)
            .ok_or(EconError::InsufficientBalance)?;
        if available < amount {
            return Err(EconError::InsufficientBalance);
        }

        let freeze_id = self.next_freeze_id;
        self.next_freeze_id += 1;

        self.freezes.push(FrozenAmount {
            freeze_id,
            account: account_id,
            amount,
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
    use crate::types::MilliYuan;
    use crate::entry::create_entry;

    #[test]
    fn test_open_account() {
        let mut store = AccountStore::new();
        let acct = store.open(100).unwrap();
        assert_eq!(acct.account_id, 1);
        assert_eq!(acct.owner, 100);
        assert_eq!(acct.balance_kwh, 0.0);
        assert_eq!(acct.balance_yuan, MilliYuan::ZERO);
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
        assert_eq!(yuan, MilliYuan(25_000), "25 元 = 25000 毫元");
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
        assert_eq!(yuan, MilliYuan(35_000), "35 元 = 35000 毫元");
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

        let freeze_id = store.freeze(1, MilliYuan(60_000)).unwrap();
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
        assert_eq!(yuan, MilliYuan(50_000), "100-50 = 50 元");
    }

    #[test]
    fn test_double_unfreeze() {
        let mut store = AccountStore::new();
        store.open(100).unwrap();

        let credit = create_entry(1, FlowDir::Credit, 100.0, 100.0, None, "deposit");
        store.post(credit).unwrap();

        let freeze_id = store.freeze(1, MilliYuan(30_000)).unwrap();
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
    /// v0.245.0（C-245）：长期累计精度——100 万次 0.1 kWh 入账，
    /// 余额漂移相对误差必须 < 1e-10（f64 累加的经典漂移检查；金额侧
    /// 全程 MilliYuan 定点无漂移）。
    ///
    /// 电量侧保留 f64 的说明：电量来自仪表输入（物理量本身为浮点），
    /// 定点化会引入 0.001 kWh 粒度量化误差；f64 累加 100 万次的绝对
    /// 漂移 ~1.3e-6 kWh（相对 1e-11），远小于仪表精度（0.01 kWh 级）。
    #[test]
    fn test_long_term_kwh_drift_below_threshold() {
        let mut store = AccountStore::new();
        store.open(100).unwrap();
        // 模拟长期交易：100 万笔 0.1 kWh 入账（0.1 是二进制不可精确表示
        // 的值，f64 累加最易暴露漂移）。
        for i in 0..1_000_000u64 {
            let entry = create_entry(1, FlowDir::Credit, 0.1, 0.01, Some(i), "drift");
            store.post(entry).unwrap();
        }
        let (_kwh, yuan) = store.balance(1).unwrap();
        // 金额侧：0.01 元 × 100 万 = 10_000 元 = 10_000_000 毫元，精确无漂移。
        assert_eq!(yuan, MilliYuan(10_000_000), "定点金额长期累计必须精确");
        // 电量侧：f64 累加允许极小漂移，相对误差必须 < 1e-10。
        let (kwh, _) = store.balance(1).unwrap();
        let expected = 100_000.0f64;
        let rel_drift = (kwh - expected).abs() / expected;
        assert!(
            rel_drift < 1e-10,
            "100 万次 0.1kWh 入账后相对漂移 {rel_drift} 超阈值 1e-10"
        );
    }
}

