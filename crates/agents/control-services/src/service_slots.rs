//! 蓝图 68 卷 68.1 WP-68.1.2：服务端分块接收槽位表——从
//! `service::control_service_main` 内嵌逻辑提取的**私有可测模块**
//! （对外接口零变更；wire 零变更，禁区 Z1 不涉）。
//!
//! 46.5 按 badge 分槽在此之上收口四类服务端竞争面（蓝图 68 [01] F8/F9）：
//! 1. **槽满**（>24 badge）：不再退化共享槽 0——新会话 Header 即拒绝（词形 `CHUNKED-SLOT-FULL badge=<n>`，客户端走既有超时/降级路径）；
//! 2. **同 badge 双逻辑会话**：前一传输在途时到达的第二会话 Header 拒绝（词形 `CHUNKED-DUP-SESSION badge=<n>`，客户端按传输错误处理）；在途孤儿传输同步清场（respawn/中止恢复路径——拒绝只作用于到达帧，清场非静默：计数 + 词形）；
//! 3. **分页响应覆盖可观测 + 自愈重同步**：未拉完的旧页被同槽覆盖性
//!    Final 覆盖即计数（词形 `CHUNKED-PAGE-EVICTED slot=<n>`）且新响应
//!    照常安装——单 badge 顺序客户端下覆盖 = 与重试客户端页序重同步
//!    （69.1 实弹订正：拒绝式改法造成重试活锁，已撤回，见 t12）；
//! 4. **处理时长观测**：单帧分发超 [`SLOW_CALL_THRESHOLD_MS`] 计数并打 `CHUNKED-SLOW call=<op> dur_ms=<n>`——每词形每分钟至多 1 行（计数不受限、行有界，防串口洪泛；BP61 背压刷日志前科）。
//!
//! 69.1（接缝判别力收口）在本表之上另收口：
//! - **陈旧 pending_full 错配**（BUG-S2）：Header 被接受即清上一传输的
//!   `pending_full`——任何通过 F3 闸门的 Final，其 `take()` 必属本次传输
//!   （t10/t10c 咬合）；
//! - **越界 Pull 拒绝记账**（O1）：`page_range_rejections` 计数 + 词形
//!   `CHUNKED-PAGE-RANGE slot=<n>`（t11）；
//! - **响应方向页数域检**（I4）：超 u8 页序域界（255×104=26520B）拒绝
//!   安装，计数 + 词形 `CHUNKED-RESP-TOOLARGE slot=<n>`（t14；界内既有
//!   生产面如 svc-config 7099B 配置分发不受扰）；
//! - **单帧回执 MR 预算**（I3）：静态锁（`MR_REPLY_BUDGET_BYTES`）+
//!   step() 出口兜底替换 + 词形 `CHUNKED-REPLY-TRUNCATED`（t13/t13b）；
//! - **单帧 ERR 回复一律不入分页态**：ERR 文本若装作 1 页"未拉响应"，
//!   既污染 evict 判据又主动制造页序发散（覆盖在拉响应）。
//!
//! 本模块 host 可编译（无 target 依赖）；词形行以 `String` 返回调用方
//! （target 主循环经 `eneros_runtime::println!` 落串口，host 测试直接断言）。
//! 迟到/交错 Data 帧在接收器内部由"传输代次 + 严格 index 序"拦截
//! （`ChunkedReceiver::generation/late_dropped`，蓝图 68.1 WP-68.1.2①）。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::{
    ChunkHeader, ChunkedReceiver, CHUNK_MAGIC_FINAL, CHUNK_MAGIC_HEADER, PULL_REQUEST_V2_LEN,
};

use crate::service::ControlServiceHandler;

/// 槽位上限（46.5 定值不变：svc-config 启动期客户端实测 <24）。
pub(crate) const MAX_CLIENTS: usize = 24;
/// CHUNKED-SLOW 阈值（毫秒；执行卡建议值 100）。
pub(crate) const SLOW_CALL_THRESHOLD_MS: u64 = 100;
/// 每词形打印最小间隔（毫秒）。计数不受限，行有界。
const WORD_LINE_INTERVAL_MS: u64 = 60_000;

/// 蓝图 70.3（WP-70.3.1）进程级分页字节预算（P 建议值 64KiB，单源）。
///
/// 组装（≤1664B/传输）+ 完整请求（≤1664B/槽）+ 分页响应（域界 26520B）
/// 的合计准入界——只有"槽数"准入时，24 槽理论峰值
/// 24×(1664+1664+26520) ≈ 715KiB 可超 512KiB 堆（方向 2 §6 预算表）；
/// 字节准入后到界 = 显式 `ERR:BudgetExceeded`（资源错误而非 OOM）。
/// 64KiB = 生产峰值（svc-config 7099B 配置分发 + 全槽 270B 往返 ≈
/// 20KiB）的 3× 余量，同时把理论峰值压到一个数量级以下。
pub(crate) const SLOT_BYTE_BUDGET_BYTES: usize = 64 * 1024;
/// 蓝图 70.3（WP-70.3.4）**纯缓存槽**回收时限（无在途传输、无待 Final、
/// 页已拉完——P 建议值 60s）。
pub(crate) const SLOT_IDLE_TTL_MS: u64 = 60_000;
/// **孤儿槽**回收时限（有未完成工作但无任何进展——Header 后死亡/
/// Complete 后无 Final/半拉响应停滞；P 建议值 300s）。
pub(crate) const SLOT_ORPHAN_TTL_MS: u64 = 300_000;

/// 词形槽位（`last_word_ms` 索引；新词形全仓唯一性由 68.1 gate 负断言执法）。
const WORD_SLOT_FULL: usize = 0;
const WORD_DUP_SESSION: usize = 1;
const WORD_PAGE_EVICTED: usize = 2;
const WORD_SLOW: usize = 3;
/// 69.1 新增词形：越界 Pull 拒绝记账（O1）/ 响应超页数上限（I4）/
/// 单帧回执超 MR 预算兜底（I3）。
const WORD_PAGE_RANGE: usize = 4;
const WORD_RESP_TOO_LARGE: usize = 5;
const WORD_REPLY_TRUNCATED: usize = 6;
/// 蓝图 70.3 新增词形：进程字节预算拒绝（WP-70.3.1）/ 槽回收（WP-70.3.4）/
/// Pull 事务标识失配或 v1 遗留形（WP-70.3.3）。
const WORD_BUDGET: usize = 7;
const WORD_RECLAIM: usize = 8;
const WORD_PULL_STALE: usize = 9;

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
    /// 本槽分页事务标识（70.3 WP-70.3.3 = 被接受 Header 的 epoch 低 16
    /// 位；Final 安装响应时绑定、Pull 校验一致才供页）。
    txn: u16,
    /// 在途组装预算记账（70.3 WP-70.3.1 = 被接受 Header 的 total_len；
    /// Complete/Malformed 归零——挂起载荷转入 pending_full 按 len 计）。
    assembly_bytes: usize,
    /// 最近一次事件时刻（单调 ms；70.3 WP-70.3.4 回收判据基准）。
    last_progress_ms: u64,
}

/// 页数超上限拒绝哨兵（69.1 I4 域检；`set_paged` 失败形态）。
struct PageCapExceeded;

impl ClientState {
    /// 覆盖分页响应并重置拉取进度。
    ///
    /// 69.1（I4 / REG-CHUNKED-PAGE-U8-CAP，t14 咬合）：响应方向页数域检
    /// ——超 [`eneros_agent_sdk::RESP_COUNT_MAX`]（u8 域界 255）×
    /// [`eneros_agent_sdk::RESP_PAGE_MAX`]（= 26520B）拒绝安装；
    /// `as u8` 静默回绕翻为显式拒绝。失败时**不动分页态**——ERR 文本若
    /// 装作 1 页"未拉响应"会污染 evict_old 判据（次生：下一个合法 Final
    /// 被误拒，t14 边界段实测咬合后订正）。
    fn set_paged(&mut self, resp: Vec<u8>) -> Result<(), PageCapExceeded> {
        if resp.len() > eneros_agent_sdk::RESP_COUNT_MAX as usize * eneros_agent_sdk::RESP_PAGE_MAX
        {
            return Err(PageCapExceeded);
        }
        self.pages_total =
            u8::try_from(resp.len().div_ceil(eneros_agent_sdk::RESP_PAGE_MAX).max(1))
                .expect("域检已保证 ≤RESP_COUNT_MAX");
        self.pages_pulled = 0;
        self.paged_resp = resp;
        Ok(())
    }

    /// 本槽字节占用（进程预算记账口径；70.3 WP-70.3.1）。
    fn bytes(&self) -> usize {
        self.assembly_bytes
            + self.pending_full.as_ref().map_or(0, |v| v.len())
            + self.paged_resp.len()
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
    /// 69.1 观测面：越界 Pull 拒绝计数（O1，t11 咬合）。
    page_range_rejections: u32,
    /// 69.1 观测面：响应超页数上限拒绝计数（I4，t14 咬合）。
    resp_too_large: u32,
    /// 69.1 观测面：单帧回执超 MR 预算兜底替换计数（I3，t13b 咬合）。
    reply_truncations: u32,
    /// 70.3 观测面：进程字节预算拒绝计数（WP-70.3.1）。
    budget_rejections: u32,
    /// 70.3 观测面：Pull 事务标识失配 / v1 遗留形拒绝计数（WP-70.3.3）。
    pull_txn_rejections: u32,
    /// 70.3 观测面：空闲/孤儿槽回收计数（WP-70.3.4）。
    slots_reclaimed: u32,
    /// 70.3：进程分页字节预算（构造期可注入；生产 = 单源默认）。
    byte_budget: usize,
    /// 每词形上次打印时刻（`u64::MAX` = 未打印过；单调 ms 时钟）。
    /// 70.3 扩 7 → 10（BUDGET / RECLAIM / PULL-STALE 三词形各占独立槽）。
    last_word_ms: [u64; 10],
}

impl Default for SlotTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SlotTable {
    pub(crate) fn new() -> Self {
        Self::with_byte_budget(SLOT_BYTE_BUDGET_BYTES)
    }

    /// 指定进程字节预算（测试注入；生产走 [`Self::new`] 单源默认）。
    pub(crate) fn with_byte_budget(byte_budget: usize) -> Self {
        Self {
            clients: core::array::from_fn(|_| None),
            slot_full_rejections: 0,
            dup_session_rejections: 0,
            page_evictions: 0,
            slow_events: 0,
            page_range_rejections: 0,
            resp_too_large: 0,
            reply_truncations: 0,
            budget_rejections: 0,
            pull_txn_rejections: 0,
            slots_reclaimed: 0,
            byte_budget,
            last_word_ms: [u64::MAX; 10],
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
    /// 69.1 观测面（O1）：越界 Pull 拒绝计数。
    pub(crate) fn page_range_rejections(&self) -> u32 {
        self.page_range_rejections
    }
    /// 69.1 观测面（I4）：响应超页数上限拒绝计数。
    pub(crate) fn resp_too_large(&self) -> u32 {
        self.resp_too_large
    }
    /// 69.1 观测面（I3）：单帧回执超 MR 预算兜底替换计数。
    pub(crate) fn reply_truncations(&self) -> u32 {
        self.reply_truncations
    }
    /// 70.3 观测面（WP-70.3.1）：进程字节预算拒绝计数。
    pub(crate) fn budget_rejections(&self) -> u32 {
        self.budget_rejections
    }
    /// 70.3 观测面（WP-70.3.3）：Pull 事务失配/遗留形拒绝计数。
    pub(crate) fn pull_txn_rejections(&self) -> u32 {
        self.pull_txn_rejections
    }
    /// 70.3 观测面（WP-70.3.4）：槽回收计数。
    pub(crate) fn slots_reclaimed(&self) -> u32 {
        self.slots_reclaimed
    }

    /// 当前分页字节占用（组装 + 挂起完整请求 + 分页响应，全槽合计）。
    fn bytes_in_use(&self) -> usize {
        self.clients.iter().flatten().map(|c| c.bytes()).sum()
    }

    /// 回收扫描（70.3 WP-70.3.4；step() 入口调用——有界 ≤24 槽，与
    /// slot_index 同复杂度，不阻塞关键调用）。
    ///
    /// 判据（单调 ms）：
    /// - **纯缓存槽**（无在途传输、无挂起 Final、页已拉完）空闲超
    ///   [`SLOT_IDLE_TTL_MS`] → 整槽回收（分页缓存一并释放）；
    /// - **孤儿槽**（有未完成工作但 [`SLOT_ORPHAN_TTL_MS`] 无任何进展
    ///   ——Header 后死亡/Complete 后无 Final/半拉响应停滞）→ 整槽回收。
    ///
    /// 回收非静默：计数 + 词形（每词形每分钟 1 行）。
    fn reclaim_sweep(&mut self, now_ms: u64, lines: &mut Vec<String>) {
        for i in 0..MAX_CLIENTS {
            let Some(cs) = self.clients[i].as_ref() else {
                continue;
            };
            let live_work = !cs.rx.idle()
                || cs.pending_full.is_some()
                || (!cs.paged_resp.is_empty() && cs.pages_pulled < cs.pages_total);
            let ttl = if live_work {
                SLOT_ORPHAN_TTL_MS
            } else {
                SLOT_IDLE_TTL_MS
            };
            if now_ms.saturating_sub(cs.last_progress_ms) > ttl {
                let badge = cs.badge;
                self.clients[i] = None;
                self.slots_reclaimed = self.slots_reclaimed.saturating_add(1);
                let line = format!("CHUNKED-SLOT-RECLAIM badge={badge} ttl={ttl}");
                self.push_line(lines, WORD_RECLAIM, line, now_ms);
            }
        }
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

    /// 单帧回执 MR 预算兜底（69.1 I3）：超
    /// [`eneros_agent_sdk::MR_REPLY_BUDGET_BYTES`] 的回执替换为定长
    /// `ERR:ReplyTooLarge`（19B）+ 计数 + 词形。静态锁保证正常路径
    /// 不可达（全 ERR 文本 ≤ 112B、分页响应只回头 10B、页载荷 ≤ 104B）；
    /// 本兜底只对"未来的意外超长形态"（如 Debug 派生变长）负责。
    fn clamp_reply_budget(&mut self, out: &mut StepOut, now_ms: u64) {
        if out.response.len() <= eneros_agent_sdk::MR_REPLY_BUDGET_BYTES {
            return;
        }
        self.reply_truncations = self.reply_truncations.saturating_add(1);
        let line = format!("CHUNKED-REPLY-TRUNCATED len={}", out.response.len());
        self.push_line(&mut out.lines, WORD_REPLY_TRUNCATED, line, now_ms);
        out.response = b"ERR:ReplyTooLarge".to_vec();
    }

    /// 处理一条入站消息（对应主循环一次 recv 的分发全流程；含 SLOW 观测
    /// 与 70.3 回收扫描——两者都有界：扫描 ≤24 槽，与 slot_index 同复杂度）。
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
        // 70.3（WP-70.3.4）：孤儿/空闲槽回收扫描（入站事件驱动；有界）。
        self.reclaim_sweep(t0, &mut out.lines);
        self.dispatch(badge, payload, handler, now_ms, &mut out);
        // 69.1（I3 / REG-CHUNKED-MR-WORDS-CAP 运行时兜底，t13b 咬合）：
        // 单帧回执超 MR 预算 ⇒ 显式替换拒绝而非静默截断（service.rs 组帧
        // break 与客户端 min(words, MR_REPLY_MAX_WORDS) 两端静默上限的
        // 不可达化——静态锁保证正常路径不触发，但 `ERR:{e:?}` 的 Debug
        // 派生长度不可静态界定）。
        self.clamp_reply_budget(&mut out, t0);
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
        // 70.3（WP-70.3.1）：Header 字节预算准入——在**任何槽创建/状态机
        // 进入之前**（被拒 badge 不占槽、不产生组装驻留）。仅在“该 badge
        // 无在途传输”时适用（在途 = DUP-SESSION 违例面，让位给既有判别）；
        // 挂起 pending_full 将被新 Header 清理（BUG-S2 语义），其字节计入
        // 可释放侧。
        if header_frame {
            if let Ok(h) = ChunkHeader::decode(payload) {
                let existing = self.clients.iter().flatten().find(|c| c.badge == badge);
                let rx_idle_or_absent = existing.map_or(true, |c| c.rx.idle());
                let freeable =
                    existing.map_or(0, |c| c.pending_full.as_ref().map_or(0, |v| v.len()));
                if rx_idle_or_absent
                    && self.bytes_in_use() - freeable + h.total_len as usize > self.byte_budget
                {
                    self.budget_rejections = self.budget_rejections.saturating_add(1);
                    let line = format!("CHUNKED-BUDGET badge={badge} need={}", h.total_len);
                    self.push_line(&mut out.lines, WORD_BUDGET, line, now_ms());
                    out.response = b"ERR:BudgetExceeded".to_vec();
                    return;
                }
            }
        }
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
                txn: 0,
                assembly_bytes: 0,
                last_progress_ms: now_ms(),
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
            // 70.3（WP-70.3.3 / ADR-0053）：Pull 请求 v2 形（7B，携带
            // txn）。v1 遗留 5B 形 / txn 失配 = 协议违例 → 显式拒绝 +
            // 计数 + 词形（不可判别通道已关闭，旧代次页绝不供出）。
            if payload.len() < PULL_REQUEST_V2_LEN
                || u16::from_le_bytes([payload[5], payload[6]]) != cs.txn
            {
                self.pull_txn_rejections = self.pull_txn_rejections.saturating_add(1);
                let line = format!("CHUNKED-PULL-STALE slot={i}");
                self.push_line(&mut out.lines, WORD_PULL_STALE, line, now_ms());
                out.response = b"ERR:PullStale".to_vec();
                return;
            }
            let idx = payload[4];
            // 68 三轮复查修复（F1/F2，t8 咬合）：越界/无存续分页响应的
            // Pull 回结构化 ERR——空响应在 target 主循环 message_info_new
            // 零长度 = 全零 MessageInfo，与"无待回复"哨兵同形，reply 阶段
            // 被吞，Call 语义客户端（非 MCS 无 IPC 超时）永久挂死。且此
            // 路径不得推进 pages_pulled（越界杂散帧毒化 PAGE-EVICTED 观测）。
            // 69.1（O1，t11 咬合）：拒绝路径记账 + 词形。
            let page = eneros_agent_sdk::page_slice(&cs.paged_resp, idx);
            if page.is_empty() {
                self.page_range_rejections = self.page_range_rejections.saturating_add(1);
                let line = format!("CHUNKED-PAGE-RANGE slot={i}");
                self.push_line(&mut out.lines, WORD_PAGE_RANGE, line, now_ms());
                out.response = b"ERR:PageRange".to_vec();
                return;
            }
            // v2 判别页帧（ENVR + page_len + txn；页 ≤104B → 帧 ≤111B
            // ≤ MR 回执预算，编译期锁在 agent-sdk）。
            out.response =
                eneros_agent_sdk::encode_page_frame(page, cs.txn).expect("页 ≤104B 编码不可失败");
            cs.pages_pulled = cs.pages_pulled.max(idx.saturating_add(1));
            cs.last_progress_ms = now_ms();
        } else if final_frame {
            // 68 三轮复查修复（F3，t9 咬合）：rx 非空闲 = 本传输块未齐，
            // Final 属协议违例——拒帧且不得 take()（否则上一传输的陈旧
            // pending_full 被当本次请求处理，以成功形态错配）。在途传输
            // 不受损：补齐剩余块后重发 Final 正常完成。
            // 69.1 注：单帧 ERR 回复（含本臂）一律**不**入分页态（不
            // set_paged）。
            let rx_idle = self.clients[i].as_ref().expect("slot").rx.idle();
            if !rx_idle {
                out.response = b"ERR:TransferIncomplete".to_vec();
                return;
            }
            // 覆盖判定先行（与槽可变借用互斥分离）。
            let evict_old = {
                let cs = self.clients[i].as_ref().expect("slot");
                !cs.paged_resp.is_empty() && cs.pages_pulled < cs.pages_total
            };
            let taken = self.clients[i].as_mut().expect("slot").pending_full.take();
            match taken {
                Some(full) => {
                    // 未拉完的旧页被新响应覆盖 → 可观测（F8③收口，68.1 语义）。
                    // 69.1 订正（VOL69.1-GATE 实弹证伪卷册 B-1"拒绝覆盖"）：
                    // 覆盖 = 与重试客户端页序**重新同步**（自愈）；拒绝 =
                    // 活锁。维持覆盖 + 计数 + 词形。70.3 起残余面（respawn/
                    // badge 复用后旧客户端续拉）由 v2 txn 判别关闭（发散源
                    // #1 / REG-CHUNKED-PULL-NO-DISCRIMINATOR）。
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
                    // 70.3（WP-70.3.1）：响应安装前做进程预算准入——替换
                    // 旧分页响应（可释放部分先记入），到界 = 显式拒绝
                    // （资源错误而非 OOM；响应本体的分配面在 handler，
                    // 此处拒绝的是"驻留"）。
                    {
                        let cs = self.clients[i].as_ref().expect("slot");
                        let freeable = cs.paged_resp.len();
                        // pending_full 已 take（字节已让出）、组装已完
                        // （assembly=0）——新驻留 = 现占用 − 旧分页 + 新响应。
                        let in_use = self.bytes_in_use();
                        if in_use - freeable + resp.len() > self.byte_budget {
                            self.budget_rejections = self.budget_rejections.saturating_add(1);
                            let line = format!("CHUNKED-BUDGET slot={i} need={}", resp.len());
                            self.push_line(&mut out.lines, WORD_BUDGET, line, now_ms());
                            out.response = b"ERR:BudgetExceeded".to_vec();
                            return;
                        }
                    }
                    let cs = self.clients[i].as_mut().expect("slot");
                    if cs.set_paged(resp).is_err() {
                        // 69.1（I4 / REG-CHUNKED-PAGE-U8-CAP，t14 咬合）：
                        // 响应超页数上限——显式拒绝 + 计数 + 词形，不得
                        // 静默截断页数声明。
                        self.resp_too_large = self.resp_too_large.saturating_add(1);
                        let line = format!("CHUNKED-RESP-TOOLARGE slot={i}");
                        self.push_line(&mut out.lines, WORD_RESP_TOO_LARGE, line, now_ms());
                        out.response = b"ERR:ResponseTooLarge".to_vec();
                        return;
                    }
                    // v2 分页头（携带本槽事务标识——客户端逐页回带校验）。
                    out.response =
                        eneros_agent_sdk::encode_paged_header_v2(cs.paged_resp.len(), cs.txn)
                            .to_vec();
                    cs.last_progress_ms = now_ms();
                }
                None => {
                    out.response = b"ERR:NoTransfer".to_vec();
                }
            }
        } else {
            let cs = self.clients[i].as_mut().expect("slot");
            match cs.rx.on_message(payload, true) {
                eneros_agent_sdk::ChunkedEvent::Header {
                    total_len, epoch, ..
                } => {
                    // 69.1（BUG-S2 / REG-CHUNK-FINAL-STALE-MATCH 补全，t10/t10c
                    // 咬合）：Header 被接受 = 新传输开始 = 上一传输的组装产物
                    // 作废。不清则 Malformed/DUP 复位 rx 后，Final 的 rx_idle
                    // 闸门（F3）会放行陈旧载荷并以合法 ENVP 错配（静默错误
                    // 成功）。完备性：任何"陈旧 pending_full + 到达 Final"的
                    // 组合，中间必有一次**被接受的** Header——DUP-SESSION 拒绝
                    // 使客户端在 Header 步即失败（永不到 Final），Malformed
                    // 复位不产生 Final；故在被接受的 Header 清理即完备。
                    cs.pending_full = None;
                    // 70.3（WP-70.3.3/70.3.1）：绑定事务标识 + 组装预算记账。
                    cs.txn = epoch as u16;
                    cs.assembly_bytes = total_len as usize;
                    cs.last_progress_ms = now_ms();
                    out.response = eneros_agent_sdk::ack_bytes().to_vec();
                }
                eneros_agent_sdk::ChunkedEvent::Data { .. } => {
                    cs.last_progress_ms = now_ms();
                }
                eneros_agent_sdk::ChunkedEvent::Complete(full) => {
                    cs.assembly_bytes = 0;
                    cs.pending_full = Some(full);
                    cs.last_progress_ms = now_ms();
                }
                eneros_agent_sdk::ChunkedEvent::Malformed(e) => {
                    // 69.1：单帧 ERR 回复不入分页态（见 final_frame 臂注）。
                    cs.assembly_bytes = 0;
                    let resp = format!("ERR:{e:?}").into_bytes();
                    out.response = resp;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use eneros_agent_sdk::chunked::{
        decode_page_frame, pull_request_v2, CHUNK_ACK, CHUNK_DATA_MAX, CHUNK_MAGIC_DATA,
        CHUNK_MAGIC_PAGED_V2, PAGED_HEADER_V2_LEN, RESP_PAGE_MAX,
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

    fn pull_frame(idx: u8, txn: u16) -> Vec<u8> {
        pull_request_v2(idx, txn).to_vec()
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
            fin.response.len() >= PAGED_HEADER_V2_LEN
                && fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2,
            "badge 1 Final 必须回分页头（传输完好）: {:?}",
            fin.response
        );
        let page0 = step(&mut t, 1, &pull_frame(0, 1), &mut clock);
        let (_, _, data) = decode_page_frame(&page0.response).expect("判别页帧");
        assert_eq!(data.len(), RESP_PAGE_MAX);
        assert!(data.iter().all(|&b| b == 0xAA), "badge 1 载荷被串扰污染");
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
        let a_page0 = step(&mut t, 1, &pull_frame(0, 1), &mut clock);
        let (_, _, data) = decode_page_frame(&a_page0.response).expect("判别页帧");
        assert_eq!(data.len(), RESP_PAGE_MAX);
        assert!(
            data.iter().all(|&b| b == 0x11),
            "A 第 0 页必须还是 A 的数据"
        );
        // B：同规格传输 + Final（异 badge 新 Final）。
        let _ = step(&mut t, 2, &header_frame(150, 2, 2), &mut clock);
        for i in 0..2 {
            let _ = step(&mut t, 2, &data_frame(i, 0x22), &mut clock);
        }
        let _ = step(&mut t, 2, &final_frame(), &mut clock);
        // A 继续拉第 1 页 → 必须仍是 A 的页（不是 B 的）。
        let a_page1 = step(&mut t, 1, &pull_frame(1, 1), &mut clock);
        let (_, _, data) = decode_page_frame(&a_page1.response).expect("判别页帧");
        assert_eq!(data.len(), 46);
        assert!(data.iter().all(|&b| b == 0x11), "A 拉到了 B 的页");
        // A 传输 2（150B，填充 0x33）+ Final → 新页缓存；只拉第 0 页。
        let _ = step(&mut t, 1, &header_frame(150, 2, 9), &mut clock);
        for i in 0..2 {
            let _ = step(&mut t, 1, &data_frame(i, 0x33), &mut clock);
        }
        let _ = step(&mut t, 1, &final_frame(), &mut clock);
        let a2_page0 = step(&mut t, 1, &pull_frame(0, 9), &mut clock);
        let (_, _, data) = decode_page_frame(&a2_page0.response).expect("判别页帧");
        assert!(data.iter().all(|&b| b == 0x33));
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
        assert!(fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2, "既有槽被串扰");
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
        assert!(fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2);
    }

    // ---- t8：越界/空响应 Pull 须结构化拒绝（F1 挂死面 + F2 毒化面） ------

    /// 68 三轮复查 t8（红测先行）：Pull 越界（或无分页响应存续）时——
    /// ① 必须回结构化 ERR（红=空响应：target 主循环 message_info_new
    /// 零长度 = 全零 MessageInfo，与"无待回复"哨兵同形，reply 阶段被
    /// 吞，Call 语义客户端（非 MCS 无 IPC 超时）永久挂死）；② 越界帧
    /// 不得推进 pages_pulled（红=毒化：后续同槽覆盖性 Final 的
    /// PAGE-EVICTED 观测被无声击穿）。
    #[test]
    fn t8_pull_out_of_range_returns_err_and_does_not_poison_eviction() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // ① 新槽（无分页响应存续）上的 Pull：结构化 ERR，不得为空。
        let fresh = step(&mut t, 5, &pull_frame(0, 0), &mut clock);
        assert_eq!(
            fresh.response,
            b"ERR:PageRange".to_vec(),
            "无分页响应的 Pull 必须结构化拒绝（空响应 = 客户端挂死面）"
        );
        // ② 2 页响应只拉第 0 页，再越界 pull(200)。
        let _ = step(&mut t, 6, &header_frame(150, 2, 6), &mut clock);
        for i in 0..2 {
            let _ = step(&mut t, 6, &data_frame(i, 0x11), &mut clock);
        }
        let _ = step(&mut t, 6, &final_frame(), &mut clock);
        let ok0 = step(&mut t, 6, &pull_frame(0, 6), &mut clock);
        assert_eq!(
            decode_page_frame(&ok0.response).map(|(l, _, _)| l),
            Some(RESP_PAGE_MAX)
        );
        let oob = step(&mut t, 6, &pull_frame(200, 6), &mut clock);
        assert_eq!(
            oob.response,
            b"ERR:PageRange".to_vec(),
            "越界 Pull 必须结构化拒绝"
        );
        // ③ 覆盖性 Final 的 PAGE-EVICTED 不得被越界帧毒化（只拉了 1/2 页）。
        let _ = step(&mut t, 6, &header_frame(10, 1, 7), &mut clock);
        let _ = step(&mut t, 6, &data_frame(0, 0x22), &mut clock);
        let out = step(&mut t, 6, &final_frame(), &mut clock);
        assert_eq!(
            t.page_evictions(),
            1,
            "越界 Pull 不得推进 pages_pulled 击穿驱逐观测"
        );
        assert!(out.lines.iter().any(|l| l.contains("CHUNKED-PAGE-EVICTED")));
    }

    // ---- t9：传输中 Final 须拒帧（F3 陈旧 pending_full 错配） ------------

    /// 68 三轮复查 t9（红测先行）：上一传输完成后未发 Final，客户端即开
    /// 新传输且块未齐就发 Final——此时 rx 非空闲，Final 必须被拒（红=
    /// take() 取到上一传输的陈旧载荷当本次请求处理，回合法 ENVP）。拒帧
    /// 后在途传输不受损：补齐剩余块 + 重发 Final 正常完成。
    #[test]
    fn t9_final_mid_transfer_rejected_and_transfer_survives() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // A：完成（pending_full = A 的 150B 载荷），但 Final 尚未发。
        let _ = step(&mut t, 9, &header_frame(150, 2, 9), &mut clock);
        for i in 0..2 {
            let _ = step(&mut t, 9, &data_frame(i, 0x11), &mut clock);
        }
        // B：新传输在途（块未齐）就发 Final → 拒帧。
        let _ = step(&mut t, 9, &header_frame(150, 2, 10), &mut clock);
        let _ = step(&mut t, 9, &data_frame(0, 0x22), &mut clock);
        let early = step(&mut t, 9, &final_frame(), &mut clock);
        assert_eq!(
            early.response,
            b"ERR:TransferIncomplete".to_vec(),
            "传输中 Final 必须拒帧（红=陈旧载荷被当本次请求处理）"
        );
        // B 传输不受损：补齐第 1 块 + Final → 正常处理 B 的载荷（0x22）。
        let _ = step(&mut t, 9, &data_frame(1, 0x22), &mut clock);
        let fin = step(&mut t, 9, &final_frame(), &mut clock);
        assert!(
            fin.response.len() >= 4 && fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2,
            "补齐后 Final 须正常完成: {:?}",
            fin.response
        );
        let page0 = step(&mut t, 9, &pull_frame(0, 10), &mut clock);
        let (_, _, data) = decode_page_frame(&page0.response).expect("判别页帧");
        assert!(
            data.iter().all(|&b| b == 0x22),
            "Final 须处理 B 的载荷（红=处理的是 A 的陈旧载荷）"
        );
    }

    // ---- t10：rx 空闲但 pending_full 陈旧，Final 不得以成功形态错配 ----

    /// 69.1 WP-69.1.0 红测（BUG-S2 服务端半边，01 §3.2 S-1 的仓内真测；
    /// t9 只覆盖 rx 非空闲的一半）：A 完成后 Final 丢失，B 的 Header 被
    /// 接受（rx idle ⇒ 无 DUP-SESSION 拦截），B 的 Data(0) 重传 ⇒
    /// BadIndex ⇒ Malformed ⇒ rx 复位 idle——B 的 Final 通过 F3 闸门
    /// （rx_idle）。红 = take() 取到 A 的陈旧载荷当本次请求处理，回
    /// **合法 ENVP**（静默错误成功）；绿 = ERR:NoTransfer 显式拒绝。
    #[test]
    fn t10_stale_pending_full_not_served_after_malformed_reset() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // A：完成（pending_full = Some(A)），Final 不发（丢失/超时）。
        assert!(ack(&step(&mut t, 10, &header_frame(150, 2, 9), &mut clock)));
        let _ = step(&mut t, 10, &data_frame(0, 0x11), &mut clock);
        let _ = step(&mut t, 10, &data_frame(1, 0x11), &mut clock);
        // B：Header 被接受（rx idle ⇒ 无 DUP-SESSION）。
        assert!(ack(&step(
            &mut t,
            10,
            &header_frame(150, 2, 10),
            &mut clock
        )));
        let _ = step(&mut t, 10, &data_frame(0, 0x22), &mut clock);
        // Data(0) 重传 → BadIndex ⇒ Malformed ⇒ rx 复位 idle。
        let _ = step(&mut t, 10, &data_frame(0, 0x22), &mut clock);
        // B 的 Final：【红】陈旧 pending_full(A) 被当 B 的请求处理。
        let fin = step(&mut t, 10, &final_frame(), &mut clock);
        assert!(
            !(fin.response.len() >= 4 && fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2),
            "陈旧 pending_full 不得以成功形态（ENVP）错配给新传输: {:?}",
            fin.response
        );
        assert_eq!(
            fin.response,
            b"ERR:NoTransfer".to_vec(),
            "B 的 Final 必须显式拒绝（take() = None）"
        );
    }

    // ---- t10b/t10c/t10d：BUG-S2 修复的变异锚（69.1 WP-69.1.1） ----------

    /// t10b（变异靶，**现状即绿**）：A 完成无 Final → Header(B) 被接受后
    /// **直接** Final(B)（无任何 Data/Malformed）——rx 非空闲 ⇒ F3 闸门拒绝。
    /// 它不是 BUG-S2 的红测：它锁的是"清理点挪位不得破坏 F3 闸门"
    /// （卷 69.1 §WP-69.1.1 变异表；t10b 绿不代表修好了）。
    #[test]
    fn t10b_header_then_immediate_final_hits_f3_gate() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        assert!(ack(&step(&mut t, 11, &header_frame(150, 2, 1), &mut clock)));
        let _ = step(&mut t, 11, &data_frame(0, 0x11), &mut clock);
        let _ = step(&mut t, 11, &data_frame(1, 0x11), &mut clock);
        // B：Header 被接受（rx idle）后直接 Final——块未齐。
        assert!(ack(&step(&mut t, 11, &header_frame(150, 2, 2), &mut clock)));
        let fin = step(&mut t, 11, &final_frame(), &mut clock);
        assert_eq!(
            fin.response,
            b"ERR:TransferIncomplete".to_vec(),
            "F3 闸门必须拒绝传输中 Final（清理点挪位不得破坏本闸门）"
        );
    }

    /// t10c（BUG-S2 变体红测 + 变异锚）：与 t10 同型，但 B 的**第一帧
    /// Data 即违例**（index=5 ≠ 0 ⇒ BadIndex ⇒ Malformed，全程无合法
    /// Data 事件）。红 = 陈旧 pending_full(A) 错配（ENVP）；绿 =
    /// ERR:NoTransfer。同时咬"清理点挪到 Data 事件臂"的变异——本序列
    /// 无任何 Data 事件，挪位即漏（t10 的合法 Data(0) 会发 Data 事件，
    /// 咬不住该变异）。
    #[test]
    fn t10c_first_data_frame_bad_index_stale_pending_not_served() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        assert!(ack(&step(&mut t, 12, &header_frame(150, 2, 1), &mut clock)));
        let _ = step(&mut t, 12, &data_frame(0, 0x11), &mut clock);
        let _ = step(&mut t, 12, &data_frame(1, 0x11), &mut clock);
        assert!(ack(&step(&mut t, 12, &header_frame(150, 2, 2), &mut clock)));
        // 第一帧 Data 即违例（index=5）→ BadIndex ⇒ Malformed ⇒ rx 复位。
        let _ = step(&mut t, 12, &data_frame(5, 0x22), &mut clock);
        let fin = step(&mut t, 12, &final_frame(), &mut clock);
        assert!(
            !(fin.response.len() >= 4 && fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2),
            "陈旧 pending_full 不得错配（无 Data 事件序列）: {:?}",
            fin.response
        );
        assert_eq!(fin.response, b"ERR:NoTransfer".to_vec());
    }

    /// t10d（行为锁）：Complete 后、Final 前夹杂一条**无传输在途**的杂散
    /// Data（NoTransfer Malformed，Send 回执客户端不读）——不得清除
    /// pending_full：A 的 Final 仍须拿到 A 的响应。咬"清理点挪到
    /// Malformed 臂"的变异（该挪位会在此杀掉合法挂起的响应）。
    #[test]
    fn t10d_junk_data_between_complete_and_final_keeps_pending() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        assert!(ack(&step(&mut t, 13, &header_frame(150, 2, 1), &mut clock)));
        let _ = step(&mut t, 13, &data_frame(0, 0x11), &mut clock);
        let _ = step(&mut t, 13, &data_frame(1, 0x11), &mut clock);
        // 杂散 Data（无传输在途）→ NoTransfer Malformed（回执不可读）。
        let _ = step(&mut t, 13, &data_frame(0, 0x99), &mut clock);
        // A 的 Final：必须照常处理 A 的载荷。
        let fin = step(&mut t, 13, &final_frame(), &mut clock);
        assert!(
            fin.response.len() >= 4 && fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2,
            "杂散 Data 不得杀死挂起的 pending_full: {:?}",
            fin.response
        );
        let page0 = step(&mut t, 13, &pull_frame(0, 1), &mut clock);
        let (_, _, data) = decode_page_frame(&page0.response).expect("判别页帧");
        assert!(
            data.iter().all(|&b| b == 0x11),
            "Final 须处理 A 的载荷（红=被杂散帧清场）"
        );
    }

    // ---- t11：越界 Pull 拒绝必须可观测、可记账（O1） ----------------------

    /// 69.1 WP-69.1.0 红测（BUG-S1 配套观测面，01 §3.3 O1）：越界 Pull 的
    /// `ERR:PageRange` 拒绝路径此前零计数零词形（第 4 个改变页序状态却
    /// 不记账的盲点）。① 拒绝词形本身 t8 已锁不重复；② 计数
    /// `page_range_rejections`；③ 词形 `CHUNKED-PAGE-RANGE slot=<n>`。
    /// 红 = 计数 0 且无词形（运维不可见）。
    #[test]
    fn t11_page_range_reject_is_observable_and_accounted() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // 新槽（无分页响应存续）上的越界 Pull → 结构化拒绝（t8① 同型）。
        let oob = step(&mut t, 14, &pull_frame(0, 0), &mut clock);
        assert_eq!(oob.response, b"ERR:PageRange".to_vec());
        assert_eq!(
            t.page_range_rejections(),
            1,
            "越界 Pull 拒绝必须记账（红=现状零计数）"
        );
        assert!(
            oob.lines
                .iter()
                .any(|l| l.contains("CHUNKED-PAGE-RANGE slot=0")),
            "PAGE-RANGE 词形缺失（红=现状运维不可见）: {:?}",
            oob.lines
        );
        // 计数累计不受限；词形行每分钟每词形至多 1 行。
        let oob2 = step(&mut t, 14, &pull_frame(3, 0), &mut clock);
        assert_eq!(t.page_range_rejections(), 2);
        assert!(
            !oob2.lines.iter().any(|l| l.contains("CHUNKED-PAGE-RANGE")),
            "词形行必须每分钟每词形至多 1 行"
        );
    }

    // ---- t12：覆盖性 Final 自愈重同步 + 计数（反活锁锁，69.1 订正版） --

    /// 69.1 WP-69.1.2（t12 重写）：A 响应未拉完时，覆盖性 Final（同 badge
    /// 客户端超时放弃半拉响应后的**整体重试**）必须照常安装新响应（ENVP）
    /// 且驱逐计数/词形保持——覆盖 = 与重试客户端（持新头）页序重新同步。
    /// **本测锁死自愈语义，防"拒绝覆盖"回潮**：VOL69.1-GATE 实弹证伪
    /// 卷册 B-1 拒绝式——拒绝 = 重试的新 Final 反复被拒 → 活锁（
    /// ServerRejected×4050 / energy-agent tick 停 0 / svc-telemetry 心跳
    /// 饿死被杀，证据 evidence/69/vol69.1）。
    #[test]
    fn t12_covering_final_self_heals_resync_and_counts() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // A：150B（2 页），只拉第 0 页后客户端"超时放弃"。
        let _ = step(&mut t, 16, &header_frame(150, 2, 1), &mut clock);
        for i in 0..2 {
            let _ = step(&mut t, 16, &data_frame(i, 0x11), &mut clock);
        }
        let _ = step(&mut t, 16, &final_frame(), &mut clock);
        let p0 = step(&mut t, 16, &pull_frame(0, 1), &mut clock);
        assert_eq!(
            decode_page_frame(&p0.response).map(|(l, _, _)| l),
            Some(RESP_PAGE_MAX)
        );
        // 重试：B 覆盖性传输 + Final——必须成功安装（不得拒）。
        let _ = step(&mut t, 16, &header_frame(10, 1, 2), &mut clock);
        let _ = step(&mut t, 16, &data_frame(0, 0x22), &mut clock);
        let fin = step(&mut t, 16, &final_frame(), &mut clock);
        assert!(
            fin.response.len() >= 4 && fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2,
            "覆盖性 Final 必须自愈安装新响应（拒绝 = 活锁面）: {:?}",
            fin.response
        );
        assert_eq!(t.page_evictions(), 1, "驱逐计数保持（超时重试热点观测）");
        assert!(
            fin.lines.iter().any(|l| l.contains("CHUNKED-PAGE-EVICTED")),
            "PAGE-EVICTED 词形保持"
        );
        // 页序已与重试客户端重同步：Pull(0) = B 的 10B 页（不再是 A 的 104B）。
        let pb = step(&mut t, 16, &pull_frame(0, 2), &mut clock);
        let (plen, _, data) = decode_page_frame(&pb.response).expect("判别页帧");
        assert_eq!(plen, 10, "页序必须重同步到新响应");
        assert!(data.iter().all(|&b| b == 0x22), "须是 B 的页");
    }

    // ---- t13/t13b：单帧回执 MR 预算（I3）---------------------------------

    /// 69.1 WP-69.1.3：`ERR:{e:?}`（Malformed 臂）长度由 Debug 派生决定，
    /// 不由字面量界定——穷举全部变体断言 ≤ `MR_REPLY_BUDGET_BYTES`（112B）。
    /// 穷举 match（无通配）= agent-sdk 新增变体漏枚举即编译失败（E0004）：
    /// 编译期 + 运行期双锁（新增变体必须同时考虑 MR 预算）。
    #[test]
    fn t13_all_chunk_error_debug_forms_fit_mr_budget() {
        use eneros_agent_sdk::{ChunkError, MR_REPLY_BUDGET_BYTES};
        fn assert_exhaustive(e: ChunkError) {
            match e {
                ChunkError::BadMagic
                | ChunkError::BadLength(_)
                | ChunkError::BadIndex { .. }
                | ChunkError::TooLarge { .. }
                | ChunkError::TooManyChunks(_)
                | ChunkError::LengthMismatch { .. }
                | ChunkError::Busy
                | ChunkError::NoTransfer
                | ChunkError::Transport
                | ChunkError::ServerRejected
                | ChunkError::BadTotalLen(_)
                | ChunkError::BadChunkSize(_) => {}
            }
        }
        // 逐变体按最长 Debug 形态构造（数值域取上界）。
        let cases = [
            ChunkError::BadMagic,
            ChunkError::BadLength(usize::MAX),
            ChunkError::BadIndex {
                got: u8::MAX,
                expected: u8::MAX,
            },
            ChunkError::TooLarge { len: usize::MAX },
            ChunkError::TooManyChunks(u8::MAX),
            ChunkError::LengthMismatch {
                total: u32::MAX,
                chunks: u8::MAX,
            },
            ChunkError::Busy,
            ChunkError::NoTransfer,
            ChunkError::Transport,
            ChunkError::ServerRejected,
            ChunkError::BadTotalLen(u32::MAX),
            ChunkError::BadChunkSize(u16::MAX),
        ];
        for e in cases {
            assert_exhaustive(e);
            let wire = format!("ERR:{e:?}").into_bytes();
            assert!(
                wire.len() <= MR_REPLY_BUDGET_BYTES,
                "ERR:{e:?} = {}B 超 MR 回执预算 {MR_REPLY_BUDGET_BYTES}B（该变体须改定长文案）",
                wire.len()
            );
        }
    }

    /// t13b（I3 运行时兜底白盒测）：step() 出口的超预算回执必须显式替换
    /// 拒绝（计数 + 词形），不得静默截断。正常路径静态不可达 ⇒ 白盒直调
    /// 钳制面构造 200B 回执。
    #[test]
    fn t13b_reply_budget_clamp_replaces_oversized_response() {
        let mut t = SlotTable::new();
        let mut over = StepOut {
            response: alloc::vec![0xAB; 200],
            lines: Vec::new(),
        };
        t.clamp_reply_budget(&mut over, 1_000);
        assert_eq!(
            over.response,
            b"ERR:ReplyTooLarge".to_vec(),
            "超预算回执必须整帧替换为定长拒绝（红=静默截断）"
        );
        assert_eq!(t.reply_truncations(), 1);
        assert!(
            over.lines
                .iter()
                .any(|l| l.contains("CHUNKED-REPLY-TRUNCATED len=200")),
            "REPLY-TRUNCATED 词形缺失: {:?}",
            over.lines
        );
        // 预算内回执原样通过（不计数）。
        let mut ok = StepOut {
            response: b"ERR:PageRange".to_vec(),
            lines: Vec::new(),
        };
        t.clamp_reply_budget(&mut ok, 1_000);
        assert_eq!(ok.response, b"ERR:PageRange".to_vec());
        assert_eq!(t.reply_truncations(), 1, "预算内回执不得计数");
    }

    // ---- t14：响应超 u8 页序域界（255×104=26520B）须显式拒绝（I4） ----

    /// 69.1 WP-69.1.4 红测（REG-CHUNKED-PAGE-U8-CAP）：响应方向页数原
    /// 无上限（`set_paged` 的 `as u8` 与 `encode_paged_header` 的
    /// `chunks as u8` 均无检查，>255 页静默回绕）。红 = 26521B（256 页）
    /// 照常分页（页数回绕）；绿 = `ERR:ResponseTooLarge` + 计数 + 词形。
    /// 边界段：26520B（255 页恰满 u8 域）必须照常 ENVP——svc-config
    /// 7099B 配置分发等既有生产面在界内不受扰（VOL69.1-GATE 实弹教训：
    /// 卷册原拟 16 页误界曾误拒 68 页配置束，级联打断全窗采集/决策链）。
    #[test]
    fn t14_response_over_page_cap_rejected_not_truncated() {
        struct FixedLen(usize);
        impl ControlServiceHandler for FixedLen {
            fn handle(&mut self, _req: &[u8], resp: &mut Vec<u8>, _now_ms: u64) -> bool {
                resp.resize(self.0, 0x5A);
                true
            }
        }
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        assert!(ack(&step(&mut t, 15, &header_frame(10, 1, 1), &mut clock)));
        let _ = step(&mut t, 15, &data_frame(0, 0x11), &mut clock);
        let out = {
            let mut nowfn = || clock.now();
            t.step(15, &final_frame(), &mut FixedLen(26521), &mut nowfn)
        };
        assert_eq!(
            out.response,
            b"ERR:ResponseTooLarge".to_vec(),
            "超 u8 页序域界的响应必须显式拒绝（红=照常 ENVP 分页/回绕）"
        );
        assert_eq!(t.resp_too_large(), 1);
        assert!(
            out.lines
                .iter()
                .any(|l| l.contains("CHUNKED-RESP-TOOLARGE slot=0")),
            "RESP-TOOLARGE 词形缺失: {:?}",
            out.lines
        );
        // 边界内照常分页（26520B = 255 页，恰满 u8 域界）。
        let _ = step(&mut t, 15, &header_frame(10, 1, 2), &mut clock);
        let _ = step(&mut t, 15, &data_frame(0, 0x22), &mut clock);
        let out2 = {
            let mut nowfn = || clock.now();
            t.step(15, &final_frame(), &mut FixedLen(26520), &mut nowfn)
        };
        assert!(
            out2.response.len() >= PAGED_HEADER_V2_LEN
                && out2.response[0..4] == *CHUNK_MAGIC_PAGED_V2,
            "恰在页数上限内的响应不得被拒: {:?}",
            out2.response
        );
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
                .any(|l| l.starts_with("CHUNKED-SLOW call=FINAL dur_ms=")),
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

    // ---- 蓝图 70.3 WP-70.3.1：进程字节预算准入 --------------------------

    /// 预算耗尽：占满预算的挂起请求使新 Header 被拒（ERR:BudgetExceeded +
    /// 计数 + 词形）；回收释放后合法调用成功（资源错误而非 OOM，可恢复）。
    #[test]
    fn budget_rejects_header_when_full_and_recovers_after_reclaim() {
        let mut clock = Clock(1_000);
        // 预算 = 2.5 个 300B 请求（小预算可测；生产默认 64KiB 同路径）。
        let mut t = SlotTable::with_byte_budget(750);
        // badge 1/2：各挂 300B 完整请求（pending_full 300B × 2 = 600B）。
        for (b, fill) in [(1u64, 0x11u8), (2, 0x22)] {
            assert!(ack(&step(&mut t, b, &header_frame(300, 3, b), &mut clock)));
            for i in 0..3 {
                let _ = step(&mut t, b, &data_frame(i, fill), &mut clock);
            }
        }
        assert_eq!(t.bytes_in_use(), 600);
        // badge 3：300B 新传输 → 超预算拒绝（600+300 > 750）。
        let rejected = step(&mut t, 3, &header_frame(300, 3, 3), &mut clock);
        assert_eq!(rejected.response, b"ERR:BudgetExceeded".to_vec());
        assert_eq!(t.budget_rejections(), 1);
        assert!(
            rejected
                .lines
                .iter()
                .any(|l| l.contains("CHUNKED-BUDGET badge=3 need=300")),
            "BUDGET 词形缺失: {:?}",
            rejected.lines
        );
        // 被拒 badge 不占槽、不产生组装驻留。
        assert_eq!(t.occupied(), 2);
        // 界内小传输仍可入（300+600=900 > 750 会拒；200+600=800 仍拒；
        // 150+600=750 = 恰到界 → 允许）。
        let edge = step(&mut t, 3, &header_frame(150, 2, 3), &mut clock);
        assert!(ack(&edge), "恰在预算内的传输必须接纳");
        // 回收路径：时钟推过孤儿时限 + 下一事件触发扫描 → 槽释放 →
        // 300B 传输重新可入。
        clock.advance(SLOT_ORPHAN_TTL_MS + 1);
        let _ = step(&mut t, 4, &header_frame(10, 1, 4), &mut clock);
        assert!(t.slots_reclaimed() >= 3, "全部孤儿槽应被回收");
        assert_eq!(t.occupied(), 1);
        let retry = step(&mut t, 5, &header_frame(300, 3, 5), &mut clock);
        assert!(ack(&retry), "预算释放后合法传输必须成功");
    }

    /// 响应安装预算：handler 产出超预算响应 → ERR:BudgetExceeded（与
    /// 域界 ERR:ResponseTooLarge 分立）；旧响应被替换释放的部分计入。
    #[test]
    fn budget_rejects_oversized_response_install() {
        struct FixedLen(usize);
        impl ControlServiceHandler for FixedLen {
            fn handle(&mut self, _req: &[u8], resp: &mut Vec<u8>, _now_ms: u64) -> bool {
                resp.resize(self.0, 0x5A);
                true
            }
        }
        let mut clock = Clock(1_000);
        let mut t = SlotTable::with_byte_budget(4_096);
        assert!(ack(&step(&mut t, 1, &header_frame(10, 1, 1), &mut clock)));
        let _ = step(&mut t, 1, &data_frame(0, 0x11), &mut clock);
        // 请求 5KiB 响应（页域界 26520 内、进程预算 4KiB 超）→ 预算拒绝。
        let out = {
            let mut nowfn = || clock.now();
            t.step(1, &final_frame(), &mut FixedLen(5 * 1024), &mut nowfn)
        };
        assert_eq!(out.response, b"ERR:BudgetExceeded".to_vec());
        assert_eq!(t.budget_rejections(), 1);
        assert!(
            out.lines.iter().any(|l| l.contains("CHUNKED-BUDGET")),
            "BUDGET 词形缺失: {:?}",
            out.lines
        );
        // 旧响应替换：同槽先装 2KiB（预算内），再装 3KiB（2KiB 释放后
        // 仍超 4KiB−2KiB 余量？不——释放后 3KiB ≤ 4KiB）→ 必须成功。
        let _ = step(&mut t, 1, &header_frame(10, 1, 2), &mut clock);
        let _ = step(&mut t, 1, &data_frame(0, 0x22), &mut clock);
        let out2 = {
            let mut nowfn = || clock.now();
            t.step(1, &final_frame(), &mut FixedLen(2 * 1024), &mut nowfn)
        };
        assert!(
            out2.response.len() >= PAGED_HEADER_V2_LEN
                && out2.response[0..4] == *CHUNK_MAGIC_PAGED_V2,
            "释放旧响应后安装必须成功: {:?}",
            out2.response
        );
        assert_eq!(t.budget_rejections(), 1);
    }

    // ---- 蓝图 70.3 WP-70.3.3：txn 事务归属 ------------------------------

    /// 发散源 #1 关闭：覆盖性 Final 安装新响应（txn=2）后，旧客户端
    /// （txn=1）续拉必须被拒（ERR:PullStale + 计数 + 词形）；新事务
    /// （txn=2）照常供页。t12 的自愈重同步语义不受影响。
    #[test]
    fn stale_txn_pull_rejected_after_covering_final() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // A：300B（3 页，epoch=1），不拉。
        let _ = step(&mut t, 1, &header_frame(300, 3, 1), &mut clock);
        for i in 0..3 {
            let _ = step(&mut t, 1, &data_frame(i, 0x11), &mut clock);
        }
        let _ = step(&mut t, 1, &final_frame(), &mut clock);
        // B：覆盖性传输（epoch=2）+ Final → 新响应安装（自愈重同步）。
        let _ = step(&mut t, 1, &header_frame(10, 1, 2), &mut clock);
        let _ = step(&mut t, 1, &data_frame(0, 0x22), &mut clock);
        let fin = step(&mut t, 1, &final_frame(), &mut clock);
        assert!(fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2, "自愈语义保持");
        // 旧客户端续拉（txn=1）→ 拒绝 + 记账。
        let stale = step(&mut t, 1, &pull_frame(0, 1), &mut clock);
        assert_eq!(stale.response, b"ERR:PullStale".to_vec());
        assert_eq!(t.pull_txn_rejections(), 1);
        assert!(
            stale
                .lines
                .iter()
                .any(|l| l.contains("CHUNKED-PULL-STALE slot=0")),
            "PULL-STALE 词形缺失: {:?}",
            stale.lines
        );
        // 新事务（txn=2）照常供页。
        let fresh = step(&mut t, 1, &pull_frame(0, 2), &mut clock);
        let (_, _, data) = decode_page_frame(&fresh.response).expect("判别页帧");
        assert!(data.iter().all(|&b| b == 0x22), "新事务页必须可供");
    }

    /// v1 遗留 5B Pull 形（无 txn）= 协议违例：拒绝 + 记账（硬退役，
    /// 非静默兼容）。
    #[test]
    fn legacy_5byte_pull_rejected() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        assert!(ack(&step(&mut t, 1, &header_frame(10, 1, 1), &mut clock)));
        let _ = step(&mut t, 1, &data_frame(0, 0x11), &mut clock);
        let mut legacy = eneros_agent_sdk::PULL_MAGIC.to_vec();
        legacy.push(0); // v1 形 = magic + index（无 txn 字段）
        let out = step(&mut t, 1, &legacy, &mut clock);
        assert_eq!(out.response, b"ERR:PullStale".to_vec());
        assert_eq!(t.pull_txn_rejections(), 1);
    }

    // ---- 蓝图 70.3 WP-70.3.4：孤儿/空闲槽回收与状态机 -------------------

    /// Header 后死亡（孤儿）：时限过后的下一事件回收整槽；迟到 Data(0)
    /// 与旧 Pull 均被拒；新会话立即重新工作。
    #[test]
    fn orphan_header_then_silence_reclaimed_and_slot_reusable() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        assert!(ack(&step(&mut t, 1, &header_frame(300, 3, 1), &mut clock)));
        let _ = step(&mut t, 1, &data_frame(0, 0x11), &mut clock);
        assert_eq!(t.occupied(), 1);
        // 时限内：不回收。
        clock.advance(SLOT_ORPHAN_TTL_MS);
        let _ = step(&mut t, 2, &header_frame(10, 1, 2), &mut clock);
        assert_eq!(t.slots_reclaimed(), 0, "时限内不得回收");
        // 超时（+1ms）后下一事件：badge 1 槽回收。
        clock.advance(1);
        let _ = step(&mut t, 2, &data_frame(0, 0x22), &mut clock);
        assert_eq!(t.slots_reclaimed(), 1);
        assert_eq!(t.occupied(), 1, "只剩 badge 2 槽");
        // 回收后的迟到帧：badge 1 的 Data(0) → NoSlot（无会话可归，Send 回执丢弃）；Pull → ERR:NoSlot。
        let _ = step(&mut t, 1, &data_frame(0, 0xAA), &mut clock);
        assert_eq!(
            t.occupied(),
            2,
            "Data 帧重建槽（原 24 槽语义保持——no-op 传输）"
        );
        // 新会话立即重新工作（Header→Data→Final→Pull 全链）。
        let t2_slot = {
            // 用 badge 1 重新完整走一轮（Header 覆盖刚由杂散 Data 重建的空槽）。
            assert!(ack(&step(&mut t, 1, &header_frame(10, 1, 9), &mut clock)));
            let _ = step(&mut t, 1, &data_frame(0, 0xBB), &mut clock);
            let fin = step(&mut t, 1, &final_frame(), &mut clock);
            assert!(fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2);
            step(&mut t, 1, &pull_frame(0, 9), &mut clock)
        };
        let (_, _, data) = decode_page_frame(&t2_slot.response).expect("判别页帧");
        assert!(data.iter().all(|&b| b == 0xBB), "回收后新会话必须完整工作");
    }

    /// Complete 后无 Final（孤儿）：挂起载荷在时限后随槽释放；
    /// 半拉响应同理（pages_pulled < total = 在途工作 → 孤儿时限）。
    #[test]
    fn orphan_pending_and_half_pulled_response_reclaimed() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        // Complete 后 Final 丢失。
        assert!(ack(&step(&mut t, 1, &header_frame(150, 2, 1), &mut clock)));
        let _ = step(&mut t, 1, &data_frame(0, 0x11), &mut clock);
        let _ = step(&mut t, 1, &data_frame(1, 0x11), &mut clock);
        // 半拉响应（badge 2）。
        assert!(ack(&step(&mut t, 2, &header_frame(150, 2, 2), &mut clock)));
        for i in 0..2 {
            let _ = step(&mut t, 2, &data_frame(i, 0x22), &mut clock);
        }
        let _ = step(&mut t, 2, &final_frame(), &mut clock);
        let _ = step(&mut t, 2, &pull_frame(0, 2), &mut clock);
        assert_eq!(t.occupied(), 2);
        clock.advance(SLOT_ORPHAN_TTL_MS + 1);
        let _ = step(&mut t, 3, &header_frame(10, 1, 3), &mut clock);
        assert_eq!(t.slots_reclaimed(), 2, "挂起/半拉两类孤儿均回收");
        // 回收后 badge 2 旧 Pull → 杂散帧建空槽（txn=0）后按 txn 失配
        // 拒绝——不得供出任何旧页（PageRange/PullStale 均为结构化拒绝）。
        let old = step(&mut t, 2, &pull_frame(1, 2), &mut clock);
        assert_eq!(old.response, b"ERR:PullStale".to_vec());
        assert_eq!(t.occupied(), 2, "badge3 + 杂散帧重建的空槽");
    }

    /// 纯缓存槽（事务完成、页已拉完）按空闲时限回收；回收先于孤儿
    /// 时限（60s vs 300s——释放缓存更积极，未完成工作给更长宽限）。
    #[test]
    fn fully_consumed_slot_reclaimed_on_idle_ttl() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        assert!(ack(&step(&mut t, 1, &header_frame(150, 2, 1), &mut clock)));
        for i in 0..2 {
            let _ = step(&mut t, 1, &data_frame(i, 0x11), &mut clock);
        }
        let _ = step(&mut t, 1, &final_frame(), &mut clock);
        // 全部页拉完 → 纯缓存。
        let _ = step(&mut t, 1, &pull_frame(0, 1), &mut clock);
        let _ = step(&mut t, 1, &pull_frame(1, 1), &mut clock);
        // 空闲时限内不回收（拉第 1 页就发生在 1_000 时刻）。
        clock.advance(SLOT_IDLE_TTL_MS);
        let _ = step(&mut t, 2, &header_frame(10, 1, 2), &mut clock);
        assert_eq!(t.slots_reclaimed(), 0);
        clock.advance(1);
        let _ = step(&mut t, 2, &data_frame(0, 0x22), &mut clock);
        assert_eq!(t.slots_reclaimed(), 1, "纯缓存槽按空闲时限回收");
        assert_eq!(t.occupied(), 1);
    }

    /// respawn 同 badge：新 SlotTable（服务重启）后旧 txn 的 Pull 无槽
    /// 可归 → ERR:NoSlot（结构化拒绝，不挂死）；后续会话正常。
    #[test]
    fn respawn_same_badge_old_pull_structurally_rejected() {
        let mut clock = Clock(1_000);
        let mut t = SlotTable::new();
        assert!(ack(&step(&mut t, 7, &header_frame(150, 2, 1), &mut clock)));
        for i in 0..2 {
            let _ = step(&mut t, 7, &data_frame(i, 0x11), &mut clock);
        }
        let _ = step(&mut t, 7, &final_frame(), &mut clock);
        // 服务重启：全新 SlotTable。
        let t2 = SlotTable::new();
        let _ = t;
        let mut t = t2;
        let old = step(&mut t, 7, &pull_frame(0, 1), &mut clock);
        assert_eq!(
            old.response,
            b"ERR:PullStale".to_vec(),
            "respawn 后旧 txn 的 Pull 结构化拒绝（txs 失配——不供旧页）"
        );
        assert_eq!(t.occupied(), 1, "杂散 Pull 重建空槽（txn=0，预存语义）");
        // 后续合法会话照常。
        assert!(ack(&step(&mut t, 7, &header_frame(10, 1, 2), &mut clock)));
        let _ = step(&mut t, 7, &data_frame(0, 0x22), &mut clock);
        let fin = step(&mut t, 7, &final_frame(), &mut clock);
        assert!(fin.response[0..4] == *CHUNK_MAGIC_PAGED_V2);
    }
}
