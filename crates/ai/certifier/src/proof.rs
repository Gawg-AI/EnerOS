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

use crate::CertifierError;

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
        // 输入 refs（[09] §4.3）：快照/目录版本随内容绑定——换快照或
        // 目录重评不产生相同证据摘要。
        h.update(r.snapshot_id);
        h.update(r.catalog_version.to_le_bytes());
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
    /// HMAC-SHA256(key, cert_wire ‖ plan_sha ‖ proof_digest)——覆盖证书
    /// 自身全部字段（有效期/计数/引用不可离 MAC 篡改）。
    pub mac: [u8; 32],
}

/// certifier key ring（生命周期：rotate/retire）。
///
/// - `rotate`：新 key 成为活跃；旧活跃 key 降为前代，**前代窗口 = N-1
///   （1 个）**——更早的 key 到期退出（蓝图 WP-70.9.3「N-1 窗口」）。
/// - 版本（`key_version`）= **轮换代数**（ring 级单调计数，不随 id 归零）：
///   审计可区分代际；退役后复用同一 id 也不会让旧代证书复活（版本已前进）。
/// - `retire`：立即撤销（活跃或前代）——被 retire 的 key 其一切证书验证
///   失败。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CertifierKeyRing {
    active: Option<(u16, u32, Vec<u8>)>,
    predecessors: Vec<(u16, u32, Vec<u8>)>,
    /// 轮换代数（`key_version` 单源；`new` 起于 1，`empty` 为 0）。
    generation: u32,
}

/// N-1 验证窗口：保留的前代数（最老先出）。
pub const KEY_PREDECESSOR_WINDOW: usize = 1;

impl CertifierKeyRing {
    /// 以活跃 key 构建首代 ring。
    pub fn new(key_id: u16, key: &[u8]) -> Self {
        Self {
            active: Some((key_id, 1, Vec::from(key))),
            predecessors: Vec::new(),
            generation: 1,
        }
    }

    /// 空 ring（无活跃 key、无前代）——**验方失败即拒**（fail-closed）。
    ///
    /// 用途：装配期缺 key 时的显式初值（不是"跳过验签"的开关）；发放方
    /// 不得用空 ring 出证（[`crate::CertifierModule::certify`] 对空
    /// `active()` 恒拒绝）。
    pub fn empty() -> Self {
        Self {
            active: None,
            predecessors: Vec::new(),
            generation: 0,
        }
    }

    /// 轮换：新 key 接管，版本 = 轮换代数（单调 +1），旧活跃降前代并
    /// 裁到 N-1。**同 id 复用（含历史前代）拒绝**——否则 (id, version)
    /// 二义，旧代证书不可验且审计无法区分代际。
    pub fn rotate(&mut self, new_key_id: u16, key: &[u8]) -> Result<(), CertifierError> {
        if key.is_empty() {
            return Err(CertifierError::InvalidInput("empty key"));
        }
        let dup = self
            .active
            .as_ref()
            .map(|(id, _, _)| *id)
            .filter(|id| *id == new_key_id)
            .is_some()
            || self.predecessors.iter().any(|(id, _, _)| *id == new_key_id);
        if dup {
            return Err(CertifierError::InvalidInput("key id already used"));
        }
        if let Some((old_id, old_ver, old_key)) = self.active.take() {
            self.predecessors.push((old_id, old_ver, old_key));
        }
        // N-1 窗口：最老的前代先出（保留最近 N_MINUS_ONE 个）。
        while self.predecessors.len() > KEY_PREDECESSOR_WINDOW {
            self.predecessors.remove(0);
        }
        self.generation = self.generation.saturating_add(1);
        self.active = Some((new_key_id, self.generation, Vec::from(key)));
        Ok(())
    }

    /// 立即撤销（回滚）：验证面即刻失效。
    ///
    /// 退役后 id 可被后续 `rotate` 复用，但**不会复活**旧代证书：版本 =
    /// 轮换代数且只前进，复用后的 key 版本必然高于旧证书携带的版本。
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

    /// 当前轮换代数（`key_version` 读出；`empty` = 0）。
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// 按 id 查 key（活跃 + N-1 前代；retired/超窗不可达）。
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

/// mac 输入规范（签发/验证单源）：证书规范 wire（覆盖全部自有字段）
/// ‖ plan_sha ‖ proof_digest，域分隔前缀防跨上下文重放。
pub fn mac_input(cert_wire: &[u8], plan_sha: &[u8; 32], proof_digest: &[u8; 32]) -> Vec<u8> {
    let mut input = Vec::with_capacity(cert_wire.len() + 64 + 16);
    input.extend_from_slice(b"eneros.cert-mac.v2");
    input.extend_from_slice(cert_wire);
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
        // 输入 refs 绑定：snapshot/catalog 变化必变（[09] §4.3）。
        let mut s = r(1.0);
        s.snapshot_id = [2; 16];
        assert_ne!(a, digest_records(&[s], &plan_sha, 1), "快照变化 → 摘要变化");
        let mut v = r(1.0);
        v.catalog_version = 99;
        assert_ne!(
            a,
            digest_records(&[v], &plan_sha, 1),
            "目录版本变化 → 摘要变化"
        );
    }

    #[test]
    fn keyring_lifecycle() {
        let mut ring = CertifierKeyRing::new(1, b"key-one");
        assert!(ring.key(1).is_some());
        assert_eq!(ring.generation(), 1);
        // 轮换：旧 key 仍可验（N-1 前代窗口）。
        ring.rotate(2, b"key-two").unwrap();
        assert!(ring.key(1).is_some());
        assert_eq!(ring.active().map(|(id, _, _)| id), Some(2));
        assert_eq!(ring.generation(), 2, "版本 = 轮换代数（单调）");
        // 同 id 复用拒绝（active 或历史前代均已占用 id 1/2）。
        assert!(ring.rotate(1, b"key-one-again").is_err());
        assert!(ring.rotate(2, b"key-two-again").is_err());
        // 空 key 拒绝。
        assert!(ring.rotate(3, b"").is_err());
        // 撤销旧 key：验证面即刻失效。
        assert!(ring.retire(1));
        assert!(ring.key(1).is_none());
        // 撤销活跃：回滚——无活跃 key。
        assert!(ring.retire(2));
        assert!(ring.active().is_none());
        // 退役后 id 可复用（回滚语义），但版本只前进——旧代证书不复活。
        assert_eq!(ring.generation(), 2);
        ring.rotate(1, b"key-one-new").unwrap();
        assert_eq!(
            ring.active().map(|(_, v, _)| v),
            Some(3),
            "复用 id 的版本必须高于退役前的旧证书版本（防复活）"
        );
    }

    /// 蓝图 WP-70.9.3「N-1 窗口」：只保留最近 1 个前代，更早的 key 退出
    /// 验证面；`generation` 单调（审计可辨代际）。
    ///
    /// 变异靶：去掉 rotate 里的 `truncate/remove` → 第 1 个 key 仍可验 → 红。
    #[test]
    fn keyring_keeps_only_n_minus_one_predecessor() {
        let mut ring = CertifierKeyRing::new(1, b"k1");
        ring.rotate(2, b"k2").unwrap();
        assert!(ring.key(1).is_some(), "N-1 窗口内仍可验");
        ring.rotate(3, b"k3").unwrap();
        assert!(ring.key(1).is_none(), "超出 N-1 窗口的 key 必须退出验证面");
        assert!(ring.key(2).is_some(), "最近前代仍在窗口内");
        assert_eq!(ring.active().map(|(id, _, _)| id), Some(3));
        assert_eq!(ring.generation(), 3);
        assert_eq!(ring.active().map(|(_, v, _)| v), Some(3));
    }

    /// 退役后复用 id 不得复活旧代证书（版本单调的单向性实证）。
    #[test]
    fn retired_key_id_reuse_cannot_resurrect_old_version() {
        let mut ring = CertifierKeyRing::new(1, b"same-material");
        let old_version = ring.active().expect("active").1;
        assert!(ring.retire(1));
        ring.rotate(1, b"same-material").expect("id reuse allowed");
        let new_version = ring.active().expect("active").1;
        assert!(
            new_version > old_version,
            "复用 id 必须拿到更高的版本号（旧证书版本失配 → 拒绝）: {old_version} -> {new_version}"
        );
    }
}
