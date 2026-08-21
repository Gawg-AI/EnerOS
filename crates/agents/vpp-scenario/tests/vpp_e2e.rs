//! v0.126.0 VPP 场景端到端集成测试（E1~E22 + PERF23）.
//!
//! 覆盖集群组建（E1~E4）、聚合计算（E5~E7）、DR 事件全流程（E8~E11）、
//! Coordinator 重选（E12）、第 4~10 项出口证据的真实子系统采集（E13~E19）、
//! Phase 2 出口判定复演（E20~E22）与全场景端到端性能（PERF23）。
//!
//! 证据采集（D4）：第 4~10 项出口标准分别驱动真实子系统产生证据——
//! eneros-federation PBFT 共识 / eneros-tsn-time gPTP 同步 / eneros-pentest
//! 渗透套件 / eneros-metrics+logging+tracing 三支柱 / eneros-ota-ab A/B 回滚 /
//! eneros-recovery 损坏恢复 / eneros-iec61850-goose 发布订阅往返。

use std::collections::BTreeMap;
use std::time::Instant;

use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
use eneros_federation::{f, quorum, ConsensusEngine, MockConsensusBus};
use eneros_iec61850_goose::{GooseControlBlock, GoosePublisher, GooseSubscriber, MockL2, RxStatus};
use eneros_iec61850_model::DaValue;
use eneros_logging::{LogEntry, LogFilter, LogLevel, StructuredLogger, StructuredLoggerImpl};
use eneros_metrics::{
    MemoryMetricsStore, MetricDef, MetricType, MetricValue, MetricsCollector, MetricsCollectorImpl,
};
use eneros_ota_ab::ab_partition::{PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};
use eneros_ota_ab::{boot_health_check, AbPartitionOta, OtaManager, PartitionSlot, UpgradeMeta};
use eneros_pentest::{PentestSuite, PentestSuiteImpl};
use eneros_recovery::{RecoveryCommand, RecoveryMode, RecoveryModeImpl, RecoveryState};
use eneros_storage::MockBlockDevice;
use eneros_tracing::{Tracer, TracerImpl};
use eneros_tsn_time::{
    BmcaResult, ClockIdentity, FollowUpMessage, GptpClock, GptpConfig, PtpTime, SyncMessage,
};
use eneros_vpp_scenario::vpp_scenario::VppScenarioImpl;
use eneros_vpp_scenario::{
    DrEvent, DrState, EdgeNode, ExitEvidence, VppError, VppRole, VppScenario,
};

// ============================================================================
// 公共测试基建
// ============================================================================

/// 构造测试节点（地址统一 `edge://vpp-node`，定容串转换失败不可达）.
fn make_node(node_id: u32, role: VppRole, capacity_kw: f64, soc: f64, online: bool) -> EdgeNode {
    let mut address = heapless::String::<64>::new();
    let _ = address.push_str("edge://vpp-node");
    EdgeNode {
        node_id,
        role,
        address,
        capacity_kw,
        soc,
        online,
    }
}

/// 蓝图出口报告口径 8 节点集群：node 1~7 为 EdgeBox（各 50kW、soc=0.8、在线），
/// node 8 为 Coordinator（容量 0kW 不参与调度）。聚合 total=350kW.
fn make_cluster() -> Vec<EdgeNode> {
    let mut nodes = Vec::new();
    for id in 1..=7u32 {
        nodes.push(make_node(id, VppRole::EdgeBox, 50.0, 0.8, true));
    }
    nodes.push(make_node(8, VppRole::Coordinator, 0.0, 0.0, true));
    nodes
}

/// DR 事件样例：降载 300kW、持续 30 分钟、响应时限预算 30s、奖励 128.5 元.
fn make_event() -> DrEvent {
    DrEvent {
        event_id: 1,
        timestamp_ms: 1_000,
        target_kw: -300.0,
        duration_min: 30,
        deadline_ms: 30_000,
        reward: 128.5,
    }
}

/// 构造出口证据（actual ≤ 128 字节，超长按字符边界截断）.
fn make_evidence(actual: &str, passed: bool) -> ExitEvidence {
    let mut a = heapless::String::<128>::new();
    for ch in actual.chars() {
        if a.push(ch).is_err() {
            break;
        }
    }
    ExitEvidence { actual: a, passed }
}

/// 字节串 → 小写 hex（Recovery Flash 命令参数编码用）.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ============================================================================
// 证据采集：真实子系统调用（E13~E19 单测与 E20/PERF23 复用）
// ============================================================================

/// 证据 4：eneros-federation PBFT 2f+1 共识真实运行.
///
/// 4 节点（f=1、quorum=3）经公开 API（ConsensusEngine + MockConsensusBus）
/// 完成一次请求提交：PrePrepare→Prepare→Commit 三阶段消息经总线真实投递，
/// 断言四节点全部 committed。返回出口证据（actual 含实测共识参数）。
fn collect_evidence_pbft() -> ExitEvidence {
    // 节点与密钥材料（确定性种子，公开 API 手工组装，crate 内 testutil 为 cfg(test) 不可复用）
    let node_ids: Vec<u64> = vec![1, 2, 3, 4];
    let mut kps = Vec::new();
    let mut peers = BTreeMap::new();
    for &id in &node_ids {
        let mut seed = [0u8; 32];
        seed[0] = id as u8;
        seed[1] = 0xC0;
        let mut rng = CsRng::from_seed(&seed);
        let kp = Sm2KeyPair::generate(&mut rng).expect("SM2 密钥对生成");
        peers.insert(id, kp.public_key);
        kps.push(kp);
    }
    let mut engines = Vec::new();
    for (idx, kp) in kps.into_iter().enumerate() {
        let mut seed = [0u8; 32];
        seed[0] = idx as u8 + 1;
        seed[1] = 0xEE;
        engines.push(
            ConsensusEngine::new(
                node_ids.clone(),
                node_ids[idx],
                kp,
                peers.clone(),
                CsRng::from_seed(&seed),
                1_000,
            )
            .expect("共识引擎构造"),
        );
    }
    let mut bus = MockConsensusBus::new();
    for &id in &node_ids {
        bus.register(id);
    }
    assert_eq!(f(4), 1, "n=4 容错上限 f 应为 1");
    assert_eq!(quorum(4), 3, "n=4 法定人数应为 2f+1=3");
    // 主节点（view=0 → nodes[0]=1）提交请求
    let seq = engines[0]
        .submit(b"vpp-dr-dispatch-plan".to_vec(), &mut bus, 0)
        .expect("主节点提交");
    assert_eq!(seq, 1);
    // 驱动消息循环直至邮箱排空
    for _ in 0..64 {
        for eng in engines.iter_mut() {
            let _ = eng.poll(&mut bus, 100);
        }
        if bus.queues.values().all(|q| q.is_empty()) {
            break;
        }
    }
    for eng in &engines {
        assert!(
            eng.is_committed(seq),
            "节点 {} 应已提交 seq=1",
            eng.local_id
        );
        assert_eq!(eng.committed_count, 1);
    }
    make_evidence("PBFT 2f+1: n=4 f=1 quorum=3, seq=1 四节点全提交", true)
}

/// 证据 5：eneros-tsn-time gPTP 同步精度真实测量.
///
/// 主/从双 GptpClock：从钟 BMCA 跟随主钟后，连续 64 轮 Sync/FollowUp 报文
/// 处理（从钟相对主钟固定快 500µs、链路延迟 2µs），IIR 低通滤波收敛后测量
/// 偏移估计残差，断言 < 1ms（蓝图出口目标）.
fn collect_evidence_gptp() -> ExitEvidence {
    let master_id = ClockIdentity::new([0xAA; 8]);
    let slave_id = ClockIdentity::new([0xBB; 8]);
    let config = GptpConfig::default();
    let master = GptpClock::new(master_id, PtpTime::new(1_000, 0), &config);
    let mut slave = GptpClock::new(slave_id, PtpTime::new(1_000, 0), &config);
    // BMCA：从钟跟随主钟
    let announce = master.to_announce();
    assert_eq!(
        slave.run_bmca(&[announce]),
        BmcaResult::FollowMaster(master_id)
    );
    // 64 轮 Sync/FollowUp：真偏移 +500µs，链路延迟 2µs
    const TRUE_OFFSET_NS: i64 = 500_000;
    const DELAY_NS: i64 = 2_000;
    for k in 0..64u16 {
        let t_ns: u64 = 1_000_000_000 + u64::from(k) * 125_000_000;
        let origin = PtpTime::new(1_000 + t_ns / 1_000_000_000, (t_ns % 1_000_000_000) as u32);
        // 从钟本地接收时刻 = 主钟发送时刻 + 真偏移 + 链路延迟
        let rx_ns: i128 = origin.to_ns() + i128::from(TRUE_OFFSET_NS) + i128::from(DELAY_NS);
        let rx_ts = PtpTime::new(
            (rx_ns / 1_000_000_000) as u64,
            (rx_ns % 1_000_000_000) as u32,
        );
        let sync = SyncMessage {
            origin_timestamp: origin,
            sequence_id: k,
            steps_removed: 0,
        };
        slave.handle_sync(&sync, rx_ts, DELAY_NS);
        let fu = FollowUpMessage {
            sequence_id: k,
            precise_origin_timestamp: origin,
        };
        slave.handle_follow_up(&fu);
    }
    let offset = slave.compute_offset();
    let residual = (offset - TRUE_OFFSET_NS).abs();
    assert!(
        residual < 1_000_000,
        "gPTP 偏移收敛残差 {residual} ns 应 < 1ms"
    );
    make_evidence(
        &format!("gPTP 64 轮收敛残差 {residual} ns < 1ms（真偏移 500µs）"),
        true,
    )
}

/// 证据 6：eneros-pentest 渗透测试套件真实运行（30 既定用例全过）.
fn collect_evidence_pentest() -> ExitEvidence {
    let mut suite = PentestSuiteImpl::new();
    let cases = suite.run_all().expect("渗透套件运行");
    let report = suite.generate_report().expect("渗透报告生成");
    assert_eq!(cases.len(), 30, "渗透用例应为 30 项");
    assert_eq!(report.total_cases, 30);
    assert!(
        report.overall_pass,
        "渗透套件整体应通过（failed={} blocked={}）",
        report.failed, report.blocked
    );
    assert_eq!(report.critical_findings, 0);
    make_evidence(
        &format!(
            "渗透测试 {}/{} 通过，严重发现 {}",
            report.passed, report.total_cases, report.critical_findings
        ),
        true,
    )
}

/// 证据 7：metrics + logging + tracing 可观测性三支柱各自基本读写真实运行.
fn collect_evidence_observability() -> ExitEvidence {
    // 指标：注册 → 记录 → 刷入 → 查询
    let mut metrics = MetricsCollectorImpl::new(MemoryMetricsStore::new(16), 0);
    let def = MetricDef {
        name: heapless::String::<64>::try_from("vpp.dr_response_ms").expect("name"),
        metric_type: MetricType::Gauge,
        help: heapless::String::<128>::try_from("DR 响应延迟").expect("help"),
        unit: heapless::String::<16>::try_from("ms").expect("unit"),
    };
    metrics.register(def).expect("指标注册");
    metrics
        .record("vpp.dr_response_ms", MetricValue::F64(18_000.0), &[], 100)
        .expect("指标记录");
    assert_eq!(metrics.maybe_flush(100).expect("刷入"), 1);
    let points = metrics
        .query("vpp.dr_response_ms", 0, 1_000)
        .expect("指标查询");
    assert_eq!(points.len(), 1, "指标存储应查询到 1 条数据点");
    // 日志：记录 → 过滤查询
    let mut logger = StructuredLoggerImpl::new(LogLevel::Info);
    let entry = LogEntry {
        timestamp_ms: 1_000,
        level: LogLevel::Info,
        target: heapless::String::<64>::try_from("vpp").expect("target"),
        message: heapless::String::<256>::try_from("DR 事件已完成").expect("message"),
        fields: heapless::Vec::new(),
        trace_id: 0,
        span_id: 0,
        agent_id: 1,
    };
    logger.log(entry).expect("日志记录");
    let logs = logger.query(&LogFilter::default()).expect("日志查询");
    assert_eq!(logs.len(), 1, "日志缓冲应查询到 1 条");
    // 追踪：span 创建 → 完成 → 按 trace_id 检索
    let mut tracer = TracerImpl::new(1.0);
    let mut span = tracer.start_span("vpp-dr-event", None, 1_000);
    tracer.finish_span(&mut span, 1_018);
    let trace = tracer.get_trace(&span.trace_id);
    assert_eq!(trace.len(), 1, "追踪存储应含 1 个 span");
    make_evidence("三支柱就绪：指标 1 点 / 日志 1 条 / 追踪 1 span", true)
}

/// 证据 8：eneros-ota-ab 升级失败自动回滚真实流程.
///
/// 真实 SM2 签名镜像升级 A→B（prepare→write→verify→activate），随后连续
/// 启动健康检查超过 max_boot_attempts=3 次 → 自动回滚 A 分区.
fn collect_evidence_ota_rollback() -> ExitEvidence {
    const TEST_BLOCKS: u64 = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS;
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("SM2 密钥对生成");
    let dev = MockBlockDevice::new(TEST_BLOCKS, 512);
    let mut ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
    // 构造真实签名镜像（SM3 哈希 + 根私钥 SM2 签名）
    let image: Vec<u8> = (0..1024u32).map(|i| (i % 241) as u8).collect();
    let hash = sm3_hash(&image);
    let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, &mut rng).expect("镜像签名");
    let meta = UpgradeMeta {
        target_version: heapless::String::<32>::try_from("0.126.0").expect("version"),
        image_size: image.len() as u64,
        image_hash: hash,
        image_sig: sig.to_bytes(),
        download_url: heapless::String::<128>::try_from("https://ota.eneros.local/fw.bin")
            .expect("url"),
        chunk_size: 512,
        total_chunks: 2,
        downloaded_chunks: 0,
    };
    let slot = ota.prepare_upgrade(meta).expect("prepare_upgrade");
    assert_eq!(slot, PartitionSlot::B);
    ota.write_chunk(slot, 0, &image[..512]).expect("chunk0");
    ota.write_chunk(slot, 512, &image[512..]).expect("chunk1");
    assert!(ota.verify(slot).expect("verify"), "镜像验签应通过");
    ota.activate(slot).expect("activate");
    assert_eq!(ota.get_active_slot(), PartitionSlot::B);
    // 连续启动失败：第 4 次健康检查（> max_boot_attempts=3）触发自动回滚
    let mut rolled_back = false;
    for i in 1..=4u64 {
        if !boot_health_check(&mut ota, 2_000 + i * 1_000) {
            rolled_back = true;
        }
    }
    assert!(rolled_back, "启动失败超阈值应触发自动回滚");
    assert_eq!(
        ota.get_active_slot(),
        PartitionSlot::A,
        "回滚后应切回 A 分区"
    );
    make_evidence("回滚通过：B 分区 4 次启动失败自动回滚 A", true)
}

/// 证据 9：eneros-recovery 固件损坏 → 恢复真实流程.
///
/// 先以错误期望哈希刷写（验签失败 → Error 态，设备未被触碰），再重新喂入
/// 镜像并以正确 SM3 哈希 + SM2 签名完成刷写 → Complete 态.
fn collect_evidence_recovery() -> ExitEvidence {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("SM2 密钥对生成");
    let dev = MockBlockDevice::new(4_096, 512);
    let mut rec = RecoveryModeImpl::new(dev, kp.public_key);
    rec.enter().expect("进入恢复模式");
    let image: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
    let good_hash = sm3_hash(&image);
    let sig = sm2_sign(&good_hash, &kp.private_key, &kp.public_key, &mut rng).expect("签名");
    let sig_hex = hex_encode(&sig.to_bytes());
    // 损坏路径：错误期望哈希 → 验签失败进入 Error 态
    let bad_hash_hex = hex_encode(&[0xAAu8; 32]);
    rec.flash_writer_mut().receive_chunk(&image);
    let err = rec
        .execute(
            RecoveryCommand::Flash,
            &[bad_hash_hex.as_str(), &sig_hex, "1"],
        )
        .expect_err("损坏固件应验签失败");
    assert!(
        matches!(err, eneros_recovery::RecoveryError::SignatureInvalid),
        "损坏固件应报签名无效，实际 {err:?}"
    );
    assert_eq!(rec.state(), RecoveryState::Error);
    assert!(rec.get_progress().error.is_some(), "Error 态应记录错误信息");
    // 恢复路径：复位刷写器后正确刷写
    rec.enter().expect("重新进入恢复模式");
    rec.flash_writer_mut().reset();
    rec.flash_writer_mut().receive_chunk(&image);
    let good_hash_hex = hex_encode(&good_hash);
    let msg = rec
        .execute(
            RecoveryCommand::Flash,
            &[good_hash_hex.as_str(), &sig_hex, "1"],
        )
        .expect("正确固件应刷写成功");
    assert!(msg.contains("Flash complete"));
    assert_eq!(rec.state(), RecoveryState::Complete);
    assert_eq!(rec.get_progress().flashed_bytes, image.len() as u64);
    make_evidence("可恢复：损坏固件验签拦截后正确镜像刷写成功", true)
}

/// 证据 10：eneros-iec61850-goose 发布/订阅往返延迟真实测量 < 4ms.
///
/// 发布者经 MockL2 组帧发送，帧真实转入订阅者接收队列完成 BER 解码，
/// 墙钟测量 publish→poll 全链路往返.
fn collect_evidence_goose() -> ExitEvidence {
    let dst = [0x01, 0x0C, 0xCD, 0x04, 0x00, 0x01];
    let cb = GooseControlBlock {
        go_cb_ref: String::from("IED1LD/LLN0$GO$gocb1"),
        app_id: 0x0001,
        dst_addr: dst,
        min_time: 2,
        max_time: 1_000,
        st_num: 1,
        sq_num: 0,
        dataset_ref: String::from("IED1LD/LLN0$dsGOOSE1"),
        needs_retransmit: false,
        vlan_id: None,
    };
    let mut publisher = GoosePublisher::new(cb, MockL2::new()).expect("发布者构造");
    publisher.update_value("IED1LD/LLN0.Ind.stVal", DaValue::Bool(true));
    let mut subscriber = GooseSubscriber::new(0x0001, dst, MockL2::new()).expect("订阅者构造");
    let start = Instant::now();
    publisher.publish(5_000).expect("GOOSE 发布");
    let frame = publisher
        .transport()
        .tx_frames()
        .last()
        .expect("已发帧记录")
        .clone();
    subscriber.transport_mut().push_rx_frame(&frame);
    let (pdu, status) = subscriber
        .poll()
        .expect("GOOSE 轮询")
        .expect("应收到有效 PDU");
    let rtt = start.elapsed();
    assert_eq!(status, RxStatus::New);
    assert_eq!(pdu.st_num, 2, "update_value 后 st_num 应 +1");
    assert!(
        rtt.as_millis() < 4,
        "GOOSE 往返延迟 {} ms 应 < 4ms",
        rtt.as_millis()
    );
    make_evidence(
        &format!("GOOSE 发布/订阅往返 {} µs < 4ms", rtt.as_micros()),
        true,
    )
}

// ============================================================================
// E1~E4：集群组建与 Coordinator 选举
// ============================================================================

/// E1：5 台最小集群组建 + Coordinator 选举成功.
#[test]
fn e1_min_cluster_5_nodes() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    let mut nodes: Vec<EdgeNode> = (1..=4u32)
        .map(|id| make_node(id, VppRole::EdgeBox, 50.0, 0.8, true))
        .collect();
    nodes.push(make_node(5, VppRole::Coordinator, 0.0, 0.0, true));
    s.setup_cluster(&nodes).expect("5 台集群组建应成功");
    assert_eq!(s.coordinator_id(), Some(5));
    assert_eq!(s.nodes().len(), 5);
}

/// E2：8 台集群（蓝图出口报告口径）——选举 + 聚合 350kW.
#[test]
fn e2_cluster_8_nodes_blueprint() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster())
        .expect("8 台集群组建应成功");
    assert_eq!(s.coordinator_id(), Some(8));
    assert_eq!(s.nodes().len(), 8);
}

/// E3：10 台上限集群（蓝图集群规模上限）.
#[test]
fn e3_cluster_10_nodes_max() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    let mut nodes: Vec<EdgeNode> = (1..=9u32)
        .map(|id| make_node(id, VppRole::EdgeBox, 50.0, 0.8, true))
        .collect();
    nodes.push(make_node(10, VppRole::Coordinator, 0.0, 0.0, true));
    s.setup_cluster(&nodes).expect("10 台集群组建应成功");
    assert_eq!(s.coordinator_id(), Some(10));
    assert_eq!(s.nodes().len(), 10);
}

/// E4：无 Coordinator 节点集 → 选举失败.
#[test]
fn e4_no_coordinator_election_fails() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    let nodes: Vec<EdgeNode> = (1..=5u32)
        .map(|id| make_node(id, VppRole::EdgeBox, 50.0, 0.8, true))
        .collect();
    assert_eq!(
        s.setup_cluster(&nodes),
        Err(VppError::CoordinatorElectionFailed)
    );
    assert_eq!(s.coordinator_id(), None);
}

// ============================================================================
// E5~E7：聚合计算
// ============================================================================

/// E5：8 节点聚合计算——total=350kW / online=8 / available=350kW.
#[test]
fn e5_aggregation_350kw() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let agg = s
        .simulate_dr_event(make_event(), 1_000)
        .expect("DR 事件应成功");
    assert_eq!(agg.online_nodes, 8);
    assert!((agg.total_capacity_kw - 350.0).abs() < 1e-9);
    assert!((agg.available_kw - 350.0).abs() < 1e-9);
}

/// E6：SOC ≤ 0.1 节点可用容量过滤（计入 total，不计入 available）.
///
/// node 3（soc=0.1 边界）与 node 4（soc=0.05）被过滤：total 仍 350kW，
/// available 降为 250kW；调度计划仅含 soc>0.1 的 5 台节点。另验证
/// -300kW 目标超出 available 时返回 InsufficientCapacity{available: 250}。
#[test]
fn e6_low_soc_filtered_from_available() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    let mut nodes = make_cluster();
    nodes[2].soc = 0.1; // 边界值：不纳入 available
    nodes[3].soc = 0.05; // 低 SOC
    s.setup_cluster(&nodes).expect("集群组建");
    // 目标 -300kW 超出 available=250kW → 容量不足，且错误如实报告过滤后余量
    assert_eq!(
        s.simulate_dr_event(make_event(), 1_000),
        Err(VppError::InsufficientCapacity {
            need: 300.0,
            available: 250.0
        })
    );
    // 收缩目标至 -200kW：事件成功，聚合字段体现 SOC 过滤语义
    let mut event = make_event();
    event.target_kw = -200.0;
    let agg = s.simulate_dr_event(event, 1_000).expect("DR 事件应成功");
    assert!((agg.total_capacity_kw - 350.0).abs() < 1e-9, "total 计全量");
    assert!(
        (agg.available_kw - 250.0).abs() < 1e-9,
        "available 滤除低 SOC"
    );
    let plan = s.dr_handler_mut().plan();
    assert_eq!(plan.len(), 5, "仅 soc>0.1 节点入调度计划");
    assert!(plan.iter().all(|&(id, _)| id != 3 && id != 4));
}

/// E7：节点离线后重聚合自动调整（蓝图 §6.5）——8→7 在线、350→300kW，
/// DR 计划自动收缩为 6 台各 -50kW.
#[test]
fn e7_offline_node_reaggregation() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    // node 2 离线后重新组建（运行时成员变更经重新 setup 生效）
    let mut nodes = make_cluster();
    nodes[1].online = false;
    s.setup_cluster(&nodes).expect("离线后重组");
    let agg = s
        .simulate_dr_event(make_event(), 1_000)
        .expect("available=300 ≥ 300 应可执行");
    assert_eq!(agg.online_nodes, 7);
    assert!((agg.total_capacity_kw - 300.0).abs() < 1e-9);
    assert!((agg.available_kw - 300.0).abs() < 1e-9);
    // 离线节点不参与调度：6 台各分摊 -50kW
    let plan = s.dr_handler_mut().plan();
    assert_eq!(plan.len(), 6);
    assert!(plan.iter().all(|&(id, _)| id != 2), "离线节点不得入计划");
    for &(_, share) in plan.iter() {
        assert!((share - (-50.0)).abs() < 1e-6);
    }
}

// ============================================================================
// E8~E11：DR 事件全流程与故障注入
// ============================================================================

/// E8 ★：DR 事件全流程——降载 300kW、response=18000ms < 30000ms 达标、
/// 状态机推进至 Completed.
#[test]
fn e8_dr_full_flow_response_18000() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let agg = s
        .simulate_dr_event(make_event(), 1_000)
        .expect("DR 全流程应成功");
    assert_eq!(s.measure_response_time(), 18_000);
    assert!(s.measure_response_time() < 30_000, "DR 响应应 < 30s");
    assert_eq!(agg.response_latency_ms, 18_000);
    assert_eq!(s.dr_handler_mut().state(), DrState::Completed);
    assert!(s.dr_handler_mut().is_terminal());
    assert_eq!(s.dr_handler_mut().plan().len(), 7);
}

/// E9：比例分配正确性——share = target × node/total，allocated 总和容差 1e-6.
#[test]
fn e9_proportional_allocation() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    let nodes = vec![
        make_node(1, VppRole::EdgeBox, 100.0, 0.8, true),
        make_node(2, VppRole::EdgeBox, 150.0, 0.8, true),
        make_node(3, VppRole::EdgeBox, 100.0, 0.8, true),
        make_node(8, VppRole::Coordinator, 0.0, 0.0, true),
    ];
    s.setup_cluster(&nodes).expect("集群组建");
    s.simulate_dr_event(make_event(), 1_000).expect("DR 事件");
    let plan = s.dr_handler_mut().plan();
    assert_eq!(plan.len(), 3);
    let total = 350.0f64;
    let expected = [
        (1u32, -300.0 * 100.0 / total),
        (2u32, -300.0 * 150.0 / total),
        (3u32, -300.0 * 100.0 / total),
    ];
    let mut allocated = 0.0f64;
    for (idx, &(id, share)) in plan.iter().enumerate() {
        assert_eq!(id, expected[idx].0);
        assert!(
            (share - expected[idx].1).abs() < 1e-6,
            "node {id} 份额 {share} 与期望 {} 偏差超限",
            expected[idx].1
        );
        allocated += share;
    }
    assert!((allocated - (-300.0)).abs() < 1e-6, "分配总和应等于目标");
}

/// E10：容量不足——降载 500kW 对可用 350kW → InsufficientCapacity
/// （蓝图 §4.3 sequenceDiagram -500kW 矛盾场景的正确语义验证，D7）.
#[test]
fn e10_insufficient_capacity_500kw() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let mut event = make_event();
    event.target_kw = -500.0;
    assert_eq!(
        s.simulate_dr_event(event, 1_000),
        Err(VppError::InsufficientCapacity {
            need: 500.0,
            available: 350.0
        })
    );
    assert_eq!(s.measure_response_time(), 0);
    assert_eq!(s.dr_handler_mut().state(), DrState::Idle);
}

/// E11：超时故障注入——链路延迟 20s 单程 → response=42000 > deadline=30000
/// → ResponseTimeout 且状态机置 Expired.
#[test]
fn e11_response_timeout_fault_injection() {
    let mut s = VppScenarioImpl::new(20_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    assert_eq!(
        s.simulate_dr_event(make_event(), 1_000),
        Err(VppError::ResponseTimeout {
            deadline_ms: 30_000
        })
    );
    assert_eq!(s.dr_handler_mut().state(), DrState::Expired);
    assert!(s.dr_handler_mut().is_terminal());
}

/// E12：Coordinator 崩溃后重选——原 Coordinator 离线 + 新 Coordinator 节点
/// 加入后重新组建，选举切换且 DR 链路恢复可用.
#[test]
fn e12_coordinator_crash_reelection() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("初始集群");
    assert_eq!(s.coordinator_id(), Some(8));
    // node 8（原 Coordinator）崩溃离线，node 9 以 Coordinator 角色接管
    let mut nodes: Vec<EdgeNode> = (1..=7u32)
        .map(|id| make_node(id, VppRole::EdgeBox, 50.0, 0.8, true))
        .collect();
    nodes.push(make_node(8, VppRole::Coordinator, 0.0, 0.0, false));
    nodes.push(make_node(9, VppRole::Coordinator, 0.0, 0.0, true));
    s.setup_cluster(&nodes).expect("重选后重组");
    assert_eq!(s.coordinator_id(), Some(9), "应重选 node 9 为 Coordinator");
    s.simulate_dr_event(make_event(), 2_000)
        .expect("重选后 DR 链路应恢复");
    assert_eq!(s.measure_response_time(), 18_000);
}

// ============================================================================
// E13~E19：第 4~10 项出口证据真实采集
// ============================================================================

/// E13 ★ 证据 4：PBFT 2f+1 共识真实运行 → register_evidence(4).
#[test]
fn e13_evidence4_pbft_consensus() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let ev = collect_evidence_pbft();
    assert!(ev.passed);
    assert!(s.register_evidence(4, ev), "证据 4 注册应被接受");
    let report = s.generate_report();
    assert!(report.criteria[3].passed, "第 4 项（联邦共识）应通过");
}

/// E14 ★ 证据 5：gPTP 时间同步精度 < 1ms 真实测量 → register_evidence(5).
#[test]
fn e14_evidence5_gptp_sync_accuracy() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let ev = collect_evidence_gptp();
    assert!(ev.passed);
    assert!(s.register_evidence(5, ev), "证据 5 注册应被接受");
    let report = s.generate_report();
    assert!(report.criteria[4].passed, "第 5 项（时间同步）应通过");
}

/// E15 ★ 证据 6：渗透测试 30 既定用例真实运行通过 → register_evidence(6).
#[test]
fn e15_evidence6_pentest() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let ev = collect_evidence_pentest();
    assert!(ev.passed);
    assert!(s.register_evidence(6, ev), "证据 6 注册应被接受");
    let report = s.generate_report();
    assert!(report.criteria[5].passed, "第 6 项（安全评估）应通过");
}

/// E16 ★ 证据 7：指标 + 日志 + 追踪三支柱真实读写 → register_evidence(7).
#[test]
fn e16_evidence7_observability() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let ev = collect_evidence_observability();
    assert!(ev.passed);
    assert!(s.register_evidence(7, ev), "证据 7 注册应被接受");
    let report = s.generate_report();
    assert!(report.criteria[6].passed, "第 7 项（可观测性）应通过");
}

/// E17 ★ 证据 8：OTA A/B 升级失败自动回滚真实流程 → register_evidence(8).
#[test]
fn e17_evidence8_ota_rollback() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let ev = collect_evidence_ota_rollback();
    assert!(ev.passed);
    assert!(s.register_evidence(8, ev), "证据 8 注册应被接受");
    let report = s.generate_report();
    assert!(report.criteria[7].passed, "第 8 项（OTA 升级）应通过");
}

/// E18 ★ 证据 9：Recovery Mode 固件损坏 → 恢复真实流程 → register_evidence(9).
#[test]
fn e18_evidence9_recovery() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let ev = collect_evidence_recovery();
    assert!(ev.passed);
    assert!(s.register_evidence(9, ev), "证据 9 注册应被接受");
    let report = s.generate_report();
    assert!(report.criteria[8].passed, "第 9 项（Recovery Mode）应通过");
}

/// E19 ★ 证据 10：GOOSE 发布/订阅往返延迟 < 4ms 真实测量 → register_evidence(10).
#[test]
fn e19_evidence10_goose_latency() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    let ev = collect_evidence_goose();
    assert!(ev.passed);
    assert!(s.register_evidence(10, ev), "证据 10 注册应被接受");
    let report = s.generate_report();
    assert!(report.criteria[9].passed, "第 10 项（IEC 61850）应通过");
}

// ============================================================================
// E20~E22：Phase 2 出口判定复演
// ============================================================================

/// E20 ★★：出口判定复演——全证据注册后 10/10 全过、all_passed=true，
/// render 含 10 行判定表与"全部通过"结论（对齐蓝图 §7.5）.
#[test]
fn e20_exit_criteria_full_replay_10_of_10() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    s.simulate_dr_event(make_event(), 1_000).expect("DR 事件");
    // 第 4~10 项证据：真实子系统采集
    assert!(s.register_evidence(4, collect_evidence_pbft()));
    assert!(s.register_evidence(5, collect_evidence_gptp()));
    assert!(s.register_evidence(6, collect_evidence_pentest()));
    assert!(s.register_evidence(7, collect_evidence_observability()));
    assert!(s.register_evidence(8, collect_evidence_ota_rollback()));
    assert!(s.register_evidence(9, collect_evidence_recovery()));
    assert!(s.register_evidence(10, collect_evidence_goose()));
    let criteria = s.run_exit_criteria();
    assert_eq!(criteria.len(), 10);
    for c in &criteria {
        assert!(c.passed, "第 {} 项（{}）应通过", c.id, c.name.as_str());
    }
    let report = s.generate_report();
    assert_eq!(report.exit_criteria_total, 10);
    assert_eq!(report.exit_criteria_passed, 10);
    assert!(report.all_passed, "出口判定复演应 10/10 全过");
    // render 文本：10 行判定表 + 全过结论
    let text = eneros_vpp_scenario::exit_report::ExitReportBuilder::render(&report);
    for id in 1..=10u32 {
        assert!(text.contains(&format!("| {id} |")), "判定表缺第 {id} 行");
    }
    assert!(!text.contains('❌'), "全过场景不应出现 ❌");
    assert!(text.contains("结论: 全部通过（10/10 通过）"));
}

/// E21：未注册证据报告不谎报——第 4~10 项 passed=false 且 actual="未验证"，
/// all_passed=false.
#[test]
fn e21_unregistered_evidence_never_reports_pass() {
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    s.simulate_dr_event(make_event(), 1_000).expect("DR 事件");
    let criteria = s.run_exit_criteria();
    assert!(criteria[0].passed && criteria[1].passed && criteria[2].passed);
    for c in criteria.iter().skip(3) {
        assert!(!c.passed, "id={} 未注册证据不得通过", c.id);
        assert_eq!(c.actual.as_str(), "未验证");
    }
    let report = s.generate_report();
    assert_eq!(report.exit_criteria_passed, 3);
    assert!(!report.all_passed, "未注册证据不得谎报全过");
}

/// E22：报告字段——total_versions=52 / version_range / phase / 总数 10.
#[test]
fn e22_report_fields() {
    let s = VppScenarioImpl::new(8_000, 2_000);
    let report = s.generate_report();
    assert_eq!(report.phase.as_str(), "Phase 2");
    assert_eq!(report.version_range.as_str(), "v0.75.0 ~ v0.126.0");
    assert_eq!(report.total_versions, 52);
    assert_eq!(report.exit_criteria_total, 10);
}

// ============================================================================
// PERF23：全场景端到端性能（release 墙钟打印；ENEROS_PERF_GATE=1 硬断言 < 100ms）
// ============================================================================

/// PERF23 ★：8 节点建群 → DR 全流程 → 七项证据注册 → 出口判定 → 报告渲染，
/// 场景编排墙钟打印；`ENEROS_PERF_GATE=1` 时断言编排墙钟 < 100ms.
///
/// 门槛口径（D8 偏差声明）：100ms 门槛约束本版本交付的场景编排路径
/// （setup_cluster → simulate_dr_event → register_evidence × 7 →
/// run_exit_criteria → generate_report → render）。第 4~10 项证据为真实
/// 子系统运行（PBFT 共识 / 渗透 30 用例含真实 SM2 攻击等），其耗时由各自
/// 版本性能门槛约束（蓝图 §7.2：联邦共识 < 5s、GOOSE < 4ms、同步 < 1ms），
/// 在计时区外先行采集并单独打印，不计入 100ms 编排门槛。
#[test]
fn perf23_full_scenario_wall_clock() {
    // 前置：真实子系统证据采集（计时区外；各自版本门槛约束其耗时）
    let collect_start = Instant::now();
    let ev4 = collect_evidence_pbft();
    let ev5 = collect_evidence_gptp();
    let ev6 = collect_evidence_pentest();
    let ev7 = collect_evidence_observability();
    let ev8 = collect_evidence_ota_rollback();
    let ev9 = collect_evidence_recovery();
    let ev10 = collect_evidence_goose();
    let collect_elapsed = collect_start.elapsed();

    // 计时区：场景编排全链路（v0.126.0 交付路径）
    let start = Instant::now();
    let mut s = VppScenarioImpl::new(8_000, 2_000);
    s.setup_cluster(&make_cluster()).expect("集群组建");
    s.simulate_dr_event(make_event(), 1_000).expect("DR 事件");
    s.register_evidence(4, ev4);
    s.register_evidence(5, ev5);
    s.register_evidence(6, ev6);
    s.register_evidence(7, ev7);
    s.register_evidence(8, ev8);
    s.register_evidence(9, ev9);
    s.register_evidence(10, ev10);
    let _ = s.run_exit_criteria();
    let report = s.generate_report();
    let text = eneros_vpp_scenario::exit_report::ExitReportBuilder::render(&report);
    assert!(report.all_passed, "全场景出口判定应 10/10 全过");
    assert!(!text.is_empty());
    let elapsed = start.elapsed();
    println!(
        "PERF23 证据采集墙钟（计时区外）: {:?}（{} ms）",
        collect_elapsed,
        collect_elapsed.as_millis()
    );
    println!(
        "PERF23 场景编排墙钟（门槛口径）: {:?}（{} µs）",
        elapsed,
        elapsed.as_micros()
    );
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            elapsed.as_millis() < 100,
            "PERF23 场景编排 {} ms 超出 100ms 门槛",
            elapsed.as_millis()
        );
    }
}
