//! SPEC-C（WP-C.2）：北向历史取回线格式——TELR 帧（单一事实源）。
//!
//! 管道：SCADA → broker MGT1 帧（`history-query`）→ svc-mgmt 执行器经
//! EIPC 分块通道按本格式向 svc-telemetry 取页；**不复用 OBSF 遥测帧
//! 语义**（对账基线保护，C 卷 F-Q3）。分块传输的 MTU 切片由既有
//! ENV1/ENVD/ENVF+ENVP 协议承担，本层定义的是其载荷语义：
//!
//! 请求帧：`b"TELR" ‖ cmd u8 ‖ point u32 ‖ last_n u32 ‖ page u32`（17B）
//! 响应帧头（23B）+ 行区：
//! ```text
//! b"TELR" ‖ status u8 ‖ flags u8 ‖ point u32 ‖ total u32
//!        ‖ page u32 ‖ page_n u32 ‖ more u8
//! row = ts u64 ‖ device u32 ‖ point u32 ‖ value f64 ‖ quality u8（25B/行）
//! ```
//!
//! 参数防线：[`clamp_last_n`] 上限 [`MAX_LAST_N`]（512，超限钳制 + 返回
//! clamped 旗标）；未知 point 由服务端回 [`HIST_UNKNOWN_POINT`] 错误帧，
//! 不允许空成功。
//!
//! 服务端策略单源：[`answer_history_query`] 把"请求解码→钳制→未知点位→
//! 页游标（[`more_for_page`]）→响应组装"收成一个函数，持 [`HistoryPageSource`]
//! 取数；svc-telemetry 与 mgmt 测试对端共用同一实现——历史病灶"bin 按全量
//! total 推 more、测试替身按窗口推"的两套公式（2026-08-28 体检）在此根治。
//! `more` 一律按**钳制窗口长度**推导，不再按全量 total（避免钳制窗外
//! 空页往返，SPEC-A 卷行为变更登记）。

use alloc::vec::Vec;

/// 帧魔数（请求与响应同源）。
pub const TELR_MAGIC: &[u8; 4] = b"TELR";
/// 查询命令码（末 K 条）。
pub const CMD_QUERY: u8 = 0x01;

/// 单页记录数（23B 头 + 48×25B ≤ 分块通道单传输 1664B 预算）。
pub const PAGE_RECORDS: usize = 48;
/// last_n 上限钳制值（初值=热环容量口径，SPEC-C §3-WP-C.2#3）。
pub const MAX_LAST_N: u32 = 512;

/// 响应状态：成功。
pub const HIST_OK: u8 = 0;
/// 响应状态：坏请求（解码失败/长度不符）。
pub const HIST_BAD_REQUEST: u8 = 1;
/// 响应状态：未知点位（索引中从未出现）→ 明确错误帧而非空成功。
pub const HIST_UNKNOWN_POINT: u8 = 2;
/// 响应状态：历史服务不可用（磁盘适配器未装配/fail-closed）。
pub const HIST_UNAVAILABLE: u8 = 3;

/// flags：last_n 超上限被钳制（客户端应提示）。
pub const FLAG_CLAMPED: u8 = 1;

/// 请求帧定长。
pub const REQ_LEN: usize = 4 + 1 + 4 + 4 + 4;
/// 响应帧头定长。
pub const RESP_HEADER_LEN: usize = 23;
/// 单条记录 wire 定长。
pub const ROW_LEN: usize = 25;

/// 查询请求参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistQuery {
    /// 统一点 ID。
    pub point: u32,
    /// 取最近 N 条（服务端按 [`MAX_LAST_N`] 钳制并回 clamped 旗标）。
    pub last_n: u32,
    /// 页游标（从 0 起；客户端按响应 `more` 续页）。
    pub page: u32,
}

/// 编码查询请求帧。
pub fn encode_query(q: &HistQuery) -> Vec<u8> {
    let mut out = Vec::with_capacity(REQ_LEN);
    out.extend_from_slice(TELR_MAGIC);
    out.push(CMD_QUERY);
    out.extend_from_slice(&q.point.to_le_bytes());
    out.extend_from_slice(&q.last_n.to_le_bytes());
    out.extend_from_slice(&q.page.to_le_bytes());
    out
}

/// 解码查询请求帧（非本协议/长度不符 → None）。
pub fn decode_request(buf: &[u8]) -> Option<HistQuery> {
    if buf.len() != REQ_LEN || &buf[0..4] != TELR_MAGIC || buf[4] != CMD_QUERY {
        return None;
    }
    Some(HistQuery {
        point: u32::from_le_bytes(buf[5..9].try_into().ok()?),
        last_n: u32::from_le_bytes(buf[9..13].try_into().ok()?),
        page: u32::from_le_bytes(buf[13..17].try_into().ok()?),
    })
}

/// 钳制防线：返回 (生效值, 是否被钳制)。
pub fn clamp_last_n(raw: u32) -> (u32, bool) {
    if raw > MAX_LAST_N {
        (MAX_LAST_N, true)
    } else {
        (raw, false)
    }
}

/// 页游标单源：本页之后是否还有行。
///
/// 按**钳制窗口长度**（`min(total, last_n 生效值)` 的实际行集大小）推导，
/// 不按全量 `total`——total 大于窗口时按 total 推导会产生钳制窗外空页，
/// 让客户端多付一次分块往返（SPEC-A 卷修复）。调用方 MUST 传窗口长度
/// （取数实现如实报告），不得传 total。
pub fn more_for_page(page: u32, window_len: usize) -> bool {
    (page as usize + 1).saturating_mul(PAGE_RECORDS) < window_len
}

/// TELR 帧识别单源（服务端路由用：长度+魔数；命令码/字段合法性仍由
/// [`decode_request`] 把关）。
pub fn is_tellr_request(buf: &[u8]) -> bool {
    buf.len() == REQ_LEN && &buf[..4] == TELR_MAGIC
}

/// TELR 页取数提供方（服务端实现；host 测试注入内存对端）。
///
/// 取数实现如实报告三件事：本页行、该点位**精确全量** `total`、钳制窗口
/// 长度（`min(total, last_n)` 的实际行集大小，供 [`more_for_page`]）。
/// 点位从未出现 → `None`（服务端转 [`HIST_UNKNOWN_POINT`] 错误帧）。
pub trait HistoryPageSource {
    fn query_page(
        &self,
        point: u32,
        last_n: usize,
        page: u32,
    ) -> Option<(Vec<HistPointWire>, u32, u32)>;
}

/// TELR 服务端策略单源：请求解码 → 参数防线 → 页游标 → 响应帧组装。
///
/// `src=None` → [`HIST_UNAVAILABLE`]；解码失败 → [`HIST_BAD_REQUEST`]；
/// 点位未知 → [`HIST_UNKNOWN_POINT`]；钳制回 [`FLAG_CLAMPED`] 旗标；
/// `more` 经 [`more_for_page`] 按窗口推导。svc-telemetry bin 与 mgmt
/// 测试对端 MUST 都经此函数，杜绝服务端/测试两套语义漂移。
pub fn answer_history_query<S: HistoryPageSource>(src: Option<&S>, req: &[u8]) -> Vec<u8> {
    let q = match decode_request(req) {
        Some(q) => q,
        None => return encode_response(HIST_BAD_REQUEST, 0, 0, 0, 0, &[], false),
    };
    let Some(src) = src else {
        return encode_response(HIST_UNAVAILABLE, 0, q.point, 0, 0, &[], false);
    };
    let (last_n, clamped) = clamp_last_n(q.last_n);
    let flags = if clamped { FLAG_CLAMPED } else { 0 };
    let Some((rows, total, window_len)) = src.query_page(q.point, last_n as usize, q.page) else {
        return encode_response(HIST_UNKNOWN_POINT, flags, q.point, 0, q.page, &[], false);
    };
    let more = more_for_page(q.page, window_len as usize);
    encode_response(HIST_OK, flags, q.point, total, q.page, &rows, more)
}

/// 单条历史记录（wire 视图；quality 为四档码 0=Good / 1=Suspect / 2=Bad /
/// 3=Unknown，单一来源 = agent-sdk `sample_wire::quality_code`；tsdb 四档
/// 如实承载，写读往返不折叠）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HistPointWire {
    /// 测量时刻毫秒。
    pub timestamp_ms: u64,
    /// 来源实例 ID。
    pub device_id: u32,
    /// 统一点 ID。
    pub point: u32,
    /// 采样值。
    pub value: f64,
    /// 品质码。
    pub quality: u8,
}

/// 服务端组装一页响应：头部 + 行区。
///
/// `total`=该点位全量行数；`page`=本次页号；`rows`=本页行（调用方保证
/// ≤[`PAGE_RECORDS`]；越界直接panic 属编程错误，用 debug_assert 提示）；
/// `more`=还有后续页。
pub fn encode_response(
    status: u8,
    flags: u8,
    point: u32,
    total: u32,
    page: u32,
    rows: &[HistPointWire],
    more: bool,
) -> Vec<u8> {
    debug_assert!(rows.len() <= PAGE_RECORDS, "单页行数必须 ≤ PAGE_RECORDS");
    let mut out = Vec::with_capacity(RESP_HEADER_LEN + rows.len() * ROW_LEN);
    out.extend_from_slice(TELR_MAGIC);
    out.push(status);
    out.push(flags);
    out.extend_from_slice(&point.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&page.to_le_bytes());
    out.extend_from_slice(&(rows.len() as u32).to_le_bytes());
    out.push(u8::from(more));
    for r in rows {
        out.extend_from_slice(&r.timestamp_ms.to_le_bytes());
        out.extend_from_slice(&r.device_id.to_le_bytes());
        out.extend_from_slice(&r.point.to_le_bytes());
        out.extend_from_slice(&r.value.to_le_bytes());
        out.push(r.quality);
    }
    out
}

/// 解码后的响应页。
#[derive(Debug, Clone, PartialEq)]
pub struct HistResponse {
    pub status: u8,
    pub flags: u8,
    pub point: u32,
    pub total: u32,
    pub page: u32,
    pub rows: Vec<HistPointWire>,
    /// 行区原始字节（`RESP_HEADER_LEN` 之后，25B × page_n）——聚合侧
    /// 拼接免再编码（SPEC-A 卷：行布局物化点收敛）。
    pub rows_raw: Vec<u8>,
    pub more: bool,
}

impl HistResponse {
    /// 行区原始字节（零拷贝视图，等价于请求缓冲的 `[RESP_HEADER_LEN..]`）。
    pub fn rows_bytes(&self) -> &[u8] {
        &self.rows_raw
    }
}

/// 解码行区（长度自洽：`buf.len()` 必须为 `ROW_LEN` 整数倍；不限单页
/// 行数——聚合/测试可传入多页拼接后的整段）。供 [`decode_response`]
/// 内部与测试/聚合复用——25B 行解析只有这一处物化；单页上限由
/// `decode_response` 的 page_n/长度校验把关，此处不重复设限。
pub fn decode_rows(buf: &[u8]) -> Option<Vec<HistPointWire>> {
    if buf.len() % ROW_LEN != 0 {
        return None;
    }
    let mut rows = Vec::with_capacity(buf.len() / ROW_LEN);
    let mut off = 0;
    while off < buf.len() {
        rows.push(HistPointWire {
            timestamp_ms: u64::from_le_bytes(buf[off..off + 8].try_into().ok()?),
            device_id: u32::from_le_bytes(buf[off + 8..off + 12].try_into().ok()?),
            point: u32::from_le_bytes(buf[off + 12..off + 16].try_into().ok()?),
            value: f64::from_le_bytes(buf[off + 16..off + 24].try_into().ok()?),
            quality: buf[off + 24],
        });
        off += ROW_LEN;
    }
    Some(rows)
}

/// 客户端解码响应帧（严格自洽校验：行区长度必须与 page_n 匹配）。
pub fn decode_response(buf: &[u8]) -> Option<HistResponse> {
    if buf.len() < RESP_HEADER_LEN || &buf[0..4] != TELR_MAGIC {
        return None;
    }
    let status = buf[4];
    let flags = buf[5];
    let point = u32::from_le_bytes(buf[6..10].try_into().ok()?);
    let total = u32::from_le_bytes(buf[10..14].try_into().ok()?);
    let page = u32::from_le_bytes(buf[14..18].try_into().ok()?);
    let page_n = u32::from_le_bytes(buf[18..22].try_into().ok()?) as usize;
    let more = buf[22] != 0;
    if page_n > PAGE_RECORDS {
        return None;
    }
    let expected = RESP_HEADER_LEN + page_n * ROW_LEN;
    if buf.len() != expected {
        return None;
    }
    let rows_raw = buf[RESP_HEADER_LEN..].to_vec();
    let rows = decode_rows(&rows_raw)?;
    Some(HistResponse {
        status,
        flags,
        point,
        total,
        page,
        rows,
        rows_raw,
        more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row(ts: u64, v: f64, q: u8) -> HistPointWire {
        HistPointWire {
            timestamp_ms: ts,
            device_id: 7,
            point: 42,
            value: v,
            quality: q,
        }
    }

    #[test]
    fn request_roundtrip_and_rejects() {
        let q = HistQuery {
            point: 5,
            last_n: 100,
            page: 2,
        };
        assert_eq!(decode_request(&encode_query(&q)), Some(q));
        // 非 magic / 错命令 / 截断。
        assert_eq!(decode_request(b"XELR\x01\x00"), None);
        assert!(decode_request(&{
            let mut b = encode_query(&q);
            b[4] = 0xFF;
            b
        })
        .is_none());
        assert_eq!(decode_request(&encode_query(&q)[..16]), None);
    }

    #[test]
    fn clamp_boundary_visible() {
        assert_eq!(clamp_last_n(10), (10, false));
        assert_eq!(clamp_last_n(MAX_LAST_N), (MAX_LAST_N, false));
        assert_eq!(clamp_last_n(MAX_LAST_N + 1), (MAX_LAST_N, true));
        assert_eq!(clamp_last_n(u32::MAX), (MAX_LAST_N, true));
    }

    #[test]
    fn response_roundtrip_with_rows() {
        let rows: Vec<HistPointWire> = (0..PAGE_RECORDS as u64)
            .map(|i| sample_row(i * 10, i as f64 * 0.5, 0))
            .collect();
        let raw = encode_response(HIST_OK, 0, 42, 999, 3, &rows, true);
        assert_eq!(raw.len(), RESP_HEADER_LEN + PAGE_RECORDS * ROW_LEN);
        let d = decode_response(&raw).expect("decode");
        assert_eq!(d.status, HIST_OK);
        assert_eq!(d.total, 999);
        assert_eq!(d.page, 3);
        assert_eq!(d.rows.len(), PAGE_RECORDS);
        assert!(d.more);
        // NaN/Inf 值原样保真（wire 层不做浮点裁决）。
        let nan_rows = alloc::vec![sample_row(1, f64::NAN, 2)];
        let d2 = decode_response(&encode_response(HIST_OK, 0, 1, 1, 0, &nan_rows, false))
            .expect("nan decode");
        assert!(d2.rows[0].value.is_nan());
        assert_eq!(d2.rows[0].quality, 2);
    }

    #[test]
    fn response_mutation_rejected() {
        let raw = encode_response(HIST_BAD_REQUEST, 0, 1, 0, 0, &[], false);
        assert_eq!(raw.len(), RESP_HEADER_LEN);
        let d = decode_response(&raw).expect("hdr-only frame decodes");
        assert!(d.rows.is_empty());
        // page_n 声明越 PAGE_RECORDS 上界拒绝（解码器在长度自洽检查前拒）。
        let mut bad = raw.clone();
        bad[18] = PAGE_RECORDS as u8 + 1;
        assert!(decode_response(&bad).is_none());
        // 行区被截断拒绝（声明 2 行但只给 1 行字节）。
        let mut trunc = encode_response(HIST_OK, 0, 1, 2, 0, &[sample_row(1, 1.0, 0)], false);
        // 手工把声明行数抬到 2 而不补第二行 → 长度自洽校验必须拒。
        trunc[18] = 2;
        assert!(decode_response(&trunc).is_none());
        // 非 magic 拒绝。
        let mut nm = raw;
        nm[0] = b'X';
        assert!(decode_response(&nm).is_none());
    }

    /// 单页预算红线：满页负载不得突破分块通道单传输预算
    /// （CHUNK_COUNT_MAX × CHUNK_DATA_MAX = 1664B）。
    #[test]
    fn full_page_fits_chunked_budget() {
        use crate::chunked::{CHUNK_COUNT_MAX, CHUNK_DATA_MAX};
        let budget = CHUNK_DATA_MAX * CHUNK_COUNT_MAX as usize;
        let rows: Vec<HistPointWire> = (0..PAGE_RECORDS as u64)
            .map(|i| sample_row(i, 0.0, 0))
            .collect();
        let raw = encode_response(HIST_OK, 0, 1, 1, 1, &rows, false);
        assert!(
            raw.len() <= budget,
            "满页 {len} 必须 ≤ {budget}",
            len = raw.len()
        );
    }

    // =====================================================================
    // SPEC-A：answer_history_query 单源策略 + 页游标窗口化（行为变更锁定）
    // =====================================================================

    /// 内存页源：stored 行集 + 精确 total（可 ≠ stored.len()，模拟
    /// "total 大于钳制窗口"的对账场景）。
    struct FakeSource {
        stored: Vec<HistPointWire>,
        total: u32,
    }

    impl FakeSource {
        fn new(stored: Vec<HistPointWire>) -> Self {
            let total = stored.len() as u32;
            Self { stored, total }
        }
        fn with_total(stored: Vec<HistPointWire>, total: u32) -> Self {
            Self { stored, total }
        }
    }

    impl HistoryPageSource for FakeSource {
        fn query_page(
            &self,
            point: u32,
            last_n: usize,
            page: u32,
        ) -> Option<(Vec<HistPointWire>, u32, u32)> {
            if point != 42 {
                return None;
            }
            let mut rows: Vec<HistPointWire> = self.stored.clone();
            if rows.len() > last_n {
                rows.drain(..rows.len() - last_n);
            }
            let window_len = rows.len();
            let start = page as usize * PAGE_RECORDS;
            let end = core::cmp::min(start + PAGE_RECORDS, rows.len());
            let page_rows = if start < rows.len() {
                rows[start..end].to_vec()
            } else {
                Vec::new()
            };
            Some((page_rows, self.total, window_len as u32))
        }
    }

    fn stored_rows(n: usize) -> Vec<HistPointWire> {
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

    fn req(point: u32, page: u32, last_n: u32) -> Vec<u8> {
        encode_query(&HistQuery {
            point,
            last_n,
            page,
        })
    }

    #[test]
    fn answer_normal_multipage_and_unknown() {
        let src = FakeSource::new(stored_rows(120));
        let r0 = decode_response(&answer_history_query(Some(&src), &req(42, 0, 200))).unwrap();
        assert_eq!(r0.status, HIST_OK);
        assert_eq!(r0.rows.len(), PAGE_RECORDS);
        assert!(r0.more);
        let r1 = decode_response(&answer_history_query(Some(&src), &req(42, 1, 200))).unwrap();
        assert_eq!(r1.rows.len(), PAGE_RECORDS);
        assert!(r1.more);
        let r2 = decode_response(&answer_history_query(Some(&src), &req(42, 2, 200))).unwrap();
        assert_eq!(r2.rows.len(), 24, "120 = 48×2+24");
        assert!(!r2.more);
        assert_eq!(r2.total, 120);
        // 未知点位 → UNKNOWN_POINT 明确错误帧（不空成功）。
        let rn = decode_response(&answer_history_query(Some(&src), &req(77, 0, 200))).unwrap();
        assert_eq!(rn.status, HIST_UNKNOWN_POINT);
        assert!(rn.rows.is_empty() && !rn.more);
        // 越界页：空行 + more=false（不伪造数据），total 仍如实。
        let rn2 = decode_response(&answer_history_query(Some(&src), &req(42, 9, 200))).unwrap();
        assert_eq!(rn2.status, HIST_OK);
        assert!(rn2.rows.is_empty() && !rn2.more);
        assert_eq!(rn2.total, 120);
    }

    #[test]
    fn answer_bad_request_and_unavailable() {
        let src = FakeSource::new(stored_rows(3));
        let bad = decode_response(&answer_history_query(Some(&src), &[b'X', 0, 0, 0])).unwrap();
        assert_eq!(bad.status, HIST_BAD_REQUEST);
        // src=None（磁盘停用降级态）→ UNAVAILABLE（point 如实回显）。
        let unav =
            decode_response(&answer_history_query::<FakeSource>(None, &req(42, 0, 10))).unwrap();
        assert_eq!(unav.status, HIST_UNAVAILABLE);
        assert_eq!(unav.point, 42);
    }

    #[test]
    fn answer_clamped_flag_visible() {
        let src = FakeSource::new(stored_rows(600));
        let r = decode_response(&answer_history_query(Some(&src), &req(42, 0, u32::MAX))).unwrap();
        assert_eq!(r.flags & FLAG_CLAMPED, FLAG_CLAMPED);
        assert_eq!(r.total, 600);
    }

    /// 行为变更锁定（SPEC-A）：total=600 > 窗口 512 时，more 按**窗口**
    /// 推导——窗口末页（start=480，32 行）more=false，客户端恰好收满即止，
    /// 不再按 total 多付一页空行往返（旧语义 528<600=true）。
    #[test]
    fn more_window_based_no_empty_page_roundtrip() {
        let src = FakeSource::with_total(stored_rows(600), 600);
        let r = decode_response(&answer_history_query(Some(&src), &req(42, 10, 512))).unwrap();
        assert_eq!(r.rows.len(), 32, "窗口第 10 页应给 480..512 的 32 行");
        assert!(!r.more, "must not push an empty page-11 roundtrip");
        // 对照：旧 total 口径会判 more=true（被修掉的空页往返），明文留下
        // 差异面，防未来回归到 total 口径。
        let old_total_based_more = (10 + 1) * PAGE_RECORDS < 600;
        assert!(old_total_based_more, "旧语义确实会多付一页");
        assert_ne!(
            more_for_page(10, 512),
            old_total_based_more,
            "more 语义已切换为窗口口径"
        );
    }

    #[test]
    fn is_tellr_request_routing() {
        assert!(is_tellr_request(&req(42, 0, 10)));
        let mut wrong = req(42, 0, 10);
        wrong[0] = b'X';
        assert!(!is_tellr_request(&wrong));
        assert!(!is_tellr_request(&req(42, 0, 10)[..16]));
    }

    #[test]
    fn decode_rows_and_rows_bytes_single_source() {
        let rows = stored_rows(3);
        let raw = encode_response(HIST_OK, 0, 42, 3, 0, &rows, false);
        let d = decode_response(&raw).unwrap();
        assert_eq!(d.rows_bytes().len(), 3 * ROW_LEN);
        assert_eq!(d.rows_bytes(), &raw[RESP_HEADER_LEN..]);
        // decode_rows 与 decode_response 的解析同源。
        let via_rows = decode_rows(d.rows_bytes()).unwrap();
        assert_eq!(via_rows, d.rows);
        // 长度自洽：非 25B 整数倍拒绝。
        assert!(decode_rows(&raw[RESP_HEADER_LEN..RESP_HEADER_LEN + 1]).is_none());
        // 空页行区。
        let empty = encode_response(HIST_OK, 0, 42, 0, 0, &[], false);
        let de = decode_response(&empty).unwrap();
        assert!(de.rows.is_empty() && de.rows_bytes().is_empty());
    }
}
