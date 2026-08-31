//! 证据记录、证书 key 绑定与 key 生命周期（[09] §4.3/§4.7）。
//!
//! - proof Blob：逐约束 {约束种类、资产、评估 lhs/rhs、单位、容差、
//!   pass/fail}；proof digest = SHA-256(规范编码 ‖ plan_sha ‖ 版本)。
//! - 证书绑定：站内 certifier key 的 HMAC-SHA256（RFC 2104，实现于
//!   decision-contract::keyed，RFC 4231 向量锁定）——只证明认证产物
//!   来源，不替代 Gateway 权限裁决。
//! - key 生命周期：rotate（旧 key 保留为前代，N-1 验证窗口内可验）/
//!   retire（立即回滚——该 key 一切验证失败）。

use alloc::vec::Vec;

use sha2::Digest;

/// 判定方向（保守方向：Ge = lhs ≥ rhs − tol；Le = lhs ≤ rhs + tol）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    /// lhs ≥ rhs − tol。
    Ge,
    /// lhs ≤ rhs + tol。
    Le,
    /// |lhs − rhs| ≤ tol。
    Eq,
}

impl Cmp {
    pub const fn code(self) -> u8 {
        match self {
            Self::Ge => 0,
            Self::Le => 1,
            Self::Eq => 2,
        }
    }
}

/// 单条约束证据（[09] §4.3 proof Blob 字段集）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProofRecord {
    /// 证据序号（评估顺序；确定性）。
    pub seq: u32,
    /// ConstraintKind 码。
    pub kind: u16,
    /// 资产 id（站级 = 0）。
    pub asset_id: u64,
    /// 评估值（范围类 = 轨迹最坏值；推进类 = 末态）。
    pub lhs: f64,
    /// 界（rhs；按 [`Cmp`] 方向保守判定）。
    pub rhs: f64,
    /// 判定方向。
    pub cmp: Cmp,
    /// 输入快照 id（[09] §4.3：证据引用输入面）。
    pub snapshot_id: [u8; 16],
    /// 约束/资源目录版本（plan.resource_catalog_version 同源）。
    pub catalog_version: u64,
    /// 单位码（UnitCatalog）。
    pub unit_code: u16,
    /// 判定容差。
    pub tolerance: f64,
    /// pass/fail（fail 即认证拒绝——记录保留供诊断重放）。
    pub pass: bool,
}

/// 证据摘要（确定性；CandidatePlan 不可篡改绑定的证据面）。
pub fn digest_records(
    records: &[ProofRecord],
    plan_sha: &[u8; 32],
    certifier_version: u32,
) -> [u8; 32] {
    let mut h = sha2::Sha256::new();
    h.update(b"eneros.certifier-proof.v1");
    h.update(certifier_version.to_le_bytes());
    h.update(plan_sha);
    h.update((records.len() as u32).to_le_bytes());
    for r in records {
        h.update(r.seq.to_le_bytes());
        h.update(r.kind.to_le_bytes());
        h.update(r.asset_id.to_le_bytes());
        h.update(r.lhs.to_le_bytes());
        h.update(r.rhs.to_le_bytes());
        h.update([r.cmp.code()]);
        h.update(r.unit_code.to_le_bytes());
        h.update(r.tolerance.to_le_bytes());
        h.update([r.pass as u8]);
    }
    h.finalize().into()
}

/// key 绑定信封（Gateway 验证输入）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CertificateEnvelope {
    /// certifier key id。
    pub key_id: u16,
    /// key 版本（同 id 换key须递增；版本不符拒绝）。
    pub key_version: u32,
    /// HMAC-SHA256(key, plan_sha ‖ proof_digest)。
    pub mac: [u8; 32],
}

/// certifier key ring（生命周期：rotate/retire/回滚）。
///
/// - `rotate`：新 key 成为活跃；旧 key 降为前代（N-1 验证窗口内仍可验）。
/// - `retire`：立即撤销（活跃或前代）——**回滚裁决**：被 retire 的 key
///   其一切证书验证失败。
pub struct CertifierKeyRing {
    active: Option<(u16, u32, Vec<u8>)>,
    predecessors: Vec<(u16, u32, Vec<u8>)>,
}

impl CertifierKeyRing {
    /// 以活跃 key 构建首代 ring。
    pub fn new(key_id: u16, key: &[u8]) -> Self {
        Self {
            active: Some((key_id, 1, Vec::from(key))),
            predecessors: Vec::new(),
        }
    }

    /// 轮换：新 id 的 key 接管；旧活跃 key 降前代（版本 +1 语义为同 id
    /// 换代；新 id 从版本 1 起）。
    pub fn rotate(&mut self, new_key_id: u16, key: &[u8]) {
        if let Some((old_id, old_ver, old_key)) = self.active.take() {
            self.predecessors.push((old_id, old_ver, old_key));
        }
        self.active = Some((new_key_id, 1, Vec::from(key)));
    }

    /// 立即撤销（回滚）：验证面即刻失效。
    pub fn retire(&mut self, key_id: u16) -> bool {
        let mut retired = false;
        if self.active.as_ref().map(|(id, _, _)| *id) == Some(key_id) {
            self.active = None;
            retired = true;
        }
        let before = self.predecessors.len();
        self.predecessors.retain(|(id, _, _)| *id != key_id);
        retired || before != self.predecessors.len()
    }

    /// 活跃 key（id, version, key）。
    pub fn active(&self) -> Option<(u16, u32, &[u8])> {
        self.active
            .as_ref()
            .map(|(id, v, k)| (*id, *v, k.as_slice()))
    }

    /// 按 id 查 key（活跃 + 前代；retired 不可达）。
    pub fn key(&self, key_id: u16) -> Option<(u16, u32, &[u8])> {
        if let Some((id, v, k)) = self.active.as_ref().filter(|(id, _, _)| *id == key_id) {
            return Some((*id, *v, k.as_slice()));
        }
        self.predecessors
            .iter()
            .find(|(id, _, _)| *id == key_id)
            .map(|(id, v, k)| (*id, *v, k.as_slice()))
    }
}

/// mac 输入规范（签发/验证单源）。
pub fn mac_input(plan_sha: &[u8; 32], proof_digest: &[u8; 32]) -> Vec<u8> {
    let mut input = Vec::with_capacity(64);
    input.extend_from_slice(plan_sha);
    input.extend_from_slice(proof_digest);
    input
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;

    #[test]
    fn record_digest_binds_content_and_order() {
        let r = |lhs: f64| ProofRecord {
            seq: 0,
            kind: 3,
            asset_id: 1,
            lhs,
            rhs: 100.0,
            cmp: Cmp::Le,
            snapshot_id: [1; 16],
            catalog_version: 1,
            unit_code: 6,
            tolerance: 1e-3,
            pass: true,
        };
        let plan_sha = [7u8; 32];
        let a = digest_records(&[r(1.0)], &plan_sha, 1);
        let b = digest_records(&[r(1.0)], &plan_sha, 1);
        assert_eq!(a, b);
        let c = digest_records(&[r(2.0)], &plan_sha, 1);
        assert_ne!(a, c, "评估值变化 → 证据摘要变化");
        let d = digest_records(&[r(1.0)], &plan_sha, 2);
        assert_ne!(a, d, "版本变化 → 证据摘要变化");
    }

    #[test]
    fn keyring_lifecycle() {
        let mut ring = CertifierKeyRing::new(1, b"key-one");
        assert!(ring.key(1).is_some());
        // 轮换：旧 key 仍可验（前代）。
        ring.rotate(2, b"key-two");
        assert!(ring.key(1).is_some());
        assert_eq!(ring.active().map(|(id, _, _)| id), Some(2));
        // 撤销旧 key：验证面即刻失效。
        assert!(ring.retire(1));
        assert!(ring.key(1).is_none());
        // 撤销活跃：回滚——无活跃 key。
        assert!(ring.retire(2));
        assert!(ring.active().is_none());
    }
}
