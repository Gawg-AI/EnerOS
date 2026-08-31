//! 模型注册表（[11] §3.3/§3.7）：注册（digest/签名/有效期/runtime/
//! 硬件预算逐项门）、生命周期簿、推理资格、回滚、重启恢复快照。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_crypto::sm2::keypair::Sm2PublicKey;

use crate::error::HarnessError;
use crate::eval::EvalSuite;
use crate::lifecycle::{
    HealthSignal, Lifecycle, LifecyclePolicy, OfflineEvalReport, PackageLifecycle, RollbackAction,
};
use crate::manifest::{ModelManifestV1, ModelPackage, PackageContents};

/// 注册表条目（内容字节注册时逐一验证；inference 前置再验模型字节）。
#[derive(Debug, Clone, PartialEq)]
pub struct PackageEntry {
    pub package: ModelPackage,
    pub model: Vec<u8>,
    pub tokenizer: Vec<u8>,
    pub eval_suite: Option<EvalSuite>,
    pub lifecycle: PackageLifecycle,
}

/// 恢复内容加载回调：包 id → (模型字节, tokenizer 字节)（恢复时重验摘要）。
pub type ContentLoader<'a> = &'a mut dyn FnMut(&[u8; 16]) -> Option<(Vec<u8>, Vec<u8>)>;

/// 模型注册表（宿主场景：一个注册表对应一个 runtime）。
pub struct ModelRegistry {
    pub runtime_id: u32,
    pub runtime_version: u32,
    pub hardware_ram_bytes: u64,
    pub policy: LifecyclePolicy,
    pub signer_keys: BTreeMap<u32, Sm2PublicKey>,
    entries: BTreeMap<[u8; 16], PackageEntry>,
}

impl ModelRegistry {
    pub fn new(
        runtime_id: u32,
        runtime_version: u32,
        hardware_ram_bytes: u64,
        policy: LifecyclePolicy,
    ) -> Self {
        Self {
            runtime_id,
            runtime_version,
            hardware_ram_bytes,
            policy,
            signer_keys: BTreeMap::new(),
            entries: BTreeMap::new(),
        }
    }

    pub fn add_signer_key(&mut self, key_id: u32, pk: Sm2PublicKey) {
        self.signer_keys.insert(key_id, pk);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn entry(&self, id: &[u8; 16]) -> Option<&PackageEntry> {
        self.entries.get(id)
    }
    pub fn all(&self) -> impl Iterator<Item = &PackageEntry> {
        self.entries.values()
    }

    /// 注册（[11] §3.1 唯一不变量：模型包、tokenizer、skill、schema、
    /// 评测、有效期、签名、运行时、硬件预算全部匹配才能进入 registry）。
    #[allow(clippy::too_many_arguments)]
    pub fn register(
        &mut self,
        manifest: &ModelManifestV1,
        contents: &PackageContents<'_>,
        eval_suite: Option<&EvalSuite>,
        now_ns: u64,
    ) -> Result<(), HarnessError> {
        let package = ModelPackage::new(manifest.clone())?;
        // 签名门：key 必须已知且验签通过。
        let pk = self
            .signer_keys
            .get(&manifest.signer_key_id)
            .ok_or(HarnessError::Manifest("unknown signer key"))?;
        if !manifest.verify_signature(pk) {
            return Err(HarnessError::Manifest("signature invalid"));
        }
        // 有效期门。
        if !manifest.is_valid_at_ns(now_ns) {
            return Err(HarnessError::PackageExpired);
        }
        // runtime 门：id 相等 + harness 版本 ≥ 模型所需版本。
        if manifest.runtime_id != self.runtime_id {
            return Err(HarnessError::RuntimeMismatch);
        }
        if self.runtime_version < manifest.runtime_version {
            return Err(HarnessError::RuntimeMismatch);
        }
        // 硬件预算门（[11] §3.3：memory_limit_bytes 必须满足）。
        if manifest.memory_limit_bytes > self.hardware_ram_bytes {
            return Err(HarnessError::HardwareInsufficient);
        }
        // 内容摘要门（模型/tokenizer/评测）。
        contents.verify(manifest)?;
        let eval_suite = match eval_suite {
            Some(s) => {
                if !s.matches(manifest) {
                    return Err(HarnessError::Manifest("eval_suite digest mismatch"));
                }
                Some(s.clone())
            }
            None => return Err(HarnessError::Manifest("eval_suite required")),
        };
        if self.entries.contains_key(&manifest.package_id) {
            return Err(HarnessError::Manifest("duplicate package_id"));
        }
        // 回滚登记：新包默认指向当前 Approved（若有）。
        let rollback_to = self.active().map(|e| e.package.manifest.package_id);
        self.entries.insert(
            manifest.package_id,
            PackageEntry {
                package,
                model: contents.model.unwrap_or_default().to_vec(),
                tokenizer: contents.tokenizer.unwrap_or_default().to_vec(),
                eval_suite,
                lifecycle: PackageLifecycle::registered(manifest.package_id, rollback_to),
            },
        );
        Ok(())
    }

    /// 当前 Approved 包（有效期窗内；Shadow/Canary 不算 active）。平局
    /// 取更新包（valid_from 相同 → package_id 字典序更大者——回滚语义
    /// 下「最新 Approved 为 active」确定性成立）。
    pub fn active(&self) -> Option<&PackageEntry> {
        self.entries
            .values()
            .filter(|e| e.lifecycle.state == Lifecycle::Approved)
            .max_by_key(|e| {
                (
                    e.package.manifest.valid_from_ns,
                    e.package.manifest.package_id,
                )
            })
    }

    /// 推理资格（[11] §3.7：Shadow/Canary/Approved 可推理；注册/离线/
    /// 暂挂/退役拒绝）。返回 (entry, state)。
    pub fn prepare_inference(
        &mut self,
        id: &[u8; 16],
        now_ns: u64,
    ) -> Result<(&PackageEntry, Lifecycle), HarnessError> {
        let entry = self.entries.get(id).ok_or(HarnessError::UnknownPackage)?;
        if !entry.lifecycle.state.may_infer() {
            return Err(HarnessError::NotApproved(entry.lifecycle.state.stage()));
        }
        if !entry.package.manifest.is_valid_at_ns(now_ns) && now_ns > 0 {
            return Err(HarnessError::PackageExpired);
        }
        // 推理前置再验模型字节（[11] §3.1 全程匹配）。
        let model_sha = crate::sha256_bytes(&entry.model);
        if model_sha != entry.package.manifest.model_sha256 {
            return Err(HarnessError::Manifest("model bytes changed"));
        }
        let state = entry.lifecycle.state;
        Ok((entry, state))
    }

    pub fn finish_eval(&mut self, id: &[u8; 16], report: &OfflineEvalReport) -> bool {
        let Some(e) = self.entries.get_mut(id) else {
            return false;
        };
        e.lifecycle.finish_eval(report, &self.policy);
        e.lifecycle.state == Lifecycle::OfflineEvaluated
    }

    pub fn enter_shadow(&mut self, id: &[u8; 16]) -> bool {
        let Some(e) = self.entries.get_mut(id) else {
            return false;
        };
        e.lifecycle.enter_shadow();
        e.lifecycle.state == Lifecycle::Shadow
    }

    pub fn enter_canary(&mut self, id: &[u8; 16]) -> bool {
        let Some(e) = self.entries.get_mut(id) else {
            return false;
        };
        e.lifecycle.enter_canary();
        e.lifecycle.state == Lifecycle::Canary
    }

    pub fn approve(&mut self, id: &[u8; 16]) -> bool {
        let Some(e) = self.entries.get_mut(id) else {
            return false;
        };
        e.lifecycle.approve();
        e.lifecycle.state == Lifecycle::Approved
    }

    pub fn retire(&mut self, id: &[u8; 16]) -> bool {
        let Some(e) = self.entries.get_mut(id) else {
            return false;
        };
        e.lifecycle.retire();
        e.lifecycle.state == Lifecycle::Retired
    }

    /// 健康信号 → 暂挂；仅当暂挂的是 **active 包** 时才触发回滚动作
    /// （非 active 包暂挂不改变当前 active——无动作）。
    pub fn health_signal(
        &mut self,
        id: &[u8; 16],
        signal: &HealthSignal,
    ) -> Option<RollbackAction> {
        let was_active = self.active().map(|e| e.package.manifest.package_id) == Some(*id);
        let entry = self.entries.get_mut(id)?;
        let suspended = entry.lifecycle.suspend_on(signal, &self.policy);
        if !suspended {
            return None;
        }
        if !was_active {
            return None;
        }
        // active 被暂挂：回滚到上一 Approved。
        let fallback = entry.lifecycle.rollback_to;
        if let Some(fid) = fallback {
            let valid = self
                .entries
                .get(&fid)
                .map(|e| e.lifecycle.state == Lifecycle::Approved)
                .unwrap_or(false);
            if valid {
                return Some(RollbackAction::RolledBack { new_active: fid });
            }
        }
        Some(RollbackAction::NoFallback)
    }

    /// 重启恢复快照（manifest + lifecycle 簿 + 评测报告；内容字节不序列
    /// 化——恢复时必须重验模型/tokenizer 字节摘要）。
    pub fn snapshot(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in self.entries.values() {
            b.extend_from_slice(&e.package.manifest.encode());
            b.push(e.lifecycle.state as u8);
            match e.lifecycle.rollback_to {
                Some(id) => {
                    b.push(1);
                    b.extend_from_slice(&id);
                }
                None => b.push(0),
            }
            match &e.lifecycle.eval {
                Some(r) => {
                    b.push(1);
                    for v in [
                        r.golden_pass_rate,
                        r.adversarial_pass_rate,
                        r.safety_refusal_rate,
                        r.deviation,
                        r.gate_refusal_rate,
                    ] {
                        b.extend_from_slice(&v.to_le_bytes());
                    }
                }
                None => b.push(0),
            }
        }
        b
    }

    /// 恢复（重验签名与模型/tokenizer 摘要；失败 → 整表拒绝，fail-closed）。
    pub fn restore(
        &mut self,
        wire: &[u8],
        load: ContentLoader<'_>,
        now_ns: u64,
    ) -> Result<(), HarnessError> {
        if wire.len() < 4 {
            return Err(HarnessError::Manifest("snapshot short"));
        }
        let n = u32::from_le_bytes(wire[0..4].try_into().unwrap()) as usize;
        let mut p = 4usize;
        for _ in 0..n {
            if p + ModelManifestV1::WIRE_LEN + 3 > wire.len() {
                return Err(HarnessError::Manifest("snapshot truncated"));
            }
            let manifest = ModelManifestV1::decode(&wire[p..p + ModelManifestV1::WIRE_LEN])?;
            p += ModelManifestV1::WIRE_LEN;
            let state = match wire[p] {
                0 => Lifecycle::Registered,
                1 => Lifecycle::OfflineEvaluated,
                2 => Lifecycle::Shadow,
                3 => Lifecycle::Canary,
                4 => Lifecycle::Approved,
                5 => Lifecycle::Suspended,
                6 => Lifecycle::Retired,
                _ => return Err(HarnessError::Manifest("snapshot lifecycle")),
            };
            p += 1;
            let has_rollback = wire[p] == 1;
            p += 1;
            let rollback_to = if has_rollback {
                if p + 16 > wire.len() {
                    return Err(HarnessError::Manifest("snapshot truncated"));
                }
                let mut id = [0u8; 16];
                id.copy_from_slice(&wire[p..p + 16]);
                p += 16;
                Some(id)
            } else {
                None
            };
            let eval = if wire[p] == 1 {
                p += 1;
                if p + 40 > wire.len() {
                    return Err(HarnessError::Manifest("snapshot truncated"));
                }
                let mut it = (0..5).map(|i| {
                    f64::from_le_bytes(wire[p + i * 8..p + i * 8 + 8].try_into().unwrap())
                });
                Some(OfflineEvalReport {
                    golden_pass_rate: it.next().unwrap_or(0.0),
                    adversarial_pass_rate: it.next().unwrap_or(0.0),
                    safety_refusal_rate: it.next().unwrap_or(0.0),
                    deviation: it.next().unwrap_or(0.0),
                    gate_refusal_rate: it.next().unwrap_or(0.0),
                })
            } else {
                p += 1;
                None
            };
            let package = ModelPackage::new(manifest.clone())?;
            // 恢复门：签名（key 必须已注入）+ 内容摘要重验。
            let pk = self
                .signer_keys
                .get(&manifest.signer_key_id)
                .ok_or(HarnessError::Manifest("unknown signer key"))?;
            if !manifest.verify_signature(pk) {
                return Err(HarnessError::Manifest("signature invalid"));
            }
            if !manifest.is_valid_at_ns(now_ns) {
                return Err(HarnessError::PackageExpired);
            }
            let (model, tokenizer) =
                load(&manifest.package_id).ok_or(HarnessError::Manifest("content unavailable"))?;
            if crate::sha256_bytes(&model) != manifest.model_sha256 {
                return Err(HarnessError::Manifest("model bytes changed"));
            }
            if crate::sha256_bytes(&tokenizer) != manifest.tokenizer_sha256 {
                return Err(HarnessError::Manifest("tokenizer bytes changed"));
            }
            let mut lc = PackageLifecycle::registered(manifest.package_id, rollback_to);
            lc.state = state;
            lc.eval = eval;
            self.entries.insert(
                manifest.package_id,
                PackageEntry {
                    package,
                    model,
                    tokenizer,
                    eval_suite: None,
                    lifecycle: lc,
                },
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::{EvalCase, EvalSuite};
    use crate::lifecycle::OfflineEvalReport;
    use crate::manifest::tests as mt;

    fn eval_suite() -> EvalSuite {
        EvalSuite::new(vec![EvalCase {
            name: "g".into(),
            input: "i".into(),
            expected: "v".into(),
            adversarial: false,
            safety_refusal: false,
        }])
    }

    /// 评测套件素材字节（与 `EvalSuite::new` 的摘要输入逐字节一致——
    /// 素材的 SHA-256 必须等于 manifest eval_suite_digest）。
    fn eval_suite_material(e: &EvalSuite) -> Vec<u8> {
        let mut b = Vec::new();
        for c in &e.cases {
            b.extend_from_slice(c.name.as_bytes());
            b.extend_from_slice(c.input.as_bytes());
            b.extend_from_slice(c.expected.as_bytes());
            b.push(c.adversarial as u8);
            b.push(c.safety_refusal as u8);
            b.push(0xFF);
        }
        b
    }

    fn registry() -> ModelRegistry {
        let mut r = ModelRegistry::new(7, 1, 16 << 30, LifecyclePolicy::default());
        let (_sk, pk) = mt::test_keypair();
        r.add_signer_key(1, pk);
        r
    }

    fn register_approved(r: &mut ModelRegistry, pkg: &[u8; 16]) -> ModelManifestV1 {
        let (sk, pk) = mt::test_keypair();
        let mut m = mt::manifest();
        m.package_id = *pkg;
        m.model_sha256 = crate::sha256_bytes(&[0x11; 256]);
        m.tokenizer_sha256 = crate::sha256_bytes(&[0x22; 128]);
        let suite = eval_suite();
        m.eval_suite_digest = suite.digest;
        m.seal(&sk, &pk);
        let contents = PackageContents {
            model: Some(&[0x11; 256]),
            tokenizer: Some(&[0x22; 128]),
            eval_suite: Some(&eval_suite_material(&suite)),
        };
        r.register(&m, &contents, Some(&suite), 5000).unwrap();
        let report = OfflineEvalReport {
            golden_pass_rate: 1.0,
            adversarial_pass_rate: 1.0,
            safety_refusal_rate: 1.0,
            deviation: 0.0,
            gate_refusal_rate: 0.0,
        };
        assert!(r.finish_eval(pkg, &report));
        assert!(r.enter_shadow(pkg));
        assert!(r.enter_canary(pkg));
        assert!(r.approve(pkg));
        let _ = pk;
        m
    }

    #[test]
    fn register_gates() {
        let pkg = [3; 16];
        let (sk, pk) = mt::test_keypair();
        let mut m = mt::manifest();
        m.package_id = pkg;
        m.model_sha256 = crate::sha256_bytes(&[0x11; 256]);
        m.tokenizer_sha256 = crate::sha256_bytes(&[0x22; 128]);
        let suite = eval_suite();
        m.eval_suite_digest = suite.digest;
        m.seal(&sk, &pk);
        let contents = PackageContents {
            model: Some(&[0x11; 256]),
            tokenizer: Some(&[0x22; 128]),
            eval_suite: Some(&eval_suite_material(&suite)),
        };
        let mut r = registry();
        r.register(&m, &contents, Some(&suite), 5000).unwrap();
        // 未知签名 key → 拒。
        let mut m2 = m.clone();
        m2.package_id = [4; 16];
        m2.signer_key_id = 99;
        assert_eq!(
            r.register(&m2, &contents, Some(&suite), 5000),
            Err(HarnessError::Manifest("unknown signer key"))
        );
        // runtime 不匹配 → 拒（改后重签——签名门先行，篡改必先被签名拦）。
        let mut m3 = m.clone();
        m3.package_id = [5; 16];
        m3.runtime_id = 8;
        m3.seal(&sk, &pk);
        assert_eq!(
            r.register(&m3, &contents, Some(&suite), 5000),
            Err(HarnessError::RuntimeMismatch)
        );
        // 硬件不足以容纳 → 拒（重签）。
        let mut m4 = m.clone();
        m4.package_id = [6; 16];
        m4.memory_limit_bytes = 64 << 30;
        m4.seal(&sk, &pk);
        assert_eq!(
            r.register(&m4, &contents, Some(&suite), 5000),
            Err(HarnessError::HardwareInsufficient)
        );
        // 模型字节篡改 → 拒。
        let contents_bad = PackageContents {
            model: Some(&[0xEE; 256]),
            tokenizer: Some(&[0x22; 128]),
            eval_suite: Some(&eval_suite_material(&suite)),
        };
        let mut m5 = m.clone();
        m5.package_id = [7; 16];
        m5.seal(&sk, &pk);
        assert_eq!(
            r.register(&m5, &contents_bad, Some(&suite), 5000),
            Err(HarnessError::Manifest("model_sha256 mismatch"))
        );
        // 过期 → 拒（重签——包 id 已变）。
        let mut m6 = m.clone();
        m6.package_id = [8; 16];
        m6.seal(&sk, &pk);
        assert_eq!(
            r.register(&m6, &contents, Some(&suite), 2_000_000),
            Err(HarnessError::PackageExpired)
        );
        // 重复 → 拒。
        assert_eq!(
            r.register(&m, &contents, Some(&suite), 5000),
            Err(HarnessError::Manifest("duplicate package_id"))
        );
        let _ = pk;
    }

    #[test]
    fn lifecycle_and_inference_eligibility() {
        let mut r = registry();
        let pkg = [1; 16];
        register_approved(&mut r, &pkg);
        let (entry, state) = r.prepare_inference(&pkg, 5000).unwrap();
        assert_eq!(state, Lifecycle::Approved);
        assert_eq!(
            entry.lifecycle.state.stage(),
            crate::error::LifecycleStage::Approved
        );
        // 未注册 → 拒。
        assert_eq!(
            r.prepare_inference(&[9; 16], 5000),
            Err(HarnessError::UnknownPackage)
        );
    }

    #[test]
    fn eligibility_denied_for_unapproved() {
        let mut r = registry();
        let (sk, pk) = mt::test_keypair();
        let mut m = mt::manifest();
        m.package_id = [2; 16];
        m.model_sha256 = crate::sha256_bytes(&[0x11; 256]);
        m.tokenizer_sha256 = crate::sha256_bytes(&[0x22; 128]);
        let suite = eval_suite();
        m.eval_suite_digest = suite.digest;
        m.seal(&sk, &pk);
        let contents = PackageContents {
            model: Some(&[0x11; 256]),
            tokenizer: Some(&[0x22; 128]),
            eval_suite: Some(&eval_suite_material(&suite)),
        };
        r.register(&m, &contents, Some(&suite), 5000).unwrap();
        // Registered → 拒。
        assert!(matches!(
            r.prepare_inference(&[2; 16], 5000),
            Err(HarnessError::NotApproved(_))
        ));
        let _ = pk;
    }

    #[test]
    fn health_signal_suspends_and_rolls_back() {
        let mut r = registry();
        register_approved(&mut r, &[1; 16]);
        // 第二包注册时第一包为 active → 回滚目标 = [1]。
        register_approved(&mut r, &[2; 16]);
        // 新包健康失败 → 回滚到上一 Approved。
        let action = r.health_signal(&[2; 16], &HealthSignal::LatencyNs(9_000_000_000));
        assert!(matches!(
            action,
            Some(RollbackAction::RolledBack { new_active }) if new_active == [1; 16]
        ));
        assert!(matches!(
            r.prepare_inference(&[2; 16], 5000),
            Err(HarnessError::NotApproved(_))
        ));
        // 首包（无更老 Approved）健康失败 → NoFallback。
        let action = r.health_signal(&[1; 16], &HealthSignal::OutputRefusalRate(0.9));
        assert!(matches!(action, Some(RollbackAction::NoFallback)));
        // 全部暂挂：active 为空（系统无模型可用；规则/solver 不受影响）。
        assert!(r.active().is_none());
        // 良性信号 → 不暂挂（第三包 approved 后）。
        register_approved(&mut r, &[3; 16]);
        let action = r.health_signal(&[3; 16], &HealthSignal::Drift(0.001));
        assert!(action.is_none());
        assert!(r.active().is_some());
    }

    #[test]
    fn snapshot_restore_roundtrip() {
        let mut r = registry();
        let pkg = [1; 16];
        register_approved(&mut r, &pkg);
        let snap = r.snapshot();
        // 新注册表（同 key）恢复；内容从 load 回调提供。
        let mut r2 = registry();
        let model = [0x11; 256].to_vec();
        let tok = [0x22; 128].to_vec();
        r2.restore(
            &snap,
            &mut |id| {
                if id == &pkg {
                    Some((model.clone(), tok.clone()))
                } else {
                    None
                }
            },
            5000,
        )
        .unwrap();
        let (entry, state) = r2.prepare_inference(&pkg, 5000).unwrap();
        assert_eq!(state, Lifecycle::Approved);
        assert_eq!(entry.model, vec![0x11; 256]);
        // 恢复时模型字节篡改 → 整表拒绝。
        let mut r3 = registry();
        let bad = [0xEE; 256].to_vec();
        assert_eq!(
            r3.restore(&snap, &mut |_| Some((bad.clone(), tok.clone())), 5000),
            Err(HarnessError::Manifest("model bytes changed"))
        );
        // 缺内容 → 拒。
        let mut r4 = registry();
        assert_eq!(
            r4.restore(&snap, &mut |_| None, 5000),
            Err(HarnessError::Manifest("content unavailable"))
        );
    }
}
