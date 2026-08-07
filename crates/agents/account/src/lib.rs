//! EnerOS Agent 能源账户 (workspace v0.192.0)
//!
//! 提供能源账户管理能力，包括余额查询、收支记账、
//! 资金冻结/解冻以及交易历史查询。
//!
//! # 核心功能
//!
//! - **账户开户**: 为 Agent 创建能源账户
//! - **收支记账**: Credit(收入) / Debit(支出) 双向记账
//! - **余额管理**: 同时维护 kWh 与元两种余额
//! - **资金冻结**: 支持冻结/解冻操作
//! - **交易历史**: 按时间范围与账户过滤查询

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod entry;
pub mod history;
pub mod ledger;
pub mod types;

pub use entry::{create_entry, entry_summary};
pub use history::{query_history, recent_entries};
pub use ledger::AccountStore;
pub use types::{EnergyAccount, FlowDir, FrozenAmount, LedgerEntry};

/// 账户系统错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EconError {
    /// 账户不存在
    AccountNotFound,
    /// 余额不足
    InsufficientBalance,
    /// 账户已冻结
    Frozen,
    /// 非法记账条目
    InvalidEntry,
    /// 冻结已释放（重复解冻）
    AlreadyReleased,
    /// 重复开户
    DuplicateAccount,
}

impl core::fmt::Display for EconError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AccountNotFound => write!(f, "account not found"),
            Self::InsufficientBalance => write!(f, "insufficient balance"),
            Self::Frozen => write!(f, "account is frozen"),
            Self::InvalidEntry => write!(f, "invalid ledger entry"),
            Self::AlreadyReleased => write!(f, "freeze already released"),
            Self::DuplicateAccount => write!(f, "duplicate account"),
        }
    }
}

/// 账户管理器 trait
///
/// 定义能源账户的核心操作接口。
pub trait AccountManager {
    /// 开户：为指定 owner 创建余额为零的账户
    fn open(&mut self, owner: u64) -> Result<EnergyAccount, EconError>;

    /// 查询余额：返回 (kwh, 毫元定点金额，ARC-003)
    fn balance(&self, account_id: u64) -> Result<(f64, eneros_market_types::MilliYuan), EconError>;

    /// 记账：根据方向增加或减少余额，余额不足时拒绝 Debit
    fn post(&mut self, entry: LedgerEntry) -> Result<(), EconError>;

    /// 冻结：冻结指定金额（毫元定点），返回 freeze_id
    fn freeze(
        &mut self,
        account_id: u64,
        amount: eneros_market_types::MilliYuan,
    ) -> Result<u64, EconError>;

    /// 解冻：释放已冻结的金额
    fn unfreeze(&mut self, freeze_id: u64) -> Result<(), EconError>;
}

#[cfg(all(test, feature = "loom"))]
pub mod loom_freeze;
