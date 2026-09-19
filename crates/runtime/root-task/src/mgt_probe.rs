//! 70.6（WP-70.6.5）：MGT1 管理动作链 root 侧探针（qemu-test-synth 测试
//! 装配专属——release 闭包零此代码，closure gate 强制）。
//!
//! 背景：外部 station → MQTT → svc-mqtt → svc-mgmt 的下行投递在现装配
//! 存在待查缺陷（登记 REG-70.6-MGMT-DOWNLINK），本探针改由 **root 直接
//! Send MGT1 v2 帧到 svc-mgmt**（与 fault 注入同 Send 语义——无 reply，
//! 应答帧被 seL4 丢弃），驱动真实生产处理链（MGT1 v2 验签/RBAC/操作
//! 账本/执行器/svc-config 事务/root MGT2），动作实效以串口词形取证。
//!
//! v2 签名：测试 station 密钥对由 gate 生成（私钥不出 gate 运行目录），
//! token/签名经 build.rs env 注入为常量（[`PROBE_TOKEN`]/[`PROBE_SIG`]，
//! 生成的数据文件在 mgt_probe_data.rs）。token 为固定 64B（签名只覆盖
//! token——request_id/action/arg 不在签名域，70.5 复查已订正该合同），
//! 故同一 (token, sig) 对可用于全部探针帧。
//!
//! 状态机（每 K tick 推进一步——不阻塞根环；svc-mgmt 的 root MGT2 回调
//! 依赖根循环轮询，探针自身永不阻塞等待 svc-mgmt）。

#![allow(dead_code)]

extern crate alloc;

/// build.rs 生成的测试数据（token/签名字面数组 + 候选束 include；release
/// /host 构建为空数组——探针代码经 cfg(feature) 隔离）。PROBE_TOKEN 为
/// 离线核对锚（gate 侧可独立重算），运行时仅消费 PROBE_SIG。
#[allow(dead_code)]
pub mod mgt_probe_data {
    include!(concat!(env!("OUT_DIR"), "/mgt_probe_data.rs"));
}

/// 探针帧节奏（tick 间隔；qemu-test 主循环 ~ms 级，步间留处理窗）。
pub const PROBE_STEP_TICKS: u64 = 12;
/// 首步触发 tick（readiness 稳定后）。
pub const PROBE_START_TICK: u64 = 90;
/// 重启完成等待窗（tick 数上限；超时继续推进并打印超时词形）。
pub const RESTART_WAIT_TICKS: u64 = 600;

/// 固定 token 域常量（与 gate 段0 签发脚本逐字节同构）。
pub const PROBE_PRINCIPAL: &[u8; 16] = b"station-ops\0\0\0\0\0";
pub const PROBE_ROLES: [u8; 4] = [2, 0, 0, 0]; // admin
pub const PROBE_TENANT: [u8; 4] = [1, 0, 0, 0];
pub const PROBE_CHANNEL: &[u8; 16] = b"probe\0\0\0\0\0\0\0\0\0\0\0";
pub const PROBE_EXPIRY: u64 = 4_102_444_800_000; // 2100（远期；qemu RTC 2025 合法）
pub const PROBE_ISSUED: u64 = 1_700_000_000_000;

/// 探针状态机（Send 序列；跨 tick 推进）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeState {
    /// 未触发（START_TICK 前）。
    Idle,
    /// config-get gen1 基线。
    ConfigGet1,
    /// 维护进入（rid 100；两帧：进入 + 幂等回放）。
    MaintEnter,
    /// 换包 stage 分片（rid 200+i，共 N 片）。
    StageParts {
        next_part: usize,
        total_parts: usize,
    },
    /// stage commit（rid 240）。
    StageCommit,
    /// activate（rid 250；维护窗内）。
    Activate,
    /// config-get gen2（rid 260；应答丢弃——T2 冷启动为同代判据）。
    ConfigGet2,
    /// 受控重启 svc-observability（rid 300）。
    Restart,
    /// 等待重启完成（root 侧代次/ready 观察）。
    RestartWait {
        since: u64,
    },
    /// 维护退出（rid 500）。
    MaintExit,
    /// 负例：保护服务重启（rid 400）。
    NegRestartDenied,
    /// 负例：幂等冲突（rid 100 异参）。
    NegConflict,
    /// 70.7（WP-70.7.2/3）：BOOT 安装链（守卫查询 → trial 事务）。
    BootGuard,
    /// 70.7 诊断二分：0x0E 只读状态查询（同通道小帧）。
    BootStatus,
    BootTrial,
    /// 负例：维护外激活（宿主测试覆盖；占位步）。
    NegMaintRequired,
    /// 负例 v2 帧批（短尾 + 伪造）。
    NegFrames,
    /// 完成。
    Done,
}

/// 帧构造（v2 会话 + 固定 token/签名 + 变长动作段）。
pub fn build_frame(request_id: u64, action: &[u8], arg: &[u8]) -> alloc::vec::Vec<u8> {
    use crate::mgt_probe::mgt_probe_data::PROBE_SIG;
    let mut out =
        alloc::vec::Vec::with_capacity(5 + 48 + 64 + 74 + 8 + 1 + action.len() + arg.len());
    out.extend_from_slice(b"MGT1");
    out.push(0x00); // 保留字节（svc-mgmt handler 自 offset 5 起 parse）
                    // session v1 48B。
    out.extend_from_slice(PROBE_PRINCIPAL);
    out.extend_from_slice(&PROBE_ROLES);
    out.extend_from_slice(&PROBE_TENANT);
    out.extend_from_slice(PROBE_CHANNEL);
    out.extend_from_slice(&PROBE_EXPIRY.to_le_bytes());
    // token 64B（魔数 + 同域字段 + issued）。
    out.extend_from_slice(b"MGTPTK1\0");
    out.extend_from_slice(PROBE_PRINCIPAL);
    out.extend_from_slice(&PROBE_ROLES);
    out.extend_from_slice(&PROBE_TENANT);
    out.extend_from_slice(PROBE_CHANNEL);
    out.extend_from_slice(&PROBE_EXPIRY.to_le_bytes());
    out.extend_from_slice(&PROBE_ISSUED.to_le_bytes());
    // 签名字段 74B = u16 实长 + DER + 零填充。
    let sig_len = PROBE_SIG.len() as u16;
    out.extend_from_slice(&sig_len.to_le_bytes());
    out.extend_from_slice(PROBE_SIG);
    out.resize(out.len() + (74 - 2 - PROBE_SIG.len()), 0);
    // request_id + action + arg。
    out.extend_from_slice(&request_id.to_le_bytes());
    out.push(action.len() as u8);
    out.extend_from_slice(action);
    out.extend_from_slice(arg);
    out
}

/// 伪 v2 帧（full-shape + 垃圾签名——BadSignature 拒绝词形负例）。
pub fn build_forged_frame(request_id: u64, action: &[u8]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::with_capacity(200);
    out.extend_from_slice(b"MGT1");
    out.push(0x00);
    out.extend_from_slice(PROBE_PRINCIPAL);
    out.extend_from_slice(&PROBE_ROLES);
    out.extend_from_slice(&PROBE_TENANT);
    out.extend_from_slice(PROBE_CHANNEL);
    out.extend_from_slice(&PROBE_EXPIRY.to_le_bytes());
    out.extend_from_slice(b"MGTPTK1\0");
    out.extend_from_slice(PROBE_PRINCIPAL);
    out.extend_from_slice(&PROBE_ROLES);
    out.extend_from_slice(&PROBE_TENANT);
    out.extend_from_slice(PROBE_CHANNEL);
    out.extend_from_slice(&PROBE_EXPIRY.to_le_bytes());
    out.extend_from_slice(&PROBE_ISSUED.to_le_bytes());
    // 垃圾签名：非 DER 乱字节（der_decode_signature 必拒）。
    out.extend_from_slice(&70u16.to_le_bytes());
    out.extend_from_slice(&[0xDE; 72]);
    out.extend_from_slice(&request_id.to_le_bytes());
    out.push(action.len() as u8);
    out.extend_from_slice(action);
    out
}

/// 短尾 v2 帧（带魔数缺签名段——62..199B 缝隙攻击帧，P0 回归负例）。
pub fn build_truncated_frame(request_id: u64, action: &[u8]) -> alloc::vec::Vec<u8> {
    let mut out = alloc::vec::Vec::with_capacity(100);
    out.extend_from_slice(b"MGT1");
    out.push(0x00);
    out.extend_from_slice(PROBE_PRINCIPAL);
    out.extend_from_slice(&PROBE_ROLES);
    out.extend_from_slice(&PROBE_TENANT);
    out.extend_from_slice(PROBE_CHANNEL);
    out.extend_from_slice(&PROBE_EXPIRY.to_le_bytes());
    out.extend_from_slice(b"MGTPTK1\0");
    out.extend_from_slice(&request_id.to_le_bytes());
    out.push(action.len() as u8);
    out.extend_from_slice(action);
    out
}

/// BOOT 守卫查询帧（OTA1 0x0A；无 arg）。
pub fn boot_guard_frame() -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec::Vec::new();
    f.extend_from_slice(b"OTA1");
    f.push(0x0A);
    f
}

/// BOOT 确认帧（OTA1 0x0C + slot_is_a + secver + image_id）。
pub fn boot_confirm_frame(slot_is_a: bool, secver: u64, image_id: [u8; 16]) -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec::Vec::new();
    f.extend_from_slice(b"OTA1");
    f.push(0x0C);
    f.push(u8::from(slot_is_a));
    f.extend_from_slice(&secver.to_le_bytes());
    f.extend_from_slice(&image_id);
    f
}

/// BOOT trial 事务帧（OTA1 0x0B + target_secver + image_id）。
pub fn boot_trial_frame(secver: u64, image_id: [u8; 16]) -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec::Vec::new();
    f.extend_from_slice(b"OTA1");
    f.push(0x0B);
    f.extend_from_slice(&secver.to_le_bytes());
    f.extend_from_slice(&image_id);
    f
}

/// 70.7（REG-70.7-BOOT-TRIAL-BIND 实弹修复）：候选载荷身份读取帧
/// （OTA1 0x0F 只读）。响应 = secver(8) ‖ release_id(16)——试启动事务的
/// 目标身份必须与候选载荷 manifest 逐字段一致（boot-manager 选槽绑定
/// 判据），生产来源 = 已验签下行包；门内下行缺席 → 探针以本帧取候选槽
/// 载荷自身身份（svc-ota 侧读非活动槽首部 manifest 解析）。
pub fn boot_candidate_frame() -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec::Vec::new();
    f.extend_from_slice(b"OTA1");
    f.push(0x0F);
    f
}

/// 70.7.5：更新状态投影帧（OTA1 0x0E 只读；MGT2 0x06 中继载荷）。
pub fn update_status_frame() -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec::Vec::new();
    f.extend_from_slice(b"OTA1");
    f.push(0x0E);
    f
}

/// 70.7.5：取消 trial 帧（OTA1 0x0D；MGT2 0x07 中继载荷）。
pub fn update_rollback_frame() -> alloc::vec::Vec<u8> {
    let mut f = alloc::vec::Vec::new();
    f.extend_from_slice(b"OTA1");
    f.push(0x0D);
    f
}

#[cfg(target_os = "none")]
pub mod driver {
    use alloc::format;
    use alloc::vec::Vec;

    use super::*;

    /// 探针一步产生的执行指令（root 主循环解释执行）。
    pub enum ProbeDirective {
        /// 无动作（节流/等待）。
        None,
        /// root 状态机直驱（MGT2 帧原样交 orch.mgmt.handle——维护与
        /// 重启裁决+执行；环形动作免 IPC）。
        DriveMgt2(Vec<u8>),
        /// chunked MGT1 帧（svc-mgmt 生产接口；无 root 回调的动作）。
        Mgt1(Vec<u8>),
        /// chunked CFG1 激活（root→svc-config 直驱；维护闸在宿主测试覆盖）。
        ConfigActivate,
        /// 完成（全部步已执行）。
        Done,
    }

    /// chunked Call 一次（ENVR 分块协议；应答到达但探针只取成败——判据
    /// 词形由 svc-mgmt/svc-config 的串口审计行承载）。
    fn call_chunked(ep: u64, frame: &[u8], epoch: &mut u64) -> bool {
        let mut session = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
            "root-mgt-probe",
            ep,
            epoch,
        );
        session.call("svc-mgmt", frame).is_ok()
    }

    /// chunked Call 并返回响应字节（70.7.5 MGT2 OTA 中继；`call_chunked`
    /// 只取成败，本函数供状态/回退的响应回填）。None = 传输失败。
    pub(crate) fn call_chunked_resp(ep: u64, frame: &[u8], epoch: &mut u64) -> Option<Vec<u8>> {
        let mut session = eneros_agent_sdk::endpoint::ChunkedSession::bind_shared_const(
            "root-mgt-relay",
            ep,
            epoch,
        );
        session.call("svc-ota", frame).ok()
    }

    /// 步进（主循环每 tick 调用；按 PROBE_STEP_TICKS 节流）。
    pub fn step(
        state: &mut ProbeState,
        step_entered: &mut u64,
        svc_mgmt_ep: u64,
        svc_config_ep: u64,
        svc_ota_ep: u64,
        tick: u64,
        settled: bool,
        boot_slot: Option<u8>,
        capsule_secver: u64,
        capsule_release_id: [u8; 16],
        restart_done: &dyn Fn() -> bool,
    ) -> ProbeDirective {
        if *state == ProbeState::Idle {
            if settled {
                *state = ProbeState::ConfigGet1;
                *step_entered = tick;
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: sequence start (settled at tick {tick})"
                );
            }
            return ProbeDirective::None;
        }
        if tick.saturating_sub(*step_entered) < PROBE_STEP_TICKS {
            return ProbeDirective::None;
        }
        macro_rules! advance {
            ($s:expr) => {{
                *state = $s;
                *step_entered = tick;
            }};
        }
        let mut epoch = 0u64;
        match *state {
            ProbeState::Idle => ProbeDirective::None,
            ProbeState::ConfigGet1 => {
                let f = build_frame(10, b"config-get", b"control-write");
                let ok = call_chunked(svc_mgmt_ep, &f, &mut epoch);
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: config-get gen1 sent ok={ok} (step 1/13)"
                );
                advance!(ProbeState::MaintEnter);
                ProbeDirective::None
            }
            ProbeState::MaintEnter => {
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: maintenance-enter driven (step 2/13)"
                );
                advance!(ProbeState::StageParts {
                    next_part: 0,
                    total_parts: 0
                });
                ProbeDirective::DriveMgt2(crate::root_mgmt_wire::encode_maint_enter())
            }
            ProbeState::StageParts {
                next_part,
                total_parts,
            } => {
                let bundle = crate::mgt_probe::mgt_probe_data::PROBE_BUNDLE;
                let total = if total_parts == 0 {
                    bundle.len().div_ceil(512)
                } else {
                    total_parts
                };
                let i = next_part;
                if i >= total {
                    // 空载荷面（70.12 实弹缺陷修复保留）：`PROBE_BUNDLE` 为空
                    // ⇒ `total == 0` ⇒ 无 part 可发/无回放对象——显式拒绝并
                    // 跳过（fail-closed；探针链继续跑其余步）。
                    // F70-03：幂等回放步已前移至 part1 落账后（下方 `i == 0`
                    // 分支）——账本持久容量（15）小于 17 部件工作集，全部
                    // 部件后再重放会落在逐出视野之外（重放=新执行）。
                    if total == 0 {
                        eneros_runtime::println!(
                            "[eneros-root-task] MGT-PROBE-REJECT reason=empty-bundle (step 3/13 skipped, need >=512B payload)"
                        );
                    }
                    advance!(ProbeState::StageCommit);
                    return ProbeDirective::None;
                }
                let start = i * 512;
                let end = core::cmp::min(start + 512, bundle.len());
                let mut arg = alloc::vec::Vec::with_capacity(3 + (end - start));
                arg.push(b'S');
                arg.push((i + 1) as u8);
                arg.push(total as u8);
                arg.extend_from_slice(&bundle[start..end]);
                let f = build_frame(200 + i as u64, b"config-set", &arg);
                let ok = call_chunked(svc_mgmt_ep, &f, &mut epoch);
                advance!(ProbeState::StageParts {
                    next_part: i + 1,
                    total_parts: total
                });
                // F70-03（vol6 实弹订正）：幂等回放步 = **part1 落账后立即
                // 重发**（同 rid=200 同 arg）——原实现把回放放在全部 17 个
                // 部件之后，而持久账本容量（15，与区段槽预算一致）在 17
                // 部件后已把 part1 的记录逐出 ⇒ "重放"变成**新执行**
                //（op=18 再跑一次 stage-part = 幂等保证被逐出视野击穿，
                // R18 实弹咬合）。回放语义 = 记录在册期间零重复执行；
                // 逐出/背压视野由 host 账本容量锁（M08）覆盖。
                if i == 0 {
                    if let Some(rarg) = replay_arg_for(bundle, total as u64) {
                        let rf = build_frame(200, b"config-set", &rarg);
                        let _ = call_chunked(svc_mgmt_ep, &rf, &mut epoch);
                        eneros_runtime::println!(
                            "[eneros-root-task] MGT-PROBE: part1 idempotent replay sent (step 3/13)"
                        );
                    }
                }
                if ok {
                    ProbeDirective::None
                } else {
                    eneros_runtime::println!(
                        "[eneros-root-task] MGT-PROBE: stage part {} transport error",
                        i + 1
                    );
                    ProbeDirective::None
                }
            }
            ProbeState::StageCommit => {
                let nb = crate::mgt_probe::mgt_probe_data::PROBE_BUNDLE.len() as u32;
                let mut arg = alloc::vec::Vec::with_capacity(5);
                arg.push(b'C');
                arg.extend_from_slice(&nb.to_le_bytes());
                let f = build_frame(240, b"config-set", &arg);
                let ok = call_chunked(svc_mgmt_ep, &f, &mut epoch);
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: stage commit sent ok={ok} (step 4/13)"
                );
                advance!(ProbeState::Activate);
                ProbeDirective::None
            }
            ProbeState::Activate => {
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: activate driven (step 5/13)"
                );
                advance!(ProbeState::ConfigGet2);
                ProbeDirective::ConfigActivate
            }
            ProbeState::ConfigGet2 => {
                let f = build_frame(260, b"config-get", b"control-write");
                let ok = call_chunked(svc_mgmt_ep, &f, &mut epoch);
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: config-get gen2 sent ok={ok} (step 6/13)"
                );
                advance!(ProbeState::Restart);
                ProbeDirective::None
            }
            ProbeState::Restart => {
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: restart driven (step 7/13)"
                );
                advance!(ProbeState::RestartWait { since: tick });
                ProbeDirective::DriveMgt2(crate::root_mgmt_wire::encode_restart(
                    "svc-observability",
                ))
            }
            ProbeState::RestartWait { since } => {
                if restart_done() {
                    eneros_runtime::println!(
                        "[eneros-root-task] MGT-PROBE: restart observed complete (gen bumped + ready) (step 8/13)"
                    );
                    advance!(ProbeState::MaintExit);
                    return ProbeDirective::None;
                }
                if tick.saturating_sub(since) > RESTART_WAIT_TICKS {
                    eneros_runtime::println!(
                        "[eneros-root-task] MGT-PROBE: restart wait TIMEOUT — continuing"
                    );
                    advance!(ProbeState::MaintExit);
                }
                ProbeDirective::None
            }
            ProbeState::MaintExit => {
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: maintenance-exit driven (step 9/13)"
                );
                advance!(ProbeState::NegRestartDenied);
                ProbeDirective::DriveMgt2(crate::root_mgmt_wire::encode_maint_exit())
            }
            ProbeState::NegRestartDenied => {
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: neg restart-denied driven (step 10/13)"
                );
                advance!(ProbeState::NegConflict);
                ProbeDirective::DriveMgt2(crate::root_mgmt_wire::encode_restart("svc-storage"))
            }
            ProbeState::NegConflict => {
                // 幂等冲突：对既有 MGT1 操作（rid=200 stage-part，已入账）
                // 以同 rid 异参重发——账本 Conflict 拒绝（零 root 回调，
                // 探针环形安全；maint-enter 为 root 直驱无 MGT1 账目）。
                let f = build_frame(200, b"config-set", b"ZZZ");
                let ok = call_chunked(svc_mgmt_ep, &f, &mut epoch);
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: neg idempotency-conflict sent ok={ok} (step 11/13)"
                );
                advance!(ProbeState::NegMaintRequired);
                ProbeDirective::None
            }
            ProbeState::NegMaintRequired => {
                // 维护外激活拒绝（svc-mgmt 闸）：环形不安全，宿主测试覆盖
                // （maintenance-gate host-locked；QEMU 侧 R17 载荷省略）。
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: neg maint-required host-covered (step 12/13)"
                );
                advance!(ProbeState::NegFrames);
                ProbeDirective::None
            }
            ProbeState::NegFrames => {
                let t = build_truncated_frame(0xBEEF, b"reboot");
                let _ = call_chunked(svc_mgmt_ep, &t, &mut epoch);
                let g = build_forged_frame(0xBEEF + 1, b"reboot");
                let _ = call_chunked(svc_mgmt_ep, &g, &mut epoch);
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: negative v2 frames sent (step 13/13)"
                );
                advance!(ProbeState::BootGuard);
                ProbeDirective::None
            }
            ProbeState::BootGuard => {
                // 70.7（WP-70.7.2）：BOOT 守卫查询（CHECK-DESC 已由 OTA
                // 下载探针武装——0x01 判定=1 置 install_armed）。
                let f = boot_guard_frame();
                let _ = call_chunked(svc_ota_ep, &f, &mut epoch);
                eneros_runtime::println!(
                    "[eneros-root-task] MGT-PROBE: boot guard queried (step 14/13)"
                );
                advance!(ProbeState::BootStatus);
                ProbeDirective::None
            }
            ProbeState::BootStatus => {
                // 70.7.5：状态投影（0x0E 只读，root 中继）——trial 前的
                // bootctrl 基线取证。
                let f = update_status_frame();
                let r = call_chunked(svc_ota_ep, &f, &mut epoch);
                eneros_runtime::println!("[eneros-root-task] MGT-PROBE: boot status probed ok={r}");
                advance!(ProbeState::BootTrial);
                ProbeDirective::None
            }
            ProbeState::BootTrial => {
                // 自适应：首启（capsule slot=A）→ trial 事务（bootctrl B
                // trial 严格高于 A，序列+1 双副本+flush）；复启（slot=B，
                // 即 trial 目标已启动）→ 启动确认（successful=true）。
                if boot_slot == Some(2) {
                    // 复启（trial 目标已启动）：确认记录运行载荷**自身**身份
                    //（capsule 投影 = 已验签 manifest 的 release_id/secver）。
                    let f = boot_confirm_frame(false, capsule_secver, capsule_release_id);
                    let r = call_chunked(svc_ota_ep, &f, &mut epoch);
                    eneros_runtime::println!(
                        "[eneros-root-task] MGT-PROBE: boot confirm sent ok={r} (step 14b/13)"
                    );
                } else {
                    // 首启：候选身份读取（0x0F 只读）——trial 记录的目标
                    // 身份 = 候选载荷 manifest 身份（与 boot-manager 选槽
                    // 绑定判据同源；硬编码身份必被 SECURITY_BINDING 拒）。
                    let f = boot_candidate_frame();
                    let id = call_chunked_resp(svc_ota_ep, &f, &mut epoch);
                    let Some(id) = id else {
                        eneros_runtime::println!(
                            "[eneros-root-task] MGT-PROBE: boot candidate read failed — trial skipped (honest)"
                        );
                        advance!(ProbeState::Done);
                        return ProbeDirective::None;
                    };
                    if id.len() < 24 {
                        eneros_runtime::println!(
                            "[eneros-root-task] MGT-PROBE: boot candidate short ({}B) — trial skipped",
                            id.len()
                        );
                        advance!(ProbeState::Done);
                        return ProbeDirective::None;
                    }
                    let secver = u64::from_le_bytes(id[..8].try_into().unwrap());
                    let mut release_id = [0u8; 16];
                    release_id.copy_from_slice(&id[8..24]);
                    eneros_runtime::println!(
                        "[eneros-root-task] MGT-PROBE: boot candidate secver={secver} release_id={:02x}{:02x}{:02x}{:02x}…",
                        release_id[0],
                        release_id[1],
                        release_id[2],
                        release_id[3]
                    );
                    let f = boot_trial_frame(secver, release_id);
                    eneros_runtime::println!(
                        "[eneros-root-task] MGT-PROBE: boot trial sending (slot={boot_slot:?})"
                    );
                    let r = call_chunked(svc_ota_ep, &f, &mut epoch);
                    eneros_runtime::println!(
                        "[eneros-root-task] MGT-PROBE: boot trial tx sent ok={r} (step 14b/13)"
                    );
                }
                advance!(ProbeState::Done);
                ProbeDirective::None
            }
            ProbeState::Done => ProbeDirective::Done,
        }
    }

    /// 负例帧批（短尾 + 伪造——拒绝词形取证；拒绝路径零 root 回调）。
    pub fn send_negative_frames(svc_mgmt_ep: u64, epoch: &mut u64) {
        let t = build_truncated_frame(0xBEEF, b"reboot");
        let _ = call_chunked(svc_mgmt_ep, &t, epoch);
        let g = build_forged_frame(0xBEEF + 1, b"reboot");
        let _ = call_chunked(svc_mgmt_ep, &g, epoch);
    }
}

/// 幂等回放帧的参数字节（`None` = 载荷不足，调用方按显式拒绝处理）。
///
/// 纯逻辑（无 IPC/全局状态）：70.12 修复的越界 panic 由此函数的前置判据
/// 兜住；host 可测，且与 `StageParts` 的正常分片路径共用 `CHUNK` 口径。
pub const PROBE_CHUNK: usize = 512;

pub fn replay_arg_for(bundle: &[u8], total_parts: u64) -> Option<alloc::vec::Vec<u8>> {
    if bundle.len() < PROBE_CHUNK {
        return None;
    }
    let mut a = alloc::vec::Vec::with_capacity(3 + PROBE_CHUNK);
    a.push(b'S');
    a.push(1);
    a.push(total_parts as u8);
    a.extend_from_slice(&bundle[..PROBE_CHUNK]);
    Some(a)
}

#[cfg(test)]
mod tests_70_12 {
    use super::*;

    #[test]
    fn empty_bundle_is_refused_not_panicking() {
        // 实弹形态：未注入 ENEROS_MGT_PROBE_BUNDLE ⇒ PROBE_BUNDLE 为空。
        assert_eq!(replay_arg_for(&[], 0), None);
        assert_eq!(replay_arg_for(&[0u8; 511], 1), None);
    }

    #[test]
    fn full_chunk_builds_replay_arg() {
        let bundle = alloc::vec![7u8; 1024];
        let arg = replay_arg_for(&bundle, 2).expect("1024B 载荷可回放");
        assert_eq!(arg.len(), 3 + 512);
        assert_eq!(&arg[..3], &[b'S', 1, 2]);
        assert!(arg[3..].iter().all(|b| *b == 7));
        // 恰好一页（512B）也成立（边界）。
        let one = replay_arg_for(&alloc::vec![1u8; 512], 1).expect("边界 512B");
        assert_eq!(one.len(), 3 + 512);
    }
}
