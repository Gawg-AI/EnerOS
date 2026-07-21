//! 启动验证器（v0.113.0，D4：四级验证逻辑同构收敛于单文件）.
//!
//! 四级验证仅验签密钥来源不同（Bootloader 用信任根，Kernel/Runtime 用
//! stage_key），其余校验流水线完全同构，故收敛为单一 [`BootVerifier`]
//! （蓝图四文件 `rom_verify/bl_verify/kernel_verify/rt_verify.rs` 属重复建设）。
//!
//! 恢复模式（蓝图 §4.4）为平台集成职责——本 crate 仅返回 [`BootError`]，
//! 由集成层据 Err 进入恢复/安全停止（no_std 无平台复位抽象，D11）。

use eneros_crypto::{sm2_verify, sm3_hash, Sm2PublicKey, Sm2Signature};

use crate::chain::{BootStage, ChainOfTrust};
use crate::header::ImageSignature;
use crate::{BootError, BootStats};

/// Secure Boot 四级信任链验证器.
///
/// - 信任根公钥与防降级时间戳下限构造注入（D8：熔丝/安全存储值由集成层供给）
/// - 验证统计 [`BootStats`] 落地 §9 可观测（D11）
pub struct BootVerifier {
    /// 信任链状态（root_key + stage_key + current_stage）.
    chain: ChainOfTrust,
    /// 防降级时间戳下限（构造注入，取自熔丝/安全存储）.
    min_timestamp: u64,
    /// 验证统计（verified_stages / rejected / last_error）.
    stats: BootStats,
}

impl BootVerifier {
    /// 构造验证器（信任根公钥 + 防降级时间戳下限注入）.
    pub fn new(root_key: Sm2PublicKey, min_timestamp: u64) -> Self {
        Self {
            chain: ChainOfTrust::new(root_key),
            min_timestamp,
            stats: BootStats {
                verified_stages: 0,
                rejected: 0,
                last_error: None,
            },
        }
    }

    /// 验证当前启动阶段的镜像签名.
    ///
    /// 严格按序执行（任一失败：`stats.rejected += 1` 并记录 `last_error`）：
    /// 1. `stage != 当前阶段` → `Err(WrongStage)`（强制逐级顺序，D10）
    /// 2. Rom / Complete → `Ok(())`（ROM 由硬件根信任验证，蓝图 §4.5 语义；
    ///    不计 `verified_stages`）
    /// 3. `sig.magic != "ESIG"` → `Err(InvalidMagic)`
    /// 4. `sig.version != 1` → `Err(UnsupportedVersion)`
    /// 5. `image.len() != sig.image_size` → `Err(SizeMismatch)`（防截断镜像）
    /// 6. `sm3_hash(image) != sig.image_hash` → `Err(HashMismatch)`
    /// 7. `sig.timestamp < min_timestamp` → `Err(StaleImage)`（防降级，D8）
    /// 8. 选钥：Bootloader → 信任根；Kernel/Runtime → stage_key
    ///    （None → `Err(MissingStageKey)`，D6）
    /// 9. SM2 验签（签名消息为镜像 SM3 哈希 32B，D9）：编码非法 / 验签
    ///    false / 内部错误 ⇒ `Err(SignatureInvalid)`
    /// 10. 通过：`stats.verified_stages += 1`
    pub fn verify_stage(
        &mut self,
        stage: BootStage,
        image: &[u8],
        sig: &ImageSignature,
    ) -> Result<(), BootError> {
        // 1. 顺序强制（蓝图未检，可跳级验签，D10）
        if stage != self.chain.current_stage() {
            return Err(self.reject(BootError::WrongStage));
        }
        // 2. Rom/Complete 直通（不计 verified_stages）
        if matches!(stage, BootStage::Rom | BootStage::Complete) {
            return Ok(());
        }
        // 3. 头魔数复检（魔数/版本在 decode_header 期已拦截，直接构造的
        //    结构体在此同样复检，spec Requirement 1）
        if sig.magic != *b"ESIG" {
            return Err(self.reject(BootError::InvalidMagic));
        }
        // 4. 帧版本复检
        if sig.version != 1 {
            return Err(self.reject(BootError::UnsupportedVersion));
        }
        // 5. 显式长度校验（蓝图有 image_size 字段未校验，D10）
        if image.len() as u64 != sig.image_size {
            return Err(self.reject(BootError::SizeMismatch));
        }
        // 6. SM3 哈希比对（复用 eneros-crypto，D9）
        if sm3_hash(image) != sig.image_hash {
            return Err(self.reject(BootError::HashMismatch));
        }
        // 7. 防降级时间戳下限（D8）
        if sig.timestamp < self.min_timestamp {
            return Err(self.reject(BootError::StaleImage));
        }
        // 8. 选钥（D6：修复蓝图 bl_pubkey 恒零 bug）
        let stage_key = self.chain.stage_key().copied();
        let key = match stage {
            BootStage::Kernel | BootStage::Runtime => match stage_key {
                Some(k) => k,
                None => return Err(self.reject(BootError::MissingStageKey)),
            },
            // 经步骤 1/2 过滤后，stage 仅剩 Bootloader（Rom/Complete 已直通）
            _ => *self.chain.root_key(),
        };
        // 9. SM2 验签（签名消息为镜像 SM3 哈希，D9）
        let sm2_sig = Sm2Signature::from_bytes(&sig.signature);
        match sm2_verify(&sig.image_hash, &sm2_sig, &key) {
            Ok(true) => {}
            Ok(false) | Err(_) => return Err(self.reject(BootError::SignatureInvalid)),
        }
        // 10. 通过计数
        self.stats.verified_stages += 1;
        Ok(())
    }

    /// 推进信任链至下一启动阶段.
    ///
    /// - Complete → `Err(AlreadyComplete)`（记 stats）
    /// - Bootloader→Kernel：`next_key` 必须为 `Some(bl_pubkey)` 并写入
    ///   stage_key（None → `Err(MissingStageKey)`，记 stats，stage 不变）；
    ///   BL 公钥随已验签镜像体传递，完整性由哈希+签名覆盖传递可信（D6）
    /// - Kernel→Runtime：`next_key` 为 None 沿用当前 stage_key（蓝图
    ///   「同 BL key」语义），Some 则轮换
    /// - Rom→Bootloader：忽略 `next_key`（Some 亦接受但不安装——Bootloader
    ///   级固定使用信任根验签）
    ///
    /// 成功返回推进后的新阶段。
    pub fn advance_stage(
        &mut self,
        next_key: Option<Sm2PublicKey>,
    ) -> Result<BootStage, BootError> {
        let next = match self.chain.current_stage() {
            BootStage::Rom => BootStage::Bootloader,
            BootStage::Bootloader => match next_key {
                Some(k) => {
                    self.chain.set_stage_key(k);
                    BootStage::Kernel
                }
                None => return Err(self.reject(BootError::MissingStageKey)),
            },
            BootStage::Kernel => {
                if let Some(k) = next_key {
                    self.chain.set_stage_key(k);
                }
                BootStage::Runtime
            }
            BootStage::Runtime => BootStage::Complete,
            BootStage::Complete => return Err(self.reject(BootError::AlreadyComplete)),
        };
        self.chain.set_stage(next);
        Ok(next)
    }

    /// 当前启动阶段.
    pub fn current_stage(&self) -> BootStage {
        self.chain.current_stage()
    }

    /// 验证统计快照.
    pub fn stats(&self) -> BootStats {
        self.stats
    }

    /// 统一拒绝出口：计数 + 记录 last_error 后返回错误本身.
    fn reject(&mut self, e: BootError) -> BootError {
        self.stats.rejected += 1;
        self.stats.last_error = Some(e);
        e
    }
}

#[cfg(test)]
mod tests {
    use eneros_crypto::{sm2_sign, CsRng, Sm2KeyPair};

    use super::*;

    /// 用真实 SM2 私钥对镜像签名，组装合法签名头（签名消息为镜像 SM3 哈希）.
    fn sign_image(
        image: &[u8],
        kp: &Sm2KeyPair,
        rng: &mut CsRng,
        timestamp: u64,
    ) -> ImageSignature {
        let hash = sm3_hash(image);
        let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, rng).unwrap();
        ImageSignature {
            magic: *b"ESIG",
            version: 1,
            image_size: image.len() as u64,
            image_hash: hash,
            signature: sig.to_bytes(),
            timestamp,
        }
    }

    // ============================================================
    // VER6~VER14：单级验证器测试
    // ============================================================

    /// VER6 Rom 阶段直接 Ok（硬件根信任语义，蓝图 §4.5），不计 verified_stages.
    #[test]
    fn ver6_rom_stage_passthrough() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        let image = b"ver6-rom-image";
        let sig = sign_image(image, &root, &mut rng, 10);
        assert_eq!(v.verify_stage(BootStage::Rom, image, &sig), Ok(()));
        assert_eq!(v.stats().verified_stages, 0);
        assert_eq!(v.stats().rejected, 0);
    }

    /// VER7 Bootloader 真实签名验过（eneros-crypto Sm2KeyPair 签名 → Ok）.
    #[test]
    fn ver7_bootloader_real_signature_ok() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        let image = b"ver7-bootloader-image";
        let sig = sign_image(image, &root, &mut rng, 10);
        assert_eq!(v.verify_stage(BootStage::Bootloader, image, &sig), Ok(()));
        assert_eq!(v.stats().verified_stages, 1);
    }

    /// VER8 篡改镜像 1 字节 → Err(HashMismatch)，rejected==1，stage 不变.
    #[test]
    fn ver8_tampered_image_hash_mismatch() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        let image = b"ver8-bootloader-image";
        let sig = sign_image(image, &root, &mut rng, 10);
        let mut tampered = *image;
        tampered[0] ^= 0x01;
        assert_eq!(
            v.verify_stage(BootStage::Bootloader, &tampered, &sig),
            Err(BootError::HashMismatch)
        );
        let stats = v.stats();
        assert_eq!(stats.rejected, 1);
        assert_eq!(stats.last_error, Some(BootError::HashMismatch));
        assert_eq!(v.current_stage(), BootStage::Bootloader);
    }

    /// VER9 错私钥签名（非 root 密钥对）→ Err(SignatureInvalid).
    #[test]
    fn ver9_wrong_key_signature_invalid() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let attacker = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        let image = b"ver9-bootloader-image";
        let sig = sign_image(image, &attacker, &mut rng, 10);
        assert_eq!(
            v.verify_stage(BootStage::Bootloader, image, &sig),
            Err(BootError::SignatureInvalid)
        );
    }

    /// VER10 签名字段非法编码（全 0xFF，r/s 越界 [1, n-1]）→ Err(SignatureInvalid).
    #[test]
    fn ver10_malformed_signature_encoding_invalid() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        let image = b"ver10-bootloader-image";
        let mut sig = sign_image(image, &root, &mut rng, 10);
        sig.signature = [0xFF; 64];
        assert_eq!(
            v.verify_stage(BootStage::Bootloader, image, &sig),
            Err(BootError::SignatureInvalid)
        );
    }

    /// VER11 坏 magic / 坏 version / size 不符 →
    /// InvalidMagic / UnsupportedVersion / SizeMismatch（三断言）.
    #[test]
    fn ver11_bad_header_fields_rejected() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        let image = b"ver11-bootloader-image";
        let good = sign_image(image, &root, &mut rng, 10);
        // 坏 magic
        let mut bad_magic = good;
        bad_magic.magic = *b"BSIG";
        assert_eq!(
            v.verify_stage(BootStage::Bootloader, image, &bad_magic),
            Err(BootError::InvalidMagic)
        );
        // 坏 version
        let mut bad_version = good;
        bad_version.version = 2;
        assert_eq!(
            v.verify_stage(BootStage::Bootloader, image, &bad_version),
            Err(BootError::UnsupportedVersion)
        );
        // image_size 不符
        let mut bad_size = good;
        bad_size.image_size = image.len() as u64 + 1;
        assert_eq!(
            v.verify_stage(BootStage::Bootloader, image, &bad_size),
            Err(BootError::SizeMismatch)
        );
    }

    /// VER12 timestamp < min_timestamp → Err(StaleImage)（防降级）.
    #[test]
    fn ver12_stale_image_rejected() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 100);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        let image = b"ver12-bootloader-image";
        let sig = sign_image(image, &root, &mut rng, 50);
        assert_eq!(
            v.verify_stage(BootStage::Bootloader, image, &sig),
            Err(BootError::StaleImage)
        );
    }

    /// VER13 跳级 verify（Rom 阶段直接验 Kernel）→ Err(WrongStage).
    #[test]
    fn ver13_skip_stage_wrong_stage() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        let image = b"ver13-kernel-image";
        let sig = sign_image(image, &bl, &mut rng, 10);
        assert_eq!(
            v.verify_stage(BootStage::Kernel, image, &sig),
            Err(BootError::WrongStage)
        );
        assert_eq!(v.current_stage(), BootStage::Rom);
    }

    /// VER14 缺 stage_key 验 Kernel → Err(MissingStageKey)（spec：直接构造场景）.
    ///
    /// 公开 API 无法到达「stage == Kernel 且 stage_key == None」状态（BL→Kernel
    /// 推进强制携带密钥，D6），故经 crate 内接口直接构造缺钥场景。
    #[test]
    fn ver14_verify_kernel_missing_stage_key() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        v.chain.set_stage(BootStage::Kernel);
        let image = b"ver14-kernel-image";
        let sig = sign_image(image, &bl, &mut rng, 10);
        assert_eq!(
            v.verify_stage(BootStage::Kernel, image, &sig),
            Err(BootError::MissingStageKey)
        );
    }

    // ============================================================
    // CHN15~CHN17：信任链推进测试
    // ============================================================

    /// CHN15 advance 全流程：Rom→Bootloader→Kernel→Runtime→Complete 依次推进.
    #[test]
    fn chn15_advance_full_chain() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        assert_eq!(v.advance_stage(Some(bl.public_key)), Ok(BootStage::Kernel));
        assert_eq!(v.advance_stage(None), Ok(BootStage::Runtime));
        assert_eq!(v.advance_stage(None), Ok(BootStage::Complete));
        assert_eq!(v.current_stage(), BootStage::Complete);
    }

    /// CHN16 Bootloader→Kernel 缺密钥 → Err(MissingStageKey)，stage 不变.
    #[test]
    fn chn16_advance_without_key_rejected() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        assert_eq!(v.advance_stage(None), Err(BootError::MissingStageKey));
        assert_eq!(v.current_stage(), BootStage::Bootloader);
        assert_eq!(v.stats().rejected, 1);
        assert_eq!(v.stats().last_error, Some(BootError::MissingStageKey));
    }

    /// CHN17 Complete 后 advance → Err(AlreadyComplete).
    #[test]
    fn chn17_advance_after_complete_rejected() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        assert_eq!(v.advance_stage(Some(bl.public_key)), Ok(BootStage::Kernel));
        assert_eq!(v.advance_stage(None), Ok(BootStage::Runtime));
        assert_eq!(v.advance_stage(None), Ok(BootStage::Complete));
        assert_eq!(v.advance_stage(None), Err(BootError::AlreadyComplete));
        assert_eq!(v.current_stage(), BootStage::Complete);
    }

    // ============================================================
    // INT18~INT19：全链集成测试
    // ============================================================

    /// INT18 全链快乐路径：root 签 BL、bl 签内核与 Runtime，两级密钥三镜像
    /// 全过 → Complete，verified_stages==3，rejected==0.
    #[test]
    fn int18_full_chain_happy_path() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        let bl_image = b"int18-bootloader-image";
        let kernel_image = b"int18-kernel-image";
        let rt_image = b"int18-runtime-image";
        let bl_sig = sign_image(bl_image, &root, &mut rng, 100);
        let kernel_sig = sign_image(kernel_image, &bl, &mut rng, 101);
        let rt_sig = sign_image(rt_image, &bl, &mut rng, 102);
        // Bootloader 级（信任根验签）
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        assert_eq!(
            v.verify_stage(BootStage::Bootloader, bl_image, &bl_sig),
            Ok(())
        );
        // Kernel 级（安装 BL 公钥为 stage_key）
        assert_eq!(v.advance_stage(Some(bl.public_key)), Ok(BootStage::Kernel));
        assert_eq!(
            v.verify_stage(BootStage::Kernel, kernel_image, &kernel_sig),
            Ok(())
        );
        // Runtime 级（沿用 stage_key）
        assert_eq!(v.advance_stage(None), Ok(BootStage::Runtime));
        assert_eq!(
            v.verify_stage(BootStage::Runtime, rt_image, &rt_sig),
            Ok(())
        );
        assert_eq!(v.advance_stage(None), Ok(BootStage::Complete));
        assert_eq!(v.current_stage(), BootStage::Complete);
        let stats = v.stats();
        assert_eq!(stats.verified_stages, 3);
        assert_eq!(stats.rejected, 0);
        assert_eq!(stats.last_error, None);
    }

    /// INT19 链中途篡改拒绝后重验：Kernel 篡改 → HashMismatch（stage 仍
    /// Kernel）→ 换正确镜像重验 → Ok，可继续推进至 Complete（拒绝不推进
    /// stage，可恢复重试）.
    #[test]
    fn int19_mid_chain_reject_then_retry() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        let bl_image = b"int19-bootloader-image";
        let kernel_image = b"int19-kernel-image";
        let rt_image = b"int19-runtime-image";
        let bl_sig = sign_image(bl_image, &root, &mut rng, 100);
        let kernel_sig = sign_image(kernel_image, &bl, &mut rng, 101);
        let rt_sig = sign_image(rt_image, &bl, &mut rng, 102);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        assert_eq!(
            v.verify_stage(BootStage::Bootloader, bl_image, &bl_sig),
            Ok(())
        );
        assert_eq!(v.advance_stage(Some(bl.public_key)), Ok(BootStage::Kernel));
        // Kernel 阶段先投篡改镜像 → HashMismatch，stage 仍 Kernel
        let mut tampered_kernel = *kernel_image;
        tampered_kernel[0] ^= 0x01;
        assert_eq!(
            v.verify_stage(BootStage::Kernel, &tampered_kernel, &kernel_sig),
            Err(BootError::HashMismatch)
        );
        assert_eq!(v.current_stage(), BootStage::Kernel);
        // 换正确镜像重验 → Ok，可继续推进至 Complete
        assert_eq!(
            v.verify_stage(BootStage::Kernel, kernel_image, &kernel_sig),
            Ok(())
        );
        assert_eq!(v.advance_stage(None), Ok(BootStage::Runtime));
        assert_eq!(
            v.verify_stage(BootStage::Runtime, rt_image, &rt_sig),
            Ok(())
        );
        assert_eq!(v.advance_stage(None), Ok(BootStage::Complete));
        let stats = v.stats();
        assert_eq!(stats.verified_stages, 3);
        assert_eq!(stats.rejected, 1);
        assert_eq!(stats.last_error, Some(BootError::HashMismatch));
    }

    // ============================================================
    // PERF20：验签性能（蓝图 §6.3/§7.2，cfg(test) Instant 口径，
    // 同 v0.111.0 PERF26 先例）
    // ============================================================

    /// PERF20 单次 verify_stage(Bootloader) 真实 SM2 验签计时：
    /// debug 仅打印；release 默认打印，设 `ENEROS_PERF_GATE=1` 时断言 < 50ms（D13：
    /// 主机纯 Rust 仿射 SM2 实测 ~150ms 超蓝图 §7.2 指标，50ms 门禁面向目标硬件
    /// SM2 加速 / 性能 CI 场景）。
    #[test]
    fn perf20_verify_stage_under_50ms() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut v = BootVerifier::new(root.public_key, 0);
        assert_eq!(v.advance_stage(None), Ok(BootStage::Bootloader));
        let image = b"perf20-bootloader-image";
        let sig = sign_image(image, &root, &mut rng, 10);
        let start = std::time::Instant::now();
        let result = v.verify_stage(BootStage::Bootloader, image, &sig);
        let elapsed = start.elapsed();
        assert_eq!(result, Ok(()));
        #[cfg(debug_assertions)]
        eprintln!(
            "[PERF20] 单次 verify_stage(Bootloader) 真实 SM2 验签耗时: {:?}",
            elapsed
        );
        #[cfg(not(debug_assertions))]
        {
            // 仅当变量值恰为 "1" 时启用门禁：规避终端残留空串变量误激活（D13）
            if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
                assert!(
                    elapsed.as_millis() < 50,
                    "PERF20 验签耗时 {:?} 超过 50ms 上限",
                    elapsed
                );
            } else {
                eprintln!(
                    "[PERF20] 单次 verify_stage(Bootloader) 真实 SM2 验签耗时: {:?}（设 ENEROS_PERF_GATE=1 启用 <50ms 断言，D13）",
                    elapsed
                );
            }
        }
    }
}
