//! 54（WP-54.5）：EL0 服务 durable outbox 附接的单一实现（host 可测内核）。
//!
//! 收编原 svc-market / svc-cloud-sync / svc-observability 三份逐字同构的
//! 附接样板四件事：grant 探测 → 设备打开 → `AppendOutbox::open` 成败分流
//! → 显式 `Unavailable` 兜底与串口日志。诚实降级纪律（蓝图 41 §3）语义
//! 一字不改：任何未就绪都显式 [`MaybeOutbox::Unavailable`]，绝不伪造。
//!
//! [`MaybeOutbox`] 的 Real 臂泛型化为 `IO: PersistIo`——生产面由
//! [`crate::target_durable::attach`] 钉死为 `BlockIo<Sel4IpcClient>`；
//! host 测试以内存假 IO 覆盖全部状态（granted→Real / 未授予→Unavailable /
//! opener None→Unavailable / open 失败→Unavailable+日志）。
//! 日志经闭包注入：生产侧接 `eneros_runtime::println`（串口断言面零漂移），
//! 测试侧接收集器做逐字断言。

use alloc::vec::Vec;

use eneros_durable_state::{
    AppendOutbox, Decode, DurableOutbox, Encode, IdempotencyKey, OutboxId, PeerReceipt, Pending,
    PersistIo, StoreError, UnavailableOutbox,
};

/// 业务/运维服务 durable outbox 二态封装（WP-54.5 自 target_durable 上移：
/// Real 臂泛型化 IO 以支持 host 三态测试；语义不变）。
pub enum MaybeOutbox<M, IO: PersistIo> {
    /// 存储已授予且打开成功：真落盘 outbox。
    Real(AppendOutbox<M, IO>),
    /// 未授予/打开失败：显式 fail-closed，绝不退回内存假实现伪装成功。
    Unavailable(UnavailableOutbox<M>),
}

impl<M: Encode + Decode + Clone, IO: PersistIo> DurableOutbox<M> for MaybeOutbox<M, IO> {
    fn enqueue(&mut self, msg: &M, idempotency: IdempotencyKey) -> Result<OutboxId, StoreError> {
        match self {
            MaybeOutbox::Real(o) => o.enqueue(msg, idempotency),
            MaybeOutbox::Unavailable(u) => u.enqueue(msg, idempotency),
        }
    }
    fn mark_acked(&mut self, id: OutboxId, peer_receipt: &PeerReceipt) -> Result<(), StoreError> {
        match self {
            MaybeOutbox::Real(o) => o.mark_acked(id, peer_receipt),
            MaybeOutbox::Unavailable(u) => u.mark_acked(id, peer_receipt),
        }
    }
    fn mark_sent(&mut self, id: OutboxId, at_ms: u64) -> Result<(), StoreError> {
        match self {
            MaybeOutbox::Real(o) => o.mark_sent(id, at_ms),
            MaybeOutbox::Unavailable(u) => u.mark_sent(id, at_ms),
        }
    }
    fn recover_pending(&mut self) -> Result<Vec<Pending<M>>, StoreError> {
        match self {
            MaybeOutbox::Real(o) => o.recover_pending(),
            MaybeOutbox::Unavailable(u) => u.recover_pending(),
        }
    }
}

/// 附接单一实现（WP-54.5）：三 bin 各剩一行调用，背后全部逻辑在此。
///
/// - `granted=false`（X3 掩码位未置）→ Unavailable + 日志（opener 零探测）；
/// - opener 返回 None（EIPC 设备打不开）→ Unavailable + 日志；
/// - `AppendOutbox::open()` Err → Unavailable + 日志（fail-closed）；
/// - 全部就绪 → Real。
///
/// `log` 收完整行（含 `[服务名]` 前缀）；文案与收编前 bin 原文逐字一致。
pub fn attach_in<M, IO, F>(
    granted: bool,
    label: &str,
    opener: F,
    offset: u64,
    slot_len: usize,
    max_slots: usize,
    log: &mut dyn FnMut(&str),
) -> MaybeOutbox<M, IO>
where
    M: Encode + Decode + Clone,
    IO: PersistIo,
    F: FnOnce() -> Option<IO>,
{
    if !granted {
        log(&alloc::format!(
            "{label} storage NOT granted -> outbox Unavailable"
        ));
        return MaybeOutbox::Unavailable(UnavailableOutbox::new());
    }
    let Some(io) = opener() else {
        log(&alloc::format!(
            "{label} storage NOT granted -> outbox Unavailable"
        ));
        return MaybeOutbox::Unavailable(UnavailableOutbox::new());
    };
    let mut ob = AppendOutbox::<M, IO>::new(io, offset, slot_len, max_slots);
    match ob.open() {
        Ok(()) => {
            log(&alloc::format!(
                "{label} durable outbox attached (svc-storage)"
            ));
            MaybeOutbox::Real(ob)
        }
        Err(e) => {
            log(&alloc::format!(
                "{label} durable outbox open FAILED {e:?} —— 显式 Unavailable"
            ));
            MaybeOutbox::Unavailable(UnavailableOutbox::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use eneros_durable_state::Generation;

    use super::*;

    /// 内存假 IO：可编程首读失败（模拟设备头损坏/EIPC 半开路径）。
    struct FakeIo {
        buf: Vec<u8>,
        fail_first_read: bool,
        read_count: usize,
    }

    impl FakeIo {
        fn blank(len: usize) -> Self {
            Self {
                buf: vec![0u8; len],
                fail_first_read: false,
                read_count: 0,
            }
        }
        fn failing(len: usize) -> Self {
            Self {
                buf: vec![0u8; len],
                fail_first_read: true,
                read_count: 0,
            }
        }
    }

    impl PersistIo for FakeIo {
        fn read(&mut self, _offset: u64, buf: &mut [u8]) -> Result<(), StoreError> {
            self.read_count += 1;
            if self.fail_first_read && self.read_count == 1 {
                return Err(StoreError::Corrupt);
            }
            let n = buf.len();
            buf.copy_from_slice(&self.buf[..n]);
            Ok(())
        }
        fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), StoreError> {
            let off = offset as usize;
            self.buf[off..off + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn flush(&mut self) -> Result<(), StoreError> {
            Ok(())
        }
        fn capacity(&self) -> u64 {
            self.buf.len() as u64
        }
    }

    /// 测试消息（定长编码，21 B——同 durable-state 测试形状）。
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Msg {
        seq: u32,
    }

    impl Encode for Msg {
        fn encode(&self) -> Result<Vec<u8>, StoreError> {
            let mut out = Vec::with_capacity(4);
            out.extend_from_slice(&self.seq.to_le_bytes());
            Ok(out)
        }
    }

    impl Decode for Msg {
        fn decode(bytes: &[u8]) -> Result<(Generation, Self), StoreError> {
            if bytes.len() != 4 {
                return Err(StoreError::Corrupt);
            }
            Ok((
                Generation(0),
                Msg {
                    seq: u32::from_le_bytes(bytes[..4].try_into().unwrap()),
                },
            ))
        }
    }

    const OFF: u64 = 0;
    const SLOT_LEN: usize = 64;
    const MAX_SLOTS: usize = 4;
    const LABEL: &str = "[test-svc]";

    /// 三态之一（DoD#2）：granted 且 open 成功 → Real；空链 recover 为空。
    #[test]
    fn wp545_granted_open_ok_gives_real() {
        let mut logs: Vec<String> = Vec::new();
        {
            let mut log = |line: &str| logs.push(line.to_string());
            let out = attach_in::<Msg, FakeIo, _>(
                true,
                LABEL,
                || Some(FakeIo::blank(4096)),
                OFF,
                SLOT_LEN,
                MAX_SLOTS,
                &mut log,
            );
            assert!(matches!(out, MaybeOutbox::Real(_)));
            let mut out = out;
            assert!(matches!(out.recover_pending(), Ok(ref p) if p.is_empty()));
        }
        assert_eq!(
            logs,
            vec![format!("{LABEL} durable outbox attached (svc-storage)")]
        );
    }

    /// 三态之二：掩码位未置 → Unavailable；opener 零探测（grant 探测前置）。
    #[test]
    fn wp545_ungranted_gives_unavailable_without_probe() {
        let mut probed = false;
        let mut logs: Vec<String> = Vec::new();
        {
            let mut log = |line: &str| logs.push(line.to_string());
            let out = attach_in::<Msg, FakeIo, _>(
                false,
                LABEL,
                || {
                    probed = true;
                    Some(FakeIo::blank(4096))
                },
                OFF,
                SLOT_LEN,
                MAX_SLOTS,
                &mut log,
            );
            assert!(matches!(out, MaybeOutbox::Unavailable(_)));
        }
        assert!(!probed, "未授予不得探测设备");
        assert_eq!(
            logs,
            vec![format!("{LABEL} storage NOT granted -> outbox Unavailable")]
        );
    }

    /// 三态之三：授予但 open 失败 → Unavailable + 失败日志（fail-closed）。
    #[test]
    fn wp545_open_failure_gives_unavailable_with_log() {
        let mut logs: Vec<String> = Vec::new();
        {
            let mut log = |line: &str| logs.push(line.to_string());
            let out = attach_in::<Msg, FakeIo, _>(
                true,
                LABEL,
                || Some(FakeIo::failing(4096)),
                OFF,
                SLOT_LEN,
                MAX_SLOTS,
                &mut log,
            );
            assert!(matches!(out, MaybeOutbox::Unavailable(_)));
        }
        assert_eq!(logs.len(), 1);
        assert!(
            logs[0].starts_with(&format!("{LABEL} durable outbox open FAILED ")),
            "实际日志：{}",
            logs[0]
        );
        assert!(logs[0].ends_with(" —— 显式 Unavailable"));
    }

    /// 第四态（同构样板原有行为）：授予但 opener None（设备打不开）→ Unavailable。
    #[test]
    fn wp545_opener_none_gives_unavailable() {
        let mut logs: Vec<String> = Vec::new();
        {
            let mut log = |line: &str| logs.push(line.to_string());
            let out = attach_in::<Msg, FakeIo, _>(
                true,
                LABEL,
                || None,
                OFF,
                SLOT_LEN,
                MAX_SLOTS,
                &mut log,
            );
            assert!(matches!(out, MaybeOutbox::Unavailable(_)));
        }
        assert_eq!(
            logs,
            vec![format!("{LABEL} storage NOT granted -> outbox Unavailable")]
        );
    }
}
