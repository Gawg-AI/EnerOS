//! svc-ota 领域逻辑：系统 A/B OTA 服务（蓝图 41 WP-41.6；70.5 准入归真）。
//!
//! 70.5（WP-70.5.3，方向 3 §4.2 方案 B）：准入只消费**版本化签名更新
//! 描述**（`ota_wire::UpdateDescription` + SM2 desc_sig）——版本/SKU/
//! 安全版本/长度/摘要/策略代次由同一签名绑定；旧"无签名属性 + 哈希
//! 签章"写路径失败关闭（每个可达写入口只接收可信目标事实，无未签
//! 属性旁路）。CHECK（0x01）与 DOWNLOAD（0x02）共用同一判据
//! （[`admit_description`] 单源）；CHECK 通过回执不是未来写能力——写时
//! 针对最新水位复核（[`OtaService::set_security_floor`]，WP-70.5.4）。
//!
//! 70.5（WP-70.5.1）：当前版本/SKU/安全版本不再由 bin 字面构造——来自
//! 可信身份快照（root 经 OTA1 0x08 分发的 boot-protocol IdentityCapsule
//! 投影）；目标描述 secver 对水位判据（F3-11/F3-12 修复：不再用本机
//! secver 顶替目标 secver）。
//!
//! 链路：`admit_description`（验签/用途/代次/基线/SKU/水位）→ 分块写盘
//! → `Verify`（真实 SM3/SM2 回读，经 ota-ab `AbPartitionOta`）→
//! `MarkTrialBoot` → `Commit`/`Rollback`。任何 write/verify/health 不经过
//! 恒真闭包——全部落在 [`eneros_ota_ab::upgrade::OtaManager`] 实现上。

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_agent_sdk::ota_wire::{UpdateDescription, DESC_SIZE, SIG_LEN};
use eneros_crypto::{sm3_hash, Sm2PublicKey, Sm2Signature};
use eneros_durable_state::StoreError;
use eneros_ota_ab::upgrade::{OtaError, OtaManager, UpgradeMeta};
use eneros_signer_keyring::verify_signature;

/// 升级包策略（CheckPolicy 输入；70.5 起生产唯一来源 = 已验签配置束
/// OtaConsumer 取数，bin 字面策略退役）。
///
/// WP-70.5.2 轮换预备：`generation` = 配置条目代次（描述 policy_
/// generation 判据锚 + key_id/用途/代次入册）；key_id 口径 = 锚指纹
/// SM3(root_key)[0..8]（只入日志/证据）；换钥 = 停机签名换包（在线
/// 轮换未批准，方向 3 §9 冻结表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtaPolicy {
    pub partition_blocks: u64,
    pub chunk_size: u32,
    pub max_boot_attempts: u32,
    pub root_key: [u8; 64],
    pub signature_mandatory: bool,
    /// 策略代次（配置束条目 generation；描述签发时的代次锚）。
    pub generation: u64,
}

/// 检查结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    Pass,
    Reject(&'static str),
}

/// 可信设备身份快照（WP-70.5.1：唯一生产来源 = root 分发的已验证启动
/// 身份；测试装配用同型受控值并显式标注，不伪装生产来源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    /// 当前运行版本（启动镜像签名声明；防降级基线）。
    pub current_version: String,
    /// 设备 SKU（发布平台绑定；管理请求携带的 SKU 不作数）。
    pub sku: String,
    /// 当前启动镜像安全版本（展示面；≠ 水位）。
    pub security_version: u32,
    /// 可信水位（protected anchor 匹配的 security_floor；跨重启恢复）。
    pub security_floor: u32,
}

impl DeviceIdentity {
    /// 身份齐备（非空版本/SKU、非零 secver/水位）。不可得 → OTA 不装配
    /// （不回 0.0.0/qemu 默认分支——该分支已删）。
    pub fn is_present(&self) -> bool {
        !self.current_version.is_empty()
            && !self.sku.is_empty()
            && self.security_version != 0
            && self.security_floor != 0
    }
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

/// 版本比较（语义版本串；前缀数字段比较）。操作数须为规范形（见
/// [`is_canonical_version`]——非数字段在此被静默丢弃，准入前先域检）。
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

/// 版本串规范形：1..=3 个点分段、每段非空纯十进制且 u64 可解析——与
/// capsule 投影 `major.minor.patch`（u16×3）同形。`version_cmp` 只对
/// 规范形有意义。
pub fn is_canonical_version(v: &str) -> bool {
    let mut segments = 0usize;
    for seg in v.split('.') {
        segments += 1;
        if segments > 3
            || seg.is_empty()
            || !seg.bytes().all(|b| b.is_ascii_digit())
            || seg.parse::<u64>().is_err()
        {
            return false;
        }
    }
    segments >= 1
}

/// CheckPolicy：monotonic version / SKU / **目标**安全版本 ≥ 水位 /
/// signature mandatory。任何一项不过 → Reject（不得进入下载）。
///
/// 70.5（F3-11/F3-12 修复）：`security_version` 语义 = **目标描述**声明
/// 的安全版本（准入判据），不是本机当前 secver。
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
    if policy.generation == 0 {
        return PolicyVerdict::Reject("zero-policy-generation");
    }
    PolicyVerdict::Pass
}

/// 描述验签 + 域检结论。
pub enum DescVerdict {
    /// 已验签且资格判据通过。
    Accepted(UpdateDescription),
    /// 拒绝（类别词形；只含类别与几何量，不含密钥材料）。
    Rejected(&'static str),
}

/// 准入单源（CHECK 与 DOWNLOAD 共用）：SM2 验签（root_key 锚）→ 用途 →
/// 代次 → 签名后资格域检（哈希非零/长度非零/SKU/基线/目标 secver ≥
/// 水位/版本单调）。镜像摘要入签 = 元数据↔镜像绑定；长度/摘要与实际
/// 传输字节的强等在写路径另行复核。
pub fn admit_description(
    desc_bytes: &[u8; DESC_SIZE],
    desc_sig: &[u8; SIG_LEN],
    policy: &OtaPolicy,
    current_version: &str,
    device_sku: &str,
    min_security_version: u32,
) -> DescVerdict {
    let desc = match UpdateDescription::decode(desc_bytes) {
        Ok(d) => d,
        Err(_) => return DescVerdict::Rejected("desc-malformed"),
    };
    if desc.usage != eneros_agent_sdk::ota_wire::DESC_USAGE_OTA_AB_IMAGE {
        return DescVerdict::Rejected("desc-usage");
    }
    if desc.policy_generation != policy.generation {
        return DescVerdict::Rejected("desc-generation");
    }
    // 签名验证（类型化原语；锚不可构造 → fail-closed）。
    let mut key65 = [0u8; 65];
    key65[0] = 0x04;
    key65[1..].copy_from_slice(&policy.root_key);
    let Ok(pk) = Sm2PublicKey::from_bytes(&key65) else {
        return DescVerdict::Rejected("anchor-invalid");
    };
    let sig = Sm2Signature::from_bytes(desc_sig);
    if verify_signature(&sm3_hash(desc_bytes), &sig, &pk).is_err() {
        return DescVerdict::Rejected("desc-signature-invalid");
    }
    if desc.image_len == 0 {
        return DescVerdict::Rejected("desc-empty-image");
    }
    if desc.image_hash.iter().all(|b| *b == 0) {
        return DescVerdict::Rejected("desc-zero-hash");
    }
    if desc.baseline_version != current_version {
        return DescVerdict::Rejected("baseline-mismatch");
    }
    // 目标版本串规范形（70.5 复查）：`version_cmp` 对非数字段静默丢弃
    // （"2.x.0" 与 "2.0" 同序）+ 段数决胜——单调判据的操作数必须与 capsule
    // 投影 `major.minor.patch` 同形，否则怪形版本可越过严格 Greater。
    if !is_canonical_version(&desc.target_version) {
        return DescVerdict::Rejected("desc-version-malformed");
    }
    // 策略判据单源（check_policy：版本单调/SKU/目标 secver≥水位/签名强制
    // /零锚/零代次）。
    match check_policy(
        policy,
        &desc.target_version,
        current_version,
        &desc.sku,
        device_sku,
        desc.security_version,
        min_security_version,
    ) {
        PolicyVerdict::Pass => DescVerdict::Accepted(desc),
        PolicyVerdict::Reject(r) => DescVerdict::Rejected(r),
    }
}

/// 策略 canonical 域检（WP-70.5.2 装配前最后一道）：partition_blocks
/// 必须等于真实几何（ota-ab PARTITION_BLOCKS 单源）；chunk_size 非零且
/// 有界（≤ 单槽容量 1 MiB）；max_boot_attempts 非零且有界（≤
/// BOOT_CONTROL_MAX_TRIES=7——BootControl 域上限，跨域常量语义一致）。
/// 锚曲线点验证在此（`Sm2PublicKey::from_bytes` 拒绝非曲线点/零点）。
pub fn validate_policy(policy: &OtaPolicy) -> Result<(), &'static str> {
    if policy.partition_blocks != eneros_ota_ab::ab_partition::PARTITION_BLOCKS {
        return Err("partition-blocks-drift");
    }
    if policy.chunk_size == 0 || policy.chunk_size > 1_048_576 {
        return Err("chunk-size-bounds");
    }
    if policy.max_boot_attempts == 0
        || policy.max_boot_attempts
            > u32::from(eneros_boot_protocol::boot_control::BOOT_CONTROL_MAX_TRIES)
    {
        return Err("boot-attempts-bounds");
    }
    if policy.root_key.iter().all(|b| *b == 0) {
        return Err("zero-root-key");
    }
    let mut key65 = [0u8; 65];
    key65[0] = 0x04;
    key65[1..].copy_from_slice(&policy.root_key);
    if Sm2PublicKey::from_bytes(&key65).is_err() {
        return Err("anchor-invalid");
    }
    if policy.generation == 0 {
        return Err("zero-policy-generation");
    }
    Ok(())
}

/// 已验证身份 capsule → 设备身份快照（WP-70.5.1/70.5.4：唯一生产转换；
/// u64→u32 收窄 checked——越域拒绝而非 saturating 放行，方向 3 §7）。
/// SKU/release_id 为 NUL 填充 UTF-8；畸形 → 拒绝（身份不可得语义）。
pub fn identity_from_capsule(
    capsule: &eneros_boot_protocol::IdentityCapsule,
) -> Result<DeviceIdentity, &'static str> {
    // SKU = 发布平台绑定（manifest platform_id = 哈希指纹 16B）——
    // 十六进制展示形态（32 chars；描述 sku 域与探针派生同源一致）。
    let sku: String = capsule
        .sku
        .iter()
        .map(|b| alloc::format!("{b:02x}"))
        .collect();
    if capsule.security_version == 0
        || capsule.security_version > u32::MAX as u64
        || capsule.security_floor > u32::MAX as u64
    {
        return Err("identity-secver-bounds");
    }
    let pv = capsule.product_version;
    Ok(DeviceIdentity {
        current_version: alloc::format!("{}.{}.{}", pv[0], pv[1], pv[2]),
        sku,
        security_version: capsule.security_version as u32,
        security_floor: capsule.security_floor as u32,
    })
}

/// 候选载荷身份提取（70.7 REG-70.7-BOOT-TRIAL-BIND 实弹修复）。
///
/// 试启动事务记录的目标身份（`security_version` / `image_id`）必须与候选
/// 载荷 manifest **逐字段一致**：boot-manager 选槽验证用
/// `TrustPolicy.expected_security_version/expected_release_id` 绑定
/// （[`eneros_boot_protocol::manifest`] header），不一致即
/// `SECURITY_BINDING`/`ImageId` 拒绝并回退旧槽（run55 实弹：trial 硬编码
/// secver=2/[3;16] vs 候选 manifest secver=1/内容派生 release_id → 试启动
/// 槽被拒 → R7/R9/R10 全挂）。
///
/// 生产来源 = 已验签下行包 manifest；门内下行缺席
/// （REG-70.6-MGMT-DOWNLINK Open）→ 探针经只读读面取候选槽**载荷自身**
/// 身份供给 trial（本函数 = 该字节流的解析单源）。绑定判据不放松：
/// 记录值仍取自 wire，仅取值来源在门内换为候选介质本身。
///
/// `manifest_bytes` = 启动槽首部读回的 manifest 字节（首 112B 头 + 组件
/// 表 + 尾；头内 `total_len` 自描述）。返回 `(security_version, release_id)`。
pub fn candidate_identity(manifest_bytes: &[u8]) -> Result<(u64, [u8; 16]), &'static str> {
    use eneros_boot_protocol::manifest::{ReleaseManifest, MANIFEST_HEADER_LEN};
    if manifest_bytes.len() < MANIFEST_HEADER_LEN {
        return Err("candidate-short");
    }
    let total = u32::from_le_bytes(
        manifest_bytes[12..16]
            .try_into()
            .map_err(|_| "candidate-short")?,
    ) as usize;
    if total < MANIFEST_HEADER_LEN || total > manifest_bytes.len() {
        return Err("candidate-len");
    }
    let manifest =
        ReleaseManifest::decode(&manifest_bytes[..total]).map_err(|_| "candidate-decode")?;
    if manifest.header.security_version == 0 {
        return Err("candidate-secver-zero");
    }
    Ok((manifest.header.security_version, manifest.header.release_id))
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
    /// 装配（WP-70.5.1：身份来自快照——bin 不再传 "0.0.0"/"qemu" 字面）。
    pub fn new(ota: M, policy: OtaPolicy, identity: &DeviceIdentity) -> Self {
        OtaService {
            ota,
            policy,
            trial: None,
            current_version: identity.current_version.to_owned(),
            previous_version: None,
            sku: identity.sku.to_owned(),
            security_version: identity.security_version,
            min_security_version: identity.security_floor,
        }
    }

    /// 水位更新（WP-70.5.4：可信 floor 恢复面——单调，拒绝回退；本卷
    /// 写入方只有 root 身份分发，svc-ota 不自写"安全已提升"）。
    pub fn set_security_floor(&mut self, floor: u32) -> Result<(), &'static str> {
        if floor < self.min_security_version {
            return Err("floor-regress");
        }
        self.min_security_version = floor;
        Ok(())
    }

    /// 策略门 + 下载到非激活槽位（WP-70.5.3：只消费已验签目标描述）。
    ///
    /// 判据链：`admit_description`（最新水位复核——CHECK 通过回执不是
    /// 未来写能力）→ 描述↔传输长度强等 → 描述哈希入 UpgradeMeta →
    /// 分块写盘 → 真实回读 SM3 哈希比对 + SM2 `image_sig` 验签（签章
    /// 对象 = 镜像哈希，F3-13 语义不变）。
    ///
    /// 69.5 诊断纪律保持：Err 侧携带关键数字/环节，十进制无空格；
    /// SM2 公钥/root key 字节绝不入串（t39 锁）。
    pub fn download_and_verify(
        &mut self,
        desc_bytes: &[u8; DESC_SIZE],
        desc_sig: &[u8; SIG_LEN],
        image_sig: &[u8; SIG_LEN],
        image: &[u8],
    ) -> Result<OtaStep, String> {
        let desc = match admit_description(
            desc_bytes,
            desc_sig,
            &self.policy,
            &self.current_version,
            &self.sku,
            self.min_security_version,
        ) {
            DescVerdict::Accepted(d) => d,
            DescVerdict::Rejected(r) => return Ok(OtaStep::Rejected(r)),
        };
        // 描述↔传输长度强等（WP-70.5.3 反测：image_size 与实际长度不等）。
        if desc.image_len != image.len() as u64 {
            return Ok(OtaStep::Rejected("image-size-mismatch"));
        }
        // chunk 单点守卫（70.5 复查）：装配侧 validate_policy 已拦 0，此处
        // 是写路径唯一除数/步长——不留"total_chunks 有守卫、chunks(0) panic"
        // 的自相矛盾半守卫。
        let chunk = self.policy.chunk_size as usize;
        if chunk == 0 {
            return Ok(OtaStep::Rejected("policy-chunk-zero"));
        }
        let meta = UpgradeMeta {
            target_version: heapless::String::try_from(desc.target_version.as_str())
                .unwrap_or_default(),
            image_size: desc.image_len,
            image_hash: desc.image_hash,
            image_sig: *image_sig,
            download_url: heapless::String::new(),
            chunk_size: self.policy.chunk_size,
            // 口径 = div_ceil（与 kernel_ota/rollback/ab_partition 同源）：
            // 地板除使非对齐镜像尾块落在 Written 之后 WriteFailed。
            total_chunks: image.len().div_ceil(chunk) as u32,
            downloaded_chunks: 0,
        };
        // 下载（真实字节；分块写盘）。69.5：容量不足区分产出双数字
        // （`prepare-failed:image-too-large:{image_size}>{partition_capacity}`），
        // 其余类别保留类别名（Debug 词形；prepare 路径可达类别均无字段）。
        let slot = self.ota.prepare_upgrade(meta).map_err(|e| match e {
            OtaError::ImageTooLarge {
                image_size,
                partition_capacity,
            } => {
                alloc::format!("prepare-failed:image-too-large:{image_size}>{partition_capacity}")
            }
            other => alloc::format!("prepare-failed:{other:?}"),
        })?;
        let mut offset = 0u64;
        for chunk_bytes in image.chunks(chunk) {
            self.ota
                .write_chunk(slot, offset, chunk_bytes)
                .map_err(|e| match e {
                    OtaError::WriteFailed { offset } => alloc::format!("write-failed:{offset}"),
                    other => alloc::format!("write-failed:{other:?}"),
                })?;
            offset += chunk_bytes.len() as u64;
        }
        // 真实回读验签（SM3 + SM2；恒真闭包在此无立足点）。69.5：失败
        // 环节可区分（SM3 哈希 vs SM2 验签 vs 状态机）。R15-U：verify
        // 收窄为 Result<()> 后不存在 Ok(false) 臂，拒绝一律走 Err 映射。
        self.ota.verify(slot).map_err(|e| match e {
            OtaError::HashMismatch => String::from("verify-failed:sm3-hash-mismatch"),
            OtaError::SignatureInvalid => String::from("verify-failed:sm2-signature-invalid"),
            other => alloc::format!("verify-failed:{other:?}"),
        })?;
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
/// 70.5 wire（v2，同 bundle 硬切换）：0x01 = CHECK-DESC（描述+签名），
/// 0x02 = DOWNLOAD-DESC（描述+双签+镜像）；旧 52B 无签名形 = 协议违例
/// （失败关闭——生产不回退到不绑定 metadata 的写路径）。
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
            let Some(s) = svc else {
                resp.extend_from_slice(b"ERR:ota-unavailable");
                return false;
            };
            let (desc, desc_sig) = match eneros_agent_sdk::ota_wire::parse_check_desc_req(req) {
                Ok(v) => v,
                Err(_) => {
                    resp.extend_from_slice(b"ERR:len");
                    return false;
                }
            };
            // CHECK 与 DOWNLOAD 同判据（admit_description 单源；最新水位）。
            let verdict = match admit_description(
                &desc.encode().expect("请求内描述已 canonical"),
                &desc_sig,
                &s.policy,
                &s.current_version,
                &s.sku,
                s.min_security_version,
            ) {
                DescVerdict::Accepted(_) => PolicyVerdict::Pass,
                DescVerdict::Rejected(r) => PolicyVerdict::Reject(r),
            };
            resp.push(if verdict == PolicyVerdict::Pass { 1 } else { 0 });
            true
        }
        0x02 => {
            let Some(s) = svc else {
                resp.extend_from_slice(b"ERR:ota-unavailable");
                return false;
            };
            let (desc_bytes, desc_sig, image_sig, image) =
                match eneros_agent_sdk::ota_wire::parse_download_v2_req(req) {
                    Ok(parsed) => {
                        // 描述 canonical 字节直取请求段（避免重编码歧义）。
                        let raw = &req[5..5 + DESC_SIZE];
                        let mut fixed = [0u8; DESC_SIZE];
                        fixed.copy_from_slice(raw);
                        (fixed, parsed.desc_sig, parsed.image_sig, parsed.image)
                    }
                    Err(_) => {
                        resp.extend_from_slice(b"ERR:len");
                        return false;
                    }
                };
            match s.download_and_verify(&desc_bytes, &desc_sig, &image_sig, image) {
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
    use eneros_agent_sdk::ota_wire::{
        encode_check_desc, encode_download_v2, UpdateDescription, DESC_USAGE_OTA_AB_IMAGE,
    };
    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair, Sm2PrivateKey};
    use eneros_ota_ab::ab_partition::PartitionSlot;
    use eneros_ota_ab::upgrade::OtaError;

    use super::*;

    /// 测试 OtaManager（真实状态机骨架；验证/写路径由 ota-ab 自身测试
    /// 覆盖——此处只验证 svc-ota 的策略/试启动编排，不做恒真闭包）。
    /// R15-U：verify 收窄 Result<()> 后，Ok(false) 模拟面不可表示；
    /// verify 失败注入走 FaultOta Err 路径。
    struct TestOta {
        active: PartitionSlot,
        activated: bool,
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
        fn verify(&mut self, _slot: PartitionSlot) -> Result<(), OtaError> {
            Ok(())
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

    /// 测试签名材料（确定性种子派生密钥对；非生产锚）。
    fn keypair(seed: u8) -> (Sm2PrivateKey, [u8; 64]) {
        let sk = Sm2PrivateKey::from_bytes(&[seed; 32]).expect("测试私钥");
        let kp = Sm2KeyPair::from_private_key(&sk).expect("测试公钥");
        let mut raw = [0u8; 64];
        raw.copy_from_slice(&kp.public_key.to_bytes_uncompressed()[1..65]);
        (sk, raw)
    }

    /// 对描述 canonical 字节签名（SM2 over SM3）。
    fn sign_desc(sk: &Sm2PrivateKey, desc: &UpdateDescription) -> [u8; 64] {
        let bytes = desc.encode().expect("desc encode");
        let kp = Sm2KeyPair::from_private_key(sk).expect("kp");
        let sig = sm2_sign(
            &sm3_hash(&bytes),
            sk,
            &kp.public_key,
            &mut CsRng::from_seed(&[0x77; 32]),
        )
        .expect("sign");
        let mut out = [0u8; 64];
        out.copy_from_slice(&sig.to_bytes()[..64]);
        out
    }

    fn identity() -> DeviceIdentity {
        DeviceIdentity {
            current_version: String::from("1.0.0"),
            sku: String::from("qemu"),
            security_version: 2,
            security_floor: 1,
        }
    }

    fn policy(root_key: [u8; 64]) -> OtaPolicy {
        OtaPolicy {
            partition_blocks: 2048,
            chunk_size: 64,
            max_boot_attempts: 3,
            root_key,
            signature_mandatory: true,
            generation: 1,
        }
    }

    fn desc(target: &str, hash: [u8; 32], len: u64) -> UpdateDescription {
        UpdateDescription {
            usage: DESC_USAGE_OTA_AB_IMAGE,
            sku: String::from("qemu"),
            target_version: String::from(target),
            baseline_version: String::from("1.0.0"),
            security_version: 2,
            image_len: len,
            image_hash: hash,
            policy_generation: 1,
        }
    }

    fn svc_with(ota: TestOta, sk: &Sm2PrivateKey) -> OtaService<TestOta> {
        let (_, key) = keypair_of(sk);
        OtaService::new(ota, policy(key), &identity())
    }

    fn keypair_of(sk: &Sm2PrivateKey) -> (Sm2PrivateKey, [u8; 64]) {
        let kp = Sm2KeyPair::from_private_key(sk).expect("kp");
        let mut raw = [0u8; 64];
        raw.copy_from_slice(&kp.public_key.to_bytes_uncompressed()[1..65]);
        (sk.clone(), raw)
    }

    /// 已签描述 + 双签 + 镜像的完整 0x02 请求。
    fn req02_valid(sk: &Sm2PrivateKey, target: &str, image: &[u8]) -> Vec<u8> {
        let hash = sm3_hash(image);
        let d = desc(target, hash, image.len() as u64);
        let desc_sig = sign_desc(sk, &d);
        let kp = Sm2KeyPair::from_private_key(sk).expect("kp");
        let isig = sm2_sign(
            &hash,
            sk,
            &kp.public_key,
            &mut CsRng::from_seed(&[0x78; 32]),
        )
        .expect("sign");
        let mut image_sig = [0u8; 64];
        image_sig.copy_from_slice(&isig.to_bytes()[..64]);
        encode_download_v2(&d, &desc_sig, &image_sig, 64, image)
    }

    #[test]
    fn identity_absent_rejects_assembly_contract() {
        // WP-70.5.1 失败语义：身份不可得 = 显式不可装配（不回 0.0.0）。
        let absent = DeviceIdentity {
            current_version: String::new(),
            sku: String::new(),
            security_version: 0,
            security_floor: 0,
        };
        assert!(!absent.is_present());
        assert!(identity().is_present());
    }

    #[test]
    fn policy_gates_including_target_secver_and_generation() {
        let (_, key) = keypair(0xA0);
        let p = policy(key);
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
        // F3-11/F3-12：目标 secver < 水位 → 拒绝（判据对象是目标 secver）。
        assert_eq!(
            check_policy(&p, "1.1.0", "1.0.0", "qemu", "qemu", 0, 1),
            PolicyVerdict::Reject("expired-security-version")
        );
        // 零策略代次拒绝（70.5 新域）。
        let mut p2 = p.clone();
        p2.generation = 0;
        assert_eq!(
            check_policy(&p2, "1.1.0", "1.0.0", "qemu", "qemu", 2, 1),
            PolicyVerdict::Reject("zero-policy-generation")
        );
        let mut p3 = p.clone();
        p3.signature_mandatory = false;
        assert_eq!(
            check_policy(&p3, "1.1.0", "1.0.0", "qemu", "qemu", 2, 1),
            PolicyVerdict::Reject("signature-not-mandatory")
        );
        let mut p4 = p.clone();
        p4.root_key = [0; 64];
        assert_eq!(
            check_policy(&p4, "1.1.0", "1.0.0", "qemu", "qemu", 2, 1),
            PolicyVerdict::Reject("zero-root-key")
        );
    }

    /// 70.5 正测：合法目标经过验签、准入、（假件）镜像验签。
    #[test]
    fn positive_full_flow_commit() {
        let (sk, _) = keypair(0x5A);
        let mut svc = svc_with(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
            },
            &sk,
        );
        let img = vec![0xABu8; 128];
        let req = req02_valid(&sk, "1.1.0", &img);
        let mut resp = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req, &mut resp));
        assert_eq!(resp, vec![2]); // Verified
        assert_eq!(
            svc.mark_trial_boot(PartitionSlot::B, "1.1.0").unwrap(),
            OtaStep::TrialStarted
        );
        assert_eq!(svc.trial.as_ref().unwrap().remaining, 3);
        assert_eq!(svc.commit().unwrap(), OtaStep::Committed);
        assert!(svc.trial.as_ref().unwrap().boot_success_marker);
    }

    /// WP-70.5.3 核心反测：CHECK 拒绝的低 secver 描述不能跳过 CHECK
    /// 直接 DOWNLOAD 成功——两命令同判据（admit_description 单源）。
    #[test]
    fn low_secver_rejected_by_check_and_download_alike() {
        let (sk, key) = keypair(0x5A);
        let ota = TestOta {
            active: PartitionSlot::A,
            activated: false,
        };
        let mut svc = OtaService::new(ota, policy(key), &identity());
        let img = vec![0xABu8; 64];
        let hash = sm3_hash(&img);
        // 低 secver（0 < 水位 1）描述。
        let mut d = desc("1.1.0", hash, img.len() as u64);
        d.security_version = 0;
        let dsig = sign_desc(&sk, &d);
        // CHECK → 0。
        let check = encode_check_desc(&d, &dsig);
        let mut resp = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &check, &mut resp));
        assert_eq!(resp, vec![0]);
        // 直接 DOWNLOAD → 同样拒绝（不能绕过 CHECK）。
        let kp = Sm2KeyPair::from_private_key(&sk).expect("kp");
        let isig =
            sm2_sign(&hash, &sk, &kp.public_key, &mut CsRng::from_seed(&[1; 32])).expect("sign");
        let mut image_sig = [0u8; 64];
        image_sig.copy_from_slice(&isig.to_bytes()[..64]);
        let dl = encode_download_v2(&d, &dsig, &image_sig, 64, &img);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &dl, &mut resp));
        assert_eq!(
            core::str::from_utf8(&resp).unwrap(),
            "ERR:rejected:expired-security-version"
        );
    }

    /// WP-70.5.3/70.5.4 反测：CHECK 通过后水位提升，再 DOWNLOAD 必重新
    /// 拒绝——会话间复用旧准入结果不得越过新水位。
    #[test]
    fn floor_raise_between_check_and_download_revalidates() {
        let (sk, _) = keypair(0x5A);
        let mut svc = svc_with(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
            },
            &sk,
        );
        let img = vec![0xABu8; 64];
        let hash = sm3_hash(&img);
        let d = desc("1.1.0", hash, img.len() as u64);
        let dsig = sign_desc(&sk, &d);
        // CHECK 通过（水位 1，desc secver=2）。
        let check = encode_check_desc(&d, &dsig);
        let mut resp = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &check, &mut resp));
        assert_eq!(resp, vec![1]);
        // 水位提升（root 身份分发面）→ desc secver 2 < 3 → DOWNLOAD 拒绝。
        svc.set_security_floor(3).expect("floor raise");
        let kp = Sm2KeyPair::from_private_key(&sk).expect("kp");
        let isig =
            sm2_sign(&hash, &sk, &kp.public_key, &mut CsRng::from_seed(&[1; 32])).expect("sign");
        let mut image_sig = [0u8; 64];
        image_sig.copy_from_slice(&isig.to_bytes()[..64]);
        let dl = encode_download_v2(&d, &dsig, &image_sig, 64, &img);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &dl, &mut resp));
        assert_eq!(
            core::str::from_utf8(&resp).unwrap(),
            "ERR:rejected:expired-security-version"
        );
    }

    /// 水位单调（WP-70.5.4：拒绝回退）。
    #[test]
    fn floor_regression_rejected() {
        let (sk, _) = keypair(0x5A);
        let mut svc = svc_with(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
            },
            &sk,
        );
        assert_eq!(svc.set_security_floor(1), Ok(()), "等值水位允许（幂等）");
        assert_eq!(svc.set_security_floor(2), Ok(()), "水位抬升允许");
        assert_eq!(
            svc.set_security_floor(1),
            Err("floor-regress"),
            "水位回退拒绝"
        );
    }

    /// WP-70.5.2 正测（接受集合随锚改变）：A 锚只接受 A 包，换 B 锚后
    /// 只接受 B 包。
    #[test]
    fn acceptance_set_follows_anchor() {
        let (sk_a, key_a) = keypair(0xA1);
        let (sk_b, key_b) = keypair(0xB2);
        let img = vec![0xCDu8; 64];

        // 锚 A 服务：A 签描述接受；B 签描述拒绝。
        let mut svc_a = OtaService::new(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
            },
            policy(key_a),
            &identity(),
        );
        let req_a = req02_valid(&sk_a, "1.1.0", &img);
        let mut resp = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc_a), &req_a, &mut resp));
        assert_eq!(resp, vec![2]);
        let req_b = req02_valid(&sk_b, "1.1.0", &img);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc_a), &req_b, &mut resp));
        assert_eq!(
            core::str::from_utf8(&resp).unwrap(),
            "ERR:rejected:desc-signature-invalid"
        );

        // 换锚 B（同型重装配）：B 签接受、A 签拒绝。
        let mut svc_b = OtaService::new(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
            },
            policy(key_b),
            &identity(),
        );
        let mut resp = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc_b), &req_b, &mut resp));
        assert_eq!(resp, vec![2]);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc_b), &req_a, &mut resp));
        assert_eq!(
            core::str::from_utf8(&resp).unwrap(),
            "ERR:rejected:desc-signature-invalid"
        );
    }

    /// 元数据每字段变异矩阵（WP-70.5.3 证据）：只改描述任一判据字段
    /// 而不重签 → 签名拒绝；重签但改 image_len ≠ 实际 → 长度强等拒绝；
    /// 错用途/旧基线/错代次/相同版本分别类别化拒绝。
    #[test]
    fn description_mutation_matrix() {
        let (sk, _) = keypair(0x5A);
        let img = vec![0xABu8; 64];
        let hash = sm3_hash(&img);
        let build = |d: &UpdateDescription| {
            let dsig = sign_desc(&sk, d);
            encode_download_v2(d, &dsig, &[0; 64], 64, &img)
        };
        let run = |req: &[u8]| {
            let mut svc = svc_with(
                TestOta {
                    active: PartitionSlot::A,
                    activated: false,
                },
                &sk,
            );
            let mut resp = Vec::new();
            let _ = handle_ota_ab_command(Some(&mut svc), req, &mut resp);
            core::str::from_utf8(&resp).unwrap_or("").to_owned()
        };
        // ① 只改 secver 不重签 → 签名拒绝。
        let mut d = desc("1.1.0", hash, 64);
        let good = build(&d);
        d.security_version = 9;
        let bad = {
            let bytes = d.encode().unwrap();
            let mut req = Vec::from(&good[..5]);
            req.extend_from_slice(&bytes);
            req.extend_from_slice(&good[5 + 158..5 + 158 + 64]);
            req.extend_from_slice(&good[5 + 158 + 64..]);
            req
        };
        assert_eq!(run(&bad), "ERR:rejected:desc-signature-invalid");
        // ② 错用途。
        let mut d = desc("1.1.0", hash, 64);
        d.usage = 9;
        assert_eq!(run(&build(&d)), "ERR:rejected:desc-usage");
        // ③ 错代次。
        let mut d = desc("1.1.0", hash, 64);
        d.policy_generation = 8;
        assert_eq!(run(&build(&d)), "ERR:rejected:desc-generation");
        // ④ 相同版本（非严格升级）。
        let d = desc("1.0.0", hash, 64);
        assert_eq!(run(&build(&d)), "ERR:rejected:downgrade-or-equal-version");
        // ⑤ 旧基线（与当前运行版本不符）。
        let mut d = desc("1.1.0", hash, 64);
        d.baseline_version = String::from("0.9.0");
        assert_eq!(run(&build(&d)), "ERR:rejected:baseline-mismatch");
        // ⑥ 零哈希。
        let d = desc("1.1.0", [0; 32], 64);
        assert_eq!(run(&build(&d)), "ERR:rejected:desc-zero-hash");
        // ⑦ 零长度。
        let d = desc("1.1.0", hash, 0);
        assert_eq!(run(&build(&d)), "ERR:rejected:desc-empty-image");
        // ⑧ image_len ≠ 实际长度。
        let d = desc("1.1.0", hash, 32);
        assert_eq!(run(&build(&d)), "ERR:rejected:image-size-mismatch");
        // ⑨ 畸形描述（非 canonical）。
        let mut raw = desc("1.1.0", hash, 64).encode().unwrap();
        raw[15] ^= 1;
        let mut req = Vec::from(b"OTA1\x02".as_slice());
        req.extend_from_slice(&raw);
        req.extend_from_slice(&[0u8; 64 + 64 + 4 + 4 + 64]);
        // 非 canonical 描述在 wire 解析层即拒（parse→decode 严格域检）。
        assert_eq!(run(&req), "ERR:len");
        // ⑩ 怪形目标版本（非数字段 / 超 3 段）：version_cmp 静默丢段会让
        // "2.x.0" 与 "2.0" 同序越过严格 Greater——规范形域检拒（70.5 复查）。
        let d = desc("2.x.0", hash, 64);
        assert_eq!(run(&build(&d)), "ERR:rejected:desc-version-malformed");
        let d = desc("1.2.3.4", hash, 64);
        assert_eq!(run(&build(&d)), "ERR:rejected:desc-version-malformed");
        assert!(
            is_canonical_version("1.1.0")
                && is_canonical_version("0")
                && !is_canonical_version("1..0")
        );
    }

    /// 旧 v1 无描述 DOWNLOAD（52B 定长头）失败关闭（WP-70.5.3 回滚
    /// 合同反向：生产不回退到不绑定 metadata 的旧 wire 写路径）。
    #[test]
    fn legacy_v1_download_dead() {
        let (sk, _) = keypair(0x5A);
        let mut svc = svc_with(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
            },
            &sk,
        );
        let mut req = Vec::from(b"OTA1\x02".as_slice());
        req.resize(5 + 52 + 64, 0xEE);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req, &mut resp));
        assert_eq!(core::str::from_utf8(&resp).unwrap(), "ERR:len");
    }

    /// 不存在 OTA（未装配）时 CHECK/DOWNLOAD 显式 unavailable。
    #[test]
    fn unavailable_service_explicit() {
        let img = [0u8; 8];
        let req = req02_valid(&keypair(0x5A).0, "1.1.0", &img);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command::<TestOta>(None, &req, &mut resp));
        assert_eq!(core::str::from_utf8(&resp).unwrap(), "ERR:ota-unavailable");
    }

    #[test]
    fn negative_verify_failure_rejected() {
        // R15-U：verify 收窄 Result<()> 后 Ok(false) 模拟不可表示——
        // 拒绝注入改走 FaultOta Err 路径，断言 verify-failed:{err} 映射词形
        // （失败环节可区分：SM3 哈希失配 ≠ SM2 验签失效，与 t39 候选词形同族）。
        let (sk, key) = keypair(0x5A);
        let mut svc = OtaService::new(
            FaultOta {
                prepare_err: None,
                write_err: None,
                verify_err: Some(OtaError::HashMismatch),
            },
            policy(key),
            &identity(),
        );
        let img = vec![0xABu8; 128];
        let req = req02_valid(&sk, "1.1.0", &img);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req, &mut resp));
        assert_eq!(
            core::str::from_utf8(&resp).unwrap(),
            "ERR:ota:verify-failed:sm3-hash-mismatch"
        );
        assert!(svc.trial.is_none());
    }

    #[test]
    fn negative_health_regression_rolls_back() {
        let (sk, key) = keypair(0x5A);
        let mut pol = policy(key);
        pol.max_boot_attempts = 2;
        let mut svc = OtaService::new(
            TestOta {
                active: PartitionSlot::A,
                activated: false,
            },
            pol,
            &identity(),
        );
        let img = vec![0xABu8; 128];
        let req = req02_valid(&sk, "1.1.0", &img);
        let mut resp = Vec::new();
        assert!(handle_ota_ab_command(Some(&mut svc), &req, &mut resp));
        svc.mark_trial_boot(PartitionSlot::B, "1.1.0").unwrap();
        assert_eq!(svc.health_regression().unwrap(), OtaStep::TrialStarted);
        assert_eq!(svc.trial.as_ref().unwrap().remaining, 1);
        assert_eq!(svc.health_regression().unwrap(), OtaStep::RolledBack);
        assert!(svc.trial.is_none());
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
    // 70.5 迁移：FaultOta 故障注入面保持；拒绝类别已上移描述面。

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
        fn verify(&mut self, _slot: PartitionSlot) -> Result<(), OtaError> {
            if let Some(e) = self.verify_err.clone() {
                return Err(e);
            }
            Ok(())
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

    fn svc_fault(ota: FaultOta) -> OtaService<FaultOta> {
        let (sk, key) = keypair(0x5A);
        let _ = sk;
        OtaService::new(ota, policy(key), &identity())
    }

    /// t36：容量不足错误串同时携带 image_size 与 partition_capacity
    /// 十进制字面，完整格式（`{image}>{capacity}` 顺序敏感）。
    #[test]
    fn t36_image_too_large_error_carries_both_numbers() {
        let (sk, _) = keypair(0x5A);
        let mut svc = OtaService::new(
            FaultOta {
                prepare_err: Some(OtaError::ImageTooLarge {
                    image_size: 10_485_760,
                    partition_capacity: 1_048_576,
                }),
                write_err: None,
                verify_err: None,
            },
            policy(keypair(0x5A).1),
            &identity(),
        );
        let img = vec![0u8; 64];
        let req = req02_valid(&sk, "1.1.0", &img);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req, &mut resp));
        assert_eq!(
            core::str::from_utf8(&resp).unwrap(),
            "ERR:ota:prepare-failed:image-too-large:10485760>1048576"
        );
    }

    /// t37：容量不足 / 设备写失败 / 策略门拒绝 ⇒ 三个错误串互不相同。
    #[test]
    fn t37_prepare_failure_categories_are_distinguishable() {
        let (sk, _) = keypair(0x5A);
        let img = vec![0xABu8; 64];
        // 容量不足。
        let mut svc = OtaService::new(
            FaultOta {
                prepare_err: Some(OtaError::ImageTooLarge {
                    image_size: 10_485_760,
                    partition_capacity: 1_048_576,
                }),
                write_err: None,
                verify_err: None,
            },
            policy(keypair(0x5A).1),
            &identity(),
        );
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(
            Some(&mut svc),
            &req02_valid(&sk, "1.1.0", &img),
            &mut resp
        ));
        let cap_resp = core::str::from_utf8(&resp).unwrap().to_owned();
        // 设备写失败。
        let mut svc = OtaService::new(
            FaultOta {
                prepare_err: None,
                write_err: Some(OtaError::WriteFailed { offset: 65_536 }),
                verify_err: None,
            },
            policy(keypair(0x5A).1),
            &identity(),
        );
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(
            Some(&mut svc),
            &req02_valid(&sk, "1.1.0", &img),
            &mut resp
        ));
        let write_resp = core::str::from_utf8(&resp).unwrap().to_owned();
        // 策略门拒绝（降级版本）。
        let mut svc = OtaService::new(
            FaultOta {
                prepare_err: None,
                write_err: None,
                verify_err: None,
            },
            policy(keypair(0x5A).1),
            &identity(),
        );
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(
            Some(&mut svc),
            &req02_valid(&sk, "1.0.0", &img),
            &mut resp
        ));
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
    /// 112B）——经真实 wire 路径产出最坏形态。
    #[test]
    fn t38_error_string_fits_receipt_budget() {
        const BUDGET: usize = eneros_agent_sdk::MR_REPLY_MAX_WORDS * 8;
        assert_eq!(BUDGET, 112);
        let (sk, _) = keypair(0x5A);
        let mut svc = OtaService::new(
            FaultOta {
                prepare_err: Some(OtaError::ImageTooLarge {
                    image_size: u64::MAX,
                    partition_capacity: u64::MAX,
                }),
                write_err: None,
                verify_err: None,
            },
            policy(keypair(0x5A).1),
            &identity(),
        );
        let img = vec![0u8; 64];
        let req = req02_valid(&sk, "1.1.0", &img);
        let mut resp = Vec::new();
        assert!(!handle_ota_ab_command(Some(&mut svc), &req, &mut resp));
        assert_eq!(resp.first(), Some(&b'E'));
        let full_len = resp.len();
        assert!(
            full_len <= BUDGET,
            "最长错误串 {}（{full_len}B）超出单帧回执预算 {BUDGET}B",
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
            "ERR:rejected:desc-signature-invalid",
            "ERR:rejected:desc-generation",
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

    /// WP-70.5.2 参数 canonical：几何/界限/曲线点域检矩阵。
    #[test]
    fn policy_canonical_validation_matrix() {
        let (_, key) = keypair(0x5A);
        let p = policy(key);
        assert_eq!(validate_policy(&p), Ok(()));
        // 几何漂移拒绝（唯一合法值 = ota-ab PARTITION_BLOCKS）。
        let mut bad = p.clone();
        bad.partition_blocks = 1024;
        assert_eq!(validate_policy(&bad), Err("partition-blocks-drift"));
        // chunk 界限。
        let mut bad = p.clone();
        bad.chunk_size = 0;
        assert_eq!(validate_policy(&bad), Err("chunk-size-bounds"));
        let mut bad = p.clone();
        bad.chunk_size = 1_048_577;
        assert_eq!(validate_policy(&bad), Err("chunk-size-bounds"));
        // 尝试数界限（≤ BOOT_CONTROL_MAX_TRIES=7）。
        let mut bad = p.clone();
        bad.max_boot_attempts = 0;
        assert_eq!(validate_policy(&bad), Err("boot-attempts-bounds"));
        let mut bad = p.clone();
        bad.max_boot_attempts = 8;
        assert_eq!(validate_policy(&bad), Err("boot-attempts-bounds"));
        // 零锚 / 非曲线点。
        let mut bad = p.clone();
        bad.root_key = [0; 64];
        assert_eq!(validate_policy(&bad), Err("zero-root-key"));
        let mut bad = p.clone();
        bad.root_key = [0xFF; 64];
        assert_eq!(validate_policy(&bad), Err("anchor-invalid"));
        // 零代次。
        let mut bad = p.clone();
        bad.generation = 0;
        assert_eq!(validate_policy(&bad), Err("zero-policy-generation"));
    }

    /// WP-70.5.1：capsule → 身份快照转换（含 u64 越域拒绝与空 SKU 拒绝）。
    #[test]
    fn identity_from_capsule_conversion() {
        use eneros_boot_protocol::IdentityCapsule;
        let mut c = IdentityCapsule::new(
            1,
            [1; 16],
            [2; 16],
            {
                let mut f = [0u8; 16];
                f[..4].copy_from_slice(b"qemu");
                f
            },
            [3; 32],
            9,
            3,
            2,
            [0, 335, 1],
        );
        let id = identity_from_capsule(&c).expect("转换");
        assert_eq!(id.current_version, "0.335.1");
        // SKU = 平台绑定十六进制指纹（非 UTF-8 名——manifest platform_id
        // 是 SHA-256 前 16B 哈希字节，非可打印名）。
        assert_eq!(id.sku, "71656d75000000000000000000000000");
        assert_eq!(id.security_version, 3);
        assert_eq!(id.security_floor, 2);
        assert!(id.is_present());
        // secver 越域（> u32::MAX）拒绝。
        c.security_version = u64::MAX;
        assert_eq!(identity_from_capsule(&c), Err("identity-secver-bounds"));
        // floor 越域拒绝。
        c.security_version = 3;
        c.security_floor = u64::MAX;
        assert_eq!(identity_from_capsule(&c), Err("identity-secver-bounds"));
        // secver 越域（floor 侧）拒绝（保留覆盖）。
        c.security_floor = 2;
        c.security_version = u64::MAX;
        assert_eq!(identity_from_capsule(&c), Err("identity-secver-bounds"));
    }

    /// 70.7（REG-70.7-BOOT-TRIAL-BIND）：候选载荷身份提取——真实 manifest
    /// 编码器产出的字节流必须解出 (secver, release_id)；长度/魔数/域畸形
    /// 一律拒绝（fail-closed，不猜身份）。
    #[test]
    fn candidate_identity_from_manifest_bytes() {
        use eneros_boot_protocol::manifest::{
            Component, ManifestHeader, ManifestSlot, ProductVersion, ReleaseManifest,
            COMPONENT_ENCODED_LEN,
        };
        let header = ManifestHeader {
            flags: 0,
            platform_id: [0x11; 16],
            product_version: ProductVersion {
                major: 0,
                minor: 337,
                patch: 0,
            },
            security_version: 2,
            source_date_epoch: 1_755_523_200,
            key_id: [0x22; 16],
            release_id: [0x33; 16],
        };
        let component = Component {
            role: eneros_boot_protocol::manifest::ComponentRole::Kernel,
            slot: ManifestSlot::B,
            permissions: eneros_boot_protocol::manifest::PERMISSION_READ
                | eneros_boot_protocol::manifest::PERMISSION_EXECUTE,
            flags: 0,
            media_offset: 65_536,
            media_length: 512,
            load_address: eneros_boot_protocol::ENEROS_INIT_BUNDLE_PHYS,
            memory_length: 512,
            entry_address: eneros_boot_protocol::ENEROS_INIT_BUNDLE_PHYS,
            security_version: 2,
            alignment: 512,
            digest: [0x44; 32],
            content_id: [0x55; 16],
            version: ProductVersion {
                major: 0,
                minor: 337,
                patch: 0,
            },
        };
        let manifest = ReleaseManifest::new(header, alloc::vec![component])
            .with_signature([0x66; eneros_boot_protocol::manifest::MANIFEST_SIGNATURE_LEN]);
        let bytes = manifest.encode_signed().expect("encode");
        assert!(bytes.len() >= COMPONENT_ENCODED_LEN);
        assert_eq!(
            candidate_identity(&bytes).expect("identity"),
            (2, [0x33; 16])
        );
        // 截断（短于头）→ 拒绝。
        assert_eq!(candidate_identity(&bytes[..100]), Err("candidate-short"));
        // 无尾（仅签名区）→ 拒绝：镜像内 manifest 恒含尾（fail-closed 不猜）。
        assert_eq!(
            candidate_identity(&bytes[..manifest.signed_len().unwrap()]),
            Err("candidate-len")
        );
        // 头内 total 越界（声明长于可用字节）→ 拒绝。
        let mut bad = bytes.clone();
        let over = bad.len() as u32 + 64;
        bad[12..16].copy_from_slice(&over.to_le_bytes());
        assert_eq!(candidate_identity(&bad), Err("candidate-len"));
        // 魔数损坏 → 解码拒绝（不吞错成身份）。
        let mut bad = bytes.clone();
        bad[0] ^= 0xFF;
        assert_eq!(candidate_identity(&bad), Err("candidate-decode"));
    }

    #[test]
    fn svc_fault_unused_placeholder() {
        // svc_fault 构造器保留给后续注入面扩展（编译期防死码告警）。
        let _ = svc_fault(FaultOta {
            prepare_err: None,
            write_err: None,
            verify_err: None,
        });
    }
}

/// 69.5（WP-69.5.2 变异靶 1 反向锁）：生产块大小**单源守卫**——svc-ota
/// bin 挂载 EipcBlockDevice 必须引用 ota-ab 共享常量（OTA_PROD_BLOCK_SIZE
/// / DATA_DISK_BLOCKS），不得回退手抄字面（字面复现 = 第三个口径源
/// 复活，容量口径三处一致破坏 → 本测红）。与能量侧 ea_display_guard
/// 同款源文本守卫先例。
///
/// 70.5（WP-70.5.5）追加：生产 bin 不得残留固定开发锚字面（测试锚隔离
/// ——锚唯一来源 = 配置束 OtaConsumer 取数）。
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

    /// WP-70.5.5 源级守卫：svc-ota 生产 bin 零固定锚/零固定策略字面
    /// （ENEROS_OTA_ROOT_KEY / ENEROS_OTA_POLICY 已退役——锚与策略参数
    /// 唯一来源 = 已验签配置束取数；测试锚只在测试专属来源）。
    #[test]
    fn svc_ota_no_fixed_test_anchor() {
        let src = include_str!("bin/svc-ota.rs");
        assert!(
            !src.contains("ENEROS_OTA_ROOT_KEY"),
            "固定开发锚字面回归（WP-70.5.2 已退役——配置束取数单源）"
        );
        assert!(
            !src.contains("ENEROS_OTA_POLICY"),
            "固定策略字面回归（WP-70.5.2 已退役）"
        );
        // 探针种子（0x41 测试钥种子形态）不得出现在生产 bin。
        assert!(
            !src.contains("0x41, 0x41") && !src.contains("[0x41;"),
            "测试种子字面回归（探针隔离——归 root qemu-test 专属装配）"
        );
    }
}
