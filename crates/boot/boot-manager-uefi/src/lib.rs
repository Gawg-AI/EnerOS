#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use ed25519_dalek::{Signature, VerifyingKey};
use eneros_boot_protocol::{
    Component, ComponentRole, ManifestSlot, ReleaseManifest, MANIFEST_HEADER_LEN, MANIFEST_MAX_LEN,
};
use sha2::{Digest, Sha256};

pub const MANIFEST_MEDIA_OFFSET: u64 = 0;
pub const MANIFEST_RESERVED_BYTES: u64 = 64 * 1024;
pub const KERNEL_LOAD_START: u64 = 0x4100_0000;
pub const KERNEL_LOAD_END: u64 = 0x4180_0000;
/// 30 RT-010：init bundle 加载区间单一事实源——引用 boot-protocol 常量
/// （0x4200_0000 起 2 MiB 内核 BOOT-007 窗口）。旧实现允许 8 MiB 浮动区间，
/// 与内核固定映射/root-task 固定读冲突（load_address ≠ 0x4200_0000 即静默
/// 断裂）。validate 收紧：load_address 必须 == INIT_LOAD_START。
pub const INIT_LOAD_START: u64 = eneros_boot_protocol::ENEROS_INIT_BUNDLE_PHYS;
pub const INIT_LOAD_END: u64 =
    INIT_LOAD_START + eneros_boot_protocol::ENEROS_INIT_BUNDLE_MAX_LEN;

#[derive(Debug, Clone, Copy)]
pub struct TrustPolicy {
    pub platform_id: [u8; 16],
    pub key_id: [u8; 16],
    pub public_key: [u8; 32],
    pub minimum_security_version: u64,
    pub expected_security_version: u64,
    pub expected_release_id: [u8; 16],
    pub selected_slot: ManifestSlot,
    pub partition_length: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoaderError {
    ManifestHeader,
    ManifestLength,
    ManifestFormat,
    Platform,
    KeyId,
    SecurityVersion,
    SecurityBinding,
    ImageId,
    PublicKey,
    Signature,
    Slot,
    MissingKernel,
    MissingInitBundle,
    DuplicateComponent,
    MediaRange,
    MediaOverlap,
    LoadRange,
    LoadOverlap,
    Permission,
    PayloadLength,
    PayloadDigest,
}

impl LoaderError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ManifestHeader => "MANIFEST_HEADER",
            Self::ManifestLength => "MANIFEST_LENGTH",
            Self::ManifestFormat => "MANIFEST_FORMAT",
            Self::Platform => "PLATFORM",
            Self::KeyId => "KEY_ID",
            Self::SecurityVersion => "ROLLBACK",
            Self::SecurityBinding => "SECURITY_BINDING",
            Self::ImageId => "IMAGE_ID",
            Self::PublicKey => "PUBLIC_KEY",
            Self::Signature => "SIGNATURE",
            Self::Slot => "SLOT",
            Self::MissingKernel => "MISSING_KERNEL",
            Self::MissingInitBundle => "MISSING_INIT_BUNDLE",
            Self::DuplicateComponent => "DUPLICATE_COMPONENT",
            Self::MediaRange => "MEDIA_RANGE",
            Self::MediaOverlap => "MEDIA_OVERLAP",
            Self::LoadRange => "LOAD_RANGE",
            Self::LoadOverlap => "LOAD_OVERLAP",
            Self::Permission => "PERMISSION",
            Self::PayloadLength => "PAYLOAD_LENGTH",
            Self::PayloadDigest => "PAYLOAD_DIGEST",
        }
    }
}

pub fn declared_manifest_len(header: &[u8]) -> Result<usize, LoaderError> {
    if header.len() < MANIFEST_HEADER_LEN || header[0..8] != *b"ENERMAN1" {
        return Err(LoaderError::ManifestHeader);
    }
    let total = u32::from_le_bytes(
        header[12..16]
            .try_into()
            .map_err(|_| LoaderError::ManifestHeader)?,
    );
    let total = usize::try_from(total).map_err(|_| LoaderError::ManifestLength)?;
    if !(MANIFEST_HEADER_LEN..=MANIFEST_MAX_LEN).contains(&total) {
        return Err(LoaderError::ManifestLength);
    }
    Ok(total)
}

pub fn verify_manifest(
    encoded: &[u8],
    policy: &TrustPolicy,
) -> Result<ReleaseManifest, LoaderError> {
    if declared_manifest_len(encoded)? != encoded.len() {
        return Err(LoaderError::ManifestLength);
    }
    let manifest = ReleaseManifest::decode(encoded).map_err(|_| LoaderError::ManifestFormat)?;
    if manifest.header.platform_id != policy.platform_id {
        return Err(LoaderError::Platform);
    }
    if manifest.header.key_id != policy.key_id {
        return Err(LoaderError::KeyId);
    }
    if manifest.header.security_version < policy.minimum_security_version {
        return Err(LoaderError::SecurityVersion);
    }
    if manifest.header.security_version != policy.expected_security_version {
        return Err(LoaderError::SecurityBinding);
    }
    if manifest.header.release_id != policy.expected_release_id {
        return Err(LoaderError::ImageId);
    }

    let verifying_key =
        VerifyingKey::from_bytes(&policy.public_key).map_err(|_| LoaderError::PublicKey)?;
    let signed =
        ReleaseManifest::signed_region(encoded).map_err(|_| LoaderError::ManifestFormat)?;
    let signature_bytes =
        ReleaseManifest::signature_region(encoded).map_err(|_| LoaderError::ManifestFormat)?;
    let signature = Signature::from_bytes(signature_bytes);
    verifying_key
        .verify_strict(signed, &signature)
        .map_err(|_| LoaderError::Signature)?;

    validate_components(&manifest, policy)?;
    Ok(manifest)
}

pub fn verify_component(component: &Component, payload: &[u8]) -> Result<(), LoaderError> {
    if u64::try_from(payload.len()).map_err(|_| LoaderError::PayloadLength)?
        != component.media_length
    {
        return Err(LoaderError::PayloadLength);
    }
    let digest: [u8; 32] = Sha256::digest(payload).into();
    if digest != component.digest {
        return Err(LoaderError::PayloadDigest);
    }
    Ok(())
}

pub fn manifest_digest(encoded: &[u8]) -> [u8; 32] {
    Sha256::digest(encoded).into()
}

fn validate_components(
    manifest: &ReleaseManifest,
    policy: &TrustPolicy,
) -> Result<(), LoaderError> {
    let mut kernel = None;
    let mut init = None;
    for component in &manifest.components {
        if component.slot != policy.selected_slot {
            return Err(LoaderError::Slot);
        }
        let media_end = component
            .media_offset
            .checked_add(component.media_length)
            .ok_or(LoaderError::MediaRange)?;
        if component.media_offset < MANIFEST_RESERVED_BYTES || media_end > policy.partition_length {
            return Err(LoaderError::MediaRange);
        }
        let load_end = component
            .load_address
            .checked_add(component.memory_length)
            .ok_or(LoaderError::LoadRange)?;
        match component.role {
            ComponentRole::Kernel => {
                if kernel.replace(component).is_some() {
                    return Err(LoaderError::DuplicateComponent);
                }
                if component.load_address < KERNEL_LOAD_START
                    || load_end > KERNEL_LOAD_END
                    || component.entry_address < component.load_address
                    || component.entry_address >= load_end
                {
                    return Err(LoaderError::LoadRange);
                }
                if component.permissions
                    != (eneros_boot_protocol::manifest::PERMISSION_READ
                        | eneros_boot_protocol::manifest::PERMISSION_EXECUTE)
                {
                    return Err(LoaderError::Permission);
                }
            }
            ComponentRole::InitBundle => {
                if init.replace(component).is_some() {
                    return Err(LoaderError::DuplicateComponent);
                }
                // 30 RT-010：收紧——load_address 必须 == INIT_LOAD_START
                // （内核 2 MiB 固定窗口 + root-task 固定读，浮动区间必然
                // 静默断裂）；load_end 不得超过窗口末端。
                if component.load_address != INIT_LOAD_START
                    || load_end > INIT_LOAD_END
                    || component.entry_address != 0
                {
                    return Err(LoaderError::LoadRange);
                }
                if component.permissions != eneros_boot_protocol::manifest::PERMISSION_READ {
                    return Err(LoaderError::Permission);
                }
            }
            _ => return Err(LoaderError::Permission),
        }
    }
    let kernel = kernel.ok_or(LoaderError::MissingKernel)?;
    let init = init.ok_or(LoaderError::MissingInitBundle)?;
    if ranges_overlap(
        kernel.media_offset,
        kernel.media_length,
        init.media_offset,
        init.media_length,
    ) {
        return Err(LoaderError::MediaOverlap);
    }
    if ranges_overlap(
        kernel.load_address,
        kernel.memory_length,
        init.load_address,
        init.memory_length,
    ) {
        return Err(LoaderError::LoadOverlap);
    }
    Ok(())
}

fn ranges_overlap(first: u64, first_len: u64, second: u64, second_len: u64) -> bool {
    match (first.checked_add(first_len), second.checked_add(second_len)) {
        (Some(first_end), Some(second_end)) => first < second_end && second < first_end,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use ed25519_dalek::{Signer, SigningKey};
    use eneros_boot_protocol::manifest::COMPONENT_FLAG_REQUIRED;
    use eneros_boot_protocol::{Component, ManifestHeader, ProductVersion};

    use super::*;

    fn signed_fixture() -> (alloc::vec::Vec<u8>, TrustPolicy, alloc::vec::Vec<u8>) {
        let signing = SigningKey::from_bytes(&[0x37; 32]);
        let platform_id = [0x41; 16];
        let key_id = [0x52; 16];
        let kernel_payload = vec![0xa5; 4096];
        let init_payload = vec![0x5a; 1024];
        let components = vec![
            Component {
                role: ComponentRole::Kernel,
                slot: ManifestSlot::A,
                permissions: eneros_boot_protocol::manifest::PERMISSION_READ
                    | eneros_boot_protocol::manifest::PERMISSION_EXECUTE,
                flags: COMPONENT_FLAG_REQUIRED,
                media_offset: 64 * 1024,
                media_length: kernel_payload.len() as u64,
                load_address: KERNEL_LOAD_START,
                memory_length: kernel_payload.len() as u64,
                entry_address: KERNEL_LOAD_START,
                security_version: 7,
                alignment: 4096,
                digest: Sha256::digest(&kernel_payload).into(),
                content_id: [1; 16],
                version: ProductVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
            },
            Component {
                role: ComponentRole::InitBundle,
                slot: ManifestSlot::A,
                permissions: eneros_boot_protocol::manifest::PERMISSION_READ,
                flags: COMPONENT_FLAG_REQUIRED,
                media_offset: 128 * 1024,
                media_length: init_payload.len() as u64,
                load_address: INIT_LOAD_START,
                memory_length: init_payload.len() as u64,
                entry_address: 0,
                security_version: 7,
                alignment: 4096,
                digest: Sha256::digest(&init_payload).into(),
                content_id: [2; 16],
                version: ProductVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
            },
        ];
        let manifest = ReleaseManifest::new(
            ManifestHeader {
                flags: 0,
                platform_id,
                product_version: ProductVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                security_version: 7,
                source_date_epoch: 1_700_000_000,
                key_id,
                release_id: [0x63; 16],
            },
            components,
        );
        let unsigned = manifest.encode_unsigned().unwrap();
        let signature = signing.sign(&unsigned).to_bytes();
        let encoded = manifest.with_signature(signature).encode_signed().unwrap();
        (
            encoded,
            TrustPolicy {
                platform_id,
                key_id,
                public_key: signing.verifying_key().to_bytes(),
                minimum_security_version: 7,
                expected_security_version: 7,
                expected_release_id: [0x63; 16],
                selected_slot: ManifestSlot::A,
                partition_length: 8 * 1024 * 1024,
            },
            kernel_payload,
        )
    }

    #[test]
    fn signed_manifest_and_payload_are_accepted() {
        let (encoded, policy, payload) = signed_fixture();
        let manifest = verify_manifest(&encoded, &policy).unwrap();
        verify_component(&manifest.components[0], &payload).unwrap();
    }

    #[test]
    fn every_manifest_byte_flip_is_rejected_or_changes_a_signed_field() {
        let (encoded, policy, _) = signed_fixture();
        for index in 0..encoded.len() {
            let mut mutated = encoded.clone();
            mutated[index] ^= 1;
            assert!(
                verify_manifest(&mutated, &policy).is_err(),
                "manifest mutation at {index} was accepted"
            );
        }
    }

    #[test]
    fn wrong_platform_key_floor_slot_and_payload_are_rejected() {
        let (encoded, policy, mut payload) = signed_fixture();
        let mut wrong = policy;
        wrong.platform_id[0] ^= 1;
        assert_eq!(
            verify_manifest(&encoded, &wrong),
            Err(LoaderError::Platform)
        );
        let mut wrong = policy;
        wrong.key_id[0] ^= 1;
        assert_eq!(verify_manifest(&encoded, &wrong), Err(LoaderError::KeyId));
        let mut wrong = policy;
        wrong.minimum_security_version += 1;
        assert_eq!(
            verify_manifest(&encoded, &wrong),
            Err(LoaderError::SecurityVersion)
        );
        let mut wrong = policy;
        wrong.expected_security_version += 1;
        assert_eq!(
            verify_manifest(&encoded, &wrong),
            Err(LoaderError::SecurityBinding)
        );
        let mut wrong = policy;
        wrong.expected_release_id[0] ^= 1;
        assert_eq!(verify_manifest(&encoded, &wrong), Err(LoaderError::ImageId));
        let mut wrong = policy;
        wrong.selected_slot = ManifestSlot::B;
        assert_eq!(verify_manifest(&encoded, &wrong), Err(LoaderError::Slot));

        let manifest = verify_manifest(&encoded, &policy).unwrap();
        payload[13] ^= 1;
        assert_eq!(
            verify_component(&manifest.components[0], &payload),
            Err(LoaderError::PayloadDigest)
        );
    }

    /// 30 RT-010 判据：init bundle load_address 浮动（≠ INIT_LOAD_START）
    /// → LoadRange（旧实现允许 8 MiB 区间内任意浮动，与内核固定窗口/
    /// root-task 固定读静默断裂）。
    #[test]
    fn floating_init_load_address_rejected() {
        let (encoded, policy, payload) = signed_fixture();
        let manifest = verify_manifest(&encoded, &policy).unwrap();
        let mut init = manifest.components[1].clone();
        assert_eq!(init.role, ComponentRole::InitBundle);
        init.load_address = INIT_LOAD_START + 0x1000; // 浮动
        let result = validate_components(
            &ReleaseManifest {
                components: alloc::vec![manifest.components[0].clone(), init],
                ..manifest.clone()
            },
            &policy,
        );
        assert_eq!(result, Err(LoaderError::LoadRange));
        // 窗口末端越界同样拒绝。
        let mut init2 = manifest.components[1].clone();
        init2.memory_length = eneros_boot_protocol::ENEROS_INIT_BUNDLE_MAX_LEN + 1;
        let result2 = validate_components(
            &ReleaseManifest {
                components: alloc::vec![manifest.components[0].clone(), init2],
                ..manifest.clone()
            },
            &policy,
        );
        assert_eq!(result2, Err(LoaderError::LoadRange));
    }

    #[test]
    fn untrusted_lengths_are_bounded_before_decode() {
        let mut header = [0u8; MANIFEST_HEADER_LEN];
        header[..8].copy_from_slice(b"ENERMAN1");
        header[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            declared_manifest_len(&header),
            Err(LoaderError::ManifestLength)
        );
        assert_eq!(
            declared_manifest_len(&header[..20]),
            Err(LoaderError::ManifestHeader)
        );
    }
}
