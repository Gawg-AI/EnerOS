//! v0.122.0 分布式追踪 集成测试（host std 环境）.
//!
//! - INT1：★ 跨 Agent A→B→C 链路——三个 Agent 各自持有 `TracerImpl`，
//!   A start_span("federate_request") 得 S1 → 经 26B 头部（encode/decode）
//!   传递 TraceContext 到 B → B 以该 ctx 为 parent start_span("forward")
//!   得 S2 → 同样到 C 得 S3 → finish 顺序 C→B→A（跨 Agent 响应回传）→
//!   汇聚三个 Tracer 的入库 spans，按同一 trace_id 过滤重建 S1←S2←S3
//!   完整链路（同 trace_id、parent 链正确、sampled 一致），render_trace
//!   输出含三个 operation 的缩进树（D5：以模拟消息验证传播，不依赖
//!   eneros-agent-bus-dds）；
//! - INT2：★ v0.121.0 日志关联回归——以 span trace_id 前 8 字节大端解读
//!   为 u64 写入 `LogBuilder::trace(...)`（并写入 span_id 大端 u64），
//!   log → flush_to(MemoryLogStorage) → 按该 trace_id 经 LogFilter 查询
//!   命中且条数与链路 span 数一致、逐条 span_id 与链路 span 对应；
//! - INT3：★ 出口判定复演（蓝图 §7.5 链路可视化）——完整链路
//!   render_trace 文本树结构正确：根无缩进、子缩进 2 空格、孙 4 空格，
//!   每行含 operation/agent/耗时 ms/状态标记；
//! - INT4：故障注入（蓝图 §6.5）——Agent B 崩溃 S2 未 finish →
//!   incomplete_spans(trace_id) 检出 S2 + render_trace 中 S2 标记未完成；
//! - PERF5：release 模式打印 ① span 创建均值（10240 次 = 20 批 × 512，
//!   每批新 TracerImpl 避免满库 remove(0) 干扰纯创建路径）② 26B 编解码
//!   均值（100000 次 encode+decode）；仅当 `ENEROS_PERF_GATE=1`（值恰为
//!   "1"，规避终端残留空串误激活）时断言均值 < 5µs / < 1µs（蓝图
//!   §6.3/§7.2），默认仅打印不断言（与 v0.113.0~v0.121.0 PERF 先例一致）。
//!
//! 集成测试运行于 host std 环境（`std::time::Instant`/`std::env` 可用），
//! 通过 `eneros_tracing` crate 公共 API 调用被测的 no_std 代码；INT2 另经
//! dev-dependency `eneros_logging` 验证跨 crate 日志关联。

// 测试代码允许 panic!/unwrap（与 src 内嵌测试模块同口径）
#![allow(clippy::disallowed_macros)]

use eneros_logging::{
    LogBuilder, LogFilter, LogLevel, LogStorage, MemoryLogStorage, StructuredLogger,
    StructuredLoggerImpl,
};
use eneros_tracing::{
    decode_trace_context, encode_trace_context, render_trace, Span, Tracer, TracerImpl,
};
use heapless::String as HString;

/// 设置 span 的 agent_id（汇聚后按来源 Agent 标记，测试代码允许 unwrap）.
fn mark_agent(span: &mut Span, agent: &str) {
    span.agent_id = HString::<32>::try_from(agent).unwrap();
}

/// 汇聚各 Tracer 中属于 `trace_id` 的入库 span 并按来源标记 agent_id.
///
/// 模拟运维侧链路重建：跨 Agent 场景下 span 分散在各 Agent 本地存储，
/// 汇聚（如经上报通道收集）后按 trace_id 过滤即可重建完整调用链。
fn gather_trace(trace_id: &eneros_tracing::TraceId, sources: &[(&TracerImpl, &str)]) -> Vec<Span> {
    let mut all: Vec<Span> = Vec::new();
    for (tracer, agent) in sources {
        for mut s in tracer.get_trace(trace_id) {
            mark_agent(&mut s, agent);
            all.push(s);
        }
    }
    all
}

/// 创建堆上 TracerImpl（内联 heapless::Vec\<Span, 512\> 约 650KB，直接放
/// 测试线程栈上多个实例会溢出 2MB 默认栈，故 Box 堆分配）.
fn boxed_tracer(rate: f32) -> Box<TracerImpl> {
    Box::new(TracerImpl::new(rate))
}

/// INT1：★ 跨 Agent A→B→C 链路——26B 头部传播 + finish C→B→A + 汇聚重建.
#[test]
fn int1_cross_agent_trace_chain() {
    // 三个 Agent 各自持有 TracerImpl（跨 Agent 不共享存储；Box 堆分配防栈溢出）
    let mut agent_a = boxed_tracer(1.0);
    let mut agent_b = boxed_tracer(1.0);
    let mut agent_c = boxed_tracer(1.0);

    // A：接收联邦请求，开启根 Span S1（T1 由 now_ms + 计数器生成）
    let mut s1 = agent_a.start_span("federate_request", None, 1000);

    // A → B：TraceContext 经 26B 头部随消息传播（D5 模拟 DDS 消息）
    let wire_ab = encode_trace_context(&agent_a.inject_context(&s1));
    let ctx_b = agent_b
        .extract_context(&decode_trace_context(&wire_ab))
        .expect("B 应能提取合法上下文");
    // B：以 ctx(T1, S1) 为 parent 开启 S2（父 Span 不在 B 本地存储，D3 核心）
    let mut s2 = agent_b.start_span("forward", Some(&ctx_b), 1010);

    // B → C：同样经 26B 头部传播
    let wire_bc = encode_trace_context(&agent_b.inject_context(&s2));
    let ctx_c = agent_c
        .extract_context(&decode_trace_context(&wire_bc))
        .expect("C 应能提取合法上下文");
    // C：以 ctx(T1, S2) 为 parent 开启 S3
    let mut s3 = agent_c.start_span("execute", Some(&ctx_c), 1020);

    // 干扰项：B 上另一条无关 trace，验证汇聚过滤只取目标链路
    let _noise = agent_b.start_span("unrelated_housekeeping", None, 5000);

    // finish 顺序 C→B→A（跨 Agent 响应逐级回传）
    agent_c.finish_span(&mut s3, 1030);
    agent_b.finish_span(&mut s2, 1040);
    agent_a.finish_span(&mut s1, 1050);

    // 汇聚三个 Tracer 的入库 span，按同一 trace_id 过滤重建链路
    let trace = gather_trace(
        &s1.trace_id,
        &[
            (agent_a.as_ref(), "agent-A"),
            (agent_b.as_ref(), "agent-B"),
            (agent_c.as_ref(), "agent-C"),
        ],
    );
    assert_eq!(trace.len(), 3, "目标链路应恰好 3 个 span（干扰项被过滤）");
    // ① 同 trace_id T1（且等于 S1 的 trace_id）
    assert!(trace.iter().all(|s| s.trace_id == s1.trace_id));
    // ② 父子关系 S1←S2←S3
    let (g1, g2, g3) = (&trace[0], &trace[1], &trace[2]);
    assert_eq!(g1.span_id, s1.span_id);
    assert_eq!(g1.parent_span_id, None, "S1 为根");
    assert_eq!(g2.span_id, s2.span_id);
    assert_eq!(g2.parent_span_id, Some(s1.span_id), "S2 父为 S1");
    assert_eq!(g3.span_id, s3.span_id);
    assert_eq!(g3.parent_span_id, Some(s2.span_id), "S3 父为 S2");
    // ③ sampled 全链一致（D4：trace 创建时判定随上下文传播）
    assert!(trace.iter().all(|s| s.sampled));
    // ④ 全部 Finished 且耗时正确（end - start）
    assert_eq!(g1.end_ms - g1.start_ms, 50);
    assert_eq!(g2.end_ms - g2.start_ms, 30);
    assert_eq!(g3.end_ms - g3.start_ms, 10);
    // ⑤ 干扰项确在 B 库存储但属另一 trace（汇聚口径不含它）
    assert_eq!(agent_b.stored(), 2);

    // render_trace 输出含三个 operation 的缩进树
    let tree = render_trace(&trace);
    assert!(tree.starts_with("federate_request [agent-A] 50ms OK\n"));
    assert!(tree.contains("\n  forward [agent-B] 30ms OK\n"));
    assert!(tree.contains("\n    execute [agent-C] 10ms OK\n"));
}

/// INT2：★ v0.121.0 日志关联回归——trace_id 前 8 字节大端 u64 写入日志后
/// 可按该 trace_id 检索命中，且逐条 span_id 与链路 span 对应.
#[test]
fn int2_logging_trace_id_correlation() {
    // 构造 A→B→C 完整链路（与 INT1 同口径，时间戳错开避免歧义）
    let mut agent_a = boxed_tracer(1.0);
    let mut agent_b = boxed_tracer(1.0);
    let mut agent_c = boxed_tracer(1.0);
    let mut s1 = agent_a.start_span("federate_request", None, 2000);
    let ctx_b = agent_b
        .extract_context(&decode_trace_context(&encode_trace_context(
            &agent_a.inject_context(&s1),
        )))
        .unwrap();
    let mut s2 = agent_b.start_span("forward", Some(&ctx_b), 2010);
    let ctx_c = agent_c
        .extract_context(&decode_trace_context(&encode_trace_context(
            &agent_b.inject_context(&s2),
        )))
        .unwrap();
    let mut s3 = agent_c.start_span("execute", Some(&ctx_c), 2020);
    agent_c.finish_span(&mut s3, 2030);
    agent_b.finish_span(&mut s2, 2040);
    agent_a.finish_span(&mut s1, 2050);
    let spans = [s1.clone(), s2.clone(), s3.clone()];

    // 以 span trace_id 前 8 字节大端解读为 u64（日志 LogEntry.trace_id 口径）
    let log_trace_id = u64::from_be_bytes(s1.trace_id.0[..8].try_into().unwrap());
    assert_eq!(
        log_trace_id, 2000,
        "trace_id 前 8 字节 = 创建时刻 now_ms 大端"
    );

    // 每个 span 产生一条日志：trace() 写派生 u64、span() 写 span_id 大端 u64
    let mut logger = StructuredLoggerImpl::new(LogLevel::Debug);
    let mut store = MemoryLogStorage::new(16);
    for (i, sp) in spans.iter().enumerate() {
        let e = LogBuilder::new(
            LogLevel::Info,
            "agent.federate",
            "联邦请求处理",
            3000 + i as u64,
        )
        .trace(log_trace_id)
        .span(u64::from_be_bytes(sp.span_id.0))
        .agent(i as u32)
        .field("operation", sp.operation_name.as_str())
        .build();
        logger.log(e).unwrap();
    }
    // 干扰项：另一 trace 的日志，验证检索不误中
    logger
        .log(
            LogBuilder::new(LogLevel::Info, "agent.federate", "无关请求", 4000)
                .trace(9999)
                .build(),
        )
        .unwrap();
    let flushed = logger.flush_to(&mut store).unwrap();
    assert_eq!(flushed, 4, "3 条链路日志 + 1 条干扰日志应全部刷入");

    // 按该 trace_id 查询：命中且条数与链路 span 数一致
    let got = store
        .query(&LogFilter {
            trace_id: Some(log_trace_id),
            ..LogFilter::default()
        })
        .unwrap();
    assert_eq!(got.len(), spans.len(), "应按 trace_id 恰好命中 3 条");
    // 逐条与链路 span 对应：span_id 大端 u64 一致、operation 字段一致
    for (entry, sp) in got.iter().zip(spans.iter()) {
        assert_eq!(entry.trace_id, log_trace_id);
        assert_eq!(entry.span_id, u64::from_be_bytes(sp.span_id.0));
        assert!(entry
            .fields
            .iter()
            .any(|(k, v)| k.as_str() == "operation" && v.as_str() == sp.operation_name.as_str()));
    }
    // 干扰 trace 独立可查，且不与本链路混淆
    let noise = store
        .query(&LogFilter {
            trace_id: Some(9999),
            ..LogFilter::default()
        })
        .unwrap();
    assert_eq!(noise.len(), 1);
}

/// INT3：★ 出口判定复演（蓝图 §7.5）——完整链路 render_trace 文本树结构正确.
#[test]
fn int3_render_trace_tree_structure() {
    let mut agent_a = boxed_tracer(1.0);
    let mut agent_b = boxed_tracer(1.0);
    let mut agent_c = boxed_tracer(1.0);
    let mut s1 = agent_a.start_span("federate_request", None, 100);
    let ctx_b = agent_b
        .extract_context(&decode_trace_context(&encode_trace_context(
            &agent_a.inject_context(&s1),
        )))
        .unwrap();
    let mut s2 = agent_b.start_span("forward", Some(&ctx_b), 110);
    let ctx_c = agent_c
        .extract_context(&decode_trace_context(&encode_trace_context(
            &agent_b.inject_context(&s2),
        )))
        .unwrap();
    let mut s3 = agent_c.start_span("execute", Some(&ctx_c), 120);
    agent_c.finish_span(&mut s3, 130);
    agent_b.finish_span(&mut s2, 140);
    agent_a.finish_span(&mut s1, 150);
    let trace = gather_trace(
        &s1.trace_id,
        &[
            (agent_a.as_ref(), "agent-A"),
            (agent_b.as_ref(), "agent-B"),
            (agent_c.as_ref(), "agent-C"),
        ],
    );

    let tree = render_trace(&trace);
    let lines: Vec<&str> = tree.lines().collect();
    // ① 恰好 3 行，与链路 span 一一对应
    assert_eq!(lines.len(), 3);
    // ② 根无缩进：首行不以空格开头，含 operation/agent/耗时/状态
    assert!(!lines[0].starts_with(' '));
    assert_eq!(lines[0], "federate_request [agent-A] 50ms OK");
    // ③ 子缩进 2 空格、孙缩进 4 空格（每层 2 空格递进）
    assert_eq!(lines[1], "  forward [agent-B] 30ms OK");
    assert_eq!(lines[2], "    execute [agent-C] 10ms OK");
    // ④ 每行均含 operation/[agent]/耗时 ms/状态标记四要素
    for (line, (op, agent, cost)) in lines.iter().zip([
        ("federate_request", "agent-A", "50ms"),
        ("forward", "agent-B", "30ms"),
        ("execute", "agent-C", "10ms"),
    ]) {
        assert!(line.contains(op), "行缺 operation：{line}");
        assert!(line.contains(&format!("[{agent}]")), "行缺 agent：{line}");
        assert!(line.contains(cost), "行缺耗时：{line}");
        assert!(line.ends_with("OK"), "行缺状态标记：{line}");
    }
}

/// INT4：故障注入——Agent B 崩溃 S2 未 finish → incomplete_spans 检出 +
/// render_trace 中 S2 标记未完成.
#[test]
fn int4_agent_crash_incomplete_span() {
    let mut agent_a = boxed_tracer(1.0);
    let mut agent_b = boxed_tracer(1.0);
    // A 开启根 Span S1 并转发到 B；B 开启 S2 后"崩溃"：不 finish、不回传
    let mut s1 = agent_a.start_span("federate_request", None, 7000);
    let ctx_b = agent_b
        .extract_context(&decode_trace_context(&encode_trace_context(
            &agent_a.inject_context(&s1),
        )))
        .unwrap();
    let s2 = agent_b.start_span("forward", Some(&ctx_b), 7010);
    // B 崩溃：s2 永不 finish；A 等待超时后结束 S1（响应路径缺失 S2 完成事件）
    agent_a.finish_span(&mut s1, 7999);

    // ① B 本地 incomplete_spans 检出 S2（Active 未结束）
    let inc_b = agent_b.incomplete_spans(&s1.trace_id);
    assert_eq!(inc_b.len(), 1, "B 应恰好检出 1 个未完成 span");
    assert_eq!(inc_b[0].span_id, s2.span_id);
    assert_eq!(inc_b[0].operation_name.as_str(), "forward");
    assert_eq!(inc_b[0].status, eneros_tracing::SpanStatus::Active);
    // A 侧无未完成（S1 已 finish）
    assert!(agent_a.incomplete_spans(&s1.trace_id).is_empty());

    // ② 汇聚后 render_trace 中 S2 标记未完成（耗时显示 "-"，无 ms 数值）
    // 崩溃 Agent 的 span 可经最后上报进入汇聚集合，gather 已含 B 的全部入库 span
    let trace = gather_trace(
        &s1.trace_id,
        &[(agent_a.as_ref(), "agent-A"), (agent_b.as_ref(), "agent-B")],
    );
    let tree = render_trace(&trace);
    let lines: Vec<&str> = tree.lines().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "federate_request [agent-A] 999ms OK");
    assert_eq!(lines[1], "  forward [agent-B] - 未完成");
}

/// PERF5：蓝图 §6.3/§7.2 性能基准——
/// ① span 创建均值：10240 次 = 20 批 × 512，每批新 TracerImpl（创建不计时），
///    避免满库 remove(0) 丢最旧的搬移成本干扰纯创建热路径；
/// ② 26B 编解码均值：100000 次 encode+decode 往返取均值。
///
/// 默认仅打印不断言（含 debug/release 口径说明）；仅当
/// `ENEROS_PERF_GATE=1`（`var(...).as_deref() == Ok("1")`，与
/// v0.113.0~v0.121.0 PERF 先例一致）时启用断言门禁（创建 < 5µs、
/// 编解码 < 1µs），面向性能 CI / 目标硬件场景。
#[test]
fn perf5_span_create_and_codec_latency() {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    // 预热 2 批（进入稳态，排除首次分配/缓存冷启动影响）
    for _ in 0..2 {
        let mut warm = boxed_tracer(1.0);
        for i in 0..512u64 {
            let s = warm.start_span("warm_op", None, i);
            std::hint::black_box(&s);
        }
    }

    // ① span 创建均值：分批计时（每批 512，批间重建 TracerImpl 不计时）
    let mut total = std::time::Duration::ZERO;
    let mut done = 0u64;
    while done < 10_000 {
        let mut t = boxed_tracer(1.0);
        let start = std::time::Instant::now();
        for i in 0..512u64 {
            let s = t.start_span("perf_span_op", None, std::hint::black_box(1000 + done + i));
            std::hint::black_box(&s);
        }
        total += start.elapsed();
        done += 512;
    }
    let create_avg = total / done as u32;
    println!(
        "[PERF5-1] span 创建耗时: {done} 次总 {total:?}，均值 {create_avg:?}（{profile} 构建；设 ENEROS_PERF_GATE=1 启用 <5µs 断言门禁）"
    );

    // ② 26B 编解码均值：encode+decode 往返 100000 次整体计时取均值
    let mut tracer = boxed_tracer(1.0);
    let span = tracer.start_span("codec_ref", None, 42);
    let ctx = tracer.inject_context(&span);
    let rounds = 100_000u32;
    let start = std::time::Instant::now();
    for _ in 0..rounds {
        let wire = encode_trace_context(std::hint::black_box(&ctx));
        let back = decode_trace_context(std::hint::black_box(&wire));
        std::hint::black_box(&back);
    }
    let round = start.elapsed();
    let codec_avg = round / rounds;
    println!(
        "[PERF5-2] 26B 编解码耗时: {rounds} 次往返总 {round:?}，均值 {codec_avg:?}（{profile} 构建；设 ENEROS_PERF_GATE=1 启用 <1µs 断言门禁）"
    );
    // 往返正确性顺带回归
    assert_eq!(decode_trace_context(&encode_trace_context(&ctx)), ctx);

    // 性能门禁：仅 ENEROS_PERF_GATE=1 时启用断言（蓝图 §6.3/§7.2）
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            create_avg.as_nanos() < 5_000,
            "PERF5 span 创建均值 {create_avg:?} 超过 5µs 上限（蓝图 §7.2）"
        );
        assert!(
            codec_avg.as_nanos() < 1_000,
            "PERF5 编解码均值 {codec_avg:?} 超过 1µs 上限（蓝图 §6.3）"
        );
    }
}
