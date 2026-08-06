use alloc::vec::Vec;

pub const MANIFEST_MAGIC: [u8; 8] = *b"ENERMAN1";
pub const MANIFEST_FORMAT_VERSION: u16 = 1;
pub const MANIFEST_HEADER_LEN: usize = 112;
pub const COMPONENT_ENCODED_LEN: usize = 128;
pub const MANIFEST_SIGNATURE_LEN: usize = 64;
pub const MANIFEST_TRAILER_LEN: usize = 72;
pub const MANIFEST_MAX_COMPONENTS: usize = 32;
pub const MANIFEST_MAX_LEN: usize =
    MANIFEST_HEADER_LEN + MANIFEST_MAX_COMPONENTS * COMPONENT_ENCODED_LEN + MANIFEST_TRAILER_LEN;

pub const MANIFEST_FLAG_RECOVERY_ALLOWED: u32 = 1 << 0;
pub const MANIFEST_KNOWN_FLAGS: u32 = MANIFEST_FLAG_RECOVERY_ALLOWED;
pub const COMPONENT_FLAG_REQUIRED: u32 = 1 << 0;
pub const COMPONENT_KNOWN_FLAGS: u32 = COMPONENT_FLAG_REQUIRED;
pub const PERMISSION_READ: u8 = 1 << 0;
pub const PERMISSION_WRITE: u8 = 1 << 1;
pub const PERMISSION_EXECUTE: u8 = 1 << 2;
pub const PERMISSION_KNOWN: u8 = PERMISSION_READ | PERMISSION_WRITE | PERMISSION_EXECUTE;

const CANONICAL_ENCODING_FIXED_LE_V1: u16 = 1;
const HASH_ALGORITHM_SHA256: u8 = 1;
const SIGNATURE_ALGORITHM_ED25519: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProductVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestHeader {
    pub flags: u32,
    pub platform_id: [u8; 16],
    pub product_version: ProductVersion,
    pub security_version: u64,
    pub source_date_epoch: u64,
    pub key_id: [u8; 16],
    pub release_id: [u8; 16],
}

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComponentRole {
    BootManager = 1,
    Kernel = 2,
    InitBundle = 3,
    Recovery = 4,
    DeviceTree = 5,
    Config = 6,
}

impl TryFrom<u16> for ComponentRole {
    type Error = ManifestError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::BootManager),
            2 => Ok(Self::Kernel),
            3 => Ok(Self::InitBundle),
            4 => Ok(Self::Recovery),
            5 => Ok(Self::DeviceTree),
            6 => Ok(Self::Config),
            _ => Err(ManifestError::Role),
        }
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManifestSlot {
    None = 0,
    A = 1,
    B = 2,
    Recovery = 3,
}

impl TryFrom<u8> for ManifestSlot {
    type Error = ManifestError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::A),
            2 => Ok(Self::B),
            3 => Ok(Self::Recovery),
            _ => Err(ManifestError::Slot),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub role: ComponentRole,
    pub slot: ManifestSlot,
    pub permissions: u8,
    pub flags: u32,
    pub media_offset: u64,
    pub media_length: u64,
    pub load_address: u64,
    pub memory_length: u64,
    pub entry_address: u64,
    pub security_version: u64,
    pub alignment: u32,
    pub digest: [u8; 32],
    pub content_id: [u8; 16],
    pub version: ProductVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub header: ManifestHeader,
    pub components: Vec<Component>,
    pub signature: [u8; MANIFEST_SIGNATURE_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    Length,
    Magic,
    Version,
    Encoding,
    Algorithm,
    Flags,
    Reserved,
    Count,
    Role,
    Slot,
    Permission,
    Range,
    Alignment,
    Digest,
    Identity,
    SecurityVersion,
    Duplicate,
    Order,
    MissingBootComponent,
    Signature,
    NonCanonical,
}

impl ReleaseManifest {
    pub fn new(header: ManifestHeader, components: Vec<Component>) -> Self {
        Self {
            header,
            components,
            signature: [0; MANIFEST_SIGNATURE_LEN],
        }
    }

    pub fn with_signature(mut self, signature: [u8; MANIFEST_SIGNATURE_LEN]) -> Self {
        self.signature = signature;
        self
    }

    pub fn signed_len(&self) -> Result<usize, ManifestError> {
        MANIFEST_HEADER_LEN
            .checked_add(
                self.components
                    .len()
                    .checked_mul(COMPONENT_ENCODED_LEN)
                    .ok_or(ManifestError::Length)?,
            )
            .ok_or(ManifestError::Length)
    }

    pub fn encoded_len(&self) -> Result<usize, ManifestError> {
        self.signed_len()?
            .checked_add(MANIFEST_TRAILER_LEN)
            .ok_or(ManifestError::Length)
    }

    pub fn encode_unsigned(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate(false)?;
        let signed_len = self.signed_len()?;
        let total_len = self.encoded_len()?;
        let total_len_u32 = u32::try_from(total_len).map_err(|_| ManifestError::Length)?;
        let count_u16 = u16::try_from(self.components.len()).map_err(|_| ManifestError::Count)?;
        let mut bytes = alloc::vec![0u8; signed_len];
        bytes[0..8].copy_from_slice(&MANIFEST_MAGIC);
        put_u16(&mut bytes, 8, MANIFEST_FORMAT_VERSION);
        put_u16(&mut bytes, 10, MANIFEST_HEADER_LEN as u16);
        put_u32(&mut bytes, 12, total_len_u32);
        put_u32(&mut bytes, 16, self.header.flags);
        put_u16(&mut bytes, 20, CANONICAL_ENCODING_FIXED_LE_V1);
        bytes[22] = HASH_ALGORITHM_SHA256;
        bytes[23] = SIGNATURE_ALGORITHM_ED25519;
        put_u16(&mut bytes, 24, count_u16);
        bytes[28..44].copy_from_slice(&self.header.platform_id);
        put_u16(&mut bytes, 44, self.header.product_version.major);
        put_u16(&mut bytes, 46, self.header.product_version.minor);
        put_u16(&mut bytes, 48, self.header.product_version.patch);
        put_u64(&mut bytes, 52, self.header.security_version);
        put_u64(&mut bytes, 60, self.header.source_date_epoch);
        bytes[68..84].copy_from_slice(&self.header.key_id);
        bytes[84..100].copy_from_slice(&self.header.release_id);

        for (index, component) in self.components.iter().enumerate() {
            let offset = MANIFEST_HEADER_LEN + index * COMPONENT_ENCODED_LEN;
            encode_component(
                component,
                &mut bytes[offset..offset + COMPONENT_ENCODED_LEN],
            );
        }
        Ok(bytes)
    }

    pub fn encode_signed(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate(true)?;
        let mut bytes = self.encode_unsigned()?;
        let signed_len = bytes.len();
        bytes.resize(signed_len + MANIFEST_TRAILER_LEN, 0);
        put_u32(
            &mut bytes,
            signed_len,
            u32::try_from(signed_len).map_err(|_| ManifestError::Length)?,
        );
        put_u16(&mut bytes, signed_len + 4, MANIFEST_SIGNATURE_LEN as u16);
        bytes[signed_len + 8..signed_len + 8 + MANIFEST_SIGNATURE_LEN]
            .copy_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        if bytes.len() < MANIFEST_HEADER_LEN + MANIFEST_TRAILER_LEN
            || bytes.len() > MANIFEST_MAX_LEN
        {
            return Err(ManifestError::Length);
        }
        if bytes[0..8] != MANIFEST_MAGIC {
            return Err(ManifestError::Magic);
        }
        if get_u16(bytes, 8) != MANIFEST_FORMAT_VERSION
            || usize::from(get_u16(bytes, 10)) != MANIFEST_HEADER_LEN
        {
            return Err(ManifestError::Version);
        }
        if usize::try_from(get_u32(bytes, 12)).map_err(|_| ManifestError::Length)? != bytes.len() {
            return Err(ManifestError::Length);
        }
        if get_u16(bytes, 20) != CANONICAL_ENCODING_FIXED_LE_V1 {
            return Err(ManifestError::Encoding);
        }
        if bytes[22] != HASH_ALGORITHM_SHA256 || bytes[23] != SIGNATURE_ALGORITHM_ED25519 {
            return Err(ManifestError::Algorithm);
        }
        let count = usize::from(get_u16(bytes, 24));
        if count == 0 || count > MANIFEST_MAX_COMPONENTS {
            return Err(ManifestError::Count);
        }
        let signed_len = MANIFEST_HEADER_LEN
            .checked_add(
                count
                    .checked_mul(COMPONENT_ENCODED_LEN)
                    .ok_or(ManifestError::Length)?,
            )
            .ok_or(ManifestError::Length)?;
        let expected_len = signed_len
            .checked_add(MANIFEST_TRAILER_LEN)
            .ok_or(ManifestError::Length)?;
        if expected_len != bytes.len() {
            return Err(ManifestError::Length);
        }
        require_zero(&bytes[26..28])?;
        require_zero(&bytes[50..52])?;
        require_zero(&bytes[100..112])?;
        if usize::try_from(get_u32(bytes, signed_len)).map_err(|_| ManifestError::Length)?
            != signed_len
            || usize::from(get_u16(bytes, signed_len + 4)) != MANIFEST_SIGNATURE_LEN
        {
            return Err(ManifestError::Signature);
        }
        require_zero(&bytes[signed_len + 6..signed_len + 8])?;

        let header = ManifestHeader {
            flags: get_u32(bytes, 16),
            platform_id: get_array_16(bytes, 28),
            product_version: ProductVersion {
                major: get_u16(bytes, 44),
                minor: get_u16(bytes, 46),
                patch: get_u16(bytes, 48),
            },
            security_version: get_u64(bytes, 52),
            source_date_epoch: get_u64(bytes, 60),
            key_id: get_array_16(bytes, 68),
            release_id: get_array_16(bytes, 84),
        };
        let mut components = Vec::with_capacity(count);
        for index in 0..count {
            let offset = MANIFEST_HEADER_LEN + index * COMPONENT_ENCODED_LEN;
            components.push(decode_component(
                &bytes[offset..offset + COMPONENT_ENCODED_LEN],
            )?);
        }
        let mut signature = [0u8; MANIFEST_SIGNATURE_LEN];
        signature.copy_from_slice(&bytes[signed_len + 8..signed_len + 8 + MANIFEST_SIGNATURE_LEN]);
        let manifest = Self {
            header,
            components,
            signature,
        };
        manifest.validate(true)?;
        if manifest.encode_signed()?.as_slice() != bytes {
            return Err(ManifestError::NonCanonical);
        }
        Ok(manifest)
    }

    pub fn signed_region(encoded: &[u8]) -> Result<&[u8], ManifestError> {
        let manifest = Self::decode(encoded)?;
        let signed_len = manifest.signed_len()?;
        Ok(&encoded[..signed_len])
    }

    pub fn signature_region(
        encoded: &[u8],
    ) -> Result<&[u8; MANIFEST_SIGNATURE_LEN], ManifestError> {
        let manifest = Self::decode(encoded)?;
        let signed_len = manifest.signed_len()?;
        encoded[signed_len + 8..signed_len + 8 + MANIFEST_SIGNATURE_LEN]
            .try_into()
            .map_err(|_| ManifestError::Signature)
    }

    fn validate(&self, require_signature: bool) -> Result<(), ManifestError> {
        if self.header.flags & !MANIFEST_KNOWN_FLAGS != 0 {
            return Err(ManifestError::Flags);
        }
        if self.components.is_empty() || self.components.len() > MANIFEST_MAX_COMPONENTS {
            return Err(ManifestError::Count);
        }
        if is_zero(&self.header.platform_id)
            || is_zero(&self.header.key_id)
            || is_zero(&self.header.release_id)
            || self.header.product_version
                == (ProductVersion {
                    major: 0,
                    minor: 0,
                    patch: 0,
                })
        {
            return Err(ManifestError::Identity);
        }
        if self.header.security_version == 0 || self.header.source_date_epoch == 0 {
            return Err(ManifestError::SecurityVersion);
        }
        if require_signature && is_zero(&self.signature) {
            return Err(ManifestError::Signature);
        }
        let mut previous_key = None;
        let mut role_slots = Vec::with_capacity(self.components.len());
        let mut has_boot_component = false;
        for component in &self.components {
            validate_component(component, self.header.security_version)?;
            let key = (
                component.role as u16,
                component.slot as u8,
                component.content_id,
            );
            if let Some(previous) = previous_key {
                if previous >= key {
                    return Err(ManifestError::Order);
                }
            }
            previous_key = Some(key);
            let role_slot = (component.role as u16, component.slot as u8);
            if role_slots.contains(&role_slot) {
                return Err(ManifestError::Duplicate);
            }
            role_slots.push(role_slot);
            has_boot_component |= matches!(
                component.role,
                ComponentRole::Kernel | ComponentRole::Recovery
            );
        }
        if !has_boot_component {
            return Err(ManifestError::MissingBootComponent);
        }
        self.encoded_len()?;
        Ok(())
    }
}

fn validate_component(component: &Component, manifest_security: u64) -> Result<(), ManifestError> {
    if component.permissions == 0 || component.permissions & !PERMISSION_KNOWN != 0 {
        return Err(ManifestError::Permission);
    }
    if component.permissions & PERMISSION_WRITE != 0
        && component.permissions & PERMISSION_EXECUTE != 0
    {
        return Err(ManifestError::Permission);
    }
    if component.flags & !COMPONENT_KNOWN_FLAGS != 0 {
        return Err(ManifestError::Flags);
    }
    if component.media_length == 0
        || component.memory_length == 0
        || component.media_length > component.memory_length
        || component
            .media_offset
            .checked_add(component.media_length)
            .is_none()
        || component
            .load_address
            .checked_add(component.memory_length)
            .is_none()
    {
        return Err(ManifestError::Range);
    }
    if component.alignment < 512
        || component.alignment > 2 * 1024 * 1024
        || !component.alignment.is_power_of_two()
        || component.media_offset % u64::from(component.alignment) != 0
        || component.load_address % u64::from(component.alignment) != 0
    {
        return Err(ManifestError::Alignment);
    }
    let executable = component.permissions & PERMISSION_EXECUTE != 0;
    if executable {
        let end = component
            .load_address
            .checked_add(component.memory_length)
            .ok_or(ManifestError::Range)?;
        if component.entry_address < component.load_address || component.entry_address >= end {
            return Err(ManifestError::Range);
        }
    } else if component.entry_address != 0 {
        return Err(ManifestError::Permission);
    }
    match component.role {
        ComponentRole::BootManager | ComponentRole::Kernel | ComponentRole::Recovery
            if !executable || component.permissions & PERMISSION_READ == 0 =>
        {
            return Err(ManifestError::Permission);
        }
        ComponentRole::InitBundle | ComponentRole::DeviceTree | ComponentRole::Config
            if executable || component.permissions & PERMISSION_READ == 0 =>
        {
            return Err(ManifestError::Permission);
        }
        _ => {}
    }
    if component.security_version < manifest_security {
        return Err(ManifestError::SecurityVersion);
    }
    if is_zero(&component.digest) || is_zero(&component.content_id) {
        return Err(ManifestError::Digest);
    }
    Ok(())
}

fn encode_component(component: &Component, bytes: &mut [u8]) {
    put_u16(bytes, 0, component.role as u16);
    bytes[2] = component.slot as u8;
    bytes[3] = component.permissions;
    put_u32(bytes, 4, component.flags);
    put_u64(bytes, 8, component.media_offset);
    put_u64(bytes, 16, component.media_length);
    put_u64(bytes, 24, component.load_address);
    put_u64(bytes, 32, component.memory_length);
    put_u64(bytes, 40, component.entry_address);
    put_u64(bytes, 48, component.security_version);
    put_u32(bytes, 56, component.alignment);
    bytes[64..96].copy_from_slice(&component.digest);
    bytes[96..112].copy_from_slice(&component.content_id);
    put_u16(bytes, 112, component.version.major);
    put_u16(bytes, 114, component.version.minor);
    put_u16(bytes, 116, component.version.patch);
}

fn decode_component(bytes: &[u8]) -> Result<Component, ManifestError> {
    require_zero(&bytes[60..64])?;
    require_zero(&bytes[118..128])?;
    Ok(Component {
        role: ComponentRole::try_from(get_u16(bytes, 0))?,
        slot: ManifestSlot::try_from(bytes[2])?,
        permissions: bytes[3],
        flags: get_u32(bytes, 4),
        media_offset: get_u64(bytes, 8),
        media_length: get_u64(bytes, 16),
        load_address: get_u64(bytes, 24),
        memory_length: get_u64(bytes, 32),
        entry_address: get_u64(bytes, 40),
        security_version: get_u64(bytes, 48),
        alignment: get_u32(bytes, 56),
        digest: get_array_32(bytes, 64),
        content_id: get_array_16(bytes, 96),
        version: ProductVersion {
            major: get_u16(bytes, 112),
            minor: get_u16(bytes, 114),
            patch: get_u16(bytes, 116),
        },
    })
}

fn is_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

fn require_zero(bytes: &[u8]) -> Result<(), ManifestError> {
    if is_zero(bytes) {
        Ok(())
    } else {
        Err(ManifestError::Reserved)
    }
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn get_array_16(bytes: &[u8], offset: usize) -> [u8; 16] {
    let mut value = [0u8; 16];
    value.copy_from_slice(&bytes[offset..offset + 16]);
    value
}

fn get_array_32(bytes: &[u8], offset: usize) -> [u8; 32] {
    let mut value = [0u8; 32];
    value.copy_from_slice(&bytes[offset..offset + 32]);
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(
        role: ComponentRole,
        slot: ManifestSlot,
        media_offset: u64,
        load_address: u64,
        content_byte: u8,
    ) -> Component {
        let executable = matches!(
            role,
            ComponentRole::BootManager | ComponentRole::Kernel | ComponentRole::Recovery
        );
        Component {
            role,
            slot,
            permissions: if executable {
                PERMISSION_READ | PERMISSION_EXECUTE
            } else {
                PERMISSION_READ
            },
            flags: COMPONENT_FLAG_REQUIRED,
            media_offset,
            media_length: 4096,
            load_address,
            memory_length: 4096,
            entry_address: if executable { load_address } else { 0 },
            security_version: 3,
            alignment: 4096,
            digest: [content_byte; 32],
            content_id: [content_byte; 16],
            version: ProductVersion {
                major: 0,
                minor: 192,
                patch: 0,
            },
        }
    }

    fn valid_manifest() -> ReleaseManifest {
        ReleaseManifest::new(
            ManifestHeader {
                flags: MANIFEST_FLAG_RECOVERY_ALLOWED,
                platform_id: [0x10; 16],
                product_version: ProductVersion {
                    major: 0,
                    minor: 192,
                    patch: 0,
                },
                security_version: 3,
                source_date_epoch: 1_700_000_000,
                key_id: [0x20; 16],
                release_id: [0x30; 16],
            },
            alloc::vec![
                component(
                    ComponentRole::Kernel,
                    ManifestSlot::A,
                    0x10_0000,
                    0x4100_0000,
                    0x41,
                ),
                component(
                    ComponentRole::InitBundle,
                    ManifestSlot::A,
                    0x20_0000,
                    0x4200_0000,
                    0x42,
                ),
            ],
        )
        .with_signature([0xA5; MANIFEST_SIGNATURE_LEN])
    }

    #[test]
    fn canonical_roundtrip_and_golden_offsets() {
        let manifest = valid_manifest();
        let bytes = manifest.encode_signed().unwrap();
        assert_eq!(bytes.len(), 440);
        assert_eq!(&bytes[0..8], b"ENERMAN1");
        assert_eq!(&bytes[8..16], &[1, 0, 112, 0, 184, 1, 0, 0]);
        assert_eq!(&bytes[24..28], &[2, 0, 0, 0]);
        assert_eq!(
            &bytes[MANIFEST_HEADER_LEN + 8..MANIFEST_HEADER_LEN + 16],
            &0x10_0000u64.to_le_bytes()
        );
        assert_eq!(ReleaseManifest::decode(&bytes), Ok(manifest));
    }

    #[test]
    fn signed_region_covers_header_and_every_component_field() {
        let bytes = valid_manifest().encode_signed().unwrap();
        let signed = ReleaseManifest::signed_region(&bytes).unwrap();
        assert_eq!(
            signed.len(),
            MANIFEST_HEADER_LEN + 2 * COMPONENT_ENCODED_LEN
        );
        assert_eq!(
            ReleaseManifest::signature_region(&bytes).unwrap(),
            &[0xA5; MANIFEST_SIGNATURE_LEN]
        );
        assert!(84 + 16 <= signed.len(), "release id must be signed");
        assert!(
            MANIFEST_HEADER_LEN + COMPONENT_ENCODED_LEN + 118 <= signed.len(),
            "last component semantic field must be signed"
        );
    }

    #[test]
    fn truncated_unknown_reserved_and_length_are_rejected() {
        let bytes = valid_manifest().encode_signed().unwrap();
        assert_eq!(
            ReleaseManifest::decode(&bytes[..bytes.len() - 1]),
            Err(ManifestError::Length)
        );

        let mut bad = bytes.clone();
        bad[26] = 1;
        assert_eq!(ReleaseManifest::decode(&bad), Err(ManifestError::Reserved));

        let mut bad = bytes;
        bad[16] = 0x80;
        assert_eq!(ReleaseManifest::decode(&bad), Err(ManifestError::Flags));
    }

    #[test]
    fn duplicate_unsorted_wx_and_overflow_fail_closed() {
        let mut manifest = valid_manifest();
        manifest.components[1].role = ComponentRole::Kernel;
        manifest.components[1].permissions = PERMISSION_READ | PERMISSION_EXECUTE;
        manifest.components[1].entry_address = manifest.components[1].load_address;
        assert_eq!(manifest.encode_signed(), Err(ManifestError::Duplicate));

        let mut manifest = valid_manifest();
        manifest.components.swap(0, 1);
        assert_eq!(manifest.encode_signed(), Err(ManifestError::Order));

        let mut manifest = valid_manifest();
        manifest.components[0].permissions |= PERMISSION_WRITE;
        assert_eq!(manifest.encode_signed(), Err(ManifestError::Permission));

        let mut manifest = valid_manifest();
        manifest.components[0].media_offset = u64::MAX - 7;
        manifest.components[0].media_length = 4096;
        assert_eq!(manifest.encode_signed(), Err(ManifestError::Range));
    }

    #[test]
    fn bounded_mutation_parser_never_panics_and_rejects_noncanonical_inputs() {
        let bytes = valid_manifest().encode_signed().unwrap();
        let mut rejected = 0usize;
        for index in 0..bytes.len() {
            let mut mutated = bytes.clone();
            mutated[index] ^= 0x80;
            if ReleaseManifest::decode(&mutated).is_err() {
                rejected += 1;
            }
        }
        assert!(
            rejected >= 100,
            "mutation oracle unexpectedly accepted too many structural mutations: {rejected}"
        );
        for length in 0..MANIFEST_HEADER_LEN + MANIFEST_TRAILER_LEN {
            assert!(ReleaseManifest::decode(&bytes[..length.min(bytes.len())]).is_err());
        }
    }
}
