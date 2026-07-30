//! 内核 OTA 主逻辑（v0.163.0 蓝图 §4.2）。
//!
//! [`KernelOta`] 在 [`AbPartitionOta`] 基础上封装内核级 OTA 完整流程：
//! 验签 → 写入备用分区 → 标记可启动 → 设置下次启动分区 → 重启/热迁移。
//!
//! # 两种升级路径
//! - **重启切换**（`update`）：简单可靠，写入备用分区后 reboot 由 Bootloader 切换；
//! - **热迁移**（`hot_migrate`）：零停机，在线迁移线程/内存/设备状态到新内核。
//!
//! # no_std 合规
//! 本模块不使用 `std::*`；时间戳经 `now_ms` 参数传入。

use eneros_storage::BlockDevice;
use heapless::String as HString;

use crate::ab_partition::{AbPartitionOta, PartitionSlot, PartitionState};
use crate::rollback::{boot_health_check, save_to_device};
use crate::upgrade::{OtaError, OtaManager, UpgradeMeta};

/// 内核镜像元数据。
#[derive(Debug, Clone)]
pub struct KernelImage {
    /// 内核版本号（≤32 字节）
    pub version: HString<32>,
    /// 镜像数据
    pub data: alloc::vec::Vec<u8>,
    /// SM3 哈希
    pub hash: [u8; 32],
    /// SM2 签名（r ‖ s，64 字节）
    pub signature: [u8; 64],
}

/// 内核 OTA 错误类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelOtaError {
    /// 底层 OTA 错误
    Ota(OtaError),
    /// 镜像过大，超出分区容量
    ImageTooLarge {
        /// 镜像大小
        image_size: u64,
        /// 分区容量
        partition_capacity: u64,
    },
    /// 热迁移失败
    MigrateFailed(MigrateError),
    /// 当前无可用备用分区
    NoInactivePartition,
}

impl From<OtaError> for KernelOtaError {
    fn from(e: OtaError) -> Self {
        KernelOtaError::Ota(e)
    }
}

impl From<MigrateError> for KernelOtaError {
    fn from(e: MigrateError) -> Self {
        KernelOtaError::MigrateFailed(e)
    }
}

/// 热迁移错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateError {
    /// 线程状态迁移失败
    ThreadMigrationFailed,
    /// 内存映射迁移失败
    MemoryMigrationFailed,
    /// 设备状态迁移失败
    DeviceMigrationFailed,
    /// 执行流切换失败
    SwitchFailed,
    /// 新内核加载失败
    LoadFailed,
}

/// 内核 OTA 管理器（蓝图 §4.2 KernelOta）。
///
/// 泛型参数 `D` 为后端块设备（eMMC / MockBlockDevice）。
pub struct KernelOta<D: BlockDevice> {
    /// 底层 A/B 分区 OTA 管理器
    ab: AbPartitionOta<D>,
    /// 分区容量（字节）
    partition_capacity: u64,
}

impl<D: BlockDevice> KernelOta<D> {
    /// 创建内核 OTA 管理器。
    ///
    /// - `device`：后端块设备
    /// - `root_key`：设备根公钥（SM2 验签）
    /// - `now_ms`：当前时间戳（毫秒）
    /// - `partition_capacity`：单分区容量（字节）
    pub fn new(
        device: D,
        root_key: eneros_crypto::Sm2PublicKey,
        now_ms: u64,
        partition_capacity: u64,
    ) -> Self {
        Self {
            ab: AbPartitionOta::new(device, root_key, now_ms),
            partition_capacity,
        }
    }

    /// 获取底层 A/B 分区管理器引用。
    pub fn ab_partition(&self) -> &AbPartitionOta<D> {
        &self.ab
    }

    /// 获取底层 A/B 分区管理器可变引用。
    pub fn ab_partition_mut(&mut self) -> &mut AbPartitionOta<D> {
        &mut self.ab
    }

    /// 执行内核 OTA（重启切换模式）。
    ///
    /// 流程：
    /// 1. 验证新内核镜像大小
    /// 2. 准备升级（选择备用分区）
    /// 3. 分块写入新内核
    /// 4. 校验（SM3 哈希 + SM2 验签）
    /// 5. 激活备用分区
    /// 6. 持久化分区表
    ///
    /// 调用后由外部触发 reboot，Bootloader 读取 BootConfig 切换分区。
    pub fn update(&mut self, image: &KernelImage, chunk_size: u32) -> Result<(), KernelOtaError> {
        // 1. 检查镜像大小
        let image_size = image.data.len() as u64;
        if image_size > self.partition_capacity {
            return Err(KernelOtaError::ImageTooLarge {
                image_size,
                partition_capacity: self.partition_capacity,
            });
        }

        // 2. 构造升级元数据
        let total_chunks = (image_size as u32).div_ceil(chunk_size);
        let meta = UpgradeMeta {
            target_version: image.version.clone(),
            image_size,
            image_hash: image.hash,
            image_sig: image.signature,
            download_url: HString::<128>::try_from("local://kernel-ota").unwrap_or_default(),
            chunk_size,
            total_chunks,
            downloaded_chunks: 0,
        };

        // 3. 准备升级（选择备用分区）
        let slot = self.ab.prepare_upgrade(meta)?;

        // 4. 分块写入
        let mut offset: u64 = 0;
        let mut remaining = &image.data[..];
        while !remaining.is_empty() {
            let n = core::cmp::min(chunk_size as usize, remaining.len());
            self.ab.write_chunk(slot, offset, &remaining[..n])?;
            offset += n as u64;
            remaining = &remaining[n..];
        }

        // 5. 校验（SM3 + SM2）
        let verified = self.ab.verify(slot)?;
        if !verified {
            return Err(KernelOtaError::Ota(OtaError::VerifyFailed));
        }

        // 6. 激活
        self.ab.activate(slot)?;

        // 7. 持久化分区表
        save_to_device(&mut self.ab)?;

        Ok(())
    }

    /// 内核热迁移（零停机模式）。
    ///
    /// 流程：
    /// 1. 验证新内核镜像
    /// 2. 写入备用分区并校验
    /// 3. 迁移线程状态
    /// 4. 迁移内存映射
    /// 5. 迁移设备状态
    /// 6. 切换执行流到新内核
    /// 7. 标记新分区为活动
    ///
    /// # 注意
    /// 热迁移复杂度极高，蓝图建议先用重启切换。本实现提供骨架接口，
    /// 实际迁移逻辑需平台适配层（PAL）支持。
    pub fn hot_migrate(
        &mut self,
        image: &KernelImage,
        chunk_size: u32,
        migrator: &mut dyn HotMigrate,
    ) -> Result<(), KernelOtaError> {
        // 1. 检查镜像大小
        let image_size = image.data.len() as u64;
        if image_size > self.partition_capacity {
            return Err(KernelOtaError::ImageTooLarge {
                image_size,
                partition_capacity: self.partition_capacity,
            });
        }

        // 2. 写入备用分区
        let total_chunks = (image_size as u32).div_ceil(chunk_size);
        let meta = UpgradeMeta {
            target_version: image.version.clone(),
            image_size,
            image_hash: image.hash,
            image_sig: image.signature,
            download_url: HString::<128>::try_from("local://hot-migrate").unwrap_or_default(),
            chunk_size,
            total_chunks,
            downloaded_chunks: 0,
        };
        let slot = self.ab.prepare_upgrade(meta)?;

        let mut offset: u64 = 0;
        let mut remaining = &image.data[..];
        while !remaining.is_empty() {
            let n = core::cmp::min(chunk_size as usize, remaining.len());
            self.ab.write_chunk(slot, offset, &remaining[..n])?;
            offset += n as u64;
            remaining = &remaining[n..];
        }

        // 3. 校验
        let verified = self.ab.verify(slot)?;
        if !verified {
            return Err(KernelOtaError::Ota(OtaError::VerifyFailed));
        }

        // 4. 执行热迁移
        migrator.migrate(image)?;
        migrator.migrate_threads()?;
        migrator.migrate_memory()?;
        migrator.migrate_devices()?;

        // 5. 切换执行流
        migrator.switch_to_new_kernel()?;

        // 6. 激活新分区
        self.ab.activate(slot)?;
        save_to_device(&mut self.ab)?;

        Ok(())
    }

    /// 启动成功回调（内核启动后调用）。
    ///
    /// 标记当前分区为 LastGood，清零重试计数。
    pub fn boot_success(&mut self, now_ms: u64) -> Result<(), KernelOtaError> {
        let ok = boot_health_check(&mut self.ab, now_ms);
        if !ok {
            // 自动回滚已触发
            save_to_device(&mut self.ab)?;
            return Err(KernelOtaError::Ota(OtaError::BootFailed));
        }
        save_to_device(&mut self.ab)?;
        Ok(())
    }

    /// 获取当前活动分区。
    pub fn get_active(&self) -> PartitionSlot {
        self.ab.get_active_slot()
    }

    /// 获取备用分区。
    pub fn get_inactive(&self) -> PartitionSlot {
        match self.ab.get_active_slot() {
            PartitionSlot::A => PartitionSlot::B,
            PartitionSlot::B => PartitionSlot::A,
        }
    }

    /// 获取分区状态。
    pub fn get_partition_state(&self, slot: PartitionSlot) -> PartitionState {
        let idx = AbPartitionOta::<D>::slot_index(slot);
        self.ab.get_partitions()[idx].state
    }

    /// 手动回滚到旧版本分区。
    pub fn rollback(&mut self) -> Result<PartitionSlot, KernelOtaError> {
        let slot = self.ab.rollback()?;
        save_to_device(&mut self.ab)?;
        Ok(slot)
    }
}

/// 内核热迁移 trait（蓝图 §4 接口定义）。
///
/// 实现者需提供平台相关的状态迁移逻辑。
pub trait HotMigrate {
    /// 迁移内核状态到新内核。
    fn migrate(&mut self, target: &KernelImage) -> Result<(), MigrateError>;
    /// 迁移线程状态。
    fn migrate_threads(&mut self) -> Result<(), MigrateError>;
    /// 迁移内存映射。
    fn migrate_memory(&mut self) -> Result<(), MigrateError>;
    /// 迁移设备状态。
    fn migrate_devices(&mut self) -> Result<(), MigrateError>;
    /// 切换执行流到新内核。
    fn switch_to_new_kernel(&mut self) -> Result<(), MigrateError>;
}

// ============================================================================
// 单元测试
// ============================================================================

#[cfg(test)]
mod tests {
    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
    use eneros_storage::MockBlockDevice;

    use super::*;
    use crate::ab_partition::{PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};

    /// 设备总块数：保留块 + 双分区。
    const TEST_BLOCKS: u64 = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS;
    /// 测试分区容量（字节）
    const TEST_CAPACITY: u64 = PARTITION_BLOCKS * 512;

    fn setup() -> (KernelOta<MockBlockDevice>, Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
        let dev = MockBlockDevice::new(TEST_BLOCKS, 512);
        let ota = KernelOta::new(dev, kp.public_key, 1_000, TEST_CAPACITY);
        (ota, kp, rng)
    }

    fn make_kernel_image(
        kp: &Sm2KeyPair,
        rng: &mut CsRng,
        size: usize,
        version: &str,
    ) -> KernelImage {
        let data: alloc::vec::Vec<u8> = (0..size).map(|i| (i % 199) as u8).collect();
        let hash = sm3_hash(&data);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng).expect("签名失败");
        KernelImage {
            version: HString::<32>::try_from(version).unwrap_or_default(),
            data,
            hash,
            signature: sig.to_bytes(),
        }
    }

    /// KOT1：完整内核 OTA 流程（重启切换模式）。
    #[test]
    fn kot1_full_kernel_ota_update() {
        let (mut ota, kp, mut rng) = setup();
        let image = make_kernel_image(&kp, &mut rng, 2048, "0.163.0");

        assert_eq!(ota.get_active(), PartitionSlot::A);
        ota.update(&image, 512).expect("OTA 应成功");

        // 新内核应激活在 B 分区
        assert_eq!(ota.get_active(), PartitionSlot::B);
        assert_eq!(
            ota.get_partition_state(PartitionSlot::B),
            PartitionState::Active
        );
        // 旧分区保留 Verified 作回滚备用
        assert_eq!(
            ota.get_partition_state(PartitionSlot::A),
            PartitionState::Verified
        );
    }

    /// KOT2：镜像过大拒绝。
    #[test]
    fn kot2_image_too_large_rejected() {
        let (mut ota, kp, mut rng) = setup();
        // 创建超大镜像
        let image = make_kernel_image(&kp, &mut rng, (TEST_CAPACITY + 1) as usize, "0.163.0");
        let err = ota.update(&image, 512).expect_err("应拒绝过大镜像");
        assert!(matches!(err, KernelOtaError::ImageTooLarge { .. }));
        // 活动分区不变
        assert_eq!(ota.get_active(), PartitionSlot::A);
    }

    /// KOT3：签名无效拒绝。
    #[test]
    fn kot3_invalid_signature_rejected() {
        let (mut ota, kp, mut rng) = setup();
        let mut image = make_kernel_image(&kp, &mut rng, 1024, "0.163.0");
        // 篡改签名
        image.signature[0] ^= 0xFF;
        let err = ota.update(&image, 512).expect_err("签名无效应拒绝");
        assert!(matches!(
            err,
            KernelOtaError::Ota(OtaError::SignatureInvalid)
        ));
    }

    /// KOT4：回滚功能。
    #[test]
    fn kot4_rollback_after_update() {
        let (mut ota, kp, mut rng) = setup();
        let image = make_kernel_image(&kp, &mut rng, 1024, "0.163.0");
        ota.update(&image, 512).expect("OTA 应成功");
        assert_eq!(ota.get_active(), PartitionSlot::B);

        // 回滚
        let slot = ota.rollback().expect("回滚应成功");
        assert_eq!(slot, PartitionSlot::A);
        assert_eq!(ota.get_active(), PartitionSlot::A);
    }

    /// KOT5：启动成功回调。
    #[test]
    fn kot5_boot_success_callback() {
        let (mut ota, kp, mut rng) = setup();
        let image = make_kernel_image(&kp, &mut rng, 1024, "0.163.0");
        ota.update(&image, 512).expect("OTA 应成功");

        // 启动成功
        ota.boot_success(2_000).expect("启动成功回调应成功");
        assert_eq!(ota.get_active(), PartitionSlot::B);
    }

    /// 模拟热迁移器（测试用）。
    struct MockMigrator {
        migrate_called: bool,
        threads_called: bool,
        memory_called: bool,
        devices_called: bool,
        switch_called: bool,
    }

    impl MockMigrator {
        fn new() -> Self {
            Self {
                migrate_called: false,
                threads_called: false,
                memory_called: false,
                devices_called: false,
                switch_called: false,
            }
        }
    }

    impl HotMigrate for MockMigrator {
        fn migrate(&mut self, _target: &KernelImage) -> Result<(), MigrateError> {
            self.migrate_called = true;
            Ok(())
        }
        fn migrate_threads(&mut self) -> Result<(), MigrateError> {
            self.threads_called = true;
            Ok(())
        }
        fn migrate_memory(&mut self) -> Result<(), MigrateError> {
            self.memory_called = true;
            Ok(())
        }
        fn migrate_devices(&mut self) -> Result<(), MigrateError> {
            self.devices_called = true;
            Ok(())
        }
        fn switch_to_new_kernel(&mut self) -> Result<(), MigrateError> {
            self.switch_called = true;
            Ok(())
        }
    }

    /// KOT6：热迁移完整流程。
    #[test]
    fn kot6_hot_migrate_success() {
        let (mut ota, kp, mut rng) = setup();
        let image = make_kernel_image(&kp, &mut rng, 1024, "0.163.0");
        let mut migrator = MockMigrator::new();

        ota.hot_migrate(&image, 512, &mut migrator)
            .expect("热迁移应成功");

        assert!(migrator.migrate_called);
        assert!(migrator.threads_called);
        assert!(migrator.memory_called);
        assert!(migrator.devices_called);
        assert!(migrator.switch_called);
        assert_eq!(ota.get_active(), PartitionSlot::B);
    }

    /// KOT7：热迁移失败回退。
    #[test]
    fn kot7_hot_migrate_failure() {
        struct FailMigrator;
        impl HotMigrate for FailMigrator {
            fn migrate(&mut self, _target: &KernelImage) -> Result<(), MigrateError> {
                Ok(())
            }
            fn migrate_threads(&mut self) -> Result<(), MigrateError> {
                Err(MigrateError::ThreadMigrationFailed)
            }
            fn migrate_memory(&mut self) -> Result<(), MigrateError> {
                Ok(())
            }
            fn migrate_devices(&mut self) -> Result<(), MigrateError> {
                Ok(())
            }
            fn switch_to_new_kernel(&mut self) -> Result<(), MigrateError> {
                Ok(())
            }
        }

        let (mut ota, kp, mut rng) = setup();
        let image = make_kernel_image(&kp, &mut rng, 1024, "0.163.0");
        let mut migrator = FailMigrator;

        let err = ota
            .hot_migrate(&image, 512, &mut migrator)
            .expect_err("热迁移应失败");
        assert!(matches!(
            err,
            KernelOtaError::MigrateFailed(MigrateError::ThreadMigrationFailed)
        ));
        // 活动分区不变
        assert_eq!(ota.get_active(), PartitionSlot::A);
    }
}
