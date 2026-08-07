//! Secure Boot 核心实现 (workspace v0.192.0).
//!
//! 提供 `SecureBoot` trait 和 `EnerosSecureBoot` 实现，
//! 集成 BootVerifier（四级验签）+ TPM PCR 度量。

use eneros_crypto::{sm3_hash, Sm2PublicKey};
use eneros_tpm::TpmDriver;

use crate::chain::BootStage;
use crate::header::ImageSignature;
use crate::report::BootChainReport;
use crate::root_of_trust::RootOfTrust;
use crate::verifier::BootVerifier;
use crate::BootError;

// ============================================================
// Boot Images Bundle
// ============================================================

/// 启动镜像集合.
///
/// 封装全链验证所需的所有镜像和签名。
pub struct BootImages<'a> {
    /// Bootloader 镜像
    pub bootloader: &'a [u8],
    /// Bootloader 签名
    pub bl_sig: &'a ImageSignature,
    /// 内核镜像
    pub kernel: &'a [u8],
    /// 内核签名
    pub kernel_sig: &'a ImageSignature,
    /// 运行时镜像
    pub runtime: &'a [u8],
    /// 运行时签名
    pub rt_sig: &'a ImageSignature,
}

// ============================================================
// SecureBoot Trait
// ============================================================

/// Secure Boot 验证接口.
///
/// 定义安全启动的核心操作：全链验证、镜像验签、信任根获取。
pub trait SecureBoot {
    /// 验证完整启动链.
    ///
    /// 逐级验证 Bootloader → Kernel → Runtime，
    /// 每级验签后扩展 TPM PCR（如可用）。
    fn verify_boot_chain(
        &mut self,
        images: &BootImages<'_>,
        manifest: &crate::manifest::BootManifest,
    ) -> Result<BootChainReport, BootError>;

    /// 验证单个镜像签名.
    fn verify_image(
        &mut self,
        image: &[u8],
        sig: &ImageSignature,
        public_key: &Sm2PublicKey,
    ) -> Result<bool, BootError>;

    /// 获取信任根.
    fn get_root_of_trust(&self) -> &RootOfTrust;
}

// ============================================================
// EnerosSecureBoot
// ============================================================

/// EnerOS Secure Boot 实现.
///
/// 集成四级验签（BootVerifier）+ TPM PCR 度量。
/// TPM 不可用时降级运行（pcr_extended = false，不阻断启动）。
pub struct EnerosSecureBoot<'a> {
    /// 四级验签器
    verifier: BootVerifier,
    /// 信任根
    root_of_trust: RootOfTrust,
    /// TPM 驱动（可选，不可用时为 None）
    tpm: Option<&'a mut dyn TpmDriver>,
    /// PCR 扩展是否成功
    pcr_extended: bool,
}

impl<'a> EnerosSecureBoot<'a> {
    /// 创建 Secure Boot 实例（含 TPM）.
    pub fn new(root_of_trust: RootOfTrust, min_timestamp: u64, tpm: &'a mut dyn TpmDriver) -> Self {
        let verifier = BootVerifier::new(root_of_trust.root_key, min_timestamp);
        Self {
            verifier,
            root_of_trust,
            tpm: Some(tpm),
            pcr_extended: false,
        }
    }

    /// 创建 Secure Boot 实例（无 TPM，降级模式）.
    pub fn new_without_tpm(root_of_trust: RootOfTrust, min_timestamp: u64) -> Self {
        let verifier = BootVerifier::new(root_of_trust.root_key, min_timestamp);
        Self {
            verifier,
            root_of_trust,
            tpm: None,
            pcr_extended: false,
        }
    }

    /// 获取验证器统计.
    pub fn stats(&self) -> crate::BootStats {
        self.verifier.stats()
    }

    /// 获取当前启动阶段.
    pub fn current_stage(&self) -> BootStage {
        self.verifier.current_stage()
    }

    /// 尝试扩展 TPM PCR（TPM 不可用时静默跳过）.
    fn try_pcr_extend(&mut self, pcr_idx: u32, data: &[u8]) {
        if let Some(tpm) = self.tpm.as_mut() {
            let digest = sm3_hash(data);
            if tpm.pcr_extend(pcr_idx, &digest).is_ok() {
                self.pcr_extended = true;
            }
        }
    }
}

impl SecureBoot for EnerosSecureBoot<'_> {
    fn verify_boot_chain(
        &mut self,
        images: &BootImages<'_>,
        manifest: &crate::manifest::BootManifest,
    ) -> Result<BootChainReport, BootError> {
        let mut report = BootChainReport::new();

        // 验证信任根
        report.root_of_trust_valid = self.root_of_trust.is_valid();
        if !report.root_of_trust_valid {
            return Err(BootError::SignatureInvalid);
        }

        // 阶段 1: Rom → Bootloader
        self.verifier
            .advance_stage(None)
            .map_err(|_| BootError::WrongStage)?;
        self.verifier
            .verify_stage(BootStage::Bootloader, images.bootloader, images.bl_sig)?;
        report.bootloader_verified = true;
        report.stages_verified += 1;
        self.try_pcr_extend(0, images.bootloader);

        // 阶段 2: Bootloader → Kernel（BL 公钥只来自已验证的 manifest，
        // 蓝图/20 R2-6：类型级绑定，BootImages 不再携带独立 key）
        self.verifier
            .advance_stage(Some(manifest.bl_key))
            .map_err(|_| BootError::MissingStageKey)?;
        self.verifier
            .verify_stage(BootStage::Kernel, images.kernel, images.kernel_sig)?;
        report.kernel_verified = true;
        report.stages_verified += 1;
        self.try_pcr_extend(4, images.kernel);

        // 阶段 3: Kernel → Runtime
        self.verifier
            .advance_stage(None)
            .map_err(|_| BootError::WrongStage)?;
        self.verifier
            .verify_stage(BootStage::Runtime, images.runtime, images.rt_sig)?;
        report.runtime_verified = true;
        report.stages_verified += 1;
        self.try_pcr_extend(2, images.runtime);

        // 推进到 Complete
        self.verifier
            .advance_stage(None)
            .map_err(|_| BootError::AlreadyComplete)?;

        report.pcr_extended = self.pcr_extended;
        Ok(report)
    }

    fn verify_image(
        &mut self,
        image: &[u8],
        sig: &ImageSignature,
        _public_key: &Sm2PublicKey,
    ) -> Result<bool, BootError> {
        // 使用当前阶段的验证逻辑
        let stage = self.verifier.current_stage();
        match self.verifier.verify_stage(stage, image, sig) {
            Ok(()) => Ok(true),
            Err(e) => Err(e),
        }
    }

    fn get_root_of_trust(&self) -> &RootOfTrust {
        &self.root_of_trust
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
    use eneros_tpm::SimulatedTpm;

    use super::*;

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

    #[test]
    fn test_secure_boot_with_tpm() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let rot = RootOfTrust::new(root.public_key, [0u8; 32]);

        let mut tpm = SimulatedTpm::new();
        tpm.init().unwrap();

        let mut sb = EnerosSecureBoot::new(rot, 0, &mut tpm);

        let bl_image = b"bootloader image data";
        let kernel_image = b"kernel image data";
        let rt_image = b"runtime image data";

        let bl_sig = sign_image(bl_image, &root, &mut rng, 100);
        let kernel_sig = sign_image(kernel_image, &bl, &mut rng, 101);
        let rt_sig = sign_image(rt_image, &bl, &mut rng, 102);

        let images = BootImages {
            bootloader: bl_image,
            bl_sig: &bl_sig,
            kernel: kernel_image,
            kernel_sig: &kernel_sig,
            runtime: rt_image,
            rt_sig: &rt_sig,
        };
        let (raw, raw_len) = crate::manifest::BootManifest::encode(&bl.public_key);
        let manifest = crate::manifest::BootManifest::parse(&raw[..raw_len]).unwrap();

        let report = sb.verify_boot_chain(&images, &manifest).unwrap();

        assert!(report.all_verified());
        assert!(report.can_boot());
        assert!(report.pcr_extended);
        assert_eq!(report.stages_verified, 3);
        assert!(report.root_of_trust_valid);
    }

    #[test]
    fn test_secure_boot_without_tpm() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let rot = RootOfTrust::new(root.public_key, [0u8; 32]);

        let mut sb = EnerosSecureBoot::new_without_tpm(rot, 0);

        let bl_image = b"bootloader";
        let kernel_image = b"kernel";
        let rt_image = b"runtime";

        let bl_sig = sign_image(bl_image, &root, &mut rng, 100);
        let kernel_sig = sign_image(kernel_image, &bl, &mut rng, 101);
        let rt_sig = sign_image(rt_image, &bl, &mut rng, 102);

        let images = BootImages {
            bootloader: bl_image,
            bl_sig: &bl_sig,
            kernel: kernel_image,
            kernel_sig: &kernel_sig,
            runtime: rt_image,
            rt_sig: &rt_sig,
        };
        let (raw, raw_len) = crate::manifest::BootManifest::encode(&bl.public_key);
        let manifest = crate::manifest::BootManifest::parse(&raw[..raw_len]).unwrap();

        let report = sb.verify_boot_chain(&images, &manifest).unwrap();

        assert!(report.all_verified());
        assert!(report.can_boot());
        assert!(!report.pcr_extended); // TPM 不可用
    }

    #[test]
    fn test_secure_boot_tampered_bootloader() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let rot = RootOfTrust::new(root.public_key, [0u8; 32]);

        let mut sb = EnerosSecureBoot::new_without_tpm(rot, 0);

        let bl_image = b"bootloader";
        let bl_sig = sign_image(bl_image, &root, &mut rng, 100);

        // 篡改 Bootloader
        let mut tampered = *bl_image;
        tampered[0] ^= 0xFF;

        let kernel_image = b"kernel";
        let rt_image = b"runtime";
        let kernel_sig = sign_image(kernel_image, &bl, &mut rng, 101);
        let rt_sig = sign_image(rt_image, &bl, &mut rng, 102);

        let images = BootImages {
            bootloader: &tampered,
            bl_sig: &bl_sig,
            kernel: kernel_image,
            kernel_sig: &kernel_sig,
            runtime: rt_image,
            rt_sig: &rt_sig,
        };

        let (raw_m, raw_m_len) = crate::manifest::BootManifest::encode(&bl.public_key);
        let manifest = crate::manifest::BootManifest::parse(&raw_m[..raw_m_len]).unwrap();
        let result = sb.verify_boot_chain(&images, &manifest);
        assert_eq!(result, Err(BootError::HashMismatch));
    }

    #[test]
    fn test_secure_boot_forged_signature() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let attacker = Sm2KeyPair::generate(&mut rng).unwrap();
        let bl = Sm2KeyPair::generate(&mut rng).unwrap();
        let rot = RootOfTrust::new(root.public_key, [0u8; 32]);

        let mut sb = EnerosSecureBoot::new_without_tpm(rot, 0);

        let bl_image = b"bootloader";
        // 攻击者签名（非信任根）
        let bl_sig = sign_image(bl_image, &attacker, &mut rng, 100);

        let kernel_image = b"kernel";
        let rt_image = b"runtime";
        let kernel_sig = sign_image(kernel_image, &bl, &mut rng, 101);
        let rt_sig = sign_image(rt_image, &bl, &mut rng, 102);

        let images = BootImages {
            bootloader: bl_image,
            bl_sig: &bl_sig,
            kernel: kernel_image,
            kernel_sig: &kernel_sig,
            runtime: rt_image,
            rt_sig: &rt_sig,
        };

        let (raw_m, raw_m_len) = crate::manifest::BootManifest::encode(&bl.public_key);
        let manifest = crate::manifest::BootManifest::parse(&raw_m[..raw_m_len]).unwrap();
        let result = sb.verify_boot_chain(&images, &manifest);
        assert_eq!(result, Err(BootError::SignatureInvalid));
    }

    #[test]
    fn test_get_root_of_trust() {
        let mut rng = CsRng::new();
        let root = Sm2KeyPair::generate(&mut rng).unwrap();
        let rot = RootOfTrust::new(root.public_key, [0xBB; 32]);
        let sb = EnerosSecureBoot::new_without_tpm(rot, 0);

        assert_eq!(sb.get_root_of_trust().tpm_pcr_anchor, [0xBB; 32]);
        assert!(sb.get_root_of_trust().is_valid());
    }
}
