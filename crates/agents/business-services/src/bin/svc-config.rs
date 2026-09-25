//! svc-config — 签名配置服务 EL0（蓝图 41 WP-41.1/41.2）。
//!
//! 嵌入 xtask `config compile` 产出的签名 RuntimeConfigBundle（build.rs）；
//! 启动时验签装载；EIPC 查询面：GET_ENTRY / GET_DIGEST / GET_GENERATION。
//! bundle 缺失/验签失败 → 拒绝一切查询（fail-closed）。
//!
//! 70.6（WP-70.6.4）：盘上签名配置持久来源——启动读 `DataRegion::
//! ConfigActive`（CRC 信封；有效已签且代次 ≥ 嵌入束 → 替换 active）；
//! 换包事务写面：STAGE-PART(0x04)/STAGE-COMMIT(0x05)/ACTIVATE(0x06)/
//! SOURCE-INFO(0x07)。提交点 = ConfigActive 区段整段写成功；撕裂/坏签名
//! → fail-safe 保持嵌入束。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(clippy::empty_loop)]


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

/// 盘上 active 配置载体坐标（字节；ota-ab 区段图单源）。
#[cfg(target_os = "none")]
const CONFIG_ACTIVE_OFFSET: u64 = eneros_ota_ab::ab_partition::DataRegion::ConfigActive.base_byte();
#[cfg(target_os = "none")]
const CONFIG_REGION_BYTES: usize = eneros_ota_ab::ab_partition::CONFIG_REGION_BYTES as usize;

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start(
    ep: u64,
    ipc_buffer: u64,
    ready_ntfn: u64,
    eipc_mask: u64,
    heartbeat_slot: u64,
) -> ! {
    use eneros_durable_state::PersistIo;
    eneros_sel4_sys::safe::set_ipc_buffer(ipc_buffer);
    // SAFETY: `_start` 单线程首调（首次 alloc 前）。
    unsafe { ALLOC.init() };

    let mut service = eneros_business_services::ConfigService::new();
    // 生产路径：嵌入的签名 bundle + 验签公钥（无 → fail-closed）。
    match service.load_embedded(
        eneros_business_services::CONFIG_BUNDLE,
        eneros_business_services::CONFIG_BUNDLE_PUBKEY_HEX,
    ) {
        Ok(()) => {
            eneros_runtime::println!(
                "[svc-config] bundle loaded gen={} entries={}",
                service.generation(),
                service
                    .bundle
                    .as_ref()
                    .map(|b| b.entries.len())
                    .unwrap_or(0)
            );
        }
        Err(e) => {
            eneros_runtime::println!("[svc-config] FAIL-CLOSED no valid bundle: {e}");
        }
    }
    // 70.6（WP-70.6.4）：盘上 active 载荷裁决（授予边缺失 = 无持久源，
    // 维持嵌入基线——换包写面不可用，如实登记）。
    let mut storage = if eipc_mask & 1 != 0 {
        eneros_business_services::block_io::eipc_block_io(
            eneros_agent_sdk::EIPC_EP_BASE_SLOT,
            0, // epoch 未知——首个响应吸收
            5_000,
        )
    } else {
        None
    };
    match &mut storage {
        Some(io) => {
            // 第三形态可诊断化（REG-70.15-VOL6-R24）：进入读裁决前先落一行——
            // 若本行出现而后续四个分支词形全缺，即"读路径挂起"形态（此前
            // 整块无词形 = 不可诊断）。
            eneros_runtime::println!("[svc-config] CONFIG-DISK: probing active region");
            // 2026-09-18 R10 三资源手术：区段 8→12 KiB 后 12 KiB 栈帧 + 块读
            // 深路径超 svc-config 32 KiB 栈（静默溢出冻结——70.14 大缓冲出
            // 栈纪律同族）——改堆缓冲。
            let mut region = alloc::vec![0u8; CONFIG_REGION_BYTES as usize];
            let read = io.read(CONFIG_ACTIVE_OFFSET, &mut region);
            match read {
                Ok(()) => {
                    let (source, err) = service.load_disk_active(&region);
                    match (source, err) {
                        (eneros_business_services::config_service::ConfigSource::Disk, None) => {
                            eneros_runtime::println!(
                                "[svc-config] CONFIG-DISK active gen={} (persistent source)",
                                service.generation()
                            );
                        }
                        (_, Some(word)) => {
                            eneros_runtime::println!(
                                "[svc-config] CONFIG-DISK invalid ({word}) — keeping embedded gen={}",
                                service.generation()
                            );
                        }
                        (_, None) => {
                            eneros_runtime::println!(
                                "[svc-config] CONFIG-DISK empty — embedded gen={} baseline",
                                service.generation()
                            );
                        }
                    }
                }
                Err(e) => {
                    eneros_runtime::println!(
                        "[svc-config] CONFIG-DISK read failed ({e:?}) — keeping embedded"
                    );
                }
            }
        }
        None => {
            eneros_runtime::println!(
                "[svc-config] CONFIG-DISK storage NOT granted — embedded baseline; config swap unavailable"
            );
        }
    }

    let mut handler = ConfigHandler {
        service,
        storage,
        staging: eneros_business_services::config_service::ConfigStaging::new(),
    };
    eneros_control_services::control_service_main(
        ep,
        "config",
        ready_ntfn,
        eneros_business_services::now_ms,
        eneros_business_services::instance_epoch(),
        heartbeat_slot,
        &mut handler,
    )
}

#[cfg(target_os = "none")]
struct ConfigHandler {
    service: eneros_business_services::ConfigService,
    /// 70.6：svc-storage 块连接（ConfigActive 读写；None = 换包不可用）。
    storage: Option<
        eneros_business_services::block_io::BlockIo<eneros_agent_sdk::ipc_sel4::Sel4IpcClient>,
    >,
    /// 70.6：stage 分片缓冲（STAGE-PART 累积 → COMMIT/ACTIVATE 消费）。
    staging: eneros_business_services::config_service::ConfigStaging,
}

#[cfg(target_os = "none")]
impl ConfigHandler {
    /// 读 active 区段（IO 失败 → Err 词形）。
    fn read_active(&mut self, out: &mut [u8]) -> Result<(), ()> {
        use eneros_durable_state::PersistIo;
        let Some(io) = self.storage.as_mut() else {
            return Err(());
        };
        io.read(CONFIG_ACTIVE_OFFSET, out).map_err(|_| ())
    }

    /// 写 active 区段（整段信封；提交点）。调用方随后应用内存态。
    fn write_active(&mut self, envelope: &[u8]) -> Result<(), ()> {
        use eneros_durable_state::PersistIo;
        let Some(io) = self.storage.as_mut() else {
            return Err(());
        };
        io.write(CONFIG_ACTIVE_OFFSET, envelope).map_err(|_| ())?;
        io.flush().map_err(|_| ())?;
        // REG-70.15-VOL6-R24 写后读回确认（2026-09-15）：写+flush 后必须读回
        // 逐字节核对才算提交成功——读回不符 = 介质/路径不可信，拒绝进入
        // CONFIG-ACTIVATED（宁可本次激活失败，不把未核实状态留给冷启动）。
        let mut back = alloc::vec![0u8; envelope.len()];
        io.read(CONFIG_ACTIVE_OFFSET, &mut back).map_err(|_| ())?;
        if back.as_slice() != envelope {
            eneros_runtime::println!(
                "[svc-config] CONFIG-DISK readback MISMATCH ({}B) — refusing activation",
                back.len()
            );
            return Err(());
        }
        Ok(())
    }
}

#[cfg(target_os = "none")]
impl eneros_control_services::ControlServiceHandler for ConfigHandler {
    fn handle(&mut self, req: &[u8], resp: &mut alloc::vec::Vec<u8>, _now_ms: u64) -> bool {
        use eneros_business_services::config_service::*;
        // 协议：magic(4) ‖ cmd(1) ‖ arg。
        if eneros_business_services::svc_wire::parse_envelope(PROTO_MAGIC, req).is_none() {
            resp.extend_from_slice(eneros_business_services::svc_wire::ERR_PROTO);
            return false;
        }
        let cmd = req[4];
        let arg = &req[5..];
        // ===== 70.6 写面（stage/activate 事务；单一持久源 = ConfigActive）=====
        match cmd {
            CMD_STAGE_PART => {
                // arg = seq(1) ‖ total(1) ‖ bytes。
                if arg.len() < 2 {
                    resp.extend_from_slice(eneros_business_services::svc_wire::ERR_LEN);
                    return false;
                }
                if self.staging.received() == 0 && arg[0] != 1 {
                    // 新事务必须从 part 1 开始（旧残留缓冲显式重置）。
                    self.staging.reset();
                }
                match self.staging.push_part(arg[0], arg[1], &arg[2..]) {
                    Ok(n) => {
                        resp.push(n as u8);
                        true
                    }
                    Err(word) => {
                        resp.extend_from_slice(b"ERR:");
                        resp.extend_from_slice(word.as_bytes());
                        false
                    }
                }
            }
            CMD_STAGE_COMMIT => {
                // arg = 真实束长 u32 LE（可选——缺省按缓冲全长，旧客户端
                // 兼容；分片缓冲尾部零填充必须截断后才能通过 bundle 解码）。
                let Some(full) = self.staging.complete() else {
                    resp.extend_from_slice(b"ERR:staging-incomplete");
                    return false;
                };
                let staged: &[u8] = if arg.len() >= 4 {
                    let declared = u32::from_le_bytes(arg[..4].try_into().unwrap()) as usize;
                    if declared == 0 || declared > full.len() {
                        resp.extend_from_slice(b"ERR:bundle-size");
                        return false;
                    }
                    &full[..declared]
                } else {
                    full
                };
                match self.service.stage_commit(staged) {
                    Ok(digest) => {
                        resp.extend_from_slice(&digest.0);
                        true
                    }
                    Err(word) => {
                        resp.extend_from_slice(b"ERR:");
                        resp.extend_from_slice(word.as_bytes());
                        false
                    }
                }
            }
            CMD_ACTIVATE => {
                // arg 可选 = 真实束长 u32 LE（截断分片缓冲零填充）。
                // 快照拷出（write_active 需 &mut self——与 staging 借用互斥）。
                let full = self.staging.complete().map(<[u8]>::to_vec);
                let Some(full) = full else {
                    resp.extend_from_slice(b"ERR:staging-incomplete");
                    return false;
                };
                let staged: &[u8] = if arg.len() >= 4 {
                    let declared = u32::from_le_bytes(arg[..4].try_into().unwrap()) as usize;
                    if declared == 0 || declared > full.len() {
                        resp.extend_from_slice(b"ERR:bundle-size");
                        return false;
                    }
                    &full[..declared]
                } else {
                    &full
                };
                // 70.7 复查修复：**准入先于盘写**。旧顺序「写盘 → 激活」在
                // 激活被拒（坏签名/代次不前进/载荷不可解码）时已经把无效
                // 字节写进 ConfigActive 区段——下次启动 load_disk_active 拒绝
                // 该槽并静默回退嵌入基线 = 一次**失败**的激活把盘上配置
                // 回退掉。预检与 activate_staged 前三门同源、零副作用。
                if let Err(word) = self.service.validate_staged(staged) {
                    resp.extend_from_slice(b"ERR:");
                    resp.extend_from_slice(word.as_bytes());
                    return false;
                }
                // 提交点顺序：先盘后内存——盘写成功后崩溃，下次启动经
                // CONFIG-DISK 路径恢复新代次（内存应用失败 = 服务缺陷）。
                let Ok(envelope) = envelope_encode(&staged) else {
                    resp.extend_from_slice(b"ERR:bundle-size");
                    return false;
                };
                if let Err(()) = self.write_active(&envelope) {
                    resp.extend_from_slice(b"ERR:disk-write");
                    return false;
                }
                match self.service.activate_staged(&staged) {
                    Ok((gen, _digest)) => {
                        resp.extend_from_slice(&gen.to_le_bytes());
                        eneros_runtime::println!(
                            "[svc-config] CONFIG-ACTIVATED gen={gen} (disk committed + applied)"
                        );
                        self.staging.reset();
                        true
                    }
                    Err(word) => {
                        resp.extend_from_slice(b"ERR:");
                        resp.extend_from_slice(word.as_bytes());
                        false
                    }
                }
            }
            CMD_SOURCE_INFO => {
                let (is_disk, disk_gen, embedded_gen, active_gen) = self.service.source_info();
                resp.push(u8::from(is_disk));
                resp.extend_from_slice(&disk_gen.to_le_bytes());
                resp.extend_from_slice(&embedded_gen.to_le_bytes());
                resp.extend_from_slice(&active_gen.to_le_bytes());
                let mut region = alloc::vec![0u8; CONFIG_REGION_BYTES as usize];
                if self.read_active(&mut region).is_ok() {
                    if let Ok(Some(b)) = envelope_decode(&region) {
                        let d = eneros_crypto::sm3_hash(&b);
                        resp.extend_from_slice(&d);
                    }
                }
                true
            }
            _ => match self.service.handle_query(cmd, arg) {
                Ok(data) => {
                    resp.extend_from_slice(&(data.len() as u32).to_le_bytes());
                    resp.extend_from_slice(&data);
                    true
                }
                Err(e) => {
                    resp.extend_from_slice(b"ERR:");
                    resp.extend_from_slice(e.as_bytes());
                    false
                }
            },
        }
    }
}

eneros_business_services::svc_panic!("svc-config");

/// host 构建 no-op 入口（与 svc-drv-* 同模式）。
#[cfg(all(not(test), not(target_os = "none")))]
fn main() {}
