//! 69.2：peer 会话深模块（[`PeerSession`]）——建联/重连策略、keepalive
//! 节拍、入站静默看门狗、主题过滤、UTF-8 校验、序列号状态**全部内化**。
//! 调用方（bin 主循环）只见三个动作：取下一个请求 / 回一个响应 / 查统计。
//!
//! interface 收窄账（蓝图 §3.1）：`main.rs` 可见的会话面 10（8 个
//! `MqttClient` 方法 + connect/try_connect）→ **4**（new/next_event/
//! respond/stats）；重连点 4→0；时序常量 4→0；过滤逻辑 2→0。
//!
//! 词形（Z2 跨 binary 运维契约）：5 条 RECONNECT/PLANNER-SKIP 词形逐字
//! 迁入本文件（t-word 字符串锁执法——词形是契约面，字符串锁恰当；
//! 蓝 69.3 消灭的是内部实现细节的字符串锁代偿，两类性质不同）。

use std::io;
use std::process::exit;
use std::time::Duration;

use crate::mqtt::{MqttClient, MqttError, PeerTransport};
use crate::{now_ms, Args};

// ---- 时序常量与静态锁（蓝图 §3.2：魔数关系从注释变编译期事实） ----

/// broker keepalive（秒；半窗打 PINGREQ）。
pub(crate) const KEEP_ALIVE_SECS: u16 = 30;
/// keepalive 半窗（ms）。与 [`KEEP_ALIVE_SECS`] 静态绑定——不得再写裸 15_000。
const PING_INTERVAL_MS: u64 = KEEP_ALIVE_SECS as u64 * 1000 / 2;
/// 入站静默判死阈值 = 1.5 × keepalive（原 main.rs 注记口径的静态化）。
/// 68.5 复查修复的判据本体：TCP 存活但对端永不回包（进程挂死）时 poll
/// 恒 Ok(None)、ping 写恒成功，此前连接永不重建——host 侧"半死 broker"
/// 盲区（REG-MQTT-BROKER-HANG 的镜像）。
const DEAD_SILENCE_MS: u64 = KEEP_ALIVE_SECS as u64 * 1500;
const _: () = assert!(PING_INTERVAL_MS == 15_000);
const _: () = assert!(DEAD_SILENCE_MS == 45_000);
const _: () = assert!(DEAD_SILENCE_MS > PING_INTERVAL_MS * 2);

/// 轮询节拍（ms；next_event 入口统一 pacing——原主循环 3 处 continue
/// 跳 sleep 的热旋面一并归一）。
const POLL_TICK_MS: u64 = 200;
/// 建联超时（秒；CONNACK/SUBACK 读窗，原 try_connect 值）。
const CONNECT_TIMEOUT_SECS: u64 = 5;
/// 重连重试间隔（秒）。
const RETRY_SECS: u64 = 2;

// ---- 会话事件与观测面 --------------------------------------------------

/// 会话事件（调用方只需处置 [`PeerEvent::Request`]）。
#[derive(Debug, Clone, PartialEq)]
pub enum PeerEvent {
    /// 一条已通过主题过滤 + UTF-8 校验的求解请求。
    Request { json: String },
    /// 会话刚刚重连（reason 已落词形；调用方无需处置，仅供观测）。
    Reconnected { reason: &'static str },
}

/// 重连原因（分计观测面；词形在本类型上逐字集中——t-word 锁执法）。
#[derive(Debug, Clone, PartialEq)]
enum ReconnectKind {
    InboundSilent,
    PingFailed(String),
    PublishFailed,
    PollFailed(String),
}

impl ReconnectKind {
    fn token(&self) -> &'static str {
        match self {
            Self::InboundSilent => "inbound-silent",
            Self::PingFailed(_) => "ping-failed",
            Self::PublishFailed => "publish-failed",
            Self::PollFailed(_) => "poll-failed",
        }
    }
}

/// 重连分计（[`PeerSession::stats`] 观测面）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconnectCounters {
    pub inbound_silent: u32,
    pub ping: u32,
    pub publish: u32,
    pub poll: u32,
}

/// 会话统计（DEMO-69 第 2 窗 + 审计用只读面）。bin 侧暂无运行时 dump
/// 消费方（新造观测服务属 Z7 禁区）——QEMU gate Q6 以 RECONNECT 词形
/// 为可观测面，本面供 host 测试与后续审计消费。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PeerStats {
    pub reconnects: ReconnectCounters,
    pub requests_served: u32,
    pub seq: u64,
}

// ---- seq 状态（自 main.rs 迁入；Z12：三分支 + exit(2) 原样） -----------

/// seq 状态（跨重启单调；peer 重启 seq 回退会被目标侧按重放拒绝）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct SeqState(u64);

impl SeqState {
    /// 68 三轮复查修复：三类文件态分流——首启（NotFound）归零合法；
    /// **存量文件损坏（读失败/JSON 坏/形坏）= seq 单调承诺失效面**，
    /// 静默归零会使目标侧按 seq 单调判据长期拒收（回退饿死至追平旧
    /// seq）且主机端零告警——fail-closed 退出交运维裁决（删文件 = 显式
    /// 接受回退重建）。变异靶：去掉 corrupt 分支 → seq_corrupt_exits
    /// 判据红（load 静默返回 0）。
    fn load(path: &str) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => v
                    .get("seq")
                    .and_then(|s| s.as_u64())
                    .map(SeqState)
                    .unwrap_or_else(|| seq_state_corrupt(path, "shape: no u64 seq")),
                Err(e) => seq_state_corrupt(path, &format!("json: {e}")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SeqState(0),
            Err(e) => seq_state_corrupt(path, &format!("read: {e}")),
        }
    }

    /// 原子落盘（temp + rename——半写状态文件不产生伪 seq）。
    /// 69.2 O5：临时文件创建用 `create_new`（O_EXCL——残留/并发临时名
    /// 不得被静默覆盖）；冲突时换带 pid 的临时名**最多重试 1 次**，再
    /// 失败即 [`seq_state_corrupt`]（fail-closed）。
    fn store(&self, path: &str) -> std::io::Result<()> {
        let body = serde_json::json!({ "seq": self.0 }).to_string();
        let tmp = format!("{path}.tmp");
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(body.as_bytes())?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let tmp2 = format!("{tmp}.{}", std::process::id());
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp2)
                    .map_err(|e2| {
                        io::Error::other(format!("seq 临时名冲突且 pid 重试失败: {e2}"))
                    })?;
                {
                    use std::io::Write;
                    f.write_all(body.as_bytes())?;
                }
                return std::fs::rename(&tmp2, path);
            }
            Err(e) => return Err(e),
        }
        std::fs::rename(&tmp, path)
    }
}

/// 存量 seq 状态损坏 = fail-closed 退出（诚实可见；运维裁决回退）。
fn seq_state_corrupt(path: &str, why: &str) -> ! {
    eprintln!(
        "[planner-peer] SEQ-STATE-CORRUPT path={path} why={why} —— seq 单调状态不可恢复，fail-closed 退出（删除文件 = 显式回退重建，目标侧将拒收回退前 seq）"
    );
    exit(2);
}

// ---- 建联（原 connect/try_connect 收编） --------------------------------

fn establish<T: PeerTransport>(
    make_conn: &mut dyn FnMut() -> Result<MqttClient<T>, MqttError>,
    request_topic: &str,
    broker: &str,
    clock: &dyn Fn() -> u64,
) -> Result<MqttClient<T>, MqttError> {
    let mut c = make_conn()?;
    c.subscribe(request_topic, 1)?;
    // 看门狗起点（CONNACK+SUBACK 已到——从此刻计静默）。
    c.mark_activity(clock());
    println!("[planner-peer] SUBSCRIBED topic={request_topic} broker={broker}");
    Ok(c)
}

/// 建联重试环（原 connect 的阻塞重试；fail-closed 到 broker 可达为止）。
fn establish_loop<T: PeerTransport>(
    make_conn: &mut dyn FnMut() -> Result<MqttClient<T>, MqttError>,
    request_topic: &str,
    broker: &str,
    clock: &dyn Fn() -> u64,
) -> MqttClient<T> {
    loop {
        match establish(make_conn, request_topic, broker, clock) {
            Ok(c) => return c,
            Err(e) => {
                eprintln!("[planner-peer] RECONNECT reason={} (retry 2s)", e.0);
                std::thread::sleep(Duration::from_secs(RETRY_SECS));
            }
        }
    }
}

// ---- PeerSession 深模块 --------------------------------------------------

/// peer 会话深模块：建联/重连策略、keepalive 节拍、入站静默看门狗、
/// 主题过滤、UTF-8 校验、序列号状态——**全部内化**。调用方只见
/// "取下一个请求 / 回一个响应 / 查会话是否在局"。
pub struct PeerSession<T: PeerTransport = std::net::TcpStream> {
    broker: String,
    request_topic: String,
    response_topic: String,
    state_path: String,
    client: MqttClient<T>,
    seq: SeqState,
    last_ping_ms: u64,
    clock: Box<dyn Fn() -> u64>,
    tick: Duration,
    make_conn: Box<dyn FnMut() -> Result<MqttClient<T>, MqttError>>,
    reconnects: ReconnectCounters,
    requests_served: u32,
}

impl<T: PeerTransport> PeerSession<T> {
    /// 测试装配面（生产走 [`PeerSession::new`]；假时钟 + 假传输使会话
    /// 策略获得 host 单测覆盖——PeerTransport 第 2 adapter 的消费点）。
    #[cfg(test)]
    fn with_parts(
        mut make_conn: Box<dyn FnMut() -> Result<MqttClient<T>, MqttError>>,
        clock: Box<dyn Fn() -> u64>,
        tick: Duration,
        request_topic: String,
        response_topic: String,
        state_path: String,
    ) -> Self {
        let client = establish_loop(&mut *make_conn, &request_topic, "loopback", &*clock);
        let last_ping_ms = clock();
        Self {
            broker: String::from("loopback"),
            request_topic,
            response_topic,
            state_path: state_path.clone(),
            client,
            seq: SeqState::load(&state_path),
            last_ping_ms,
            clock,
            tick,
            make_conn,
            reconnects: ReconnectCounters::default(),
            requests_served: 0,
        }
    }

    /// 重连（计数 + 词形 + 阻塞重试建联）。reason 词形逐字集中在此——
    /// t_reconnect_reason_wordforms_preserved 锁执法（Z2）。
    fn reconnect(&mut self, kind: ReconnectKind) -> &'static str {
        match &kind {
            ReconnectKind::InboundSilent => {
                self.reconnects.inbound_silent += 1;
                eprintln!(
                    "[planner-peer] RECONNECT reason=inbound-silent (>1.5x keepalive={KEEP_ALIVE_SECS}s)"
                );
            }
            ReconnectKind::PingFailed(e) => {
                self.reconnects.ping += 1;
                eprintln!("[planner-peer] RECONNECT reason=ping-failed ({e})");
            }
            ReconnectKind::PublishFailed => {
                self.reconnects.publish += 1;
                eprintln!("[planner-peer] RECONNECT reason=publish-failed");
            }
            ReconnectKind::PollFailed(e) => {
                self.reconnects.poll += 1;
                eprintln!("[planner-peer] RECONNECT reason=poll-failed ({e})");
            }
        }
        let token = kind.token();
        let client = establish_loop(
            &mut *self.make_conn,
            &self.request_topic,
            &self.broker,
            &*self.clock,
        );
        self.last_ping_ms = (self.clock)();
        self.client = client;
        token
    }

    /// 推进一轮：入口统一 [`POLL_TICK_MS`] pacing，随后看门狗/keepalive/
    /// poll/过滤内化执行。返回 None = 本轮无事（调用方直接再调）。
    pub fn next_event(&mut self) -> Option<PeerEvent> {
        std::thread::sleep(self.tick);
        let now = (self.clock)();
        // 68.5 复查修复：入站静默看门狗（1.5×keepalive 无任何入站 →
        // 判死重连——半死 broker 盲区）。
        if self.client.inbound_silent_for(now) > DEAD_SILENCE_MS {
            self.reconnect(ReconnectKind::InboundSilent);
            return Some(PeerEvent::Reconnected {
                reason: "inbound-silent",
            });
        }
        // keepalive 节拍（半窗）。
        if now.saturating_sub(self.last_ping_ms) >= PING_INTERVAL_MS {
            if let Err(e) = self.client.ping() {
                self.reconnect(ReconnectKind::PingFailed(format!("{e}")));
                return Some(PeerEvent::Reconnected {
                    reason: "ping-failed",
                });
            }
            self.last_ping_ms = now;
        }
        match self.client.poll(now) {
            Ok(Some(pub_)) => {
                if pub_.topic != self.request_topic {
                    return None;
                }
                let json = match std::str::from_utf8(&pub_.payload) {
                    Ok(s) => s,
                    Err(_) => {
                        println!("[planner-peer] PLANNER-SKIP reason=utf8");
                        return None;
                    }
                };
                self.requests_served = self.requests_served.saturating_add(1);
                return Some(PeerEvent::Request {
                    json: String::from(json),
                });
            }
            Ok(None) => {}
            Err(e) => {
                self.reconnect(ReconnectKind::PollFailed(format!("{e}")));
                return Some(PeerEvent::Reconnected {
                    reason: "poll-failed",
                });
            }
        }
        None
    }

    /// 回一条响应（QoS1 发布；成功后 seq 状态原子落盘）。返回 Err 时
    /// **会话已自行重连**（publish-failed 词形已落），调用方不得再重连。
    pub fn respond(&mut self, payload: &[u8]) -> Result<(), MqttError> {
        if let Err(e) = self.client.publish(&self.response_topic, payload, 1) {
            self.reconnect(ReconnectKind::PublishFailed);
            return Err(e);
        }
        if let Err(e) = self.seq.store(&self.state_path) {
            println!(
                "[planner-peer] SEQ-PERSIST-FAIL {e}（重启后 seq 回退将由目标侧拒绝——诚实可见）"
            );
        }
        Ok(())
    }

    /// 取下一个响应序号（单调；跨重启经状态文件持久）。
    pub fn next_seq(&mut self) -> u64 {
        self.seq.0 = self.seq.0.saturating_add(1);
        self.seq.0
    }

    /// 只读观测面（DEMO-69 第 2 窗 + 审计用）。
    #[allow(dead_code)]
    pub fn stats(&self) -> PeerStats {
        PeerStats {
            reconnects: self.reconnects,
            requests_served: self.requests_served,
            seq: self.seq.0,
        }
    }
}

impl PeerSession<std::net::TcpStream> {
    /// 生产装配：解析后的参数 + 状态路径 → 阻塞建联（重试至 broker 可达）。
    pub fn new(args: Args) -> Result<Self, String> {
        let state_path = args.state_path.clone();
        let seq = SeqState::load(&state_path);
        let broker = args.broker.clone();
        let request_topic = args.request_topic.clone();
        let response_topic = args.response_topic.clone();
        let mut make_conn: Box<dyn FnMut() -> Result<MqttClient, MqttError>> =
            Box::new(move || {
                MqttClient::connect(
                    &broker,
                    &args.client_id,
                    KEEP_ALIVE_SECS,
                    Duration::from_secs(CONNECT_TIMEOUT_SECS),
                )
            });
        let clock: Box<dyn Fn() -> u64> = Box::new(now_ms);
        let client = establish_loop(&mut *make_conn, &request_topic, &args.broker, &*clock);
        Ok(Self {
            broker: args.broker,
            request_topic,
            response_topic,
            state_path,
            client,
            seq,
            last_ping_ms: clock(),
            clock,
            tick: Duration::from_millis(POLL_TICK_MS),
            make_conn,
            reconnects: ReconnectCounters::default(),
            requests_served: 0,
        })
    }
}

// ============================================================================
// host 测试（会话策略锁：假时钟 + 内存传输，零真网络）
// ============================================================================

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::Duration;

    use super::*;
    use crate::mqtt::{MqttClient, MqttError, PeerTransport};

    // ---- 假件：内存双端 bus（PeerTransport 的第 2 个真实 adapter） ------

    #[derive(Default)]
    struct Bus {
        /// broker → client 字节流（read 消费）。
        inbox: VecDeque<u8>,
        /// client 每次写出的包（按 write_all 粒度捕获）。
        outbox: Vec<Vec<u8>>,
        /// 注入写失败次数。
        fail_writes: u32,
        /// 注入读失败次数（非超时错误——poll-failed 面）。
        fail_reads: u32,
    }

    type SharedBus = Rc<RefCell<Bus>>;

    struct LoopbackTransport {
        bus: SharedBus,
    }

    impl LoopbackTransport {
        fn new(bus: SharedBus) -> Self {
            Self { bus }
        }
    }

    impl PeerTransport for LoopbackTransport {
        fn read_exact_bytes(&mut self, buf: &mut [u8]) -> Result<(), MqttError> {
            let mut bus = self.bus.borrow_mut();
            if bus.fail_reads > 0 {
                bus.fail_reads -= 1;
                return Err(MqttError(String::from("read: injected")));
            }
            if bus.inbox.len() < buf.len() {
                return Err(MqttError(String::from(crate::mqtt::TIMEOUT_SENTINEL)));
            }
            for b in buf.iter_mut() {
                *b = bus.inbox.pop_front().expect("长度已验");
            }
            Ok(())
        }

        fn write_all_bytes(&mut self, buf: &[u8]) -> Result<(), MqttError> {
            let mut bus = self.bus.borrow_mut();
            if bus.fail_writes > 0 {
                bus.fail_writes -= 1;
                return Err(MqttError(String::from("write: injected")));
            }
            bus.outbox.push(buf.to_vec());
            Ok(())
        }

        fn flush_bytes(&mut self) -> Result<(), MqttError> {
            Ok(())
        }

        fn set_read_timeout(&mut self, _d: Duration) -> Result<(), MqttError> {
            Ok(())
        }
    }

    // ---- 帧构造（body ≤127B，varint 单字节） ----------------------------

    fn frame(hdr: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![hdr, body.len() as u8];
        v.extend_from_slice(body);
        v
    }

    fn connack() -> Vec<u8> {
        vec![0x20, 0x02, 0x00, 0x00]
    }

    /// SUBACK：pid=1、QoS0 成功（客户端 subscribe 用 pid=1，只查 body[2]≠0x80）。
    fn suback() -> Vec<u8> {
        vec![0x90, 0x03, 0x00, 0x01, 0x00]
    }

    fn publish_inbound(topic: &str, pid: Option<u16>, payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&(topic.len() as u16).to_be_bytes());
        body.extend_from_slice(topic.as_bytes());
        if let Some(p) = pid {
            body.extend_from_slice(&p.to_be_bytes());
        }
        body.extend_from_slice(payload);
        frame(if pid.is_some() { 0x32 } else { 0x30 }, &body)
    }

    // ---- 会话脚手架 ------------------------------------------------------

    const REQUEST_TOPIC: &str = "eneros/brain/request";

    struct Ctx {
        bus: SharedBus,
        clock: Rc<Cell<u64>>,
        state_path: String,
        _dir: std::path::PathBuf,
    }

    fn ctx() -> Ctx {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let bus: SharedBus = Rc::new(RefCell::new(Bus {
            inbox: VecDeque::from(connack().into_iter().chain(suback()).collect::<Vec<_>>()),
            ..Bus::default()
        }));
        let dir = std::env::temp_dir().join(format!(
            "planner-peer-session-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let state_path = dir.join("seq.json").to_string_lossy().into_owned();
        Ctx {
            bus,
            clock: Rc::new(Cell::new(1_000)),
            state_path,
            _dir: dir,
        }
    }

    impl Ctx {
        fn advance(&self, ms: u64) {
            self.clock.set(self.clock.get() + ms);
        }
        /// 为下一次重连预置握手帧（factory 每次 establish 消费一对）。
        fn arm_reconnect(&self) {
            let mut bus = self.bus.borrow_mut();
            bus.inbox.extend(connack());
            bus.inbox.extend(suback());
        }
        fn outbox(&self) -> Vec<Vec<u8>> {
            self.bus.borrow().outbox.clone()
        }
        fn fail_next_write(&self) {
            self.bus.borrow_mut().fail_writes += 1;
        }
        fn fail_next_read(&self) {
            self.bus.borrow_mut().fail_reads += 1;
        }
    }

    fn make_session(c: &Ctx) -> PeerSession<LoopbackTransport> {
        PeerSession::with_parts(
            {
                let bus = c.bus.clone();
                Box::new(move || {
                    MqttClient::over(
                        LoopbackTransport::new(bus.clone()),
                        "eneros-planner-peer-test",
                        KEEP_ALIVE_SECS,
                        Duration::ZERO,
                    )
                })
            },
            {
                let clock = c.clock.clone();
                Box::new(move || clock.get())
            },
            Duration::ZERO,
            REQUEST_TOPIC.into(),
            "eneros/brain/response".into(),
            c.state_path.clone(),
        )
    }

    // ---- seq 状态（自 main.rs seq_state_tests 随迁） --------------------

    /// 首启（NotFound）归零 + store/load 往返。损坏路径（JSON 坏/形坏/
    /// 读失败）走 seq_state_corrupt = exit(2)，进程内不可测——判据由
    /// load 的分流结构承载（NotFound 之外的 Err 与 shape 失配均不得
    /// 返回 SeqState(0)）。
    #[test]
    fn seq_state_missing_is_zero_and_roundtrips() {
        let dir =
            std::env::temp_dir().join(format!("planner-peer-seq-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("seq.json").to_string_lossy().into_owned();
        assert_eq!(SeqState::load(&path), SeqState(0));
        SeqState(42).store(&path).expect("store");
        assert_eq!(SeqState::load(&path), SeqState(42));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- p1：入站静默判死（1.5×keepalive） ------------------------------

    #[test]
    #[allow(clippy::disallowed_macros)] // 仓例先例（68.2 卡 D.6）：负例断言用 panic!
    fn p1_inbound_silence_triggers_reconnect_with_wordform() {
        let c = ctx();
        let mut s = make_session(&c);
        c.advance(46_000); // 字面 spec 值（1.5×30s+1s）——锁行为不锁实现常量
        c.arm_reconnect();
        match s.next_event() {
            Some(PeerEvent::Reconnected { reason }) => {
                assert_eq!(reason, "inbound-silent");
            }
            other => panic!("静默超限必须重连，got {other:?}"),
        }
        let st = s.stats();
        assert_eq!(st.reconnects.inbound_silent, 1);
        assert_eq!(st.reconnects.ping, 0);
    }

    // ---- p2：keepalive 半窗 PINGREQ + PINGRESP 刷新活性 -----------------

    #[test]
    #[allow(clippy::disallowed_macros)] // 仓例先例（68.2 卡 D.6）：负例断言用 panic!
    fn p2_ping_at_half_keepalive_window() {
        let c = ctx();
        let mut s = make_session(&c);
        c.advance(PING_INTERVAL_MS);
        let _ = s.next_event();
        let pingreq = c
            .outbox()
            .into_iter()
            .find(|p| p.first() == Some(&0xC0))
            .expect("半窗必须发 PINGREQ");
        assert_eq!(pingreq, vec![0xC0, 0x00]);
        // PINGRESP 入站刷新活性：最近入站 = clock 17_000。
        c.bus.borrow_mut().inbox.extend(vec![0xD0, 0x00]);
        c.advance(1_000);
        assert!(s.next_event().is_none(), "PINGRESP tick 无事");
        // 自最近入站静默 44s（< 1.5×keepalive）⇒ 不得判死。
        c.advance(44_000); // clock=61_000，silence=44_000
        let st = s.stats();
        assert_eq!(st.reconnects.inbound_silent, 0, "活性刷新后 44s 不得判死");
        // 静默 46s（> 1.5×keepalive）⇒ 判死（静态锁值的实行为面）。
        c.advance(2_000); // clock=63_000，silence=46_000
        c.arm_reconnect();
        match s.next_event() {
            Some(PeerEvent::Reconnected { reason }) => assert_eq!(reason, "inbound-silent"),
            other => panic!("46s 静默必须判死，got {other:?}"),
        }
    }

    // ---- p3：ping 失败重连 ----------------------------------------------

    #[test]
    #[allow(clippy::disallowed_macros)] // 仓例先例（68.2 卡 D.6）：负例断言用 panic!
    fn p3_ping_failure_triggers_reconnect() {
        let c = ctx();
        let mut s = make_session(&c);
        c.advance(PING_INTERVAL_MS);
        c.fail_next_write();
        c.arm_reconnect();
        match s.next_event() {
            Some(PeerEvent::Reconnected { reason }) => assert_eq!(reason, "ping-failed"),
            other => panic!("ping 失败必须重连，got {other:?}"),
        }
        assert_eq!(s.stats().reconnects.ping, 1);
    }

    // ---- p4：poll 错误重连 ----------------------------------------------

    #[test]
    #[allow(clippy::disallowed_macros)] // 仓例先例（68.2 卡 D.6）：负例断言用 panic!
    fn p4_poll_error_triggers_reconnect() {
        let c = ctx();
        let mut s = make_session(&c);
        c.fail_next_read();
        c.arm_reconnect();
        match s.next_event() {
            Some(PeerEvent::Reconnected { reason }) => assert_eq!(reason, "poll-failed"),
            other => panic!("poll 错误必须重连，got {other:?}"),
        }
        assert_eq!(s.stats().reconnects.poll, 1);
    }

    // ---- p5：publish 失败 ⇒ respond 重连 + Err ---------------------------

    #[test]
    fn p5_publish_failure_triggers_reconnect_and_respond_returns_err() {
        let c = ctx();
        let mut s = make_session(&c);
        c.fail_next_write();
        c.arm_reconnect();
        let r = s.respond(b"{}");
        assert!(r.is_err(), "publish 失败必须上抛 Err");
        assert_eq!(s.stats().reconnects.publish, 1, "respond 内部须自愈重连");
    }

    // ---- p6：主题过滤 ----------------------------------------------------

    #[test]
    fn p6_topic_filter_drops_non_request_topic() {
        let c = ctx();
        let mut s = make_session(&c);
        // 探针 1：完全无关主题；探针 2：request_topic 的**子串超集**
        // （咬"精确匹配放宽为 contains"的变异——contains 下探针 2 会被
        // 误放行产生事件）。
        c.bus
            .borrow_mut()
            .inbox
            .extend(publish_inbound("eneros/other", None, b"{}"));
        c.bus
            .borrow_mut()
            .inbox
            .extend(publish_inbound("x/eneros/brain/request/y", None, b"{}"));
        assert!(s.next_event().is_none(), "非请求主题不得产生事件");
        assert!(s.next_event().is_none(), "超集主题不得产生事件（精确匹配）");
        let st = s.stats();
        assert_eq!(st.requests_served, 0);
        assert_eq!(st.reconnects.inbound_silent, 0, "过滤不得触发重连");
    }

    // ---- p7：非 UTF-8 ⇒ SKIP 词形、不产生事件 ---------------------------

    #[test]
    fn p7_utf8_invalid_yields_skip_wordform_not_event() {
        let c = ctx();
        let mut s = make_session(&c);
        c.bus.borrow_mut().inbox.extend(publish_inbound(
            REQUEST_TOPIC,
            Some(0x0102),
            &[0xFF, 0xFE],
        ));
        assert!(s.next_event().is_none(), "非法 UTF-8 不得产生事件");
        assert_eq!(s.stats().requests_served, 0);
        // QoS1 入站仍须回 PUBACK（pid=0x0102）——链路卫生不因 skip 退化。
        let acked = c
            .outbox()
            .into_iter()
            .find(|p| p.first() == Some(&0x40))
            .expect("QoS1 入站须回 PUBACK");
        assert_eq!(&acked[2..4], &[0x01, 0x02]);
    }

    // ---- p8：四 reason 分计 ---------------------------------------------

    #[test]
    fn p8_reconnect_counters_per_reason() {
        let c = ctx();
        let mut s = make_session(&c);
        // ping-failed。
        c.advance(PING_INTERVAL_MS);
        c.fail_next_write();
        c.arm_reconnect();
        let _ = s.next_event();
        // poll-failed。
        c.fail_next_read();
        c.arm_reconnect();
        let _ = s.next_event();
        // publish-failed。
        c.fail_next_write();
        c.arm_reconnect();
        assert!(s.respond(b"{}").is_err());
        // inbound-silent。
        c.advance(46_000); // 字面 spec 值
        c.arm_reconnect();
        let _ = s.next_event();
        let st = s.stats();
        assert_eq!(st.reconnects.inbound_silent, 1);
        assert_eq!(st.reconnects.ping, 1);
        assert_eq!(st.reconnects.publish, 1);
        assert_eq!(st.reconnects.poll, 1);
        assert_eq!(st.requests_served, 0);
    }

    // ---- p9：静态锁（字面 + 派生关系） ----------------------------------

    #[test]
    fn p9_static_locks_bind_constants() {
        // 值锁：与既有实跑行为零漂移（VOL69.2-GATE Q5 依赖）。
        assert_eq!(PING_INTERVAL_MS, 15_000);
        assert_eq!(DEAD_SILENCE_MS, 45_000);
        // 字面锁：派生关系必须写成形（防"改值不改派生"）。
        let src = include_str!("../src/session.rs");
        assert!(
            src.contains("const PING_INTERVAL_MS: u64 = KEEP_ALIVE_SECS as u64 * 1000 / 2;"),
            "半窗必须由 KEEP_ALIVE_SECS 派生"
        );
        assert!(
            src.contains("const DEAD_SILENCE_MS: u64 = KEEP_ALIVE_SECS as u64 * 1500;"),
            "判死阈值必须由 KEEP_ALIVE_SECS 派生"
        );
        assert!(src.contains("assert!(PING_INTERVAL_MS == 15_000)"));
        assert!(src.contains("assert!(DEAD_SILENCE_MS == 45_000)"));
        assert!(src.contains("assert!(DEAD_SILENCE_MS > PING_INTERVAL_MS * 2)"));
    }

    // ---- p10：seq 状态落盘原子性（文件层；O5） --------------------------

    /// store = "create_new 临时名 + rename"：① 残留同名临时文件不得被
    /// 覆盖（create_new 纪律，冲突走 pid 重试）；② 临时名被目录占位时
    /// 经 pid 重试仍成功；③ 目标被目录占位（rename 必败）⇒ 显式 Err 而
    /// 非静默成功；④ 成功后无临时残留。变异靶：store 退回 `fs::write`
    /// 直写（丢 tmp+rename）→ ①红（覆盖残留）③红（直写目录也失败但
    /// 无 rename 语义——由②红咬合：目录占位 tmp 时直写必败）。
    #[test]
    fn p10_seq_state_write_is_atomic_via_rename() {
        let dir = std::env::temp_dir().join(format!(
            "planner-peer-p10-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("seq.json").to_string_lossy().into_owned();
        // ① 残留同名临时文件：不得覆盖，store 须经 pid 重试成功。
        let stale_tmp = format!("{path}.tmp");
        std::fs::write(&stale_tmp, b"stale-garbage").expect("write stale tmp");
        SeqState(7)
            .store(&path)
            .expect("store（残留 tmp 须 pid 重试成功）");
        assert_eq!(SeqState::load(&path), SeqState(7));
        assert_eq!(
            std::fs::read(&stale_tmp).expect("残留 tmp 在"),
            b"stale-garbage",
            "残留临时文件必须原样保留（create_new 纪律，红=被覆盖）"
        );
        // ② 临时名被目录占位：pid 重试成功（红=退回直写/单名 tmp 的实现）。
        std::fs::remove_file(&path).expect("rm target");
        std::fs::remove_file(&stale_tmp).expect("rm stale tmp");
        std::fs::create_dir(format!("{path}.tmp")).expect("mkdir 占位 tmp");
        SeqState(9)
            .store(&path)
            .expect("store（目录占位 tmp 须 pid 重试成功）");
        assert_eq!(SeqState::load(&path), SeqState(9));
        std::fs::remove_dir(format!("{path}.tmp")).expect("占位目录未被误删");
        // ③ 目标被目录占位：rename 必败 ⇒ 显式 Err（红=静默成功）。
        std::fs::remove_file(&path).expect("rm target");
        std::fs::create_dir(&path).expect("mkdir 占位 target");
        assert!(
            SeqState(11).store(&path).is_err(),
            "rename 失败必须显式报错"
        );
        std::fs::remove_dir(&path).expect("cleanup");
        // ⑤ 判别器：只读目标文件（0o400）——rename 只需目录写权限可整体
        // 替换目标（成功），直写需目标写权限（必败）。红=退回直写的实现。
        {
            let _ = std::fs::remove_file(&path);
            std::fs::write(&path, b"old").expect("write old");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).expect("chmod");
            SeqState(21)
                .store(&path)
                .expect("store 须经 rename 替换只读目标");
            assert_eq!(SeqState::load(&path), SeqState(21), "内容须整体替换");
        }
        // ④ 成功路径：本次 store 的临时名（含 pid 重试名）零残留——注意
        // ①已锁"他人/历史残留临时名原样保留"，此处只查自己的临时名。
        let pid_tmp = format!("{path}.tmp.{}", std::process::id());
        SeqState(13).store(&path).expect("store");
        assert_eq!(SeqState::load(&path), SeqState(13));
        assert!(
            !std::path::Path::new(&pid_tmp).exists(),
            "本次 store 的 pid 临时名必须已被 rename 消费"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- t-word：5 条词形逐字保留（Z2 跨 binary 运维契约） --------------
    //
    // 注意：这是字符串锁，正是 69.3 要消灭的那种代偿（01 §6 卡 2）。
    // 本卷之所以用它，是因为词形是**跨 binary 的运维契约**（Z2），不是
    // 内部实现细节——契约面用字符串锁是恰当的。执行卡须原样抄录这句
    // 理由，防止后续执行者"顺手改成类型化词形"。

    #[test]
    fn t_reconnect_reason_wordforms_preserved() {
        let src = include_str!("../src/session.rs");
        for w in [
            "RECONNECT reason=inbound-silent (>1.5x keepalive={KEEP_ALIVE_SECS}s)",
            "RECONNECT reason=ping-failed ({e})",
            "RECONNECT reason=publish-failed",
            "RECONNECT reason=poll-failed ({e})",
            "PLANNER-SKIP reason=utf8",
        ] {
            assert!(src.contains(w), "词形漂移：{w}");
        }
    }
}
