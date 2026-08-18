//! Secure Boot v0.160.0 集成测试.
//!
//! 全链验证 + TPM PCR 度量 + 篡改拒绝 + 降级模式。

use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};
use eneros_secure_boot::manifest::BootManifest;
use eneros_secure_boot::{
    BootError, BootImages, EnerosSecureBoot, ImageSignature, RootOfTrust, SecureBoot,
};
use eneros_tpm::{SimulatedTpm, TpmDriver};

fn sign_image(image: &[u8], kp: &Sm2KeyPair, rng: &mut CsRng, timestamp: u64) -> ImageSignature {
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
// 全链验证快乐路径
// ============================================================

#[test]
fn test_full_chain_with_tpm() {
    let mut rng = CsRng::new();
    let root = Sm2KeyPair::generate(&mut rng).unwrap();
    let bl = Sm2KeyPair::generate(&mut rng).unwrap();
    let rot = RootOfTrust::new(root.public_key, [0u8; 32]);

    let mut tpm = SimulatedTpm::new();
    tpm.init().unwrap();

    let mut sb = EnerosSecureBoot::new(rot, 0, &mut tpm);

    let bl_img = b"EnerOS Bootloader v2.0";
    let kern_img = b"EnerOS Kernel v2.0";
    let rt_img = b"EnerOS Runtime v2.0";

    let bl_sig = sign_image(bl_img, &root, &mut rng, 1000);
    let kern_sig = sign_image(kern_img, &bl, &mut rng, 1001);
    let rt_sig = sign_image(rt_img, &bl, &mut rng, 1002);

    let images = BootImages {
        bootloader: bl_img,
        bl_sig: &bl_sig,
        kernel: kern_img,
        kernel_sig: &kern_sig,
        runtime: rt_img,
        rt_sig: &rt_sig,
    };
    let (raw, raw_len) = BootManifest::encode(&bl.public_key);
    let mut bl_img_with_manifest = vec![0x51u8; 64];
    bl_img_with_manifest.extend_from_slice(&raw[..raw_len]);
    let manifest_sig = sign_image(&bl_img_with_manifest, &root, &mut rng, 100);
    // 29 SE-02：manifest 必须经 verify_and_parse 验签解析（未验签 parse
    // 暴露下级公钥绑定断裂，B4-02 失效）。
    let manifest =
        BootManifest::verify_and_parse(&bl_img_with_manifest, &root.public_key, &manifest_sig, 0)
            .unwrap();

    let report = sb.verify_boot_chain(&images, &manifest).unwrap();

    assert!(report.bootloader_verified);
    assert!(report.kernel_verified);
    assert!(report.runtime_verified);
    assert!(report.pcr_extended);
    assert!(report.can_boot());
    assert_eq!(report.stages_verified, 3);
}

// ============================================================
// 篡改测试（故障注入）
// ============================================================

#[test]
fn test_tamper_bootloader_rejected() {
    let mut rng = CsRng::new();
    let root = Sm2KeyPair::generate(&mut rng).unwrap();
    let bl = Sm2KeyPair::generate(&mut rng).unwrap();
    let rot = RootOfTrust::new(root.public_key, [0u8; 32]);
    let mut sb = EnerosSecureBoot::new_without_tpm(rot, 0);

    let bl_img = b"trusted bootloader";
    let bl_sig = sign_image(bl_img, &root, &mut rng, 100);
    let mut tampered = *bl_img;
    tampered[0] ^= 0xFF;

    let kern_img = b"kernel";
    let rt_img = b"runtime";
    let kern_sig = sign_image(kern_img, &bl, &mut rng, 101);
    let rt_sig = sign_image(rt_img, &bl, &mut rng, 102);

    let images = BootImages {
        bootloader: &tampered,
        bl_sig: &bl_sig,
        kernel: kern_img,
        kernel_sig: &kern_sig,
        runtime: rt_img,
        rt_sig: &rt_sig,
    };

    let (raw, raw_len) = BootManifest::encode(&bl.public_key);
    let mut bl_img_with_manifest = vec![0x51u8; 64];
    bl_img_with_manifest.extend_from_slice(&raw[..raw_len]);
    let manifest_sig = sign_image(&bl_img_with_manifest, &root, &mut rng, 100);
    // 29 SE-02：manifest 必须经 verify_and_parse 验签解析（未验签 parse
    // 暴露下级公钥绑定断裂，B4-02 失效）。
    let manifest =
        BootManifest::verify_and_parse(&bl_img_with_manifest, &root.public_key, &manifest_sig, 0)
            .unwrap();
    let result = sb.verify_boot_chain(&images, &manifest);
    assert_eq!(result, Err(BootError::HashMismatch));
}

#[test]
fn test_tamper_kernel_rejected() {
    let mut rng = CsRng::new();
    let root = Sm2KeyPair::generate(&mut rng).unwrap();
    let bl = Sm2KeyPair::generate(&mut rng).unwrap();
    let rot = RootOfTrust::new(root.public_key, [0u8; 32]);
    let mut sb = EnerosSecureBoot::new_without_tpm(rot, 0);

    let bl_img = b"bootloader";
    let kern_img = b"kernel image";
    let rt_img = b"runtime";

    let bl_sig = sign_image(bl_img, &root, &mut rng, 100);
    let kern_sig = sign_image(kern_img, &bl, &mut rng, 101);
    let rt_sig = sign_image(rt_img, &bl, &mut rng, 102);

    let mut tampered_kern = *kern_img;
    tampered_kern[3] ^= 0x01;

    let images = BootImages {
        bootloader: bl_img,
        bl_sig: &bl_sig,
        kernel: &tampered_kern,
        kernel_sig: &kern_sig,
        runtime: rt_img,
        rt_sig: &rt_sig,
    };

    let (raw, raw_len) = BootManifest::encode(&bl.public_key);
    let mut bl_img_with_manifest = vec![0x51u8; 64];
    bl_img_with_manifest.extend_from_slice(&raw[..raw_len]);
    let manifest_sig = sign_image(&bl_img_with_manifest, &root, &mut rng, 100);
    // 29 SE-02：manifest 必须经 verify_and_parse 验签解析（未验签 parse
    // 暴露下级公钥绑定断裂，B4-02 失效）。
    let manifest =
        BootManifest::verify_and_parse(&bl_img_with_manifest, &root.public_key, &manifest_sig, 0)
            .unwrap();
    let result = sb.verify_boot_chain(&images, &manifest);
    assert_eq!(result, Err(BootError::HashMismatch));
}

#[test]
fn test_forged_signature_rejected() {
    let mut rng = CsRng::new();
    let root = Sm2KeyPair::generate(&mut rng).unwrap();
    let attacker = Sm2KeyPair::generate(&mut rng).unwrap();
    let bl = Sm2KeyPair::generate(&mut rng).unwrap();
    let rot = RootOfTrust::new(root.public_key, [0u8; 32]);
    let mut sb = EnerosSecureBoot::new_without_tpm(rot, 0);

    let bl_img = b"bootloader";
    let bl_sig = sign_image(bl_img, &attacker, &mut rng, 100);

    let kern_img = b"kernel";
    let rt_img = b"runtime";
    let kern_sig = sign_image(kern_img, &bl, &mut rng, 101);
    let rt_sig = sign_image(rt_img, &bl, &mut rng, 102);

    let images = BootImages {
        bootloader: bl_img,
        bl_sig: &bl_sig,
        kernel: kern_img,
        kernel_sig: &kern_sig,
        runtime: rt_img,
        rt_sig: &rt_sig,
    };

    let (raw, raw_len) = BootManifest::encode(&bl.public_key);
    let mut bl_img_with_manifest = vec![0x51u8; 64];
    bl_img_with_manifest.extend_from_slice(&raw[..raw_len]);
    let manifest_sig = sign_image(&bl_img_with_manifest, &root, &mut rng, 100);
    // 29 SE-02：manifest 必须经 verify_and_parse 验签解析（未验签 parse
    // 暴露下级公钥绑定断裂，B4-02 失效）。
    let manifest =
        BootManifest::verify_and_parse(&bl_img_with_manifest, &root.public_key, &manifest_sig, 0)
            .unwrap();
    let result = sb.verify_boot_chain(&images, &manifest);
    assert_eq!(result, Err(BootError::SignatureInvalid));
}

// ============================================================
// TPM 降级模式
// ============================================================

#[test]
fn test_tpm_unavailable_degraded_boot() {
    let mut rng = CsRng::new();
    let root = Sm2KeyPair::generate(&mut rng).unwrap();
    let bl = Sm2KeyPair::generate(&mut rng).unwrap();
    let rot = RootOfTrust::new(root.public_key, [0u8; 32]);

    let mut sb = EnerosSecureBoot::new_without_tpm(rot, 0);

    let bl_img = b"bootloader";
    let kern_img = b"kernel";
    let rt_img = b"runtime";
    let bl_sig = sign_image(bl_img, &root, &mut rng, 100);
    let kern_sig = sign_image(kern_img, &bl, &mut rng, 101);
    let rt_sig = sign_image(rt_img, &bl, &mut rng, 102);

    let images = BootImages {
        bootloader: bl_img,
        bl_sig: &bl_sig,
        kernel: kern_img,
        kernel_sig: &kern_sig,
        runtime: rt_img,
        rt_sig: &rt_sig,
    };
    let (raw, raw_len) = BootManifest::encode(&bl.public_key);
    let mut bl_img_with_manifest = vec![0x51u8; 64];
    bl_img_with_manifest.extend_from_slice(&raw[..raw_len]);
    let manifest_sig = sign_image(&bl_img_with_manifest, &root, &mut rng, 100);
    // 29 SE-02：manifest 必须经 verify_and_parse 验签解析（未验签 parse
    // 暴露下级公钥绑定断裂，B4-02 失效）。
    let manifest =
        BootManifest::verify_and_parse(&bl_img_with_manifest, &root.public_key, &manifest_sig, 0)
            .unwrap();

    let report = sb.verify_boot_chain(&images, &manifest).unwrap();
    assert!(report.can_boot());
    assert!(!report.pcr_extended);
}

// ============================================================
// 防降级测试
// ============================================================

#[test]
fn test_stale_image_rejected() {
    let mut rng = CsRng::new();
    let root = Sm2KeyPair::generate(&mut rng).unwrap();
    let bl = Sm2KeyPair::generate(&mut rng).unwrap();
    let rot = RootOfTrust::new(root.public_key, [0u8; 32]);

    let mut sb = EnerosSecureBoot::new_without_tpm(rot, 200);

    let bl_img = b"old bootloader";
    let bl_sig = sign_image(bl_img, &root, &mut rng, 100);

    let kern_img = b"kernel";
    let rt_img = b"runtime";
    let kern_sig = sign_image(kern_img, &bl, &mut rng, 201);
    let rt_sig = sign_image(rt_img, &bl, &mut rng, 202);

    let images = BootImages {
        bootloader: bl_img,
        bl_sig: &bl_sig,
        kernel: kern_img,
        kernel_sig: &kern_sig,
        runtime: rt_img,
        rt_sig: &rt_sig,
    };

    let (raw, raw_len) = BootManifest::encode(&bl.public_key);
    let mut bl_img_with_manifest = vec![0x51u8; 64];
    bl_img_with_manifest.extend_from_slice(&raw[..raw_len]);
    let manifest_sig = sign_image(&bl_img_with_manifest, &root, &mut rng, 100);
    // 29 SE-02：manifest 必须经 verify_and_parse 验签解析（未验签 parse
    // 暴露下级公钥绑定断裂，B4-02 失效）。
    let manifest =
        BootManifest::verify_and_parse(&bl_img_with_manifest, &root.public_key, &manifest_sig, 0)
            .unwrap();
    let result = sb.verify_boot_chain(&images, &manifest);
    assert_eq!(result, Err(BootError::StaleImage));
}

// ============================================================
// v0.158.0/v0.159.0 回归测试
// ============================================================

#[test]
fn test_regression_crypto_sm3() {
    let hash = eneros_crypto::sm3::hash(b"abc");
    let expected: [u8; 32] = [
        0x66, 0xC7, 0xF0, 0xF4, 0x62, 0xEE, 0xED, 0xD9, 0xD1, 0xF2, 0xD4, 0x6B, 0xDC, 0x10, 0xE4,
        0xE2, 0x41, 0x67, 0xC4, 0x87, 0x5C, 0xF2, 0xF7, 0xA2, 0x29, 0x7D, 0xA0, 0x2B, 0x8F, 0x4B,
        0xA8, 0xE0,
    ];
    assert_eq!(hash, expected);
}

#[test]
fn test_regression_tpm_pcr() {
    let mut tpm = SimulatedTpm::new();
    tpm.init().unwrap();
    let pcr0 = tpm.pcr_read(0).unwrap();
    assert_eq!(pcr0, [0u8; 32]);

    let digest = [0xAA; 32];
    tpm.pcr_extend(0, &digest).unwrap();
    let pcr0_after = tpm.pcr_read(0).unwrap();
    assert_ne!(pcr0_after, [0u8; 32]);
}
