//! 回滚机制、启动健康检查与分区表断电持久化（蓝图 §4.3 / §5.4，D5）。
//!
//! - [`rollback`]：升级失败后切回旧版本分区（目标必须为 Verified 备用态）；
//! - [`boot_health_check`]：每次启动调用，连续失败超过 `max_boot_attempts`
//!   （默认 3）自动触发回滚；
//! - [`PartitionTable`] + [`save_to_device`] / [`load_from_device`]：分区表
//!   定长序列化（魔数 "EOTA" + CRC32 + **SM3-HMAC**）持久化到保留块，
//!   断电后上电恢复；校验失败回退安全默认（A Active v0.0.0）。
//!   33 SE-18：HMAC 密钥由集成层注入（与 OTA root_key 同源派生），
//!   伪造分区表（重算 CRC 无密钥 HMAC）必拒。
//!
//! 注：`PartitionState::Rolling` 为回滚切换的瞬时中间态语义：实际实现中
//! 状态切换在同一调用内原子完成（Failed/Active 一次性落位），Rolling 仅作
//! 为状态机文档化标记，不持久化落盘。

use alloc::vec;

use eneros_crypto::{ct_eq, hmac_sm3};
use eneros_storage::{crc32, BlockDevice};

use crate::ab_partition::{AbPartitionOta, Partition, PartitionSlot, PartitionState, META_BLOCK};
use crate::upgrade::OtaError;

/// 回滚到旧版本分区。
///
/// 回滚目标（非激活分区）必须为 Verified 备用态，否则返回
/// [`OtaError::NoActiveSlot`]；成功时将当前分区置 Failed、目标分区置
/// Active、切换 `active_slot` 并清零 `boot_attempts`，返回回滚到的槽位。
pub fn rollback<D: BlockDevice>(ota: &mut AbPartitionOta<D>) -> Result<PartitionSlot, OtaError> {
    let rollback_slot = match ota.active_slot {
        PartitionSlot::A => PartitionSlot::B,
        PartitionSlot::B => PartitionSlot::A,
    };
    let rollback_idx = AbPartitionOta::<D>::slot_index(rollback_slot);
    if ota.partitions[rollback_idx].state != PartitionState::Verified {
        return Err(OtaError::NoActiveSlot);
    }
    // 当前（新镜像）分区标记失败；旧分区恢复激活
    let failed_idx = AbPartitionOta::<D>::slot_index(ota.active_slot);
    ota.partitions[failed_idx].state = PartitionState::Failed;
    ota.partitions[rollback_idx].state = PartitionState::Active;
    ota.active_slot = rollback_slot;
    ota.boot_attempts = 0;
    ota.pending = None;
    Ok(rollback_slot)
}

/// 启动健康检查：每次启动调用一次（D1：时间戳经 `now_ms` 传入）。
///
/// 累加 `boot_attempts` 与当前分区 `boot_count` / `last_boot_ms`；
/// 当 `boot_attempts` 超过 `max_boot_attempts`（默认 3）时自动触发回滚
/// 并返回 `false`（本次启动判定失败）；否则返回 `true`。
pub fn boot_health_check<D: BlockDevice>(ota: &mut AbPartitionOta<D>, now_ms: u64) -> bool {
    ota.boot_attempts += 1;
    let idx = AbPartitionOta::<D>::slot_index(ota.active_slot);
    ota.partitions[idx].boot_count += 1;
    ota.partitions[idx].last_boot_ms = now_ms;
    if ota.boot_attempts > ota.max_boot_attempts {
        // 新分区反复启动失败，自动回滚旧版本
        let _ = rollback(ota);
        return false;
    }
    true
}

// ============================================================================
// 分区表持久化（D5：定长序列化 + 魔数 + CRC32）
// ============================================================================

/// 分区表魔数 "EOTA"。
const TABLE_MAGIC: [u8; 4] = *b"EOTA";
/// 单分区定长编码长度：slot 1B + state 1B + version_len 1B + version 32B
/// + size 8B + hash 32B + signature 64B + boot_count 4B + last_boot_ms 8B。
const PARTITION_LEN: usize = 1 + 1 + 1 + 32 + 8 + 32 + 64 + 4 + 8;
/// SM3-HMAC 摘要长度（33 SE-18：分区表认证）。
pub const TABLE_HMAC_LEN: usize = 32;
/// 分区表序列化总长度：魔数 4B + active_slot 1B + 双分区 + CRC32 4B
/// + HMAC-SM3 32B。
pub const TABLE_LEN: usize = 4 + 1 + 2 * PARTITION_LEN + 4 + TABLE_HMAC_LEN;

/// 分区表快照（用于持久化）。
#[derive(Debug, Clone)]
pub struct PartitionTable {
    /// 双分区全字段
    pub partitions: [Partition; 2],
    /// 当前激活槽位
    pub active_slot: PartitionSlot,
}

fn encode_slot(slot: PartitionSlot) -> u8 {
    match slot {
        PartitionSlot::A => 0,
        PartitionSlot::B => 1,
    }
}

fn decode_slot(byte: u8) -> Option<PartitionSlot> {
    match byte {
        0 => Some(PartitionSlot::A),
        1 => Some(PartitionSlot::B),
        _ => None,
    }
}

fn encode_state(state: PartitionState) -> u8 {
    match state {
        PartitionState::Empty => 0,
        PartitionState::Writing => 1,
        PartitionState::Written => 2,
        PartitionState::Verified => 3,
        PartitionState::Active => 4,
        PartitionState::Failed => 5,
        PartitionState::Rolling => 6,
    }
}

fn decode_state(byte: u8) -> Option<PartitionState> {
    match byte {
        0 => Some(PartitionState::Empty),
        1 => Some(PartitionState::Writing),
        2 => Some(PartitionState::Written),
        3 => Some(PartitionState::Verified),
        4 => Some(PartitionState::Active),
        5 => Some(PartitionState::Failed),
        6 => Some(PartitionState::Rolling),
        _ => None,
    }
}

impl PartitionTable {
    /// 从管理器取快照。
    pub fn snapshot<D: BlockDevice>(ota: &AbPartitionOta<D>) -> Self {
        Self {
            partitions: ota.partitions.clone(),
            active_slot: ota.active_slot,
        }
    }

    /// 定长序列化：魔数 + active_slot + 双分区全字段 + CRC32（末尾 4B）
    /// + SM3-HMAC（覆盖全表含 CRC，最末 32B）。
    ///
    /// `hmac_key`：分区表认证密钥（33 SE-18，由集成层从 OTA root_key
    /// 同源派生注入，禁止硬编码）。
    pub fn serialize(&self, hmac_key: &[u8]) -> [u8; TABLE_LEN] {
        let mut out = [0u8; TABLE_LEN];
        out[..4].copy_from_slice(&TABLE_MAGIC);
        out[4] = encode_slot(self.active_slot);
        let mut pos = 5;
        for p in &self.partitions {
            out[pos] = encode_slot(p.slot);
            out[pos + 1] = encode_state(p.state);
            let vlen = p.version.len().min(32);
            out[pos + 2] = vlen as u8;
            out[pos + 3..pos + 3 + vlen].copy_from_slice(&p.version.as_bytes()[..vlen]);
            out[pos + 35..pos + 43].copy_from_slice(&p.size.to_be_bytes());
            out[pos + 43..pos + 75].copy_from_slice(&p.hash);
            out[pos + 75..pos + 139].copy_from_slice(&p.signature);
            out[pos + 139..pos + 143].copy_from_slice(&p.boot_count.to_be_bytes());
            out[pos + 143..pos + 151].copy_from_slice(&p.last_boot_ms.to_be_bytes());
            pos += PARTITION_LEN;
        }
        // CRC32 覆盖魔数至双分区末尾，追加在 HMAC 之前
        let crc = crc32(&out[..TABLE_LEN - 4 - TABLE_HMAC_LEN]);
        out[TABLE_LEN - 4 - TABLE_HMAC_LEN..TABLE_LEN - TABLE_HMAC_LEN]
            .copy_from_slice(&crc.to_be_bytes());
        // 33 SE-18：SM3-HMAC 覆盖全表（含 CRC）——伪造表（重算 CRC 但
        // 无密钥）必拒。
        let mac = hmac_sm3(hmac_key, &out[..TABLE_LEN - TABLE_HMAC_LEN]);
        out[TABLE_LEN - TABLE_HMAC_LEN..].copy_from_slice(&mac);
        out
    }

    /// 反序列化：校验魔数、SM3-HMAC（常量时间比较）与 CRC32，任一失败
    /// 返回 `None`。
    pub fn deserialize(buf: &[u8], hmac_key: &[u8]) -> Option<Self> {
        if buf.len() < TABLE_LEN {
            return None;
        }
        if buf[..4] != TABLE_MAGIC {
            return None;
        }
        // 33 SE-18：先验 HMAC——CRC 可被无密钥方重算，HMAC 不可伪造。
        let mac = hmac_sm3(hmac_key, &buf[..TABLE_LEN - TABLE_HMAC_LEN]);
        if !ct_eq(&mac, &buf[TABLE_LEN - TABLE_HMAC_LEN..TABLE_LEN]) {
            return None;
        }
        let mut crc_bytes = [0u8; 4];
        crc_bytes.copy_from_slice(&buf[TABLE_LEN - 4 - TABLE_HMAC_LEN..TABLE_LEN - TABLE_HMAC_LEN]);
        let expected = u32::from_be_bytes(crc_bytes);
        if crc32(&buf[..TABLE_LEN - 4 - TABLE_HMAC_LEN]) != expected {
            return None;
        }
        let active_slot = decode_slot(buf[4])?;
        let mut decoded: [Option<Partition>; 2] = [None, None];
        let mut pos = 5;
        for entry in decoded.iter_mut() {
            let slot = decode_slot(buf[pos])?;
            let state = decode_state(buf[pos + 1])?;
            let vlen = (buf[pos + 2] as usize).min(32);
            let version = core::str::from_utf8(&buf[pos + 3..pos + 3 + vlen])
                .ok()
                .and_then(|s| heapless::String::<32>::try_from(s).ok())?;
            let mut size = [0u8; 8];
            size.copy_from_slice(&buf[pos + 35..pos + 43]);
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&buf[pos + 43..pos + 75]);
            let mut signature = [0u8; 64];
            signature.copy_from_slice(&buf[pos + 75..pos + 139]);
            let mut boot_count = [0u8; 4];
            boot_count.copy_from_slice(&buf[pos + 139..pos + 143]);
            let mut last_boot_ms = [0u8; 8];
            last_boot_ms.copy_from_slice(&buf[pos + 143..pos + 151]);
            *entry = Some(Partition {
                slot,
                state,
                version,
                size: u64::from_be_bytes(size),
                hash,
                signature,
                boot_count: u32::from_be_bytes(boot_count),
                last_boot_ms: u64::from_be_bytes(last_boot_ms),
            });
            pos += PARTITION_LEN;
        }
        // 双分区必然解码成功（上方循环任一失败已提前返回）
        match (decoded[0].take(), decoded[1].take()) {
            (Some(pa), Some(pb)) => Some(Self {
                partitions: [pa, pb],
                active_slot,
            }),
            _ => None,
        }
    }
}

/// 将分区表写入后端设备保留块（块 0）。
///
/// `hmac_key`：分区表认证密钥（33 SE-18，与 [`load_from_device`] 一致）。
pub fn save_to_device<D: BlockDevice>(
    ota: &mut AbPartitionOta<D>,
    hmac_key: &[u8],
) -> Result<(), OtaError> {
    let table = PartitionTable::snapshot(ota);
    let bytes = table.serialize(hmac_key);
    let bs = ota.device.block_size();
    if bs < TABLE_LEN {
        return Err(OtaError::WriteFailed { offset: 0 });
    }
    let mut buf = vec![0u8; bs];
    buf[..TABLE_LEN].copy_from_slice(&bytes);
    ota.device
        .write_block(META_BLOCK, &buf)
        .map_err(|_| OtaError::WriteFailed { offset: 0 })
}

/// 从后端设备保留块读回分区表。
///
/// 返回 `Ok(true)` 表示成功恢复；魔数/HMAC/CRC32 校验失败时回退安全默认
/// （A Active v0.0.0）并返回 `Ok(false)`；仅设备 I/O 错误返回 `Err`。
pub fn load_from_device<D: BlockDevice>(
    ota: &mut AbPartitionOta<D>,
    hmac_key: &[u8],
    now_ms: u64,
) -> Result<bool, OtaError> {
    let bs = ota.device.block_size();
    if bs < TABLE_LEN {
        apply_safe_default(ota, now_ms);
        return Ok(false);
    }
    let mut buf = vec![0u8; bs];
    ota.device
        .read_block(META_BLOCK, &mut buf)
        .map_err(|_| OtaError::VerifyFailed)?;
    match PartitionTable::deserialize(&buf[..TABLE_LEN], hmac_key) {
        Some(table) => {
            ota.partitions = table.partitions;
            ota.active_slot = table.active_slot;
            ota.boot_attempts = 0;
            ota.pending = None;
            Ok(true)
        }
        None => {
            apply_safe_default(ota, now_ms);
            Ok(false)
        }
    }
}

/// 回退安全默认：A Active(workspace v0.192.0) / B Empty / active_slot=A。
fn apply_safe_default<D: BlockDevice>(ota: &mut AbPartitionOta<D>, now_ms: u64) {
    ota.partitions[0].state = PartitionState::Active;
    ota.partitions[0].version = heapless::String::<32>::try_from("0.0.0").unwrap_or_default();
    ota.partitions[0].size = 0;
    ota.partitions[0].hash = [0; 32];
    ota.partitions[0].signature = [0; 64];
    ota.partitions[0].boot_count = 0;
    ota.partitions[0].last_boot_ms = now_ms;
    ota.partitions[1].state = PartitionState::Empty;
    ota.partitions[1].version = heapless::String::<32>::try_from("0.0.0").unwrap_or_default();
    ota.partitions[1].size = 0;
    ota.partitions[1].hash = [0; 32];
    ota.partitions[1].signature = [0; 64];
    ota.partitions[1].boot_count = 0;
    ota.partitions[1].last_boot_ms = 0;
    ota.active_slot = PartitionSlot::A;
    ota.boot_attempts = 0;
    ota.pending = None;
}

// ============================================================================
// 单元测试（RBK1~4 回滚 / PST5~7 持久化）
// ============================================================================

#[cfg(test)]
mod tests {
    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
    use eneros_storage::MockBlockDevice;
    use heapless::String as HString;

    use super::*;
    use crate::ab_partition::{META_BLOCK, PARTITION_BLOCKS, SLOT_A_BASE_BLOCK};
    use crate::upgrade::{OtaManager, UpgradeMeta};

    /// 设备总块数：保留块 + 双分区。
    // 蓝图 70.2：META_BLOCK 移至保护间隙（块 4704）后，测试设备须容纳
    // meta 块与 A/B 双槽的整体跨度。
    const TEST_BLOCKS: u64 = if META_BLOCK + 1 > SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS {
        META_BLOCK + 1
    } else {
        SLOT_A_BASE_BLOCK + 2 * PARTITION_BLOCKS
    };

    /// 33 SE-18：测试用分区表 HMAC 密钥（生产由集成层从 root_key 派生注入）。
    const TEST_HMAC_KEY: [u8; 32] = [0x42; 32];

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
        let image: alloc::vec::Vec<u8> = (0..size).map(|i| (i % 241) as u8).collect();
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

    /// 完成一次完整升级到 B：prepare → write → verify → activate。
    fn upgrade_to_b(ota: &mut AbPartitionOta<MockBlockDevice>, kp: &Sm2KeyPair, rng: &mut CsRng) {
        let (image, meta) = make_image(kp, rng, 1024, 512);
        let slot = ota.prepare_upgrade(meta).expect("prepare");
        ota.write_chunk(slot, 0, &image[..512]).expect("chunk0");
        ota.write_chunk(slot, 512, &image[512..]).expect("chunk1");
        assert!(ota.verify(slot).expect("verify"));
        ota.activate(slot).expect("activate");
        assert_eq!(ota.get_active_slot(), PartitionSlot::B);
    }

    /// RBK1：回滚成功路径字段断言。
    #[test]
    fn rbk1_rollback_success() {
        let (mut ota, kp, mut rng) = setup();
        upgrade_to_b(&mut ota, &kp, &mut rng);
        let slot = rollback(&mut ota).expect("回滚应成功");
        assert_eq!(slot, PartitionSlot::A);
        let parts = ota.get_partitions();
        assert_eq!(parts[1].state, PartitionState::Failed, "新分区置 Failed");
        assert_eq!(parts[0].state, PartitionState::Active, "旧分区恢复 Active");
        assert_eq!(ota.get_active_slot(), PartitionSlot::A);
        assert_eq!(ota.boot_attempts, 0);
    }

    /// RBK2：回滚目标非 Verified 拒绝。
    #[test]
    fn rbk2_rollback_rejected_when_target_not_verified() {
        let (mut ota, _, _) = setup();
        // 初始态：B 为 Empty，不可作为回滚目标
        let err = rollback(&mut ota).expect_err("目标非 Verified 应拒绝");
        assert_eq!(err, OtaError::NoActiveSlot);
        assert_eq!(ota.get_active_slot(), PartitionSlot::A);
        assert_eq!(ota.get_partitions()[0].state, PartitionState::Active);
    }

    /// RBK3：★ 出口判定复演——activate(B) 后连超 3 次启动失败自动回滚。
    #[test]
    fn rbk3_auto_rollback_after_max_boot_attempts() {
        let (mut ota, kp, mut rng) = setup();
        upgrade_to_b(&mut ota, &kp, &mut rng);
        // 前 3 次启动检查：未超限，返回 true，boot_count 累加
        for i in 1..=3u32 {
            assert!(boot_health_check(&mut ota, 2_000 + i as u64));
            assert_eq!(ota.boot_attempts, i);
            assert_eq!(ota.get_partitions()[1].boot_count, i);
            assert_eq!(ota.get_active_slot(), PartitionSlot::B);
        }
        // 第 4 次超限 → 自动回滚：B Failed、A Active、active_slot=A、返回 false
        assert!(!boot_health_check(&mut ota, 2_004));
        let parts = ota.get_partitions();
        assert_eq!(parts[1].state, PartitionState::Failed);
        assert_eq!(parts[0].state, PartitionState::Active);
        assert_eq!(ota.get_active_slot(), PartitionSlot::A);
    }

    /// RBK4：回滚后 boot_attempts 清零。
    #[test]
    fn rbk4_boot_attempts_reset_after_rollback() {
        let (mut ota, kp, mut rng) = setup();
        upgrade_to_b(&mut ota, &kp, &mut rng);
        assert!(boot_health_check(&mut ota, 2_000));
        assert!(boot_health_check(&mut ota, 2_001));
        assert_eq!(ota.boot_attempts, 2);
        let slot = rollback(&mut ota).expect("回滚应成功");
        assert_eq!(slot, PartitionSlot::A);
        assert_eq!(ota.boot_attempts, 0, "回滚后启动尝试计数清零");
    }

    /// PST5：序列化往返逐字段一致。
    #[test]
    fn pst5_serialize_round_trip() {
        let (mut ota, kp, mut rng) = setup();
        upgrade_to_b(&mut ota, &kp, &mut rng);
        // 让字段非平凡：boot_count / last_boot_ms 非零
        assert!(boot_health_check(&mut ota, 3_333));
        let table = PartitionTable::snapshot(&ota);
        let bytes = table.serialize(&TEST_HMAC_KEY);
        assert_eq!(bytes.len(), TABLE_LEN);
        let restored = PartitionTable::deserialize(&bytes, &TEST_HMAC_KEY).expect("反序列化应成功");
        assert_eq!(restored.active_slot, table.active_slot);
        for (a, b) in restored.partitions.iter().zip(table.partitions.iter()) {
            assert_eq!(a.slot, b.slot);
            assert_eq!(a.state, b.state);
            assert_eq!(a.version.as_str(), b.version.as_str());
            assert_eq!(a.size, b.size);
            assert_eq!(a.hash, b.hash);
            assert_eq!(a.signature, b.signature);
            assert_eq!(a.boot_count, b.boot_count);
            assert_eq!(a.last_boot_ms, b.last_boot_ms);
        }
    }

    /// PST6：篡改任一字节 → CRC 不匹配 → 安全默认。
    #[test]
    fn pst6_tampered_byte_falls_back_to_default() {
        let (mut ota, kp, mut rng) = setup();
        upgrade_to_b(&mut ota, &kp, &mut rng);
        let table = PartitionTable::snapshot(&ota);
        let mut bytes = table.serialize(&TEST_HMAC_KEY);
        // 篡改数据区一个字节
        bytes[10] ^= 0x5A;
        assert!(
            PartitionTable::deserialize(&bytes, &TEST_HMAC_KEY).is_none(),
            "HMAC/CRC 校验失败应拒绝（HMAC 先验，伪造表在此被拒）"
        );
        // 篡改魔数
        let mut bad_magic = table.serialize(&TEST_HMAC_KEY);
        bad_magic[0] = b'X';
        assert!(
            PartitionTable::deserialize(&bad_magic, &TEST_HMAC_KEY).is_none(),
            "魔数错误应拒绝"
        );

        // 持久化后篡改保留块，load 应回退安全默认
        save_to_device(&mut ota, &TEST_HMAC_KEY).expect("save 应成功");
        let bs = ota.device.block_size();
        let mut buf = alloc::vec![0u8; bs];
        ota.device.read_block(META_BLOCK, &mut buf).expect("read");
        buf[7] ^= 0xFF;
        ota.device.write_block(META_BLOCK, &buf).expect("write");
        let loaded = load_from_device(&mut ota, &TEST_HMAC_KEY, 9_999).expect("load 不应 I/O 失败");
        assert!(!loaded, "CRC 不匹配应回退安全默认");
        let parts = ota.get_partitions();
        assert_eq!(ota.get_active_slot(), PartitionSlot::A);
        assert_eq!(parts[0].state, PartitionState::Active);
        assert_eq!(parts[0].version.as_str(), "0.0.0");
        assert_eq!(parts[0].last_boot_ms, 9_999);
        assert_eq!(parts[1].state, PartitionState::Empty);
    }

    /// PST7：经 MockBlockDevice 保留块 save → load 恢复一致。
    #[test]
    fn pst7_save_load_via_device_round_trip() {
        let (mut ota, kp, mut rng) = setup();
        upgrade_to_b(&mut ota, &kp, &mut rng);
        assert!(boot_health_check(&mut ota, 4_444));
        // 保存前快照
        let before = PartitionTable::snapshot(&ota);
        save_to_device(&mut ota, &TEST_HMAC_KEY).expect("save 应成功");
        // 破坏内存态（模拟断电后重新上电的脏状态）
        assert!(boot_health_check(&mut ota, 5_000));
        let _ = rollback(&mut ota);
        // 从保留块恢复
        let loaded = load_from_device(&mut ota, &TEST_HMAC_KEY, 6_000).expect("load 应成功");
        assert!(loaded, "应从保留块恢复");
        assert_eq!(ota.get_active_slot(), before.active_slot);
        let after = ota.get_partitions();
        for (a, b) in after.iter().zip(before.partitions.iter()) {
            assert_eq!(a.slot, b.slot);
            assert_eq!(a.state, b.state);
            assert_eq!(a.version.as_str(), b.version.as_str());
            assert_eq!(a.size, b.size);
            assert_eq!(a.hash, b.hash);
            assert_eq!(a.signature, b.signature);
            assert_eq!(a.boot_count, b.boot_count);
            assert_eq!(a.last_boot_ms, b.last_boot_ms);
        }
        assert_eq!(ota.boot_attempts, 0);
    }

    /// PST8（33 SE-18）：伪造分区表——重算 CRC 但无 HMAC → 必拒；
    /// 错误密钥 → 必拒；正确密钥 → 通过。
    #[test]
    fn pst8_forged_table_without_hmac_rejected() {
        let (mut ota, kp, mut rng) = setup();
        upgrade_to_b(&mut ota, &kp, &mut rng);
        let table = PartitionTable::snapshot(&ota);
        let bytes = table.serialize(&TEST_HMAC_KEY);

        // 攻击者模型：无 HMAC 密钥，篡改 active_slot（upgrade_to_b 后为
        // B，改为 A 是真实变更）后重算 CRC 并重写 CRC 区（旧实现只验
        // CRC——此表可通过）。
        let mut forged = bytes;
        forged[4] = encode_slot(PartitionSlot::A); // 篡改 active_slot
        assert_ne!(forged[4], bytes[4], "篡改必须产生真实变更");
        let crc = crc32(&forged[..TABLE_LEN - 4 - TABLE_HMAC_LEN]);
        forged[TABLE_LEN - 4 - TABLE_HMAC_LEN..TABLE_LEN - TABLE_HMAC_LEN]
            .copy_from_slice(&crc.to_be_bytes());
        // 无 HMAC 重算：即使 CRC 正确，HMAC 不匹配 → 拒绝
        assert!(
            PartitionTable::deserialize(&forged, &TEST_HMAC_KEY).is_none(),
            "重算 CRC 无 HMAC 的伪造表必须拒绝"
        );

        // 错误密钥（哪怕表未篡改）→ 拒绝
        let wrong_key = [0x24u8; 32];
        assert!(
            PartitionTable::deserialize(&bytes, &wrong_key).is_none(),
            "错误 HMAC 密钥必须拒绝"
        );

        // 正确密钥 → 通过
        let restored = PartitionTable::deserialize(&bytes, &TEST_HMAC_KEY).expect("正确密钥应通过");
        assert_eq!(restored.active_slot, table.active_slot);

        // 持久化路径：伪造保留块（重算 CRC 无 HMAC）→ load 回退安全默认
        save_to_device(&mut ota, &TEST_HMAC_KEY).expect("save 应成功");
        let bs = ota.device.block_size();
        let mut buf = alloc::vec![0u8; bs];
        ota.device.read_block(META_BLOCK, &mut buf).expect("read");
        // 篡改 active_slot 字节（B→A，真实变更）后重算 CRC（无 HMAC）
        buf[4] = encode_slot(PartitionSlot::A);
        assert_ne!(buf[4], bytes[4], "篡改必须产生真实变更");
        let crc2 = crc32(&buf[..TABLE_LEN - 4 - TABLE_HMAC_LEN]);
        buf[TABLE_LEN - 4 - TABLE_HMAC_LEN..TABLE_LEN - TABLE_HMAC_LEN]
            .copy_from_slice(&crc2.to_be_bytes());
        ota.device.write_block(META_BLOCK, &buf).expect("write");
        let loaded = load_from_device(&mut ota, &TEST_HMAC_KEY, 9_999).expect("load 不应 I/O 失败");
        assert!(!loaded, "伪造表（无 HMAC）应回退安全默认");
        assert_eq!(ota.get_active_slot(), PartitionSlot::A);
        assert_eq!(ota.get_partitions()[0].version.as_str(), "0.0.0");
    }
}
