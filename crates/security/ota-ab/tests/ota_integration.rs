//! v0.123.0 A/B 分区 OTA 升级 集成测试（host std 环境）。
//!
//! - INT1（★ 完整升级流程）：共享句柄 MockBlockDevice + 真实 SM2 密钥对签名
//!   10MB 确定性伪随机镜像 → prepare_upgrade → 按 64KiB 分块 write_chunk →
//!   verify（SM3 回读比对 + SM2 根公钥验签）→ activate → PartitionTable
//!   save_to_device → 模拟重启（丢弃管理器内存态，新实例冷启动于同一底层
//!   存储）→ load_from_device → 断言新分区 Active 且元数据（版本/大小/哈希/
//!   签名）完整、旧分区保留 Verified 备用、槽位镜像字节与原镜像一致；
//! - INT2（★ 出口判定复演：升级失败自动回滚）：activate(B) 后模拟新镜像
//!   反复启动失败，boot_health_check 连超 3 次 → 自动回滚：B Failed、
//!   A Active、active_slot=A、返回 false；
//! - INT3（故障注入·写入中断）：写到一半停止 → 分区停留 Writing →
//!   verify/activate 均拒绝 Err(VerifyFailed) → 重新 prepare_upgrade 可
//!   重试并成功完成升级；
//! - INT4（篡改拒绝）：① 替换签名任一字节 → verify 拒绝 SignatureInvalid；
//!   ② 写入数据与 image_hash 不符 → HashMismatch；两种情况分区均置 Failed；
//! - PERF5：release 打印 10MB 镜像 write_chunk+verify 端到端耗时；
//!   `ENEROS_PERF_GATE=1`（`var(...).as_deref() == Ok("1")`，规避终端残留
//!   空串误激活）硬断言 < 30s（蓝图 §6.3/§7.2），未设置仅打印不断言
//!   （与 v0.113.0~v0.122.0 PERF 先例一致）。
//!
//! 集成测试运行于 host std 环境（`std::time::Instant`/`std::env`/`Rc` 可用），
//! 通过 `eneros_ota_ab` crate 公共 API 调用被测的 no_std 代码。

// 测试代码允许 panic!/unwrap（与 src 内嵌测试模块同口径）
#![allow(clippy::disallowed_macros)]

use std::cell::RefCell;
use std::rc::Rc;

use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
use eneros_ota_ab::ab_partition::{
    META_BLOCK, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK, SLOT_B_BASE_BLOCK,
};
use eneros_ota_ab::{
    boot_health_check, load_from_device, save_to_device, AbPartitionOta, OtaError, OtaManager,
    Partition, PartitionSlot, PartitionState, UpgradeMeta,
};
use eneros_storage::{BlockDevice, DeviceHealth, MockBlockDevice, StorageError};
use heapless::String as HString;

/// 10 MiB 镜像大小（蓝图 §6.3/§7.2 性能口径 10MB）。
const IMAGE_SIZE_10MB: usize = 10 * 1024 * 1024;
/// OTA 下载分块大小（64 KiB，对应 configs/ota.toml [upgrade] chunk_size）。
const CHUNK_SIZE: u32 = 64 * 1024;
/// 大镜像用块大小：8 KiB 时单分区容量 = PARTITION_BLOCKS × 8 KiB = 16 MiB
/// ≥ 10 MiB 镜像（分区容量随 `block_size()` 缩放，见 ab_partition 注释）。
const LARGE_BLOCK_SIZE: usize = 8 * 1024;
/// 设备总块数：保留块 + 双分区。
const TOTAL_BLOCKS: u64 = SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS;

/// 共享句柄块设备：INT1 模拟重启时同一底层存储可被新旧管理器实例接力持有
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

/// 构造测试环境：真实 SM2 密钥对 + MockBlockDevice（512B 块，小镜像用）。
fn setup() -> (AbPartitionOta<MockBlockDevice>, Sm2KeyPair, CsRng) {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let dev = MockBlockDevice::new(TOTAL_BLOCKS, 512);
    let ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
    (ota, kp, rng)
}

/// 确定性伪随机填充（xorshift32，固定种子 → 可复现；模拟真实固件镜像字节流，
/// 避免规律性填充掩盖跨块读-改-写路径的对齐问题）。
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

/// 构造正样本镜像与元数据：确定性伪随机内容 + 真实 SM3 哈希 + 根私钥 SM2 签名。
fn make_image(kp: &Sm2KeyPair, rng: &mut CsRng, size: usize, chunk: u32) -> (Vec<u8>, UpgradeMeta) {
    let image = pseudo_random_image(0x1234_5678, size);
    let hash = sm3_hash(&image);
    let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng).expect("签名失败");
    let meta = UpgradeMeta {
        target_version: HString::<32>::try_from("0.123.0").expect("版本号超长"),
        image_size: size as u64,
        image_hash: hash,
        image_sig: sig.to_bytes(),
        download_url: HString::<128>::try_from("https://ota.eneros.local/fw-0.123.0.bin")
            .expect("URL 超长"),
        chunk_size: chunk,
        total_chunks: (size as u32).div_ceil(chunk),
        downloaded_chunks: 0,
    };
    (image, meta)
}

/// 取指定槽位的分区快照（get_partitions 数组口径：索引 0=A，1=B）。
fn part<D: BlockDevice>(ota: &AbPartitionOta<D>, slot: PartitionSlot) -> Partition {
    let parts = ota.get_partitions();
    match slot {
        PartitionSlot::A => parts[0].clone(),
        PartitionSlot::B => parts[1].clone(),
    }
}

/// 按 chunk_size 分块写入完整镜像；断言每块成功且写完后分区进入 Written。
fn write_image<D: BlockDevice>(
    ota: &mut AbPartitionOta<D>,
    slot: PartitionSlot,
    image: &[u8],
    chunk: u32,
) {
    let mut offset = 0u64;
    for c in image.chunks(chunk as usize) {
        ota.write_chunk(slot, offset, c)
            .expect("write_chunk 应成功");
        offset += c.len() as u64;
    }
    assert_eq!(
        part(ota, slot).state,
        PartitionState::Written,
        "全部分块写完应置 Written"
    );
}

/// INT1：★ 完整升级流程——10MB 真实签名镜像全链路 + 持久化 + 模拟重启恢复。
#[test]
fn int1_full_upgrade_persist_reboot() {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    // 共享句柄存储：重启后新管理器实例接力持有同一底层块设备
    let storage = SharedDevice::new(TOTAL_BLOCKS, LARGE_BLOCK_SIZE);
    let (image, meta) = make_image(&kp, &mut rng, IMAGE_SIZE_10MB, CHUNK_SIZE);
    assert_eq!(meta.total_chunks, 160, "10 MiB / 64 KiB = 160 块");

    // ── 第一启动周期：A(workspace v0.192.0) 运行中，OTA 升级 10MB 镜像到 B ──
    let mut ota = AbPartitionOta::new(storage.handle(), kp.public_key, 1_000);
    let slot = ota.prepare_upgrade(meta.clone()).expect("prepare 应成功");
    assert_eq!(slot, PartitionSlot::B, "非激活分区为 B");
    assert_eq!(part(&ota, slot).state, PartitionState::Writing);

    write_image(&mut ota, slot, &image, CHUNK_SIZE);

    assert!(ota.verify(slot).expect("verify 应通过"));
    assert_eq!(part(&ota, slot).state, PartitionState::Verified);

    ota.activate(slot).expect("activate 应成功");
    assert_eq!(ota.get_active_slot(), PartitionSlot::B);
    assert_eq!(part(&ota, PartitionSlot::B).state, PartitionState::Active);
    assert_eq!(
        part(&ota, PartitionSlot::A).state,
        PartitionState::Verified,
        "旧分区保留 Verified 作回滚备用"
    );

    // 分区表持久化到保留块（魔数 "EOTA" 落盘），随后掉电丢弃内存态
    save_to_device(&mut ota).expect("save 应成功");
    drop(ota);
    {
        let bs = storage.block_size();
        let mut meta_block = vec![0u8; bs];
        storage
            .read_block(META_BLOCK, &mut meta_block)
            .expect("读保留块");
        assert_eq!(&meta_block[..4], b"EOTA", "保留块应含分区表魔数");
    }

    // ── 第二启动周期：冷启动（安全默认 A Active v0.0.0）→ 从保留块恢复 ──
    let mut ota2 = AbPartitionOta::new(storage.handle(), kp.public_key, 2_000);
    assert_eq!(ota2.get_active_slot(), PartitionSlot::A, "冷启动默认为 A");
    assert_eq!(part(&ota2, PartitionSlot::A).version.as_str(), "0.0.0");
    assert_eq!(part(&ota2, PartitionSlot::B).state, PartitionState::Empty);
    let loaded = load_from_device(&mut ota2, 3_000).expect("load 不应 I/O 失败");
    assert!(loaded, "应从保留块恢复分区表");

    // 新分区 Active 且元数据完整（版本/大小/哈希/签名逐字段一致）
    assert_eq!(ota2.get_active_slot(), PartitionSlot::B);
    let pb = part(&ota2, PartitionSlot::B);
    assert_eq!(pb.state, PartitionState::Active);
    assert_eq!(pb.version.as_str(), "0.123.0");
    assert_eq!(pb.size, IMAGE_SIZE_10MB as u64);
    assert_eq!(pb.hash, meta.image_hash);
    assert_eq!(pb.signature, meta.image_sig);
    let pa = part(&ota2, PartitionSlot::A);
    assert_eq!(pa.state, PartitionState::Verified, "旧分区仍为回滚备用");

    // 槽位 B 镜像字节与原镜像一致（抽首/尾块核验读-改-写落盘正确）
    let bs = storage.block_size();
    let mut first = vec![0u8; bs];
    storage
        .read_block(SLOT_B_BASE_BLOCK, &mut first)
        .expect("读首块");
    assert_eq!(first[..], image[..bs], "首块内容应一致");
    let last_off = IMAGE_SIZE_10MB - bs;
    let mut tail = vec![0u8; bs];
    storage
        .read_block(SLOT_B_BASE_BLOCK + (last_off / bs) as u64, &mut tail)
        .expect("读尾块");
    assert_eq!(tail[..], image[last_off..], "尾块内容应一致");
}

/// INT2：★ 出口判定复演——activate(B) 后新镜像反复启动失败自动回滚。
#[test]
fn int2_auto_rollback_after_boot_failures() {
    let (mut ota, kp, mut rng) = setup();
    // 完成一次完整升级：A(0.0.0) → B(0.123.0)
    let (image, meta) = make_image(&kp, &mut rng, 128 * 1024, 32 * 1024);
    let slot = ota.prepare_upgrade(meta).expect("prepare");
    write_image(&mut ota, slot, &image, 32 * 1024);
    assert!(ota.verify(slot).expect("verify"));
    ota.activate(slot).expect("activate");
    assert_eq!(ota.get_active_slot(), PartitionSlot::B);

    // 模拟新镜像反复启动失败：每次上电调用一次 boot_health_check
    for i in 1..=3u32 {
        assert!(
            boot_health_check(&mut ota, 2_000 + u64::from(i)),
            "第 {i} 次未超 max_boot_attempts"
        );
        assert_eq!(ota.get_active_slot(), PartitionSlot::B);
        assert_eq!(part(&ota, PartitionSlot::B).boot_count, i);
    }
    // 第 4 次超过 max_boot_attempts=3 → 自动回滚，本次启动判定失败
    assert!(
        !boot_health_check(&mut ota, 2_004),
        "超限应判定启动失败并触发回滚"
    );
    assert_eq!(ota.get_active_slot(), PartitionSlot::A, "自动回滚到旧版本");
    let pb = part(&ota, PartitionSlot::B);
    assert_eq!(pb.state, PartitionState::Failed, "新分区置 Failed");
    assert_eq!(pb.version.as_str(), "0.123.0", "失败镜像版本留痕可查询");
    let pa = part(&ota, PartitionSlot::A);
    assert_eq!(pa.state, PartitionState::Active, "旧版本恢复 Active");
    assert_eq!(pa.version.as_str(), "0.0.0");
}

/// INT3：故障注入——写入中断停留 Writing，activate 拒绝；重新 prepare 重试成功。
#[test]
fn int3_interrupted_write_then_retry() {
    let (mut ota, kp, mut rng) = setup();
    let (image, meta) = make_image(&kp, &mut rng, 64 * 1024, 16 * 1024); // 4 块
    let slot = ota.prepare_upgrade(meta.clone()).expect("prepare");
    assert_eq!(slot, PartitionSlot::B);

    // 故障注入：只写前 2/4 块即"断电/断网"中断
    ota.write_chunk(slot, 0, &image[..16 * 1024])
        .expect("chunk0");
    ota.write_chunk(slot, 16 * 1024, &image[16 * 1024..32 * 1024])
        .expect("chunk1");
    assert_eq!(
        part(&ota, slot).state,
        PartitionState::Writing,
        "未写完整应停留 Writing"
    );

    // 中断态下 verify/activate 均拒绝（状态机保护，残缺镜像不可启动）
    assert_eq!(
        ota.verify(slot).expect_err("Writing 态 verify 应拒绝"),
        OtaError::VerifyFailed
    );
    assert_eq!(
        ota.activate(slot).expect_err("未 Verified 应拒绝"),
        OtaError::VerifyFailed
    );
    assert_eq!(ota.get_active_slot(), PartitionSlot::A, "A 仍为激活分区");

    // 重新 prepare_upgrade 重试：downloaded_chunks 归零、状态重置 Writing
    let slot2 = ota.prepare_upgrade(meta).expect("重试 prepare 应成功");
    assert_eq!(slot2, slot);
    assert_eq!(part(&ota, slot2).state, PartitionState::Writing);

    // 重传完整镜像并完成升级
    write_image(&mut ota, slot2, &image, 16 * 1024);
    assert!(ota.verify(slot2).expect("重试 verify 应通过"));
    ota.activate(slot2).expect("重试 activate 应成功");
    assert_eq!(ota.get_active_slot(), PartitionSlot::B);
    assert_eq!(part(&ota, slot2).version.as_str(), "0.123.0");
}

/// INT4：篡改拒绝——① 签名替换一字节 → SignatureInvalid；
/// ② 写入数据与 image_hash 不符 → HashMismatch；两种情况分区均置 Failed。
#[test]
fn int4_tampered_image_rejected() {
    // ① 签名被替换任一字节（哈希仍匹配，SM2 验签失败）
    let (mut ota, kp, mut rng) = setup();
    let (image, mut meta) = make_image(&kp, &mut rng, 64 * 1024, 16 * 1024);
    meta.image_sig[7] ^= 0xFF;
    let slot = ota.prepare_upgrade(meta).expect("prepare");
    write_image(&mut ota, slot, &image, 16 * 1024);
    assert_eq!(
        ota.verify(slot).expect_err("签名篡改应拒绝"),
        OtaError::SignatureInvalid
    );
    assert_eq!(part(&ota, slot).state, PartitionState::Failed);
    assert_eq!(ota.get_active_slot(), PartitionSlot::A, "A 不受影响");

    // ② 传输数据被篡改（写入字节与 image_hash 不符，SM3 回读比对失败）
    let (mut ota2, kp2, mut rng2) = setup();
    let (image, meta2) = make_image(&kp2, &mut rng2, 64 * 1024, 16 * 1024);
    let mut tampered = image;
    tampered[33 * 1024] ^= 0xA5; // 第 3 块内翻转一字节
    let slot2 = ota2.prepare_upgrade(meta2).expect("prepare");
    write_image(&mut ota2, slot2, &tampered, 16 * 1024);
    assert_eq!(
        ota2.verify(slot2).expect_err("数据篡改应拒绝"),
        OtaError::HashMismatch
    );
    assert_eq!(part(&ota2, slot2).state, PartitionState::Failed);
    assert_eq!(ota2.get_active_slot(), PartitionSlot::A, "A 不受影响");
}

/// PERF5：蓝图 §6.3/§7.2 性能基准——10MB 镜像 write_chunk+verify 端到端耗时。
///
/// 计时口径：从首个 write_chunk 起至 verify 返回止（镜像生成/签名/prepare
/// 为前置准备不计时）；MockBlockDevice 内存后端实测远超 30s 指标。
/// 默认仅打印不断言（含 debug/release 口径说明）；仅当
/// `ENEROS_PERF_GATE=1`（`var(...).as_deref() == Ok("1")`，与
/// v0.113.0~v0.122.0 PERF 先例一致）时启用 < 30s 硬断言门禁，
/// 面向性能 CI / 目标硬件场景。
#[test]
fn perf5_write_and_verify_10mb() {
    let mut rng = CsRng::new();
    let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
    let dev = MockBlockDevice::new(TOTAL_BLOCKS, LARGE_BLOCK_SIZE);
    let mut ota = AbPartitionOta::new(dev, kp.public_key, 1_000);
    // 前置准备（不计时）：确定性伪随机 10MB 镜像 + SM3 哈希 + SM2 签名 + prepare
    let (image, meta) = make_image(&kp, &mut rng, IMAGE_SIZE_10MB, CHUNK_SIZE);
    let slot = ota.prepare_upgrade(meta).expect("prepare");

    // 计时：write_chunk ×160 + verify（SM3 回读 10MB + SM2 验签）
    let start = std::time::Instant::now();
    let mut offset = 0u64;
    for chunk in image.chunks(CHUNK_SIZE as usize) {
        ota.write_chunk(slot, offset, std::hint::black_box(chunk))
            .expect("write_chunk 应成功");
        offset += chunk.len() as u64;
    }
    assert!(ota.verify(slot).expect("verify 应通过"));
    let elapsed = start.elapsed();

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "[PERF5] 10MB 镜像 write_chunk(160×64KiB)+verify 端到端耗时: {elapsed:?}（{profile} 构建；设 ENEROS_PERF_GATE=1 启用 <30s 断言门禁）"
    );

    // 性能门禁：仅 ENEROS_PERF_GATE=1 时启用断言（蓝图 §6.3/§7.2）
    if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
        assert!(
            elapsed.as_secs() < 30,
            "PERF5 端到端 {elapsed:?} 超过 30s 上限（蓝图 §7.2）"
        );
    }
}
