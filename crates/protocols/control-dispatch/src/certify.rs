//! 蓝图 70.9 WP-70.9.1/70.9.4——**独立认证来源**接缝与执行准入验真。
//!
//! # 责任边界
//!
//! - 生成者（host planner / 启发式后端 / 任何提案者）**拿不到**证书发放
//!   能力：本模块只定义"向独立认证者索取证书"的接缝与"验真"判据，不含
//!   任何发放路径（发放只在 `eneros-certifier` crate 内，且需 `certifier`
//!   feature 的封印）。
//! - 执行者（本引擎）在编译/投递前**逐字段验真**：计划-证书绑定、
//!   key id/version、HMAC、有效期、独立重算证据（`hard_constraint_count`
//!   > 0）。任何一项不成立 → 整批拒绝（零投递）。
//! - 过渡发放面（`transition-certifier` / `CertifierSeal::issue_transition`）
//!   已按 ADR-051 出口条件删除：没有"证书缺失就自动启用旧 seal"的分支。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_decision_contract::{CandidatePlan, CertifiedPlan};

/// 认证者应答（wire 中立：证书字节 + MAC 三件套）。
///
/// 证书本体 = `CertifiedPlan::encode_v1()`；MAC = 独立认证者对
/// `cert_wire ‖ plan_sha ‖ proof_digest` 的 HMAC（`cert-mac.v2` 域）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertifierReply {
    /// `CertifiedPlan` V1 wire。
    pub certificate: Vec<u8>,
    /// 发放 key id。
    pub key_id: u16,
    /// 发放 key 版本（验方按 N-1 窗口接受）。
    pub key_version: u32,
    /// HMAC-SHA256（`eneros.cert-mac.v2` 域）。
    pub mac: [u8; 32],
}

/// 认证请求被拒（结构化；调用方零投递）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertifyReject {
    /// 认证者不可达/未装配（进程失联、预算超时、OOM）。
    Unavailable(String),
    /// 认证者明确拒绝（输入不全/约束不满足/计划无效）。
    Rejected(String),
}

impl core::fmt::Display for CertifyReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "certifier-unavailable:{s}"),
            Self::Rejected(s) => write!(f, "certifier-rejected:{s}"),
        }
    }
}

/// 独立认证来源接缝（生产 = 独立认证者进程；测试 = 进程内认证器）。
///
/// 契约：输入 = 候选计划 wire + 当前时刻；输出 = 证书或具名拒绝。
/// 实现方**不得**读取设备写通道、不得修改计划（只认证）。
pub trait CertificateSource {
    fn certify(&mut self, plan_wire: &[u8], now_ms: u64) -> Result<CertifierReply, CertifyReject>;

    /// 验方 key 环（可选）：认证者自带环时，**验真以它为准**——轮换/退役
    /// 在发放侧发生即刻对验方生效（否则发放与验真各持一份拷贝，退役只能
    /// 靠带外同步；WP-70.9.3 的"退役立即失效"就会退化成"下次重启才失效"）。
    /// 缺省 `None` = 用引擎侧装配的环。
    fn verifier_ring(&self) -> Option<&eneros_certifier::proof::CertifierKeyRing> {
        None
    }
}

/// 未装配认证来源：一律不可用（fail-closed；不得回退过渡发放）。
pub struct NoCertifier;

impl CertificateSource for NoCertifier {
    fn certify(
        &mut self,
        _plan_wire: &[u8],
        _now_ms: u64,
    ) -> Result<CertifierReply, CertifyReject> {
        Err(CertifyReject::Unavailable(String::from(
            "certifier-not-wired（ADR-051 过渡发放面已删除，无回退分支）",
        )))
    }
}

/// 执行准入验真判据（引擎调用；返回 `Ok(())` 方可进入编译/投递）。
///
/// 1. 证书解码（wire 自校验：magic/版本/字段合法性）；
/// 2. 计划-证书绑定：`cert.plan_sha256 == plan_wire_sha256(plan_wire)` 且
///    `cert.plan_id == plan.id()`；
/// 3. MAC 验真（key id/version 在环内 + HMAC 恒时比较）；
/// 4. 有效期（`now <= cert.valid_until_ms`）；
/// 5. 独立重算证据：`hard_constraint_count > 0`（过渡 seal 恒 0，删除后
///    不可能通过——"没有为了可用性放宽"的可核对形式）。
pub fn verify_reply(
    plan: &CandidatePlan,
    plan_wire: &[u8],
    reply: &CertifierReply,
    ring: &eneros_certifier::proof::CertifierKeyRing,
    now_ms: u64,
) -> Result<CertifiedPlan, String> {
    let cert = CertifiedPlan::decode_v1(&reply.certificate)
        .map_err(|e| alloc::format!("cert-wire:{e:?}"))?;
    let plan_sha = CandidatePlan::plan_wire_sha256_of_wire(plan_wire);
    if cert.plan_sha256() != &plan_sha {
        return Err(String::from("cert-plan-sha-mismatch"));
    }
    if cert.plan_id() != plan.id() {
        return Err(String::from("cert-plan-id-mismatch"));
    }
    if !cert.is_valid_at(now_ms) {
        return Err(String::from("cert-expired"));
    }
    if cert.hard_constraint_count() == 0 {
        return Err(String::from(
            "cert-no-independent-eval（零约束记录：过渡发放面已删除，不接受）",
        ));
    }
    let envelope = eneros_certifier::proof::CertificateEnvelope {
        key_id: reply.key_id,
        key_version: reply.key_version,
        mac: reply.mac,
    };
    eneros_certifier::CertifierModule::verify(&cert, plan_wire, &envelope, ring, now_ms)
        .map_err(|e| alloc::format!("cert-verify:{e:?}"))?;
    Ok(cert)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)] // 测试内判定 panic（clippy.toml 测试模块豁免）

    use super::*;

    #[test]
    fn no_certifier_is_fail_closed_with_explicit_reason() {
        let mut c = NoCertifier;
        let e = c.certify(&[1, 2, 3], 0).unwrap_err();
        match e {
            CertifyReject::Unavailable(ref s) => {
                assert!(s.contains("过渡发放面已删除"), "{s}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// 过渡发放面已删除：`issue_transition` / `new_transition` 两个符号
    /// 在本文件（执行准入面）零在场——字面锁（回归即红）。
    #[test]
    fn transition_seal_surface_is_absent() {
        let src = include_str!("certify.rs");
        // 断言目标在源码文本中拼接而成（本测试自身不含被判字面）。
        let needle_a = ["issue", "trans", "ition"].concat();
        let needle_b = ["new", "trans", "ition"].concat();
        assert!(
            !src.contains(&needle_a),
            "过渡发放面回归（ADR-051 出口已关）"
        );
        assert!(!src.contains(&needle_b), "过渡构造面回归");
    }
}
