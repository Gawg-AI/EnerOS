//! GB/T 27930 插件（`ProtocolPlugin` 合同实现——蓝图 48 WP-48.4）.
//!
//! 角色=充电机侧（桩端，owner 默认裁决）。CAN 传输（SJA1000+TP）接线
//! =51 卷真机路径，QEMU virt 无 CAN——descriptor `transport_kinds=&[]`
//! （诚实：当前无可获取的传输）；状态机经 `poll` 由宿主时钟驱动，
//! 报文注入 seam = [`CsmHost`]（字节解码 BLOCKED——宿主解出抽象
//! [`Msg`] 后喂入）。

use eneros_protocol_abstract::{
    AdapterState, InteropLevel, PointRegistry, Progress, ProtocolConfig, ProtocolDescriptor,
    ProtocolError, ProtocolHealth, ProtocolPlugin, ProtocolRole, ResourceBudget, SecurityProfile,
    TransportFactory, WorkBudget,
};

use crate::config::{validate, Gbt27930Config};
use crate::sm::{AbortReason, Action, ChargePhase, ChargerLimits, ChargerSm, Msg};

/// 插件 id。
pub const GBT27930_PLUGIN_ID: &str = "gbt27930";

/// 充电机默认输出能力（宿主可按桩参数覆盖——非规约值，桩自身铭牌参数）。
#[derive(Debug, Clone, Copy)]
pub struct HostLimits {
    /// 最高输出电压（V）。
    pub max_voltage_v: f32,
    /// 最大输出电流（A）。
    pub max_current_a: f32,
}

/// 状态机宿主 seam：插件持有状态机；宿主（服务层/测试）经此注入
/// 解码后的报文并取出发送动作（发送字节层 BLOCKED）。
pub struct Gbt27930Plugin {
    cfg: Option<Gbt27930Config>,
    sm: Option<ChargerSm>,
    last_poll_ms: u64,
    error_count: u64,
    last_error: Option<ProtocolError>,
    last_progress: Progress,
    routed_count: usize,
    /// 最近一次状态机输出（宿主取走后清空——单向出队语义）。
    pending_action: Option<Action>,
}

impl Gbt27930Plugin {
    pub fn new() -> Self {
        Self {
            cfg: None,
            sm: None,
            last_poll_ms: 0,
            error_count: 0,
            last_error: None,
            last_progress: Progress::Idle,
            routed_count: 0,
            pending_action: None,
        }
    }

    /// 路由点数（观测）。
    pub fn routed_count(&self) -> usize {
        self.routed_count
    }

    /// 已消费配置（观测）。
    pub fn active_config(&self) -> Option<&Gbt27930Config> {
        self.cfg.as_ref()
    }

    /// 当前充电相（观测/遥测映射）。
    pub fn phase(&self) -> ChargePhase {
        self.sm.as_ref().map_or(ChargePhase::Idle, |s| s.phase())
    }

    /// 取走最近一次状态机动作（有则返回并清空）。
    pub fn take_action(&mut self) -> Option<Action> {
        self.pending_action.take()
    }

    /// 宿主发起充电流程（桩端业务入口；非 ProtocolPlugin 合同部分）。
    pub fn start_charging(&mut self, now_ms: u64) -> Result<(), ProtocolError> {
        let Some(sm) = self.sm.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        match sm.start(now_ms) {
            Some(a) => {
                self.pending_action = Some(a);
                Ok(())
            }
            None => Err(ProtocolError::InvalidConfig),
        }
    }

    /// 宿主注入解码后报文（字节解码 BLOCKED seam）。
    pub fn on_message(&mut self, msg: Msg, now_ms: u64) {
        let Some(sm) = self.sm.as_mut() else {
            return;
        };
        if let Some(a) = sm.on(crate::sm::Event::Rx(msg), now_ms) {
            self.pending_action = Some(a);
        }
    }

    /// codec/地址表/超时表 BLOCKED 登记行。
    pub fn spec_blocked_note() -> &'static str {
        "CAN codec/PGN/addr-table BLOCKED (owner input: GB/T 27930-2015 spec text); L1 USB-CAN = owner hardware"
    }
}

impl Default for Gbt27930Plugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProtocolPlugin for Gbt27930Plugin {
    fn descriptor(&self) -> &ProtocolDescriptor {
        static D: ProtocolDescriptor = ProtocolDescriptor {
            id: GBT27930_PLUGIN_ID,
            version: "0.1",
            role: ProtocolRole::Master,
            // 诚实：CAN 传输未接线（QEMU 无 CAN；SJA1000 真机路径=51 卷）。
            transport_kinds: &[],
            security_profile: SecurityProfile::None,
            point_types: &["float", "int", "bool"],
            resource_budget: ResourceBudget {
                max_connections: 1,
                max_poll_ms: 50,
                max_frame: 64,
            },
            // 诚实分级：状态机/TP 逻辑完成（host 单测），字节层 BLOCKED；
            // HardwareVerified 未达成（蓝图 DoD 诚实登记）。
            interop_level: InteropLevel::SpecBlocked,
        };
        &D
    }

    fn validate_config(&self, cfg: &ProtocolConfig) -> Result<(), ProtocolError> {
        validate(cfg)
    }

    fn start(
        &mut self,
        cfg: &ProtocolConfig,
        _transports: &mut dyn TransportFactory,
        points: &mut dyn PointRegistry,
    ) -> Result<(), ProtocolError> {
        self.routed_count =
            eneros_protocol_abstract::plugin::count_matching_mappings(points, |m| {
                matches!(
                    m.protocol_addr,
                    eneros_protocol_abstract::ProtocolAddress::Can { .. }
                )
            });
        if self.sm.is_some() {
            return Err(ProtocolError::ProtocolInit);
        }
        // 配置全键显式消费（fail-closed）；状态机就绪但 Idle——充电流程
        // 由宿主经 start_charging 显式发起。
        let c = Gbt27930Config::from_validated(cfg)?;
        self.cfg = Some(c.clone());
        self.sm = Some(ChargerSm::new(
            c,
            ChargerLimits {
                max_voltage_v: 500.0,
                max_current_a: 250.0,
            },
        ));
        Ok(())
    }

    fn poll(&mut self, now_ms: u64, _budget: WorkBudget) -> Result<Progress, ProtocolError> {
        let Some(sm) = self.sm.as_mut() else {
            return Err(ProtocolError::ProtocolNotStarted);
        };
        self.last_poll_ms = now_ms;
        // 时钟推进驱动每相超时判定（跨进程同基准——40 卷教训）。
        if let Some(a) = sm.on(crate::sm::Event::Tick, now_ms) {
            match a {
                Action::Abort(reason) => {
                    self.error_count += 1;
                    self.last_progress = Progress::Degraded;
                    let _ = reason;
                    let _ = AbortReason::HandshakeTimeout;
                    self.last_error = Some(ProtocolError::ReadFailed);
                    self.pending_action = Some(a);
                    return Ok(Progress::Degraded);
                }
                other => self.pending_action = Some(other),
            }
        }
        if self.last_progress != Progress::Degraded {
            self.last_progress = Progress::Idle;
        }
        Ok(self.last_progress)
    }

    fn read_point(
        &mut self,
        _now_ms: u64,
        _mapping: &eneros_protocol_abstract::ProtocolPointMapping,
    ) -> Result<eneros_protocol_abstract::PointRead, ProtocolError> {
        Err(ProtocolError::Unsupported)
    }

    fn health(&self) -> ProtocolHealth {
        let state = if self.sm.is_some() {
            AdapterState::Running
        } else {
            AdapterState::Uninitialized
        };
        ProtocolHealth {
            state,
            last_poll_ms: self.last_poll_ms,
            error_count: self.error_count,
            last_error: self.last_error.clone(),
            last_progress: self.last_progress,
        }
    }

    fn debug_line(&self) -> Option<alloc::string::String> {
        Some(alloc::format!(
            "gbt27930 phase={:?} routed={} {}",
            self.phase(),
            self.routed_count,
            Self::spec_blocked_note()
        ))
    }

    fn stop(&mut self) -> Result<(), ProtocolError> {
        self.sm = None;
        self.cfg = None;
        self.pending_action = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use eneros_protocol_abstract::{
        AdapterState, EmptyPointRegistry, InteropLevel, ProtocolAddress, ProtocolPointMapping,
        TransportError, TransportFactory, TransportHandle, TransportKind, TransportSpec,
    };

    use super::*;

    struct NoTransportFactory;
    impl TransportFactory for NoTransportFactory {
        fn open(
            &mut self,
            _spec: &TransportSpec,
        ) -> Result<Box<dyn TransportHandle>, TransportError> {
            Err(TransportError::Unsupported)
        }
        fn supports(&self, _kind: TransportKind) -> bool {
            // 诚实：无传输种类可支持（descriptor transport_kinds=&[]）。
            false
        }
    }

    fn full_cfg() -> ProtocolConfig {
        let mut cfg = ProtocolConfig::default();
        cfg.insert("charger_addr", "86");
        cfg.insert("bms_addr", "1");
        cfg.insert("handshake_timeout_ms", "100");
        cfg.insert("param_timeout_ms", "200");
        cfg.insert("charging_timeout_ms", "300");
        cfg.insert("end_timeout_ms", "50");
        cfg.insert("retransmit_max", "3");
        cfg
    }

    #[test]
    fn descriptor_honest_no_transport_and_spec_blocked() {
        let p = Gbt27930Plugin::new();
        assert_eq!(p.descriptor().id, "gbt27930");
        assert_eq!(p.descriptor().interop_level, InteropLevel::SpecBlocked);
        assert!(p.descriptor().transport_kinds.is_empty());
        assert_eq!(p.descriptor().role, ProtocolRole::Master);
    }

    #[test]
    fn start_requires_full_config_and_consumes_it() {
        let mut p = Gbt27930Plugin::new();
        assert_eq!(
            p.start(
                &ProtocolConfig::default(),
                &mut NoTransportFactory,
                &mut EmptyPointRegistry
            ),
            Err(ProtocolError::InvalidConfig),
            "缺超时表/地址表 → fail-closed"
        );
        p.start(
            &full_cfg(),
            &mut NoTransportFactory,
            &mut EmptyPointRegistry,
        )
        .expect("start");
        assert_eq!(p.health().state, AdapterState::Running);
        assert_eq!(p.active_config().expect("cfg").charger_addr, 86);
    }

    #[test]
    fn charging_flow_via_host_seam_and_poll_timeouts() {
        let mut p = Gbt27930Plugin::new();
        p.start(
            &full_cfg(),
            &mut NoTransportFactory,
            &mut EmptyPointRegistry,
        )
        .expect("start");
        p.start_charging(0).expect("charge start");
        assert_eq!(p.take_action(), Some(Action::Send(Msg::Chm)));
        assert_eq!(p.take_action(), None, "取走即清空");
        p.on_message(Msg::Bhm, 10);
        assert_eq!(p.take_action(), Some(Action::Send(Msg::Crm)));
        p.on_message(Msg::Brm, 20);
        assert_eq!(p.take_action(), Some(Action::Send(Msg::Cml)));
        assert_eq!(p.phase(), ChargePhase::ParamConfig);
        // 握手相已离开——握手超时不再命中；参数相超时经 poll(Tick) 判定。
        assert_eq!(
            p.poll(
                219,
                WorkBudget {
                    max_work_ms: 10,
                    max_polls: 1
                }
            ),
            Ok(Progress::Idle)
        );
        assert_eq!(
            p.poll(
                220,
                WorkBudget {
                    max_work_ms: 10,
                    max_polls: 1
                }
            ),
            Ok(Progress::Degraded)
        );
        assert_eq!(
            p.take_action(),
            Some(Action::Abort(crate::sm::AbortReason::ParamTimeout))
        );
        assert_eq!(p.health().error_count, 1);
    }

    #[test]
    fn routed_count_counts_can_addresses() {
        let mappings = vec![ProtocolPointMapping {
            point_id: 8001,
            device_id: 9,
            protocol_addr: ProtocolAddress::Can {
                can_id: 0x18FF50E5,
                start_byte: 0,
                length: 8,
            },
            data_type: eneros_upa_model::PointType::Analog,
            scale: 0.1,
            offset: 0.0,
        }];
        let mut reg = eneros_protocol_abstract::VecPointRegistry::new(mappings);
        let mut p = Gbt27930Plugin::new();
        p.start(&full_cfg(), &mut NoTransportFactory, &mut reg)
            .expect("start");
        assert_eq!(p.routed_count(), 1);
        let line = p.debug_line().unwrap_or_default();
        assert!(line.contains("BLOCKED"));
        assert!(line.contains("GB/T 27930"));
    }

    #[test]
    fn read_point_unsupported_and_stop_clears_state() {
        let mut p = Gbt27930Plugin::new();
        p.start(
            &full_cfg(),
            &mut NoTransportFactory,
            &mut EmptyPointRegistry,
        )
        .expect("start");
        let m = ProtocolPointMapping {
            point_id: 8001,
            device_id: 9,
            protocol_addr: ProtocolAddress::Can {
                can_id: 1,
                start_byte: 0,
                length: 8,
            },
            data_type: eneros_upa_model::PointType::Analog,
            scale: 1.0,
            offset: 0.0,
        };
        assert_eq!(p.read_point(0, &m), Err(ProtocolError::Unsupported));
        p.stop().expect("stop");
        assert_eq!(p.health().state, AdapterState::Uninitialized);
        assert_eq!(p.phase(), ChargePhase::Idle);
        assert_eq!(
            p.poll(
                0,
                WorkBudget {
                    max_work_ms: 1,
                    max_polls: 1
                }
            ),
            Err(ProtocolError::ProtocolNotStarted)
        );
    }
}
