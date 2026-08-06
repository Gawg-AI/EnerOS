//! FlashWriter 固件刷写（Task 3）：流式接收 + 真实 SM3/SM2 验签 + 真实写盘。
//!
//! - `receive_chunk` 流式累积镜像字节并增量更新 SM3 哈希（D3：恢复环境无文件
//!   系统假设，字节流经串口/网络喂入）；
//! - `finish_and_verify` SM3 哈希常时比对 + SM2 根公钥验签（D4：真实验签，
//!   与 ota-ab `ab_partition.rs` 同口径——对 SM3 哈希验签）；
//! - `write_to_device` 按块写入后端 [`BlockDevice`]，末块零填充整块，写失败
//!   返回 [`RecoveryError::FlashFailed`]。
//!
//! 安全不变量：验签未通过前禁止写盘（`write_to_device` 直接返回
//! [`RecoveryError::SignatureInvalid`]，不触碰设备）。

use alloc::vec;
use alloc::vec::Vec;
use core::mem;

use eneros_crypto::{ct_eq, sm2_verify, Sm2PublicKey, Sm2Signature, Sm3Hasher};
use eneros_storage::BlockDevice;

use crate::RecoveryError;

/// 固件刷写器：流式接收镜像、SM3 增量哈希、SM2 验签、分块写盘。
///
/// 泛型 `D` 为后端块设备（生产为 eMMC/NVMe 驱动，测试为 `MockBlockDevice`）。
pub struct FlashWriter<D: BlockDevice> {
    /// 后端块设备
    device: D,
    /// 设备根公钥（SM2 验签，D4）
    root_key: Sm2PublicKey,
    /// 镜像累积缓冲（流式接收的全部字节）
    buffer: Vec<u8>,
    /// SM3 增量哈希状态
    hasher: Sm3Hasher,
    /// 已接收总字节数
    total_bytes: u64,
    /// 验签通过标记（哈希比对 + 签名验证双过才置 true）
    verified: bool,
    /// `finish_and_verify` 计算出的 SM3 哈希（finalize 消耗 hasher，存此备用）
    computed_hash: Option<[u8; 32]>,
}

impl<D: BlockDevice> FlashWriter<D> {
    /// 创建刷写器：注入后端块设备与设备根公钥。
    pub fn new(device: D, root_key: Sm2PublicKey) -> Self {
        Self {
            device,
            root_key,
            buffer: Vec::new(),
            hasher: Sm3Hasher::new(),
            total_bytes: 0,
            verified: false,
            computed_hash: None,
        }
    }

    /// 流式接收一块镜像数据：累积缓冲 + 增量哈希 + 累计字节数（D3）。
    pub fn receive_chunk(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
        self.hasher.update(data);
        self.total_bytes += data.len() as u64;
    }

    /// 复位刷写状态：清空缓冲/计数/验签标记，重置哈希状态（可重新接收）。
    pub fn reset(&mut self) {
        self.buffer.clear();
        self.hasher = Sm3Hasher::new();
        self.total_bytes = 0;
        self.verified = false;
        self.computed_hash = None;
    }

    /// 已接收总字节数。
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// 当前缓冲字节数。
    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    /// 验签是否已通过。
    pub fn is_verified(&self) -> bool {
        self.verified
    }

    /// 后端块设备只读引用（测试回读校验用）。
    pub fn device(&self) -> &D {
        &self.device
    }

    /// 完成接收并验签：SM3 哈希常时比对 + SM2 根公钥验签（D4）。
    ///
    /// `expected_hash` 为镜像 SM3 哈希；`signature` 为对该哈希的 SM2 签名
    /// （r ‖ s，64 字节）。哈希不符或签名无效均返回
    /// [`RecoveryError::SignatureInvalid`]，不触碰设备。
    pub fn finish_and_verify(
        &mut self,
        expected_hash: &[u8; 32],
        signature: &[u8; 64],
    ) -> Result<(), RecoveryError> {
        // Sm3Hasher::finalize 消耗自身且未实现 Clone：换出旧状态后 finalize，
        // 槽位重置为新状态（接收已完毕，如需再刷应调 reset）。
        let hasher = mem::replace(&mut self.hasher, Sm3Hasher::new());
        let computed = hasher.finalize();
        self.computed_hash = Some(computed);
        // 1) SM3 哈希常时比对
        if !ct_eq(&computed, expected_hash) {
            self.verified = false;
            return Err(RecoveryError::SignatureInvalid);
        }
        // 2) SM2 根公钥验签（对 SM3 哈希验签，与 ota-ab ab_partition.rs 同口径）
        let sig = Sm2Signature::from_bytes(signature);
        let valid = sm2_verify(&computed, &sig, &self.root_key).unwrap_or(false);
        if !valid {
            self.verified = false;
            return Err(RecoveryError::SignatureInvalid);
        }
        self.verified = true;
        Ok(())
    }

    /// 将已验签镜像分块写入设备：`slot_base` 为起始块地址（块索引）。
    ///
    /// 按 `block_size()` 分块写入，末块不足整块零填充；任一块写失败返回
    /// [`RecoveryError::FlashFailed`]（offset 为失败块的起始字节偏移）；写完
    /// flush。验签未通过直接返回 [`RecoveryError::SignatureInvalid`]，不触碰
    /// 设备。成功返回已写字节数（不含末块填充）。
    pub fn write_to_device(&mut self, slot_base: u64) -> Result<u64, RecoveryError> {
        if !self.verified {
            return Err(RecoveryError::SignatureInvalid);
        }
        let bs = self.device.block_size();
        let total = self.buffer.len();
        let n_blocks = total.div_ceil(bs);
        let mut block_buf = vec![0u8; bs];
        for i in 0..n_blocks {
            let start = i * bs;
            let end = core::cmp::min(start + bs, total);
            let chunk_len = end - start;
            block_buf[..chunk_len].copy_from_slice(&self.buffer[start..end]);
            // 末块剩余区域零填充（清除复用缓冲中上一块的残留数据）
            for b in &mut block_buf[chunk_len..] {
                *b = 0;
            }
            let block_idx = slot_base + i as u64;
            if self.device.write_block(block_idx, &block_buf).is_err() {
                return Err(RecoveryError::FlashFailed {
                    offset: start as u64,
                });
            }
        }
        self.device
            .flush()
            .map_err(|_| RecoveryError::StorageError)?;
        Ok(self.total_bytes)
    }
}

#[cfg(test)]
mod tests {
    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
    use eneros_storage::{DeviceHealth, MockBlockDevice, StorageError};

    use super::*;

    /// 构造伪固件镜像：确定性递增字节（模 251 避开块对齐周期）。
    fn fake_firmware(len: usize) -> Vec<u8> {
        (0..len as u32).map(|i| (i % 251) as u8).collect()
    }

    /// 构造测试环境：真实 SM2 密钥对 + 指定几何的 MockBlockDevice。
    fn setup(
        block_count: u64,
        block_size: usize,
    ) -> (FlashWriter<MockBlockDevice>, Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
        let writer = FlashWriter::new(MockBlockDevice::new(block_count, block_size), kp.public_key);
        (writer, kp, rng)
    }

    /// 用根私钥对数据 SM3 哈希签名（与 finish_and_verify 验签口径一致）。
    fn sign_firmware(data: &[u8], kp: &Sm2KeyPair, rng: &mut CsRng) -> ([u8; 32], [u8; 64]) {
        let hash = sm3_hash(data);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng).expect("签名失败");
        (hash, sig.to_bytes())
    }

    /// 写失败注入 mock：写第 `fail_at` 块时返回 BadBlock，其余委托内部
    /// MockBlockDevice（mock 自身的 mark_bad 会重定向写而非失败，故自包一层）。
    struct FailAtBlockMock {
        /// 内部 RAM 后端
        inner: MockBlockDevice,
        /// 注入写失败的块地址
        fail_at: u64,
    }

    impl FailAtBlockMock {
        /// 创建失败注入 mock。
        fn new(block_count: u64, block_size: usize, fail_at: u64) -> Self {
            Self {
                inner: MockBlockDevice::new(block_count, block_size),
                fail_at,
            }
        }
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

    /// FLS1：分块接收的增量哈希与一次性 SM3 哈希一致。
    #[test]
    fn fls1_chunked_receive_hash_consistent() {
        let (mut writer, kp, mut rng) = setup(64, 512);
        let image = fake_firmware(10000);
        // 非对齐 7 块切分，覆盖跨块/跨 SM3 分组边界
        let cuts = [7usize, 100, 512, 1023, 2000, 3000];
        let mut offset = 0;
        for &c in &cuts {
            writer.receive_chunk(&image[offset..offset + c]);
            offset += c;
        }
        writer.receive_chunk(&image[offset..]);
        assert_eq!(writer.total_bytes(), 10000);
        assert_eq!(writer.buffered_len(), 10000);

        let (hash, sig) = sign_firmware(&image, &kp, &mut rng);
        writer.finish_and_verify(&hash, &sig).expect("验签应通过");
        // 增量哈希 == 一次性哈希（finalize 结果存于 computed_hash 字段）
        assert_eq!(writer.computed_hash, Some(sm3_hash(&image)));
    }

    /// FLS2：期望哈希不符 → SignatureInvalid，verified 保持 false。
    #[test]
    fn fls2_hash_mismatch_rejected() {
        let (mut writer, kp, mut rng) = setup(64, 512);
        let image = fake_firmware(10000);
        writer.receive_chunk(&image);
        let (mut wrong_hash, sig) = sign_firmware(&image, &kp, &mut rng);
        wrong_hash[0] ^= 0xFF; // 故意错 1 字节
        let err = writer.finish_and_verify(&wrong_hash, &sig).unwrap_err();
        assert_eq!(err, RecoveryError::SignatureInvalid);
        assert!(!writer.is_verified());
    }

    /// FLS3：根私钥签发的合法签名验签通过。
    #[test]
    fn fls3_valid_signature_accepted() {
        let (mut writer, kp, mut rng) = setup(64, 512);
        let image = fake_firmware(4096);
        writer.receive_chunk(&image);
        let (hash, sig) = sign_firmware(&image, &kp, &mut rng);
        writer
            .finish_and_verify(&hash, &sig)
            .expect("合法签名应通过");
        assert!(writer.is_verified());
    }

    /// FLS4：非根私钥签名 → 验签拒绝，随后写盘仍拒绝且设备零写入。
    #[test]
    fn fls4_foreign_signature_rejected_no_write() {
        let (mut writer, _kp, mut rng) = setup(32, 512);
        let image = fake_firmware(2048);
        writer.receive_chunk(&image);
        // 另一密钥对签名（rng 流已推进，密钥材料不同于 root_key 对应密钥对）
        let other = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
        let hash = sm3_hash(&image);
        let sig = sm2_sign(&hash, &other.private_key, &other.public_key, &mut rng)
            .expect("签名失败")
            .to_bytes();
        let err = writer.finish_and_verify(&hash, &sig).unwrap_err();
        assert_eq!(err, RecoveryError::SignatureInvalid);
        // 未验签禁止写盘
        let err = writer.write_to_device(10).unwrap_err();
        assert_eq!(err, RecoveryError::SignatureInvalid);
        // 设备内容全零未变
        let mut blk = [0u8; 512];
        for i in 0..writer.device().block_count() {
            writer.device().read_block(i, &mut blk).expect("读块失败");
            assert!(blk.iter().all(|&b| b == 0), "块 {i} 不应被写入");
        }
    }

    /// FLS5：验签写盘后回读逐字节一致，末块填充区为 0。
    #[test]
    fn fls5_write_readback_byte_identical() {
        let (mut writer, kp, mut rng) = setup(64, 512);
        let image = fake_firmware(10000);
        // 分 3 块接收，模拟流式喂入
        writer.receive_chunk(&image[..3000]);
        writer.receive_chunk(&image[3000..7777]);
        writer.receive_chunk(&image[7777..]);
        let (hash, sig) = sign_firmware(&image, &kp, &mut rng);
        writer.finish_and_verify(&hash, &sig).expect("验签应通过");

        let slot_base = 10u64;
        let written = writer.write_to_device(slot_base).expect("写盘应成功");
        assert_eq!(written, 10000);

        // 从 slot_base 起逐块回读拼接，前 total_bytes 字节与原镜像完全相等
        let bs = writer.device().block_size();
        let n_blocks = 10000usize.div_ceil(bs) as u64;
        let mut readback = Vec::new();
        let mut blk = vec![0u8; bs];
        for i in 0..n_blocks {
            writer
                .device()
                .read_block(slot_base + i, &mut blk)
                .expect("读块失败");
            readback.extend_from_slice(&blk);
        }
        assert_eq!(&readback[..10000], &image[..]);
        // 末块填充区为 0
        assert!(readback[10000..].iter().all(|&b| b == 0));
    }

    /// FLS6：写失败注入 → FlashFailed{offset}（offset 为失败块起始字节偏移）。
    #[test]
    fn fls6_write_failure_reports_offset() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).expect("密钥对生成失败");
        // 写 slot_base(10)+2 块时注入失败
        let dev = FailAtBlockMock::new(32, 512, 12);
        let mut writer = FlashWriter::new(dev, kp.public_key);
        let image = fake_firmware(1500); // 3 块：512 + 512 + 476
        writer.receive_chunk(&image);
        let (hash, sig) = sign_firmware(&image, &kp, &mut rng);
        writer.finish_and_verify(&hash, &sig).expect("验签应通过");
        let err = writer.write_to_device(10).unwrap_err();
        assert_eq!(err, RecoveryError::FlashFailed { offset: 2 * 512 });
    }

    /// FLS7：reset 清空状态并可重新接收（哈希不串扰）。
    #[test]
    fn fls7_reset_clears_state() {
        let (mut writer, kp, mut rng) = setup(64, 512);
        let image_a = fake_firmware(2048);
        writer.receive_chunk(&image_a);
        let (hash_a, sig_a) = sign_firmware(&image_a, &kp, &mut rng);
        writer
            .finish_and_verify(&hash_a, &sig_a)
            .expect("验签应通过");

        writer.reset();
        assert_eq!(writer.total_bytes(), 0);
        assert_eq!(writer.buffered_len(), 0);
        assert!(!writer.is_verified());
        assert_eq!(writer.computed_hash, None);

        // 重新接收不同数据：哈希仅覆盖新数据
        let image_b: Vec<u8> = (0..3000u32).map(|i| (i % 97) as u8).collect();
        writer.receive_chunk(&image_b);
        let (hash_b, sig_b) = sign_firmware(&image_b, &kp, &mut rng);
        writer
            .finish_and_verify(&hash_b, &sig_b)
            .expect("复位后验签应通过");
        assert_eq!(writer.computed_hash, Some(sm3_hash(&image_b)));
        assert!(writer.is_verified());
    }
}
