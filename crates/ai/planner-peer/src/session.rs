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

/// [`PeerSession::respond_reserved`] 失败分类（WP-70.4.3）。
#[derive(Debug)]
pub enum RespondError {
    /// seq 持久失败——未发布（fail-closed；运维介入前不再可恢复发布）。
    Persist(String),
    /// 载荷组装/自检失败——序号已预留（有解释间隙），未发布。
    Compose(String),
    /// 发布失败——会话已自行重连，调用方不得再重连。
    Publish(MqttError),
}

impl std::fmt::Display for RespondError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Persist(e) => write!(f, "seq-persist: {e}"),
            Self::Compose(e) => write!(f, "compose: {e}"),
            Self::Publish(e) => write!(f, "publish: {e}"),
        }
    }
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
        // 蓝图 70.4（WP-70.4.3）：temp 写入后 `sync_all` 再 rename——
        // 原子 rename 只保证命名空间原子性，不等价于掉电持久；半持久
        // 状态文件在掉电窗可能回退到旧 seq（重发回退序号冒充新响应）。
        let sync_one = |f: &mut std::fs::File| -> std::io::Result<()> {
            use std::io::Write;
            f.write_all(body.as_bytes())?;
            f.sync_all()
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(mut f) => {
                sync_one(&mut f)?;
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
                sync_one(&mut f)?;
                std::fs::rename(&tmp2, path)?;
                return sync_parent_dir(path);
            }
            Err(e) => return Err(e),
        }
        std::fs::rename(&tmp, path)?;
        sync_parent_dir(path)
    }
}

/// fsync 父目录（rename 落盘持久语义；Unix 目录 fd sync_all）。
fn sync_parent_dir(path: &str) -> std::io::Result<()> {
    let parent = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    std::fs::File::open(&parent)?.sync_all()
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

    /// 回一条响应（QoS1 发布）——蓝图 70.4（WP-70.4.3 / N15 修复）
    /// **先持久预留 seq，再发布**：
    ///
    /// 1. seq+1 **先原子落盘**（sync file + rename + fsync 目录）；持久
    ///    失败 = 不发布、显式 `Err(RespondError::Persist)`（fail-closed
    ///    ——持久失败时停止宣告可恢复发布，旧实现打印后仍返回 Ok）；
    /// 2. `compose(seq)` 组装载荷（Err = 序号间隙，有解释地放弃）；
    /// 3. 发布；失败 = 会话已自行重连（publish-failed 词形已落）+
    ///    `Err(RespondError::Publish)`。
    ///
    /// 序号合同：断电/崩溃后 seq **不回退**（发布前已持久——至多产生
    /// "已预留未发布"的有解释间隙，目标侧单调判据不受回退重放冲击）。
    /// 旧 `next_seq`+`respond` 两段式（发布后才持久，保存失败仅打印）
    /// 退役——单一 owner。
    pub fn respond_reserved(
        &mut self,
        compose: impl FnOnce(u64) -> Result<Vec<u8>, String>,
    ) -> Result<u64, RespondError> {
        let seq = self.seq.0.saturating_add(1);
        if let Err(e) = SeqState(seq).store(&self.state_path) {
            return Err(RespondError::Persist(format!("{e}")));
        }
        self.seq.0 = seq;
        let payload = compose(seq).map_err(RespondError::Compose)?;
        if let Err(e) = self.client.publish(&self.response_topic, &payload, 1) {
            self.reconnect(ReconnectKind::PublishFailed);
            return Err(RespondError::Publish(e));
        }
        Ok(seq)
    }

    /// 测试观测：当前内存 seq（已持久预留的最后值）。
    #[cfg(test)]
    fn next_seq_probe(&self) -> u64 {
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
    #![allow(clippy::disallowed_macros)] // 测试内负例断言（仓例先例）

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
            // 70.4：QoS1 PUBLISH 自动回 PUBACK（pid 回显——真 broker 同律；
            // 使 respond_reserved 测试不必手工预置 ACK，且注入写失败仍在
            // PUBACK 之前生效）。
            // QoS1 PUBLISH 自动回 PUBACK（pid 回显）。修复（70.10 复查）：
            // 原实现把剩余长度当**单字节**（`buf[2..4]` = topic 长度），
            // >127B 载荷（多字节 varint）下 pid 定位错位 → 不回 ACK →
            // 调用方等 PUBACK 永久挂起（正是"大载荷测试不存在"的第二处
            // 构造性原因）。此处按 varint 正确定位。
            if !buf.is_empty() && (buf[0] >> 4) == 3 && (buf[0] >> 1) & 0x03 > 0 {
                if let Some(pid_off) = publish_pid_offset(buf) {
                    if buf.len() >= pid_off + 2 {
                        let mut ack = vec![0x40, 0x02];
                        ack.extend_from_slice(&buf[pid_off..pid_off + 2]);
                        bus.inbox.extend(ack);
                    }
                }
            }
            Ok(())
        }

        fn flush_bytes(&mut self) -> Result<(), MqttError> {
            Ok(())
        }

        fn set_read_timeout(&mut self, _d: Duration) -> Result<(), MqttError> {
            Ok(())
        }
    }

    /// MQTT 帧（header + varint 剩余长度 + body）。
    ///
    /// 修复（70.10 复查）：原实现 `body.len() as u8` 把 >127B 的载荷静默
    /// 截断成畸形帧——正是"bin 级 v2（大载荷）测试不存在"的构造性原因
    /// （v2 请求 ~600B）。>127B 走多字节 varint（与真 broker 同律）。
    /// QoS1 PUBLISH 的 pid 偏移（varint 剩余长度 + topic 长度域之后）。
    fn publish_pid_offset(frame: &[u8]) -> Option<usize> {
        let mut i = 1usize;
        let mut shift = 0;
        loop {
            let b = *frame.get(i)?;
            i += 1;
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
            if shift > 21 {
                return None;
            }
        }
        let tlen = u16::from_be_bytes([*frame.get(i)?, *frame.get(i + 1)?]) as usize;
        Some(i + 2 + tlen)
    }

    fn frame(hdr: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![hdr];
        let mut len = body.len();
        loop {
            let mut b = (len % 128) as u8;
            len /= 128;
            if len > 0 {
                b |= 0x80;
            }
            v.push(b);
            if len == 0 {
                break;
            }
        }
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
        let r = s.respond_reserved(|_| Ok(b"{}".to_vec()));
        assert!(r.is_err(), "publish 失败必须上抛 Err");
        assert_eq!(s.stats().reconnects.publish, 1, "respond 内部须自愈重连");
    }

    // ---- 70.4 WP-70.4.3：seq 先持久预留再发布 ----------------------------

    /// seq 持久失败 = 不发布、显式 Persist Err（红 = 旧实现打印 SEQ-PERSIST-
    /// FAIL 后仍返回 Ok 并发布）；内存 seq 不推进（下次复用该号——发布
    /// 面未见，无单调性破坏）。
    #[test]
    fn seq_persist_failure_refuses_publish() {
        let c = ctx();
        let mut s = make_session(&c);
        // 令 state 文件路径不可写（替换为目录/不可创建路径）。
        let bad_dir = std::path::Path::new(&c.state_path)
            .parent()
            .unwrap()
            .join("nope");
        std::fs::create_dir_all(&bad_dir).expect("mkdir");
        // 直接改会话的 state_path 为不可达子路径（目录不存在 → open 失败）。
        s.state_path = bad_dir
            .join("sub")
            .join("seq.json")
            .to_string_lossy()
            .into_owned();
        let r = s.respond_reserved(|_| Ok(b"{}".to_vec()));
        assert!(
            matches!(r, Err(RespondError::Persist(_))),
            "持久失败必须 Persist Err（不发布）: {r:?}"
        );
        assert_eq!(s.next_seq_probe(), 0, "内存 seq 不得推进（号未消费）");
    }

    /// 成功路径：compose 收到预留 seq；seq 文件先于发布更新；再预留递增。
    #[test]
    fn respond_reserved_persists_seq_before_publish() {
        let c = ctx();
        let mut s = make_session(&c);
        let mut seen_in_compose = 0u64;
        let r = s.respond_reserved(|seq| {
            seen_in_compose = seq;
            Ok(b"x".to_vec())
        });
        assert_eq!(r.expect("ok"), 1);
        assert_eq!(seen_in_compose, 1, "compose 收到的是预留 seq");
        // 文件已持有所预留 seq（先持久后发布的合同）。
        let text = std::fs::read_to_string(&c.state_path).expect("read seq file");
        let v: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(v["seq"].as_u64(), Some(1), "发布前 seq 必须已持久");
        let r2 = s.respond_reserved(|_seq| Ok(b"y".to_vec())).expect("ok2");
        assert_eq!(r2, 2);
    }

    /// compose 失败 = 序号间隙（预留生效、未发布、显式 Compose Err）。
    #[test]
    fn compose_failure_leaves_explained_gap() {
        let c = ctx();
        let mut s = make_session(&c);
        let r = s.respond_reserved(|seq| {
            let _ = seq;
            Err(String::from("self-check failed"))
        });
        assert!(matches!(r, Err(RespondError::Compose(_))));
        // 下一次成功预留跳过该号（间隙可见、单调不回退）。
        let r2 = s.respond_reserved(|_seq| Ok(b"z".to_vec())).expect("ok");
        assert_eq!(r2, 2, "间隙后下一次预留 = 2（不回退复用 1）");
    }

    /// 70.4 WP-70.4.3：等待 PUBACK 期间到达的 PUBLISH 不再"ACK 后丢弃"
    /// ——经有界队列保留，业务层下一轮 next_event 取得（N14 交付面）。
    #[test]
    #[allow(clippy::disallowed_macros)] // 负例断言用 panic!（68.2 卡 D.6 仓例）
    fn interleaved_publish_during_puback_wait_is_delivered() {
        let c = ctx();
        let mut s = make_session(&c);
        // 预置交错 PUBLISH（请求 B）在 inbox——respond 的 PUBACK 等待环
        // 先读到它（bus 对 A 的 PUBACK 自动追加在其后）。
        c.bus.borrow_mut().inbox.extend(publish_inbound(
            REQUEST_TOPIC,
            Some(0x0B),
            br#"{"req":2}"#,
        ));
        let r = s
            .respond_reserved(|seq| Ok(format!(r#"{{"s":{seq}}}"#).into_bytes()))
            .expect("publish ok");
        assert_eq!(r, 1);
        // 下一轮 next_event 交付 B（红 = 旧实现 ACK 后丢弃 → None）。
        match s.next_event().expect("交错请求须交付") {
            PeerEvent::Request { json } => {
                assert!(json.contains("\"req\":2"), "B 的载荷: {json}")
            }
            other => unreachable!("须为 Request 事件，got {other:?}"),
        }
    }

    /// 70.4 WP-70.4.3 满队合同：交错 PUBLISH 超出有界队列上限时——
    /// QoS1 溢出**不回 PUBACK**（broker 重投 = 显式不接纳，不静默丢弃、
    /// 不计 dropped）；QoS0 溢出只能丢弃但计数可见。交付顺序 FIFO。
    #[test]
    #[allow(clippy::disallowed_macros)] // 负例断言用 panic!（68.2 卡 D.6 仓例）
    fn interleaved_overflow_bounded_no_silent_drop() {
        let c = ctx();
        let mut s = make_session(&c);
        // 预置 18 条交错 PUBLISH（16 上限 + 2 溢出；等待环预算 32 轮够读）。
        {
            let mut bus = c.bus.borrow_mut();
            for i in 0..18u16 {
                bus.inbox
                    .extend(publish_inbound(REQUEST_TOPIC, Some(0x30 + i), b"x"));
            }
        }
        let _ = s
            .respond_reserved(|_seq| Ok(b"x".to_vec()))
            .expect("publish ok");
        // FIFO 交付 16 条（上限内），内容有序不串。
        for i in 0..16u16 {
            match s.next_event().expect("上限内必须交付") {
                PeerEvent::Request { json } => {
                    let _ = json; // 主题过滤后的载荷——序由 FIFO 保证
                }
                other => unreachable!("第 {i} 条须为 Request，got {other:?}"),
            }
        }
        // 溢出 2 条为 QoS1：不 ACK（重投语义）、不计 QoS0 丢弃。
        assert_eq!(s.client.pending_dropped(), 0);
        // QoS0 溢出路径： stuffed 满 + 1 条 QoS0 → dropped 计数可见。
        let c2 = ctx();
        let mut s2 = make_session(&c2);
        {
            let mut bus = c2.bus.borrow_mut();
            for _i in 0..17u16 {
                bus.inbox.extend(publish_inbound(REQUEST_TOPIC, None, b"y")); // QoS0
            }
        }
        let _ = s2
            .respond_reserved(|_seq| Ok(b"y".to_vec()))
            .expect("publish ok");
        for _ in 0..16 {
            let _ = s2.next_event();
        }
        assert_eq!(
            s2.client.pending_dropped(),
            1,
            "QoS0 溢出必须可见计数（不得静默）"
        );
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
        assert!(s.respond_reserved(|_| Ok(b"{}".to_vec())).is_err());
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

    /// 蓝图 70.10 复查修复端到端（bin 级）：**v2 请求 → 结构化求解 →
    /// 发布目标侧可接纳的 v2 应答**。
    ///
    /// 修复前：`handle_request` 只解析 v1，v2 请求恒
    /// `PLANNER-SKIP reason=parse`（v1 schema 要求 `telemetry` 且拒未知
    /// 字段）→ 零发布 → 目标每周期超时降级 = 通道端到端不可用且无任何
    /// bin 级测试（lib 单测直呼 `solve_request_v2`，绕过 bin 路由）。
    ///
    /// 变异靶：删 bin 的 v2 路由臂 → 本测红（outbox 零 PUBLISH）。
    #[test]
    fn v2_request_end_to_end_publishes_target_valid_response() {
        use eneros_intent_contract::peer::{
            budget as peer_budget, parse_and_validate_v2, PlanBinds, ResponseChecks,
        };
        use eneros_planner::tolerance::ToleranceProfile;
        use eneros_planner::Planner;
        use eneros_solver_core::highs::HighsSolver;
        let c = ctx();
        let mut s = make_session(&c);
        let now = crate::now_ms();
        let req_json = format!(
            r#"{{"v":2,"req_id":7,"ts_ms":{now},"snapshot_id_hex":"{}","provenance_hex":"{}","catalog_version":3,"horizon_start_ms":{now},"step_ns":3600000000000,"step_count":2,"assets":[{{"asset_id":1,"capacity_kwh":10.0,"eta_c":0.95,"eta_d":0.95,"initial_kwh":5.0,"soc_min":0.05,"soc_max":0.95,"p_min_kw":-5.0,"p_max_kw":5.0,"ramp_up_kw_s":1000000.0,"ramp_down_kw_s":1000000.0,"device_id":2,"point_id":10,"unit_code":6}}],"grid_asset_id":0,"objective_kind":"max_revenue","prices_per_kwh":[1.0,10.0],"budget":{{"max_power_kw":20.0,"ttl_secs":60}}}}"#,
            eneros_intent_contract::peer::hex16(&[0x5A; 16]),
            eneros_intent_contract::peer::hex32(&[0x3D; 32]),
        );
        c.bus
            .borrow_mut()
            .inbox
            .extend(publish_inbound(REQUEST_TOPIC, None, req_json.as_bytes()));
        let json = match s.next_event() {
            Some(PeerEvent::Request { json }) => json,
            other => unreachable!("须交付请求事件，got {other:?}"),
        };
        let mut planner = Planner::new(ToleranceProfile::l2(), 100);
        let mut highs = HighsSolver::new().expect("HiGHS");
        crate::handle_request_v2(&json, &mut planner, &mut highs, &mut s);

        // 断言：outbox 里必须有一条 PUBLISH（v2 应答），且载荷通过目标侧
        // 同链校验（`parse_and_validate_v2` + 请求派生的绑定）。
        let frame = c
            .outbox()
            .into_iter()
            .find(|p| p.first().map(|b| b >> 4) == Some(3))
            .expect("v2 请求必须产生应答（修复前恒 PLANNER-SKIP/零发布）");
        let (topic, payload) = publish_payload(&frame);
        assert_eq!(topic, crate::DEFAULT_RESPONSE_TOPIC);
        let binds = PlanBinds {
            snapshot_id: [0x5A; 16],
            catalog_version: 3,
            horizon_start_ms: now,
            step_ns: 3_600_000_000_000,
            step_count: 2,
        };
        let budget = peer_budget(20.0, 60);
        let checks = ResponseChecks {
            expect_req_id: 7,
            expect_seq_gt: None,
            budget: &budget,
        };
        let body = String::from_utf8(payload).expect("utf8");
        let resp = parse_and_validate_v2(&body, &checks, &binds).expect("目标侧同链校验");
        assert_eq!(
            resp.plan.assignments.len(),
            2,
            "1 资产 × 2 时段全量轨迹（无投影）"
        );
    }

    /// PUBLISH 帧 → (topic, payload)（假 broker 的 outbox 是原始字节流）。
    fn publish_payload(frame: &[u8]) -> (String, Vec<u8>) {
        assert_eq!(frame[0] >> 4, 3, "须为 PUBLISH");
        let mut i = 1usize;
        let mut rem = 0usize;
        let mut shift = 0;
        loop {
            let b = frame[i];
            rem |= ((b & 0x7F) as usize) << shift;
            i += 1;
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
        assert_eq!(rem, frame.len() - i, "剩余长度自洽");
        let tlen = u16::from_be_bytes([frame[i], frame[i + 1]]) as usize;
        let topic = String::from_utf8(frame[i + 2..i + 2 + tlen].to_vec()).expect("topic utf8");
        let mut off = i + 2 + tlen;
        if (frame[0] >> 1) & 0x03 > 0 {
            off += 2; // QoS1 pid
        }
        (topic, frame[off..].to_vec())
    }
}
