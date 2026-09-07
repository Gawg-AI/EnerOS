//! A/B 分区数据模型与升级状态机（蓝图 §4.1 / §4.5）。
//!
//! `AbPartitionOta<D: BlockDevice>` 管理双分区镜像的写入、校验与激活：
//! - `prepare_upgrade` 选择非激活分区并回填元数据；
//! - `write_chunk` 经 `BlockDevice` 读-改-写真实落盘（整块语义，容忍非对齐分块）；
//! - `verify` 从存储回读镜像，SM3 增量哈希比对 + SM2 根公钥验签（D2）；
//! - `activate` 原子切换启动标志，旧分区保留 Verified 作回滚备用。

use alloc::vec;

use eneros_crypto::{ct_eq, Sm2PublicKey, Sm2Signature, Sm3Hasher};
use eneros_storage::BlockDevice;
use heapless::String as HString;

use crate::upgrade::{OtaError, OtaManager, UpgradeMeta};

/// 分区表保留块地址（块 0，见 [`crate::rollback`] 持久化）。
pub const META_BLOCK: u64 = 0;
/// A 槽位起始块地址。
pub const SLOT_A_BASE_BLOCK: u64 = 1;
/// 每个分区占用的块数（按 512 字节块计 1 MiB；实际口径随 `block_size()` 缩放）。
pub const PARTITION_BLOCKS: u64 = 2048;
/// B 槽位起始块地址。
pub const SLOT_B_BASE_BLOCK: u64 = SLOT_A_BASE_BLOCK + PARTITION_BLOCKS;

/// SPEC-C（D-TSDB-2 方案 A）：svc-storage 数据盘上让渡给 svc-telemetry 的
/// TSDB 区段起始块（512B 块计，字节 4 MiB）。避开全部既有占用：META 块 0、
/// 意图日志信封 [0,128KiB)、各业务状态区 [8KiB..64KiB)、A/B 槽 [块 1,
/// 4097)，前置 ≈2 MiB 保护间隙。内存审计与区段清单见
/// docs/delivery/wp-c-memory-audit-spec-c-20260827.md §三。
pub const DATA_TSDB_BASE_BLOCK: u64 = SLOT_B_BASE_BLOCK + PARTITION_BLOCKS + 4095;
/// TSDB 区段块数（512B 块计 8 MiB；16 MiB 数据盘上方再留 4 MiB 未分配余量）。
pub const DATA_TSDB_BLOCKS: u64 = 16384;

/// SPEC-E（WP-E.2，CAQ-2 下方余量划区）：svc-control-dispatch 决策审计链
/// 持久区段起始块（紧随 TSDB 末 = 字节 12 MiB；64 KiB = 64 槽 × 1 KiB，
/// 槽 = 单条 DurableAuditRecord）。区段清单同上 §三表；TSDB 实际只用其
/// 头部 256 KiB 窗口（LFS 卷几何），但物理区段边界按声明全长隔离。
pub const AUDIT_CHAIN_BASE_BLOCK: u64 = DATA_TSDB_BASE_BLOCK + DATA_TSDB_BLOCKS;
/// 决策审计链区段块数（512B 块计 64 KiB；容量策略 = 槽满丢新 + 计数）。
pub const AUDIT_CHAIN_BLOCKS: u64 = 128;

/// SPEC-E（WP-E.3）：svc-ai-worker 有界学习参数快照区段起始块
/// （紧随审计链区；4 KiB = RegionStateStore 双槽代次切换（E 续作订正，2 KiB/槽），余量充足）。
pub const LEARN_STATE_BASE_BLOCK: u64 = AUDIT_CHAIN_BASE_BLOCK + AUDIT_CHAIN_BLOCKS;
/// 学习参数快照区段块数（512B 块计 4 KiB）。
pub const LEARN_STATE_BLOCKS: u64 = 8;

/// 蓝图 68 卷 68.4（WP-68.4.0 裁决 2）：OBS 事件面分区事件日志区段起始块
/// （紧随学习参数区末 24712 = 4KiB 块 3089，零保护缝隙划区）。让渡方
/// svc-telemetry；几何 128 KiB = 3 固定块 + 7×16KiB segment 槽（event-store
/// StoreConfig segment_bytes=16384）+ 3 块余。命令面成本丈量与三项裁决见
/// 蓝图/68 卷 68.4 执行卡「执行记录」。
pub const DATA_EVENT_BASE_BLOCK: u64 = LEARN_STATE_BASE_BLOCK + LEARN_STATE_BLOCKS;
/// OBS 事件面区段块数（512B 块计 128 KiB）。
pub const DATA_EVENT_BLOCKS: u64 = 256;

/// 蓝图 68 卷 68.4：外部事实持久化区段起始块（紧随事件面）。让渡方
/// svc-market（StoreFactEventSink；几何与事件面同参）。
pub const DATA_FACT_BASE_BLOCK: u64 = DATA_EVENT_BASE_BLOCK + DATA_EVENT_BLOCKS;
/// 外部事实区段块数（512B 块计 128 KiB）。
pub const DATA_FACT_BLOCKS: u64 = 256;

/// 蓝图 68 卷 68.4：快照面 LFS 卷区段起始块（紧随事实面；256 KiB =
/// 64×4KiB，BlockDeviceStorage BLOCK_COUNT=64 卷几何上限内，与 TSDB 同
/// 卷几何纪律）。让渡方 svc-telemetry（SnapshotModule 快照 blob 存档）。
pub const DATA_SNAP_BASE_BLOCK: u64 = DATA_FACT_BASE_BLOCK + DATA_FACT_BLOCKS;
/// 快照面区段块数（512B 块计 256 KiB）。
pub const DATA_SNAP_BLOCKS: u64 = 512;

/// 数据盘总块数（512B 块计 16 MiB；审计表 §三"QEMU virtio-blk 数据面"）。
/// 块命令面整盘视口折算 4KiB 块 = 本数 / 8（EipcBlockDevice 面容量——
/// RegionScope 之下的寻址上界，非让渡语义；访问仍被区段钳制）。
pub const DATA_DISK_BLOCKS: u64 = 32768;

/// 蓝图 69 卷 69.5（WP-69.5.2 裁决 A+C）：**生产** svc-ota 挂载 OTA A/B
/// 时的设备块大小（字节）——单源常量，`svc-ota` bin 与容量口径锁测试
/// **都引用本值**（此前 512 是 svc-ota.rs:82 的手抄字面，configs/ota.toml
/// ⑧ 又按 chunk_size=64 KiB 心算容量，三个源互不知晓：生产单槽容量实为
/// `PARTITION_BLOCKS × 512 = 1 MiB`，与"10 MiB 性能基线"文档口径差
/// 128 倍且无机械信号）。
///
/// Z15 纪律：本值与 [`DATA_DISK_BLOCKS`] 的块号语义按 512 B 块绑定
/// （TSDB/审计/事件/事实/快照各区段基址均按 512B 块计）——**改本值等价
/// 改盘上几何**，须 owner 裁决 + 独立迁移方案，不属调参旋钮。
/// `production_slot_capacity_matches_documented_baseline` 测试锁
/// 「本常量 × [`PARTITION_BLOCKS`] ≥ configs/ota.toml ③ 声明的性能基线
/// 镜像尺寸」三处一致。
pub const OTA_PROD_BLOCK_SIZE: usize = 512;

/// 数据盘区段角色（布局类型化，SPEC-B/C 2026-08-28：`×512` 字节换算收进
/// 唯一 accessor，消灭消费端散抄魔数；常量数值逐字节不变）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataRegion {
    /// TSDB 历史区段（[`DATA_TSDB_BASE_BLOCK`]/[`DATA_TSDB_BLOCKS`]）。
    Tsdb,
    /// 决策审计链区段（[`AUDIT_CHAIN_BASE_BLOCK`]/[`AUDIT_CHAIN_BLOCKS`]）。
    AuditChain,
    /// 学习参数快照区段（[`LEARN_STATE_BASE_BLOCK`]/[`LEARN_STATE_BLOCKS`]）。
    LearnState,
    /// OBS 事件面区段（[`DATA_EVENT_BASE_BLOCK`]/[`DATA_EVENT_BLOCKS`]）。
    EventFace,
    /// 外部事实区段（[`DATA_FACT_BASE_BLOCK`]/[`DATA_FACT_BLOCKS`]）。
    FactFace,
    /// 快照面区段（[`DATA_SNAP_BASE_BLOCK`]/[`DATA_SNAP_BLOCKS`]）。
    SnapshotFace,
}

impl DataRegion {
    /// 区段基址（512B 块号，ota-ab 常量口径）。
    pub fn base_block_512(self) -> u64 {
        match self {
            DataRegion::Tsdb => DATA_TSDB_BASE_BLOCK,
            DataRegion::AuditChain => AUDIT_CHAIN_BASE_BLOCK,
            DataRegion::LearnState => LEARN_STATE_BASE_BLOCK,
            DataRegion::EventFace => DATA_EVENT_BASE_BLOCK,
            DataRegion::FactFace => DATA_FACT_BASE_BLOCK,
            DataRegion::SnapshotFace => DATA_SNAP_BASE_BLOCK,
        }
    }
    /// 区段长度（512B 块数）。
    pub fn blocks_512(self) -> u64 {
        match self {
            DataRegion::Tsdb => DATA_TSDB_BLOCKS,
            DataRegion::AuditChain => AUDIT_CHAIN_BLOCKS,
            DataRegion::LearnState => LEARN_STATE_BLOCKS,
            DataRegion::EventFace => DATA_EVENT_BLOCKS,
            DataRegion::FactFace => DATA_FACT_BLOCKS,
            DataRegion::SnapshotFace => DATA_SNAP_BLOCKS,
        }
    }
    /// 区段字节基址（`×512` 唯一换算点）。
    pub fn base_byte(self) -> u64 {
        self.base_block_512() * 512
    }
    /// 区段字节长度（`×512` 唯一换算点）。
    pub fn len_bytes(self) -> u64 {
        self.blocks_512() * 512
    }
}

/// 分区标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionSlot {
    /// 槽位 A
    A,
    /// 槽位 B
    B,
}

/// 分区状态（七态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionState {
    /// 空
    Empty,
    /// 写入中
    Writing,
    /// 写入完成待验证
    Written,
    /// 验证通过
    Verified,
    /// 当前启动
    Active,
    /// 验证失败
    Failed,
    /// 回滚中（瞬时中间态语义，不落盘，见 [`crate::rollback`]）
    Rolling,
}

/// 分区信息（蓝图 §4.1 九字段）。
#[derive(Debug, Clone)]
pub struct Partition {
    /// 槽位
    pub slot: PartitionSlot,
    /// 状态
    pub state: PartitionState,
    /// 镜像版本（≤32 字节）
    pub version: HString<32>,
    /// 镜像大小（字节）
    pub size: u64,
    /// 镜像 SM3 哈希
    pub hash: [u8; 32],
    /// 镜像 SM2 签名（r ‖ s，64 字节）
    pub signature: [u8; 64],
    /// 启动计数
    pub boot_count: u32,
    /// 最后启动时间（毫秒，由 `now_ms` 参数传入，D1）
    pub last_boot_ms: u64,
}

/// A/B 分区 OTA 管理器（D4：泛型化后端存储）。
pub struct AbPartitionOta<D: BlockDevice> {
    /// 双分区（索引 0 = A，1 = B）
    pub(crate) partitions: [Partition; 2],
    /// 当前激活槽位
    pub(crate) active_slot: PartitionSlot,
    /// 新分区连续启动尝试次数
    pub(crate) boot_attempts: u32,
    /// 最大启动尝试次数（默认 3，超限自动回滚）
    pub(crate) max_boot_attempts: u32,
    /// 设备根公钥（SM2 验签，D2）
    pub(crate) root_key: Sm2PublicKey,
    /// 后端块设备
    pub(crate) device: D,
    /// 进行中的升级元数据（Writing/Written 态有效）
    pub(crate) pending: Option<UpgradeMeta>,
}

impl<D: BlockDevice> AbPartitionOta<D> {
    /// 创建管理器：A 分区 Active(workspace v0.192.0)、B 分区 Empty、active_slot=A（D1/D2）。
    pub fn new(device: D, root_key: Sm2PublicKey, now_ms: u64) -> Self {
        Self {
            partitions: [
                Partition {
                    slot: PartitionSlot::A,
                    state: PartitionState::Active,
                    version: HString::<32>::try_from("0.0.0").unwrap_or_default(),
                    size: 0,
                    hash: [0; 32],
                    signature: [0; 64],
                    boot_count: 0,
                    last_boot_ms: now_ms,
                },
                Partition {
                    slot: PartitionSlot::B,
                    state: PartitionState::Empty,
                    version: HString::<32>::try_from("0.0.0").unwrap_or_default(),
                    size: 0,
                    hash: [0; 32],
                    signature: [0; 64],
                    boot_count: 0,
                    last_boot_ms: 0,
                },
            ],
            active_slot: PartitionSlot::A,
            boot_attempts: 0,
            max_boot_attempts: 3,
            root_key,
            device,
            pending: None,
        }
    }

    /// 获取非激活槽位。
    fn get_inactive_slot(&self) -> PartitionSlot {
        match self.active_slot {
            PartitionSlot::A => PartitionSlot::B,
            PartitionSlot::B => PartitionSlot::A,
        }
    }

    /// 槽位 → partitions 数组索引。
    pub(crate) fn slot_index(slot: PartitionSlot) -> usize {
        match slot {
            PartitionSlot::A => 0,
            PartitionSlot::B => 1,
        }
    }

    /// 槽位起始块地址。
    pub(crate) fn slot_base_block(slot: PartitionSlot) -> u64 {
        match slot {
            PartitionSlot::A => SLOT_A_BASE_BLOCK,
            PartitionSlot::B => SLOT_B_BASE_BLOCK,
        }
    }

    /// 槽容量（字节）= `PARTITION_BLOCKS` × 设备块大小——槽跨度在本模块
    /// 自身寻址中以设备块计（`SLOT_B_BASE = SLOT_A_BASE + PARTITION_
    /// BLOCKS`；生产设备 bs=512 ⇒ 1 MiB，integration 大块 mock 同式自洽）。
    /// 68 三轮复查（REG-OTA-SLOT-OVERWRITE）：AbPartitionOta 挂载的设备
    /// 是整数据盘（svc-ota `EipcBlockDevice::new(io, OTA_PROD_BLOCK_SIZE,
    /// DATA_DISK_BLOCKS)`），无槽容量域检时越槽写物理落入槽后块——保护
    /// 带 → TSDB/审计/学习/事件/事实/快照区段（同盘静默损毁），且自洽
    /// 超长镜像回读自洽可过 verify+activate。写面（write_bytes 硬界）与
    /// 声明面（prepare 早门）双门封闭。
    /// 69.5（WP-69.5.4）：pub(crate)——`KernelOta` 容量改派生（与本法
    /// 同一函数，删除注入式裸 u64 的第二口径源）。
    pub(crate) fn slot_cap_bytes(bs: usize) -> u64 {
        PARTITION_BLOCKS * bs as u64
    }

    /// 借出后端设备（持久化等只读场景）。
    pub fn device(&self) -> &D {
        &self.device
    }

    /// 将 `data` 写入槽位内偏移 `offset` 处（读-改-写保持整块语义）。
    fn write_bytes(
        &mut self,
        slot: PartitionSlot,
        offset: u64,
        data: &[u8],
    ) -> Result<(), OtaError> {
        // 68 三轮复查：槽容量硬界（写面唯一咽喉）——越界整体拒绝（无
        // 部分写），防越槽物理写破坏同盘数据区段。
        let bs = self.device.block_size();
        let end = offset.saturating_add(data.len() as u64);
        if end > Self::slot_cap_bytes(bs) {
            return Err(OtaError::WriteFailed { offset });
        }
        let base = Self::slot_base_block(slot);
        let mut buf = vec![0u8; bs];
        let mut written = 0usize;
        while written < data.len() {
            let cur = offset + written as u64;
            let block_idx = base + cur / bs as u64;
            let inner = (cur % bs as u64) as usize;
            let n = core::cmp::min(bs - inner, data.len() - written);
            // 读-改-写：write_block 为整块语义，容忍非块对齐的分块
            self.device
                .read_block(block_idx, &mut buf)
                .map_err(|_| OtaError::WriteFailed { offset: cur })?;
            buf[inner..inner + n].copy_from_slice(&data[written..written + n]);
            self.device
                .write_block(block_idx, &buf)
                .map_err(|_| OtaError::WriteFailed { offset: cur })?;
            written += n;
        }
        Ok(())
    }

    /// 从存储回读槽位镜像前 `size` 字节并增量计算 SM3 哈希。
    fn hash_slot_image(&self, slot: PartitionSlot, size: u64) -> Result<[u8; 32], OtaError> {
        let bs = self.device.block_size();
        let base = Self::slot_base_block(slot);
        let mut buf = vec![0u8; bs];
        let mut hasher = Sm3Hasher::new();
        let mut read: u64 = 0;
        while read < size {
            let block_idx = base + read / bs as u64;
            self.device
                .read_block(block_idx, &mut buf)
                .map_err(|_| OtaError::VerifyFailed)?;
            let n = core::cmp::min(bs as u64, size - read) as usize;
            hasher.update(&buf[..n]);
            read += n as u64;
        }
        Ok(hasher.finalize())
    }
}

impl<D: BlockDevice> OtaManager for AbPartitionOta<D> {
    fn prepare_upgrade(&mut self, mut meta: UpgradeMeta) -> Result<PartitionSlot, OtaError> {
        // CRY-006：版本单调防降级（target 不得低于当前激活版本）
        let cur_idx = Self::slot_index(self.active_slot);
        crate::upgrade::check_monotonic(
            meta.target_version.as_str(),
            self.partitions[cur_idx].version.as_str(),
        )?;
        // 68 三轮复查：声明镜像尺寸超槽容量即拒（早门——下游写面另有
        // write_bytes 硬界兜底；两门口径同源 slot_cap_bytes）。
        let cap = Self::slot_cap_bytes(self.device.block_size());
        if meta.image_size > cap {
            return Err(OtaError::ImageTooLarge {
                image_size: meta.image_size,
                partition_capacity: cap,
            });
        }
        // 选非激活分区（允许在 Failed/Empty/Verified 上重新发起，支持失败重试）
        let slot = self.get_inactive_slot();
        let idx = Self::slot_index(slot);
        meta.downloaded_chunks = 0;
        let p = &mut self.partitions[idx];
        p.state = PartitionState::Writing;
        p.version = meta.target_version.clone();
        p.size = meta.image_size;
        p.hash = meta.image_hash;
        p.signature = meta.image_sig;
        self.pending = Some(meta);
        Ok(slot)
    }

    fn write_chunk(
        &mut self,
        slot: PartitionSlot,
        offset: u64,
        data: &[u8],
    ) -> Result<(), OtaError> {
        let idx = Self::slot_index(slot);
        if self.partitions[idx].state != PartitionState::Writing {
            return Err(OtaError::WriteFailed { offset });
        }
        self.write_bytes(slot, offset, data)?;
        // 推进下载计数，全部分块写完置 Written
        let meta = match self.pending.as_mut() {
            Some(m) => m,
            None => return Err(OtaError::WriteFailed { offset }),
        };
        meta.downloaded_chunks += 1;
        if meta.downloaded_chunks >= meta.total_chunks {
            self.partitions[idx].state = PartitionState::Written;
        }
        Ok(())
    }

    fn verify(&mut self, slot: PartitionSlot) -> Result<bool, OtaError> {
        let idx = Self::slot_index(slot);
        if self.partitions[idx].state != PartitionState::Written {
            return Err(OtaError::VerifyFailed);
        }
        let size = self.partitions[idx].size;
        // 1) SM3 哈希校验（回读镜像，常时比较）
        let digest = self.hash_slot_image(slot, size)?;
        if !ct_eq(&digest, &self.partitions[idx].hash) {
            self.partitions[idx].state = PartitionState::Failed;
            return Err(OtaError::HashMismatch);
        }
        // 2) SM2 签名验证（设备根公钥）——WP-64.3.5：类型化原语
        // verify_signature（Ok(false)/Err 统一 Err → SignatureInvalid，
        // fail-closed 行为不变；布尔陷阱消亡）。
        let hash = self.partitions[idx].hash;
        let sig = Sm2Signature::from_bytes(&self.partitions[idx].signature);
        let sig_valid =
            eneros_signer_keyring::verify_signature(&hash, &sig, &self.root_key).is_ok();
        if !sig_valid {
            self.partitions[idx].state = PartitionState::Failed;
            return Err(OtaError::SignatureInvalid);
        }
        self.partitions[idx].state = PartitionState::Verified;
        Ok(true)
    }

    fn activate(&mut self, slot: PartitionSlot) -> Result<(), OtaError> {
        let idx = Self::slot_index(slot);
        if self.partitions[idx].state != PartitionState::Verified {
            return Err(OtaError::VerifyFailed);
        }
        // 新分区 Active 并清零启动计数；旧分区保留 Verified 作回滚备用
        self.partitions[idx].state = PartitionState::Active;
        self.partitions[idx].boot_count = 0;
        let old_idx = Self::slot_index(self.active_slot);
        self.partitions[old_idx].state = PartitionState::Verified;
        self.active_slot = slot;
        self.boot_attempts = 0;
        Ok(())
    }

    fn rollback(&mut self) -> Result<PartitionSlot, OtaError> {
        crate::rollback::rollback(self)
    }

    fn get_active_slot(&self) -> PartitionSlot {
        self.active_slot
    }

    fn get_partitions(&self) -> [Partition; 2] {
        self.partitions.clone()
    }
}

// ============================================================================
// 单元测试（PRT1~3 分区模型 / UPG4~8 升级流程）
// ============================================================================

#[cfg(test)]
mod tests {

    fn new_ota() -> AbPartitionOta<MockBlockDevice> {
        let dev = MockBlockDevice::new(64, 512);
        AbPartitionOta::new(
            dev,
            Sm2KeyPair::generate(&mut CsRng::new()).unwrap().public_key,
            0,
        )
    }

    #[test]
    fn test_cry006_prepare_upgrade_rejects_downgrade() {
        // 当前 A 激活版本 0.123.0 → 尝试装 0.122.0 被拒（防降级）
        let mut ota = new_ota();
        // 先做一次升级到 0.123.0 并激活
        let meta = UpgradeMeta {
            target_version: HString::try_from("0.123.0").unwrap(),
            image_size: 4096,
            image_hash: [0x11; 32],
            image_sig: [0x22; 64],
            download_url: HString::try_from("http://x/0.123.0").unwrap(),
            chunk_size: 4096,
            total_chunks: 1,
            downloaded_chunks: 0,
        };
        let slot = ota.prepare_upgrade(meta.clone()).unwrap();
        let idx = AbPartitionOta::<MockBlockDevice>::slot_index(slot);
        ota.partitions[idx].state = PartitionState::Active;
        ota.active_slot = slot;
        // 降级尝试必须拒绝
        let mut down_meta = meta.clone();
        down_meta.target_version = HString::try_from("0.122.0").unwrap();
        assert_eq!(
            ota.prepare_upgrade(down_meta),
            Err(OtaError::DowngradeRejected)
        );
        // 升级放行
        let mut up_meta = meta.clone();
        up_meta.target_version = HString::try_from("0.124.0").unwrap();
        assert!(ota.prepare_upgrade(up_meta).is_ok());
    }

    #[test]
    fn test_cry006_boot_count_rollback_contract() {
        // 启动计数：同一分区连续失败 max_boot_attempts(3) 次后自动回滚
        let mut ota = new_ota();
        // 模拟：A 激活，B 已验证可回滚
        ota.partitions[0].state = PartitionState::Active;
        ota.partitions[1].state = PartitionState::Verified;
        ota.active_slot = PartitionSlot::A;
        // 前 3 次健康检查通过（计数累积）
        for i in 0..3 {
            assert!(
                crate::rollback::boot_health_check(&mut ota, i as u64),
                "第 {} 次启动未触发回滚",
                i
            );
        }
        // 第 4 次超过 max_boot_attempts → 自动回滚到 B 并返回 false
        assert!(!crate::rollback::boot_health_check(&mut ota, 3));
        assert_eq!(ota.active_slot, PartitionSlot::B, "回滚到备用分区");
        assert_eq!(
            ota.partitions[0].state,
            PartitionState::Failed,
            "失败分区标记 Failed"
        );
        assert_eq!(ota.boot_attempts, 0, "回滚后计数清零");
    }

    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
    use eneros_storage::MockBlockDevice;

    use super::*;

    /// 设备总块数：保留块 + 双分区。
    const TEST_BLOCKS: u64 = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS;

    /// 构造测试环境：真实 SM2 密钥对 + MockBlockDevice。
    fn setup() -> (AbPartitionOta<MockBlockDevice>, Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
        let dev = MockBlockDevice::new(TEST_BLOCKS, 512);
        let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        (ota, kp, rng)
    }

    /// 构造正样本镜像与元数据：真实 SM3 哈希 + 根私钥 SM2 签名。
    fn make_image(
        kp: &Sm2KeyPair,
        rng: &mut CsRng,
        size: usize,
        chunk: u32,
    ) -> (alloc::vec::Vec<u8>, UpgradeMeta) {
        let image: alloc::vec::Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
        let hash = sm3_hash(&image);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng).expect("签名失败");
        let meta = UpgradeMeta {
            target_version: HString::<32>::try_from("0.123.0").unwrap_or_default(),
            image_size: size as u64,
            image_hash: hash,
            image_sig: sig.to_bytes(),
            download_url: HString::<128>::try_from("https://ota.eneros.local/fw.bin")
                .unwrap_or_default(),
            chunk_size: chunk,
            total_chunks: (size as u32).div_ceil(chunk),
            downloaded_chunks: 0,
        };
        (image, meta)
    }

    /// PRT1：初始状态字段断言。
    #[test]
    fn prt1_initial_state() {
        let (ota, _, _) = setup();
        let parts = ota.get_partitions();
        assert_eq!(ota.get_active_slot(), PartitionSlot::A);
        assert_eq!(parts[0].slot, PartitionSlot::A);
        assert_eq!(parts[0].state, PartitionState::Active);
        assert_eq!(parts[0].version.as_str(), "0.0.0");
        assert_eq!(parts[0].size, 0);
        assert_eq!(parts[0].boot_count, 0);
        assert_eq!(parts[0].last_boot_ms, 1_000);
        assert_eq!(parts[1].slot, PartitionSlot::B);
        assert_eq!(parts[1].state, PartitionState::Empty);
        assert_eq!(ota.boot_attempts, 0);
        assert_eq!(ota.max_boot_attempts, 3);
    }

    /// PRT2：非激活槽位随 active_slot 切换。
    #[test]
    fn prt2_inactive_slot_switch() {
        let (mut ota, _, _) = setup();
        assert_eq!(ota.get_inactive_slot(), PartitionSlot::B);
        ota.active_slot = PartitionSlot::B;
        assert_eq!(ota.get_inactive_slot(), PartitionSlot::A);
    }

    /// PRT3：prepare_upgrade 回填元数据并置 Writing。
    #[test]
    fn prt3_prepare_backfills_meta() {
        let (mut ota, kp, mut rng) = setup();
        let (_, meta) = make_image(&kp, &mut rng, 1024, 512);
        let slot = ota.prepare_upgrade(meta.clone()).expect("prepare 应成功");
        assert_eq!(slot, PartitionSlot::B);
        let parts = ota.get_partitions();
        assert_eq!(parts[1].state, PartitionState::Writing);
        assert_eq!(parts[1].version.as_str(), "0.123.0");
        assert_eq!(parts[1].size, 1024);
        assert_eq!(parts[1].hash, meta.image_hash);
        assert_eq!(parts[1].signature, meta.image_sig);
        // A 分区不受影响
        assert_eq!(parts[0].state, PartitionState::Active);
        // 进行中元数据 downloaded_chunks 归零
        let pending = ota.pending.as_ref().expect("pending 应存在");
        assert_eq!(pending.downloaded_chunks, 0);
        assert_eq!(pending.total_chunks, 2);
    }

    /// UPG4：完整状态机 Writing→Written→Verified→Active。
    #[test]
    fn upg4_full_upgrade_flow() {
        let (mut ota, kp, mut rng) = setup();
        let (image, meta) = make_image(&kp, &mut rng, 1024, 512);
        let slot = ota.prepare_upgrade(meta).expect("prepare");
        assert_eq!(slot, PartitionSlot::B);
        assert_eq!(ota.get_partitions()[1].state, PartitionState::Writing);

        // 分块写入（含跨块读-改-写路径）
        ota.write_chunk(slot, 0, &image[..512]).expect("chunk0");
        assert_eq!(ota.get_partitions()[1].state, PartitionState::Writing);
        ota.write_chunk(slot, 512, &image[512..]).expect("chunk1");
        assert_eq!(ota.get_partitions()[1].state, PartitionState::Written);

        // 校验：真实 SM3 回读 + SM2 验签
        assert!(ota.verify(slot).expect("verify 应通过"));
        assert_eq!(ota.get_partitions()[1].state, PartitionState::Verified);

        // 激活：原子切换
        ota.activate(slot).expect("activate 应成功");
        let parts = ota.get_partitions();
        assert_eq!(parts[1].state, PartitionState::Active);
        assert_eq!(parts[1].boot_count, 0);
        assert_eq!(
            parts[0].state,
            PartitionState::Verified,
            "旧分区保留 Verified 备用"
        );
        assert_eq!(ota.get_active_slot(), PartitionSlot::B);
        assert_eq!(ota.boot_attempts, 0);
    }

    /// UPG5：非 Writing 态 write_chunk 拒绝。
    #[test]
    fn upg5_write_chunk_rejected_when_not_writing() {
        let (mut ota, _, _) = setup();
        // B 处于 Empty 态
        let err = ota
            .write_chunk(PartitionSlot::B, 0, &[0xAA; 64])
            .expect_err("非 Writing 态应拒绝");
        assert_eq!(err, OtaError::WriteFailed { offset: 0 });
        assert_eq!(ota.get_partitions()[1].state, PartitionState::Empty);
    }

    /// UPG6：哈希不匹配 → Failed + HashMismatch。
    #[test]
    fn upg6_hash_mismatch_marks_failed() {
        let (mut ota, kp, mut rng) = setup();
        let (image, mut meta) = make_image(&kp, &mut rng, 512, 512);
        // 篡改期望哈希（与实际镜像内容不符）
        meta.image_hash = [0xEE; 32];
        let slot = ota.prepare_upgrade(meta).expect("prepare");
        ota.write_chunk(slot, 0, &image).expect("write");
        let err = ota.verify(slot).expect_err("哈希不匹配应拒绝");
        assert_eq!(err, OtaError::HashMismatch);
        assert_eq!(ota.get_partitions()[1].state, PartitionState::Failed);
    }

    /// UPG7：篡改签名 → Failed + SignatureInvalid。
    #[test]
    fn upg7_tampered_signature_marks_failed() {
        let (mut ota, kp, mut rng) = setup();
        let (image, mut meta) = make_image(&kp, &mut rng, 512, 512);
        // 篡改签名字节（哈希仍匹配，验签应失败）
        meta.image_sig[0] ^= 0xFF;
        let slot = ota.prepare_upgrade(meta).expect("prepare");
        ota.write_chunk(slot, 0, &image).expect("write");
        let err = ota.verify(slot).expect_err("签名无效应拒绝");
        assert_eq!(err, OtaError::SignatureInvalid);
        assert_eq!(ota.get_partitions()[1].state, PartitionState::Failed);
    }

    /// UPG8：未 Verified 的分区 activate 拒绝。
    #[test]
    fn upg8_activate_rejected_when_not_verified() {
        let (mut ota, kp, mut rng) = setup();
        let (image, meta) = make_image(&kp, &mut rng, 512, 512);
        let slot = ota.prepare_upgrade(meta).expect("prepare");
        ota.write_chunk(slot, 0, &image).expect("write");
        // Written 态（未 verify）直接 activate
        let err = ota.activate(slot).expect_err("未 Verified 应拒绝");
        assert_eq!(err, OtaError::VerifyFailed);
        assert_eq!(ota.get_active_slot(), PartitionSlot::A);
        // Empty 态同样拒绝
        let err2 = ota.activate(PartitionSlot::B).expect_err("仍应拒绝");
        assert_eq!(err2, OtaError::VerifyFailed);
    }

    // ---- 68 三轮复查（REG-OTA-SLOT-OVERWRITE）：槽容量域 ----------------

    /// 越槽写物理落点测试（红测先行）：设备大于双槽（svc-ota 真实形态 =
    /// 整数据盘 32768 块）时，槽末之后的 write 此前物理成功落入槽后块
    /// （保护带→TSDB/审计/事件/事实/快照区段——同盘静默损毁）。红=写
    /// 成功且槽后块非零。变异靶：去掉 write_bytes 槽容量域检 → 本测红。
    #[test]
    fn t10_write_beyond_slot_cap_rejected_and_physical_block_untouched() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
        // 设备 = 双槽 + 槽后 8 块：暴露越槽写的真实落点。
        let dev = MockBlockDevice::new(TEST_BLOCKS + 8, 512);
        let mut ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
        let (_, meta) = make_image(&kp, &mut rng, 512, 512);
        let slot = ota.prepare_upgrade(meta).expect("prepare");
        let base = AbPartitionOta::<MockBlockDevice>::slot_base_block(slot);
        let cap = PARTITION_BLOCKS * 512; // bs=512 设备的槽容量（字节）
                                          // 槽末之后 512B 写：必须拒绝。
        let err = ota
            .write_chunk(slot, cap, &[0xAA; 512])
            .expect_err("越槽写必须拒绝（红=物理写入槽后区段）");
        assert!(matches!(err, OtaError::WriteFailed { .. }));
        // 物理证据：槽后第一块必须保持零。
        let mut blk = vec![0u8; 512];
        ota.device()
            .read_block(base + PARTITION_BLOCKS, &mut blk)
            .expect("read");
        assert!(
            blk.iter().all(|&b| b == 0),
            "槽后物理块不得被写入（TSDB/事件/事实/快照区段同盘）"
        );
        // 跨界写（最后 256B 在槽内 + 256B 越槽）：整体拒绝（无部分写）。
        let err2 = ota
            .write_chunk(slot, cap - 256, &[0xBB; 512])
            .expect_err("跨界写必须整体拒绝");
        assert!(matches!(err2, OtaError::WriteFailed { .. }));
    }

    /// 声明镜像尺寸超槽容量：prepare 即拒（早门，红=Ok 放行）。
    /// 变异靶：去掉 prepare_upgrade 的 ImageTooLarge 门 → 本测红。
    #[test]
    fn t10b_prepare_rejects_oversized_declared_image() {
        let (mut ota, kp, mut rng) = setup();
        let (_, mut meta) = make_image(&kp, &mut rng, 512, 512);
        let cap = PARTITION_BLOCKS * 512;
        meta.image_size = cap + 512;
        assert_eq!(
            ota.prepare_upgrade(meta),
            Err(OtaError::ImageTooLarge {
                image_size: cap + 512,
                partition_capacity: cap,
            }),
            "声明尺寸超槽容量必须在 prepare 拒绝（写面另有硬界）"
        );
    }

    // ---- 69.5（WP-69.5.2 裁决 A+C）：容量口径单源锁 --------------------

    /// 容量口径**文档一致性锁**（变异靶 3 的牙；出口清单第 4 项的机械
    /// 锚）：configs/ota.toml ③⑥⑧ 订正后的关键文字逐条锁定——撤回 ⑧
    /// 订正（恢复 chunk/block 混淆心算）或改掉 ③ 的 1 MiB 裁决口径 →
    /// 本测红（与 production_slot_capacity_matches_documented_baseline
    /// 组成"文字 ↔ 常量 ↔ 断言"三处一致的文字侧）。
    #[test]
    fn ota_toml_capacity_caliber_doc_locked() {
        let toml = include_str!("../../../../configs/ota.toml");
        // ③：生产实际口径 + 裁决基线（旧"当前性能基线 10 MiB"表述须消失）。
        assert!(
            toml.contains("2048 × 512 B ="),
            "③ 须写明生产实际容量 = 2048 × 512 B"
        );
        assert!(toml.contains("≤ 1 MiB"), "③ 性能基线须如实化为 ≤ 1 MiB");
        assert!(
            !toml.contains("当前性能"),
            "③ 旧「当前性能基线 10 MiB 镜像」表述须已订正"
        );
        // ⑥：生产后端块大小注记 + 单源常量指向（公式本身不动）。
        assert!(toml.contains("OTA_PROD_BLOCK_SIZE"), "⑥ 须指向单源常量");
        assert!(toml.contains("512 B"), "⑥ 须注明生产后端 = 512 B");
        // ⑧：chunk_size 与设备块大小两个量的区分（混淆心算在位即红）。
        assert!(
            toml.contains("不是**设备块大小"),
            "⑧ 须明确 chunk_size ≠ 设备块大小（容量按 ⑥ block_size 算）"
        );
        assert!(
            !toml.contains("10 MiB 镜像 = 160 块。"),
            "⑧ 旧混淆心算句须已订正"
        );
    }

    /// configs/ota.toml ③ 声明的性能基线镜像尺寸（69.5 裁决 A-1：基线按
    /// 生产实际口径如实化为 1 MiB——生产单槽容量 = PARTITION_BLOCKS ×
    /// OTA_PROD_BLOCK_SIZE = 1 MiB；原「10 MiB 基线」在当前盘上几何下
    /// 不成立，>1 MiB 镜像能力须 owner 裁决 + 盘上迁移方案（Z15））。
    const BASELINE_IMAGE_BYTES: u64 = 1024 * 1024;

    /// 容量口径单源锁（WP-69.5.2 的牙）：configs/ota.toml ③「单分区容量
    /// ≥ 最大镜像尺寸」此前只活在文档心算里（⑧ 把 chunk_size 当
    /// block_size，隐含 64 KiB 块 ⇒ 心算自洽 128 MiB），而生产 svc-ota
    /// 以 512 B 块实例化 ⇒ 实际容量 1 MiB，差 128 倍且无机械信号。本锁
    /// 把「生产块大小」（[`OTA_PROD_BLOCK_SIZE`] 单源常量，svc-ota bin
    /// 同引用）与「声明的基线镜像尺寸」绑成一处可执行断言——文档 ③⑥⑧
    /// / 常量 / 断言三者任一漂移即红。
    /// 变异靶 1（把常量改成 65536 假装生产是大块）→ 末行 512 语义锚红 +
    /// svc-ota 实例化行为连带改变（bin 侧另有源文本守卫锁引用关系）。
    #[test]
    fn production_slot_capacity_matches_documented_baseline() {
        let cap = AbPartitionOta::<MockBlockDevice>::slot_cap_bytes(OTA_PROD_BLOCK_SIZE);
        assert_eq!(cap, PARTITION_BLOCKS * OTA_PROD_BLOCK_SIZE as u64);
        assert!(
            cap >= BASELINE_IMAGE_BYTES,
            "生产单槽容量 {cap} B < 文档性能基线 {BASELINE_IMAGE_BYTES} B —— \
             容量口径分歧（configs/ota.toml ③⑥⑧ 与 OTA_PROD_BLOCK_SIZE 不同源；\
             Z15：改生产块大小须 owner 裁决 + 盘上迁移方案）"
        );
        // 512 B 语义锚：全部盘上常量（DATA_*_BASE_BLOCK 族）的块号语义
        // 按 512 B 块绑定（Z15）——这不是"第二来源"，是把常量钉在盘上
        // 几何纪律上的可执行锚（改常量而不改盘 = 本行红）。
        assert_eq!(OTA_PROD_BLOCK_SIZE, 512);
    }
}

#[cfg(test)]
mod spec_c_region_tests {
    /// SPEC-C（D-TSDB-2 方案 A）：TSDB 区段与全部既有数据盘占用非交叠
    /// （内存审计表 docs/delivery/wp-c-memory-audit-spec-c-20260827.md §三
    /// 的 const 级防线——改布局常量必须先过审计表）。
    #[test]
    fn data_tsdb_region_disjoint_from_existing_claims() {
        use super::{DATA_TSDB_BASE_BLOCK, DATA_TSDB_BLOCKS, META_BLOCK, PARTITION_BLOCKS};
        let tsdb = DATA_TSDB_BASE_BLOCK..DATA_TSDB_BASE_BLOCK + DATA_TSDB_BLOCKS;
        // OTA 元数据块 0。
        assert!(!tsdb.contains(&META_BLOCK), "不得覆盖 OTA 元数据块");
        // A/B 镜像槽（块 1 .. 4097）。
        let slots = super::SLOT_A_BASE_BLOCK..super::SLOT_B_BASE_BLOCK + PARTITION_BLOCKS;
        assert!(
            tsdb.start >= slots.end,
            "TSDB 区段 {tsdb:?} 必须整体位于 A/B 槽之后"
        );
        // 意图日志信封（字节 [0, 64+255*512) → 512B 块 [0, 256)）。
        const INTENT_ENVELOPE_BYTES: u64 = 64 + 255 * 512;
        const INTENT_ENVELOPE_BLOCKS: u64 = INTENT_ENVELOPE_BYTES.div_ceil(512);
        assert!(tsdb.start >= INTENT_ENVELOPE_BLOCKS, "不得覆盖意图日志信封");
        // 各业务状态区（OTA trial/ledger/market/cloud/bus/obs ≤ 64KiB+长度，
        // 保守以 128 KiB 上界核验，细节见审计表 §三清单）。
        assert!(
            tsdb.start * 512 >= 256 * 1024,
            "字节基址必须 ≥256 KiB 保护带"
        );
        // 区段自洽：16 MiB 数据盘内且上留余量。
        assert_eq!(tsdb.start, 8192, "默认基址=字节 4MiB（审计表 §三）");
        assert!(tsdb.end <= 32768 - 4096, "上方须保留 ≥2 MiB 未分配余量");
    }

    /// SPEC-E（WP-E.2/E.3）：审计链区段与学习参数区段紧随 TSDB 且两两
    /// 非交叠、盘内收敛（CAQ-2 下方 4 MiB 余量内的划区防线）。
    #[test]
    fn spec_e_audit_and_learn_regions_disjoint() {
        use super::{
            AUDIT_CHAIN_BASE_BLOCK, AUDIT_CHAIN_BLOCKS, DATA_TSDB_BASE_BLOCK, DATA_TSDB_BLOCKS,
            LEARN_STATE_BASE_BLOCK, LEARN_STATE_BLOCKS,
        };
        let tsdb = DATA_TSDB_BASE_BLOCK..DATA_TSDB_BASE_BLOCK + DATA_TSDB_BLOCKS;
        let audit = AUDIT_CHAIN_BASE_BLOCK..AUDIT_CHAIN_BASE_BLOCK + AUDIT_CHAIN_BLOCKS;
        let learn = LEARN_STATE_BASE_BLOCK..LEARN_STATE_BASE_BLOCK + LEARN_STATE_BLOCKS;
        assert_eq!(
            audit.start, tsdb.end,
            "审计链区必须紧随 TSDB（无保护间隙=零缝隙划区）"
        );
        assert_eq!(learn.start, audit.end, "学习参数区必须紧随审计链区");
        // 盘内收敛：16 MiB 盘（32768 块）内且不侵 TSDB 上方既有余量声明。
        assert!(learn.end <= 32768, "学习参数区越出 16 MiB 数据盘");
        // 相互隔离（顺序声明不等于隔离——双保险）。
        assert!(!audit.contains(&learn.start) && !learn.contains(&audit.start));
    }

    /// 蓝图 68 卷 68.4（WP-68.4.0 裁决 2）：全数据盘区段表（六区段）
    /// 排序不相交 + 4KiB 对齐 + 盘内收敛——执行卡「区段分配」表的无重叠
    /// 证明 const 级防线。新增区段必须先过本测试与内存审计。
    #[test]
    fn data_regions_disjoint_aligned_and_bounded() {
        use super::{
            DataRegion, AUDIT_CHAIN_BASE_BLOCK, AUDIT_CHAIN_BLOCKS, DATA_EVENT_BASE_BLOCK,
            DATA_EVENT_BLOCKS, DATA_FACT_BASE_BLOCK, DATA_FACT_BLOCKS, DATA_SNAP_BASE_BLOCK,
            DATA_SNAP_BLOCKS, DATA_TSDB_BASE_BLOCK, DATA_TSDB_BLOCKS, LEARN_STATE_BASE_BLOCK,
            LEARN_STATE_BLOCKS,
        };
        const DATA_DISK_BLOCKS: u64 = 32768; // 16 MiB
        let regions = [
            ("tsdb", DATA_TSDB_BASE_BLOCK, DATA_TSDB_BLOCKS),
            ("audit", AUDIT_CHAIN_BASE_BLOCK, AUDIT_CHAIN_BLOCKS),
            ("learn", LEARN_STATE_BASE_BLOCK, LEARN_STATE_BLOCKS),
            ("event", DATA_EVENT_BASE_BLOCK, DATA_EVENT_BLOCKS),
            ("fact", DATA_FACT_BASE_BLOCK, DATA_FACT_BLOCKS),
            ("snap", DATA_SNAP_BASE_BLOCK, DATA_SNAP_BLOCKS),
        ];
        // 排序不相交：前区段末 ≤ 后区段起（允许相邻，禁止重叠/倒序）。
        for w in regions.windows(2) {
            let (prev_end, next_start) = (w[0].1 + w[0].2, w[1].1);
            assert!(
                prev_end <= next_start,
                "区段 {} [{}] 与 {} [{}] 重叠/倒序",
                w[0].0,
                prev_end,
                w[1].0,
                next_start
            );
        }
        // 4KiB 对齐：基址与长度均为 4096B 整除（512B 块即 8 块整除）。
        // 例外：学习区长度 8 块 = 4KiB 恰好对齐；全表零例外。
        for (name, base, blocks) in regions {
            assert_eq!((base * 512) % 4096, 0, "区段 {name} 字节基址非 4KiB 对齐");
            assert_eq!((blocks * 512) % 4096, 0, "区段 {name} 长度非 4KiB 对齐");
        }
        // 盘内收敛 + 未分配余量 ≥2 MiB（上方未来卷划区空间）。
        let last_end = regions[regions.len() - 1].1 + regions[regions.len() - 1].2;
        assert!(last_end <= DATA_DISK_BLOCKS, "区段表越出 16 MiB 数据盘");
        assert!(
            DATA_DISK_BLOCKS - last_end >= 4096,
            "上方未分配余量 <2 MiB（未来卷划区空间不足）"
        );
        // DataRegion 枚举与常量同源（accessor 零漂移）。
        for r in [
            DataRegion::Tsdb,
            DataRegion::AuditChain,
            DataRegion::LearnState,
            DataRegion::EventFace,
            DataRegion::FactFace,
            DataRegion::SnapshotFace,
        ] {
            let hit = regions
                .iter()
                .find(|(_, b, n)| *b == r.base_block_512() && *n == r.blocks_512());
            assert!(hit.is_some(), "DataRegion::{r:?} 与常量表不同源");
        }
    }
}
