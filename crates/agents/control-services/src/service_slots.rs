//! 蓝图 68 卷 68.1 WP-68.1.2：服务端分块接收槽位表——从
//! `service::control_service_main` 内嵌逻辑提取的**私有可测模块**
//! （对外接口零变更；wire 零变更，禁区 Z1 不涉）。
//!
//! 46.5 按 badge 分槽在此之上收口四类服务端竞争面（蓝图 68 [01] F8/F9）：
//! 1. **槽满**（>24 badge）：不再退化共享槽 0——新会话 Header 即拒绝（词形 `CHUNKED-SLOT-FULL badge=<n>`，客户端走既有超时/降级路径）；
//! 2. **同 badge 双逻辑会话**：前一传输在途时到达的第二会话 Header 拒绝（词形 `CHUNKED-DUP-SESSION badge=<n>`，客户端按传输错误处理）；在途孤儿传输同步清场（respawn/中止恢复路径——拒绝只作用于到达帧，清场非静默：计数 + 词形）；
//! 3. **分页响应覆盖可观测**：未拉完的旧页被同槽新 Final 覆盖即计数（词形 `CHUNKED-PAGE-EVICTED slot=<n>`）；
//! 4. **处理时长观测**：单帧分发超 [`SLOW_CALL_THRESHOLD_MS`] 计数并打 `CHUNKED-SLOW call=<op> dur_ms=<n>`——每词形每分钟至多 1 行（计数不受限、行有界，防串口洪泛；BP61 背压刷日志前科）。
//!
//! 本模块 host 可编译（无 target 依赖）；词形行以 `String` 返回调用方
//! （target 主循环经 `eneros_runtime::println!` 落串口，host 测试直接断言）。
//! 迟到/交错 Data 帧在接收器内部由"传输代次 + 严格 index 序"拦截
//! （`ChunkedReceiver::generation/late_dropped`，蓝图 68.1 WP-68.1.2①）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::{ChunkedReceiver, CHUNK_MAGIC_FINAL, CHUNK_MAGIC_HEADER};

use crate::service::ControlServiceHandler;

/// 槽位上限（46.5 定值不变：svc-config 启动期客户端实测 <24）。
pub(crate) const MAX_CLIENTS: usize = 24;
/// CHUNKED-SLOW 阈值（毫秒；执行卡建议值 100）。
pub(crate) const SLOW_CALL_THRESHOLD_MS: u64 = 100;
/// 每词形打印最小间隔（毫秒）。计数不受限，行有界。
const WORD_LINE_INTERVAL_MS: u64 = 60_000;

/// 词形槽位（`last_word_ms` 索引；新词形全仓唯一性由 68.1 gate 负断言执法）。
const WORD_SLOT_FULL: usize = 0;
const WORD_DUP_SESSION: usize = 1;
const WORD_PAGE_EVICTED: usize = 2;
const WORD_SLOW: usize = 3;

/// 单帧分发分类（CHUNKED-SLOW 的 `call=` 字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Header,
    Data,
    Final,
    Pull,
}

impl FrameKind {
    fn of(payload: &[u8]) -> Self {
        if payload.len() >= 4 && &payload[0..4] == CHUNK_MAGIC_HEADER {
            Self::Header
        } else if payload.len() >= 4 && &payload[0..4] == CHUNK_MAGIC_FINAL {
            Self::Final
        } else if eneros_agent_sdk::is_pull_request(payload) {
            Self::Pull
        } else {
            Self::Data
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Header => "HEADER",
            Self::Data => "DATA",
            Self::Final => "FINAL",
            Self::Pull => "PULL",
        }
    }
}

/// 单槽状态（每 badge 一槽；页进度记账为 68.1 新增 2B/槽）。
struct ClientState {
    badge: u64,
    rx: ChunkedReceiver,
    paged_resp: Vec<u8>,
    pending_full: Option<Vec<u8>>,
    /// 上一响应总页数（`ceil(len/104)`，≥1）。
    pages_total: u8,
    /// 上一响应已拉取的最大页序 +1（0 = 未拉过）。
    pages_pulled: u8,
}

impl ClientState {
    /// 覆盖分页响应并重置拉取进度。
    fn set_paged(&mut self, resp: Vec<u8>) {
        self.pages_total = resp.len().div_ceil(eneros_agent_sdk::RESP_PAGE_MAX).max(1) as u8;
        self.pages_pulled = 0;
        self.paged_resp = resp;
    }
}

/// 单帧分发结果：应答字节 + 诊断行（词形行由 target 主循环落串口）。
pub(crate) struct StepOut {
    pub response: Vec<u8>,
    pub lines: Vec<String>,
}

/// 服务端分块接收槽位表（每 EL0 服务循环一个实例）。
pub(crate) struct SlotTable {
    clients: [Option<ClientState>; MAX_CLIENTS],
    slot_full_rejections: u32,
    dup_session_rejections: u32,
    page_evictions: u32,
    slow_events: u32,
    /// 每词形上次打印时刻（`u64::MAX` = 未打印过；单调 ms 时钟）。
    last_word_ms: [u64; 4],
}

impl Default for SlotTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SlotTable {
    pub(crate) fn new() -> Self {
        Self {
            clients: core::array::from_fn(|_| None),
            slot_full_rejections: 0,
            dup_session_rejections: 0,
            page_evictions: 0,
            slow_events: 0,
            last_word_ms: [u64::MAX; 4],
        }
    }

    /// 已占用槽数（观测/测试面）。
    pub(crate) fn occupied(&self) -> usize {
        self.clients.iter().flatten().count()
    }
    pub(crate) fn slot_full_rejections(&self) -> u32 {
        self.slot_full_rejections
    }
    pub(crate) fn dup_session_rejections(&self) -> u32 {
        self.dup_session_rejections
    }
    pub(crate) fn page_evictions(&self) -> u32 {
        self.page_evictions
    }
    pub(crate) fn slow_events(&self) -> u32 {
        self.slow_events
    }

    /// badge → 槽位：既有槽精确命中，否则首个空槽；全满 = 拒绝（不再退化
    /// 共享槽 0——46.5 遗留的交错 reset 回归窗口，F8④）。
    fn slot_index(&self, badge: u64) -> Result<usize, ()> {
        for (i, c) in self.clients.iter().enumerate() {
            if c.as_ref().map(|c| c.badge) == Some(badge) {
                return Ok(i);
            }
        }
        for (i, c) in self.clients.iter().enumerate() {
            if c.is_none() {
                return Ok(i);
            }
        }
        Err(())
    }

    /// 词形行进队（每词形每分钟至多 1 行；计数在调用方不受限）。
    fn push_line(&mut self, lines: &mut Vec<String>, word: usize, line: String, now_ms: u64) {
        let last = self.last_word_ms[word];
        if last != u64::MAX && now_ms.saturating_sub(last) < WORD_LINE_INTERVAL_MS {
            return;
        }
        self.last_word_ms[word] = now_ms;
        lines.push(line);
    }

    /// 处理一条入站消息（对应主循环一次 recv 的分发全流程；含 SLOW 观测）。
    pub(crate) fn step(
        &mut self,
        badge: u64,
        payload: &[u8],
        handler: &mut dyn ControlServiceHandler,
        now_ms: &mut dyn FnMut() -> u64,
    ) -> StepOut {
        let t0 = now_ms();
        let kind = FrameKind::of(payload);
        let mut out = StepOut {
            response: Vec::new(),
            lines: Vec::new(),
        };
        self.dispatch(badge, payload, handler, now_ms, &mut out);
        let dur = now_ms().saturating_sub(t0);
        if dur >= SLOW_CALL_THRESHOLD_MS {
            self.slow_events = self.slow_events.saturating_add(1);
            let line = format!("CHUNKED-SLOW call={} dur_ms={dur}", kind.name());
            self.push_line(&mut out.lines, WORD_SLOW, line, t0);
        }
        out
    }

    fn dispatch(
        &mut self,
        badge: u64,
        payload: &[u8],
        handler: &mut dyn ControlServiceHandler,
        now_ms: &mut dyn FnMut() -> u64,
        out: &mut StepOut,
    ) {
        let header_frame = payload.len() >= 4 && &payload[0..4] == CHUNK_MAGIC_HEADER;
        let final_frame = payload.len() >= 4 && &payload[0..4] == CHUNK_MAGIC_FINAL;
        let pull_frame = eneros_agent_sdk::is_pull_request(payload);
        let i = match self.slot_index(badge) {
            Ok(i) => i,
            Err(()) => {
                // 槽满：新会话拒绝（词形 + 计数）；非 Header 帧无会话可归，
                // 请求面回 ERR、Send 面静默（seL4 reply 自动丢弃）。
                if header_frame {
                    self.slot_full_rejections = self.slot_full_rejections.saturating_add(1);
                    let line = format!("CHUNKED-SLOT-FULL badge={badge}");
                    self.push_line(&mut out.lines, WORD_SLOT_FULL, line, now_ms());
                    out.response = b"ERR:SlotFull".to_vec();
                } else if final_frame || pull_frame {
                    out.response = b"ERR:NoSlot".to_vec();
                }
                return;
            }
        };
        if self.clients[i].is_none() {
            self.clients[i] = Some(ClientState {
                badge,
                rx: ChunkedReceiver::new(),
                paged_resp: Vec::new(),
                pending_full: None,
                pages_total: 1,
                pages_pulled: 0,
            });
        }
        // 同 badge 第二逻辑会话（前一传输在途时到达的 Header）：拒绝到达帧
        // （客户端按传输错误处理）+ 清场孤儿传输（respawn/中止恢复路径）。
        // 清场非静默：计数 + 词形（t5 纪律）。
        let orphan_in_flight = header_frame && !self.clients[i].as_ref().expect("slot").rx.idle();
        if orphan_in_flight {
            self.dup_session_rejections = self.dup_session_rejections.saturating_add(1);
            let line = format!("CHUNKED-DUP-SESSION badge={badge}");
            self.push_line(&mut out.lines, WORD_DUP_SESSION, line, now_ms());
            let cs = self.clients[i].as_mut().expect("slot");
            cs.rx = ChunkedReceiver::new();
            out.response = b"ERR:DupSession".to_vec();
            return;
        }
        if pull_frame {
            let cs = self.clients[i].as_mut().expect("slot");
            let idx = payload[4];
            cs.pages_pulled = cs.pages_pulled.max(idx.saturating_add(1));
            out.response = eneros_agent_sdk::page_slice(&cs.paged_resp, idx).to_vec();
        } else if final_frame {
            // 覆盖判定先行（与槽可变借用互斥分离）。
            let evict_old = {
                let cs = self.clients[i].as_ref().expect("slot");
                !cs.paged_resp.is_empty() && cs.pages_pulled < cs.pages_total
            };
            let taken = self.clients[i].as_mut().expect("slot").pending_full.take();
            match taken {
                Some(full) => {
                    // 未拉完的旧页被新响应覆盖 → 可观测（F8③收口）。
                    if evict_old {
                        self.page_evictions = self.page_evictions.saturating_add(1);
                        let line = format!("CHUNKED-PAGE-EVICTED slot={i}");
                        self.push_line(&mut out.lines, WORD_PAGE_EVICTED, line, now_ms());
                    }
                    let mut resp = Vec::new();
                    let ok = handler.handle(&full, &mut resp, now_ms());
                    if !ok && resp.is_empty() {
                        resp.extend_from_slice(b"ERR");
                    }
                    let n = {
                        let cs = self.clients[i].as_mut().expect("slot");
                        cs.set_paged(resp);
                        cs.paged_resp.len()
                    };
                    out.response = eneros_agent_sdk::encode_paged_header(n).to_vec();
                }
                None => {
                    let resp = b"ERR:NoTransfer".to_vec();
                    let ret = resp.clone();
                    self.clients[i].as_mut().expect("slot").set_paged(resp);
                    out.response = ret;
                }
            }
        } else {
            let cs = self.clients[i].as_mut().expect("slot");
            match cs.rx.on_message(payload, true) {
                eneros_agent_sdk::ChunkedEvent::Header { .. } => {
                    out.response = eneros_agent_sdk::ack_bytes().to_vec();
                }
                eneros_agent_sdk::ChunkedEvent::Data { .. } => {}
                eneros_agent_sdk::ChunkedEvent::Complete(full) => {
                    cs.pending_full = Some(full);
                }
                eneros_agent_sdk::ChunkedEvent::Malformed(e) => {
                    let resp = format!("ERR:{e:?}").into_bytes();
                    cs.set_paged(resp.clone());
                    out.response = resp;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use eneros_agent_sdk::chunked::{
        CHUNK_ACK, CHUNK_DATA_MAX, CHUNK_MAGIC_DATA, CHUNK_MAGIC_PAGED,
    };

    use super::*;

    // ---- 测试脚手架 ------------------------------------------------------

    /// Echo handler：回显请求载荷（构造多页响应用）。
    struct Echo;
    impl ControlServiceHandler for Echo {
        fn handle(&mut self, req: &[u8], resp: &mut Vec<u8>, _now_ms: u64) -> bool {
            resp.extend_from_slice(req);
            true
        }
    }

    /// 手拨时钟。
    struct Clock(u64);
    impl Clock {
        fn now(&self) -> u64 {
            self.0
        }
        fn advance(&mut self, ms: u64) {
            self.0 += ms;
        }
    }

    fn header_frame(total_len: u32, chunk_count: u8, epoch: u64) -> Vec<u8> {
        let h = eneros_agent_sdk::ChunkHeader {
            total_len,
            chunk_count,
            chunk_size: CHUNK_DATA_MAX as u16,
            epoch,
        };
        h.encode().to_vec()
    }

    fn data_frame(index: u8, fill: u8) -> Vec<u8> {
        let mut frame = [0u8; 109];
        frame[0..4].copy_from_slice(CHUNK_MAGIC_DATA);
        frame[4] = index;
        frame[5..].fill(fill);
        frame.to_vec()
    }

    fn final_frame() -> Vec<u8> {
        eneros_agent_sdk::final_bytes().to_vec()
    }

    fn pull_frame(idx: u8) -> Vec<u8> {
        eneros_agent_sdk::pull_request(idx).to_vec()
    }

    fn step(table: &mut SlotTable, badge: u64, payload: &[u8], clock: &mut Clock) -> StepOut {
        let mut nowfn = || clock.now();
        table.step(badge, payload, &mut Echo, &mut nowfn)
    }

    // 占满槽表（badge 1..=count 各发一条 Header；各槽遗留单块在途传输）。
    fn occupy(table: &mut SlotTable, count: usize, clock: &mut Clock) {
        for b in 1..=count as u64 {
            let _ = step(table, b, &header_frame(10, 1, b), clock);
        }
    }

    fn ack(resp: &StepOut) -> bool {
        resp.response.len() >= 4 && resp.response[0..4] == *CHUNK_ACK
    }

    // ---- t1：迟到 Data(index=0) 不得并入新传输（F8①④；服务端分槽拦截） --

    /// 24 槽满 + 第 25 badge 溢出：其 Header 被拒（不退化共享槽 0），
    /// 既有在途传输（badge 1）不受串扰，迟到数据完整并入自身传输。
    #[test]
    fn t1_late_data_index0_does_not_merge_into_overflow_session() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // badge 1：三块传输在途（发 Header + 2 块）。
        assert!(ack(&step(&mut t, 1, &header_frame(270, 3, 1), &mut clock)));
        let _ = step(&mut t, 1, &data_frame(0, 0xAA), &mut clock);
        let _ = step(&mut t, 1, &data_frame(1, 0xAA), &mut clock);
        // badges 2..=24 占满余下 23 槽。
        for b in 2..=24u64 {
            assert!(ack(&step(&mut t, b, &header_frame(10, 1, b), &mut clock)));
        }
        assert_eq!(t.occupied(), 24);
        // 第 25 badge：Header 被拒（红=现状退化共享槽 0 并 reset badge 1）。
        let b25 = step(&mut t, 25, &header_frame(50, 1, 25), &mut clock);
        assert!(!ack(&b25), "槽满 badge 的新会话 Header 必须被拒");
        assert_eq!(t.slot_full_rejections(), 1);
        assert!(
            b25.lines
                .iter()
                .any(|l| l.contains("CHUNKED-SLOT-FULL badge=25")),
            "SLOT-FULL 词形缺失: {:?}",
            b25.lines
        );
        // badge 1 迟到第 3 块 → 完整并入自身传输（不被 B 污染）。
        let _ = step(&mut t, 1, &data_frame(2, 0xAA), &mut clock);
        let fin = step(&mut t, 1, &final_frame(), &mut clock);
        assert!(
            fin.response.len() >= 10 && fin.response[0..4] == *CHUNK_MAGIC_PAGED,
            "badge 1 Final 必须回分页头（传输完好）: {:?}",
            fin.response
        );
        let page0 = step(&mut t, 1, &pull_frame(0), &mut clock);
        assert_eq!(page0.response.len(), eneros_agent_sdk::RESP_PAGE_MAX);
        assert!(
            page0.response.iter().all(|&b| b == 0xAA),
            "badge 1 载荷被串扰污染"
        );
    }

    // ---- t3：分页响应 badge 隔离 + 同槽覆盖可观测（F8③） ----------------

    /// 客户端 A 未拉完的页不受客户端 B 新 Final 影响（分槽隔离）；同槽
    /// 新 Final 覆盖未拉完旧页 → PAGE-EVICTED 计数 + 词形。
    #[test]
    fn t3_paged_resp_isolation_and_eviction_observed() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // A：150B 回显（2 页），拉第 0 页后停。
        let _ = step(&mut t, 1, &header_frame(150, 2, 1), &mut clock);
        for i in 0..2 {
            let _ = step(&mut t, 1, &data_frame(i, 0x11), &mut clock);
        }
        let _ = step(&mut t, 1, &final_frame(), &mut clock);
        let a_page0 = step(&mut t, 1, &pull_frame(0), &mut clock);
        assert_eq!(a_page0.response.len(), 104);
        assert!(
            a_page0.response.iter().all(|&b| b == 0x11),
            "A 第 0 页必须还是 A 的数据"
        );
        // B：同规格传输 + Final（异 badge 新 Final）。
        let _ = step(&mut t, 2, &header_frame(150, 2, 2), &mut clock);
        for i in 0..2 {
            let _ = step(&mut t, 2, &data_frame(i, 0x22), &mut clock);
        }
        let _ = step(&mut t, 2, &final_frame(), &mut clock);
        // A 继续拉第 1 页 → 必须仍是 A 的页（不是 B 的）。
        let a_page1 = step(&mut t, 1, &pull_frame(1), &mut clock);
        assert_eq!(a_page1.response.len(), 46);
        assert!(
            a_page1.response.iter().all(|&b| b == 0x11),
            "A 拉到了 B 的页"
        );
        // A 传输 2（150B，填充 0x33）+ Final → 新页缓存；只拉第 0 页。
        let _ = step(&mut t, 1, &header_frame(150, 2, 9), &mut clock);
        for i in 0..2 {
            let _ = step(&mut t, 1, &data_frame(i, 0x33), &mut clock);
        }
        let _ = step(&mut t, 1, &final_frame(), &mut clock);
        let a2_page0 = step(&mut t, 1, &pull_frame(0), &mut clock);
        assert!(a2_page0.response.iter().all(|&b| b == 0x33));
        // 同槽覆盖：传输 3 的 Final 覆盖只拉了 1/2 页的旧响应 → 驱逐可观测。
        let _ = step(&mut t, 1, &header_frame(10, 1, 10), &mut clock);
        let _ = step(&mut t, 1, &data_frame(0, 0x44), &mut clock);
        let out = step(&mut t, 1, &final_frame(), &mut clock);
        assert_eq!(t.page_evictions(), 1, "未拉完旧页被覆盖必须计数");
        assert!(
            out.lines
                .iter()
                .any(|l| l.contains("CHUNKED-PAGE-EVICTED slot=0")),
            "PAGE-EVICTED 词形缺失: {:?}",
            out.lines
        );
    }

    // ---- t4：>24 槽满拒绝（锁住"不再退化共享槽 0"） ----------------------

    #[test]
    fn t4_slot_overflow_rejects_new_session_and_keeps_table() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        occupy(&mut t, 24, &mut clock);
        assert_eq!(t.occupied(), 24);
        // 第 25 badge：拒 + 词形；重试再拒（计数累加，不占槽）。
        let b25 = step(&mut t, 25, &header_frame(50, 1, 25), &mut clock);
        assert!(!ack(&b25));
        let b25_retry = step(&mut t, 25, &header_frame(50, 1, 26), &mut clock);
        assert!(!ack(&b25_retry));
        assert_eq!(t.slot_full_rejections(), 2);
        assert_eq!(t.occupied(), 24, "被拒 badge 不得占槽");
        // 既有槽（badge 7）传输不受影响。
        let _ = step(&mut t, 7, &data_frame(0, 0x44), &mut clock);
        let fin = step(&mut t, 7, &final_frame(), &mut clock);
        assert!(fin.response[0..4] == *CHUNK_MAGIC_PAGED, "既有槽被串扰");
    }

    // ---- t5：同 badge 双逻辑会话（不得静默互 reset） ---------------------

    #[test]
    fn t5_same_badge_second_session_header_rejected_not_silent() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // badge 7：传输在途（Header + 1 块，共 3 块）。
        assert!(ack(&step(&mut t, 7, &header_frame(270, 3, 1), &mut clock)));
        let _ = step(&mut t, 7, &data_frame(0, 0x55), &mut clock);
        // 第二逻辑会话 Header：拒绝（红=现状 ENVK 静默 reset）。
        let dup = step(&mut t, 7, &header_frame(100, 1, 2), &mut clock);
        assert!(!ack(&dup), "同 badge 在途传输上的第二会话 Header 必须被拒");
        assert_eq!(t.dup_session_rejections(), 1);
        assert!(
            dup.lines
                .iter()
                .any(|l| l.contains("CHUNKED-DUP-SESSION badge=7")),
            "DUP-SESSION 词形缺失: {:?}",
            dup.lines
        );
        // 孤儿已清场：新会话重试（同 badge）→ 正常建传输并完成（恢复路径）。
        assert!(ack(&step(&mut t, 7, &header_frame(10, 1, 3), &mut clock)));
        let _ = step(&mut t, 7, &data_frame(0, 0x66), &mut clock);
        let fin = step(&mut t, 7, &final_frame(), &mut clock);
        assert!(fin.response[0..4] == *CHUNK_MAGIC_PAGED);
    }

    // ---- 观测面：SLOW（阈值 + 每词形每分钟 1 行） ------------------------

    #[test]
    fn slow_call_counted_and_line_rate_limited_per_word() {
        struct SlowHandler;
        impl ControlServiceHandler for SlowHandler {
            fn handle(&mut self, _req: &[u8], _resp: &mut Vec<u8>, _now_ms: u64) -> bool {
                true
            }
        }
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        let _ = step(&mut t, 1, &header_frame(10, 1, 1), &mut clock);
        let _ = step(&mut t, 1, &data_frame(0, 0x77), &mut clock);
        // Final 处理期间时钟推进 150ms ≥ 100 阈值。
        let mut nowfn = || {
            let t0 = clock.0;
            clock.0 += 100;
            t0
        };
        let out = t.step(1, &final_frame(), &mut SlowHandler, &mut nowfn);
        assert_eq!(t.slow_events(), 1);
        assert!(
            out.lines
                .iter()
                .any(|l| l.contains("CHUNKED-SLOW call=FINAL dur_ms=200")),
            "SLOW 词形缺失: {:?}",
            out.lines
        );
        // 1 分钟内第二次慢调用：计数 +1、行不再打（有界）。
        let mut nowfn2 = || {
            let t0 = clock.0;
            clock.0 += 100;
            t0
        };
        let out2 = t.step(1, &final_frame(), &mut SlowHandler, &mut nowfn2);
        assert_eq!(t.slow_events(), 2);
        assert!(
            !out2.lines.iter().any(|l| l.contains("CHUNKED-SLOW")),
            "SLOW 行必须每分钟每词形至多 1 行"
        );
        // 超过间隔 → 行恢复。
        clock.advance(60_001);
        let mut nowfn3 = || {
            let t0 = clock.0;
            clock.0 += 100;
            t0
        };
        let out3 = t.step(1, &final_frame(), &mut SlowHandler, &mut nowfn3);
        assert!(
            out3.lines.iter().any(|l| l.contains("CHUNKED-SLOW")),
            "间隔后 SLOW 行必须恢复"
        );
    }
}
