//! svc-device-agent — 蓝图 40 WP-40.9：设备执行服务（EL0）。
//!
//! 收分块 ControlEnvelope（经 EIPC）→ 复验/认证/路由 → AuthorizedWriter
//! 执行 → 独立 readback → 结构化回执（OutcomeFrame）。registry 装配期
//! 注册（持久化加载见 persist API——存储句柄移交后接线）。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", feature(alloc_error_handler))]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
extern crate eneros_control_services;

/// bin 专属堆（2026-09-18 R10 三资源手术）：蓝图 70 R10 起 device-agent
/// 按**绑定注册表**每可控资产构造一条 `EnvelopeIngress`（各含
/// `Box<MetricsCollectorImpl>` = 276,040B）——三资源目录（asset 1/2/3）
/// 下 3 × 276 KiB ≈ 828 KiB + 设备注册表/服务环 ≈ 900 KiB，超 crate 级
/// `heap-large` 768 KiB（QEMU 实弹 `OOM: size=276040` 于第 3 条构造）。
/// 本 bin 用 1 MiB 专属堆（镜像 +256 KiB、root cnode +64 槽/16384——
/// 静态审计通过）；其余共享 768 KiB 堆的服务零变化。
/// 实现同源 `eneros_drv_svc::allocator`（linked_list_allocator + CAS
/// 单次 init）——仅堆区本 bin 私有。
#[cfg(target_os = "none")]
mod device_agent_heap {
    use core::alloc::{GlobalAlloc, Layout};
    use core::cell::UnsafeCell;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use linked_list_allocator::LockedHeap;

    const HEAP_SIZE: usize = 1024 * 1024;
    struct HeapSpace(UnsafeCell<[u8; HEAP_SIZE]>);
    // SAFETY: `_start` 单线程首调 init；此后 LockedHeap 自旋锁串行化。
    unsafe impl Sync for HeapSpace {}
    static HEAP_SPACE: HeapSpace = HeapSpace(UnsafeCell::new([0u8; HEAP_SIZE]));

    pub struct Allocator {
        heap: UnsafeCell<LockedHeap>,
        init: AtomicUsize,
    }
    #[allow(clippy::new_without_default)]
    impl Allocator {
        pub const fn new() -> Self {
            Self {
                heap: UnsafeCell::new(LockedHeap::empty()),
                init: AtomicUsize::new(0),
            }
        }
        /// # Safety
        /// 单线程启动阶段调用一次（首次分配前）。
        pub unsafe fn init(&self) {
            if self
                .init
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                // SAFETY: 静态 BSS；单线程首调；LockedHeap 经内部锁访问。
                unsafe {
                    (*self.heap.get())
                        .lock()
                        .init(HEAP_SPACE.0.get() as *mut u8, HEAP_SIZE);
                }
            }
        }
    }
    // SAFETY: LockedHeap 内部自旋锁 + 单初始化（CAS 防重入）。
    unsafe impl Sync for Allocator {}

    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if self.init.load(Ordering::Acquire) != 1 {
                return core::ptr::null_mut();
            }
            // SAFETY: init 已完成；LockedHeap 实现 GlobalAlloc（内部自旋锁）
            // ——与 drv-svc 分配器同款转发。
            unsafe { core::alloc::GlobalAlloc::alloc(&*self.heap.get(), layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if self.init.load(Ordering::Acquire) != 1 {
                return;
            }
            // SAFETY: 同 alloc。
            unsafe { core::alloc::GlobalAlloc::dealloc(&*self.heap.get(), ptr, layout) }
        }
    }
}

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: device_agent_heap::Allocator = device_agent_heap::Allocator::new();

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    eneros_runtime::println!("[svc-device-agent] _start entry ipc_buffer={ipc_buffer:x}");
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    eneros_runtime::println!("[svc-device-agent] allocator initialized");
    eneros_runtime::println!("[svc-device-agent] constructing handler");
    let mut handler = DeviceAgentHandler::new(eipc_mask);
    eneros_runtime::println!("[svc-device-agent] handler constructed");
    eneros_control_services::control_service_main(
        ep,
        "device-agent",
        ready_ntfn,
        eneros_drv_svc::now_ms,
        eneros_drv_svc::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
struct DeviceAgentHandler {
    /// WP-A.1：密钥不可得 ⇒ 空（拒载态：一切控制写 Denied，串口留判定行）。
    /// 蓝图 70 R10：**多收口点注册表**——每绑定一条 (device, point,
    /// EnvelopeIngress)（独立复验链，与签发侧路由表同构）；[0] = 默认
    /// 收口点（未注册目标的 Decode/Policy 拒绝语义归它，default-deny）。
    ingresses: alloc::vec::Vec<(u16, u32, eneros_device_agent::EnvelopeIngress)>,
    devices: eneros_device_agent::DeviceRegistry,
}

#[cfg(target_os = "none")]
impl DeviceAgentHandler {
    fn new(eipc_mask: u64) -> Self {
        // 装配（40 收尾 + 47 WP-47.2 + 蓝图 55）：产品配置与 svc-control-dispatch
        // 同源——两侧经同一 ControlProfile（dispatch_service::prod_profile）
        // 派生 key 之外的档案事实，不同源互相拒绝。
        // 写目标（设备/点）由 45 卷点表 Control 点绑定（svc-config）；
        // 不可得 → 0/0 fail-closed（default-deny）。
        use alloc::collections::BTreeMap;

        use eneros_control_services::dispatch_service::prod_profile;
        let now = eneros_drv_svc::now_ms();
        // 蓝图 67 WP-67.2.3（[01] §2.1a）：config 传输面 = ChunkedEndpoint
        // （ConfigEntrySource impl 已迁 endpoint；epoch 单点纪律）。
        let mut config_client = if eipc_mask & 1 != 0 {
            eneros_agent_sdk::endpoint::ChunkedEndpoint::bind_cap(
                "device-agent",
                eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            )
        } else {
            None
        };
        // 62 v0.315.0 WP-62.1.4（S7）：fetch_control_binding 收装配入口
        // 拉取的目录（本装配仍恰好 1 次 catalog 往返）。蓝图 70 R10：
        // 绑定注册表（每可控资产一条）——首绑定 = 默认收口点，其余绑定
        // 追加（每条独立复验链，与签发侧扩展路由同构同源）。
        let bindings: alloc::vec::Vec<(u64, u32, u32)> = match &mut config_client {
            Some(c) => eneros_control_services::catalog_gateway::fetch_catalog(c)
                .and_then(|cat| {
                    eneros_control_services::cfg_bindings::fetch_control_binding(&cat, c)
                })
                .unwrap_or_default(),
            None => {
                eneros_runtime::println!(
                    "[svc-device-agent] config EP NOT granted —— 控制绑定缺失（fail-closed）"
                );
                alloc::vec::Vec::new()
            }
        };
        let (device_id, point_id) = bindings.first().map(|&(_, d, p)| (d, p)).unwrap_or((0, 0));
        // SPEC WP-A.1：信封密钥经 svc-config `security` 节注入取得；
        // 全零/短键/缺节 ⇒ CHAIN-KEY-MISSING 拒载（None 态 default-deny）。
        let chain_key: Option<[u8; 32]> = match &mut config_client {
            Some(c) => eneros_control_services::cfg_bindings::fetch_control_chain_key(c),
            None => None,
        };
        let mut ingresses: alloc::vec::Vec<(u16, u32, eneros_device_agent::EnvelopeIngress)> =
            alloc::vec::Vec::new();
        match chain_key {
            Some(key) => {
                for &(asset, d, p) in &bindings {
                    let profile = prod_profile(d, p);
                    let i = eneros_device_agent::EnvelopeIngress::from_profile(&key, &profile, now);
                    eneros_runtime::println!(
                        "[svc-device-agent] ingress: constructed asset={asset} device={d} point={p} (lease_ok={})",
                        i.lease_ok()
                    );
                    ingresses.push((u16::try_from(d).unwrap_or(0), p, i));
                }
                if ingresses.is_empty() {
                    // 绑定注册表缺失 → 0/0 fail-closed 收口点（全部拒绝）。
                    let profile = prod_profile(0, 0);
                    let i = eneros_device_agent::EnvelopeIngress::from_profile(&key, &profile, now);
                    eneros_runtime::println!(
                        "[svc-device-agent] ingress: fail-closed 0/0 (lease_ok={})",
                        i.lease_ok()
                    );
                    ingresses.push((0, 0, i));
                }
            }
            None => {
                eneros_runtime::println!(
                    "[svc-device-agent] CHAIN-KEY-MISSING reason=unavailable —— 信封密钥不可得（缺节/解码失败/全零或短键），拒载（fail-closed）"
                );
            }
        }
        // 虚拟 PCS 设备（profile 声明 virtual-pcs 时装配；写/读回为真实
        // 寄存器语义——串口 VIRTUAL-PCS 行为外部观察证据）。
        let mut devices = eneros_device_agent::DeviceRegistry::new();
        let dev = eneros_device_agent::VirtualPcsDevice::new(
            eneros_device_agent::VIRTUAL_PCS_DEVICE_ID,
            eneros_drv_svc::now_ms,
        );
        let mut pm = BTreeMap::new();
        for (name, point) in eneros_device_agent::VIRTUAL_PCS_POINTS {
            pm.insert(alloc::string::String::from(*name), *point);
        }
        // WP-A.3：登记守卫（空点映射等退化输入）拒绝时 fail-closed——注册表
        // 无设备可写，后续控制写由路由层明确失败；串口判定行留证。
        match devices.register(
            "pcs",
            eneros_device_agent::DeviceType::Pcs,
            eneros_device_agent::VIRTUAL_PCS_DEVICE_ID,
            alloc::boxed::Box::new(dev),
            pm,
        ) {
            Ok(()) => {
                eneros_runtime::println!(
                    "[svc-device-agent] virtual-pcs registered (device=1 point=10)"
                );
            }
            Err(e) => {
                eneros_runtime::println!(
                    "[svc-device-agent] REGISTER-REJECTED pcs: {e}（fail-closed，注册表无设备）"
                );
            }
        }
        Self { ingresses, devices }
    }
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for DeviceAgentHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, now_ms: u64) -> bool {
        // 请求 = ControlEnvelope 字节 → 目标路由 → 收口点处理 →
        // OutcomeFrame。蓝图 70 R10：按信封目标 (device,point) 分派到
        // 对应收口点（验证仍全链执行）；未注册目标/解码失败 → 默认
        // 收口点（[0]）= 原 default-deny 语义；拒载态 ⇒ 一律 Policy
        // 拒绝帧。
        let target = eneros_device_agent::request_target(req);
        let idx = target
            .and_then(|(d, p)| {
                self.ingresses
                    .iter()
                    .position(|(rd, rp, _)| *rd == d && *rp == p)
            })
            .unwrap_or(0);
        let out = match self.ingresses.get_mut(idx) {
            Some((_, _, i)) => i.process(&mut self.devices, req, now_ms),
            None => eneros_device_agent::IngressOutcome::Denied {
                class: eneros_device_agent::DenyClass::Policy,
                reason: alloc::string::String::from("chain-key-missing (service refused)"),
            },
        };
        eneros_runtime::println!("[svc-device-agent] INGRESS: {:?}", out);
        *resp = eneros_device_agent::outcome_to_frame(&out)
            .encode()
            .to_vec();
        true
    }
}

eneros_runtime::svc_panic!("svc-device-agent");

#[cfg(target_os = "none")]
#[alloc_error_handler]
fn oom(layout: core::alloc::Layout) -> ! {
    eneros_runtime::println!(
        "[svc-device-agent] OOM: size={} align={}",
        layout.size(),
        layout.align()
    );
    loop {
        core::hint::spin_loop();
    }
}

/// host 构建 no-op 入口。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
