//! svc-ota — 系统 A/B OTA EL0（蓝图 41 WP-41.6；70.5 信任归真）。
//! EIPC 协议：magic "OTA1" ‖ cmd ‖ payload。
//! - 0x01 CHECK-DESC：描述 158B‖desc_sig 64B → verdict(1)（v2 签名描述）
//! - 0x02 DOWNLOAD-DESC：描述‖desc_sig‖image_sig‖chunk‖len‖image → step(1)
//! - 0x03 TRIAL：→ step(1)
//! - 0x04 COMMIT / 0x05 REGRESSION：→ step(1)
//! - 0x06 PERSIST-TRIAL：remaining(4)‖version(16) → gen(8)（trial 状态真
//!   落盘——经 EIPC 调 svc-storage 块命令面；存储未授予 → Unavailable）
//! - 0x07 LOAD-TRIAL：→ found(1)‖remaining(4)‖version(16)（重启恢复读回）
//! - 0x08 IDENTITY：capsule 136B → ack(1)（root 独占分发可信启动身份）
//!
//! 70.5（WP-70.5.1/70.5.2，方向 3 方案 B）：
//! - **信任锚与策略参数唯一来源 = 已验签配置束取数**（svc-ota→svc-config
//!   授予边，槽序 storage=bit0/config=bit1；OtaConsumer 解码 + 装配侧
//!   `validate_policy` 域检）——固定开发锚/固定策略静态字面**已退役**
//!   （源守卫 svc_ota_no_fixed_test_anchor 拦截回归）；
//! - **设备身份唯一来源 = root 分发的 IdentityCapsule**（0x08；可信启动
//!   路径投影，MGT1 不能构造）——bin 不再字面构造 "0.0.0"/"qemu"/secver；
//! - 惰性装配：身份与策略齐备才构造 `AbPartitionOta + OtaService`；
//!   缺一 → OTA 显式 not-ready（`ERR:ota-unavailable`），不回默认分支。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
extern crate alloc;
#[cfg(target_os = "none")]
use alloc::string::ToString;
#[cfg(target_os = "none")]
extern crate eneros_business_services;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOC: eneros_business_services::HeapAllocator =
    eneros_business_services::HeapAllocator::new();

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
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };
    // 41（WP-41.3 落盘）：存储句柄移交——EIPC 授予表 `svc-ota →
    // svc-storage`（槽序第 1 条 = EIPC_EP_BASE_SLOT+0，bit 0）。授予 →
    // 经块命令面真落盘（RegionStateStore<TrialState>）；未授予 → 显式
    // Unavailable（不伪造持久化）。
    let storage = if eipc_mask & 1 != 0 {
        eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0, // epoch 未知——首个响应吸收（svc-storage 实例 epoch）
            5_000,
        )
    } else {
        None
    };
    if storage.is_some() {
        eneros_runtime::println!(
            "[svc-ota] block storage GRANTED (EIPC slot {}) —— trial 状态真落盘",
            eneros_agent_sdk::EIPC_EP_BASE_SLOT
        );
    } else {
        eneros_runtime::println!(
            "[svc-ota] block storage NOT granted —— 持久化显式 Unavailable（不伪造）"
        );
    }
    // 70.5（WP-70.5.2）：config 授予边（表序第 2 条 = 槽 +1，bit 1）。
    // 未授予 → 策略取数不可达 → OTA 永久 not-ready（诚实降级）。
    let config_slot = eneros_agent_sdk::eipc_granted_slot("svc-ota", eipc_mask, "svc-config");
    match config_slot {
        Some(slot) => {
            eneros_runtime::println!(
                "[svc-ota] config GRANTED (EIPC slot {slot}) —— OTA 策略取数待自检"
            );
        }
        None => {
            eneros_runtime::println!(
                "[svc-ota] config NOT granted —— OTA 策略不可得，服务 not-ready（fail-closed）"
            );
        }
    }
    let mut handler = OtaHandler {
        storage,
        config_slot,
        config_endpoint: None,
        policy: None,
        identity: None,
        ota: None,
        fetch_failures: 0,
        install_armed: false,
    };
    eneros_control_services::control_service_main(
        ep,
        "ota",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

/// 数据盘持久化区域（字节偏移；避开首块引导区/GPT 保护区）。
#[cfg(target_os = "none")]
// 蓝图 70.2 WP-70.2.4：试启动状态区坐标单源 = DataRegion::TrialState
//（旧字面 8192 与意图日志区重叠——N01 P0 静态冲突，迁移见登记册）。
const OTA_PERSIST_OFFSET: u64 = eneros_ota_ab::ab_partition::DataRegion::TrialState.base_byte();
#[cfg(target_os = "none")]
const OTA_PERSIST_LEN: u64 = 128;

#[cfg(target_os = "none")]
type TargetOta = eneros_business_services::ota_service::OtaService<
    eneros_ota_ab::ab_partition::AbPartitionOta<
        eneros_agent_sdk::block_io::EipcBlockDevice<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
>;

#[cfg(target_os = "none")]
struct OtaHandler {
    storage: Option<
        eneros_business_services::block_io::BlockIo<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
    /// svc-config 授予槽（70.5；None = 未授予 → 策略永不可得）。
    config_slot: Option<u64>,
    /// svc-config 分块端点（惰性绑定——服务就绪前调用会失败，重试有界）。
    config_endpoint: Option<
        eneros_agent_sdk::endpoint::ChunkedEndpoint<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
    /// 已验签策略（配置束取数 + validate_policy 域检后入册）。
    policy: Option<eneros_business_services::ota_service::OtaPolicy>,
    /// root 分发的可信身份快照（0x08；单调水位）。
    identity: Option<eneros_business_services::ota_service::DeviceIdentity>,
    ota: Option<TargetOta>,
    /// 配置取数失败计数（观测面；驱动重试有界性 = 每条 OTA1 命令至多
    /// 一次取数尝试——探针流量即上界）。
    fetch_failures: u64,
    /// 70.7（WP-70.7.2）：安装武装旗——CHECK-DESC 判定=1（70.5 验签链
    /// 通过）后置位；BOOT 安装命令（0x0A-0x0C）要求已武装。
    install_armed: bool,
}

#[cfg(target_os = "none")]
impl OtaHandler {
    /// 打开 trial 状态存储（RegionIo 从介质读回；fail-closed：任何 IO
    /// 错误 → None → 显式 Unavailable）。
    fn trial_store(
        &mut self,
    ) -> Option<
        eneros_durable_state::RegionStateStore<
            eneros_business_services::ota_service::TrialState,
            &mut eneros_business_services::block_io::BlockIo<
                eneros_agent_sdk::ipc_sel4::Sel4IpcClient,
            >,
        >,
    > {
        let io = self.storage.as_mut()?;
        let mut store =
            eneros_durable_state::RegionStateStore::new(io, OTA_PERSIST_OFFSET, OTA_PERSIST_LEN);
        store.open().ok()?;
        Some(store)
    }

    /// 配置取数（每条 OTA1 命令至多一次；成功后不再取）。
    fn fetch_config(&mut self) -> bool {
        if self.policy.is_some() {
            return true;
        }
        let Some(slot) = self.config_slot else {
            return false;
        };
        if self.config_endpoint.is_none() {
            self.config_endpoint =
                eneros_agent_sdk::endpoint::ChunkedEndpoint::bind("svc-ota", "svc-config");
        }
        let Some(endpoint) = self.config_endpoint.as_mut() else {
            self.fetch_failures += 1;
            return false;
        };
        // 取数两步：entry payload（OtaConsumer 解码）+ bundle generation
        // （策略代次锚——描述 policy_generation 判据 + 轮换预备记账）。
        let raw = match eneros_agent_sdk::config_client::get_entry(endpoint, "ota") {
            Ok(raw) => raw,
            Err(_) => {
                self.fetch_failures += 1;
                return false;
            }
        };
        let generation = match eneros_agent_sdk::config_client::get_generation(endpoint) {
            Ok(g) => g,
            Err(_) => {
                self.fetch_failures += 1;
                return false;
            }
        };
        let body = match eneros_agent_sdk::config_client::decode_response_body(&raw) {
            Ok(body) => body,
            Err(_) => {
                self.fetch_failures += 1;
                return false;
            }
        };
        let decoded = {
            use eneros_config_bundle::TypedConfigConsumer;
            let consumer = eneros_business_services::config_service::OtaConsumer::default();
            consumer.decode_validate(body)
        };
        let cfg = match decoded {
            Ok(c) => c,
            Err(_) => {
                self.fetch_failures += 1;
                return false;
            }
        };
        let policy = eneros_business_services::ota_service::OtaPolicy {
            partition_blocks: cfg.partition_blocks,
            chunk_size: cfg.chunk_size,
            max_boot_attempts: cfg.max_boot_attempts,
            root_key: cfg.root_key,
            signature_mandatory: true,
            generation,
        };
        if eneros_business_services::ota_service::validate_policy(&policy).is_err() {
            self.fetch_failures += 1;
            eneros_runtime::println!("[svc-ota] OTA-CONFIG invalid —— 策略域检拒绝（not-ready）");
            return false;
        }
        // 锚指纹（key_id 轮换预备口径）= SM3(root_key) 前 8B hex——只入
        // 日志，key 字节与原始配置不落任何词形。
        let fp = eneros_crypto::sm3_hash(&policy.root_key);
        eneros_runtime::println!(
            "[svc-ota] OTA-CONFIG gen={:?} partition_blocks={} chunk={} attempts={} anchor_fp={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            generation,
            policy.partition_blocks,
            policy.chunk_size,
            policy.max_boot_attempts,
            fp[0], fp[1], fp[2], fp[3], fp[4], fp[5], fp[6], fp[7]
        );
        self.policy = Some(policy);
        true
    }

    /// 惰性装配（身份 + 策略齐备才构造；装配点 = 0x08 后与每条 0x01/0x02
    /// 命令前——探针流量即重试上界）。A/B 镜像面持独立 storage 连接
    /// （41 卷装配语义：trial 面与镜像面各一连接，互不串扰）。
    fn ensure_ready(&mut self, now_ms: u64) {
        if self.ota.is_some() {
            return;
        }
        if !self.fetch_config() {
            return;
        }
        let (Some(policy), Some(identity)) = (self.policy.clone(), self.identity.clone()) else {
            return;
        };
        let Some(io) = eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0, // epoch 未知——首个响应吸收（svc-storage 实例 epoch）
            5_000,
        ) else {
            eneros_runtime::println!("[svc-ota] storage 连接不可得 —— A/B not-ready");
            return;
        };
        // 69.5（WP-69.5.2）：生产块大小单源 = ota-ab OTA_PROD_BLOCK_SIZE
        // （512）；盘上块号语义按 512B 块绑定（DATA_DISK_BLOCKS 同源）。
        let dev = eneros_agent_sdk::block_io::EipcBlockDevice::new(
            io,
            eneros_ota_ab::ab_partition::OTA_PROD_BLOCK_SIZE,
            eneros_ota_ab::ab_partition::DATA_DISK_BLOCKS,
        );
        let mut key65 = [0u8; 65];
        key65[0] = 0x04;
        key65[1..].copy_from_slice(&policy.root_key);
        let Ok(pk) = eneros_crypto::Sm2PublicKey::from_bytes(&key65) else {
            // fetch_config 域检已验过锚；此处防御性双检（不可达即退）。
            eneros_runtime::println!("[svc-ota] anchor invalid —— A/B not-ready");
            return;
        };
        let mgr = eneros_ota_ab::ab_partition::AbPartitionOta::new(dev, pk, now_ms);
        self.ota = Some(TargetOta::new(mgr, policy, &identity));
        eneros_runtime::println!("[svc-ota] OTA READY (config + identity assembled)");
    }
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for OtaHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        use eneros_business_services::ota_service::*;
        use eneros_durable_state::DurableStateStore;
        const MAGIC: &[u8; 4] = b"OTA1";
        if req.len() < 5 || &req[..4] != MAGIC {
            resp.extend_from_slice(b"ERR:proto");
            return false;
        }
        // 0x08 IDENTITY（root 独占分发）：capsule 校验 → 身份入册（单调
        // 水位）→ 尝试装配。
        if req[4] == 0x08 {
            let capsule_bytes = match eneros_agent_sdk::ota_wire::parse_identity_req(req) {
                Ok(c) => c,
                Err(_) => {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
            };
            let capsule = match eneros_boot_protocol::IdentityCapsule::decode(capsule_bytes) {
                Ok(c) => c,
                Err(e) => {
                    eneros_runtime::println!("[svc-ota] OTA-IDENTITY rejected ({e:?})");
                    resp.extend_from_slice(b"ERR:identity");
                    return false;
                }
            };
            let identity = match identity_from_capsule(&capsule) {
                Ok(i) => i,
                Err(e) => {
                    eneros_runtime::println!("[svc-ota] OTA-IDENTITY rejected ({e})");
                    resp.extend_from_slice(b"ERR:identity");
                    return false;
                }
            };
            // 单调水位：回退 capsule 拒绝（WP-70.5.4 反测面之一）。
            if let Some(prev) = &self.identity {
                if identity.security_floor < prev.security_floor {
                    eneros_runtime::println!(
                        "[svc-ota] OTA-IDENTITY stale floor {}<{} —— 拒绝",
                        identity.security_floor,
                        prev.security_floor
                    );
                    resp.extend_from_slice(b"ERR:identity-stale");
                    return false;
                }
            }
            // 已装配服务：二次身份只允许水位单调推进并**传播到在用准入**
            // （`set_security_floor` 唯一生产调用点——WP-70.5.4 传播面；
            // 70.5 复查：原实现只替换快照、装配冻结旧水位，"接受回 1"与
            // "按旧水位准入"并存）；sku/版本/secver 变化 = 与在用装配分叉，
            // 拒绝而非接受不生效。
            if let (Some(ota), Some(prev)) = (self.ota.as_mut(), &self.identity) {
                if identity.sku != prev.sku
                    || identity.current_version != prev.current_version
                    || identity.security_version != prev.security_version
                {
                    eneros_runtime::println!(
                        "[svc-ota] OTA-IDENTITY conflicts with assembled identity —— 拒绝"
                    );
                    resp.extend_from_slice(b"ERR:identity-conflict");
                    return false;
                }
                if let Err(e) = ota.set_security_floor(identity.security_floor) {
                    // 上方单调检查已拦回退——不可达分支，诚实不吞。
                    eneros_runtime::println!(
                        "[svc-ota] OTA-IDENTITY floor propagate failed ({e}) —— 拒绝"
                    );
                    resp.extend_from_slice(b"ERR:identity-stale");
                    return false;
                }
            }
            eneros_runtime::println!(
                "[svc-ota] OTA-IDENTITY slot={} gen={} secver={} floor={} sku={} ver={}",
                capsule.selected_slot,
                capsule.bootctrl_generation,
                identity.security_version,
                identity.security_floor,
                identity.sku,
                identity.current_version
            );
            self.identity = Some(identity);
            self.ensure_ready(_now_ms);
            if self.ota.is_some() {
                resp.push(1);
            } else {
                // 身份已收但策略未得——诚实 not-ready（root 侧只读分发，
                // 不重试）。
                resp.extend_from_slice(b"ERR:not-ready");
                return false;
            }
            return true;
        }
        // 0x09 POLICY-INFO（只读观测）：策略代次 + 锚指纹（零密钥出域）。
        // 未就绪 → ERR:not-ready（诚实不可得，不伪造零值）。
        if req[4] == 0x09 {
            if req.len() != 5 {
                resp.extend_from_slice(b"ERR:len");
                return false;
            }
            self.ensure_ready(_now_ms);
            let Some(policy) = &self.policy else {
                resp.extend_from_slice(b"ERR:not-ready");
                return false;
            };
            resp.extend_from_slice(&policy.generation.to_le_bytes());
            let fp = eneros_crypto::sm3_hash(&policy.root_key);
            resp.extend_from_slice(&fp[..8]);
            return true;
        }
        // 0x01-0x05 由可测的纯逻辑函数处理（ota_service::handle_ota_ab_command）。
        if (0x01..=0x05).contains(&req[4]) {
            self.ensure_ready(_now_ms);
            let ok = handle_ota_ab_command(self.ota.as_mut(), req, resp);
            // 70.7：CHECK-DESC 判定=1（验签链通过）→ 安装武装。
            if req[4] == 0x01 && resp.first() == Some(&1) {
                self.install_armed = true;
            }
            return ok;
        }
        match req[4] {
            // 41：trial 状态真落盘（RegionStateStore CAS；首次写入
            // expected=None → gen 1；重启后 LOAD 从介质读回）。
            0x06 => {
                // 蓝图 58：请求解析走 ota_wire 单源（与 root 探针同型）。
                let (remaining, version) =
                    match eneros_agent_sdk::ota_wire::parse_persist_trial_req(req) {
                        Ok(v) => v,
                        Err(_) => {
                            resp.extend_from_slice(b"ERR:len");
                            return false;
                        }
                    };
                let state = TrialState::new(remaining, &version);
                let mut store = match self.trial_store() {
                    Some(s) => s,
                    None => {
                        resp.extend_from_slice(b"ERR:store-unavailable");
                        return false;
                    }
                };
                // CAS：期望当前代次（load 已知）——不存在则 None（首次）。
                let expected = match store.load() {
                    Ok(Some(v)) => Some(v.generation),
                    _ => None,
                };
                match store.compare_and_swap(expected, &state) {
                    Ok(gen) => {
                        resp.extend_from_slice(&gen.0.to_le_bytes());
                        true
                    }
                    Err(e) => {
                        resp.extend_from_slice(b"ERR:persist:");
                        resp.extend_from_slice(e.to_string().as_bytes());
                        false
                    }
                }
            }
            0x07 => {
                let mut store = match self.trial_store() {
                    Some(s) => s,
                    None => {
                        resp.extend_from_slice(b"ERR:store-unavailable");
                        return false;
                    }
                };
                match store.load() {
                    Ok(Some(v)) => {
                        resp.push(1); // found
                        resp.extend_from_slice(&v.state.remaining.to_le_bytes());
                        let vb = v.state.trial_version.as_bytes();
                        let mut name = [0u8; 16];
                        let n = core::cmp::min(vb.len(), 16);
                        name[..n].copy_from_slice(&vb[..n]);
                        resp.extend_from_slice(&name);
                        true
                    }
                    Ok(None) => {
                        resp.push(0); // not found
                        true
                    }
                    Err(e) => {
                        resp.extend_from_slice(b"ERR:load:");
                        resp.extend_from_slice(e.to_string().as_bytes());
                        false
                    }
                }
            }
            _ => {
                // 70.7（WP-70.7.2/3/5）：BOOT 安装事务（0x0A 守卫/0x0B
                // trial/0x0C 确认/0x0D 取消 trial/0x0E 状态投影/0x0F 候选
                // 身份读取；root 为唯一调用方——无 (*, svc-ota) 授予边）
                // ——要求 CHECK-DESC 验签链先行武装（0x0E/0x0F 只读除外）。
                if (0x0A..=0x0F).contains(&req[4]) {
                    // 只读面（0x0E 状态投影 / 0x0F 候选身份读）无安装面写权，
                    // 不要求武装门。
                    if req[4] != 0x0E && req[4] != 0x0F && !self.install_armed {
                        resp.extend_from_slice(b"ERR:not-armed");
                        return false;
                    }
                    let Some(io) = self.storage.as_mut() else {
                        resp.extend_from_slice(b"ERR:store-unavailable");
                        return false;
                    };
                    return handle_boot_install(req[4], req, resp, io);
                }
                resp.extend_from_slice(b"ERR:cmd");
                false
            }
        }
    }
}

/// 70.7（WP-70.7.3）：BOOT 安装事务命令（0x0A 守卫查询 / 0x0B trial
/// 事务 / 0x0C 启动确认；均要求 CHECK-DESC 验签链先行武装）。
#[cfg(target_os = "none")]
fn handle_boot_install(
    cmd: u8,
    req: &[u8],
    resp: &mut alloc::vec::Vec<u8>,
    io: &mut eneros_business_services::block_io::BlockIo<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
) -> bool {
    use eneros_agent_sdk::block_io::{
        BLK_BOOT_CANCEL, BLK_BOOT_CONFIRM, BLK_BOOT_FLUSH, BLK_BOOT_GUARD, BLK_BOOT_INSTALL_WRITE,
        BLK_BOOT_READ, BLK_BOOT_STATUS, BLK_BOOT_TRIAL,
    };
    let _ = (BLK_BOOT_INSTALL_WRITE, BLK_BOOT_FLUSH);
    // 70.7.5（事务下沉）：事务臂 = **单次** guard 式 IPC 到 svc-storage
    // 新子码（BLK_BOOT_TRIAL/CONFIRM/CANCEL/STATUS）——bootctrl 记录零
    // IPC 搬运（run16-22 实弹：多调用拆分模式稳定挂死于第二次 seL4 Call）。
    match cmd {
        0x0A => {
            let p = alloc::vec::Vec::new();
            match io.raw_command(BLK_BOOT_GUARD, &p) {
                Ok(r) if r.len() == 40 => {
                    // 73 批A（C5）：解包单源 = InstallRanges::unpack_le
                    //（drv-svc 侧 pack_le 的对偶；手工偏移解包退役）。
                    match eneros_drv_svc::boot_guard::InstallRanges::unpack_le(&r) {
                        Some(ranges) => {
                            eneros_runtime::println!(
                                "[svc-ota] BOOT-GUARD: inactive_lba={} inactive_lbas={}",
                                ranges.inactive_start_lba,
                                ranges.inactive_lbas
                            );
                            resp.extend_from_slice(&r);
                            true
                        }
                        // 不可达（len==40 已由臂卫判定）；对称拒绝，不 panic。
                        None => {
                            resp.extend_from_slice(b"ERR:guard-len");
                            false
                        }
                    }
                }
                Ok(_) => {
                    resp.extend_from_slice(b"ERR:guard-len");
                    false
                }
                Err(_) => {
                    eneros_runtime::println!("[svc-ota] BOOT-GUARD: io error —— 安装面异常");
                    resp.extend_from_slice(b"ERR:guard-io");
                    false
                }
            }
        }
        0x0B => {
            eneros_runtime::println!("[svc-ota] BOOT-TRIAL: begin len={}", req.len());
            if req.len() < 5 + 8 + 16 {
                resp.extend_from_slice(b"ERR:len");
                return false;
            }
            // 单次中继：svc-storage 侧事务（bootctrl 零 IPC 搬运）。
            // 载荷 = **body only**（子码由 `BlockIo::cmd` 前置；构造器
            // `boot_body::trial` 单源——旧手拼实现多带 1B 子码 → 全体
            // 右移 → 记录内 secver/image_id 变噪声，REG-70.7-OTA1-
            // DOUBLE-SUB run56 实弹）。
            let mut secver_bytes = [0u8; 8];
            secver_bytes.copy_from_slice(&req[5..13]);
            let mut image_id = [0u8; 16];
            image_id.copy_from_slice(&req[13..29]);
            let p = eneros_agent_sdk::block_io::boot_body::trial(
                u64::from_le_bytes(secver_bytes),
                &image_id,
            );
            // 70.14（REG-70.12-PROBE-BOOT-TRIAL-STALL 定位插桩）：trial 分支
            // 曾"进得去、出不来"且无任何失败词形（`drv_call` 的 timeout 只在
            // `seL4_Call` **返回后**才检查 ⇒ 阻塞在 Call 上时超时形同不存在）。
            // 两行日志把"storage 未回"与"ota 未收"分开：
            //   `TX-SENT` 之后无 `RX-ERR/RX-OK` ⇒ 卡在 Call（收方未回）
            //   `TX-SENT` 出现且 `RX-*` 出现 ⇒ 回程问题（不再是本缺陷）
            // 判据位置：插桩行只在串口证据里，不改任何 wire/判定语义。
            //
            // 70.14（第六轮）：另读 boot 面写探针计数——它由 **svc-storage**
            // 侧在"进入驱动调用前"自增。同一窗口内比较同一时刻两份证据：
            //   trials 计数递增而串口无 `boot-write inner pre` ⇒ console 路径阻塞
            //   计数不递增 ⇒ 阻塞在调用边界（打印之前）
            // 实测打印（供下轮对照）：`BOOT-TRIAL-PROBE pre={}`。
            #[cfg(target_os = "none")]
            eneros_runtime::println!(
                "[svc-ota] BOOT-TRIAL-PROBE pre_count={} drv_enter_count={}",
                eneros_drv_svc::block_cmd::BOOT_WRITE_PRE_PROBES
                    .load(core::sync::atomic::Ordering::Relaxed),
                eneros_drv_svc::block_cmd::BLK_WRITE_ENTER_PROBES
                    .load(core::sync::atomic::Ordering::Relaxed)
            );
            let t0 = eneros_drv_svc::now_ms();
            eneros_runtime::println!(
                "[svc-ota] BOOT-TRIAL: TX-SENT slot_req=0x{:02x} t={} body={}",
                BLK_BOOT_TRIAL,
                t0,
                p.len()
            );
            match io.raw_command(BLK_BOOT_TRIAL, &p) {
                // 退出插桩（成功/失败两臂各一行；`now_ms` 与 `BOOT-STATUS`
                // 同源单调计数，用于给出"实际阻塞时长"）。
                Ok(v) => {
                    let dt = eneros_drv_svc::now_ms().saturating_sub(t0);
                    eneros_runtime::println!(
                        "[svc-ota] BOOT-TRIAL: RX-OK resp={} blocked_ms={}",
                        v.len(),
                        dt
                    );
                    if v.len() == 8 {
                        let seq = u64::from_le_bytes(v[..8].try_into().unwrap_or([0u8; 8]));
                        eneros_runtime::println!(
                            "[svc-ota] BOOT-TRIAL: tx committed seq={seq} target_secver={}",
                            u64::from_le_bytes(secver_bytes)
                        );
                        resp.extend_from_slice(&seq.to_le_bytes());
                        return true;
                    }
                    eneros_runtime::println!("[svc-ota] BOOT-TRIAL: rejected (len)");
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
                Err(e) => {
                    let dt = eneros_drv_svc::now_ms().saturating_sub(t0);
                    eneros_runtime::println!(
                        "[svc-ota] BOOT-TRIAL: RX-ERR {e:?} blocked_ms={}",
                        dt
                    );
                    eneros_runtime::println!("[svc-ota] BOOT-TRIAL: rejected (io)");
                    resp.extend_from_slice(b"ERR:trial-io");
                    return false;
                }
            }
        }
        0x0C => {
            if req.len() < 5 + 1 + 8 + 16 {
                resp.extend_from_slice(b"ERR:len");
                return false;
            }
            let slot_is_a = req[5] == 1;
            // body only（构造器单源；见 0x0B 注）。
            let mut secver_bytes = [0u8; 8];
            secver_bytes.copy_from_slice(&req[6..14]);
            let mut image_id = [0u8; 16];
            image_id.copy_from_slice(&req[14..30]);
            let p = eneros_agent_sdk::block_io::boot_body::confirm(
                slot_is_a,
                u64::from_le_bytes(secver_bytes),
                &image_id,
            );
            match io.raw_command(BLK_BOOT_CONFIRM, &p) {
                Ok(r) if r.len() == 8 => {
                    let seq = u64::from_le_bytes(r[..8].try_into().unwrap());
                    eneros_runtime::println!(
                        "[svc-ota] BOOT-CONFIRM: slot={} secver={} seq={seq}",
                        if slot_is_a { "A" } else { "B" },
                        u64::from_le_bytes(req[6..14].try_into().unwrap())
                    );
                    resp.extend_from_slice(&seq.to_le_bytes());
                    true
                }
                Ok(_) => {
                    eneros_runtime::println!("[svc-ota] BOOT-CONFIRM: rejected (len)");
                    resp.extend_from_slice(b"ERR:len");
                    false
                }
                Err(_) => {
                    eneros_runtime::println!("[svc-ota] BOOT-CONFIRM: rejected (io)");
                    resp.extend_from_slice(b"ERR:confirm-io");
                    false
                }
            }
        }
        // 70.7.5：取消未确认 trial（mgmt update-rollback 后端）——单次
        // BLK_BOOT_CANCEL 中继。floor 不动（未确认 = 无不可逆提交）。
        0x0D => match io.raw_command(BLK_BOOT_CANCEL, &[]) {
            Ok(r) if r.len() == 8 => {
                let seq = u64::from_le_bytes(r[..8].try_into().unwrap());
                eneros_runtime::println!("[svc-ota] BOOT-CANCEL: trial canceled seq={seq}");
                resp.extend_from_slice(&seq.to_le_bytes());
                true
            }
            Ok(_) => {
                eneros_runtime::println!("[svc-ota] BOOT-CANCEL: rejected (len)");
                resp.extend_from_slice(b"ERR:len");
                false
            }
            Err(_) => {
                eneros_runtime::println!("[svc-ota] BOOT-CANCEL: rejected (io)");
                resp.extend_from_slice(b"ERR:cancel-io");
                false
            }
        },
        // 状态投影（只读）：单次 BLK_BOOT_STATUS 中继。
        0x0E => match io.raw_command(BLK_BOOT_STATUS, &[]) {
            Ok(r) => {
                let line = core::str::from_utf8(&r).unwrap_or("?");
                eneros_runtime::println!("[svc-ota] BOOT-STATUS: {line}");
                resp.extend_from_slice(r.as_slice());
                true
            }
            Err(_) => {
                eneros_runtime::println!("[svc-ota] BOOT-STATUS: rejected (io)");
                resp.extend_from_slice(b"ERR:status-io");
                false
            }
        },
        // 候选载荷身份读取（只读，同 0x0E 豁免武装门）：守卫取非活动槽基址
        // → BLK_BOOT_READ 读候选槽首部 manifest → 解 (secver, release_id)。
        //
        // REG-70.7-BOOT-TRIAL-BIND（run55 实弹）：试启动事务记录的目标
        // 身份必须与候选载荷 manifest **逐字段一致**——boot-manager 选槽
        // 以 `expected_security_version/expected_release_id` 绑定校验
        //（不符 = SECURITY_BINDING/ImageId 拒绝 + 回退旧槽）。生产来源 =
        // 已验签下行包 manifest（REG-70.6-MGMT-DOWNLINK Open）；门内下行
        // 缺席 → 探针经本读面取候选槽载荷**自身**身份供给 trial——记录值
        // 仍走 wire，绑定判据不放松（槽内容事后被改 = 记录与载荷不再一致
        // → 校验失败 fail-closed）。
        0x0F => {
            let guard = match io.raw_command(BLK_BOOT_GUARD, &[]) {
                Ok(r) if r.len() == 40 => r,
                Ok(_) => {
                    resp.extend_from_slice(b"ERR:guard-len");
                    return false;
                }
                Err(_) => {
                    eneros_runtime::println!("[svc-ota] BOOT-CANDIDATE: guard io error");
                    resp.extend_from_slice(b"ERR:guard-io");
                    return false;
                }
            };
            // 73 批A（C5）：位置消费 guard[..8] 收编为 unpack_le 字段访问。
            let inactive_lba = match eneros_drv_svc::boot_guard::InstallRanges::unpack_le(&guard) {
                Some(ranges) => ranges.inactive_start_lba,
                // 不可达（len==40 已由臂卫判定）；对称拒绝，不 panic。
                None => {
                    resp.extend_from_slice(b"ERR:guard-len");
                    return false;
                }
            };
            // body only（构造器单源；见 0x0B 注）。
            let p = eneros_agent_sdk::block_io::boot_body::read(
                inactive_lba,
                eneros_agent_sdk::block_io::BLK_READ_CHUNK as u32,
            );
            match io.raw_command(BLK_BOOT_READ, &p) {
                Ok(d) => match eneros_business_services::ota_service::candidate_identity(&d) {
                    Ok((secver, release_id)) => {
                        eneros_runtime::println!(
                            "[svc-ota] BOOT-CANDIDATE: secver={secver} release_id={:02x}{:02x}{:02x}{:02x}…",
                            release_id[0],
                            release_id[1],
                            release_id[2],
                            release_id[3]
                        );
                        resp.extend_from_slice(&secver.to_le_bytes());
                        resp.extend_from_slice(&release_id);
                        true
                    }
                    Err(e) => {
                        eneros_runtime::println!("[svc-ota] BOOT-CANDIDATE: rejected ({e})");
                        resp.extend_from_slice(b"ERR:candidate");
                        false
                    }
                },
                Err(_) => {
                    eneros_runtime::println!("[svc-ota] BOOT-CANDIDATE: read io error");
                    resp.extend_from_slice(b"ERR:read-io");
                    false
                }
            }
        }
        _ => {
            resp.extend_from_slice(b"ERR:cmd");
            false
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // REG-70.7 排查：静默黑洞有声化（E 卷教训——无声 spin 掩盖死因）。
    eneros_runtime::println!("[svc-ota] PANIC: {}", info);
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
