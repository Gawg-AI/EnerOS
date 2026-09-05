//! 蓝图 62 卷 62.2 WP-62.2.4（S4）：chunked 端点单源——epoch 只有一个
//! 递增点。
//!
//! 单一事实：此前 `ChunkedClient::new` 非测试调用点 ×22、bins+root
//! service.rs 的 epoch `saturating_add` 手递增 ×14（mqtt 一 bin 背 3 组
//! epoch 字段 + 绑定块 + 递增）。结构性风险 = 新消费点漏递增/错递增即
//! 破坏会话身份线序（68.1 口径订正见下）。本类型把
//! "client + epoch" 收拢为一个端点：epoch 私有，递增只发生在
//! [`ChunkedEndpoint::call`] 内——消费方**结构性不可能**漏递增。
//!（68.1 口径订正：服务端不比较 epoch 值——迟到帧拦截由传输代次 +
//! 严格 index 序承担；单调递增纪律保线序身份，见 chunked 模块 doc。）
//!
//! [`ChunkedEndpoint::bind`] 同源收编 EIPC 建端三行闭包
//! （`eipc_resolve` + `Sel4IpcClient::new` + `ok()`），未授予 → None
//! （fail-closed 语义不变）。
//!
//! 禁改项：chunked wire（104B/块、16 块上限）；epoch 初值 0 与回绕语义
//! （saturating）；`Sel4IpcClient` 本体接口。

use alloc::vec::Vec;

use crate::chunked::{ChunkError, ChunkedClient, RawIpc};

/// chunked Call 默认超时（原各消费方手传 5_000ms，值不变）。
pub const CHUNKED_CALL_TIMEOUT_MS: u64 = 5_000;

/// chunked 端点：`client + epoch` 单一持有者。
///
/// `C = Sel4IpcClient`（target）或 mock `RawIpc`（host 测试）。
pub struct ChunkedEndpoint<C: RawIpc> {
    client: C,
    /// 发送方 epoch（重启计数；**私有**——递增纪律由本类型单点承载；
    /// 服务端不比较 epoch 值，单调性保会话身份线序，见 chunked 模块 doc）。
    epoch: u64,
}

impl<C: RawIpc> ChunkedEndpoint<C> {
    /// 包装既有客户端（epoch 从 0 起——与原字段初值语义一致）。
    pub fn wrap(client: C) -> Self {
        Self { client, epoch: 0 }
    }

    /// 包装既有客户端并指定起始 epoch（root 侧跨函数共享 epoch 计数器的
    /// 迁移面：调用前读 `*epoch` 作初值、调用后 [`Self::epoch`] 写回——
    /// 递增点仍只在 [`Self::call`]，共享语义逐字节保真）。
    pub fn wrap_at(client: C, epoch: u64) -> Self {
        Self { client, epoch }
    }

    /// 当前 epoch（只读观测；消费方无递增面——递增仅 [`Self::call`]）。
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// 一次 chunked Call（Header/Data/Final/分页拉取），默认超时
    /// [`CHUNKED_CALL_TIMEOUT_MS`]。
    ///
    /// epoch 每次调用**恰好 +1**（与传输成败无关——原各消费方"建
    /// transfer 即递增"语义保真：传输失败后旧 epoch 序号不得复用；
    /// 迟到帧由服务端传输代次 + 严格 index 序拦截，非 epoch 比较）。
    pub fn call(&mut self, target: &str, req: &[u8]) -> Result<Vec<u8>, ChunkError> {
        self.call_with_timeout(target, req, CHUNKED_CALL_TIMEOUT_MS)
    }

    /// [`Self::call`] 的指定超时形态（蓝图 67 WP-67.2.1）：per-call 超时
    /// 覆盖（原 Sel4EipcChannel 等消费方"timeout_ms==0 → 自持默认"的
    /// 手写面收编为参数），epoch 递增纪律与 [`Self::call`] 完全一致。
    pub fn call_with_timeout(
        &mut self,
        target: &str,
        req: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, ChunkError> {
        let mut transfer = ChunkedClient::new(&mut self.client, self.epoch, timeout_ms);
        self.epoch = self.epoch.saturating_add(1);
        transfer.transfer(target, req)
    }

    /// 非 chunked 的单次 Call 面（config_entry / config_generation 等
    /// `Sel4IpcClient` 直呼方法）——epoch 不经过此面，无递增语义。
    pub fn client_mut(&mut self) -> &mut C {
        &mut self.client
    }

    /// 测试专用 epoch 观测（生产消费方无递增可能——编译期由私有保证）。
    #[cfg(test)]
    pub fn epoch_for_test(&self) -> u64 {
        self.epoch
    }
}

#[cfg(feature = "sel4")]
impl ChunkedEndpoint<crate::ipc_sel4::Sel4IpcClient> {
    /// EIPC 端点绑定（原各消费方
    /// `eipc_resolve(service, target).and_then(|(_, slot)| Sel4IpcClient::
    /// new(label, slot, now_ns).ok())` 三行闭包单源）。未授予/建端失败
    /// → `None`（fail-closed，语义同原闭包）。
    pub fn bind(service: &str, target: &str) -> Option<Self> {
        let (_, slot) = crate::eipc_resolve(service, target)?;
        let client =
            crate::ipc_sel4::Sel4IpcClient::new(target, slot, eneros_drv_svc::now_ns).ok()?;
        Some(Self::wrap(client))
    }

    /// cap 寻址绑定（蓝图 64 卷 64.2 WP-64.2.1d——bind 的槽位寻址姊妹
    /// 形态）：root 持 `SpawnedHandle.ep` 裸 cap 直连，不经 eipc_resolve
    /// 授予表。建端失败 → `None`（fail-closed，语义同 bind）。
    pub fn bind_cap(label: &str, dest_cap: u64) -> Option<Self> {
        let client =
            crate::ipc_sel4::Sel4IpcClient::new(label, dest_cap, eneros_drv_svc::now_ns).ok()?;
        Some(Self::wrap(client))
    }

    /// [`Self::bind_cap`] 的指定起始 epoch 形态（蓝图 67 WP-67.2.1）：
    /// 供 svc-energy-agent/channel 类「instance_epoch 起步」的装配——
    /// epoch 初值语义与原手搓字段一致，递增点仍只在 [`Self::call`]。
    /// 错误语义与 bind_cap 一致（`None` fail-closed）。
    pub fn bind_cap_at(label: &str, dest_cap: u64, epoch: u64) -> Option<Self> {
        let client =
            crate::ipc_sel4::Sel4IpcClient::new(label, dest_cap, eneros_drv_svc::now_ns).ok()?;
        Some(Self::wrap_at(client, epoch))
    }
}

/// 共享 epoch 会话：每次 [`Self::call`] 后**无条件**把端点 epoch 写回
/// 共享计数器（与成败无关）。
///
/// root 侧三步手工协议（`wrap_at(ipc, *epoch)` → `call` →
/// `*epoch = endpoint.epoch()`）的结构化收拢——写回是 [`Self::call`]
/// 返回前的固定步骤，调用方结构性不可能漏写（63.2 WP-63.2.1；f207fab
/// 修的正是漏写实例）。共享计数器跨函数/跨会话连续：`wrap_shared` 以
/// 当前值为起始 epoch；同一函数内两个会话顺序复用同一 `&mut u64`
/// （obs_report enqueue/flush 形态）即可，禁止为绕借用检查复制计数器。
pub struct ChunkedSession<'a, C: RawIpc> {
    endpoint: ChunkedEndpoint<C>,
    shared: &'a mut u64,
}

impl<'a, C: RawIpc> ChunkedSession<'a, C> {
    /// 以共享计数器当前值为起始 epoch 建会话（迁移面语义：调用前读
    /// `*shared` 作初值）。
    pub fn wrap_shared(ipc: C, shared: &'a mut u64) -> Self {
        Self {
            endpoint: ChunkedEndpoint::wrap_at(ipc, *shared),
            shared,
        }
    }

    /// 一次 chunked Call；返回前无条件写回共享计数器（成败无关——
    /// 失败路径同样前进，传输失败后旧 epoch 序号不复用）。
    pub fn call(&mut self, target: &str, payload: &[u8]) -> Result<Vec<u8>, ChunkError> {
        let r = self.endpoint.call(target, payload);
        *self.shared = self.endpoint.epoch();
        r
    }

    /// 非 chunked 的单次 Call 面（stats 观测等）——透传端点客户端。
    pub fn client_mut(&mut self) -> &mut C {
        self.endpoint.client_mut()
    }
}

#[cfg(feature = "sel4")]
impl<'a> ChunkedSession<'a, crate::ipc_sel4::Sel4IpcClient> {
    /// cap 寻址 + 共享 epoch 会话一步装配（蓝图 64 卷 64.2 WP-64.2.1d：
    /// root 侧 `Sel4IpcClient::new` + `wrap_shared` 手抄闭包的归宿）。
    /// 标签恒短字面（≤64B）= **必然成功**装配（蓝图 66 WP-66.5.3 死防御
    /// 收口）：错误源唯一 = 标签 >64B（ipc_sel4 heapless `String::<64>`
    /// 唯一 `endpoint too long`）——调用方以静态字面标签装配时该错误
    /// 结构性不可达，失败脚手架（None 分支 / ok_or_else 文案）即死防御，
    /// 违反前置 = panic（装配期即时暴露，不静默降级）。历史无锚
    /// `bind_shared -> Option`（蓝图 64.2）于本卷删除，不留双入口——
    /// 运行期可变标签路径自行调 `Sel4IpcClient::new` 并如实处理 Err。
    pub fn bind_shared_const(label: &str, dest_cap: u64, shared: &'a mut u64) -> Self {
        let ipc = crate::ipc_sel4::Sel4IpcClient::new(label, dest_cap, eneros_drv_svc::now_ns)
            .expect(
                "bind_shared_const: label exceeds 64B (statically unreachable for literal labels)",
            );
        Self::wrap_shared(ipc, shared)
    }
}

// ============================================================================
// host 红测→定义锁：epoch 每次调用恰好 +1（wire 抓包断言）
// ============================================================================

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;
    use crate::chunked::{ChunkHeader, CHUNK_MAGIC_HEADER};

    #[test]
    fn call_increments_epoch_exactly_once_per_call() {
        let mut ep = ChunkedEndpoint::wrap(SpyIpc::new());
        let _ = ep.call("svc-x", &[1, 2, 3]);
        let _ = ep.call("svc-x", &[4, 5]);
        let _ = ep.call("svc-x", &[6]);
        assert_eq!(ep.epoch_for_test(), 3, "三次 call → epoch 恰好 +3");
        // wire 抓包：三次 Header 帧的 epoch 字段 = 0, 1, 2。
        assert_eq!(ep.client.header_epochs, alloc::vec![0, 1, 2]);
    }

    #[test]
    fn wrap_at_preserves_shared_counter_and_epoch_is_read_only_view() {
        let mut ep = ChunkedEndpoint::wrap(SpyIpc::new());
        let _ = ep.call("svc-x", &[1]);
        assert_eq!(ep.epoch(), 1);
        // 共享计数器写回面：wrap_at 以写回值为初值，继续递增序列。
        let mut ep2 = ChunkedEndpoint::wrap_at(SpyIpc::new(), ep.epoch());
        let _ = ep2.call("svc-x", &[2]);
        assert_eq!(ep2.epoch(), 2, "共享计数序列 0→1→2 连续不重置");
    }

    #[test]
    fn epoch_not_reused_after_transport_error() {
        let mut ep = ChunkedEndpoint::wrap(SpyIpc::rejecting());
        assert!(ep.call("svc-x", &[1]).is_err(), "mock 拒绝 → Err");
        assert!(ep.call("svc-x", &[2]).is_err());
        assert_eq!(
            ep.epoch_for_test(),
            2,
            "失败调用同样消耗 epoch（原语义：建 transfer 即递增）"
        );
    }

    // ---- 蓝图 67 WP-67.2.1：call_with_timeout 定义锁 ----

    #[test]
    fn call_with_timeout_non_default_observed_by_transport() {
        let mut ep = ChunkedEndpoint::wrap(SpyIpc::new());
        let _ = ep.call_with_timeout("svc-x", &[1], 250);
        // 一次 transfer = Header(request) + Final(request) 两拍，超时同参。
        assert_eq!(
            ep.client.timeouts_ns,
            alloc::vec![250 * 1_000_000, 250 * 1_000_000],
            "非默认超时透传传输面（ms→ns）"
        );
        assert_eq!(
            ep.epoch_for_test(),
            1,
            "指定超时形态 epoch 仍恰好 +1（纪律与 call 一致）"
        );
    }

    #[test]
    fn call_uses_default_timeout_5000() {
        let mut ep = ChunkedEndpoint::wrap(SpyIpc::new());
        let _ = ep.call("svc-x", &[1]);
        assert_eq!(
            ep.client.timeouts_ns,
            alloc::vec![
                CHUNKED_CALL_TIMEOUT_MS * 1_000_000,
                CHUNKED_CALL_TIMEOUT_MS * 1_000_000
            ],
            "call = call_with_timeout(默认 5_000)"
        );
    }

    // ---- ChunkedSession（63.2 WP-63.2.1）：共享 epoch 会话定义锁 ----

    #[test]
    fn session_advances_shared_counter_even_on_transport_error() {
        let mut shared: u64 = 7;
        {
            let mut session = ChunkedSession::wrap_shared(SpyIpc::rejecting(), &mut shared);
            assert!(
                session.call("svc-x", &[1]).is_err(),
                "mock 拒绝 → Err（失败注入）"
            );
        }
        assert_eq!(
            shared, 8,
            "传输 Err 后共享计数器仍前进——失败路径推进 = 结构不变量"
        );
    }

    #[test]
    fn session_shared_counter_monotonic_across_calls() {
        let mut shared: u64 = 0;
        let mut session = ChunkedSession::wrap_shared(SpyIpc::new(), &mut shared);
        let _ = session.call("svc-x", &[1]);
        let _ = session.call("svc-x", &[2]);
        assert_eq!(shared, 2, "连续两次 call 共享计数器单调");
    }

    #[test]
    fn two_sessions_sequentially_reuse_same_counter() {
        // obs_report enqueue/flush 形态：两个会话（两个独立客户端）顺序
        // 复用同一 &mut u64——借用合规（R-63.c）且计数连续。
        let mut shared: u64 = 0;
        {
            let mut enq = ChunkedSession::wrap_shared(SpyIpc::new(), &mut shared);
            let _ = enq.call("svc-observability", &[1]);
        }
        {
            let mut flush = ChunkedSession::wrap_shared(SpyIpc::new(), &mut shared);
            let _ = flush.call("svc-observability", &[2]);
            assert!(
                flush.client_mut().calls >= 1,
                "client_mut 透传端点客户端（chunked 一次 call 含多帧 request）"
            );
        }
        assert_eq!(shared, 2, "两会话共享计数器连续（0→1→2，不重置）");
    }

    // ---- 简化 mock（本文件专用；完整 chunked 服务端语义见
    //      protocol-services ingestion_stage 测试）----
    struct SpyIpc {
        reject: bool,
        header_epochs: Vec<u64>,
        timeouts_ns: Vec<u64>,
        calls: usize,
    }
    impl SpyIpc {
        fn new() -> Self {
            Self {
                reject: false,
                header_epochs: Vec::new(),
                timeouts_ns: Vec::new(),
                calls: 0,
            }
        }
        fn rejecting() -> Self {
            Self {
                reject: true,
                header_epochs: Vec::new(),
                timeouts_ns: Vec::new(),
                calls: 0,
            }
        }
    }
    impl RawIpc for SpyIpc {
        fn request(
            &mut self,
            _target: &str,
            payload: &[u8],
            timeout_ns: u64,
        ) -> Result<Vec<u8>, crate::AgentError> {
            self.calls += 1;
            self.timeouts_ns.push(timeout_ns);
            if payload.len() >= 4 && payload[0..4] == *CHUNK_MAGIC_HEADER {
                if let Ok(h) = ChunkHeader::decode(payload) {
                    self.header_epochs.push(h.epoch);
                }
            }
            if self.reject {
                // Header ack 非法 → ServerRejected。
                return Ok(alloc::vec![0xDE, 0xAD, 0xBE, 0xEF]);
            }
            Ok(alloc::vec![b'E', b'N', b'V', b'K'])
        }
        fn send(&mut self, _target: &str, _payload: &[u8]) -> Result<(), crate::AgentError> {
            Ok(())
        }
    }
}

// 蓝图 66 WP-66.5.3：>64B 标签 = panic（前置违反即时暴露）。
// bind_shared_const 为 sel4 特征门——本用例仅 sel4 特征下编译
// （target 面实弹；host 默认面无 sel4 impl，路径不可达故无从用例）。
#[cfg(all(test, feature = "sel4"))]
mod bind_shared_const_tests {
    #[test]
    #[should_panic(expected = "bind_shared_const: label exceeds 64B")]
    fn bind_shared_const_panics_on_overlong_label() {
        let mut shared: u64 = 0;
        let long_label: alloc::string::String = alloc::string::String::from("L").repeat(65);
        let _ = crate::endpoint::ChunkedSession::bind_shared_const(&long_label, 1, &mut shared);
    }
}

// 蓝图 67 WP-67.2.1：bind_cap_at 起始 epoch 可观测（epoch_for_test）。
// 同 bind_shared_const 先例——sel4 特征门（host 默认面无 Sel4IpcClient
// impl，路径不可达；起始 epoch 语义的 host 侧定义锁 =
// wrap_at_preserves_shared_counter_and_epoch_is_read_only_view）。
#[cfg(all(test, feature = "sel4"))]
mod bind_cap_at_tests {
    #[test]
    fn bind_cap_at_starts_at_given_epoch() {
        let ep = crate::endpoint::ChunkedEndpoint::bind_cap_at("bindcapat-probe", 1, 41);
        assert!(
            ep.is_some(),
            "合法短标签建端必然成功（fail-closed 仅限建端失败）"
        );
        assert_eq!(
            ep.unwrap().epoch_for_test(),
            41,
            "bind_cap_at 起始 epoch = 实参"
        );
    }
}

// ============================================================================
// 蓝图 67 WP-67.2.4：仓走守卫——`ChunkedClient::new(` 非测试调用只允许
// 存在于本 crate（epoch 递增纪律单点 = ChunkedEndpoint）。照
// signer-keyring sm2_verify 仓走先例（walk crates/ + 注释行豁免）。
// ============================================================================

#[cfg(test)]
mod repo_walk_guard {
    use std::path::Path;

    /// 白名单：**无**。蓝图预判需豁免 ingestion_stage.rs:729 字符串守卫
    /// 字面，实际守卫模式含尾括号 `ChunkedClient::new(`——字符串守卫行
    /// `src.contains("ChunkedClient::new")` 不含括号，结构性不匹配
    /// （变异验证②实测：白名单文件植入真实调用 → 红；字面守卫在场 →
    /// 绿），零豁免 = 全精度，且杜绝白名单被滥用为第二违纪入口。
    fn walk(dir: &Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("crates/ 可读") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, hits);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let as_str = path.to_string_lossy().to_string();
                // epoch 纪律单点 crate：endpoint/chunked/config_client 本体。
                if as_str.contains("agent-sdk") {
                    continue;
                }
                let src = std::fs::read_to_string(&path).expect("read src");
                for (i, line) in src.lines().enumerate() {
                    // 测试段豁免（简化口径）：`#[cfg(test)]` 起文件尾部不计
                    // ——与 signer-keyring 守卫同粒度。
                    if line.trim_start().starts_with("#[cfg(test)]") {
                        break;
                    }
                    // 注释行不匹配（signer-keyring 先例）。
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    if line.contains("ChunkedClient::new(") {
                        hits.push(format!("{as_str}:{}: {}", i + 1, line.trim()));
                    }
                }
            }
        }
    }

    #[test]
    fn chunked_client_new_only_inside_agent_sdk() {
        // CARGO_MANIFEST_DIR = <root>/crates/agents/agent-sdk → ancestors:
        // [0]=agent-sdk [1]=agents [2]=crates。
        let crates_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("crates root")
            .to_path_buf();
        let mut hits = Vec::new();
        walk(&crates_root, &mut hits);
        assert!(
            hits.is_empty(),
            "ChunkedClient::new 非测试调用越出 agent-sdk（必须改走 ChunkedEndpoint——\
             epoch 单点递增纪律，蓝图 67 WP-67.2.4）:\n{}",
            hits.join("\n")
        );
    }
}
