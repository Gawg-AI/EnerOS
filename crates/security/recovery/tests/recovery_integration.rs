//! v0.125.0 Recovery Mode 集成测试（host std 环境）。
//!
//! - INT1（★ 损坏→恢复端到端）：OTA 侧构造 B 分区 Failed（篡改签名 verify 拒绝）
//!   并 save_to_device 持久化 → 恢复会话经 load_from_device 读回分区表注入诊断器
//!   → execute(Diagnose) 检出 B 非 ok → 集成方 receive_chunk 流式喂入完整固件 +
//!   execute(Flash) 验签写盘 state=Complete → 设备回读与镜像逐字节一致；
//! - INT2（★ v0.123.0 兼容回归）：AbPartitionOta 真实构造 A Active/B Failed
//!   （boot_count 非零）→ save_to_device → 新实例 load_from_device 读回 →
//!   Diagnostician 查询结果与分区表逐字段一致，ListPartitions 文本含 A/B
//!   状态与版本；
//! - INT3（★ 出口判定复演：系统损坏后可恢复）——完整"诊断→刷写→恢复出厂→重启"
//!   流程的返回文本与状态机迁移全部正确；
//! - INT4（故障注入）：① 非根私钥签名镜像 → SignatureInvalid 且设备零写入；
//!   ② 写盘失败注入 → FlashFailed{offset} 且 progress.error 记录；
//! - INT5（出厂恢复）：set 用户配置 → backup → execute(FactoryReset) → 配置等于
//!   出厂集 → restore 备份字节流 → 用户配置还原；
//! - PERF6：release 打印 10MB 伪固件"接收+验签+写盘"总耗时（≥3 次取均值）；
//!   `ENEROS_PERF_GATE=1`（`var(...).as_deref() == Ok("1")`，与 v0.113.0~
//!   v0.123.0 PERF 先例一致）硬断言均值 < 60s，未设置仅打印不断言。
//!
//! 集成测试运行于 host std 环境（`std::time::Instant`/`std::env`/`Rc` 可用），
//! 通过 `eneros_recovery` crate 公共 API 调用被测的 no_std 代码。

// 测试代码允许 panic!/unwrap（与 src 内嵌测试模块同口径）
#![allow(clippy::disallowed_macros)]

use std::cell::RefCell;
use std::rc::Rc;

use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
use eneros_ota_ab::ab_partition::{PARTITION_BLOCKS, SLOT_A_BASE_BLOCK, SLOT_B_BASE_BLOCK};
use eneros_ota_ab::{
    boot_health_check, load_from_device, save_to_device, AbPartitionOta, OtaManager, PartitionSlot,
    PartitionState, PartitionTable, UpgradeMeta,
};
use eneros_recovery::{
    RecoveryCommand, RecoveryError, RecoveryMode, RecoveryModeImpl, RecoveryState,
};
use eneros_storage::{BlockDevice, DeviceHealth, MockBlockDevice, StorageError};
use heapless::String as HString;

/// 33 SE-18（签名变更涟漪）：测试用分区表 HMAC 密钥。
const TEST_HMAC_KEY: [u8; 32] = [0x42; 32];

/// 10 MiB 镜像大小（蓝图 §6.3/§7.2 性能口径 10MB）。
const IMAGE_SIZE_10MB: usize = 10 * 1024 * 1024;
/// 恢复刷写流式接收分块大小（4 KiB，对应 configs/recovery.toml [flash] chunk_size）。
const FLASH_CHUNK: usize = 4096;
/// 大镜像用块大小：8 KiB 时单分区容量 = PARTITION_BLOCKS × 8 KiB = 16 MiB ≥ 10 MiB。
const LARGE_BLOCK_SIZE: usize = 8 * 1024;
/// 设备总块数：保留块 + 双分区。
const TOTAL_BLOCKS: u64 = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS;

/// 共享句柄块设备：OTA 管理器与 RecoveryModeImpl 接力持有同一底层存储
/// （host 单线程测试场景，`Rc<RefCell>` 语义足够；crate 内实现零改动）。
struct SharedDevice(Rc<RefCell<MockBlockDevice>>);

impl SharedDevice {
    fn new(block_count: u64, block_size: usize) -> Self {
        Self(Rc::new(RefCell::new(MockBlockDevice::new(
            block_count,
            block_size,
        ))))
    }

    /// 复制指向同一底层存储的新句柄。
    fn handle(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl BlockDevice for SharedDevice {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        self.0.borrow().read_block(block_idx, buf)
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        self.0.borrow_mut().write_block(block_idx, buf)
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        self.0.borrow_mut().erase_block(block_idx)
    }

    fn block_count(&self) -> u64 {
        self.0.borrow().block_count()
    }

    fn block_size(&self) -> usize {
        self.0.borrow().block_size()
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.0.borrow_mut().flush()
    }

    fn health_status(&self) -> DeviceHealth {
        self.0.borrow().health_status()
    }
}

/// 写失败注入 mock：写第 `fail_at` 块时返回 BadBlock，其余委托内部
/// MockBlockDevice（mock 自身的 mark_bad 会重定向写而非失败，故自包一层）。
struct FailAtBlockMock {
    /// 内部 RAM 后端
    inner: MockBlockDevice,
    /// 注入写失败的块地址
    fail_at: u64,
}

impl BlockDevice for FailAtBlockMock {
    fn read_block(&self, block_idx: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        self.inner.read_block(block_idx, buf)
    }

    fn write_block(&mut self, block_idx: u64, buf: &[u8]) -> Result<(), StorageError> {
        if block_idx == self.fail_at {
            return Err(StorageError::BadBlock { block_idx });
        }
        self.inner.write_block(block_idx, buf)
    }

    fn erase_block(&mut self, block_idx: u64) -> Result<(), StorageError> {
        self.inner.erase_block(block_idx)
    }

    fn block_count(&self) -> u64 {
        self.inner.block_count()
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.inner.flush()
    }

    fn health_status(&self) -> DeviceHealth {
        self.inner.health_status()
    }
}

/// 确定性伪随机填充（xorshift32，固定种子 → 可复现；模拟真实固件镜像字节流，
/// 避免规律性填充掩盖跨块写盘路径的对齐问题）。
fn pseudo_random_image(seed: u32, len: usize) -> Vec<u8> {
    let mut state = seed.max(1);
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 16) as u8
        })
        .collect()
}

/// 测试用 hex 编码（小写，无分隔）。
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 用根私钥对镜像 SM3 哈希签名，返回 (hash_hex, sig_hex) 刷写命令参数。
fn sign_image(image: &[u8], kp: &Sm2KeyPair, rng: &mut CsRng) -> (String, String) {
    let hash = sm3_hash(image);
    let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng)
        .expect("签名失败")
        .to_bytes();
    (hex_encode(&hash), hex_encode(&sig))
}

/// 构造 OTA 升级元数据（真实 SM3 哈希 + 根私钥 SM2 签名）。
fn make_meta(image: &[u8], kp: &Sm2KeyPair, rng: &mut CsRng, version: &str) -> UpgradeMeta {
    let hash = sm3_hash(image);
    let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng).expect("签名失败");
    UpgradeMeta {
        target_version: HString::<32>::try_from(version).expect("版本号超长"),
        image_size: image.len() as u64,
        image_hash: hash,
        image_sig: sig.to_bytes(),
        download_url: HString::<128>::try_from("https://ota.eneros.local/fw.bin")
            .expect("URL 超长"),
        chunk_size: 16 * 1024,
        total_chunks: (image.len() as u32).div_ceil(16 * 1024),
        downloaded_chunks: 0,
    }
}

/// 按 16KiB 分块经 OTA 写入完整镜像（与 make_meta chunk_size 对齐）。
fn ota_write_image<D: BlockDevice>(ota: &mut AbPartitionOta<D>, slot: PartitionSlot, image: &[u8]) {
    let mut offset = 0u64;
    for c in image.chunks(16 * 1024) {
        ota.write_chunk(slot, offset, c)
            .expect("write_chunk 应成功");
        offset += c.len() as u64;
    }
}

/// 取指定槽位的分区快照（get_partitions 数组口径：索引 0=A，1=B）。
fn part<D: BlockDevice>(ota: &AbPartitionOta<D>, slot: PartitionSlot) -> eneros_ota_ab::Partition {
    let parts = ota.get_partitions();
    match slot {
        PartitionSlot::A => parts[0].clone(),
        PartitionSlot::B => parts[1].clone(),
    }
}

/// 集成方恢复会话装配：新 OTA 实例从保留块 load_from_device 恢复分区表，
/// 快照注入 RecoveryModeImpl 诊断器（D5 真实查询路径）。
fn attach_partition_table<D: BlockDevice, P: eneros_recovery::HealthProbe>(
    rec: &mut RecoveryModeImpl<D, P>,
    ota: &mut AbPartitionOta<D>,
    now_ms: u64,
) -> PartitionTable {
    let loaded = load_from_device(ota, &TEST_HMAC_KEY, now_ms).expect("load 不应 I/O 失败");
    assert!(loaded, "应从保留块恢复分区表");
    let table = PartitionTable::snapshot(ota);
    rec.diagnostician_mut().set_partition_table(table.clone());
    table
}

/// INT1：★ 损坏→恢复端到端——B 分区损坏（OTA 验签拒绝置 Failed）持久化后，
/// 恢复模式检出损坏并重刷固件，设备回读与镜像逐字节一致。
#[test]
fn int1_corrupted_partition_recovery_end_to_end() {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let storage = SharedDevice::new(TOTAL_BLOCKS, 512);

    // ── 主系统侧：OTA 升级被篡改镜像 → verify 拒绝 → B Failed，分区表落盘 ──
    let mut ota = AbPartitionOta::new(storage.handle(), kp.public_key, 1_000);
    let bad_image = pseudo_random_image(0xBAD0_0001, 64 * 1024);
    let mut meta = make_meta(&bad_image, &kp, &mut rng, "0.125.0");
    meta.image_sig[7] ^= 0xFF; // 篡改签名
    let slot = ota.prepare_upgrade(meta).expect("prepare 应成功");
    assert_eq!(slot, PartitionSlot::B);
    ota_write_image(&mut ota, slot, &bad_image);
    assert!(
        ota.verify(slot).is_err(),
        "篡改镜像 verify 应拒绝并置 B Failed"
    );
    assert_eq!(part(&ota, PartitionSlot::B).state, PartitionState::Failed);
    save_to_device(&mut ota, &TEST_HMAC_KEY).expect("save 应成功");
    drop(ota); // 主系统"损坏无法启动"，丢弃内存态

    // ── 恢复模式侧：enter → 恢复分区表注入诊断器 → Diagnose 检出 B 非 ok ──
    let mut rec = RecoveryModeImpl::new(storage.handle(), kp.public_key);
    rec.enter().expect("enter 应成功");
    let mut ota2 = AbPartitionOta::new(storage.handle(), kp.public_key, 2_000);
    let table = attach_partition_table(&mut rec, &mut ota2, 3_000);
    assert_eq!(table.partitions[1].state, PartitionState::Failed);
    drop(ota2);

    let diag_text = rec
        .execute(RecoveryCommand::Diagnose, &[])
        .expect("诊断应成功");
    assert!(
        diag_text.contains("partition B: not ok"),
        "诊断应检出 B 损坏: {diag_text}"
    );
    let diag = rec.get_diag().expect("get_diag 应成功");
    assert!(diag.partition_a_ok, "A Active 应健康");
    assert!(!diag.partition_b_ok, "B Failed 应检出非 ok");

    // ── 重刷固件：集成方流式喂入 → execute(Flash) 验签写盘 → Complete ──
    let firmware = pseudo_random_image(0x600D_F00D, 96 * 1024);
    for chunk in firmware.chunks(FLASH_CHUNK) {
        rec.flash_writer_mut().receive_chunk(chunk);
    }
    let (hash_hex, sig_hex) = sign_image(&firmware, &kp, &mut rng);
    let slot_base = SLOT_B_BASE_BLOCK.to_string();
    let out = rec
        .execute(
            RecoveryCommand::Flash,
            &[hash_hex.as_str(), sig_hex.as_str(), slot_base.as_str()],
        )
        .expect("刷写应成功");
    assert!(out.contains("Flash complete"));
    assert_eq!(rec.state(), RecoveryState::Complete);
    let progress = rec.get_progress();
    assert_eq!(progress.total_bytes, firmware.len() as u64);
    assert_eq!(progress.flashed_bytes, firmware.len() as u64);
    assert_eq!(progress.state, RecoveryState::Complete);
    assert!(progress.error.is_none());

    // ── 设备回读：槽位 B 内容与镜像逐字节一致 ──
    let bs = storage.block_size();
    let n_blocks = firmware.len().div_ceil(bs) as u64;
    let mut readback = Vec::new();
    let mut blk = vec![0u8; bs];
    for i in 0..n_blocks {
        storage
            .read_block(SLOT_B_BASE_BLOCK + i, &mut blk)
            .expect("读块失败");
        readback.extend_from_slice(&blk);
    }
    assert_eq!(
        &readback[..firmware.len()],
        &firmware[..],
        "重刷后槽位 B 应与固件镜像逐字节一致"
    );
}

/// INT2：★ v0.123.0 兼容回归——真实分区表（A Active/B Failed，boot_count 非零）
/// 持久化读回后，诊断查询逐字段一致，ListPartitions 文本含真实状态与版本。
#[test]
fn int2_ota_ab_compat_partition_table_regression() {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let storage = SharedDevice::new(TOTAL_BLOCKS, 512);

    // ── 构造真实分区表：A(0.0.0)→B(0.123.0) 升级后连续启动失败自动回滚 ──
    let mut ota = AbPartitionOta::new(storage.handle(), kp.public_key, 1_000);
    let image = pseudo_random_image(0x1234_5678, 64 * 1024);
    let meta = make_meta(&image, &kp, &mut rng, "0.123.0");
    let slot = ota.prepare_upgrade(meta).expect("prepare");
    ota_write_image(&mut ota, slot, &image);
    assert!(ota.verify(slot).expect("verify"));
    ota.activate(slot).expect("activate");
    for i in 1..=3u32 {
        assert!(boot_health_check(&mut ota, 2_000 + u64::from(i)));
    }
    assert!(
        !boot_health_check(&mut ota, 2_004),
        "连超 3 次应自动回滚：B Failed / A Active"
    );
    assert_eq!(part(&ota, PartitionSlot::B).state, PartitionState::Failed);
    assert_eq!(
        part(&ota, PartitionSlot::B).boot_count,
        4,
        "boot_count 非零"
    );
    save_to_device(&mut ota, &TEST_HMAC_KEY).expect("save 应成功");
    drop(ota);

    // ── 恢复会话：load_from_device 读回 → Diagnostician 逐字段一致 ──
    let mut rec = RecoveryModeImpl::new(storage.handle(), kp.public_key);
    rec.enter().expect("enter 应成功");
    let mut ota2 = AbPartitionOta::new(storage.handle(), kp.public_key, 3_000);
    let table = attach_partition_table(&mut rec, &mut ota2, 4_000);

    let diag = rec.get_diag().expect("get_diag 应成功");
    let pa = &table.partitions[0];
    let pb = &table.partitions[1];
    assert_eq!(pa.state, PartitionState::Active);
    assert_eq!(pb.state, PartitionState::Failed);
    assert!(diag.partition_a_ok, "A Active 应为 ok");
    assert!(!diag.partition_b_ok, "B Failed 应为非 ok");
    assert_eq!(
        diag.boot_fail_count,
        pa.boot_count + pb.boot_count,
        "boot_fail_count 应为双分区 boot_count 之和"
    );
    assert!(diag.last_error.as_str().contains("B"));
    assert!(diag.last_error.as_str().contains("Failed"));
    assert!(
        diag.last_error.as_str().contains(pb.version.as_str()),
        "错误描述应含 B 分区版本"
    );

    // ListPartitions 文本含 A/B 状态与版本（D5 真实查询，非硬编码）
    let list = rec
        .execute(RecoveryCommand::ListPartitions, &[])
        .expect("list 应成功");
    assert!(
        list.contains(&format!(
            "A: {:?} version={}",
            pa.state,
            pa.version.as_str()
        )),
        "应含 A 真实状态与版本: {list}"
    );
    assert!(
        list.contains(&format!(
            "B: {:?} version={}",
            pb.state,
            pb.version.as_str()
        )),
        "应含 B 真实状态与版本: {list}"
    );
    assert!(list.contains(&format!("boot_count={}", pb.boot_count)));
}

/// INT3：★ 出口判定复演——系统损坏后可恢复：完整"诊断→刷写→恢复出厂→重启"
/// 流程的返回文本与状态机迁移正确。
#[test]
fn int3_exit_criteria_full_recovery_flow() {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let storage = SharedDevice::new(TOTAL_BLOCKS, 512);

    // 预置损坏现场：B Failed 分区表落盘
    let mut ota = AbPartitionOta::new(storage.handle(), kp.public_key, 1_000);
    let bad = pseudo_random_image(0xDEAD_BEEF, 32 * 1024);
    let mut meta = make_meta(&bad, &kp, &mut rng, "0.125.0");
    meta.image_sig[0] ^= 0xFF;
    let slot = ota.prepare_upgrade(meta).expect("prepare");
    ota_write_image(&mut ota, slot, &bad);
    assert!(ota.verify(slot).is_err());
    save_to_device(&mut ota, &TEST_HMAC_KEY).expect("save");
    drop(ota);

    let mut rec = RecoveryModeImpl::new(storage.handle(), kp.public_key);
    rec.enter().expect("enter 应成功");
    assert_eq!(rec.state(), RecoveryState::Waiting);

    // ① 诊断：检出损坏
    let mut ota2 = AbPartitionOta::new(storage.handle(), kp.public_key, 2_000);
    attach_partition_table(&mut rec, &mut ota2, 3_000);
    drop(ota2);
    let diag_text = rec
        .execute(RecoveryCommand::Diagnose, &[])
        .expect("诊断应成功");
    assert!(diag_text.contains("partition B: not ok"));
    assert_eq!(rec.state(), RecoveryState::Waiting, "诊断后回到 Waiting");

    // ② 刷写：重刷固件成功
    let firmware = pseudo_random_image(0xCAFE_BABE, 48 * 1024);
    for chunk in firmware.chunks(FLASH_CHUNK) {
        rec.flash_writer_mut().receive_chunk(chunk);
    }
    let (hash_hex, sig_hex) = sign_image(&firmware, &kp, &mut rng);
    let slot_base = SLOT_B_BASE_BLOCK.to_string();
    let flash_out = rec
        .execute(
            RecoveryCommand::Flash,
            &[hash_hex.as_str(), sig_hex.as_str(), slot_base.as_str()],
        )
        .expect("刷写应成功");
    assert!(flash_out.contains("Flash complete. Reboot to activate."));
    assert_eq!(rec.state(), RecoveryState::Complete);

    // ③ 恢复出厂：用户配置清除并写入默认集
    rec.factory_reset_mut().set("site_id", "A01");
    let reset_out = rec
        .execute(RecoveryCommand::FactoryReset, &[])
        .expect("恢复出厂应成功");
    assert!(reset_out.contains("Factory reset complete."));
    assert!(rec.factory_reset_mut().get("site_id").is_none());
    assert_eq!(
        rec.factory_reset_mut()
            .get("device_name")
            .map(|v| v.as_str()),
        Some("eneros-edge")
    );
    assert_eq!(rec.state(), RecoveryState::Complete);

    // ④ 重启：提示文本 + exit 收尾
    let reboot_out = rec
        .execute(RecoveryCommand::Reboot, &[])
        .expect("reboot 应成功");
    assert!(reboot_out.contains("Rebooting..."));
    rec.exit().expect("exit 应成功");
    assert_eq!(rec.state(), RecoveryState::Complete);
}

/// INT4：故障注入——① 非根私钥签名镜像 → SignatureInvalid 且设备零写入；
/// ② 写盘失败注入 → FlashFailed{offset} 且 progress.error 记录。
#[test]
fn int4_fault_injection_foreign_sig_and_write_failure() {
    // ① 非根私钥签名：验签拒绝，设备零写入
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let dev = MockBlockDevice::new(64, 512);
    let mut rec = RecoveryModeImpl::new(dev, kp.public_key);
    rec.enter().expect("enter 应成功");
    let image = pseudo_random_image(0xF0E1_D2C3, 2048);
    rec.flash_writer_mut().receive_chunk(&image);
    // 攻击者密钥对签名（非设备根私钥）
    let attacker = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let (hash_hex, sig_hex) = sign_image(&image, &attacker, &mut rng);
    assert_eq!(
        rec.execute(
            RecoveryCommand::Flash,
            &[hash_hex.as_str(), sig_hex.as_str(), "10"]
        ),
        Err(RecoveryError::SignatureInvalid)
    );
    assert_eq!(rec.state(), RecoveryState::Error);
    let err = rec
        .get_progress()
        .error
        .as_ref()
        .expect("错误应被记录")
        .clone();
    assert!(err.as_str().contains("verify"));
    let dev = rec.flash_writer_mut().device();
    let mut blk = [0u8; 512];
    for i in 0..dev.block_count() {
        dev.read_block(i, &mut blk).expect("读块失败");
        assert!(blk.iter().all(|&b| b == 0), "块 {i} 不应被写入");
    }

    // ② 写盘失败注入：写 slot_base(10)+2 块时失败 → FlashFailed{offset=1024}
    let kp2 = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let fail_dev = FailAtBlockMock {
        inner: MockBlockDevice::new(64, 512),
        fail_at: 12,
    };
    let mut rec2 = RecoveryModeImpl::new(fail_dev, kp2.public_key);
    rec2.enter().expect("enter 应成功");
    let image2 = pseudo_random_image(0xA5A5_5A5A, 1500); // 3 块：512+512+476
    rec2.flash_writer_mut().receive_chunk(&image2);
    let (hash_hex2, sig_hex2) = sign_image(&image2, &kp2, &mut rng);
    assert_eq!(
        rec2.execute(
            RecoveryCommand::Flash,
            &[hash_hex2.as_str(), sig_hex2.as_str(), "10"]
        ),
        Err(RecoveryError::FlashFailed { offset: 1024 })
    );
    assert_eq!(rec2.state(), RecoveryState::Error);
    let p2 = rec2.get_progress();
    assert_eq!(p2.state, RecoveryState::Error);
    let err2 = p2.error.as_ref().expect("写失败应被记录");
    assert!(
        err2.as_str().contains("write"),
        "错误应标记写盘阶段: {err2}"
    );
    assert!(
        err2.as_str().contains("FlashFailed"),
        "错误应含 FlashFailed: {err2}"
    );
}

/// INT5：出厂恢复——set 用户配置 → backup → FactoryReset → 配置等于出厂集 →
/// restore 备份字节流 → 用户配置还原。
#[test]
fn int5_factory_reset_backup_restore_flow() {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let dev = MockBlockDevice::new(64, 512);
    let mut rec = RecoveryModeImpl::new(dev, kp.public_key);
    rec.enter().expect("enter 应成功");

    // 用户配置
    rec.factory_reset_mut().set("site_id", "A01");
    rec.factory_reset_mut().set("owner", "ops");
    rec.factory_reset_mut().set("log_level", "debug");

    // 备份
    let out = rec
        .execute(RecoveryCommand::Backup, &[])
        .expect("备份应成功");
    assert!(out.contains("Backup complete."));

    // 恢复出厂：配置等于出厂集（四项默认，用户键全部清除）
    rec.execute(RecoveryCommand::FactoryReset, &[])
        .expect("恢复出厂应成功");
    let fr = rec.factory_reset_mut();
    assert_eq!(fr.len(), 4, "出厂集应为 4 项默认配置");
    for (key, value) in eneros_recovery::FactoryReset::factory_defaults() {
        assert_eq!(
            fr.get(key).map(|v| v.as_str()),
            Some(value),
            "出厂默认项 {key} 应等于 {value}"
        );
    }
    assert!(fr.get("site_id").is_none());
    assert!(fr.get("owner").is_none());
    // 用户改过的 log_level 已被默认值覆盖
    assert_eq!(fr.get("log_level").map(|v| v.as_str()), Some("info"));

    // 恢复备份：用户配置还原
    let out = rec
        .execute(RecoveryCommand::Restore, &[])
        .expect("恢复应成功");
    assert!(out.contains("Restore complete."));
    let fr = rec.factory_reset_mut();
    assert_eq!(fr.get("site_id").map(|v| v.as_str()), Some("A01"));
    assert_eq!(fr.get("owner").map(|v| v.as_str()), Some("ops"));
    assert_eq!(fr.get("log_level").map(|v| v.as_str()), Some("debug"));
    assert_eq!(fr.len(), 3, "恢复后应为备份时的 3 项用户配置");
}

/// PERF6：蓝图 §6.3/§7.2 性能基准——10MB 伪固件"接收+验签+写盘"总耗时（≥3 次取均值）。
///
/// 计时口径：从首个 receive_chunk 起至 execute(Flash) 返回止（镜像生成/签名
/// 为前置准备不计时）；每次迭代前 reset 刷写器 + enter 复位进度，三次取均值。
/// 默认仅打印不断言（含 debug/release 口径说明）；仅当
/// `ENEROS_PERF_GATE=1`（`var(...).as_deref() == Ok("1")`，与 v0.113.0~
/// v0.123.0 PERF 先例一致）时启用均值 < 60s 硬断言门禁，
/// 面向性能 CI / 目标硬件场景。
#[test]
fn perf6_flash_10mb_receive_verify_write() {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let dev = MockBlockDevice::new(TOTAL_BLOCKS, LARGE_BLOCK_SIZE);
    let mut rec = RecoveryModeImpl::new(dev, kp.public_key);
    // 前置准备（不计时）：确定性伪随机 10MB 镜像 + SM3 哈希 + SM2 签名
    let image = pseudo_random_image(0x1234_5678, IMAGE_SIZE_10MB);
    let (hash_hex, sig_hex) = sign_image(&image, &kp, &mut rng);
    let slot_base = SLOT_B_BASE_BLOCK.to_string();

    let mut total = std::time::Duration::ZERO;
    const ROUNDS: u32 = 3;
    for round in 1..=ROUNDS {
        // 会话与刷写器复位：进度/缓冲/哈希状态不串扰
        rec.enter().expect("enter 应成功");
        rec.flash_writer_mut().reset();

        // 计时：receive_chunk ×2560（4KiB）+ execute(Flash)（SM3 比对 + SM2 验签 + 1280 块写盘）
        let start = std::time::Instant::now();
        for chunk in image.chunks(FLASH_CHUNK) {
            rec.flash_writer_mut()
                .receive_chunk(std::hint::black_box(chunk));
        }
        let out = rec
            .execute(
                RecoveryCommand::Flash,
                &[hash_hex.as_str(), sig_hex.as_str(), slot_base.as_str()],
            )
            .expect("刷写应成功");
        let elapsed = start.elapsed();
        total += elapsed;
        assert!(out.contains("Flash complete"));
        assert_eq!(rec.get_progress().flashed_bytes, IMAGE_SIZE_10MB as u64);
        println!("[PERF6] 第 {round}/{ROUNDS} 轮 10MB 接收+验签+写盘耗时: {elapsed:?}");
    }
    let avg = total / ROUNDS;

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "[PERF6] 10MB 镜像接收(2560×4KiB)+验签+写盘(1280×8KiB) 平均耗时: {avg:?}（{ROUNDS} 轮均值，{profile} 构建；设 ENEROS_PERF_GATE=1 启用 <60s 断言门禁）"
    );

    // 性能门禁：仅 ENEROS_PERF_GATE=1 时启用断言（蓝图 §6.3/§7.2）
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            avg.as_secs() < 60,
            "PERF6 平均耗时 {avg:?} 超过 60s 上限（蓝图 §7.2）"
        );
    }
}
