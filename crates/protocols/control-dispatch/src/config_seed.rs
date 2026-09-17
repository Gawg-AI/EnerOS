//! DispatchConfig 装配种子（蓝图 69.4 WP-69.4.4）。
//!
//! 把蓝图 64 WP-64.3.3.1 的 `TestConfigSeed` 模式（少数字段可变、其余取
//! 安全默认）提升到 src 侧：身份事实（链/钥/档案/fencing token/签发
//! 服务/时钟）+ 注入面（intent 存储/投递通道/资源与合同视图）由装配方
//! 声明，其余字段由 [`ConfigSeed::assemble`] 以**安全默认**单源填充。
//! 默认值 = settlement_lock base_config 公共值（护栏窗/爬坡/限功率/
//! gate/feedback_mapping/target_endpoint），由 tests/support_defaults.rs
//! 变异靶锁锁定——默认值漂移即红。

use alloc::string::String;

use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::ControlProfile;
use eneros_dual_brain::gate::DecisionGate;

use crate::channel::DeliveryChannel;
use crate::engine::{
    AssetRoute, AuditSinks, DeliveryBinding, DispatchConfig, IssuerIdentity, SafetyLimits,
};
use crate::feedback::FeedbackPointMapping;
use crate::intent::IntentStore;

/// DispatchConfig 装配种子：身份事实 + 注入面；其余字段安全默认。
pub struct ConfigSeed {
    /// 授权链。
    pub chain: ControlChain,
    /// 信封签名密钥（与 chain gateway 同源）。
    pub key: alloc::vec::Vec<u8>,
    /// 同源档案（蓝图 55）。
    pub profile: ControlProfile,
    /// 首 fencing token。
    pub fencing_token: u64,
    /// 签发服务标识。
    pub issuer_service: [u8; 16],
    /// 单调时钟（毫秒）。
    pub now_fn: fn() -> u64,
    /// durable intent 存储（测试：`InMemoryIntentStore`）。
    pub intent_log: alloc::boxed::Box<dyn IntentStore>,
    /// 投递通道（生产：Sel4EipcChannel；测试：内存通道）。
    pub delivery: alloc::boxed::Box<dyn DeliveryChannel>,
    /// 资源注册表只读视图（缺装配 = `EmptyResourceRegistry` fail-closed）。
    pub resources: alloc::boxed::Box<dyn eneros_decision_contract::ResourceRegistry + 'static>,
    /// 控制合同编译视图（缺装配 = 空视图 fail-closed）。
    pub contracts: alloc::boxed::Box<dyn crate::compiler::PlanContractView + 'static>,
    /// 蓝图 70 R10：per-asset 扩展执行路由（多资产；单资产装配留空——
    /// 链实例由装配方以同源档案预建，语义与 `chain` 字段一致）。
    pub extra_routes: alloc::vec::Vec<AssetRoute>,
}

impl ConfigSeed {
    /// 组装 `DispatchConfig`：安全默认单源（差异字段由装配方以结构更新
    /// 语法覆盖，完整字面量只在此处）。
    pub fn assemble(self) -> DispatchConfig {
        DispatchConfig {
            extra_routes: self.extra_routes,
            issuer: IssuerIdentity {
                chain: self.chain,
                key: self.key,
                profile: self.profile,
                fencing_token: self.fencing_token,
                issuer_service: self.issuer_service,
                // 签发代次默认 0（生产装配与 host 夹具当前均用此默认；
                // 重启种子 PROD_EPOCH_SEED 派生的是首签 fencing token，
                // 见 control-services/dispatch_assembly，非本字段）。
                issuer_epoch: 0,
            },
            gate: DecisionGate::new(100, 1000.0, 3_600_000),
            safety: SafetyLimits {
                max_power_kw: 500.0,
                safety_voltage_window: (340.0, 420.0),
                safety_freq_window: (49.5, 50.5),
                safety_ramp_kw_per_min: 200.0,
            },
            audit_sinks: AuditSinks {
                audit: None,
                control_audit: None,
                intent_log: self.intent_log,
            },
            delivery_binding: DeliveryBinding {
                target_endpoint: String::from("svc-device-agent"),
                delivery: self.delivery,
                feedback_mapping: FeedbackPointMapping {
                    grid_up: 5,
                    soc: 4,
                    dc_bus_v: 1,
                    overcurrent: 6,
                    overtemp: 6,
                },
            },
            resources: self.resources,
            contracts: self.contracts,
            compiler_policy: crate::compiler::CompilerPolicy::default(),
            control_mode: crate::arbiter::ControlMode::Auto,
            now_fn: self.now_fn,
            trusted_input: crate::engine::TrustedInputPolicy::default(),
            // 蓝图 70.9：缺省 = 无认证来源（fail-closed）+ 空 key 环。
            // 生产/测试装配方以结构更新语法注入真实来源与环。
            certifier: alloc::boxed::Box::new(crate::certify::NoCertifier),
            certifier_ring: eneros_certifier::proof::CertifierKeyRing::empty(),
        }
    }
}
