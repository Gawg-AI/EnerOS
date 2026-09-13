//! WP-64.3.3.1 夹具默认值锁定（64.3 复查加固）：`support::test_config`
//! 安全默认的细粒度变异有牙——任何默认值漂移即红。套件特有值（KEY/
//! ISSUER/fencing/audit/链时钟/FlakyStore 等）经 seed 或结构更新注入，
//! 不在本锁面；此处只锁 seed 之外的硬编码默认（= settlement_lock
//! base_config 公共值，迁移自 10 套件旧内联字面）。
mod support;

use eneros_control_dispatch::arbiter::ControlMode;
use eneros_control_dispatch::{ChannelError, DeliveryChannel};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::ControlProfile;
use eneros_dual_brain::gate::GateDecision;

struct NopDelivery;

impl DeliveryChannel for NopDelivery {
    fn deliver(
        &mut self,
        _target: &str,
        _payload: &[u8],
        _timeout_ms: u64,
    ) -> Result<Vec<u8>, ChannelError> {
        Err(ChannelError::Timeout)
    }
}

fn profile() -> ControlProfile {
    ControlProfile::ai_pcs_setpoint(1, 10, 4, [7u8; 16], 0)
}

fn seed() -> support::TestConfigSeed {
    support::TestConfigSeed {
        chain: ControlChain::from_profile(b"support-defaults-lock-key-0123456", &profile()),
        key: Vec::new(),
        profile: profile(),
        issuer_service: [0x22; 16],
        fencing_token: 1,
        now_fn: || 1_000,
        intent_log: Box::new(eneros_control_dispatch::InMemoryIntentStore::new(8)),
        delivery: Box::new(NopDelivery),
    }
}

#[test]
fn test_config_defaults_locked() {
    // 蓝图 69.4 WP-69.4.4：字段分组后断言取径改为组路径
    // （issuer/safety/audit_sinks/delivery_binding）；断言条数与期望值
    // 逐字节不变（变异靶锁不得削弱——卷出口第 4 项）。
    let mut cfg = support::test_config_legacy(seed());
    assert_eq!(cfg.issuer.issuer_epoch, 0);
    assert_eq!(cfg.delivery_binding.target_endpoint, "svc-device-agent");
    assert_eq!(cfg.safety.max_power_kw, 500.0);
    assert_eq!(cfg.safety.safety_voltage_window, (340.0, 420.0));
    assert_eq!(cfg.safety.safety_freq_window, (49.5, 50.5));
    assert_eq!(cfg.safety.safety_ramp_kw_per_min, 200.0);
    assert!(cfg.audit_sinks.audit.is_none());
    assert!(cfg.audit_sinks.control_audit.is_none());
    assert_eq!(cfg.control_mode, ControlMode::Auto);
    let fm = &cfg.delivery_binding.feedback_mapping;
    assert_eq!(
        (fm.grid_up, fm.soc, fm.dc_bus_v, fm.overcurrent, fm.overtemp),
        (5, 4, 1, 6, 6)
    );
    // 资源/合同视图派生默认：catalog_version 1、合同可逆位 1。
    assert!(cfg
        .resources
        .asset_exists(u64::from(cfg.issuer.profile.device_id)));
    assert!(!cfg
        .resources
        .asset_exists(u64::from(cfg.issuer.profile.device_id) + 1));
    // gate 三参行为锁（DecisionGate 无 limit getter；此处检验的始终是
    // test_config 注入的实例）：100/分钟——check 不记账，以 record 填满
    // 速率窗后第 101 判拒；能量闸 1000.0（+1e-9 容差边界）放行、
    // 1000.5 越限拒。
    let mut gate = std::mem::replace(
        &mut cfg.gate,
        eneros_dual_brain::gate::DecisionGate::new(1, 1.0, 1),
    );
    for _ in 0..100 {
        assert_eq!(gate.check_rate(0), GateDecision::Allow);
        gate.record(0, 0.0);
    }
    assert_eq!(gate.check_rate(0), GateDecision::RateLimited);
    assert_eq!(gate.check_energy(0, 1000.0), GateDecision::Allow);
    assert_eq!(gate.check_energy(0, 1000.5), GateDecision::EnergyExceeded);
}
