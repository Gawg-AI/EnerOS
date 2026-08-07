//! v0.247.0（C-247）：账户冻结/清算并发模型验证（loom，host 侧）。
//!
//! 生产 `AccountStore` 为 `&mut self` 单线程 API；并发访问由上层（如
//! autonomous 的 TradeOrchestrator）经 Mutex 串行化。本模型验证经
//! `loom::sync::Mutex` 串行化后的**冻结不变量**：
//! - 任意时刻已冻结总额 ≤ 账户余额（超冻在串行语义下不可能）；
//! - 并发 freeze 请求的总额不超可用余额——余额不足时正确拒绝；
//! - freeze_id 全局唯一（计数单调）。
//!
//! loom 复刻与生产 `AccountStore::freeze` 的判定逻辑同构（可用额 =
//! 余额 - 已冻结；不足拒绝）。

extern crate std;

use loom::sync::Mutex;
use std::vec::Vec;

/// 与生产 `AccountStore::freeze` 同构的 loom 复刻（经 Mutex 串行化）。
struct LoomAccount {
    balance: i64, // 毫元
    frozen: i64,  // 已冻结毫元
    next_freeze_id: u64,
}

impl LoomAccount {
    fn new(balance: i64) -> Self {
        Self {
            balance,
            frozen: 0,
            next_freeze_id: 1,
        }
    }

    /// 冻结 amount 毫元；返回 freeze_id 或 None（余额不足/非法）。
    fn freeze(&mut self, amount: i64) -> Option<u64> {
        if amount <= 0 {
            return None;
        }
        let available = self.balance - self.frozen;
        if available < amount {
            return None;
        }
        self.frozen += amount;
        let id = self.next_freeze_id;
        self.next_freeze_id += 1;
        Some(id)
    }

    /// 解冻：释放冻结金额。
    fn unfreeze(&mut self, amount: i64) {
        self.frozen -= amount;
        debug_assert!(self.frozen >= 0);
    }
}

/// 并发冻结：两线程各冻结 30_000 毫元（余额 100_000）——
/// 冻结总额必须 ≤ 100_000，且两个 freeze_id 不同。
#[test]
fn vrf_006_concurrent_freeze_no_overfreeze() {
    loom::model(|| {
        let acct = loom::sync::Arc::new(Mutex::new(LoomAccount::new(100_000)));

        let mut handles = Vec::new();
        for _ in 0..2 {
            let a = acct.clone();
            handles.push(loom::thread::spawn(move || {
                let mut guard = a.lock().unwrap();
                guard.freeze(30_000)
            }));
        }
        let ids: Vec<Option<u64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let acct = acct.lock().unwrap();
        assert!(acct.frozen <= 100_000, "冻结总额不得超余额");
        assert!(ids[0] != ids[1], "freeze_id 必须唯一");
        // 两次 30k 冻结都成功（可用 100k ≥ 60k）。
        assert!(ids[0].is_some() && ids[1].is_some());
    });
}

/// 并发超冻拒绝：两线程各冻结 60_000 毫元（余额 100_000）——
/// 至少一个被拒绝（100k < 60k+60k），且冻结总额 ≤ 100_000。
#[test]
fn vrf_006_concurrent_freeze_overfreeze_rejected() {
    loom::model(|| {
        let acct = loom::sync::Arc::new(Mutex::new(LoomAccount::new(100_000)));

        let mut handles = Vec::new();
        for _ in 0..2 {
            let a = acct.clone();
            handles.push(loom::thread::spawn(move || {
                let mut guard = a.lock().unwrap();
                guard.freeze(60_000)
            }));
        }
        let ids: Vec<Option<u64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let acct = acct.lock().unwrap();
        assert!(acct.frozen <= 100_000, "超冻必须被拒绝");
        assert!(
            ids.iter().filter(|id| id.is_some()).count() <= 1,
            "60k+60k>100k：至多一笔冻结成功"
        );
    });
}

/// 冻结-解冻往返：冻结后解冻，余额恢复可用；总额最终为 0。
#[test]
fn vrf_006_freeze_unfreeze_roundtrip() {
    loom::model(|| {
        let acct = loom::sync::Arc::new(Mutex::new(LoomAccount::new(50_000)));
        let a1 = acct.clone();
        let h1 = loom::thread::spawn(move || {
            let mut g = a1.lock().unwrap();
            let id = g.freeze(20_000).expect("freeze ok");
            (id, 20_000)
        });
        let (id, amount) = h1.join().unwrap();
        {
            let mut g = acct.lock().unwrap();
            g.unfreeze(amount);
            assert_eq!(g.frozen, 0, "解冻后冻结总额归零");
            let _ = id;
        }
    });
}
