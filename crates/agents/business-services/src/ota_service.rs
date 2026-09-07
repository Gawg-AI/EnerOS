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
use eneros_ota_ab::upgrade::{OtaError, OtaManager, UpgradeMeta};

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
    ///
    /// 69.5（WP-69.5.3，DEMO-69 第 5 窗）：Err 侧从抹平的 `&'static str`
    /// 改为携带**关键数字/环节**的诊断串——此前 `prepare-failed` 抹掉了
    /// `ImageTooLarge { image_size, partition_capacity }` 的两个数字，
    /// 口径分歧导致的拒绝说不出口径是什么（§2.3/§2.4：容量分歧的危险
    /// 一半在拒绝不可诊断）。格式：十进制、无空格、无千分位（词形基线
    /// 脚本 token 解析口径）。
    /// Z3 检查：`image_size`/`partition_capacity`/`offset` 均为几何量，
    /// 可安全暴露；SM2 公钥/root key 字节**绝不入串**（t39 锁）。
    pub fn download_and_verify(
        &mut self,
        meta: &UpgradeMeta,
        target_sku: &str,
        image_bytes: &[u8],
    ) -> Result<OtaStep, String> {
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
        // 下载（真实字节；分块写盘）。69.5：容量不足区分产出双数字
        // （`prepare-failed:image-too-large:{image_size}>{partition_capacity}`），
        // 其余类别保留类别名（Debug 词形；prepare 路径可达类别均无字段）。
        let slot = self
            .ota
            .prepare_upgrade(meta.clone())
            .map_err(|e| match e {
                OtaError::ImageTooLarge {
                    image_size,
                    partition_capacity,
                } => {
                    alloc::format!(
                        "prepare-failed:image-too-large:{image_size}>{partition_capacity}"
                    )
                }
                other => alloc::format!("prepare-failed:{other:?}"),
            })?;
        let chunk = self.policy.chunk_size as usize;
        let mut offset = 0u64;
        for chunk_bytes in image_bytes.chunks(chunk) {
            self.ota
                .write_chunk(slot, offset, chunk_bytes)
                .map_err(|e| match e {
                    OtaError::WriteFailed { offset } => alloc::format!("write-failed:{offset}"),
                    other => alloc::format!("write-failed:{other:?}"),
                })?;
            offset += chunk_bytes.len() as u64;
        }
        // 真实回读验签（SM3 + SM2；恒真闭包在此无立足点）。69.5：失败
        // 环节可区分（SM3 哈希 vs SM2 验签 vs 状态机）。
        let verified = self.ota.verify(slot).map_err(|e| match e {
            OtaError::HashMismatch => String::from("verify-failed:sm3-hash-mismatch"),
            OtaError::SignatureInvalid => String::from("verify-failed:sm2-signature-invalid"),
            other => alloc::format!("verify-failed:{other:?}"),
        })?;
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

    // ---- 69.5（WP-69.5.3）：错误可诊断性 t36-t39（DEMO-69 第 5 窗）----

    /// 可注入故障的 OtaManager（prepare/write/verify 三门独立指定）。
    struct FaultOta {
        prepare_err: Option<OtaError>,
        write_err: Option<OtaError>,
        verify_err: Option<OtaError>,
    }
    impl OtaManager for FaultOta {
        fn prepare_upgrade(&mut self, _meta: UpgradeMeta) -> Result<PartitionSlot, OtaError> {
            if let Some(e) = self.prepare_err.clone() {
                return Err(e);
            }
            Ok(PartitionSlot::B)
        }
        fn write_chunk(
            &mut self,
            _slot: PartitionSlot,
            _offset: u64,
            _data: &[u8],
        ) -> Result<(), OtaError> {
            if let Some(e) = self.write_err.clone() {
                return Err(e);
            }
            Ok(())
        }
        fn verify(&mut self, _slot: PartitionSlot) -> Result<bool, OtaError> {
            if let Some(e) = self.verify_err.clone() {
                return Err(e);
            }
            Ok(true)
        }
        fn activate(&mut self, _slot: PartitionSlot) -> Result<(), OtaError> {
            Ok(())
        }
        fn rollback(&mut self) -> Result<PartitionSlot, OtaError> {
            Ok(PartitionSlot::A)
        }
        fn get_active_slot(&self) -> PartitionSlot {
            PartitionSlot::A
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

    /// 0x02 DOWNLOAD+VERIFY 请求构造（OTA1 魔数 + 定长头 + 镜像字节）；
    /// target="9.9.9"/sku="qemu" 过策略门（策略拒绝用例单独走降级版本）。
    fn req02(image_size: u64, image: &[u8]) -> Vec<u8> {
        let mut hdr = alloc::vec![0u8; 32 + 16 + 4 + 8 + 32 + 64 + 4 + 4];
        hdr[..5].copy_from_slice(b"9.9.9");
        hdr[32..36].copy_from_slice(b"qemu");
        hdr[32 + 16 + 4..32 + 16 + 4 + 8].copy_from_slice(&image_size.to_le_bytes());
        hdr[32 + 16 + 4 + 8 + 32 + 64..32 + 16 + 4 + 8 + 32 + 64 + 4]
            .copy_from_slice(&64u32.to_le_bytes());
        hdr[32 + 16 + 4 + 8 + 32 + 64 + 4..].copy_from_slice(&(image.len() as u32).to_le_bytes());
        let mut req = alloc::vec::Vec::with_capacity(5 + hdr.len() + image.len());
        req.extend_from_slice(b"OTA1");
        req.push(0x02);
        req.extend_from_slice(&hdr);
        req.extend_from_slice(image);
        req
    }

    fn svc_with(ota: FaultOta) -> OtaService<FaultOta> {
        OtaService::new(ota, policy(), "1.0.0", "qemu", 2, 1)
    }

    /// t36：容量不足错误串**同时携带** image_size 与 partition_capacity
    /// 的十进制字面，且断言**完整格式**（`{image}>{capacity}` 顺序敏感
    /// ——变异靶 1：交换两个数字 → 本测红）。
    #[test]
    fn t36_image_too_large_error_carries_both_numbers() {
        let mut svc = svc_with(FaultOta {
            prepare_err: Some(OtaError::ImageTooLarge {
                image_size: 10_485_760,
                partition_capacity: 1_048_576,
            }),
            write_err: None,
            verify_err: None,
        });
        let req = req02(10_485_760, &[0u8; 8]);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req, &mut resp));
        assert_eq!(
            core::str::from_utf8(&resp).unwrap(),
            "ERR:ota:prepare-failed:image-too-large:10485760>1048576",
            "回执须携带双数字（DEMO-69 第 5 窗；顺序=声明>容量）"
        );
    }

    /// t37：容量不足 / 设备写失败 / 策略门拒绝 ⇒ 三个错误串互不相同
    /// （变异靶 2：抹平回 "prepare-failed" → 本测红）。
    #[test]
    fn t37_prepare_failure_categories_are_distinguishable() {
        // 容量不足。
        let mut svc = svc_with(FaultOta {
            prepare_err: Some(OtaError::ImageTooLarge {
                image_size: 10_485_760,
                partition_capacity: 1_048_576,
            }),
            write_err: None,
            verify_err: None,
        });
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(
            Some(&mut svc),
            &req02(10_485_760, &[0u8; 8]),
            &mut resp
        ));
        let cap_resp = core::str::from_utf8(&resp).unwrap().to_owned();
        // 设备写失败（WriteFailed 携带 offset 数字）。
        let mut svc = svc_with(FaultOta {
            prepare_err: None,
            write_err: Some(OtaError::WriteFailed { offset: 65_536 }),
            verify_err: None,
        });
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(
            Some(&mut svc),
            &req02(128, &[0xABu8; 128]),
            &mut resp
        ));
        let write_resp = core::str::from_utf8(&resp).unwrap().to_owned();
        // 策略门拒绝（降级版本 → Rejected 静态臂）。
        let mut svc = svc_with(FaultOta {
            prepare_err: None,
            write_err: None,
            verify_err: None,
        });
        let mut req = req02(128, &[0xABu8; 128]);
        req[4] = 0x02;
        req[5..10].copy_from_slice(b"1.0.0"); // == current → downgrade-or-equal
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req, &mut resp));
        let policy_resp = core::str::from_utf8(&resp).unwrap().to_owned();
        assert_eq!(
            cap_resp,
            "ERR:ota:prepare-failed:image-too-large:10485760>1048576"
        );
        assert_eq!(write_resp, "ERR:ota:write-failed:65536");
        assert_eq!(policy_resp, "ERR:rejected:downgrade-or-equal-version");
        assert_ne!(cap_resp, write_resp);
        assert_ne!(cap_resp, policy_resp);
        assert_ne!(write_resp, policy_resp);
    }

    /// t38：最长错误串 ≤ 单帧回执可用上界（MR_REPLY_MAX_WORDS×8 =
    /// 112B；69.1 I3 单源常量——施工前实测的回执预算锁）。**经真实
    /// wire 路径产出**最坏形态（双 u64::MAX），格式串膨胀即本测红
    /// （变异靶 3 的牙；自抄字面算长度无牙，故走 handle_ota_ab_command）。
    #[test]
    fn t38_error_string_fits_receipt_budget() {
        const BUDGET: usize = eneros_agent_sdk::MR_REPLY_MAX_WORDS * 8;
        assert_eq!(BUDGET, 112);
        let mut svc = svc_with(FaultOta {
            prepare_err: Some(OtaError::ImageTooLarge {
                image_size: u64::MAX,
                partition_capacity: u64::MAX,
            }),
            write_err: None,
            verify_err: None,
        });
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(
            Some(&mut svc),
            &req02(128, &[0u8; 8]),
            &mut resp
        ));
        assert_eq!(resp.first(), Some(&b'E'));
        let full_len = resp.len();
        assert!(
            full_len <= BUDGET,
            "最长错误串 {}（{full_len}B）超出单帧回执预算 {BUDGET}B（格式串膨胀即截断/丢字）",
            core::str::from_utf8(&resp).unwrap_or("?")
        );
    }

    /// t39：错误串不泄密——不含 root key / 65B 公钥的任何 4 字节窗口
    /// （原始字节与 hex 两形态；Z3 检查的回归锁）。
    #[test]
    fn t39_error_string_leaks_no_key_material() {
        // 确定性伪随机 key/公钥（真实密钥不入仓——测试用模式字节）。
        let mut key = [0u8; 64];
        let mut x = 0x1234_5678u32;
        for b in key.iter_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *b = (x >> 24) as u8;
        }
        let mut pubkey = [0u8; 65];
        pubkey[0] = 0x04;
        pubkey[1..].copy_from_slice(&key);
        let candidates = [
            "prepare-failed:image-too-large:10485760>1048576",
            "write-failed:65536",
            "verify-failed:sm3-hash-mismatch",
            "verify-failed:sm2-signature-invalid",
            "prepare-failed:DowngradeRejected",
        ];
        for s in candidates {
            assert!(s.is_ascii(), "诊断串须为 ASCII（可打印几何/类别量）: {s}");
            for win in key.windows(4).chain(pubkey.windows(4)) {
                assert!(
                    !s.as_bytes().windows(4).any(|w| w == win),
                    "诊断串 {s} 泄露密钥窗口 {win:02x?}"
                );
                let hex_win: String = win.iter().map(|b| format!("{b:02x}")).collect();
                assert!(
                    !s.contains(&hex_win),
                    "诊断串 {s} 泄露密钥 hex 窗口 {hex_win}"
                );
            }
        }
    }
}

/// 69.5（WP-69.5.2 变异靶 1 反向锁）：生产块大小**单源守卫**——svc-ota
/// bin 挂载 EipcBlockDevice 必须引用 ota-ab 共享常量（OTA_PROD_BLOCK_SIZE
/// / DATA_DISK_BLOCKS），不得回退手抄字面（字面复现 = 第三个口径源
/// 复活，容量口径三处一致破坏 → 本测红）。与能量侧 ea_display_guard
/// 同款源文本守卫先例。
#[cfg(test)]
mod svc_ota_source_guard {
    #[test]
    fn svc_ota_block_size_uses_shared_constants() {
        let src = include_str!("bin/svc-ota.rs");
        assert!(
            src.contains("OTA_PROD_BLOCK_SIZE"),
            "svc-ota 须引用 ota-ab::OTA_PROD_BLOCK_SIZE 单源常量（容量口径三处一致）"
        );
        assert!(
            src.contains("DATA_DISK_BLOCKS"),
            "svc-ota 整盘块数须引用 ota-ab::DATA_DISK_BLOCKS"
        );
        assert!(
            !src.contains("EipcBlockDevice::new(io, 512"),
            "手抄块大小字面回归（第三口径源复活）"
        );
        assert!(
            !src.contains("32768"),
            "手抄整盘块数字面回归（应引用 DATA_DISK_BLOCKS）"
        );
    }
}
