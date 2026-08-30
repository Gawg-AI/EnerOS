//! 外部 crate 无法构造 CertifiedPlan：字段私有、无 pub 构造器、
//! CertifierSeal 无公开构造（transition seal 只在 dispatch 的
//! transition-certifier feature 下、经 ControlChain 复核路径发放）。
use eneros_decision_contract::{CertificateId, CertifiedPlan, PlanId};

fn main() {
    // 期望失败：CertifiedPlan 没有公开构造器（字段私有 + new_sealed 是
    // pub(crate)——外部看不见）。
    let _c = CertifiedPlan::new(
        CertificateId([9u8; 16]),
        PlanId([1u8; 16]),
        [0xAB; 32],
        1_000,
        2_000,
    );
}
