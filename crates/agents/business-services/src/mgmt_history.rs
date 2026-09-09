//! SPEC-C（WP-C.2）：history-query 执行器内核——分页取数组装（单一实现，
//! host/target 共用；RawIpc 泛型宿主注入内存对端）。
//!
//! 管道：svc-mgmt executor ←(arg: point‖last_n)→ 本模块经
//! [`eneros_agent_sdk::endpoint::ChunkedEndpoint`] 逐页向 svc-telemetry 发
//! TELR 帧（`more` 驱动续页，页大小 = history_wire::PAGE_RECORDS；每页一次
//! 独立分块传输，单线程服务循环天然串行——页间可能插入新的 ingest 批次，
//! 尾部行集随之前移属登记过的可接受语义），拼装为**定长 25B 行的二进制
//! 载荷**。
//!
//! 回程帧纪律：payload 是 MGT 响应的**二进制扩展段**（detail 之后追加
//! `[len u32 LE][bytes]`，svc-mgmt MgmtHandler 负责）——不复用 OBSF 遥测
//! 帧语义、不把记录塞进 detail 文本（C 卷 F-Q3 对账基线保护）。能力边界：
//! 仅最后 N 条（服务端钳制 MAX_LAST_N=512 并回 clamped 旗标）、非全量导出。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::chunked::RawIpc;
use eneros_agent_sdk::endpoint::ChunkedEndpoint;
use eneros_agent_sdk::history_wire::{
    clamp_last_n, decode_response, encode_query, HistQuery, HistResponse, FLAG_CLAMPED, HIST_OK,
    HIST_UNKNOWN_POINT, ROW_LEN,
};

/// 分页取数结果。
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryFetch {
    /// 拼装后的行区二进制（25B × 行数，全局升序）。
    pub rows_bin: Vec<u8>,
    /// 人读摘要（进 ActionResult.detail）。
    pub summary: String,
    /// 服务端精确总行数（该点位跨设备；与钳制窗口无关）。
    pub total: u32,
    /// last_n 超上限被钳制（提示位，透传给 SCADA 面）。
    pub clamped: bool,
}

/// 取数失败（明确错误面，不挂死不伪成功）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryError {
    /// EIPC/分块通道失败（服务死亡/超时/断连）。
    Transport,
    /// svc-telemetry 返回 UNKNOWN_POINT（索引中从未出现该点位）。
    NoSuchPoint,
    /// 协议响应解码失败/异常状态（版本错位、畸形帧、空页自旋防御）。
    BadFrame,
}

/// 核心入口：拉取点位最近 `last_n_raw` 条的全部页并拼装。
///
/// 单线程阻塞逐页拉取；页数天然 ≤ `MAX_LAST_N / PAGE_RECORDS` 上界；
/// 空页立即终止（防御性双条件，不对端异常自旋）。
/// `endpoint` 可变借用（蓝图 67 WP-67.2.3：epoch 单点递增纪律——调用方
/// 持 endpoint，跨 fetch 页序单调延续不重启；原"每 fetch 重建恒 0"面
/// 删除；超时 = 常量默认 5_000，与原全调用方实传值一致）。
pub fn fetch_history<C: RawIpc>(
    endpoint: &mut ChunkedEndpoint<C>,
    target: &'static str,
    point: u32,
    last_n_raw: u32,
) -> Result<HistoryFetch, HistoryError> {
    let (last_n, clamped) = clamp_last_n(last_n_raw);
    let mut out_rows: Vec<u8> = Vec::new();
    let mut total;
    let mut page = 0u32;
    let mut saw_clamped = false;
    loop {
        let req = encode_query(&HistQuery {
            point,
            last_n,
            page,
        });
        let raw = endpoint
            .call(target, &req)
            .map_err(|_| HistoryError::Transport)?;
        let resp: HistResponse = decode_response(&raw).ok_or(HistoryError::BadFrame)?;
        match resp.status {
            HIST_OK => {}
            HIST_UNKNOWN_POINT => return Err(HistoryError::NoSuchPoint),
            _ => return Err(HistoryError::BadFrame),
        }
        if resp.flags & FLAG_CLAMPED != 0 {
            saw_clamped = true;
        }
        // 行区原样拼接（SPEC-A：rows_bytes 行区字节段视图——不再
        // decode 后逐字段 re-encode，25B 行布局物化点收敛到 history_wire）。
        out_rows.extend_from_slice(resp.rows_bytes());
        total = resp.total;
        // more=false 或空页 ⇒ 终止（防御性双条件保留：正确 more 语义下
        // 不该出现空页——真服务端由 history_wire::answer_history_query
        // 按钳制窗口推导，见 SPEC-A；此兜底是跨进程契约最后防线）。
        if !resp.more || resp.rows.is_empty() {
            break;
        }
        page += 1;
    }
    let rows_count = out_rows.len() / ROW_LEN;
    Ok(HistoryFetch {
        rows_bin: out_rows,
        summary: format!(
            "history-query:point={} total={} rows={}",
            point, total, rows_count
        ),
        total,
        clamped: saw_clamped || clamped,
    })
}

// ============================================================================
// SPEC-C（WP-C.2 判据 C1 host）：四臂单测——服务端契约由内存对端仿真
// （与 control_service_main 分块状态机同语义 + history_wire 契约），
// 执行器内核按值消费。真实 target 侧字符串/帧见 QEMU transcript 证据。
// ============================================================================

#[cfg(test)]
mod spec_c_four_arm_tests {
    use eneros_agent_sdk::chunked::{
        ack_bytes, is_pull_request, ChunkHeader, ChunkedEvent, PagedServerV2, CHUNK_MAGIC_FINAL,
        CHUNK_MAGIC_HEADER,
    };
    use eneros_agent_sdk::history_wire::{
        answer_history_query, HistPointWire, HistoryPageSource, ROW_LEN,
    };

    use super::*;

    /// svc-telemetry 服务端仿真：存量行集可配置；**分页/钳制/more 语义不
    /// 再自持**——数据提供经 [`HistoryPageSource`]，响应组装经单源
    /// [`answer_history_query`]（SPEC-A：两套 more 公式漂移在此根治）；
    /// 本仿真只模拟分块传输与断连（RawIpc 职责）。
    struct FakeTelemetry {
        stored: Vec<HistPointWire>,
        disconnect: bool,
        /// 收到的 EIPC 请求计数（行为锁定：钳制窗收满页数断言）。
        requests: alloc::rc::Rc<core::cell::RefCell<usize>>,
        /// 逻辑页数（FINAL 载荷到达即一页）——钳制窗页数收敛断言用。
        pages: alloc::rc::Rc<core::cell::RefCell<usize>>,
        /// 收到的 Header epoch 序列（R-67.4 页序单调断言）。
        epochs: alloc::rc::Rc<core::cell::RefCell<Vec<u64>>>,
        // —— 分块状态机三件套（同 control_service_main；分页帧律 v2
        // 由 PagedServerV2 承载——70.3 收编）——
        rx: eneros_agent_sdk::chunked::ChunkedReceiver,
        pending: Option<Vec<u8>>,
        pages_v2: PagedServerV2,
    }

    impl FakeTelemetry {
        fn new(stored: Vec<HistPointWire>) -> Self {
            Self {
                stored,
                disconnect: false,
                requests: alloc::rc::Rc::new(core::cell::RefCell::new(0)),
                pages: alloc::rc::Rc::new(core::cell::RefCell::new(0)),
                epochs: alloc::rc::Rc::new(core::cell::RefCell::new(Vec::new())),
                rx: Default::default(),
                pending: None,
                pages_v2: PagedServerV2::new(),
            }
        }

        /// 取逻辑页计数句柄（行为锁定：窗口收满页数断言）。
        fn page_counter(&self) -> alloc::rc::Rc<core::cell::RefCell<usize>> {
            alloc::rc::Rc::clone(&self.pages)
        }

        /// 取 Header epoch 序列句柄（R-67.4 页序单调断言）。
        fn epoch_log(&self) -> alloc::rc::Rc<core::cell::RefCell<Vec<u64>>> {
            alloc::rc::Rc::clone(&self.epochs)
        }

        /// 取请求计数句柄（断连负例：恰一次传输尝试）。
        fn requests_handle(&self) -> alloc::rc::Rc<core::cell::RefCell<usize>> {
            alloc::rc::Rc::clone(&self.requests)
        }
    }

    impl HistoryPageSource for FakeTelemetry {
        /// 与真服务端（TsdbHistoryReader）同语义：total=精确全量、窗口按
        /// last_n 钳制、点位 42 之外 → None（UNKNOWN_POINT）。
        fn query_page(
            &self,
            point: u32,
            last_n: usize,
            page: u32,
        ) -> Option<(Vec<HistPointWire>, u32, u32)> {
            if point != 42 {
                return None;
            }
            let total = self.stored.len() as u32;
            let mut rows: Vec<HistPointWire> = self.stored.clone();
            if rows.len() > last_n {
                rows.drain(..rows.len() - last_n);
            }
            let window_len = rows.len() as u32;
            let start = page as usize * eneros_agent_sdk::history_wire::PAGE_RECORDS;
            let end = core::cmp::min(
                start + eneros_agent_sdk::history_wire::PAGE_RECORDS,
                rows.len(),
            );
            let page_rows = if start < rows.len() {
                rows[start..end].to_vec()
            } else {
                Vec::new()
            };
            Some((page_rows, total, window_len))
        }
    }

    impl FakeTelemetry {
        fn answer(&mut self, req: &[u8]) -> Vec<u8> {
            answer_history_query(Some(self), req)
        }
    }

    impl RawIpc for FakeTelemetry {
        fn request(&mut self, _t: &str, payload: &[u8], _to: u64) -> Result<Vec<u8>, AgentErrName> {
            *self.requests.borrow_mut() += 1;
            if self.disconnect {
                return Err(AgentErrName::Timeout);
            }
            if &payload[..4] == CHUNK_MAGIC_HEADER {
                if let Ok(h) = ChunkHeader::decode(payload) {
                    self.epochs.borrow_mut().push(h.epoch);
                }
                let h = ChunkHeader::decode(payload).unwrap();
                if let ChunkedEvent::Header { epoch, .. } = self.rx.on_message(&h.encode(), true) {
                    self.pages_v2.set_txn(epoch);
                }
                return Ok(ack_bytes().to_vec());
            }
            if &payload[..4] == CHUNK_MAGIC_FINAL {
                match self.pending.take() {
                    Some(full) => {
                        *self.pages.borrow_mut() += 1;
                        let resp = self.answer(&full);
                        self.pages_v2.on_request(payload, resp).ok_or_else(|| {
                            AgentErrAlias::IpcError(
                                heapless::String::try_from("paged-reject").unwrap_or_default(),
                            )
                        })
                    }
                    None => Ok(b"ERR:NoTransfer".to_vec()),
                }
            } else if is_pull_request(payload) {
                self.pages_v2
                    .on_request(payload, Vec::new())
                    .ok_or_else(|| {
                        AgentErrAlias::IpcError(
                            heapless::String::try_from("paged-reject").unwrap_or_default(),
                        )
                    })
            } else {
                Err(AgentErrAlias::IpcError(
                    heapless::String::try_from("unexpected").unwrap_or_default(),
                ))
            }
        }

        fn send(&mut self, _t: &str, payload: &[u8]) -> Result<(), AgentErrName> {
            if self.disconnect {
                return Err(AgentErrName::Timeout);
            }
            if let ChunkedEvent::Complete(full) = self.rx.on_message(payload, false) {
                self.pending = Some(full);
            }
            Ok(())
        }
    }

    use eneros_agent_sdk::AgentError as AgentErrAlias;
    type AgentErrName = AgentErrAlias;

    /// 存量构造：point=42，ts=i*10 全局升序，值=i*0.5。
    fn stored_point(n: usize) -> Vec<HistPointWire> {
        (0..n as u64)
            .map(|i| HistPointWire {
                timestamp_ms: i * 10,
                device_id: 7,
                point: 42,
                value: i as f64 * 0.5,
                quality: 0,
            })
            .collect()
    }

    /// 臂①：正常多页取回——行区逐字段一致（经 history_wire 单源解码器
    /// decode_rows，不再手工逐字段解）、total 准确、升序。
    #[test]
    fn arm1_normal_fetch_fieldwise_multi_page() {
        // 120 行 ⇒ 3 页（48×2+24）。
        let mut ep = eneros_agent_sdk::endpoint::ChunkedEndpoint::wrap(FakeTelemetry::new(
            stored_point(120),
        ));
        let f = fetch_history(&mut ep, "svc-telemetry", 42, 200).expect("ok");
        assert_eq!(f.total, 120);
        assert!(!f.clamped);
        assert_eq!(f.rows_bin.len(), 120 * ROW_LEN);
        let rows = eneros_agent_sdk::history_wire::decode_rows(&f.rows_bin).expect("rows decode");
        assert_eq!(rows.len(), 120);
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r.timestamp_ms, i as u64 * 10);
            assert_eq!(r.device_id, 7);
            assert_eq!(r.point, 42);
            assert!((r.value - i as f64 * 0.5).abs() < f64::EPSILON);
            assert_eq!(r.quality, 0);
        }
        assert!(
            f.summary.contains("history-query:point=42"),
            "摘要进 detail：{}",
            f.summary
        );
    }

    /// 臂②：last_n 截断——存量 300 取 40 ⇒ 恰最新 40 行、total=300。
    #[test]
    fn arm2_truncation_takes_newest_window() {
        let mut ep = eneros_agent_sdk::endpoint::ChunkedEndpoint::wrap(FakeTelemetry::new(
            stored_point(300),
        ));
        let f = fetch_history(&mut ep, "svc-telemetry", 42, 40).expect("ok");
        assert_eq!(f.total, 300);
        assert_eq!(f.rows_bin.len(), 40 * ROW_LEN);
        // 首行必须是窗口起点 = 最新 40 条中最早者 ts=(300-40)*10。
        let rows = eneros_agent_sdk::history_wire::decode_rows(&f.rows_bin).expect("rows");
        assert_eq!(rows.first().expect("nonempty").timestamp_ms, 260 * 10);
    }

    /// 臂③：空 point ⇒ NoSuchPoint 明确失败（非空成功、非挂死）。
    #[test]
    fn arm3_unknown_point_is_explicit_error() {
        let mut ep =
            eneros_agent_sdk::endpoint::ChunkedEndpoint::wrap(FakeTelemetry::new(stored_point(4)));
        let err = fetch_history(&mut ep, "svc-telemetry", 77, 16).expect_err("must fail");
        assert_eq!(err, HistoryError::NoSuchPoint);
    }

    /// 臂④：超上限钳制 + 提示——请求 u32::MAX 且存量>512：
    /// 服务端钳至 512 并置旗标；执行器如实透传 clamped=true。
    ///
    /// SPEC-A 行为锁定：more 按窗口推导 → 512 窗口恰收 11 页（10 满页 +
    /// 32 行尾页），**无第 12 页空页往返**（历史病灶按 total=600 推导会
    /// 多付一页空行）。
    #[test]
    fn arm4_over_limit_clamped_with_hint() {
        let peer = FakeTelemetry::new(stored_point(600));
        let pages = peer.page_counter();
        let mut ep = eneros_agent_sdk::endpoint::ChunkedEndpoint::wrap(peer);
        let f = fetch_history(&mut ep, "svc-telemetry", 42, u32::MAX).expect("ok");
        assert!(f.clamped, "必须携带钳制提示");
        // total 契约 = 精确全量（600）；行区窗口 = 钳制上限（512）。
        assert_eq!(f.total, 600);
        assert_eq!(f.rows_bin.len(), 512 * ROW_LEN);
        // 窗口仍是最新端：首行 ts=(600-512)*10。
        let rows = eneros_agent_sdk::history_wire::decode_rows(&f.rows_bin).expect("rows");
        assert_eq!(rows.first().expect("nonempty").timestamp_ms, 88 * 10);
        assert_eq!(
            *pages.borrow(),
            11,
            "512 窗口 = 10 满页 + 1 尾页，more 必须按窗口终止（无空页往返）"
        );
    }

    /// 补充负例：通道断连 → Transport（不挂死、不伪成功）。
    #[test]
    fn transport_failure_maps_to_transport_error() {
        let mut peer = FakeTelemetry::new(stored_point(10));
        peer.disconnect = true;
        let requests = peer.requests_handle();
        let mut ep = eneros_agent_sdk::endpoint::ChunkedEndpoint::wrap(peer);
        let err = fetch_history(&mut ep, "svc-telemetry", 42, 5).expect_err("transport");
        assert_eq!(err, HistoryError::Transport);
        assert_eq!(
            *requests.borrow(),
            1,
            "失败页一次传输尝试即终止（不挂死自旋）"
        );
    }

    /// R-67.4：页序 epoch 严格递增（单 fetch 内 0,1,2…）且**跨 fetch
    /// 延续**不重启 0（蓝图 67 WP-67.2.3——原"每 fetch 重建恒 0"面删除；
    /// 服务端忽略 epoch 值，wire 中性 [01] §2.1）。
    #[test]
    fn page_epochs_strictly_increasing_and_continue_across_fetches() {
        // 120 行 ⇒ 单 fetch 3 页。
        let peer = FakeTelemetry::new(stored_point(120));
        let epochs = peer.epoch_log();
        let mut ep = eneros_agent_sdk::endpoint::ChunkedEndpoint::wrap(peer);
        fetch_history(&mut ep, "svc-telemetry", 42, 200).expect("fetch1");
        assert_eq!(
            *epochs.borrow(),
            alloc::vec![0, 1, 2],
            "单 fetch 页序严格递增"
        );
        fetch_history(&mut ep, "svc-telemetry", 42, 200).expect("fetch2");
        assert_eq!(
            *epochs.borrow(),
            alloc::vec![0, 1, 2, 3, 4, 5],
            "跨 fetch epoch 延续单调（不重启 0）"
        );
    }
}
