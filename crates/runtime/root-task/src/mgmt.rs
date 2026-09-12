//! 70.6（WP-70.6.3）：root 管理状态机（MGT2 端点的裁决单源）。
//!
//! 职责：维护状态、实例代次、受控重启白名单/风暴守卫、退出维护的
//! 健康门。**纯状态机**（host 可测）——真实 seL4 触点（TCB 重启、
//! ready ntfn poll、串口词形）由 [`crate::service`] 调用方执行；
//! 状态机只回答"准/不准"与"下一步动作"。
//!
//! 授权面：唯一发起方 = svc-mgmt（授予边单边；MGT1 认证对端经
//! mgmt_rpc RBAC 后才到达此处——root 侧不再重复角色判定，只做
//! 目标合法性/时序/健康裁决）。

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// 受保护服务（禁止管理重启——root 自身/存储/配置/控制链/管理面/探针）。
pub const RESTART_DENIED: &[&str] = &[
    "root",
    // 存储与配置 = 全系统持久真相；重启窗口内所有落盘请求将失败。
    "svc-storage",
    "svc-config",
    // 控制链关键路径：采集/分发/设备写——重启 = 控制中断（维护窗口
    // 内的受控重启归 70.11 计划持有面裁决，本卷不开）。
    "svc-telemetry",
    "svc-control-dispatch",
    "svc-device-agent",
    // 管理面自身（重启即孤儿化在途操作）。
    "svc-mgmt",
    // 探针/测试锚点。
    "svc-hello",
    "svc-fault",
    "svc-hang",
];

/// 手动重启风暴守卫：两次手动重启的最小间隔（主循环 tick）。
pub const RESTART_MIN_INTERVAL_TICKS: u64 = 200;

/// MGT2 裁决动作（状态机 → 调用方）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MgmtAction {
    /// 执行受控重启（目标句柄索引；代次由调用方在真实重启完成后 bump）。
    Restart(usize),
    /// 重启后等待 ready 重验（服务句柄索引；调用方 poll 其 ready ntfn）。
    AwaitReady(usize),
}

/// 裁决结果：回复帧 + 可选动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MgmtVerdict {
    pub resp: Vec<u8>,
    pub action: Option<MgmtAction>,
    /// 串口词形（调用方原样打印；None = 静默）。
    pub word: Option<String>,
}

/// root 管理状态机。
#[cfg_attr(not(target_os = "none"), allow(dead_code))] // host 面死码（target run_ipc_loop 活）
pub struct RootMgmtState {
    /// 维护激活中（ControlChain 新写暂停）。
    maintenance: bool,
    entered_tick: u64,
    /// 每服务实例代次（任何重启 +1——人工/故障/心跳同径）。
    generations: Vec<u64>,
    /// 每服务重启计数。
    restart_counts: Vec<u64>,
    /// 待 ready 重验（手动重启后重新 Signal 就绪前非 ready）。
    awaiting_ready: Vec<usize>,
    /// 上次手动重启 tick（风暴守卫）。
    last_manual_restart: Option<u64>,
}

impl RootMgmtState {
    pub fn new(service_count: usize) -> Self {
        RootMgmtState {
            maintenance: false,
            entered_tick: 0,
            generations: alloc::vec![0; service_count],
            restart_counts: alloc::vec![0; service_count],
            awaiting_ready: Vec::new(),
            last_manual_restart: None,
        }
    }

    pub fn maintenance_active(&self) -> bool {
        self.maintenance
    }

    pub fn generation_of(&self, idx: usize) -> u64 {
        self.generations.get(idx).copied().unwrap_or(0)
    }

    pub fn restart_count_of(&self, idx: usize) -> u64 {
        self.restart_counts.get(idx).copied().unwrap_or(0)
    }

    /// 服务是否在 ready 重验等待中。
    pub fn is_awaiting_ready(&self, idx: usize) -> bool {
        self.awaiting_ready.contains(&idx)
    }

    /// ready 重验等待名单（快照；调用方逐个 poll 对应 ntfn）。
    pub fn awaiting_indices(&self) -> alloc::vec::Vec<usize> {
        self.awaiting_ready.clone()
    }

    /// 真实重启完成回调（apply_restart_action 全路径调用——人工/故障/
    /// 心跳同径 bump；故障路径不含 ready 重验，服务由自身 init 重跑）。
    pub fn note_restarted(&mut self, idx: usize, manual: bool, tick: u64) {
        if let Some(g) = self.generations.get_mut(idx) {
            *g = g.wrapping_add(1);
        }
        if let Some(c) = self.restart_counts.get_mut(idx) {
            *c = c.wrapping_add(1);
        }
        if manual {
            self.last_manual_restart = Some(tick);
            if !self.awaiting_ready.contains(&idx) {
                self.awaiting_ready.push(idx);
            }
        }
    }

    /// ready 重验完成回调（调用方 poll 到就绪 Signal 后调用）。
    pub fn note_ready(&mut self, idx: usize) -> bool {
        let before = self.awaiting_ready.len();
        self.awaiting_ready.retain(|&i| i != idx);
        before != self.awaiting_ready.len()
    }

    /// MGT2 请求裁决。
    ///
    /// `services` = 句柄名表；`required_ready` = 全部 required 服务
    /// Ready（维护退出的健康门）；`ready_of` = 每服务当前 ready 判定
    ///（实例查询的 ready 位；awaiting_ready 待重验 = false）。
    pub fn handle(
        &mut self,
        req: &[u8],
        tick: u64,
        services: &[&str],
        required_ready: bool,
        ready_of: &dyn Fn(usize) -> bool,
    ) -> MgmtVerdict {
        use crate::root_mgmt_wire as wire;
        let (cmd, arg) = match wire::parse_request(req) {
            Ok(v) => v,
            Err(()) => {
                return MgmtVerdict {
                    resp: wire::err_resp("proto"),
                    action: None,
                    word: None,
                }
            }
        };
        match cmd {
            wire::CMD_RESTART => {
                let target = match wire::parse_target(arg) {
                    Ok(t) => t,
                    Err(()) => {
                        return self.reject("restart-bad-target");
                    }
                };
                if crate::mgmt::RESTART_DENIED.contains(&target) {
                    return self.reject("restart-denied:protected");
                }
                let Some(idx) = services.iter().position(|s| *s == target) else {
                    return self.reject("restart-unknown-target");
                };
                // 风暴守卫。
                if let Some(last) = self.last_manual_restart {
                    if tick.saturating_sub(last) < RESTART_MIN_INTERVAL_TICKS {
                        return self.reject("restart-throttled");
                    }
                }
                // 已在途（上一重启未完成 ready 重验）→ 拒绝并发。
                if self.awaiting_ready.contains(&idx) {
                    return self.reject("restart-in-progress");
                }
                MgmtVerdict {
                    resp: wire::ok_resp(),
                    action: Some(MgmtAction::Restart(idx)),
                    word: Some(alloc::format!(
                        "MGMT-RESTART: {} requested (tick={tick})",
                        services[idx]
                    )),
                }
            }
            wire::CMD_INSTANCE => {
                let target = match wire::parse_target(arg) {
                    Ok(t) => t,
                    Err(()) => {
                        return self.reject("instance-bad-target");
                    }
                };
                let Some(idx) = services.iter().position(|s| *s == target) else {
                    return self.reject("instance-unknown-target");
                };
                let ready = !self.awaiting_ready.contains(&idx) && ready_of(idx);
                MgmtVerdict {
                    resp: wire::encode_instance_resp(
                        self.generation_of(idx),
                        0, // lifecycle 投影位（细化归 supervisor 联动卷）
                        self.restart_count_of(idx),
                        ready,
                    ),
                    action: None,
                    word: None,
                }
            }
            wire::CMD_MAINT_ENTER => {
                if self.maintenance {
                    return self.reject("maint-already-active");
                }
                self.maintenance = true;
                self.entered_tick = tick;
                MgmtVerdict {
                    resp: wire::ok_resp(),
                    action: None,
                    word: Some(alloc::format!(
                        "MGMT-MAINT: entered (control writes paused, tick={tick})"
                    )),
                }
            }
            wire::CMD_MAINT_EXIT => {
                if !self.maintenance {
                    return self.reject("maint-not-active");
                }
                // 健康门：required 服务全 Ready 才放行（退出前必要健康检查）。
                if !required_ready {
                    return self.reject("maint-health-gate");
                }
                self.maintenance = false;
                MgmtVerdict {
                    resp: wire::ok_resp(),
                    action: None,
                    word: Some(alloc::format!(
                        "MGMT-MAINT: exited (control writes resumed, held {} ticks)",
                        tick.saturating_sub(self.entered_tick)
                    )),
                }
            }
            wire::CMD_MAINT_QUERY => MgmtVerdict {
                resp: wire::encode_maint_query_resp(self.maintenance, self.entered_tick),
                action: None,
                word: None,
            },
            _ => self.reject("unknown-cmd"),
        }
    }

    fn reject(&self, reason: &str) -> MgmtVerdict {
        MgmtVerdict {
            resp: crate::root_mgmt_wire::err_resp(reason),
            action: None,
            word: Some(alloc::format!("MGMT-REJECT: {reason}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root_mgmt_wire as wire;

    const SERVICES: &[&str] = &[
        "svc-storage",
        "svc-config",
        "svc-ota",
        "svc-observability",
        "svc-mgmt",
    ];
    const OTA: usize = 2;
    const OBS: usize = 3;

    fn all_ready(_idx: usize) -> bool {
        true
    }

    fn state() -> RootMgmtState {
        RootMgmtState::new(SERVICES.len())
    }

    #[test]
    fn restart_allow_and_gen_bump_via_note() {
        let mut s = state();
        let v = s.handle(
            &wire::encode_restart("svc-ota"),
            100,
            SERVICES,
            true,
            &all_ready,
        );
        assert_eq!(v.resp, vec![1]);
        assert_eq!(v.action, Some(MgmtAction::Restart(OTA)));
        assert!(v.word.as_deref().unwrap_or("").contains("svc-ota"));
        // 调用方执行重启后回调：代次/计数 bump + ready 重验挂起。
        s.note_restarted(OTA, true, 101);
        assert_eq!(s.generation_of(OTA), 1);
        assert_eq!(s.restart_count_of(OTA), 1);
        assert!(s.is_awaiting_ready(OTA), "手动重启后必须等待 ready 重验");
        // ready 前实例查询 ready=0。
        let q = s.handle(
            &wire::encode_instance_query("svc-ota"),
            102,
            SERVICES,
            true,
            &|_| true,
        );
        let (gen, _lc, cnt, ready) = wire::parse_instance_resp(&q.resp).unwrap();
        assert_eq!((gen, cnt, ready), (1, 1, false));
        // poll 到 ready Signal → 重验解除。
        assert!(s.note_ready(OTA));
        let q = s.handle(
            &wire::encode_instance_query("svc-ota"),
            103,
            SERVICES,
            true,
            &all_ready,
        );
        let (_, _, _, ready) = wire::parse_instance_resp(&q.resp).unwrap();
        assert!(ready);
    }

    #[test]
    fn restart_denied_protected_and_unknown() {
        let mut s = state();
        for t in ["svc-storage", "svc-config", "svc-mgmt", "svc-telemetry"] {
            let v = s.handle(&wire::encode_restart(t), 10, SERVICES, true, &all_ready);
            assert!(v.resp.starts_with(b"ERR:restart-denied"), "{t}");
        }
        let v = s.handle(
            &wire::encode_restart("svc-nosuch"),
            11,
            SERVICES,
            true,
            &all_ready,
        );
        assert!(v.resp.starts_with(b"ERR:restart-unknown-target"));
    }

    #[test]
    fn restart_storm_guard_and_concurrent_reject() {
        let mut s = state();
        let v = s.handle(
            &wire::encode_restart("svc-ota"),
            100,
            SERVICES,
            true,
            &all_ready,
        );
        assert_eq!(v.action, Some(MgmtAction::Restart(OTA)));
        s.note_restarted(OTA, true, 101);
        // 间隔内 → 节流拒绝。
        let v = s.handle(
            &wire::encode_restart("svc-ota"),
            101 + RESTART_MIN_INTERVAL_TICKS - 1,
            SERVICES,
            true,
            &all_ready,
        );
        assert!(v.resp.starts_with(b"ERR:restart-throttled"));
        // 已在 ready 重验等待 → 并发拒绝（即使间隔已过）。
        let v = s.handle(
            &wire::encode_restart("svc-ota"),
            101 + RESTART_MIN_INTERVAL_TICKS,
            SERVICES,
            true,
            &all_ready,
        );
        assert!(v.resp.starts_with(b"ERR:restart-in-progress"));
        // 其他服务不受在途影响（间隔后）。
        let v = s.handle(
            &wire::encode_restart("svc-observability"),
            101 + RESTART_MIN_INTERVAL_TICKS,
            SERVICES,
            true,
            &all_ready,
        );
        assert_eq!(v.action, Some(MgmtAction::Restart(OBS)));
    }

    #[test]
    fn maintenance_lifecycle_with_health_gate() {
        let mut s = state();
        // 未进入即退出 → 拒绝。
        assert!(s
            .handle(&wire::encode_maint_exit(), 10, SERVICES, true, &all_ready)
            .resp
            .starts_with(b"ERR:maint-not-active"));
        // 进入。
        let v = s.handle(&wire::encode_maint_enter(), 20, SERVICES, true, &all_ready);
        assert_eq!(v.resp, vec![1]);
        assert!(s.maintenance_active());
        // 重复进入 → 拒绝。
        assert!(s
            .handle(&wire::encode_maint_enter(), 21, SERVICES, true, &all_ready)
            .resp
            .starts_with(b"ERR:maint-already-active"));
        // required 未就绪 → 健康门拒绝退出。
        let v = s.handle(&wire::encode_maint_exit(), 30, SERVICES, false, &all_ready);
        assert!(v.resp.starts_with(b"ERR:maint-health-gate"));
        assert!(s.maintenance_active(), "健康门拒绝后维持维护态");
        // 恢复就绪 → 退出放行（词形含持续时长）。
        let v = s.handle(&wire::encode_maint_exit(), 45, SERVICES, true, &all_ready);
        assert_eq!(v.resp, vec![1]);
        assert!(v.word.as_deref().unwrap_or("").contains("held 25 ticks"));
        assert!(!s.maintenance_active());
        // 状态查询。
        let q = s.handle(&wire::encode_maint_query(), 46, SERVICES, true, &all_ready);
        assert_eq!(wire::parse_maint_query_resp(&q.resp).unwrap(), (false, 20));
    }

    #[test]
    fn bad_frames_rejected() {
        let mut s = state();
        assert!(s
            .handle(b"MGT1\x01", 0, SERVICES, true, &all_ready)
            .resp
            .starts_with(b"ERR:proto"));
        assert!(s
            .handle(b"MGT2\x77", 0, SERVICES, true, &all_ready)
            .resp
            .starts_with(b"ERR:unknown-cmd"));
        assert!(s
            .handle(&wire::encode_restart(""), 0, SERVICES, true, &all_ready)
            .resp
            .starts_with(b"ERR:restart-bad-target"));
    }
}
