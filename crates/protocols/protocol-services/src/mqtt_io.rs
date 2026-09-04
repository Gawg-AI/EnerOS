//! svc-mqtt 中继面（蓝图 64 卷 64.2 WP-64.2.4：出 bin——host 可测化）。
//!
//! 单一事实：`MqttWriteIoService`（47 写意图转发 + 49 AI 应答中继 + 50
//! 管理请求转发 + 59 双面分片重组）此前整块压在 bin 的
//! `#[cfg(target_os = "none")]` 之下（host 零测试）。本模块以 `C: RawIpc`
//! 泛化端点后迁入 lib：中继循环面 host 全可测（mock 端点 + 批量注入），
//! target 装配以 `Sel4IpcClient` 实例化（行为/词形逐字节不变）。
//!
//! 三条转发环（intents / ai / mgmt）同形收敛 [`drain_and_forward`] 骨架
//! （循环 + 端点缺席 fail-closed + chunked Call 三分支分类）；10 个手记
//! 计数器收进 [`FwdCounters`]（类型化）。**打印词形逐字节冻结**
//! （INTENT-FWD / AI-RESP-* / MGMT-FWD-* 族——47-50 卷证据行依赖）。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::chunked::{ChunkError, RawIpc};
use eneros_agent_sdk::endpoint::ChunkedEndpoint;
use eneros_agent_sdk::relay_face::RelayAssembler;
use eneros_protocol_abstract::write::PointWriteIntent;

/// 转发计数（原 bin 的 10 个手记 u64 字段类型化——观测面单点）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FwdCounters {
    /// 写意图转发成功（回执 Committed/Failed/Indeterminate 均算已受理）。
    pub forwarded_total: u64,
    /// 写意图被拒（服务侧拒绝/回执 DISP_ERR）。
    pub rejected_total: u64,
    /// AI 请求发布成功（分片 END 定稿后入 app outbox）。
    pub ai_req_published: u64,
    /// AI 应答转发成功。
    pub ai_resp_forwarded: u64,
    /// AI 链路错误（端点缺席/传输失败/分片坏）。
    pub ai_errors: u64,
    /// 遥测帧发布成功。
    pub tel_published: u64,
    /// 遥测帧拒绝（头坏/断连/未配置主题/outbox 满）。
    pub tel_rejected: u64,
    /// 管理请求转发成功。
    pub mgmt_forwarded: u64,
    /// 管理请求转发失败（端点缺席/传输失败）。
    pub mgmt_errors: u64,
}

/// 47（WP-47.1）：写意图转发服务——插件 poll（含下行写解析）+ 意图
/// EIPC 上报 svc-control-dispatch（`DISP_SUBMIT_INTENT` + 意图线码 +
/// actor）。dispatch 端点未授予 → 意图不转发（fail-closed，串口登记）。
///
/// `C = Sel4IpcClient`（target）或 mock `RawIpc`（host 测试）。
pub struct MqttWriteIoService<C: RawIpc> {
    /// 协议 I/O 服务（poll 驱动 + 插件所有权）。
    pub io: crate::service::PluginIoService<eneros_mqtt::plugin::MqttPlugin>,
    /// svc-control-dispatch EIPC 客户端（授予槽按名解析——S1 起不手数槽位；
    /// 首边 = svc-config 协议配置）。
    pub dispatch: Option<ChunkedEndpoint<C>>,
    /// 49（WP-49.2）：AI 请求分片组装器。
    ai_assembler: RelayAssembler,
    /// 49：svc-ai-worker EIPC 客户端（授予槽按名解析；表序第 3 条授予）。
    /// 未授予 → 应答丢弃计数（fail-closed）。
    pub ai_worker: Option<ChunkedEndpoint<C>>,
    /// 49：AI 请求发布主题（protocol-mqtt 配置；未配置 = AI 链路关闭）。
    ai_request_topic: Option<String>,
    /// 50（WP-50.1）：遥测帧分片组装器（与 AI 请求组装同语义；不解释内容）。
    tel_assembler: RelayAssembler,
    /// 50（WP-50.2）：svc-mgmt EIPC 客户端（授予槽按名解析；表序第 4 条
    /// 授予）。未授予 → 管理请求丢弃计数（fail-closed）。
    pub mgmt: Option<ChunkedEndpoint<C>>,
    /// 转发计数（观测面）。
    pub counters: FwdCounters,
}

impl<C: RawIpc> MqttWriteIoService<C> {
    /// 构造（端点/主题由 bin 装配面注入；计数清零、组装器全新）。
    pub fn new(
        io: crate::service::PluginIoService<eneros_mqtt::plugin::MqttPlugin>,
        dispatch: Option<ChunkedEndpoint<C>>,
        ai_worker: Option<ChunkedEndpoint<C>>,
        mgmt: Option<ChunkedEndpoint<C>>,
        ai_request_topic: Option<String>,
    ) -> Self {
        Self {
            io,
            dispatch,
            ai_assembler: RelayAssembler::new(),
            ai_worker,
            ai_request_topic,
            tel_assembler: RelayAssembler::new(),
            mgmt,
            counters: FwdCounters::default(),
        }
    }

    /// 47（WP-47.1）：本轮 poll 解析到的下行写意图 → dispatch 授权链。
    /// drain 面 touch 插件；转发环在 [`Self::forward_intent_batch`]（host
    /// 可测核）。
    pub fn forward_intents(&mut self) {
        let mut intents: Vec<PointWriteIntent> = Vec::new();
        self.io.drain_write_intents(&mut intents);
        if intents.is_empty() {
            return;
        }
        self.forward_intent_batch(&intents);
    }

    /// 写意图转发环（host 可测核：批量注入 + 端点/计数断言）。
    pub fn forward_intent_batch(&mut self, intents: &[PointWriteIntent]) {
        let ep = &mut self.dispatch;
        let counters = &mut self.counters;
        drain_and_forward(
            intents,
            ep,
            "svc-control-dispatch",
            |intent| {
                // 请求布局：[DISP_SUBMIT_INTENT=2][intent wire 49B][actor 2=Ai]。
                let mut req: Vec<u8> = Vec::with_capacity(1 + 49 + 1);
                req.push(2);
                req.extend_from_slice(&eneros_protocol_abstract::write::encode_intent(intent));
                req.push(2);
                req
            },
            |resp| resp.len() >= 2 && resp[0] == 0,
            |intent, r| match r {
                FwdResult::NoEndpoint => {
                    eneros_runtime::println!(
                        "[svc-mqtt] INTENT-FWD dispatch EP NOT granted —— 意图未转发（fail-closed）"
                    );
                    counters.rejected_total = counters.rejected_total.saturating_add(1);
                }
                FwdResult::Accepted(resp) => {
                    counters.forwarded_total = counters.forwarded_total.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] INTENT-FWD protocol=mqtt device={} point={} power={:?} seq={} state={} total={}",
                        intent.device_id,
                        intent.point_id,
                        intent.value,
                        intent.sequence,
                        resp[1],
                        counters.forwarded_total
                    );
                }
                FwdResult::Rejected(resp) => {
                    counters.rejected_total = counters.rejected_total.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] INTENT-FWD rejected full-receipt {:02x?}",
                        &resp[..resp.len().min(82)]
                    );
                }
                FwdResult::Transport(e) => {
                    counters.rejected_total = counters.rejected_total.saturating_add(1);
                    eneros_runtime::println!("[svc-mqtt] INTENT-FWD eipc={e:?}");
                }
            },
        );
    }

    /// 49（WP-49.2）：把插件收到的 peer 应答原始字节推送 svc-ai-worker
    ///（ChunkedEndpoint chunked Call，载荷 = [AI_PEER_MSG][json]）。未授予/
    /// 失败 → 计数丢弃（fail-closed；ai-worker 侧超时降级接管）。
    pub fn forward_ai_responses(&mut self) {
        let mut msgs: Vec<Vec<u8>> = Vec::new();
        self.io.plugin_mut().drain_peer_messages(&mut msgs);
        self.forward_ai_batch(&msgs);
    }

    /// AI 应答转发环（host 可测核）。
    pub fn forward_ai_batch(&mut self, msgs: &[Vec<u8>]) {
        let ep = &mut self.ai_worker;
        let counters = &mut self.counters;
        drain_and_forward(
            msgs,
            ep,
            "svc-ai-worker",
            |payload| {
                let mut req: Vec<u8> = Vec::with_capacity(1 + payload.len());
                req.push(eneros_agent_sdk::ai_relay::AI_PEER_MSG);
                req.extend_from_slice(payload);
                req
            },
            |_| true,
            |payload, r| match r {
                FwdResult::NoEndpoint => {
                    counters.ai_errors = counters.ai_errors.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] AI-RESP-DROP ai-worker EP NOT granted（fail-closed）"
                    );
                }
                FwdResult::Accepted(_) => {
                    counters.ai_resp_forwarded = counters.ai_resp_forwarded.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] AI-RESP-FWD len={} total={}",
                        payload.len(),
                        counters.ai_resp_forwarded
                    );
                }
                // is_accepted 恒真——Rejected 分支不可达（占位如实计数）。
                FwdResult::Rejected(_) => {
                    counters.ai_errors = counters.ai_errors.saturating_add(1);
                }
                FwdResult::Transport(e) => {
                    counters.ai_errors = counters.ai_errors.saturating_add(1);
                    eneros_runtime::println!("[svc-mqtt] AI-RESP-DROP eipc={e:?}");
                }
            },
        );
    }

    /// 50（WP-50.2）：把插件收到的管理请求（mgmt_request_topic 原始
    /// MGT1 帧）经 EIPC 推送 svc-mgmt；应答原样发布回 mgmt_response_topic。
    /// 未授予/失败 → 计数丢弃（fail-closed——管理通道降级，不伪造成功）。
    pub fn forward_mgmt_requests(&mut self) {
        let mut reqs: Vec<Vec<u8>> = Vec::new();
        self.io.plugin_mut().drain_mgmt_requests(&mut reqs);
        self.forward_mgmt_batch(&reqs);
    }

    /// 管理请求转发环（host 可测核；受理分支回发布应答）。
    pub fn forward_mgmt_batch(&mut self, reqs: &[Vec<u8>]) {
        let ep = &mut self.mgmt;
        let counters = &mut self.counters;
        let plugin = self.io.plugin_mut();
        drain_and_forward(
            reqs,
            ep,
            "svc-mgmt",
            |payload| payload.clone(),
            |_| true,
            |payload, r| match r {
                FwdResult::NoEndpoint => {
                    counters.mgmt_errors = counters.mgmt_errors.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] MGMT-FWD-DROP svc-mgmt EP NOT granted（fail-closed）"
                    );
                }
                FwdResult::Accepted(resp) => {
                    counters.mgmt_forwarded = counters.mgmt_forwarded.saturating_add(1);
                    // 应答原样发布（MGT1 响应帧：status 1B + detail）。
                    let published = plugin.enqueue_mgmt_response(resp);
                    eneros_runtime::println!(
                        "[svc-mqtt] MGMT-FWD len={} total={} resp-publish={}",
                        payload.len(),
                        counters.mgmt_forwarded,
                        if published.is_ok() {
                            "ok"
                        } else {
                            "drop（未配置应答主题/outbox 满）"
                        }
                    );
                }
                // is_accepted 恒真——Rejected 分支不可达（占位如实计数）。
                FwdResult::Rejected(_) => {
                    counters.mgmt_errors = counters.mgmt_errors.saturating_add(1);
                }
                FwdResult::Transport(e) => {
                    counters.mgmt_errors = counters.mgmt_errors.saturating_add(1);
                    eneros_runtime::println!("[svc-mqtt] MGMT-FWD eipc={e:?}");
                }
            },
        );
    }

    /// 蓝图 59：EXECUTE 载荷分片子码统一处理（AI 请求面 + 遥测帧面）。
    /// BEGIN/CONT 两面同构走 relay_face 单源；END 按面定稿（域语义不同：
    /// AI 发布应答主题 vs 遥测帧头三重校验）。返回 Some(1B 状态) = 已消费；
    /// None = 非本两面子码（调用方继续既有 EXECUTE 处理）。
    pub fn handle_relay_fragment(&mut self, payload: &[u8]) -> Option<u8> {
        use eneros_agent_sdk::ai_relay::FACE as AI_FACE;
        use eneros_agent_sdk::relay_face::FragmentStep;
        use eneros_agent_sdk::telemetry_relay::FACE as TEL_FACE;

        for (face, is_ai) in [(AI_FACE, true), (TEL_FACE, false)] {
            match face.classify(payload) {
                Some(FragmentStep::Begin { total, data }) => {
                    let asm = self.assembler_mut(is_ai);
                    return match asm.begin(total, data, face.max_total) {
                        Ok(()) => Some(0),
                        Err(e) => {
                            self.relay_reject(is_ai, "begin", &e);
                            Some(1)
                        }
                    };
                }
                Some(FragmentStep::Cont { data }) => {
                    let asm = self.assembler_mut(is_ai);
                    return match asm.cont(data) {
                        Ok(()) => Some(0),
                        Err(e) => {
                            self.relay_reject(is_ai, "cont", &e);
                            Some(1)
                        }
                    };
                }
                Some(FragmentStep::End) => {
                    return Some(if is_ai {
                        self.ai_finalize()
                    } else {
                        self.tel_finalize()
                    });
                }
                None => continue,
            }
        }
        None
    }

    /// 面选择器（AI/遥测组装器）。
    fn assembler_mut(&mut self, is_ai: bool) -> &mut RelayAssembler {
        if is_ai {
            &mut self.ai_assembler
        } else {
            &mut self.tel_assembler
        }
    }

    /// 分片错误公共出口：计数 + 前缀化打印（文本与原双 handler 逐字一致）。
    fn relay_reject(
        &mut self,
        is_ai: bool,
        step: &str,
        e: &eneros_agent_sdk::relay_face::AssembleError,
    ) {
        if is_ai {
            self.counters.ai_errors = self.counters.ai_errors.saturating_add(1);
            eneros_runtime::println!("[svc-mqtt] AI-FRAG-REJECT {step} {e:?}");
        } else {
            self.counters.tel_rejected = self.counters.tel_rejected.saturating_add(1);
            eneros_runtime::println!("[svc-mqtt] TEL-FRAG-REJECT {step} {e:?}");
        }
    }

    /// AI 面 END 定稿（原 handle_ai_fragment END 分支原样迁移）。
    fn ai_finalize(&mut self) -> u8 {
        match self.ai_assembler.end() {
            Ok(json) => {
                let topic = self.ai_request_topic.clone();
                let enq = match topic {
                    Some(t) => self.io.plugin_mut().enqueue_app_publish(&t, &json),
                    None => {
                        eneros_runtime::println!(
                            "[svc-mqtt] AI-REQ-DROP ai_request_topic 未配置（fail-closed）"
                        );
                        Err(eneros_protocol_abstract::ProtocolError::InvalidConfig)
                    }
                };
                match enq {
                    Ok(()) => {
                        self.counters.ai_req_published =
                            self.counters.ai_req_published.saturating_add(1);
                        eneros_runtime::println!(
                            "[svc-mqtt] AI-REQ-ENQUEUE len={} total={}",
                            json.len(),
                            self.counters.ai_req_published
                        );
                        0
                    }
                    Err(_) => {
                        self.counters.ai_errors = self.counters.ai_errors.saturating_add(1);
                        1
                    }
                }
            }
            Err(e) => {
                self.counters.ai_errors = self.counters.ai_errors.saturating_add(1);
                eneros_runtime::println!("[svc-mqtt] AI-FRAG-REJECT end {e:?}");
                1
            }
        }
    }

    /// 遥测面 END 定稿（原 handle_telemetry_fragment END 分支原样迁移；
    /// doc 序 = 代码序——蓝图 65 WP-65.5 订正，原 doc topic 第 2/connected
    /// 第 3 与代码相反）：
    /// 1. 帧头（magic `OBSF` + schema 版本）不匹配 → 拒绝解析 + 计数；
    /// 2. 插件未连接（broker 断）→ 拒绝 → 调用方 durable outbox 兜底补传；
    /// 3. telemetry_topic 校验在 enqueue_telemetry_publish 内（QoS1 发布；
    ///    未配置主题插件拒绝）。
    fn tel_finalize(&mut self) -> u8 {
        match self.tel_assembler.end() {
            Ok(frame) => {
                // 1. 帧头校验（schema 版本不匹配 → 拒绝解析）。
                if let Err(e) = eneros_agent_sdk::telemetry_relay::check_frame_header(&frame) {
                    self.counters.tel_rejected = self.counters.tel_rejected.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] TEL-FRAME-REJECT header={e:?} len={}（fail-closed）",
                        frame.len()
                    );
                    return 1;
                }
                // 2. 插件已连接（broker 在线——断连时拒绝 → durable 兜底）。
                if !self.io.plugin_mut().is_connected() {
                    self.counters.tel_rejected = self.counters.tel_rejected.saturating_add(1);
                    eneros_runtime::println!(
                        "[svc-mqtt] TEL-FRAME-REJECT broker 断连（fail-closed → 补传）"
                    );
                    return 1;
                }
                // 3. 入插件 app outbox（QoS1 发布；未配置主题插件拒绝）。
                match self.io.plugin_mut().enqueue_telemetry_publish(&frame) {
                    Ok(()) => {
                        self.counters.tel_published = self.counters.tel_published.saturating_add(1);
                        eneros_runtime::println!(
                            "[svc-mqtt] TEL-FRAME-ACCEPT len={} total={}",
                            frame.len(),
                            self.counters.tel_published
                        );
                        0
                    }
                    Err(_) => {
                        self.counters.tel_rejected = self.counters.tel_rejected.saturating_add(1);
                        eneros_runtime::println!(
                            "[svc-mqtt] TEL-FRAME-REJECT 未配置主题/outbox 满（fail-closed）"
                        );
                        1
                    }
                }
            }
            Err(e) => {
                self.counters.tel_rejected = self.counters.tel_rejected.saturating_add(1);
                eneros_runtime::println!("[svc-mqtt] TEL-FRAG-REJECT end {e:?}");
                1
            }
        }
    }
}

impl<C: RawIpc> eneros_drv_svc::runtime::DriverService for MqttWriteIoService<C> {
    fn capability(&self) -> Option<&'static str> {
        self.io.capability()
    }

    fn handle(
        &mut self,
        req: &eneros_drv_svc::runtime::DriverRequest,
        resp: &mut eneros_drv_svc::runtime::DriverResponse,
    ) {
        use eneros_drv_svc::runtime::*;
        if req.opcode == OP_EXECUTE {
            let p = &req.payload[..req.payload_len as usize];
            // 先 io.handle（本轮 poll 驱动插件 connect 状态刷新），再处理
            // 分片——telemetry 的 broker 断连拒绝依赖新鲜 connected。
            self.io.handle(req, resp);
            // 蓝图 59：两面分片统一处理（原双 handler 收敛）。
            if let Some(status) = self.handle_relay_fragment(p) {
                resp.status = ST_OK;
                resp.payload[0] = status;
                resp.payload_len = 1;
                return;
            }
            // 47（WP-47.1）：本轮 poll 解析到的下行写意图 → dispatch 授权链。
            self.forward_intents();
            // 49（WP-49.2）：本轮收到的 peer 应答 → ai-worker 推送。
            self.forward_ai_responses();
            // 50（WP-50.2）：本轮收到的管理请求 → svc-mgmt（应答回发布）。
            self.forward_mgmt_requests();
        } else {
            resp.status = ST_INVALID;
        }
    }
}

// ============================================================================
// 三条转发环的同形骨架（WP-64.2.4）
// ============================================================================

/// chunked Call 转发结果（骨架的分类面——受理判定由面谓词注入；
/// 端点缺席 = [`FwdResult::NoEndpoint`]，fail-closed 语义由面回调承载）。
enum FwdResult {
    /// 端点未授予（None）。
    NoEndpoint,
    /// 受理（面谓词通过）。
    Accepted(Vec<u8>),
    /// 服务侧拒（面谓词未过——intents 面的回执非 0 状态）。
    Rejected(Vec<u8>),
    /// 传输失败。
    Transport(ChunkError),
}

/// 转发环骨架：循环 + chunked Call 三分支分类 + 单回调分发。计数选择/
/// 打印/回发布的面差异由回调注入——骨架不拼词形（打印逐字节归回调持有）；
/// 单回调单借用（计数结构不拆借）。
fn drain_and_forward<T, C: RawIpc>(
    items: &[T],
    ep: &mut Option<ChunkedEndpoint<C>>,
    target: &str,
    encode: impl Fn(&T) -> Vec<u8>,
    is_accepted: impl Fn(&[u8]) -> bool,
    mut on_item: impl FnMut(&T, &FwdResult),
) {
    for item in items {
        let r = match ep.as_mut() {
            None => FwdResult::NoEndpoint,
            Some(client) => match client.call(target, &encode(item)) {
                Ok(resp) if is_accepted(&resp) => FwdResult::Accepted(resp),
                Ok(resp) => FwdResult::Rejected(resp),
                Err(e) => FwdResult::Transport(e),
            },
        };
        on_item(item, &r);
    }
}

// ============================================================================
// host 测试：三条转发环 × 三路径（正常 / None 端点 / 传输失败）——迁移前
// 这些路径 host 不可测（bin 整块 cfg 门，0 测试）。
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FixedRespServer;

    fn service(
        dispatch: Option<ChunkedEndpoint<FixedRespServer>>,
    ) -> MqttWriteIoService<FixedRespServer> {
        MqttWriteIoService::new(
            crate::service::PluginIoService::new(
                eneros_mqtt::plugin::MqttPlugin::new(),
                "eneros_mqtt::plugin::MqttPlugin",
            ),
            dispatch,
            None,
            None,
            None,
        )
    }

    fn intent(seq: u64) -> PointWriteIntent {
        PointWriteIntent::new(
            "mqtt",
            3,
            7,
            eneros_upa_model::PointValue::Float(50.5),
            seq,
            1_000,
        )
    }

    #[test]
    fn intents_accepted_counts_forwarded() {
        // 正常路径：回执 [0, state] → forwarded_total。
        let mut svc = service(Some(ChunkedEndpoint::wrap(FixedRespServer::responding(
            alloc::vec![0, 5],
        ))));
        svc.forward_intent_batch(&[intent(1), intent(2)]);
        assert_eq!(svc.counters.forwarded_total, 2);
        assert_eq!(svc.counters.rejected_total, 0);
    }

    #[test]
    fn intents_none_endpoint_and_reject_and_transport_fail_closed() {
        // None 端点：fail-closed 计数拒收。
        let mut svc = service(None);
        svc.forward_intent_batch(&[intent(1)]);
        assert_eq!(svc.counters.rejected_total, 1);
        assert_eq!(svc.counters.forwarded_total, 0);

        // 服务侧拒：回执首字节非 0（1B 0xFF——len<2 同判）。
        let mut svc = service(Some(ChunkedEndpoint::wrap(FixedRespServer::responding(
            alloc::vec![0xFF],
        ))));
        svc.forward_intent_batch(&[intent(1)]);
        assert_eq!(svc.counters.rejected_total, 1);
        assert_eq!(svc.counters.forwarded_total, 0);

        // 传输失败（Header 即拒）→ 计数拒收。
        let mut svc = service(Some(ChunkedEndpoint::wrap(FixedRespServer::failing())));
        svc.forward_intent_batch(&[intent(1)]);
        assert_eq!(svc.counters.rejected_total, 1);
        assert_eq!(svc.counters.forwarded_total, 0);
    }

    #[test]
    fn ai_responses_forwarded_and_fail_closed() {
        // 正常：任意应答 → Accepted（is_accepted 恒真）。
        let mut svc = service(None);
        svc.ai_worker = Some(ChunkedEndpoint::wrap(FixedRespServer::responding(
            alloc::vec![0],
        )));
        svc.forward_ai_batch(&[alloc::vec![1, 2, 3]]);
        assert_eq!(svc.counters.ai_resp_forwarded, 1);
        assert_eq!(svc.counters.ai_errors, 0);

        // None 端点：ai_errors。
        let mut svc = service(None);
        svc.forward_ai_batch(&[alloc::vec![1]]);
        assert_eq!(svc.counters.ai_errors, 1);
        assert_eq!(svc.counters.ai_resp_forwarded, 0);

        // 传输失败：ai_errors。
        let mut svc = service(None);
        svc.ai_worker = Some(ChunkedEndpoint::wrap(FixedRespServer::failing()));
        svc.forward_ai_batch(&[alloc::vec![1]]);
        assert_eq!(svc.counters.ai_errors, 1);
    }

    #[test]
    fn mgmt_requests_forwarded_and_fail_closed() {
        // 正常：mgmt_forwarded（应答回发布走插件——未连接 = drop，如实）。
        let mut svc = service(None);
        svc.mgmt = Some(ChunkedEndpoint::wrap(FixedRespServer::responding(
            alloc::vec![0, 0, 0],
        )));
        svc.forward_mgmt_batch(&[alloc::vec![9, 8]]);
        assert_eq!(svc.counters.mgmt_forwarded, 1);
        assert_eq!(svc.counters.mgmt_errors, 0);

        // None 端点：mgmt_errors。
        let mut svc = service(None);
        svc.forward_mgmt_batch(&[alloc::vec![9]]);
        assert_eq!(svc.counters.mgmt_errors, 1);

        // 传输失败：mgmt_errors。
        let mut svc = service(None);
        svc.mgmt = Some(ChunkedEndpoint::wrap(FixedRespServer::failing()));
        svc.forward_mgmt_batch(&[alloc::vec![9]]);
        assert_eq!(svc.counters.mgmt_errors, 1);
    }
}
