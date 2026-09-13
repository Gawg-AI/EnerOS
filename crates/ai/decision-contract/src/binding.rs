//! 蓝图 70.8 WP-70.8.4——**候选身份绑定**（生成者与验证者共用公式）。
//!
//! # 为什么需要单源公式
//!
//! 70.8 之前 `proposal_to_candidate` 把 `objective_digest` 写成全零、
//! `step_ns` 写零、`snapshot_id` 取 `input_digest[..16]`——三者都不是
//! 可核对的身份，dispatch 入口也**无法**验证候选与当前可信输入同代。
//! 本模块给出唯一公式：候选签发的 `objective_digest` 必须等于该公式的
//! 输出，验证方（[`crate::binding`] 之外的 certifier / dispatch）用同一
//! 公式重算即可判定"候选与哪一代输入绑定"。
//!
//! # 语义边界（诚实登记）
//!
//! 迁移窗内的单步启发式路径**没有** `ObjectiveSpec`（无求解目标文档），
//! 因此本摘要绑定的是**输入代次身份**：快照 ID + 目录合同版本 + 计划
//! 时域三元组。它可被独立重算（certifier 持有同一快照/目录/计划时域），
//! 不是自证。结构化计划（70.10 起）携带完整 `ObjectiveSpec` 时，摘要
//! 升级为 `spec_digest`（同一字段，更强语义），届时本函数只作迁移读数。

/// 域分隔前缀（v1；公式冻结，改公式即改域串）。
pub const OBJECTIVE_BINDING_DOMAIN: &[u8] = b"eneros.objective-binding.v1";

/// 输入代次绑定摘要（生成者写入 `CandidatePlan.objective_digest`，
/// 验证者重算比对）。
///
/// 输入：快照 ID、目录合同版本、计划时域（起点/步长/步数）。
/// 输出：32 字节 SHA-256。
pub fn objective_binding_v1(
    snapshot_id: &[u8; 16],
    catalog_version: u64,
    horizon_start_ms: u64,
    step_ns: u64,
    step_count: u32,
) -> [u8; 32] {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(OBJECTIVE_BINDING_DOMAIN);
    h.update(snapshot_id);
    h.update(catalog_version.to_le_bytes());
    h.update(horizon_start_ms.to_le_bytes());
    h.update(step_ns.to_le_bytes());
    h.update(step_count.to_le_bytes());
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_is_deterministic_and_field_sensitive() {
        let sid = [7u8; 16];
        let a = objective_binding_v1(&sid, 1, 1_000, 60_000_000_000, 4);
        let b = objective_binding_v1(&sid, 1, 1_000, 60_000_000_000, 4);
        assert_eq!(a, b, "同输入同摘要");
        // 每个字段变化都必须改变摘要（五维逐一变异）。
        assert_ne!(
            a,
            objective_binding_v1(&[8u8; 16], 1, 1_000, 60_000_000_000, 4)
        );
        assert_ne!(a, objective_binding_v1(&sid, 2, 1_000, 60_000_000_000, 4));
        assert_ne!(a, objective_binding_v1(&sid, 1, 1_001, 60_000_000_000, 4));
        assert_ne!(a, objective_binding_v1(&sid, 1, 1_000, 60_000_000_001, 4));
        assert_ne!(a, objective_binding_v1(&sid, 1, 1_000, 60_000_000_000, 5));
        assert_ne!(a, [0u8; 32], "非零（零摘要即未绑定）");
    }
}
