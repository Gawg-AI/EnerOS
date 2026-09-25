//! 集成测试共享 DTO：最小资源注册表视图（绑定来自 profile 常数——与
//! 迁移期 `ProfileResourceView` 同构；生产装配注入 resource-registry
//! `PlannerView`，见 [08] §4.8）+ WP-5.1 合同编译视图。

use eneros_control_dispatch::compiler::{MapContractView, PlanContract};
use eneros_decision_contract::ResourceRegistry;

/// 测试视图（asset=设备位、device/point/unit 由调用方给）。
pub struct TestResourceView {
    pub asset_id: u64,
    pub device_id: u16,
    pub point_id: u32,
    pub unit_code: u16,
    pub catalog_version: u64,
}

impl ResourceRegistry for TestResourceView {
    fn asset_exists(&self, asset_id: u64) -> bool {
        asset_id == self.asset_id
    }

    fn asset_control_binding(&self, asset_id: u64) -> Option<(u16, u32, u16)> {
        if asset_id == self.asset_id {
            Some((self.device_id, self.point_id, self.unit_code))
        } else {
            None
        }
    }

    fn catalog_version(&self) -> u64 {
        self.catalog_version
    }
}

/// 测试合同视图（WP-5.1）：单资产 setpoint 合同（类 2 = setpoint、
/// 值域 ±500、可逆、回读点 1001——与 configs/catalog.toml contract 1
/// 同形）。`reversible=0` 变体供不可逆语义用例。
pub fn test_contract_view(asset_id: u64, reversible: u8) -> MapContractView {
    let mut v = MapContractView::new();
    v.insert(
        asset_id,
        PlanContract {
            class: 2,
            readback_point_id: 1001,
            reversible,
            min: -500.0,
            max: 500.0,
            settle_ms: 0,
            compensable: false,
        },
    );
    v
}

// ---------------------------------------------------------------------------
// WP-64.3.3.1 · DispatchConfig 测试夹具单源
// ---------------------------------------------------------------------------

use eneros_control_dispatch::intent::IntentStore;
use eneros_control_dispatch::{DeliveryChannel, DispatchConfig};
use eneros_control_gateway::chain::ControlChain;
use eneros_control_gateway::ControlProfile;

/// 套件身份组：每套件**刻意隔离**的本地事实（KEY/ISSUER/chain/profile/
/// clock 各文件本地常量）+ 两个注入面。其余字段由 [`test_config`] 安全
/// 默认填充；套件差异经结构更新语法只声明所变字段。
pub struct TestConfigSeed {
    pub chain: ControlChain,
    pub key: Vec<u8>,
    pub profile: ControlProfile,
    pub issuer_service: [u8; 16],
    pub fencing_token: u64,
    pub now_fn: fn() -> u64,
    pub intent_log: Box<dyn IntentStore>,
    pub delivery: Box<dyn DeliveryChannel>,
}

/// DispatchConfig 测试夹具单源——蓝图 69.4 WP-69.4.4：主体委托 src 侧
/// [`eneros_control_dispatch::ConfigSeed`] 装配器（安全默认单源，由
/// support_defaults.rs 变异靶锁锁定）；本函数只补测试视图派生
/// （resources/contracts 由 seed.profile 派生——与各套件原字面同构：
/// asset = profile.device_id、catalog_version 1、合同可逆）。
pub fn test_config(seed: TestConfigSeed) -> DispatchConfig {
    let profile = seed.profile;
    eneros_control_dispatch::ConfigSeed {
        chain: seed.chain,
        key: seed.key,
        fencing_token: seed.fencing_token,
        issuer_service: seed.issuer_service,
        now_fn: seed.now_fn,
        intent_log: seed.intent_log,
        delivery: seed.delivery,
        resources: Box::new(TestResourceView {
            asset_id: u64::from(profile.device_id),
            device_id: profile.device_id as u16,
            point_id: profile.point_id,
            unit_code: eneros_telemetry_model::unit_catalog::code_of(profile.unit),
            catalog_version: 1,
        }),
        contracts: Box::new(test_contract_view(u64::from(profile.device_id), 1)),
        profile,
        // 测试夹具默认单路由（历史形态逐字节）；多资产套件显式覆盖
        //（DispatchConfig::extra_routes 结构更新注入）。
        extra_routes: Vec::new(),
    }
    .assemble()
}

/// 测试轨装配（迁移/差分轨道：旧派生身份放行）。生产装配不经本函数。
pub fn test_config_legacy(seed: TestConfigSeed) -> DispatchConfig {
    with_fixture_certifier(legacy_rail(test_config(seed)))
}

/// C2 迁移轨构造器（2026-09-24）：legacy facade `engine.submit(proposal)`
/// 退役后，LegacyMigration 轨测试以本函数把夹具 proposal 转 typed 候选，
/// 再走 `submit_typed`。身份 = 派生伪快照（unbound adapter 单源）、
/// 目录版本 1（与旧 `run` 内联的 catalog 常量同值）；绑定全零——编译期
/// 经 ResourceRegistry 解析（与 v1 wire 臂/stub 后端同型）。
/// Required 轨用它会被 `admit_trusted_input` 拒绝——那正是 typed_chain
/// 差分锁（Required 拒无身份候选）的意图，勿在此补偿。
// （跨二进制共享模块：未迁移套件（audit/intent_ingress/support_defaults）
// 不使用本 helper，各二进制独立编译时按未用处理。）
#[allow(dead_code)]
pub fn legacy_candidate(
    plan: &eneros_dual_brain::decision::ProposedControlPlan,
) -> eneros_decision_contract::CandidatePlan {
    eneros_control_dispatch::adapter::proposal_to_candidate_unbound(plan, 1)
        .expect("legacy cand（夹具 solve_status=Optimal、ttl≥1——构造期恒过）")
}

/// 测试轨可信输入策略（**显式声明**）：迁移/差分轨允许旧派生身份。
/// 生产装配（control-services `dispatch_assembly`）走 `Required`。
pub fn legacy_rail(config: DispatchConfig) -> DispatchConfig {
    DispatchConfig {
        trusted_input: eneros_control_dispatch::TrustedInputPolicy::LegacyMigration,
        ..config
    }
}

// ============================================================================
// 蓝图 70.9：测试夹具认证来源（真实评估器出证 + 真实验签环）
// ============================================================================

/// 夹具认证来源（与生产 `TrustedInputCertifier` 同契约）。
///
/// 证书由 `eneros_certifier::FixtureCertifier` 用**真实**评估器签发
/// （逐约束记录 + HMAC）；身份输入取自被认证的计划。
pub struct FixtureCertifier {
    inner: eneros_certifier::FixtureCertifier,
}

impl FixtureCertifier {
    /// 构造（key 材）。
    pub fn new(key: &[u8]) -> Self {
        Self {
            inner: eneros_certifier::FixtureCertifier::new(key, 100.0, -500.0, 500.0, 0.5),
        }
    }

    /// 验方环（引擎 `certifier_ring` 注入）。
    pub fn ring(&self) -> eneros_certifier::proof::CertifierKeyRing {
        self.inner.ring().clone()
    }
}

impl eneros_control_dispatch::CertificateSource for FixtureCertifier {
    fn certify(
        &mut self,
        plan_wire: &[u8],
        now_ms: u64,
    ) -> Result<eneros_control_dispatch::CertifierReply, eneros_control_dispatch::CertifyReject>
    {
        let plan = eneros_decision_contract::CandidatePlan::decode_v1(plan_wire)
            .map_err(|e| eneros_control_dispatch::CertifyReject::Rejected(format!("{e:?}")))?;
        let r = self
            .inner
            .certify(&plan, now_ms)
            .map_err(|e| eneros_control_dispatch::CertifyReject::Rejected(format!("{e:?}")))?;
        Ok(eneros_control_dispatch::CertifierReply {
            certificate: r.certificate,
            key_id: r.key_id,
            key_version: r.key_version,
            mac: r.mac,
        })
    }
}

/// 夹具 key 材（测试固定值；生产走 KMS 域分隔派生）。
pub const FIXTURE_CERT_KEY: &[u8] = b"fixture-cert-key-0123456789abcdef";

/// 给已装配的配置注入夹具认证来源 + 验方环（测试单源）。
pub fn with_fixture_certifier(config: DispatchConfig) -> DispatchConfig {
    let cert = FixtureCertifier::new(FIXTURE_CERT_KEY);
    let ring = cert.ring();
    DispatchConfig {
        certifier: Box::new(cert),
        certifier_ring: ring,
        ..config
    }
}
