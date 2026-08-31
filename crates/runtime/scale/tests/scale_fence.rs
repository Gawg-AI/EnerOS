//! WP-7.2 exit gate 证据：双主/旧 fencing 零设备写（蓝图 61 §12 §3.7）。
//!
//! 场景：leader A（term 2）持有租约并写设备；分区后旧 leader B 复活
//! （更低 term/token）尝试写 → 设备边 FenceGuard 拒绝，写入账本零记录；
//! 新 leader C（term 3 更高）接管后正常写。

#![allow(clippy::disallowed_macros)]
use eneros_scale::{FenceGuard, FenceToken, QuorumError};

/// 模拟设备写账本（所有「真实设备写」的唯一记录通道）。
#[derive(Default)]
struct DeviceWriteLedger {
    writes: Vec<(u32, u64, u64, u64)>, // (writer, term, token, ms)
}

impl DeviceWriteLedger {
    fn record(&mut self, writer: u32, term: u64, token: u64, ms: u64) {
        self.writes.push((writer, term, token, ms));
    }
}

#[test]
fn stale_leader_zero_device_writes() {
    let mut guard = FenceGuard::new();
    let mut ledger = DeviceWriteLedger::default();
    let mut token = FenceToken(100);
    let now = 1_000u64;

    // 1) leader A（term 2, token 101）写：允许。
    let t = token.bump();
    assert_eq!(guard.check_write(7, 2, t, now), Ok(()));
    ledger.record(7, 2, t.0, now);
    assert_eq!(ledger.writes.len(), 1);

    // 2) 旧 leader B 复活（term 1，token 伪高 999）：拒。
    match guard.check_write(8, 1, FenceToken(999), now) {
        Err(QuorumError::StaleTerm) => {}
        other => panic!("旧 term 必须 StarkTerm 拒绝: {other:?}"),
    }
    assert_eq!(guard.rejected_writes, 1);
    assert_eq!(ledger.writes.len(), 1, "旧 leader 零设备写");

    // 3) 同 term 旧 token（A 的重放 token 101）：拒。
    match guard.check_write(7, 2, FenceToken(101), now) {
        Err(QuorumError::FenceRejected) => {}
        other => panic!("同 term 旧 token 必须拒: {other:?}"),
    }
    assert_eq!(ledger.writes.len(), 1);

    // 4) 分区期间 A 的租约过期后仍尝试（token 102, term 2, now 超租约）：拒。
    match guard.check_write(7, 2, token.bump(), now + 31_000) {
        Err(QuorumError::LeaseExpired) => {}
        other => panic!("过期租约必须拒: {other:?}"),
    }
    assert_eq!(ledger.writes.len(), 1);

    // 5) 新 leader C（term 3, token 新高）：接管写。
    let t = token.bump();
    assert_eq!(guard.check_write(9, 3, t, now + 31_000), Ok(()));
    ledger.record(9, 3, t.0, now + 31_000);
    assert_eq!(ledger.writes.len(), 2);
    // C 之后 A 再来（term 2）→ 拒（term 沉降不可逆）。
    assert_eq!(
        guard.check_write(7, 2, FenceToken(5000), now + 32_000),
        Err(QuorumError::StaleTerm)
    );
    assert_eq!(ledger.writes.len(), 2, "双主场景零旧写入");
    assert_eq!(guard.rejected_writes, 4);
    assert!(guard.accepted_writes >= 2);
}

#[test]
fn fence_guard_is_monotonic_across_restarts() {
    // 重启场景：token 从持久化值继续（单调）。
    let mut guard = FenceGuard::new();
    let mut token = FenceToken(7);
    // 第一次写 term 5。
    assert_eq!(guard.check_write(1, 5, token.bump(), 0), Ok(()));
    // 模拟重启后用同一 token 回放 → 拒。
    assert_eq!(
        guard.check_write(1, 5, FenceToken(8), 0),
        Err(QuorumError::FenceRejected),
        "重启后旧 token 不得重放"
    );
}
