//! root-task IPC 主循环（KRN-004 / K2.2 / 21-A）：BootInfo 消费后进入
//! EL0 服务 IPC 调度——hello 往返、故障注入、supervisor 重启。
//!
//! W2.2（蓝图23）：删除了"帧分发影子 API"（`dispatch_frame`/
//! `DispatchAction`/`config_service_roundtrip`/`service_name`/
//! `load_services_from_bundle`）——这套纯函数有测试但真实 [`run_ipc_loop`]
//! 从不调用它（用的是 `hello_roundtrip`/`send_frame` 直连 seL4 IPC），
//! 测的不是跑的。MR 帧编解码契约（真实路径共享）保留 host 测试覆盖。

pub use eneros_ipc_wire::mr::{
    frame_from_mrs, frame_into_mrs, FrameMrError, MAX_FRAME_BYTES, MR_FRAME_LEN_SLOT,
};

extern crate alloc;

// ============================================================================
// K2.2：root-task IPC 调度主循环（21-A，替换 spin_loop）
// ============================================================================

/// 47（WP-47.1 编排驱动）：向协议服务发送 drv-svc `OP_EXECUTE` 信封并读取
/// 状态字节（观测：成功/失败均登记串口）。驱动失败不阻断主循环（服务侧
/// fail-closed 语义由服务自己保证）。
///
/// 蓝图 64 卷 64.2（WP-64.2.1a）：手抄 4B/word 打包循环 + 状态字节裸
/// 偏移读删除——wire 面归 `Sel4IpcClient::drv_call`（打包/解包单源）+
/// `wire::decode_response_padded`（响应信封单源）+ `status_handled`
/// （"已处理"判据单源，注释/代码矛盾修正：ST_INVALID(5)=坏信封不算已
/// 处理，0..=4 维持原行为——R-64.b 对照表见执行卡）。
#[cfg(target_os = "none")]
fn protocol_execute(ep: u64, corr: u32, name: &str) -> bool {
    use eneros_drv_svc::runtime::{status_handled, wire, DriverRequest, OP_EXECUTE};
    let mut ipc = match eneros_agent_sdk::ipc_sel4::Sel4IpcClient::new(
        "root-protocol-exec",
        ep,
        eneros_time::get_monotonic_ns,
    ) {
        Ok(c) => c,
        Err(e) => {
            eneros_runtime::println!("[eneros-root-task] PROTO-EXEC {name} client FAIL {e:?}");
            return false;
        }
    };
    let req = DriverRequest::new(corr, OP_EXECUTE);
    match ipc.drv_call(&wire::encode_request(&req), PROTO_EXEC_TIMEOUT_NS) {
        Ok(resp) => {
            // 解不出 = 0xFF（原 get_mr 失败兜底语义；decode_response_padded
            // 含 QEMU 实测 20-words 尾 pad 容差）。
            let status = wire::decode_response_padded(&resp).map_or(0xFF, |r| r.status);
            eneros_runtime::println!("[eneros-root-task] PROTO-EXEC {name} status=0x{status:02x}");
            status_handled(status)
        }
        Err(e) => {
            eneros_runtime::println!("[eneros-root-task] PROTO-EXEC {name} call FAIL {e:?}");
            false
        }
    }
}

/// PROTO-EXEC drv_call 超时：原手抄路径为无限期阻塞 `ipc_call`；drv_call
/// 契约要求显式 deadline——取近无限值保持阻塞语义（≤2^62 防 `checked_add`
/// 溢出；超时判定与 deadline 同钟自洽，真实应答在同一次 Call 内返回）。
#[cfg(target_os = "none")]
const PROTO_EXEC_TIMEOUT_NS: u64 = 1 << 62;

/// IPC 循环 fail-closed 挂起（红线 4：任何 IPC 错误 = 挂起，禁止降级运行）。
#[cfg(target_os = "none")]
fn ipc_fail_closed(step: &str, e: impl core::fmt::Debug) -> ! {
    eneros_runtime::println!(
        "[eneros-root-task] IPC loop FAILED at {}: {:?} — 挂起（fail-closed）",
        step,
        e
    );
    loop {
        core::hint::spin_loop();
    }
}

/// WP-37.1 #8：readiness 判定结果（readiness 来自 IPC handshake/health
/// probe，非"spawn 返回成功"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessVerdict {
    /// 就绪通知在 deadline 内到达（handshake 通过）。
    Ready,
    /// deadline 内无就绪通知（服务未响应/未实现 IPC 循环）→ 显式降级，
    /// 不虚报 Ready（蓝图：optional 失败写 CapabilityUnavailable）。
    Degraded,
}

/// WP-37.1 #8：单服务 readiness 判定（**异步收集**，纯函数 host 可测）——
/// 每轮调用一次，非忙等轮询。语义：
/// - 就绪信号已到 → `Ready`；
/// - 未到且已超 deadline → `Degraded`（不虚报 Ready）；
/// - 未到且未超时 → `None`（继续轮询）。
///
/// target 主循环在 hello 等阻塞点之间调用（服务在此期间运行并 Signal
/// 就绪通知）；QEMU 实测 root 高优先级自旋 busy_wait 会饿死 EL0 服务，
/// 禁止用忙等实现 deadline。
pub fn check_readiness(
    start_ms: u64,
    deadline_ms: u64,
    now_ms: u64,
    signaled: bool,
) -> Option<ReadinessVerdict> {
    if signaled {
        Some(ReadinessVerdict::Ready)
    } else if now_ms.saturating_sub(start_ms) >= deadline_ms {
        Some(ReadinessVerdict::Degraded)
    } else {
        None
    }
}

/// WP-37.1 #8：readiness 探测 deadline（每服务；就绪 Signal 在服务 init
/// 完成后首个调度周期内到达，drv 占位服务自旋不 Signal → 超时降级）。
/// 单位毫秒（虚拟时钟；QEMU TCG 无 icount 时约慢 50-60×，1s 虚拟 ≈ 60s
/// 真实，落在 200s QEMU 预算内；真机虚拟≈真实时间时 1s 对首调度周期
/// 内到达的 Signal 仍属宽裕）。与 [`ReadinessContract::default_handshake`]
/// 同语义（超时 → 显式降级）。
/// 41：1_000 → 3_000——svc-config 启动含 1 次 SM2 bundle 验签（QEMU
/// TCG 实测 ~2s 虚拟），1s 窗口在验签期间耗尽 → required 误降级
/// （bundle 已装载、服务正常服务，仅 readiness 计数错过）。
/// 70.6：3_000 → 15_000——(a) svc-config 启动可含 2 次 SM2（嵌入束 + 盘上
/// ConfigActive 载荷启动级验签，WP-70.6.4）；(b) qemu-prod 实弹两轮：dispatch
/// 装配（catalog/binding/chain-key 拉取 + durable intent/audit 块 IO 打开）
/// 的 ready 信号在 TCG 抖动下晚至 ~8-10s，3s/6s 窗均误降级 required 服务
///（PRODUCT-READY false 级联）。15s 仍保底"永不发信号"的有界判定。
#[allow(dead_code)] // 44.2：readiness 超时上限，当前路径未用（44.4 冻结登记）
const READINESS_DEADLINE_MS: u64 = 15_000;

// ============================================================================
// 蓝图 64 卷 64.2（WP-64.2.5）：root 编排状态单源——run_ipc_loop 循环捕获
// 的全部可变局部归一（"何时"单源 = wiring::PERIODIC_SCHEDULE，63.2 建表；
// 本组类型收"做什么/是否已发/进度"）。类型面免 cfg 门 → host 可单测
// （tick 推进 / once 语义 / readiness 状态迁移——布尔散装时 host 不可达
// 的状态面）；seL4 触点步保留 cfg 门 impl 块。词形/节拍/表零变化。
// ============================================================================

/// 一次性触发旗（原循环内布尔散装 `hang_injected`/`storage_persist_done`/
/// `ota_download_done` 收拢）。once 语义 = 现布尔语义：**真实触发轮立旗**
/// （句柄缺席时保持未立 = 原重试语义），触发一次后永假。
struct OnceFlags {
    hang_injected: bool,
    storage_persist_done: bool,
    ota_download_done: bool,
    identity_push_done: bool,
}

#[cfg_attr(not(target_os = "none"), allow(dead_code))] // host 面死码（target run_ipc_loop 活；66.5.2 收窄）
impl OnceFlags {
    const fn new() -> Self {
        Self {
            hang_injected: false,
            storage_persist_done: false,
            ota_download_done: false,
            identity_push_done: false,
        }
    }

    /// hang 注入触发窗（旗未立 && 到期）。
    fn hang_due(&self, due: bool) -> bool {
        !self.hang_injected && due
    }
    /// 立 hang 旗（真实触发轮调用）。
    fn mark_hang(&mut self) {
        self.hang_injected = true;
    }
    /// 存储落盘触发窗。
    fn storage_due(&self, due: bool) -> bool {
        !self.storage_persist_done && due
    }
    /// 立存储旗。
    fn mark_storage(&mut self) {
        self.storage_persist_done = true;
    }
    /// OTA 下载触发窗。
    fn ota_due(&self, due: bool) -> bool {
        !self.ota_download_done && due
    }
    /// 立 OTA 旗。
    fn mark_ota(&mut self) {
        self.ota_download_done = true;
    }
    /// 70.5：身份推送触发窗。
    fn identity_due(&self, due: bool) -> bool {
        !self.identity_push_done && due
    }
    /// 立身份推送旗。
    fn mark_identity(&mut self) {
        self.identity_push_done = true;
    }
}

/// readiness 异步收集状态（原 `readiness_start_ms`/`readiness_state` 双
/// 平行 Vec + `readiness_summary_printed` 旗收拢——同文件 HangMonitor
/// 先例：防错位不变量随状态机内藏）。
struct ReadinessTracker {
    start_ms: alloc::vec::Vec<u64>,
    /// None = 未判定（继续收集）；Some = 已判定。
    state: alloc::vec::Vec<Option<ReadinessVerdict>>,
    summary_printed: bool,
}

#[cfg_attr(not(target_os = "none"), allow(dead_code))] // host 面死码（66.5.2 收窄）
impl ReadinessTracker {
    fn new(start_ms: u64, count: usize) -> Self {
        Self {
            start_ms: alloc::vec![start_ms; count],
            state: alloc::vec![None; count],
            summary_printed: false,
        }
    }

    /// 该服务已判定（判过后循环跳过其 ntfn poll——原 `is_some` continue）。
    fn is_decided(&self, idx: usize) -> bool {
        self.state[idx].is_some()
    }

    /// 全部判定完成（汇总行前提）。
    fn all_decided(&self) -> bool {
        self.state.iter().all(Option::is_some)
    }

    /// 单服务判定步（[`check_readiness`] 判定 + 状态登记）。返回 `None`
    /// = 未到期，继续收集。
    fn decide(
        &mut self,
        idx: usize,
        deadline_ms: u64,
        now_ms: u64,
        signaled: bool,
    ) -> Option<ReadinessVerdict> {
        let verdict = check_readiness(self.start_ms[idx], deadline_ms, now_ms, signaled)?;
        self.state[idx] = Some(verdict);
        Some(verdict)
    }

    /// 汇总步：全部判定且未打印过 → 打印 PRODUCT-READY 行（词形逐字节
    /// 不变）并置已打印旗；返回是否本次打印（调用方忽略；host 测试的
    /// 可观测面）。`rows` = (服务名, 是否 required)——调用方从 handles
    /// 投影（类型解耦使状态机 host 可测）。
    fn summarize(&mut self, rows: &[(&str, bool)]) -> bool {
        if self.summary_printed || !self.all_decided() {
            return false;
        }
        self.summary_printed = true;
        let degraded: alloc::vec::Vec<&str> = rows
            .iter()
            .zip(self.state.iter())
            .filter(|(_, v)| **v == Some(ReadinessVerdict::Degraded))
            .map(|(h, _)| h.0)
            .collect();
        // WP-37.1：PRODUCT-READY 判据 = **required 服务**全 Ready（root
        // 运行到本行即 Ready——elfloader 装载，无 readiness 轮询；optional
        // 降级只写 CapabilityUnavailable，不阻断）。
        let required_degraded: alloc::vec::Vec<&str> = rows
            .iter()
            .zip(self.state.iter())
            .filter(|(h, v)| h.1 && **v == Some(ReadinessVerdict::Degraded))
            .map(|(h, _)| h.0)
            .collect();
        eneros_runtime::println!(
            "[eneros-root-task] PRODUCT-READY: {} (degraded: {}; required degraded: {})",
            required_degraded.is_empty(),
            degraded.join(","),
            required_degraded.join(",")
        );
        true
    }
}

/// root 编排状态单源（[`run_ipc_loop`] 循环捕获的全部可变局部归一；
/// 死局部 `fault_tick` 不入——63.2 并表后恒等冗余，删除）。
#[cfg_attr(not(target_os = "none"), allow(dead_code))] // host 面字段不回读（target run_ipc_loop 活；66.5.2 收窄）
struct Orchestration {
    /// 主循环轮次（hello 往返后 +1——原 `tick` 语义）。
    tick: u64,
    /// 一次性触发旗。
    once: OnceFlags,
    /// readiness 异步收集。
    readiness: ReadinessTracker,
    /// 心跳监控（WP-40.4/57.3——状态机本体在 supervisor，收编为字段）。
    hang_monitor: crate::supervisor::HangMonitor,
    /// 33 WP-33.5：驱动 IRQ 计数（每驱动一槽）。
    drv_irq_counts: alloc::vec::Vec<u64>,
    /// IRQ 计数（root 自身 ntfn；write-only 序列字段蓝图 66 已删）。
    irq_count: u64,
    /// 控制链采集序列 / epoch / 完成轮次。
    collect_seq: u64,
    chain_epoch: u64,
    /// 70.7.5：MGT2 OTA 中继的 chunked 会话 epoch（独立于探针/链）。
    ota_relay_epoch: u64,
    chain_rounds: u64,
    /// 50（WP-50.1）：可观测遥测上报 epoch。
    obs_epoch: u64,
    /// 70.5 复查：身份推送连续传输错误计数（有界重试——见
    /// `identity_push_step`；成功/not-ready 回执归零）。
    identity_push_errs: u8,
    /// 70.6（WP-70.6.3）：root 管理状态机（维护/实例代次/受控重启裁决
    /// 单源；seL4 触点在 mgmt_poll_step/apply_manual_restart）。
    mgmt: crate::mgmt::RootMgmtState,
    /// 70.6（WP-70.6.5）：MGT1 探针状态机（qemu-test-synth 装配驱动；
    /// release 构建字段恒 Idle 且 step 体 cfg 隔离）。
    probe_state: crate::mgt_probe::ProbeState,
    probe_entered: u64,
    probe_neg_sent: bool,
    probe_done: bool,
}

#[cfg_attr(not(target_os = "none"), allow(dead_code))] // host 面死码（66.5.2 收窄）
impl Orchestration {
    fn new(
        service_count: usize,
        driver_count: usize,
        start_ms: u64,
        heartbeat_miss_limit: i64,
    ) -> Self {
        Self {
            tick: 0,
            once: OnceFlags::new(),
            readiness: ReadinessTracker::new(start_ms, service_count),
            hang_monitor: crate::supervisor::HangMonitor::new(service_count, heartbeat_miss_limit),
            drv_irq_counts: alloc::vec![0; driver_count],
            irq_count: 0,
            collect_seq: 0,
            chain_epoch: 0,
            ota_relay_epoch: 0,
            chain_rounds: 0,
            obs_epoch: 0,
            identity_push_errs: 0,
            mgmt: crate::mgmt::RootMgmtState::new(service_count),
            probe_state: crate::mgt_probe::ProbeState::Idle,
            probe_entered: 0,
            probe_neg_sent: false,
            probe_done: false,
        }
    }

    /// 直驱服务句柄按名查找（原循环内五处 `.find(|h| h.name == ...)`
    /// 手写收口；名单与查找语义零变化——不扩 SERVICE_WIRING，§4 边界）。
    fn direct_handle(
        handles: &[crate::spawn::SpawnedHandle],
        name: &str,
    ) -> Option<crate::spawn::SpawnedHandle> {
        handles.iter().find(|h| h.name == name).cloned()
    }
}

// 一次性周期的 seL4 触点步（cfg 门——host 仅测 OnceFlags 状态机本体）。
#[cfg(target_os = "none")]
impl Orchestration {
    /// 身份推送传输错误有界重试上限（连续计）：超时/NACK 是可恢复态
    /// （svc-ota 忙于 config 取数 / TCG 慢速），首败即立旗 = 一次瞬时超时
    /// 换整 boot 期 OTA 身份缺失；无界重试则每 10 tick 一次 5s 同步 call
    /// 超时拖慢根环（同步 Call 卡死教训）——3 次后诚实放弃。
    const IDENTITY_PUSH_ERR_LIMIT: u8 = 3;

    /// hang 注入步（原主循环 hang 块整体迁移；旗与到期判定进 OnceFlags，
    /// 词形 HEARTBEAT-TEST 行逐字节不变）。
    fn hang_inject_step(
        &mut self,
        hang_handle: &Option<crate::spawn::SpawnedHandle>,
        handles: &[crate::spawn::SpawnedHandle],
    ) {
        if !self.once.hang_due(crate::wiring::periodic_due(
            crate::wiring::PeriodicTask::HangInject,
            self.tick,
        )) {
            return;
        }
        let Some(h) = hang_handle else {
            return;
        };
        self.once.mark_hang();
        if let Some(idx) = handles.iter().position(|x| x.name == "svc-hang") {
            self.hang_monitor.note_request(idx, self.tick as i64);
        }
        eneros_runtime::println!(
            "[eneros-root-task] HEARTBEAT-TEST: inject hang to svc-hang (one-shot)"
        );
        use eneros_sel4_sys::safe::{ipc_send, message_info_new, set_mr};
        let _ = set_mr(0, u64::from_le_bytes(*b"hang\0\0\0\0"));
        let mi = match message_info_new(0, 0, 0, 1) {
            Ok(m) => m,
            Err(e) => ipc_fail_closed("hang mi", e),
        };
        if let Err(e) = ipc_send(h.ep, mi) {
            ipc_fail_closed("send hang", e);
        }
    }

    /// 存储落盘探针步（一次性；STORAGE-PERSIST 词形逐字节不变）。
    fn storage_persist_step(&mut self, ota_handle: &Option<crate::spawn::SpawnedHandle>) {
        if !self.once.storage_due(crate::wiring::periodic_due(
            crate::wiring::PeriodicTask::StoragePersist,
            self.tick,
        )) {
            return;
        }
        let Some(h) = ota_handle else {
            return;
        };
        self.once.mark_storage();
        match storage_persist_probe(h.ep, &mut self.chain_epoch) {
            Ok(()) => {
                eneros_runtime::println!(
                    "[eneros-root-task] STORAGE-PERSIST: OK (trial persisted + reloaded via svc-storage)"
                );
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[eneros-root-task] STORAGE-PERSIST: FAILED ({e}) — fail-closed 不伪造"
                );
            }
        }
    }

    /// 70.5（WP-70.5.1/70.5.4）：可信身份推送步（一次性）——root 读
    /// boot-manager 写入的 IdentityCapsule（验签链投影）并经 OTA1 0x08
    /// 分发 svc-ota。capsule 缺失 → 发布装配不推送（OTA not-ready
    /// fail-closed）；qemu-test 装配以显式 SYNTHETIC 标签兜底。
    fn identity_push_step(&mut self, ota_handle: &Option<crate::spawn::SpawnedHandle>) {
        if !self.once.identity_due(crate::wiring::periodic_due(
            crate::wiring::PeriodicTask::IdentityPush,
            self.tick,
        )) {
            return;
        }
        // 70.5 实弹订正：svc-config 未就绪窗的 OTA not-ready = 可恢复态
        // ——限频重试（每 10 tick；成功/永久拒绝才立旗）。
        if self.tick % 10 != 0 {
            return;
        }
        let Some(capsule) = resolve_identity_capsule() else {
            self.once.mark_identity();
            eneros_runtime::println!(
                "[eneros-root-task] IDENTITY-PUSH: skipped (capsule absent) — OTA not-ready (fail-closed, WP-70.5.1)"
            );
            return;
        };
        let Some(h) = ota_handle else {
            return;
        };
        // capsule 已由 boot-manager 封印且 decode 时证 canonical——
        // to_bytes() 复现原字节（CRC 不变，非重签）。
        let req = eneros_agent_sdk::ota_wire::encode_identity(&capsule.to_bytes());
        let mut endpoint = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
            "root-ota-identity",
            h.ep,
            &mut self.chain_epoch,
        );
        let resp = endpoint.call("svc-ota", &req);
        match resp {
            Ok(resp) if resp.first() == Some(&1) => {
                self.once.mark_identity();
                eneros_runtime::println!(
                    "[eneros-root-task] IDENTITY-PUSH: OK (slot={} gen={} secver={} floor={} ver={}.{}.{} — svc-ota assembled)",
                    capsule.selected_slot,
                    capsule.bootctrl_generation,
                    capsule.security_version,
                    capsule.security_floor,
                    capsule.product_version[0],
                    capsule.product_version[1],
                    capsule.product_version[2]
                );
            }
            Ok(resp) if resp.starts_with(b"ERR:not-ready") => {
                // 可恢复（config 取数未完成）——不立旗，限频重试。
                self.identity_push_errs = 0;
                if self.tick % 50 == 0 {
                    eneros_runtime::println!(
                        "[eneros-root-task] IDENTITY-PUSH: deferred (svc-ota not-ready — retrying)"
                    );
                }
            }
            Ok(resp) => {
                self.once.mark_identity();
                eneros_runtime::println!(
                    "[eneros-root-task] IDENTITY-PUSH: FAILED (resp={:?}) — OTA not-ready（诚实）",
                    &resp[..resp.len().min(16)]
                );
            }
            Err(e) => {
                // 传输错误（超时/NACK）= 可恢复态——与 not-ready 同径限频
                // 重试，连续 IDENTITY_PUSH_ERR_LIMIT 次后才立旗诚实放弃
                // （70.5 复查：原首败即立旗，与上方"成功/永久拒绝才立旗"
                // 承诺矛盾——一次瞬时超时 = 整 boot 期 OTA 身份缺失）。
                self.identity_push_errs = self.identity_push_errs.saturating_add(1);
                if self.identity_push_errs >= Self::IDENTITY_PUSH_ERR_LIMIT {
                    self.once.mark_identity();
                    eneros_runtime::println!(
                        "[eneros-root-task] IDENTITY-PUSH: FAILED ({e:?}) after {} transport errors — OTA not-ready（诚实）",
                        self.identity_push_errs
                    );
                } else {
                    eneros_runtime::println!(
                        "[eneros-root-task] IDENTITY-PUSH: transport error ({e:?}) — retrying ({}/{})",
                        self.identity_push_errs,
                        Self::IDENTITY_PUSH_ERR_LIMIT
                    );
                }
            }
        }
    }

    /// OTA 下载探针步（一次性）。70.5（WP-70.5.5）：**探针生成与调用都
    /// 受测试专属装配控制**——探针体（固定测试种子签章路径）仅在
    /// `qemu-test-synth` 装配存在；release 闭包（closure gate 强制该
    /// feature 不在）只有显式"probe absent"观测行，零签章路径。
    fn ota_download_step(&mut self, ota_handle: &Option<crate::spawn::SpawnedHandle>) {
        if !self.once.ota_due(crate::wiring::periodic_due(
            crate::wiring::PeriodicTask::OtaDownload,
            self.tick,
        )) {
            return;
        }
        self.once.mark_ota();
        let Some(h) = ota_handle else {
            return;
        };
        ota_probe_body(h.ep, &mut self.chain_epoch);
    }

    /// 70.6（WP-70.6.3）：root 管理端点轮询步——非阻塞 Poll（暗装
    /// reply cap）→ 状态机裁决 → 先 Reply 再执行动作，主循环不阻塞等
    /// 待下一请求。授权面：唯一发起方 svc-mgmt（EIPC 单边授予；MGT1
    /// 认证/RBAC 已在 svc-mgmt 完成，root 只做目标/时序/健康裁决）。
    fn mgmt_poll_step(
        &mut self,
        supervisor: &mut crate::supervisor::Supervisor,
        handles: &[crate::spawn::SpawnedHandle],
        service_names: &[&str],
    ) {
        use eneros_sel4_sys::safe::{get_mr, ipc_poll, ipc_reply, message_info_new, set_mr};
        let ep = crate::spawn::ROOT_MGMT_EP.load(core::sync::atomic::Ordering::Acquire);
        if ep == 0 {
            return;
        }
        let mut sender = 0u64;
        let Ok(mi) = ipc_poll(ep, Some(&mut sender)) else {
            return;
        };
        if sender == 0 {
            return; // 无消息（Poll 空转）。
        }
        let len = eneros_sel4_sys::types::message_info_length(mi);
        let mut words = alloc::vec::Vec::with_capacity(len as usize);
        for i in 0..len {
            match get_mr(i) {
                Ok(w) => words.push(w),
                Err(_) => {
                    eneros_runtime::println!("[eneros-root-task] MGMT-EP: MR read failed");
                    return;
                }
            }
        }
        let Ok(req) = eneros_ipc_wire::mr::frame_from_mrs(&words) else {
            eneros_runtime::println!("[eneros-root-task] MGMT-EP: bad frame (sender={sender})");
            return;
        };
        // ready 投影：boot readiness Ready 且不在重验等待。先取快照
        //（readiness/awaiting 均拷出——下方 &mut self.mgmt 借用互斥）。
        let ready_boot: alloc::vec::Vec<bool> = self
            .readiness
            .state
            .iter()
            .map(|v| *v == Some(ReadinessVerdict::Ready))
            .collect();
        let awaiting = self.mgmt.awaiting_indices();
        let ready_of =
            |idx: usize| ready_boot.get(idx).copied().unwrap_or(false) && !awaiting.contains(&idx);
        // 维护退出健康门：全部 required 服务 lifecycle Ready。
        let required_ready = handles.iter().enumerate().all(|(i, h)| {
            h.requirement != crate::bundle::Requirement::Required
                || supervisor
                    .services
                    .get(i)
                    .map(|s| s.lifecycle_state() == crate::supervisor::LifecycleState::Ready)
                    .unwrap_or(false)
        });
        let mut verdict =
            self.mgmt
                .handle(&req, self.tick, service_names, required_ready, &ready_of);
        // 70.7.5：OTA1 单程中继（0x06 status / 0x07 rollback）——root 是
        // OTA1 唯一可信调用方（无 (*, svc-ota) 授予边，agent-sdk 红灯锁）；
        // OTA1 响应字节原样回填 MGT2 响应（失败 → ERR:ota-relay 不伪造）。
        if let Some(crate::mgmt::MgmtAction::OtaRelay { rollback }) = verdict.action {
            verdict.resp = self.ota_relay_step(handles, rollback);
        }
        if let Some(word) = &verdict.word {
            eneros_runtime::println!("[eneros-root-task] {word}");
        }
        // 先回复（Reply 消费 Poll 暗装 cap；失败不阻断动作侧审计）。
        if let Ok(mrs) = eneros_ipc_wire::mr::frame_into_mrs(&verdict.resp) {
            for (i, v) in mrs.iter().enumerate() {
                let _ = set_mr(i as u64, *v);
            }
            if let Ok(mi2) = message_info_new(0, 0, 0, mrs.len() as u64) {
                let _ = ipc_reply(mi2);
            }
        }
        // 动作（重启在回复之后执行——请求方先拿到 ack）。
        if let Some(crate::mgmt::MgmtAction::Restart(idx)) = verdict.action {
            apply_manual_restart(self, supervisor, handles, idx, self.tick);
        }
    }

    /// 70.7.5：MGT2 OTA 中继（同步 chunked Call svc-ota；与探针同型）。
    /// 响应字节原样返回；失败/空响应 → `ERR:ota-relay`（fail-closed）。
    fn ota_relay_step(
        &mut self,
        handles: &[crate::spawn::SpawnedHandle],
        rollback: bool,
    ) -> alloc::vec::Vec<u8> {
        let Some(h) = handles.iter().find(|h| h.name == "svc-ota") else {
            return crate::root_mgmt_wire::err_resp("ota-unavailable");
        };
        let frame = if rollback {
            crate::mgt_probe::update_rollback_frame()
        } else {
            crate::mgt_probe::update_status_frame()
        };
        match crate::mgt_probe::driver::call_chunked_resp(h.ep, &frame, &mut self.ota_relay_epoch) {
            Some(bytes) if !bytes.is_empty() => bytes,
            _ => crate::root_mgmt_wire::err_resp("ota-relay"),
        }
    }

    /// 70.6：手动重启后 ready 重验步——poll 目标就绪通知，Signal 到达
    /// 即解除等待（实例查询 ready 位自此为真）。
    fn mgmt_ready_reverify_step(&mut self, handles: &[crate::spawn::SpawnedHandle]) {
        use eneros_sel4_sys::safe::ipc_poll;
        for idx in self.mgmt.awaiting_indices() {
            let Some(h) = handles.get(idx) else {
                continue;
            };
            let mut sender = 0u64;
            if let Ok(_mi) = ipc_poll(h.ready_ntfn, Some(&mut sender)) {
                if sender != 0 && self.mgmt.note_ready(idx) {
                    eneros_runtime::println!(
                        "[eneros-root-task] MGMT-RESTART: {} ready re-verified",
                        h.name
                    );
                }
            }
        }
    }

    /// 70.6（WP-70.6.5）：MGT1 探针步（仅 qemu-test-synth 装配编译）。
    /// chunked 直驱 svc-mgmt 非环形动作；维护/重启经 root 状态机直驱
    ///（环形动作免 IPC）；激活直驱 svc-config。
    #[cfg(feature = "qemu-test-synth")]
    fn mgt_probe_step(
        &mut self,
        supervisor: &mut crate::supervisor::Supervisor,
        handles: &[crate::spawn::SpawnedHandle],
        service_names: &[&str],
    ) {
        use crate::mgt_probe::driver::{self, ProbeDirective};
        if self.probe_done {
            return;
        }
        let Some(svc_mgmt_ep) = handles.iter().find(|h| h.name == "svc-mgmt").map(|h| h.ep) else {
            return;
        };
        let Some(svc_config_ep) = handles
            .iter()
            .find(|h| h.name == "svc-config")
            .map(|h| h.ep)
        else {
            return;
        };
        let Some(svc_ota_ep) = handles.iter().find(|h| h.name == "svc-ota").map(|h| h.ep) else {
            return;
        };
        let capsule = read_identity_capsule();
        let boot_slot = capsule.as_ref().map(|c| c.selected_slot);
        let capsule_secver = capsule
            .as_ref()
            .map(|c| c.security_version as u64)
            .unwrap_or(0);
        // 70.7（REG-70.7-BOOT-TRIAL-BIND）：确认记录运行载荷自身身份——
        // capsule.release_id = 已验签 manifest 投影（trial 后的确认必须
        // 与该槽 manifest 逐字段一致，boot-manager 下一轮绑定判据）。
        let capsule_release_id = capsule.as_ref().map(|c| c.release_id).unwrap_or([0u8; 16]);
        let settled = self.readiness.all_decided();
        let restart_done = || {
            handles
                .iter()
                .position(|h| h.name == "svc-observability")
                .map(|i| self.mgmt.generation_of(i) > 0 && !self.mgmt.is_awaiting_ready(i))
                .unwrap_or(false)
        };
        let mut st = self.probe_state;
        let mut entered = self.probe_entered;
        let directive = driver::step(
            &mut st,
            &mut entered,
            svc_mgmt_ep,
            svc_config_ep,
            svc_ota_ep,
            self.tick,
            settled,
            boot_slot,
            capsule_secver,
            capsule_release_id,
            &restart_done,
        );
        self.probe_state = st;
        self.probe_entered = entered;
        match directive {
            ProbeDirective::None => {}
            ProbeDirective::DriveMgt2(req) => {
                // root 状态机直驱（与 mgmt_poll_step 同裁决/词形/动作路径）。
                let ready_boot: alloc::vec::Vec<bool> = self
                    .readiness
                    .state
                    .iter()
                    .map(|v| *v == Some(ReadinessVerdict::Ready))
                    .collect();
                let awaiting = self.mgmt.awaiting_indices();
                let ready_of = |idx: usize| {
                    ready_boot.get(idx).copied().unwrap_or(false) && !awaiting.contains(&idx)
                };
                let required_ready = handles.iter().enumerate().all(|(i, h)| {
                    h.requirement != crate::bundle::Requirement::Required
                        || supervisor
                            .services
                            .get(i)
                            .map(|s| {
                                s.lifecycle_state() == crate::supervisor::LifecycleState::Ready
                            })
                            .unwrap_or(false)
                });
                let verdict =
                    self.mgmt
                        .handle(&req, self.tick, service_names, required_ready, &ready_of);
                if let Some(word) = &verdict.word {
                    eneros_runtime::println!("[eneros-root-task] {word}");
                }
                if let Some(crate::mgmt::MgmtAction::Restart(idx)) = verdict.action {
                    apply_manual_restart(self, supervisor, handles, idx, self.tick);
                }
            }
            ProbeDirective::ConfigActivate => {
                // 激活直驱 svc-config（CFG1 0x06；svc-config 侧打印
                // CONFIG-ACTIVATED 词形并落盘提交）。
                // CFG1 wire 常量与 business-services::config_service 同值
                //（root-task 不依赖该 crate——本地字面 + 布局注释互认）。
                // 激活帧携带真实束长 u32 LE（分片缓冲尾部零填充截断）。
                let mut req = alloc::vec::Vec::new();
                req.extend_from_slice(b"CFG1");
                req.push(0x06); // CMD_ACTIVATE
                let nb = crate::mgt_probe::mgt_probe_data::PROBE_BUNDLE.len() as u32;
                req.extend_from_slice(&nb.to_le_bytes());
                let mut epoch = 0u64;
                let mut session = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
                    "root-mgt-probe-cfg",
                    svc_config_ep,
                    &mut epoch,
                );
                match session.call("svc-config", &req) {
                    Ok(r) if r.len() == 8 => {
                        let gen = u64::from_le_bytes(r[..8].try_into().unwrap());
                        eneros_runtime::println!(
                            "[eneros-root-task] MGT-PROBE: config-activated gen={gen} (via svc-config CFG1)"
                        );
                    }
                    Ok(r) => {
                        eneros_runtime::println!(
                            "[eneros-root-task] MGT-PROBE: activate rejected: {:?}",
                            &r[..r.len().min(24)]
                        );
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[eneros-root-task] MGT-PROBE: activate transport error {e:?}"
                        );
                    }
                }
            }
            ProbeDirective::Mgt1(_) => {
                // 预留（当前探针帧均由 step 内部 chunked 发送）。
            }
            ProbeDirective::Done => {
                self.probe_done = true;
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: sequence complete (probe finished)"
                );
            }
        }
    }
}

/// 70.5（WP-70.5.5）：探针体（qemu-test 专属装配；固定测试种子 0x41 与
/// 测试链 config compile 注入的测试锚配对——release 无此路径亦无该种子）。
#[cfg(all(target_os = "none", feature = "qemu-test-synth"))]
fn ota_probe_body(ota_ep: u64, epoch: &mut u64) {
    let Some(capsule) = resolve_identity_capsule() else {
        eneros_runtime::println!(
            "[eneros-root-task] OTA-DOWNLOAD: FAILED (no identity capsule) — 测试装配亦需身份"
        );
        return;
    };
    match ota_download_probe(ota_ep, epoch, &capsule) {
        Ok(()) => {
            eneros_runtime::println!(
                "[eneros-root-task] OTA-DOWNLOAD: OK (real A/B download/verify/trial/commit)"
            );
        }
        Err(e) => {
            eneros_runtime::println!(
                "[eneros-root-task] OTA-DOWNLOAD: FAILED ({e}) — fail-closed 不伪造"
            );
        }
    }
}

/// release 装配（无 qemu-test-synth feature）：零探针路径——固定测试
/// 种子签章代码不在闭包（WP-70.5.5 隔离判据：qemu-prod 门断言本词形
/// 出现且 OTA-DOWNLOAD 零出现）。
#[cfg(all(target_os = "none", not(feature = "qemu-test-synth")))]
fn ota_probe_body(_ota_ep: u64, _epoch: &mut u64) {
    eneros_runtime::println!(
        "[eneros-root-task] OTA-DOWNLOAD: probe absent (release assembly — test-probe isolated, WP-70.5.5)"
    );
}

/// K2.2/K2.3：root-task IPC 主循环（21-A §2.K2.2/K2.3）。
///
/// 首轮迭代（fault 注入只执行一次，svc-fault 挂起后不再接收消息）：
/// 1. 向 svc-hello 发 hello 请求帧（`ipc_call` 同步往返），打印响应；
/// 2. 向 svc-fault 发 `fault=IllegalAccess` 请求（`ipc_send`——aarch64
///    无除零异常，用 null 地址写触发 Data Abort）；
/// 3. 在 svc-fault 的故障 notification 上阻塞 `ipc_recv`——收到通知即
///    svc-fault 已被内核挂起（故障隔离：不扩散到其他服务）；
/// 4. 再次 hello 往返验证 svc-hello 仍响应（隔离成立证据）。
/// 此后循环仅 hello 心跳（svc-hello tick 递增可见）。
///
/// 任何 IPC 步骤失败 → [`ipc_fail_closed`] 挂起（红线 4）。
#[cfg(target_os = "none")]
pub fn run_ipc_loop(
    handles: &[crate::spawn::SpawnedHandle],
    irq_table: crate::irq::IrqTable,
    irq_binding: Option<crate::irq::IrqBinding>,
    mut irq_drivers: alloc::vec::Vec<crate::driver_probe::IrqDriver>,
    // 38（WP-38.8）：驱动所有权在 driver_supervisor——事件循环经 badge
    // 路由 handle_irq；存活期 = 本循环（驱动不再随局部变量 drop）。
    mut driver_supervisor: eneros_driver_framework::DriverSupervisor,
) -> ! {
    use eneros_ipc_wire::el0::{encode_frame, El0FrameType};
    use eneros_sel4_sys::safe::{get_mr, ipc_poll};

    // 服务表：按名寻址。
    let by_name = |name: &str| {
        handles
            .iter()
            .find(|h| h.name == name)
            .ok_or_else(|| alloc::format!("{name} not spawned"))
    };
    // 44.3（探针可选）：svc-hello/svc-fault 是探针锚点——纯产品装配
    // （qemu-prod/board-prod）不 spawn 它们。主循环探针缺失时自定步幅
    // （busy_wait+yield），故障注入跳过；fail-closed 仅针对已 spawn 服务。
    let hello = match by_name("svc-hello") {
        Ok(h) => Some(h.clone()),
        Err(_) => {
            eneros_runtime::println!(
                "[eneros-root-task] DBG no svc-hello (probe absent) — self-paced supervisor"
            );
            None
        }
    };
    if let Some(h) = &hello {
        eneros_runtime::println!(
            "[eneros-root-task] DBG hello.ep={} hello.tcb={} ep_type={}",
            h.ep,
            h.tcb,
            eneros_sel4_sys::safe::debug_cap_identify(h.ep)
        );
    }
    let fault = match by_name("svc-fault") {
        Ok(h) => Some(h.clone()),
        Err(_) => {
            eneros_runtime::println!(
                "[eneros-root-task] DBG no svc-fault (probe absent) — fault injection disabled"
            );
            None
        }
    };
    if let Some(f) = &fault {
        eneros_runtime::println!(
            "[eneros-root-task] DBG fault.ep={} fault.ntfn={}",
            f.ep,
            f.ntfn
        );
    }

    // 22 §2.W1.3：supervisor——每服务故障状态机（按 handles 顺序）+ fault
    // sender(TCB cap) → 服务索引 映射。故障注入周期性执行（注入前检查
    // supervisor 状态：Suspended/Faulted 不注入——seL4_Send 同步语义，
    // 向挂起服务 send 会永久阻塞 root-task）。
    // 30 RT-029：真实服务名（旧实现 new_for_count 用占位名 `svc-<i>`，
    // SUPERVISOR 故障日志服务名无定位价值——故障重启真证据（WP-27.4）
    // 出证前必须关闭）。
    let service_names: alloc::vec::Vec<&str> = handles.iter().map(|h| h.name.as_str()).collect();
    let mut supervisor = crate::supervisor::Supervisor::new(&service_names, now_ms());
    // WP-37.2：能力授权填充——bundle flags 高 16 位（唯一授权来源）→
    // CapabilityGrantSet；未声明能力一律拒绝（构造保证）。打印 CAPS 行
    // 作为机器可读证据（声明即授予，无隐式授权）。
    for (idx, handle) in handles.iter().enumerate() {
        let caps = crate::bundle::capability_names(handle.capabilities)
            .unwrap_or_default()
            .join(",");
        for name in crate::bundle::capability_names(handle.capabilities).unwrap_or_default() {
            supervisor.services[idx].grants.grant(name);
        }
        eneros_runtime::println!(
            "[eneros-root-task] CAPS: {}={}",
            handle.name,
            if caps.is_empty() { "<none>" } else { &caps }
        );
    }
    // fault 识别：poll 的 sender = fault_ep 的 badge（服务索引+1，spawn 时
    // mint 设置；0 保留 = 未知）。23 G-I 总出口确认：seL4 fault IPC 的
    // sender = fault handler endpoint 的 badge（doFaultTransfer 写入 badge
    // 寄存器）——badge 必须设在 fault_ep mint（服务 CSpace 槽 1），而非
    // notification（1368180 的错位实现，QEMU 实测 sender=0）。
    // 22 §2.W1.6：看门狗接入（hello 心跳后第一轮喂狗；QEMU 无 SP805
    // 时软件模式 base=0，MMIO 不写）。
    let wdt_base = if crate::panic::WDT_MMIO_MAPPED.load(core::sync::atomic::Ordering::Relaxed) {
        crate::panic::WDT_BASE.load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    };
    eneros_watchdog::api::wdt_init(10_000, wdt_base);
    // 38（WP-38.8）：进入事件循环前打印驱动生命周期汇总（证据行）。
    eneros_runtime::println!(
        "[eneros-root-task] ENEROS stage=event-loop-drivers {} healthy={}",
        driver_supervisor.report_line(),
        driver_supervisor.healthy_count()
    );
    // 44.3：svc-fault 探针可选——缺失时 fault_idx=None，注入块跳过。
    let fault_idx = handles.iter().position(|h| h.name == "svc-fault");

    // ============================================================================
    // 蓝图 40 收尾：控制链驱动 + 心跳监控（WP-40.4 隔离面 + Exit #6/#10
    // 实弹）。root 持全部服务 ep——作为采集链代理与编排触发器。
    // ============================================================================
    // 64.2 WP-64.2.5：直驱服务句柄按名查找单点（Orchestration::direct_handle
    // ——原五处手写 .find 收口，名单/语义零变化）。
    let telemetry_handle = Orchestration::direct_handle(handles, "svc-telemetry");
    let energy_handle = Orchestration::direct_handle(handles, "svc-energy-agent");
    let hang_handle = Orchestration::direct_handle(handles, "svc-hang");
    // 41（WP-41.3 落盘）：svc-ota 经 EIPC 调 svc-storage 真落盘——
    // root 作为编排方触发一次 PERSIST+LOAD 往返验证（trial 状态写盘
    // 再读回；证据行 STORAGE-PERSIST）。
    let ota_handle = Orchestration::direct_handle(handles, "svc-ota");
    // 50（WP-50.1）：svc-observability 句柄（可观测遥测上报目标）。
    let obs_handle = Orchestration::direct_handle(handles, "svc-observability");
    if let (Some(t), Some(e)) = (&telemetry_handle, &energy_handle) {
        eneros_runtime::println!(
            "[eneros-root-task] CONTROL-CHAIN: drivers bound telemetry.ep={} energy.ep={}",
            t.ep,
            e.ep
        );
    } else {
        eneros_runtime::println!(
            "[eneros-root-task] CONTROL-CHAIN: telemetry/energy-agent not spawned — chain idle"
        );
    }
    // 心跳缺失容忍轮次。必须 **> 驱动周期**（正常服务在两驱动轮间无
    // 请求无心跳——probe 已清除不误杀；svc-hang probe 未清除 → 超限
    // kill）。150 轮 ≈ 0.2s 真实（hello ~760 轮/s）。
    const HEARTBEAT_MISS_LIMIT: i64 = 150;
    // 周期编排（控制链 5 / 可观测 25+20 / hang 200 / 存储落盘 60 / OTA
    // 70 / 故障注入 50 / 看门狗 100）自本函数常量迁入
    // `wiring::PERIODIC_SCHEDULE` 单源（蓝图 63 卷 63.2 WP-63.2.2）——
    // 到期判定经 `wiring::periodic_due`，本循环零手写周期算式。
    // 64.2 WP-64.2.5：循环捕获的可变局部（tick/once/readiness/心跳/IRQ
    // 计数/链序列）归一进 Orchestration（原 16 个散装 let mut 收拢；
    // 死局部 fault_tick 删除——63.2 并表后恒等冗余）。
    let mut orch = Orchestration::new(
        handles.len(),
        irq_drivers.len(),
        now_ms(),
        HEARTBEAT_MISS_LIMIT,
    );

    loop {
        // 22 §2.W1.6：每轮喂硬件狗（喂狗间隔 < 看门狗超时 10s）。
        eneros_watchdog::api::wdt_kick();

        // WP-26.8（26-C §2.2）：IRQ poll（方案乙：独立 ntfn）——中断触发 →
        // badge ntfn 置位；sender != 0 即有中断；处理后必须 Ack。
        if let Some(binding) = &irq_binding {
            let mut irq_sender = 0u64;
            if let Ok(_mi) = ipc_poll(binding.ntfn_slot, Some(&mut irq_sender)) {
                if irq_sender != 0 {
                    orch.irq_count += 1;
                    if orch.irq_count % 8 == 1 || orch.irq_count < 4 {
                        eneros_runtime::println!(
                            "[eneros-root-task] IRQ: irq={} badge={} count={}",
                            binding.irq,
                            irq_sender,
                            orch.irq_count
                        );
                    }
                    if let Err(e) = irq_table.ack(binding) {
                        ipc_fail_closed("irq ack", e);
                    }
                }
            }
        }

        // WP-27.3 D1：驱动 IRQ 收包接线（S4：IRQ ntfn 与服务 EP 分槽——
        // 每个驱动 IRQ 独立 ntfn，与既有 timer IRQ 并行走 poll；badge 由
        // IrqTable::route 语义保证唯一）。
        // 33 WP-33.5：驱动 IRQ 计数判据行（pl011 RX 等驱动中断 → 计数
        // 递增打印；UART 输入注入时可见递增，不注入则保持 0 如实）。
        for (i, drv) in irq_drivers.iter_mut().enumerate() {
            let mut drv_sender = 0u64;
            if let Ok(_mi) = ipc_poll(drv.binding.ntfn_slot, Some(&mut drv_sender)) {
                if drv_sender != 0 {
                    // 38（WP-38.8）：驱动在 driver_supervisor 内（单所有权），
                    // 按 badge 路由 handle_irq。
                    if let Some(d) = driver_supervisor.driver_for_badge(drv.badge) {
                        d.handle_irq(drv.irq);
                    }
                    if let Err(e) = irq_table.ack(&drv.binding) {
                        ipc_fail_closed("driver irq ack", e);
                    }
                    orch.drv_irq_counts[i] += 1;
                    if orch.drv_irq_counts[i] % 8 == 1 || orch.drv_irq_counts[i] < 4 {
                        eneros_runtime::println!(
                            "[eneros-root-task] DRIVER-IRQ: irq={} count={}",
                            drv.irq,
                            orch.drv_irq_counts[i]
                        );
                    }
                }
            }
        }

        // 39 WP-39.7：virtio-net 外部 peer 回显（0x88b5 ENEROS-NET-RX →
        // TX；非阻塞，无网卡/无帧立即返回）。
        crate::driver_probe::net_echo_poll();

        // 1. 心跳/liveness 锚点往返（svc-hello 探针可选）。
        match &hello {
            Some(hello) => {
                let resp = match hello_roundtrip(hello.ep) {
                    Ok(f) => f,
                    Err(e) => ipc_fail_closed("hello roundtrip", e),
                };
                let payload = match eneros_ipc_wire::el0::decode_frame(&resp) {
                    Ok(f) => f.payload,
                    Err(e) => ipc_fail_closed("decode hello resp", e),
                };
                eneros_runtime::println!(
                    "[eneros-root-task] IPC: recv {}",
                    core::str::from_utf8(payload).unwrap_or("<non-utf8>")
                );
            }
            None => {
                // 探针缺失：自定步幅（root 优先级低于服务优先级，不饿死
                // EL0；yield 让出调度）。纯产品装配下的 supervisor 节拍。
                busy_wait_ms(1);
                eneros_sel4_sys::safe::yield_cpu();
            }
        }

        orch.tick += 1;

        // WP-37.1 #8：readiness 异步收集（每轮在 hello 阻塞点后 poll；
        // 服务在此期间已运行并 Signal 就绪通知）。全部判定后打印一次
        // 汇总行 PRODUCT-READY。状态面 = ReadinessTracker（WP-64.2.5）。
        for (idx, handle) in handles.iter().enumerate() {
            if orch.readiness.is_decided(idx) {
                continue;
            }
            let mut sender = 0u64;
            let signaled = if let Ok(_mi) = ipc_poll(handle.ready_ntfn, Some(&mut sender)) {
                sender != 0
            } else {
                false
            };
            let Some(verdict) =
                orch.readiness
                    .decide(idx, READINESS_DEADLINE_MS, now_ms(), signaled)
            else {
                continue;
            };
            // spawn 已完成 → 生命周期推进到 Started（readiness
            // 判定的合法基线），再转 Ready/Degraded。
            supervisor.services[idx].advance_lifecycle_to_started();
            let _ = supervisor.services[idx].lifecycle_transition(match verdict {
                ReadinessVerdict::Ready => crate::supervisor::LifecycleState::Ready,
                ReadinessVerdict::Degraded => crate::supervisor::LifecycleState::Degraded,
            });
            eneros_runtime::println!(
                "[eneros-root-task] READINESS: {}={:?}",
                handle.name,
                verdict
            );
        }
        if orch.readiness.all_decided() {
            let rows: alloc::vec::Vec<(&str, bool)> = handles
                .iter()
                .map(|h| {
                    (
                        h.name.as_str(),
                        h.requirement == crate::bundle::Requirement::Required,
                    )
                })
                .collect();
            orch.readiness.summarize(&rows);
        }

        // 70.6（WP-70.6.3）：管理端点轮询（MGT2：重启/维护/实例查询——
        // 非阻塞；先 Reply 再执行重启动作）+ 手动重启后 ready 重验。
        orch.mgmt_poll_step(&mut supervisor, handles, &service_names);
        orch.mgmt_ready_reverify_step(handles);

        // 蓝图 40 收尾：控制链驱动（采集代理 INGEST → 编排 TICK）。
        // 诚实边界：39 采集链设备句柄移交（CVR-39-02，41 Entry 前置）未
        // 完成——root 作为**采集链代理**注入样本（source=1 与 svc-telemetry
        // 覆盖声明一致）；样本经 svc-telemetry evaluator 全量校验
        // （coverage/freshness/quality/integrity），不绕过信任评估。
        // 70.6（WP-70.6.3）：维护窗口内停止新的控制写（在途意图由
        // dispatch 自行收敛；恢复写 = 维护退出健康门放行后）。
        if crate::wiring::periodic_due(crate::wiring::PeriodicTask::ControlChain, orch.tick) {
            if orch.mgmt.maintenance_active() {
                if orch.tick % 50 == 0 {
                    eneros_runtime::println!(
                        "[eneros-root-task] MGMT-MAINT: control write paused (maintenance active)"
                    );
                }
            } else if let (Some(t), Some(e)) = (&telemetry_handle, &energy_handle) {
                if let Some(i) = handles.iter().position(|h| h.name == "svc-telemetry") {
                    orch.hang_monitor.note_request(i, orch.tick as i64);
                }
                if let Some(i) = handles.iter().position(|h| h.name == "svc-energy-agent") {
                    orch.hang_monitor.note_request(i, orch.tick as i64);
                }
                match control_chain_tick(t.ep, e.ep, &mut orch.collect_seq, &mut orch.chain_epoch) {
                    Ok((accepted, ea_status, disp_state)) => {
                        orch.chain_rounds += 1;
                        // 蓝图 64 卷 64.2（WP-64.2.1b）：EA 显示词形单源
                        // agent-sdk ea_wire（原手写 4 臂镜像删除——词形
                        // OK/NOACTION/UNAVAILABLE/INVALID 逐字节不变）。
                        let ea = eneros_agent_sdk::ea_wire::ea_display(ea_status);
                        // 蓝图 63 WP-63.1.1：终态字节解码/显示收窄——码表单源
                        // eneros-dispatch-state（原内联 4 态 match + 偏移知识
                        // 删除；终态才显示，推进态/未知码如实 "-"——与 45-62
                        // 证据行词形一致）。
                        // 蓝图 66 WP-66.5.1：投影已返回 DispatchState
                        // （解码在 ea_tick_terminal_state 内完成）。
                        let disp = disp_state
                            .filter(|s| s.is_terminal())
                            .map(|s| s.display_name())
                            .unwrap_or("-");
                        eneros_runtime::println!(
                            "[eneros-root-task] CONTROL-CHAIN: round={} telemetry-ingest={} agent={} dispatch={}",
                            orch.chain_rounds,
                            accepted,
                            ea,
                            disp
                        );
                        // 70.14 探针读数（第七轮，REG-70.12-PROBE-BOOT-TRIAL-STALL）：
                        // boot 面写两枚原子探针（调用前 / 驱动方法体首行）由 root
                        // 每轮读出——trial 停摆后仍可判定"POST 是否递增"：
                        //   pre 增 / enter 不增 ⇒ 阻塞在 trait 分派调用边界
                        //   两者都增 ⇒ 阻塞在驱动方法体内部
                        // 零行为影响（两个 Relaxed load）。
                        #[cfg(feature = "qemu-test-synth")]
                        eneros_runtime::println!(
                            "[eneros-root-task] BOOT-WRITE-PROBE pre={} drv_enter={}",
                            eneros_drv_svc::block_cmd::BOOT_WRITE_PRE_PROBES
                                .load(core::sync::atomic::Ordering::Relaxed),
                            eneros_drv_svc::block_cmd::BLK_WRITE_ENTER_PROBES
                                .load(core::sync::atomic::Ordering::Relaxed)
                        );
                    }
                    Err(e) => {
                        eneros_runtime::println!(
                            "[eneros-root-task] CONTROL-CHAIN: transport error ({e}) — fail-closed"
                        );
                    }
                }
            }
        }
        // 蓝图 69 卷 69.5（WP-69.5.1）：只读观测探针（period 25/offset 8
        // 与 ControlChain 错峰；服务未 spawn 时自然 no-op——handle 缺失
        // 即跳过，与 HangInject 同款装配容错）。
        if crate::wiring::periodic_due(crate::wiring::PeriodicTask::DegradeObserve, orch.tick) {
            if let Some(h) = handles.iter().find(|h| h.name == "svc-ai-worker") {
                degrade_observe_tick(h.ep, &mut orch.chain_epoch);
            }
        }
        // 47（WP-47.1 Exit 编排驱动）→ 蓝图 62 卷 62.2（WP-62.2.1 接线表
        // 单源 + D-1 修复）：协议服务 EXECUTE 编排驱动——驱动名单/周期/
        // 错峰相位全部来自 wiring::SERVICE_WIRING 的 driven 登记（svc-iec104
        // 采集轮自此接入默认编排：offset 5 与 modbus-tcp/mqtt 的 10 错峰；
        // 同相位服务按表序执行——先 svc-modbus-tcp ACQ 保鲜 {意图授权前
        // 遥测新鲜}，后 svc-mqtt 下行写命令轮询 → 意图转发 dispatch；同轮
        // 顺序使 intent 的 SNAPSHOT 与 ACQ 相隔毫秒）。
        for w in crate::wiring::SERVICE_WIRING {
            if let Some(d) = &w.driven {
                if d.is_due(orch.tick) {
                    if let Some(h) = handles.iter().find(|h| h.name == w.name) {
                        let _ =
                            protocol_execute(h.ep, crate::wiring::execute_nonce(orch.tick), w.name);
                    }
                }
            }
        }

        // 50（WP-50.1 exporter 真通道）：可观测遥测上报驱动——root 作为
        // 编排方，周期性向 svc-observability 入队一帧**真实服务健康摘要**
        //（各服务 readiness 实况，非合成 ACQ 读数）并触发 FLUSH → 导出器
        // 经 svc-mqtt 批量帧送达 MQTT broker。驱动失败不阻断主循环。
        if crate::wiring::periodic_due(crate::wiring::PeriodicTask::ObsReport, orch.tick) {
            if let Some(h) = &obs_handle {
                let _ = telemetry_report_tick(h.ep, handles, &mut orch.obs_epoch);
            }
        }

        // 蓝图 40 收尾：hang 注入（一次性；svc-hang 为心跳超时注入参考
        // 服务——蓝图 §9 isolation 行实弹）。单向 Send 裸帧（svc-hang
        // 自定协议，非 el0 wire；卡死后 root 不阻塞）。
        // 64.2 WP-64.2.5：旗与到期判定进 OnceFlags（hang_inject_step）。
        orch.hang_inject_step(&hang_handle, handles);

        // 41（WP-41.3 落盘）：存储真落盘往返验证（一次性）——root 经
        // EIPC 调 svc-ota PERSIST-TRIAL（trial 状态经 svc-storage 块命令
        // 面真写数据盘）再 LOAD-TRIAL 读回。证据行 STORAGE-PERSIST。
        // 64.2 WP-64.2.5：旗与到期判定进 OnceFlags（storage_persist_step）。
        orch.storage_persist_step(&ota_handle);

        // 41（OTA A/B）：真实签名镜像 download→verify→trial→commit 探针
        //（70.5 WP-70.5.5：探针体受 qemu-test-synth 专属装配控制）。
        // 64.2 WP-64.2.5：旗与到期判定进 OnceFlags（ota_download_step）。
        orch.ota_download_step(&ota_handle);

        // 70.5（WP-70.5.1/70.5.4）：可信身份推送（一次性；先于 OTA 探针）。
        orch.identity_push_step(&ota_handle);

        // 70.6（WP-70.6.5）：MGT1 管理动作链探针（qemu-test-synth 专属）。
        // 门控与 `mgt_probe_step` 定义同源：生产 profile（sel4-eipc 无
        // qemu-test-synth）下本调用点必须一并消失，否则 E0599 编译断裂
        //（70.7 门只构建 qemu-test profile，未暴露——回归锁见
        // `.github`/feature-matrix 的 root-task 生产面构建）。
        #[cfg(feature = "qemu-test-synth")]
        if crate::wiring::periodic_due(crate::wiring::PeriodicTask::MgtProbe, orch.tick) {
            orch.mgt_probe_step(&mut supervisor, handles, &service_names);
        }
        #[cfg(not(feature = "qemu-test-synth"))]
        let _ = (&mut supervisor, handles, &service_names);

        // 蓝图 40 收尾：心跳监控（WP-40.4）——poll 受监控服务心跳 ntfn；
        // "发过请求却无进展超限" → suspend + restart（无 fault 卡死也终止，
        // 不拖垮控制服务）。kill 后清 probe 记录（重启即翻篇）。
        for (idx, handle) in handles.iter().enumerate() {
            if handle.heartbeat_ntfn == 0 {
                continue;
            }
            let mut sender = 0u64;
            if let Ok(_mi) = ipc_poll(handle.heartbeat_ntfn, Some(&mut sender)) {
                if sender != 0 {
                    // 蓝图 57：心跳簿记入 HangMonitor（probe 后已响应即解除
                    // pending 的防误杀不变量随状态机内藏——QEMU 实弹教训，
                    // 原 :620-628 注释语义不变）。
                    orch.hang_monitor.observe_beat(idx, orch.tick as i64);
                }
            }
            if let Some(crate::supervisor::HangVerdict::TimedOut { .. }) =
                orch.hang_monitor.hang_verdict(idx, orch.tick as i64)
            {
                let sent = orch.hang_monitor.pending_since(idx).unwrap_or_default();
                let last_beat = orch.hang_monitor.last_beat(idx);
                eneros_runtime::println!(
                    "[eneros-root-task] HEARTBEAT-TIMEOUT: {} no progress {} rounds since probe (last_beat={}, probe={}) — kill+restart",
                    handle.name,
                    HEARTBEAT_MISS_LIMIT,
                    last_beat,
                    sent
                );
                supervisor.advance(now_ms());
                apply_restart_action(
                    &mut supervisor,
                    &mut orch.mgmt,
                    orch.tick,
                    idx,
                    handles,
                    &hello,
                    RestartReason::Heartbeat,
                );
                // 重启翻篇：清除 pending（重启后空闲不心跳不误杀）。
                orch.hang_monitor.clear_after_kill(idx, orch.tick as i64);
            }
        }

        // 2. supervisor：轮询 fault ep（非阻塞）——检测服务故障并决策。
        // 23 G-I 总出口修复：`seL4_Poll` 无消息时返回的 MessageInfo 未定义
        // （QEMU 实测 0x2239c8——垃圾值，length>0 误判为有消息 → 每轮
        // `unknown fault sender=0`）。fault IPC 必有 badge（= 服务索引+1），
        // 以 `sender != 0` 为有消息判据。
        let mut sender = 0u64;
        // 44.3：svc-fault 探针可选——缺失时不 poll fault ntfn（产品装配）。
        if let Some(fault) = &fault {
            if let Ok(_mi) = ipc_poll(fault.ntfn, Some(&mut sender)) {
                let has_msg = sender != 0;
                if has_msg {
                    let name = |i: usize| handles[i].name.as_str();
                    // WP-37.3：fault 详情（seL4 VMFault IPC 消息布局，
                    // aarch64 constants.h：MR0=fault IP, MR1=addr,
                    // MR2=PrefetchFault(1=取指故障), MR3=FSR）——NX 证据：
                    // ExecStack 注入后 MR2=1 且 addr 落在栈区 → 可归因取指
                    // 故障（NX 生效）。读取失败仅影响证据行，不阻断决策。
                    let fault_ip = get_mr(0).unwrap_or(u64::MAX);
                    let fault_addr = get_mr(1).unwrap_or(u64::MAX);
                    let prefetch = get_mr(2).unwrap_or(u64::MAX);
                    let fsr = get_mr(3).unwrap_or(u64::MAX);
                    if let Some(idx) = sender
                        .checked_sub(1)
                        .filter(|&b| (b as usize) < handles.len())
                        .map(|b| b as usize)
                    {
                        eneros_runtime::println!(
                        "[eneros-root-task] FAULT-INFO: {} ip=0x{:x} addr=0x{:x} prefetch={} fsr=0x{:x}",
                        name(idx),
                        fault_ip,
                        fault_addr,
                        prefetch,
                        fsr
                    );
                        apply_restart_action(
                            &mut supervisor,
                            &mut orch.mgmt,
                            orch.tick,
                            idx,
                            handles,
                            &hello,
                            RestartReason::Fault,
                        );
                    } else {
                        eneros_runtime::println!(
                        "[eneros-root-task] SUPERVISOR: unknown fault sender={} — ignore (fail-closed)",
                        sender
                    );
                    }
                }
            }
        }

        // 22 §2.W1.6：定期检查分层看门狗状态（诊断；无 layer 时为 AllFed）。
        if crate::wiring::periodic_due(crate::wiring::PeriodicTask::WatchdogCheck, orch.tick) {
            match eneros_watchdog::api::wdt_check() {
                eneros_watchdog::layered::WatchdogStatus::AllFed => {}
                eneros_watchdog::layered::WatchdogStatus::LayerTimeout(id) => {
                    eneros_runtime::println!(
                        "[eneros-root-task] WDT: layer {:?} timeout — supervisor path",
                        id
                    );
                }
                eneros_watchdog::layered::WatchdogStatus::HardReset => {
                    eneros_runtime::println!(
                        "[eneros-root-task] WDT: HARD RESET due — arming reset"
                    );
                }
            }
        }

        // 3. 周期故障注入（每 50 轮）：svc-fault 探针存在且 Running 时才
        //    注入（Suspended/Faulted 不注入，防 seL4_Send 阻塞 root-task）。
        //    WP-37.3：注入 ExecStack（栈页取指）——NX 生效时产生
        //    instruction-fetch vm fault（可归因：栈地址），supervisor
        //    restart 证明隔离；旧 IllegalAccess（null 写）不证明 NX。
        //    （原独立计数器 fault_tick 与 tick 每轮各 +1 且主循环无
        //    continue——恒等；63.2 WP-63.2.2 并入编排表 Repeat(50,0)，
        //    64.2 WP-64.2.5 死局部删除——节拍即 orch.tick。）
        if crate::wiring::periodic_due(crate::wiring::PeriodicTask::FaultInject, orch.tick) {
            if let (Some(fault), Some(fault_idx)) = (&fault, fault_idx) {
                if supervisor.state_of(fault_idx) == Some(crate::supervisor::ServiceState::Running)
                {
                    eneros_runtime::println!(
                        "[eneros-root-task] IPC: send fault=ExecStack to svc-fault (cycle)"
                    );
                    let req = encode_frame(El0FrameType::Report, b"fault=ExecStack");
                    if let Err(e) = send_frame(fault.ep, &req) {
                        ipc_fail_closed("send fault", e);
                    }
                }
            }
        }

        // 循环回到 1：hello 心跳持续（svc-hello tick 递增可见）。
    }
}

/// restart 决策来源（63.2 WP-63.2.2）：只选择日志词形——序列本体一份。
#[cfg(target_os = "none")]
enum RestartReason {
    /// 心跳超时（HEARTBEAT-TIMEOUT → HEARTBEAT-KILL 族）。
    Heartbeat,
    /// fault ep 故障上报（SUPERVISOR 族）。
    Fault,
}

/// restart 序列单方法（蓝图 63 卷 63.2 WP-63.2.2：原 HEARTBEAT-KILL 与
/// SUPERVISOR 两处逐字双抄收拢）。
///
/// 序列本体：supervisor 推进 → 故障决策 match → 退避 → 重启 → 重启完成
/// 登记一步不缺、顺序不变；`reason` 仅选择串口词形——HEARTBEAT-KILL /
/// SUPERVISOR 两族日志行（含 fault_count/total 字段）逐字节保持
/// （VOL-GATE 词形对照判据）。SUPERVISOR 臂的 hello 复验（隔离成立证据）
/// 与心跳臂的 pending 清除留在调用方（各自独有）。
#[cfg(target_os = "none")]
fn apply_restart_action(
    supervisor: &mut crate::supervisor::Supervisor,
    mgmt: &mut crate::mgmt::RootMgmtState,
    tick: u64,
    idx: usize,
    handles: &[crate::spawn::SpawnedHandle],
    hello: &Option<crate::spawn::SpawnedHandle>,
    reason: RestartReason,
) {
    supervisor.advance(now_ms());
    match supervisor.on_service_fault(idx) {
        Some(crate::supervisor::Action::Restart { delay_ms }) => {
            let name = handles[idx].name.as_str();
            if matches!(reason, RestartReason::Fault) {
                eneros_runtime::println!(
                    "[eneros-root-task] SUPERVISOR: {} fault → restart in {}ms (fault_count={})",
                    name,
                    delay_ms,
                    supervisor
                        .services
                        .get(idx)
                        .map(|s| s.fault_count)
                        .unwrap_or(0)
                );
            }
            busy_wait_ms(delay_ms);
            if let Err(e) = restart_service(&handles[idx]) {
                match reason {
                    RestartReason::Heartbeat => ipc_fail_closed("heartbeat restart", e),
                    RestartReason::Fault => ipc_fail_closed("restart service", e),
                }
            }
            supervisor.on_restart_done(idx);
            // 70.6：实例代次/计数 bump（故障/心跳重启同径；非 manual，
            // 不挂 ready 重验——服务 init 重跑即恢复）。
            mgmt.note_restarted(idx, false, tick);
            match reason {
                RestartReason::Heartbeat => {
                    eneros_runtime::println!(
                        "[eneros-root-task] HEARTBEAT-KILL: {} restarted (hang isolation verified)",
                        name
                    );
                }
                RestartReason::Fault => {
                    eneros_runtime::println!(
                        "[eneros-root-task] SUPERVISOR: {} restarted (total={})",
                        name,
                        supervisor
                            .services
                            .get(idx)
                            .map(|s| s.restart_count)
                            .unwrap_or(0)
                    );
                    // 复验 svc-hello 仍响应（隔离成立；探针缺失时跳过——
                    // 纯产品装配无 hello 锚点）。
                    if let Some(hello) = hello {
                        match hello_roundtrip(hello.ep) {
                            Ok(f) => {
                                if let Ok(fr) = eneros_ipc_wire::el0::decode_frame(&f) {
                                    eneros_runtime::println!(
                                        "[eneros-root-task] SUPERVISOR: svc-hello still responsive ({})",
                                        core::str::from_utf8(&fr.payload)
                                            .unwrap_or("<non-utf8>")
                                    );
                                }
                            }
                            Err(e) => ipc_fail_closed("hello recheck", e),
                        }
                    }
                }
            }
        }
        Some(crate::supervisor::Action::MarkFaulted) => {
            let name = handles[idx].name.as_str();
            match reason {
                RestartReason::Heartbeat => {
                    eneros_runtime::println!(
                        "[eneros-root-task] HEARTBEAT-KILL: {} FAULTED (threshold) — no auto restart",
                        name
                    );
                }
                RestartReason::Fault => {
                    eneros_runtime::println!(
                        "[eneros-root-task] SUPERVISOR: {} FAULTED (≥{} faults/hour) — no auto restart, alert",
                        name,
                        crate::supervisor::FAULT_THRESHOLD_PER_HOUR
                    );
                }
            }
        }
        None => {}
    }
}

/// 70.6（WP-70.6.3）：管理面受控重启（人工授权路径——与 supervisor 自动
/// 重启分径：跳过故障决策/计数，直接复用真实生命周期执行序列
/// `restart_service`）。执行后 `note_restarted(manual=true)`：实例代次
/// bump + ready 重验挂起（调用方轮询就绪通知）。
#[cfg(target_os = "none")]
fn apply_manual_restart(
    orch: &mut Orchestration,
    supervisor: &mut crate::supervisor::Supervisor,
    handles: &[crate::spawn::SpawnedHandle],
    idx: usize,
    tick: u64,
) {
    let name = handles[idx].name.as_str();
    let next_gen = orch.mgmt.generation_of(idx).wrapping_add(1);
    supervisor.advance(now_ms());
    // 受控退避（短——管理重启非故障路径；20ms 让目标消息排空）。
    busy_wait_ms(20);
    if let Err(e) = restart_service(&handles[idx]) {
        ipc_fail_closed("mgmt restart", e);
    }
    supervisor.on_restart_done(idx);
    orch.mgmt.note_restarted(idx, true, tick);
    eneros_runtime::println!(
        "[eneros-root-task] MGMT-RESTART: {} restarted (gen={next_gen}) — awaiting ready re-verify",
        name
    );
}

/// 当前毫秒时间（supervisor 退避/窗口用；无墙钟，用单调时钟折算）。
#[cfg(target_os = "none")]
fn now_ms() -> u64 {
    eneros_time::get_monotonic_ns() / 1_000_000
}

/// 绝对单调毫秒（CNTPCT_EL0 直接换算——与 EL0 服务 `drv_svc::now_ms`
/// 同基准；40 收尾：root 的相对时钟（MonotonicClock 减 boot_ns）与
/// 服务端绝对时钟混用导致跨进程样本 Stale 误判，QEMU 实弹暴露——
/// 遥测样本时间戳必须与 evaluator 的 now 同基准）。
#[cfg(target_os = "none")]
fn abs_now_ms() -> u64 {
    eneros_hal::arm64::timer::clock().now_ns() / 1_000_000
}

/// 忙等指定毫秒（root-task 单线程无睡眠原语；退避等待用）。
#[cfg(target_os = "none")]
fn busy_wait_ms(ms: u64) {
    let start = eneros_time::get_monotonic_ns();
    let target = start + ms.saturating_mul(1_000_000);
    while eneros_time::get_monotonic_ns() < target {
        core::hint::spin_loop();
    }
}

/// 重启服务（22 §2.W1.3）：fault 后 TCB 被内核挂起且对象保留——
/// `tcb_write_registers` 重置 PC/SP/X0..X7（与 configure_service 相同的
/// 布局）+ `tcb_resume` 即从 `_start` 重新执行（无需
/// revoke/retype 对象；bss 未清零为已知语义——服务重启保留静态状态，
/// 生产服务由 supervisor 重启策略保证幂等性）。重启后服务重新执行
/// init → 重新 Signal 就绪 ntfn（X2=READY_NTFN_CSPACE_SLOT；root 侧
/// readiness 重新判定）。心跳超时 kill 路径同用本函数（蓝图 40 收尾：
/// 无 fault 卡死 worker → suspend + 本函数重启）。
#[cfg(target_os = "none")]
fn restart_service(
    handle: &crate::spawn::SpawnedHandle,
) -> Result<(), eneros_sel4_sys::safe::Sel4Error> {
    use eneros_sel4_sys::safe::{tcb_resume, tcb_suspend, tcb_write_registers};

    // 寄存器布局由 spawn::initial_registers 单一持有（W2.1）——与 configure_service
    // 共享同一构造点，count 由该函数契约保证。X3/X4 经 handle 记录的
    // mask/心跳槽推导（与 spawn 同源）；svc-storage/net 的 X3/X4/X5 经
    // handle.storage_handover / net_handover 重建（41/43/46 设备移交——
    // MMIO/DMA 映射 restart 期间保持，物理地址不变）。
    // SPEC-B WP-B.1：X6 每次重灌前**新读** PL031 并过闸门（stage=restart）
    // ——受控重启后墙钟继续前进，不回退到 spawn 时刻旧值。
    // SPEC-D WP-D.1：X7 经 handle.uart_handover 同源重建。
    let regs = crate::spawn::initial_registers(
        handle.entry,
        handle.stack_top,
        handle.buffer_vaddr,
        crate::spawn::READY_NTFN_CSPACE_SLOT,
        handle.eipc_mask,
        crate::spawn::heartbeat_slot_of(&handle.name),
        crate::spawn::gated_wall_epoch("restart"),
        handle.storage_handover,
        handle.net_handover,
        handle.uart_handover,
    );
    // 心跳超时路径：目标可能仍在运行（死循环无 fault）——先 suspend 再
    // 重写寄存器（fault 路径 TCB 已被内核挂起，suspend 幂等）。
    tcb_suspend(handle.tcb)?;
    // count=spawn::INITIAL_REG_COUNT（=11）：与 spawn 一致（X5 供 svc-storage/
    // net 设备移交，其他服务 X5=0；X6=SPEC-B 墙钟 epoch 秒；X7=SPEC-D UART
    // MMIO vaddr **或** 70.7 svc-storage boot 池基址——两集互斥；写入面到
    // X7（规范索引 10）为止，无 X8（C-ABI 第 9 参不经寄存器，run12/13 实弹））。
    tcb_write_registers(handle.tcb, 0, crate::spawn::INITIAL_REG_COUNT as u64, &regs)?;
    tcb_resume(handle.tcb)
}

/// 蓝图 40 收尾：一轮控制链驱动（root 为采集链代理 + 编排触发器）。
///
/// 1. INGEST：向 svc-telemetry 注入 6 点样本（source=1，quality Good，
///    integrity Verified，seq 单调）——39 采集链设备句柄移交（CVR-39-02，
///    41 Entry 前置）完成前的 QEMU 装配路径；样本经 evaluator 全量校验；
/// 2. TICK：触发 svc-energy-agent 编排（telemetry → worker → dispatch →
///    EIPC → device-agent 全链在 EL0 服务间完成）。
///
/// 返回 (ingest 接受数, EA 状态码, dispatch 终态码)；传输错误 → Err
/// （fail-closed 打印，不猜成功）。
#[cfg(all(target_os = "none", feature = "sel4-eipc"))]
fn storage_persist_probe(ota_ep: u64, epoch: &mut u64) -> Result<(), alloc::string::String> {
    use eneros_agent_sdk::endpoint::ChunkedSession;
    // cap 寻址 + 共享 epoch 会话一步装配（WP-64.2.1d；蓝图 66 WP-66.5.3
    // 起走 bind_shared_const——静态标签 ≤64B 结构性必成，Option 脚手架
    // 死防御已删）。
    let mut endpoint = ChunkedSession::bind_shared_const("root-storage-probe", ota_ep, epoch);
    // OTA1 0x06 PERSIST-TRIAL：remaining(4)‖version(16) → gen(8)。
    // 服务循环为分块 EIPC（ChunkedReceiver）——经共享 epoch 会话发送
    //（蓝图 63 卷 63.2 WP-63.2.1：写回内化进 ChunkedSession::call，
    // 成败无关——结构性不可能漏写回）。蓝图 58：请求构造/响应解析走
    // ota_wire 单源。
    let req = eneros_agent_sdk::ota_wire::encode_persist_trial(2, "1.2.0");
    let resp = endpoint.call("svc-ota", &req);
    let resp = resp.map_err(|e| alloc::format!("persist transport: {e:?}"))?;
    // 41 修复：svc-ota 失败响应以 "ERR:" 前缀返回（如存储未授予
    // "ERR:store-unavailable"）。不检查前缀就把错误串前 8 字节解成 gen
    // 会打印假成功行（"trial persisted gen=<'ERR:stor' 的 u64>"）——
    // 41 第二轮 QEMU 日志实锤，fail-closed 必须前置拦截。
    let gen = match eneros_agent_sdk::ota_wire::parse_persist_resp(&resp) {
        Ok(g) => g,
        Err(eneros_agent_sdk::ota_wire::OtaWireError::Rejected(msg)) => {
            return Err(alloc::format!("persist rejected: {msg}"));
        }
        Err(eneros_agent_sdk::ota_wire::OtaWireError::Short(len)) => {
            return Err(alloc::format!("persist resp short: {len}"));
        }
        Err(e) => return Err(alloc::format!("persist resp: {e:?}")),
    };
    eneros_runtime::println!(
        "[eneros-root-task] STORAGE-PERSIST: trial persisted gen={} (svc-ota -> svc-storage -> data disk)",
        gen
    );
    // OTA1 0x07 LOAD-TRIAL：→ found(1)‖remaining(4)‖version(16)。
    let req2 = eneros_agent_sdk::ota_wire::encode_load_trial();
    let resp2 = endpoint.call("svc-ota", &req2);
    let resp2 = resp2.map_err(|e| alloc::format!("load transport: {e:?}"))?;
    let (remaining, version) = match eneros_agent_sdk::ota_wire::parse_load_resp(&resp2) {
        Ok(v) => v,
        Err(eneros_agent_sdk::ota_wire::OtaWireError::Rejected(msg)) => {
            return Err(alloc::format!("load rejected: {msg}"));
        }
        Err(_) => {
            return Err(alloc::format!(
                "trial not found after persist (len={})",
                resp2.len()
            ));
        }
    };
    let version = version.as_str();
    eneros_runtime::println!(
        "[eneros-root-task] STORAGE-PERSIST: trial reloaded remaining={} version={}",
        remaining,
        version
    );
    if remaining != 2 || version != "1.2.0" {
        return Err(alloc::format!(
            "trial mismatch: remaining={} version={}",
            remaining,
            version
        ));
    }
    Ok(())
}

/// 70.5（WP-70.5.1）：读取 boot-manager 写入的可信身份 capsule
/// （BOOT-007 内核映射窗口尾页，boot-protocol 单源常量）。无 capsule
/// （旧镜像/写失败/直启）→ `None`（全零页/残页 decode 失败 = 诚实缺席）。
#[cfg(target_os = "none")]
fn read_identity_capsule() -> Option<eneros_boot_protocol::IdentityCapsule> {
    use eneros_boot_protocol::{IdentityCapsule, IDENTITY_CAPSULE_PHYS, IDENTITY_CAPSULE_SIZE};
    let mut raw = [0u8; IDENTITY_CAPSULE_SIZE];
    // SAFETY: IDENTITY_CAPSULE_PHYS 为内核 BOOT-007 显式 1:1 映射窗口
    //（boot-protocol 与 kernel boot.c 单源），只读 IDENTITY_CAPSULE_SIZE
    // 字节；逐字节 volatile 读（无对齐假设）。
    let src = IDENTITY_CAPSULE_PHYS as *const u8;
    for (i, b) in raw.iter_mut().enumerate() {
        *b = unsafe { core::ptr::read_volatile(src.add(i)) };
    }
    IdentityCapsule::decode(&raw).ok()
}

/// 70.5（WP-70.5.1）：身份解析唯一入口——真实 capsule 优先；qemu-test
/// 专属装配（`qemu-test-synth`，closure gate 强制不出现在 release 闭包）
/// 以显式 SYNTHETIC 标签构造测试身份；release 缺 capsule → None
/// （OTA not-ready fail-closed，无任何默认身份分支）。
#[cfg(target_os = "none")]
fn resolve_identity_capsule() -> Option<eneros_boot_protocol::IdentityCapsule> {
    let real = read_identity_capsule();
    #[cfg(feature = "qemu-test-synth")]
    {
        if real.is_none() {
            eneros_runtime::println!(
                "[eneros-root-task] IDENTITY: capsule absent — TEST assembly synthetic identity (qemu-test-synth, non-production)"
            );
            return Some(synthetic_identity_capsule());
        }
    }
    #[cfg(not(feature = "qemu-test-synth"))]
    {
        if real.is_none() {
            eneros_runtime::println!(
                "[eneros-root-task] IDENTITY: capsule absent — OTA not-ready (release fail-closed, WP-70.5.1)"
            );
        }
    }
    real
}

/// qemu-test 专属合成身份（显式测试标签；固定测试模式字节——不冒充
/// 任何生产身份，sku="qemu" 与测试链配置域一致）。
#[cfg(all(target_os = "none", feature = "qemu-test-synth"))]
fn synthetic_identity_capsule() -> eneros_boot_protocol::IdentityCapsule {
    let mut sku = [0u8; 16];
    sku[..4].copy_from_slice(b"qemu");
    let mut c = eneros_boot_protocol::IdentityCapsule::new(
        1,          // slot A
        [0x42; 16], // boot_id（测试模式字节）
        [0x43; 16], // release_id
        sku,
        [0x44; 32],  // manifest_digest（测试模式字节）
        1,           // bootctrl_generation
        1,           // security_version
        1,           // security_floor
        [0, 120, 0], // product_version "0.120.0"（< 探针目标 9.9.9）
    );
    c.seal();
    c
}

/// 47（WP-47.3）：OTA 探针签章测试键种子（**qemu-test 专属装配**——
/// 70.5 WP-70.5.5：本常量与探针调用点同受 `qemu-test-synth` feature 门控，
/// release 闭包零出现；测试链 config compile 显式注入同源测试锚）。
#[cfg(all(target_os = "none", feature = "sel4-eipc", feature = "qemu-test-synth"))]
const OTA_PROBE_TEST_SEED: [u8; 32] = [0x41; 32];

/// 70.5（WP-70.5.1/70.5.3）：探针目标版本——必须高于身份快照当前版本
/// （防降级判据跨测试/生产同律）；"9.9.9" 为测试域惯例形（69.5 t36 同）。
#[cfg(all(target_os = "none", feature = "sel4-eipc", feature = "qemu-test-synth"))]
const OTA_PROBE_TARGET_VERSION: &str = "9.9.9";

/// capsule → 身份字符串字段的 NUL 去除解码（探针描述构造用）。
#[cfg(all(target_os = "none", feature = "sel4-eipc", feature = "qemu-test-synth"))]
fn capsule_field(raw: &[u8]) -> alloc::string::String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    alloc::string::String::from(core::str::from_utf8(&raw[..end]).unwrap_or(""))
}

/// 70.5 v2 探针（qemu-test 专属）：
/// ① 0x09 POLICY-INFO 查询策略代次 + 锚指纹（描述 policy_generation 锚定；
/// ② 从身份 capsule 派生描述（sku/baseline/secver 与可信身份同源——
///    描述与身份不一致在服务端必拒，探针不硬编码身份字面）；
/// ③ 0x01 CHECK-DESC（判定=1）→ 0x02 DOWNLOAD-DESC（双签+镜像）→
///    0x03 TRIAL → 0x04 COMMIT（签名描述 v2 wire 全链）。
#[cfg(all(target_os = "none", feature = "sel4-eipc", feature = "qemu-test-synth"))]
fn ota_download_probe(
    ota_ep: u64,
    epoch: &mut u64,
    capsule: &eneros_boot_protocol::IdentityCapsule,
) -> Result<(), alloc::string::String> {
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    use eneros_agent_sdk::ota_wire::{
        encode_check_desc, encode_download_v2, encode_policy_query, expect_subcode_resp,
        parse_policy_resp, UpdateDescription, DESC_USAGE_OTA_AB_IMAGE, SIG_LEN, SUB_COMMIT,
        SUB_DOWNLOAD, SUB_TRIAL,
    };
    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair, Sm2PrivateKey};

    let mut endpoint = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
        "root-ota-download",
        ota_ep,
        epoch,
    );

    // ① 策略代次锚（未就绪 → 探针诚实失败）。
    let resp = endpoint
        .call("svc-ota", &encode_policy_query())
        .map_err(|e| format!("policy transport: {e:?}"))?;
    let (policy_generation, anchor_fp) =
        parse_policy_resp(&resp).map_err(|e| format!("policy resp: {e:?}"))?;
    eneros_runtime::println!(
        "[eneros-root-task] OTA-DOWNLOAD: policy gen={} anchor_fp={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        policy_generation,
        anchor_fp[0], anchor_fp[1], anchor_fp[2], anchor_fp[3],
        anchor_fp[4], anchor_fp[5], anchor_fp[6], anchor_fp[7]
    );

    // 探针签章**测试键**（种子 0x41——与测试链 config 注入锚同源配对）。
    let sk = Sm2PrivateKey::from_bytes(&OTA_PROBE_TEST_SEED)
        .map_err(|e| format!("ota test key invalid: {e:?}"))?;
    let kp =
        Sm2KeyPair::from_private_key(&sk).map_err(|e| format!("ota test pubkey derive: {e:?}"))?;

    // ② 身份派生描述（sku = 平台绑定 hex32——identity_from_capsule 同源
    // 规则；baseline 来自 capsule；secver ≥ 可信 floor）。
    let image: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
    let hash = sm3_hash(&image);
    let sku_hex: String = capsule
        .sku
        .iter()
        .map(|b| alloc::format!("{b:02x}"))
        .collect();
    let desc = UpdateDescription {
        usage: DESC_USAGE_OTA_AB_IMAGE,
        sku: sku_hex,
        target_version: String::from(OTA_PROBE_TARGET_VERSION),
        baseline_version: format!(
            "{}.{}.{}",
            capsule.product_version[0], capsule.product_version[1], capsule.product_version[2]
        ),
        security_version: capsule.security_floor as u32,
        image_len: image.len() as u64,
        image_hash: hash,
        policy_generation,
    };
    let desc_bytes = desc.encode().map_err(|e| format!("desc encode: {e:?}"))?;
    let mut rng = CsRng::from_seed(&[0x41u8; 32]);
    let mut sign_digest = |digest: &[u8; 32]| -> Result<[u8; SIG_LEN], String> {
        let sig = sm2_sign(digest, &sk, &kp.public_key, &mut rng)
            .map_err(|e| format!("ota sign: {e:?}"))?;
        let mut out = [0u8; SIG_LEN];
        out.copy_from_slice(&sig.to_bytes()[..SIG_LEN]);
        Ok(out)
    };
    let desc_sig = sign_digest(&sm3_hash(&desc_bytes))?;
    let image_sig = sign_digest(&hash)?;

    // ③ CHECK-DESC 先行（判定=1 才继续——CHECK/DOWNLOAD 同判据的正面样本）。
    let req_check = encode_check_desc(&desc, &desc_sig);
    let resp = endpoint
        .call("svc-ota", &req_check)
        .map_err(|e| format!("check transport: {e:?}"))?;
    match resp.as_slice() {
        [1] => {}
        other => {
            return Err(format!(
                "ota check rejected (verdict={:?})",
                other.first().copied().unwrap_or(0)
            ));
        }
    }

    // ④ DOWNLOAD-DESC（描述 + 双签 + 镜像；真实 A/B 写 + 回读验签）。
    let req = encode_download_v2(&desc, &desc_sig, &image_sig, 512, &image);
    let resp = endpoint
        .call("svc-ota", &req)
        .map_err(|e| format!("ota download transport: {e:?}"))?;
    if let Err(eneros_agent_sdk::ota_wire::OtaWireError::Rejected(msg)) =
        expect_subcode_resp(&resp, SUB_DOWNLOAD)
    {
        return Err(format!("ota download rejected: {msg}"));
    }
    expect_subcode_resp(&resp, SUB_DOWNLOAD)
        .map_err(|e| format!("ota verify unexpected resp: {e:?}"))?;
    eneros_runtime::println!(
        "[eneros-root-task] OTA-DOWNLOAD: verified via svc-ota (real A/B write+readback)"
    );

    // 0x03 TRIAL
    let req3 = eneros_agent_sdk::ota_wire::encode_trial(1, OTA_PROBE_TARGET_VERSION.as_bytes());
    let resp3 = endpoint
        .call("svc-ota", &req3)
        .map_err(|e| format!("ota trial transport: {e:?}"))?;
    expect_subcode_resp(&resp3, SUB_TRIAL)
        .map_err(|e| format!("ota trial unexpected resp: {e:?}"))?;
    eneros_runtime::println!("[eneros-root-task] OTA-DOWNLOAD: trial started");

    // 0x04 COMMIT
    let req4 = eneros_agent_sdk::ota_wire::encode_commit();
    let resp4 = endpoint
        .call("svc-ota", &req4)
        .map_err(|e| format!("ota commit transport: {e:?}"))?;
    expect_subcode_resp(&resp4, SUB_COMMIT)
        .map_err(|e| format!("ota commit unexpected resp: {e:?}"))?;
    eneros_runtime::println!("[eneros-root-task] OTA-DOWNLOAD: committed");
    Ok(())
}

/// 45.5 合成遥测注入——**仅 qemu-test 介质**（feature `qemu-test-synth`）：
/// 6 点合成样本（与 svc-telemetry 覆盖声明/绑定一致：电压/电流/频率/SOC/
/// 并网/告警——source=1 为其 allowed_sources）INGEST → accepted。
///
/// TEL_INGEST = 1（TEL_SNAPSHOT = 2——QEMU 实弹曾误用 2 收到
/// SNAPSHOT_REJECTED，协议码以 control_services::telemetry 为准）。
#[cfg(all(target_os = "none", feature = "sel4-eipc", feature = "qemu-test-synth"))]
fn control_chain_synth_ingest(
    telemetry_ep: u64,
    seq: u64,
    epoch: &mut u64,
    now: u64,
) -> Result<u8, alloc::string::String> {
    use eneros_safety_validator::{IntegrityStatus, SampleQuality, TelemetrySample};

    // 蓝图 66 WP-66.5.4：请求 opcode 单源 = agent-sdk tel_wire。
    let mut req: alloc::vec::Vec<u8> = alloc::vec![eneros_agent_sdk::tel_wire::TEL_REQ_INGEST];
    for (point, value) in [
        (1u32, 380.0f64),
        (2, 10.0),
        (3, 50.0),
        // SPEC-E WP-E.3：SOC 确定性双值振荡（按轮次翻转 0.475↔0.525，
        // 振幅 0.05 跨 System1 出厂死区边界 0.5）——持续喂死区学习闭环
        // （|Δsoc|=0.05 噪声序列 ⇒ 半宽收敛至 ~0.1396，带扩至 ~[0.26,0.54]，
        // 触上界 0.14 后转 LEARN-REJECT-CAP），并使学习前后对同域 SOC
        // 的边界判定可对照（C3 前后对照帧）。相位随 seq 单调，transcript 可复现。
        (4, if seq % 2 == 0 { 0.475 } else { 0.525 }),
        (5, 1.0),
        (6, 0.0),
        // 61 复查矫正：点 7 = 独立过温告警（0x0106）——与 points.toml/
        // PROD_FEEDBACK_MAPPING/SAFETY_COVERAGE_POINTS 同步解 6/6 别名。
        (7, 0.0),
    ] {
        let sample = TelemetrySample {
            point,
            value,
            source: 1,
            source_epoch: 0,
            measured_at_ms: now.saturating_sub(50),
            received_at_ms: now,
            quality: SampleQuality::Good,
            sequence: seq,
            integrity: IntegrityStatus::Verified,
            mapping_version: 1,
        };
        // 45.5：58B 编码收归 agent-sdk sample_wire（单一事实）。
        req.extend_from_slice(&eneros_agent_sdk::sample_wire::encode_sample(&sample));
    }
    // INGEST（分块：349B → 4 块；cap 寻址会话一步装配 WP-64.2.1d）。
    let mut endpoint = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
        "root-collect",
        telemetry_ep,
        epoch,
    );
    let resp = endpoint.call("svc-telemetry", &req);
    let resp = resp.map_err(|e| {
        let stats = endpoint.client_mut().stats();
        alloc::format!(
            "ingest transport: {:?} (sent={} recvd={} err={})",
            e,
            stats.0,
            stats.1,
            stats.2
        )
    })?;
    // INGEST 响应：[0, accepted]——解码单源 tel_wire::decode_ingest_ack
    // （蓝图 66 WP-66.5.4：root 手解 resp[0] 删除；证据行词形不变）。
    let accepted = match eneros_agent_sdk::tel_wire::decode_ingest_ack(&resp) {
        Ok(a) => a,
        Err(_) => {
            return Err(alloc::format!(
                "ingest rejected: {:02x?}",
                &resp[..resp.len().min(8)]
            ));
        }
    };
    Ok(accepted)
}

/// 45.5 合成源降级：release/qemu-prod 装配**零合成遥测路径**——closure gate
/// 强制 `qemu-test-synth` 不在 release 闭包；本占位无注入（accepted=0）。
#[cfg(all(
    target_os = "none",
    feature = "sel4-eipc",
    not(feature = "qemu-test-synth")
))]
fn control_chain_synth_ingest(
    _telemetry_ep: u64,
    _seq: u64,
    _epoch: &mut u64,
    _now: u64,
) -> Result<u8, alloc::string::String> {
    Ok(0)
}

/// 50（WP-50.1 exporter 真通道）：可观测遥测上报驱动——root 周期性向
/// svc-observability 入队一帧**真实服务健康摘要**（spawned 服务清单，非
/// 合成 ACQ 读数）并触发 FLUSH。返回 exported 计数（-1 = 失败；失败不
/// 阻断主循环——观测行）。
#[cfg(all(target_os = "none", feature = "sel4-eipc"))]
fn telemetry_report_tick(
    obs_ep: u64,
    handles: &[crate::spawn::SpawnedHandle],
    epoch: &mut u64,
) -> i64 {
    // 1. 服务健康摘要 payload（真实：spawned 服务计数按必需性；
    // 50 实弹修复——逐服务名清单单条 ≈450B × batch8 超帧限 1024B
    // （frame-oversize），改紧凑计数）。
    let required = handles
        .iter()
        .filter(|h| h.requirement == crate::bundle::Requirement::Required)
        .count();
    let optional = handles.len().saturating_sub(required);
    let mut payload: alloc::vec::Vec<u8> = alloc::format!(
        "health:req={} opt={} total={}",
        required,
        optional,
        handles.len()
    )
    .into_bytes();
    // ENQUEUE 帧头已带 created（8B）时间戳——payload 只含健康摘要。
    if payload.len() > 255 {
        payload.truncate(255);
    }
    // 2. ENQUEUE 帧：蓝图 58 走 obs_wire 单源（布局字节不变）。
    let req = match eneros_agent_sdk::obs_wire::encode_enqueue(
        3, // class=Telemetry
        b"root-task",
        b"health",
        abs_now_ms(),
        &payload,
    ) {
        Ok(r) => r,
        Err(_) => {
            // 已截断仍失败 = 不可达；fail-closed 不伪造。
            eneros_runtime::println!("[eneros-root-task] OBS-REPORT enqueue encode FAIL");
            return -1;
        }
    };
    let mut endpoint = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
        "root-obs-enq",
        obs_ep,
        epoch,
    );
    let enq_resp = endpoint.call("svc-observability", &req);
    let enq_resp = match enq_resp {
        Ok(r) => r,
        Err(e) => {
            eneros_runtime::println!("[eneros-root-task] OBS-REPORT enqueue FAIL {e:?}");
            return -1;
        }
    };
    // ENQUEUE 应答约定单源（WP-64.2.1c）：ack=0x01 受理；拒绝携带原始字节
    // （打印面不变——{:02x?} 前 8 字节即原打印源）。
    if eneros_agent_sdk::obs_wire::parse_enqueue_resp(&enq_resp).is_err() {
        eneros_runtime::println!(
            "[eneros-root-task] OBS-REPORT enqueue rejected {:02x?}",
            &enq_resp[..enq_resp.len().min(8)]
        );
        return -1;
    }
    // 3. FLUSH：蓝图 58 走 obs_wire 单源。
    let flush_req = eneros_agent_sdk::obs_wire::encode_flush(8);
    let mut endpoint2 = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
        "root-obs-flush",
        obs_ep,
        epoch,
    );
    let flush_resp = endpoint2.call("svc-observability", &flush_req);
    let flush_resp = match flush_resp {
        Ok(r) => r,
        Err(e) => {
            eneros_runtime::println!("[eneros-root-task] OBS-REPORT flush FAIL {e:?}");
            return -1;
        }
    };
    // FLUSH 成功响应 = exported u64 LE（8B）；失败 = "ERR:..."（不误读为数字）。
    // ERR 约定单源 obs_wire::parse_flush_resp（WP-64.2.1c——原 starts_with
    // 手解删除；拒绝对打印面携带原始字节，词形不变）。
    let exported = match eneros_agent_sdk::obs_wire::parse_flush_resp(&flush_resp) {
        Ok(n) => n as i64,
        Err(eneros_agent_sdk::obs_wire::ObsWireError::FlushRejected(_)) => {
            eneros_runtime::println!(
                "[eneros-root-task] OBS-REPORT flush rejected {:02x?}",
                &flush_resp[..flush_resp.len().min(16)]
            );
            -1
        }
        Err(_) => 0,
    };
    eneros_runtime::println!(
        "[eneros-root-task] OBS-REPORT svc={} exported={}",
        handles.len(),
        exported
    );
    exported
}

/// host/无 sel4-eipc 构建的占位（可观测上报不可用——如实报错，不伪造）。
#[cfg(all(target_os = "none", not(feature = "sel4-eipc")))]
fn telemetry_report_tick(
    _obs_ep: u64,
    _handles: &[crate::spawn::SpawnedHandle],
    _epoch: &mut u64,
) -> i64 {
    -1
}

#[cfg(all(target_os = "none", feature = "sel4-eipc"))]
fn control_chain_tick(
    telemetry_ep: u64,
    energy_ep: u64,
    seq: &mut u64,
    epoch: &mut u64,
) -> Result<(u8, u8, Option<eneros_dispatch_state::DispatchState>), alloc::string::String> {
    let now = abs_now_ms();
    *seq = seq.saturating_add(1);
    // 45.5（合成源降级）：遥测注入经 `qemu-test-synth` 门控——feature 默认
    // 关；qemu-prod/board-prod 等 release 装配走 `control_chain_synth_ingest`
    // 的零合成路径（accepted=0，无注入）。真实采集由协议服务
    // （先行者 svc-modbus-tcp）经 EIPC 上报。TICK 触发段保留（编排触发器）。
    let accepted = control_chain_synth_ingest(telemetry_ep, *seq, epoch, now)?;
    // 2. TICK（1B 请求；响应 = EA 状态 + receipt wire 83B）——cap 寻址会话
    // 一步装配（WP-64.2.1d）。
    let mut endpoint = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
        "root-orchestrator",
        energy_ep,
        epoch,
    );
    let tick_resp = endpoint.call("svc-energy-agent", &[1u8 /* EA_TICK */]);
    let tick_resp = tick_resp.map_err(|e| alloc::format!("tick transport: {:?}", e))?;
    if tick_resp.is_empty() {
        return Err("empty tick response".into());
    }
    let ea_status = tick_resp[0];
    // 蓝图 66 WP-66.5.1：终态投影单源 = dispatch-state
    // `ea_tick_terminal_state`（EA_OK 语义源 = ea_wire；应答布局契约随
    // 投影函数入 crate，root 零手读零偏移知识）。
    let disp_state =
        eneros_dispatch_state::ea_tick_terminal_state(&tick_resp, eneros_agent_sdk::ea_wire::EA_OK);
    Ok((accepted, ea_status, disp_state))
}

/// host/无 sel4-eipc 构建的占位（控制链驱动不可用——如实报错，不伪造）。
#[cfg(all(target_os = "none", not(feature = "sel4-eipc")))]
fn control_chain_tick(
    _telemetry_ep: u64,
    _energy_ep: u64,
    _seq: &mut u64,
    _epoch: &mut u64,
) -> Result<(u8, u8, Option<eneros_dispatch_state::DispatchState>), alloc::string::String> {
    Err(alloc::string::String::from(
        "control chain driver requires sel4-eipc feature",
    ))
}

/// 蓝图 69 卷 69.5（WP-69.5.1）：降级/学习**只读**观测探针——对
/// svc-ai-worker 连发 5 次 `AI_DEGRADE_OBSERVE(0x22)`（QEMU Q5 只读性
/// 判据的「连发 5 次」；`DEGRADE-OBS` 串口行由 svc-ai-worker 处理时落，
/// root 不解析不转发不打印——词形零新增，观测对决策链零接触）。线码
/// 单源 agent-sdk::ai_relay（与 svc-ai-worker 同取一处）。
#[cfg(all(target_os = "none", feature = "sel4-eipc"))]
fn degrade_observe_tick(ai_ep: u64, epoch: &mut u64) {
    use eneros_agent_sdk::ai_relay::AI_DEGRADE_OBSERVE;
    let mut endpoint = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
        "root-orchestrator",
        ai_ep,
        epoch,
    );
    for _ in 0..5u8 {
        // 只读：传输失败静默（观测不可用不得反噬决策链）。
        let _ = endpoint.call("svc-ai-worker", &[AI_DEGRADE_OBSERVE]);
    }
}

/// host/无 sel4-eipc 构建占位（观测探针不可用——同 control_chain_tick 门控）。
#[cfg(all(target_os = "none", not(feature = "sel4-eipc")))]
fn degrade_observe_tick(_ai_ep: u64, _epoch: &mut u64) {}

/// host/无 sel4-eipc 构建的占位（存储探针不可用——如实报错，不伪造）。
#[cfg(all(target_os = "none", not(feature = "sel4-eipc")))]
/// storage persist probe 占位（同 control_chain_tick 门控）。
#[cfg(all(target_os = "none", not(feature = "sel4-eipc")))]
fn storage_persist_probe(_ota_ep: u64, _epoch: &mut u64) -> Result<(), alloc::string::String> {
    Err(alloc::string::String::from(
        "storage persist probe requires sel4-eipc feature",
    ))
}

/// hello 请求→响应往返（K2.2 契约）。返回完整响应帧字节。
#[cfg(target_os = "none")]
fn hello_roundtrip(ep: u64) -> Result<alloc::vec::Vec<u8>, eneros_sel4_sys::safe::Sel4Error> {
    use eneros_ipc_wire::el0::{encode_frame, El0FrameType};
    use eneros_sel4_sys::safe::{get_mr, ipc_call, message_info_new, set_mr};
    use eneros_sel4_sys::types::message_info_length;

    let req = encode_frame(El0FrameType::Report, b"hello");
    let mrs = eneros_ipc_wire::mr::frame_into_mrs(&req)
        .map_err(|_| eneros_sel4_sys::safe::Sel4Error::InvalidArgument)?;
    for (i, w) in mrs.iter().enumerate() {
        set_mr(i as u64, *w)?;
    }
    let mi = message_info_new(El0FrameType::Report as u64, 0, 0, mrs.len() as u64)?;
    eneros_runtime::println!("[eneros-root-task] IPC: send hello-req to svc-hello");
    // 阻塞 call 前让出 CPU：服务（优先级 10）有机会先运行并 recv。
    eneros_sel4_sys::safe::yield_cpu();
    let reply = ipc_call(ep, mi)?;
    let words = message_info_length(reply);
    let mut mr_buf = alloc::vec::Vec::with_capacity(words as usize);
    for i in 0..words {
        mr_buf.push(get_mr(i)?);
    }
    eneros_ipc_wire::mr::frame_from_mrs(&mr_buf)
        .map_err(|_| eneros_sel4_sys::safe::Sel4Error::InvalidArgument)
}

/// 发送一帧（fire-and-forget，K2.3 故障注入用）。
#[cfg(target_os = "none")]
fn send_frame(ep: u64, frame: &[u8]) -> Result<(), eneros_sel4_sys::safe::Sel4Error> {
    use eneros_ipc_wire::el0::El0FrameType;
    use eneros_sel4_sys::safe::{ipc_send, message_info_new, set_mr};

    let mrs = eneros_ipc_wire::mr::frame_into_mrs(frame)
        .map_err(|_| eneros_sel4_sys::safe::Sel4Error::InvalidArgument)?;
    for (i, w) in mrs.iter().enumerate() {
        set_mr(i as u64, *w)?;
    }
    let mi = message_info_new(El0FrameType::Report as u64, 0, 0, mrs.len() as u64)?;
    ipc_send(ep, mi)
}

#[cfg(test)]
mod tests {
    use eneros_ipc_wire::el0::{decode_frame, encode_frame, El0FrameType};

    use super::*;

    // 21-A K2.2：MR 帧编解码往返（root-task ↔ 服务共享契约）。
    // 真实路径 `hello_roundtrip`/`send_frame` 走的就是 frame_into_mrs/
    // frame_from_mrs——这是测的即跑的（W2.2 红线5）。
    #[test]
    fn test_frame_mr_roundtrip() {
        let frame = encode_frame(El0FrameType::Report, b"hello:eneros:tick=1");
        let mrs = frame_into_mrs(&frame).expect("encode");
        assert_eq!(mrs[MR_FRAME_LEN_SLOT as usize], frame.len() as u64);
        let out = frame_from_mrs(&mrs).expect("decode");
        assert_eq!(out, frame, "MR 往返必须无损");
    }

    // W2.2（蓝图23）：真实 run_ipc_loop 往返契约——hello 请求帧与 fault 注入帧
    // 经 MR 编码→解码后，帧类型与 payload 必须完整保留（hello_roundtrip 依赖此
    // 契约解析响应；send_frame 依赖此契约投递故障请求）。这是真实 IPC loop 的
    // host 可测覆盖，替代已删的影子 API 测试。
    #[test]
    fn test_ipc_loop_frame_contract_roundtrip() {
        // hello 请求（hello_roundtrip 发送/接收的帧）。
        let hello = encode_frame(El0FrameType::Report, b"hello");
        let hello_back = frame_from_mrs(&frame_into_mrs(&hello).expect("enc")).expect("dec");
        let f = decode_frame(&hello_back).unwrap();
        assert_eq!(f.frame_type, El0FrameType::Report);
        assert_eq!(f.payload, b"hello");

        // fault 注入请求（send_frame 每 50 tick 发送一次）。
        let fault = encode_frame(El0FrameType::Report, b"fault=IllegalAccess");
        let fault_back = frame_from_mrs(&frame_into_mrs(&fault).expect("enc")).expect("dec");
        let f2 = decode_frame(&fault_back).unwrap();
        assert_eq!(f2.payload, b"fault=IllegalAccess");
    }

    #[test]
    fn test_frame_mr_oversize_rejected() {
        let big = alloc::vec![0u8; MAX_FRAME_BYTES + 1];
        assert_eq!(frame_into_mrs(&big), Err(FrameMrError::FrameTooLong));
        assert_eq!(
            frame_from_mrs(&[(MAX_FRAME_BYTES + 1) as u64]),
            Err(FrameMrError::FrameTooLong)
        );
        assert_eq!(frame_from_mrs(&[]), Err(FrameMrError::NoLengthSlot));
        // 长度声明 16 但只有 2 个 word（1 长度 + 1 载荷）→ BadLength。
        assert_eq!(frame_from_mrs(&[16u64, 0]), Err(FrameMrError::BadLength));
    }

    #[test]
    fn test_frame_mr_partial_word_padding() {
        // 1 字节帧：MR0=1，MR1 只有低 8 位有效。
        let mrs = frame_into_mrs(&[0xAB]).expect("encode");
        assert_eq!(mrs, vec![1, 0xAB]);
        let out = frame_from_mrs(&mrs).expect("decode");
        assert_eq!(out, vec![0xAB]);
    }

    // ==================== WP-64.2.1：root wire 面单源结构守卫 ====================

    /// 四张 seam（drv-svc wire / EA 码表 / obs wire / chunked 端点）的手抄
    /// 形态回归即红（沿 ingestion_stage:536 结构守卫先例；token 按最终代码
    /// 形态锁定——本文件内再出现手抄 wire 面即违纪。token 经 `concat!`
    /// 拼接防守卫字面量自匹配）。
    #[test]
    fn root_wire_faces_live_in_modules() {
        let src = include_str!("service.rs");
        // a) drv-svc wire：手抄 4B/word 打包 / 字面判据 / 裸偏移状态读。
        let t_pack = concat!("chunks", "(4)");
        assert!(
            !src.contains(t_pack),
            "手抄 4B/word 打包回归（应走 drv_call）"
        );
        let t_pred = concat!("status ", "<= 4");
        assert!(
            !src.contains(t_pred),
            "字面状态判据回归（应走 status_handled）"
        );
        let t_off = concat!(".unwrap_or(", "0xFF) & 0xFF");
        assert!(
            !src.contains(t_off),
            "裸偏移状态读回归（应走 decode_response_padded）"
        );
        // b) EA 码表：手写 4 臂镜像回归（词形归 ea_wire 单源）。
        let t_noact = concat!("\"NO", "ACTION\"");
        let t_unavail = concat!("\"UNAV", "AILABLE\"");
        assert!(
            !src.contains(t_noact) && !src.contains(t_unavail),
            "EA 手写臂回归（应走 ea_wire::ea_display）"
        );
        assert!(
            src.contains("ea_wire::ea_display"),
            "EA 显示面必须经 agent-sdk ea_wire 单源"
        );
        // 谓词面同守（蓝图 65 WP-65.5）：EA_OK 常量在谓词上正断言 + 裸
        // 0 字面判据反断言（显示面在守卫面 ≠ 谓词面在守卫面）。
        assert!(
            src.contains("ea_wire::EA_OK"),
            "EA 谓词面必须走 agent-sdk ea_wire::EA_OK 常量"
        );
        let t_pred0 = concat!("ea_status == ", "0");
        assert!(
            !src.contains(t_pred0),
            "ea_status 裸 0 字面判据回归（应走 ea_wire::EA_OK）"
        );
        // c) obs wire：ack 字节 / ERR 前缀手解回归。
        let t_ack = concat!("first() != ", "Some(&1)");
        assert!(
            !src.contains(t_ack),
            "ENQUEUE ack 手解回归（应走 parse_enqueue_resp）"
        );
        let t_err = concat!("starts_with", "(b\"ERR\")");
        assert!(
            !src.contains(t_err),
            "ERR 前缀手解回归（应走 parse_flush_resp）"
        );
        // d) chunked 端点：cap 寻址手工装配回归（唯一合法残留 = protocol_execute
        //    的 drv_call 客户端构造）。
        assert_eq!(
            src.matches(concat!("Sel4IpcClient", "::new")).count(),
            1,
            "Sel4IpcClient 手工建端必须仅存 protocol_execute 一处（chunked 面走 bind_shared）"
        );
        let t_wrap = concat!("wrap_shared", "(ipc");
        assert!(
            !src.contains(t_wrap),
            "wrap_shared 手工装配回归（应走 ChunkedSession::bind_shared）"
        );
    }

    // ==================== WP-64.2.5：Orchestration 状态机 ====================

    /// once 语义：触发一次后永假（原布尔散装语义逐字节——触发窗在旗立起
    /// 前为真，mark 后恒假；句柄缺席不立旗 = 重试语义保持）。
    #[test]
    fn once_flags_fire_exactly_once() {
        let mut once = OnceFlags::new();
        assert!(once.hang_due(true));
        assert!(!once.hang_due(false), "未到期不触发");
        once.mark_hang();
        assert!(!once.hang_due(true), "触发一次后永假");

        let mut once = OnceFlags::new();
        assert!(once.storage_due(true));
        once.mark_storage();
        assert!(!once.storage_due(true));

        let mut once = OnceFlags::new();
        assert!(once.ota_due(true));
        once.mark_ota();
        assert!(!once.ota_due(true));
    }

    /// readiness 状态迁移：未到期 None（继续收集）→ 信号 Ready / 超时
    /// Degraded；已判定的服务跳过（is_decided）；汇总仅在全判定后打印一次。
    #[test]
    fn readiness_tracker_transitions_and_summary_once() {
        let mut t = ReadinessTracker::new(0, 2);
        assert!(!t.is_decided(0));
        assert!(!t.all_decided());
        // 未到期 → None。
        assert_eq!(t.decide(0, 5_000, 100, false), None);
        // 信号 → Ready（登记）。
        assert_eq!(t.decide(0, 5_000, 200, true), Some(ReadinessVerdict::Ready));
        assert!(t.is_decided(0));
        // 已判定服务再 decide 不回退（原 is_some continue 语义）。
        assert_eq!(
            t.decide(0, 5_000, 9_999, true),
            Some(ReadinessVerdict::Ready)
        );
        // 超时 → Degraded。
        assert_eq!(
            t.decide(1, 5_000, 5_000, false),
            Some(ReadinessVerdict::Degraded)
        );
        assert!(t.all_decided());
        // 汇总：rows[1] 为 optional（降级不算 required degraded）——
        // 1 个 required Ready + 1 个 optional Degraded → 判据 true。
        let rows = [("req-svc", true), ("opt-svc", false)];
        assert!(t.summarize(&rows), "全判定后首次汇总必须打印");
        // 幂等：第二次 summarize 不再打印（旗已立）。
        assert!(!t.summarize(&rows), "汇总行一次性（旗进类型）");
    }

    /// Orchestration 构造：计数/epoch 初值（原循环局部初值语义）。
    #[test]
    fn orchestration_initial_state() {
        let o = Orchestration::new(4, 2, 1_000, 150);
        assert_eq!(o.tick, 0);
        assert_eq!(o.irq_count, 0);
        assert_eq!(o.collect_seq, 0);
        assert_eq!(o.chain_epoch, 0);
        assert_eq!(o.chain_rounds, 0);
        assert_eq!(o.obs_epoch, 0);
        assert_eq!(o.drv_irq_counts.len(), 2);
        assert!(!o.readiness.is_decided(0));
        assert_eq!(o.readiness.state.len(), 4);
        assert!(!o.once.hang_injected);
    }

    // ==================== WP-37.1 #8：readiness 判定 ====================

    /// 就绪信号已到 → Ready（首轮即判定）。
    #[test]
    fn test_check_readiness_signaled_ready() {
        assert_eq!(
            check_readiness(0, 5_000, 100, true),
            Some(ReadinessVerdict::Ready)
        );
        // 信号优先于 deadline（即使已超时）。
        assert_eq!(
            check_readiness(0, 5_000, 9_999, true),
            Some(ReadinessVerdict::Ready)
        );
    }

    /// deadline 内无信号 → None（继续收集，不提前降级）。
    #[test]
    fn test_check_readiness_pending_before_deadline() {
        assert_eq!(check_readiness(0, 5_000, 4_999, false), None);
        assert_eq!(check_readiness(1_000, 5_000, 5_999, false), None);
    }

    /// 超过 deadline 无信号 → Degraded（不虚报 Ready）。
    #[test]
    fn test_check_readiness_timeout_degrades() {
        assert_eq!(
            check_readiness(0, 5_000, 5_000, false),
            Some(ReadinessVerdict::Degraded)
        );
        assert_eq!(
            check_readiness(1_000, 5_000, 6_001, false),
            Some(ReadinessVerdict::Degraded)
        );
    }

    /// 异步收集全流程：先 None（未到期），信号后 Ready；无信号超时
    /// Degraded（drv 占位服务语义）。
    #[test]
    fn test_check_readiness_async_collection_flow() {
        // 参考服务：第二轮信号到达 → Ready。
        let mut v = check_readiness(0, 5_000, 500, false);
        assert_eq!(v, None);
        v = check_readiness(0, 5_000, 1_000, true);
        assert_eq!(v, Some(ReadinessVerdict::Ready));
        // 占位服务：永不信号 → 超时 Degraded。
        v = check_readiness(0, 5_000, 4_999, false);
        assert_eq!(v, None);
        v = check_readiness(0, 5_000, 5_000, false);
        assert_eq!(v, Some(ReadinessVerdict::Degraded));
    }
}
