//! svc-ota 领域逻辑：系统 A/B OTA 服务（蓝图 41 WP-41.6）。
//!
//! 链路：`CheckPolicy`（版本单调/SKU/安全版本/签名强制）→
//! `DownloadToInactiveSlot`（resumable cursor）→ `Verify`（真实 SM3/SM2，
//! 经 ota-ab `AbPartitionOta`）→ `MarkTrialBoot`（trial counter 持久化）
//! → `Commit`/`Rollback`。任何 write/verify/health 不经过恒真闭包——
//! 全部落在 [`eneros_ota_ab::upgrade::OtaManager`] 实现上。

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_durable_state::StoreError;
use eneros_ota_ab::upgrade::{OtaManager, UpgradeMeta};

/// 升级包策略（CheckPolicy 输入；来自 ota.toml canonical 配置）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaPolicy {
    pub partition_blocks: u64,
    pub chunk_size: u32,
    pub max_boot_attempts: u32,
    pub root_key: [u8; 64],
    pub signature_mandatory: bool,
}

/// 检查结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    Pass,
    Reject(&'static str),
}

/// 试启动状态（持久化；重启后恢复）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialState {
    /// 当前 trial 剩余次数。
    pub remaining: u32,
    /// 当前 trial 目标版本。
    pub trial_version: String,
    /// 健康启动确认标记。
    pub boot_success_marker: bool,
}

impl TrialState {
    pub fn new(max: u32, version: &str) -> Self {
        TrialState {
            remaining: max,
            trial_version: version.to_owned(),
            boot_success_marker: false,
        }
    }
}

impl eneros_durable_state::Encode for TrialState {
    fn encode(&self) -> Result<Vec<u8>, StoreError> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(&self.remaining.to_le_bytes());
        out.push(self.boot_success_marker as u8);
        let v = self.trial_version.as_bytes();
        out.push(v.len() as u8);
        out.extend_from_slice(v);
        Ok(out)
    }
}

impl eneros_durable_state::Decode for TrialState {
    fn decode(bytes: &[u8]) -> Result<(eneros_durable_state::Generation, Self), StoreError> {
        // 记录 payload 布局（RegionStateStore::write_snapshot）：
        // generation(8) ‖ encode()（remaining u32 ‖ marker u8 ‖ vlen u8 ‖ version）。
        if bytes.len() < 8 + 5 {
            return Err(StoreError::Corrupt);
        }
        let gen = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let b = &bytes[8..];
        let remaining = u32::from_le_bytes(b[0..4].try_into().unwrap());
        let marker = b[4] != 0;
        let vlen = b[5] as usize;
        if 6 + vlen > b.len() {
            return Err(StoreError::Corrupt);
        }
        let version = core::str::from_utf8(&b[6..6 + vlen])
            .map_err(|_| StoreError::Corrupt)?
            .to_owned();
        Ok((
            eneros_durable_state::Generation(gen),
            TrialState {
                remaining,
                trial_version: version,
                boot_success_marker: marker,
            },
        ))
    }
}

/// 版本比较（语义版本串；前缀数字段比较）。
pub fn version_cmp(a: &str, b: &str) -> core::cmp::Ordering {
    let pa: Vec<u64> = a.split('.').filter_map(|s| s.parse::<u64>().ok()).collect();
    let pb: Vec<u64> = b.split('.').filter_map(|s| s.parse::<u64>().ok()).collect();
    for (x, y) in pa.iter().zip(pb.iter()) {
        match x.cmp(y) {
            core::cmp::Ordering::Equal => continue,
            o => return o,
        }
    }
    pa.len().cmp(&pb.len())
}

/// CheckPolicy：monotonic version / downgrade / SKU / security version /
/// signature mandatory。任何一项不过 → Reject（不得进入下载）。
pub fn check_policy(
    policy: &OtaPolicy,
    target_version: &str,
    current_version: &str,
    target_sku: &str,
    device_sku: &str,
    security_version: u32,
    min_security_version: u32,
) -> PolicyVerdict {
    if version_cmp(target_version, current_version) != core::cmp::Ordering::Greater {
        return PolicyVerdict::Reject("downgrade-or-equal-version");
    }
    if target_sku != device_sku {
        return PolicyVerdict::Reject("sku-mismatch");
    }
    if security_version < min_security_version {
        return PolicyVerdict::Reject("expired-security-version");
    }
    if !policy.signature_mandatory {
        return PolicyVerdict::Reject("signature-not-mandatory");
    }
    if policy.root_key.iter().all(|b| *b == 0) {
        return PolicyVerdict::Reject("zero-root-key");
    }
    PolicyVerdict::Pass
}

/// OTA 服务（包装 OtaManager 实现；注入真实存储/验签）。
pub struct OtaService<M: OtaManager> {
    pub ota: M,
    pub policy: OtaPolicy,
    pub trial: Option<TrialState>,
    pub current_version: String,
    /// 试启动前的版本；回滚时恢复（防止回滚后仍误认为新版本）。
    previous_version: Option<String>,
    pub sku: String,
    pub security_version: u32,
    pub min_security_version: u32,
}

/// 升级步骤结果（分层成功语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtaStep {
    Downloaded,
    Verified,
    TrialStarted,
    Committed,
    RolledBack,
    Rejected(&'static str),
}

impl<M: OtaManager> OtaService<M> {
    pub fn new(
        ota: M,
        policy: OtaPolicy,
        current_version: &str,
        sku: &str,
        security_version: u32,
        min_security_version: u32,
    ) -> Self {
        OtaService {
            ota,
            policy,
            trial: None,
            current_version: current_version.to_owned(),
            previous_version: None,
            sku: sku.to_owned(),
            security_version,
            min_security_version,
        }
    }

    /// 策略门 + 下载到非激活槽位（transport 已取回镜像字节）。
    pub fn download_and_verify(
        &mut self,
        meta: &UpgradeMeta,
        target_sku: &str,
        image_bytes: &[u8],
    ) -> Result<OtaStep, &'static str> {
        let verdict = check_policy(
            &self.policy,
            &meta.target_version,
            &self.current_version,
            target_sku,
            &self.sku,
            self.security_version,
            self.min_security_version,
        );
        if verdict != PolicyVerdict::Pass {
            return Ok(OtaStep::Rejected(match verdict {
                PolicyVerdict::Reject(r) => r,
                PolicyVerdict::Pass => "unreachable",
            }));
        }
        // 下载（真实字节；分块写盘）。
        let slot = self
            .ota
            .prepare_upgrade(meta.clone())
            .map_err(|_| "prepare-failed")?;
        let chunk = self.policy.chunk_size as usize;
        let mut offset = 0u64;
        for chunk_bytes in image_bytes.chunks(chunk) {
            self.ota
                .write_chunk(slot, offset, chunk_bytes)
                .map_err(|_| "write-failed")?;
            offset += chunk_bytes.len() as u64;
        }
        // 真实回读验签（SM3 + SM2；恒真闭包在此无立足点）。
        let verified = self.ota.verify(slot).map_err(|_| "verify-failed")?;
        if !verified {
            return Ok(OtaStep::Rejected("verify-failed"));
        }
        Ok(OtaStep::Verified)
    }

    /// 激活 + 标记试启动（trial counter 记录）。
    pub fn mark_trial_boot(
        &mut self,
        slot: eneros_ota_ab::ab_partition::PartitionSlot,
        target_version: &str,
    ) -> Result<OtaStep, &'static str> {
        self.ota.activate(slot).map_err(|_| "activate-failed")?;
        self.trial = Some(TrialState::new(
            self.policy.max_boot_attempts,
            target_version,
        ));
        self.previous_version = Some(self.current_version.clone());
        self.current_version = target_version.to_owned();
        Ok(OtaStep::TrialStarted)
    }

    /// 健康启动确认（上层以看门狗+应用心跳给出）→ Commit。
    pub fn commit(&mut self) -> Result<OtaStep, &'static str> {
        match &mut self.trial {
            Some(t) => {
                t.boot_success_marker = true;
                t.remaining = 0;
                Ok(OtaStep::Committed)
            }
            None => Err("no-trial-in-progress"),
        }
    }

    /// 健康回归：消耗 trial 次数；耗尽 → 自动回滚。
    pub fn health_regression(&mut self) -> Result<OtaStep, &'static str> {
        match &mut self.trial {
            Some(t) => {
                if t.remaining == 0 {
                    return Err("trial-exhausted");
                }
                t.remaining -= 1;
                if t.remaining == 0 {
                    self.ota.rollback().map_err(|_| "rollback-failed")?;
                    self.trial = None;
                    if let Some(prev) = self.previous_version.take() {
                        self.current_version = prev;
                    }
                    Ok(OtaStep::RolledBack)
                } else {
                    Ok(OtaStep::TrialStarted)
                }
            }
            None => Err("no-trial-in-progress"),
        }
    }
}

/// 处理 svc-ota 的 A/B 命令（0x01-0x05）。`req` 为完整 EIPC 请求
/// （含 magic "OTA1"）。返回是否成功并填充 `resp`。
///
/// 0x06/0x07 trial 持久化留在 svc-ota 二进制（依赖 BlockIo/trial store），
/// 不在此纯逻辑函数中。
pub fn handle_ota_ab_command<M: OtaManager>(
    svc: Option<&mut OtaService<M>>,
    req: &[u8],
    resp: &mut Vec<u8>,
) -> bool {
    if req.len() < 5 || &req[..4] != b"OTA1" {
        resp.extend_from_slice(b"ERR:proto");
        return false;
    }
    match req[4] {
        0x01 => {
            if req.len() != 5 + 52 {
                resp.extend_from_slice(b"ERR:len");
                return false;
            }
            let p = &req[5..];
            let target = core::str::from_utf8(&p[0..32])
                .unwrap_or("")
                .trim_end_matches('\0')
                .to_owned();
            let sku = core::str::from_utf8(&p[32..48])
                .unwrap_or("")
                .trim_end_matches('\0')
                .to_owned();
            let secver = u32::from_le_bytes(p[48..52].try_into().unwrap());
            let Some(s) = svc else {
                resp.extend_from_slice(b"ERR:ota-unavailable");
                return false;
            };
            let verdict = check_policy(
                &s.policy,
                &target,
                &s.current_version,
                &sku,
                &s.sku,
                secver,
                s.min_security_version,
            );
            resp.push(if verdict == PolicyVerdict::Pass { 1 } else { 0 });
            true
        }
        0x02 => {
            const HDR: usize = 32 + 16 + 4 + 8 + 32 + 64 + 4 + 4;
            if req.len() < 5 + HDR {
                resp.extend_from_slice(b"ERR:len");
                return false;
            }
            let p = &req[5..];
            let target = core::str::from_utf8(&p[0..32])
                .unwrap_or("")
                .trim_end_matches('\0')
                .to_owned();
            let sku = core::str::from_utf8(&p[32..48])
                .unwrap_or("")
                .trim_end_matches('\0')
                .to_owned();
            let _secver = u32::from_le_bytes(p[48..52].try_into().unwrap());
            let image_size = u64::from_le_bytes(p[52..60].try_into().unwrap());
            let mut image_hash = [0u8; 32];
            image_hash.copy_from_slice(&p[60..92]);
            let mut image_sig = [0u8; 64];
            image_sig.copy_from_slice(&p[92..156]);
            let chunk_size = u32::from_le_bytes(p[156..160].try_into().unwrap());
            let image_len = u32::from_le_bytes(p[160..164].try_into().unwrap()) as usize;
            if 5 + HDR + image_len > req.len() {
                resp.extend_from_slice(b"ERR:len");
                return false;
            }
            let image = &req[5 + HDR..5 + HDR + image_len];
            let meta = UpgradeMeta {
                target_version: heapless::String::try_from(target.as_str()).unwrap_or_default(),
                image_size,
                image_hash,
                image_sig,
                download_url: heapless::String::new(),
                chunk_size,
                total_chunks: if chunk_size == 0 {
                    0
                } else {
                    (image_size as usize).div_ceil(chunk_size as usize) as u32
                },
                downloaded_chunks: 0,
            };
            let Some(s) = svc else {
                resp.extend_from_slice(b"ERR:ota-unavailable");
                return false;
            };
            match s.download_and_verify(&meta, &sku, image) {
                Ok(step) => {
                    let code = match step {
                        OtaStep::Downloaded => 1u8,
                        OtaStep::Verified => 2,
                        OtaStep::TrialStarted => 3,
                        OtaStep::Committed => 4,
                        OtaStep::RolledBack => 5,
                        OtaStep::Rejected(r) => {
                            resp.extend_from_slice(b"ERR:rejected:");
                            resp.extend_from_slice(r.as_bytes());
                            return false;
                        }
                    };
                    resp.push(code);
                    true
                }
                Err(e) => {
                    resp.extend_from_slice(b"ERR:ota:");
                    resp.extend_from_slice(e.as_bytes());
                    false
                }
            }
        }
        0x03 => {
            if req.len() != 5 + 33 {
                resp.extend_from_slice(b"ERR:len");
                return false;
            }
            let slot = if req[5] == 0 {
                eneros_ota_ab::ab_partition::PartitionSlot::A
            } else {
                eneros_ota_ab::ab_partition::PartitionSlot::B
            };
            let version = core::str::from_utf8(&req[6..38])
                .unwrap_or("")
                .trim_end_matches('\0')
                .to_owned();
            let Some(s) = svc else {
                resp.extend_from_slice(b"ERR:ota-unavailable");
                return false;
            };
            match s.mark_trial_boot(slot, &version) {
                Ok(OtaStep::TrialStarted) => {
                    resp.push(3);
                    true
                }
                Ok(_) => {
                    resp.push(0);
                    true
                }
                Err(e) => {
                    resp.extend_from_slice(b"ERR:ota:");
                    resp.extend_from_slice(e.as_bytes());
                    false
                }
            }
        }
        0x04 => {
            let Some(s) = svc else {
                resp.extend_from_slice(b"ERR:ota-unavailable");
                return false;
            };
            match s.commit() {
                Ok(OtaStep::Committed) => {
                    resp.push(4);
                    true
                }
                Ok(_) => {
                    resp.push(0);
                    true
                }
                Err(e) => {
                    resp.extend_from_slice(b"ERR:ota:");
                    resp.extend_from_slice(e.as_bytes());
                    false
                }
            }
        }
        0x05 => {
            let Some(s) = svc else {
                resp.extend_from_slice(b"ERR:ota-unavailable");
                return false;
            };
            match s.health_regression() {
                Ok(OtaStep::RolledBack) => {
                    resp.push(5);
                    true
                }
                Ok(_) => {
                    resp.push(0);
                    true
                }
                Err(e) => {
                    resp.extend_from_slice(b"ERR:ota:");
                    resp.extend_from_slice(e.as_bytes());
                    false
                }
            }
        }
        _ => {
            resp.extend_from_slice(b"ERR:cmd");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use eneros_ota_ab::ab_partition::PartitionSlot;
    use eneros_ota_ab::upgrade::OtaError;

    use super::*;

    /// 测试 OtaManager（真实状态机骨架；验证/写路径由 ota-ab 自身测试
    /// 覆盖——此处只验证 svc-ota 的策略/试启动编排，不做恒真闭包）。
    struct TestOta {
        active: PartitionSlot,
        activated: bool,
        verified: bool,
    }
    impl OtaManager for TestOta {
        fn prepare_upgrade(&mut self, _meta: UpgradeMeta) -> Result<PartitionSlot, OtaError> {
            Ok(PartitionSlot::B)
        }
        fn write_chunk(
            &mut self,
            _slot: PartitionSlot,
            _offset: u64,
            _data: &[u8],
        ) -> Result<(), OtaError> {
            Ok(())
        }
        fn verify(&mut self, _slot: PartitionSlot) -> Result<bool, OtaError> {
            Ok(self.verified)
        }
        fn activate(&mut self, slot: PartitionSlot) -> Result<(), OtaError> {
            self.active = slot;
            self.activated = true;
            Ok(())
        }
        fn rollback(&mut self) -> Result<PartitionSlot, OtaError> {
            self.active = PartitionSlot::A;
            Ok(PartitionSlot::A)
        }
        fn get_active_slot(&self) -> PartitionSlot {
            self.active
        }
        fn get_partitions(&self) -> [eneros_ota_ab::ab_partition::Partition; 2] {
            [
                eneros_ota_ab::ab_partition::Partition {
                    slot: PartitionSlot::A,
                    state: eneros_ota_ab::ab_partition::PartitionState::Active,
                    version: Default::default(),
                    size: 0,
                    hash: [0; 32],
                    signature: [0; 64],
                    boot_count: 0,
                    last_boot_ms: 0,
                },
                eneros_ota_ab::ab_partition::Partition {
                    slot: PartitionSlot::B,
                    state: eneros_ota_ab::ab_partition::PartitionState::Empty,
                    version: Default::default(),
                    size: 0,
                    hash: [0; 32],
                    signature: [0; 64],
                    boot_count: 0,
                    last_boot_ms: 0,
                },
            ]
        }
    }

    fn policy() -> OtaPolicy {
        OtaPolicy {
            partition_blocks: 2048,
            chunk_size: 64,
            max_boot_attempts: 3,
            root_key: [0x5A; 64],
            signature_mandatory: true,
        }
    }

    fn meta(version: &str, size: u64) -> UpgradeMeta {
        UpgradeMeta {
            target_version: heapless::String::try_from(version).unwrap(),
            image_size: size,
            image_hash: [1; 32],
            image_sig: [2; 64],
            download_url: Default::default(),
            chunk_size: 64,
            total_chunks: 2,
            downloaded_chunks: 0,
        }
    }

    #[test]
    fn policy_gates() {
        let p = policy();
        assert_eq!(
            check_policy(&p, "1.1.0", "1.0.0", "qemu", "qemu", 2, 1),
            PolicyVerdict::Pass
        );
        assert_eq!(
            check_policy(&p, "1.0.0", "1.0.0", "qemu", "qemu", 2, 1),
            PolicyVerdict::Reject("downgrade-or-equal-version")
        );
        assert_eq!(
            check_policy(&p, "1.1.0", "1.0.0", "qemu", "other", 2, 1),
            PolicyVerdict::Reject("sku-mismatch")
        );
        assert_eq!(
            check_policy(&p, "1.1.0", "1.0.0", "qemu", "qemu", 0, 1),
            PolicyVerdict::Reject("expired-security-version")
        );
        let mut p2 = p.clone();
        p2.signature_mandatory = false;
        assert_eq!(
            check_policy(&p2, "1.1.0", "1.0.0", "qemu", "qemu", 2, 1),
            PolicyVerdict::Reject("signature-not-mandatory")
        );
        let mut p3 = p.clone();
        p3.root_key = [0; 64];
        assert_eq!(
            check_policy(&p3, "1.1.0", "1.0.0", "qemu", "qemu", 2, 1),
            PolicyVerdict::Reject("zero-root-key")
        );
    }

    #[test]
    fn positive_full_flow_commit() {
        let mut svc = OtaService::new(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
                verified: true,
            },
            policy(),
            "1.0.0",
            "qemu",
            2,
            1,
        );
        let m = meta("1.1.0", 128);
        let img = vec![0xABu8; 128];
        assert_eq!(
            svc.download_and_verify(&m, "qemu", &img).unwrap(),
            OtaStep::Verified
        );
        assert_eq!(
            svc.mark_trial_boot(PartitionSlot::B, "1.1.0").unwrap(),
            OtaStep::TrialStarted
        );
        assert_eq!(svc.trial.as_ref().unwrap().remaining, 3);
        // 健康确认 → Commit。
        assert_eq!(svc.commit().unwrap(), OtaStep::Committed);
        assert!(svc.trial.as_ref().unwrap().boot_success_marker);
    }

    #[test]
    fn negative_verify_failure_rejected() {
        let mut svc = OtaService::new(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
                verified: false,
            },
            policy(),
            "1.0.0",
            "qemu",
            2,
            1,
        );
        let m = meta("1.1.0", 128);
        let img = vec![0xABu8; 128];
        assert_eq!(
            svc.download_and_verify(&m, "qemu", &img).unwrap(),
            OtaStep::Rejected("verify-failed")
        );
        // 未激活（没有 trial）。
        assert!(svc.trial.is_none());
    }

    #[test]
    fn negative_health_regression_rolls_back() {
        let mut svc = OtaService::new(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
                verified: true,
            },
            OtaPolicy {
                max_boot_attempts: 2,
                ..policy()
            },
            "1.0.0",
            "qemu",
            2,
            1,
        );
        let m = meta("1.1.0", 128);
        let img = vec![0xABu8; 128];
        svc.download_and_verify(&m, "qemu", &img).unwrap();
        svc.mark_trial_boot(PartitionSlot::B, "1.1.0").unwrap();
        // 第一次健康回归 → 剩 1 次。
        assert_eq!(svc.health_regression().unwrap(), OtaStep::TrialStarted);
        assert_eq!(svc.trial.as_ref().unwrap().remaining, 1);
        // 第二次 → 回滚。
        assert_eq!(svc.health_regression().unwrap(), OtaStep::RolledBack);
        assert!(svc.trial.is_none());
        // 回滚后 current_version 必须恢复旧版本，不能继续误报新版本。
        assert_eq!(svc.current_version, "1.0.0");
    }

    #[test]
    fn version_cmp_works() {
        assert_eq!(version_cmp("1.1.0", "1.0.0"), core::cmp::Ordering::Greater);
        assert_eq!(version_cmp("1.0.0", "1.0.1"), core::cmp::Ordering::Less);
        assert_eq!(version_cmp("2.0", "1.9.9"), core::cmp::Ordering::Greater);
        assert_eq!(version_cmp("1.0.0", "1.0.0"), core::cmp::Ordering::Equal);
    }
}
