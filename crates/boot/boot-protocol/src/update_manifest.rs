pub const UPDATE_MANIFEST_MAGIC: [u8; 8] = *b"ENRUPD01";
pub const UPDATE_MANIFEST_VERSION: u16 = 1;
pub const UPDATE_MANIFEST_SIGNED_SIZE: usize = 256;
pub const UPDATE_MANIFEST_SIGNATURE_SIZE: usize = 64;
pub const UPDATE_MANIFEST_SIZE: usize =
    UPDATE_MANIFEST_SIGNED_SIZE + UPDATE_MANIFEST_SIGNATURE_SIZE;

const MAX_IMAGE_SIZE: u64 = 1024 * 1024 * 1024 * 1024;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatePayloadKind {
    Full = 1,
    Delta = 2,
}

impl TryFrom<u8> for UpdatePayloadKind {
    type Error = UpdateManifestError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Full),
            2 => Ok(Self::Delta),
            _ => Err(UpdateManifestError::PayloadKind),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateManifest {
    pub payload_kind: UpdatePayloadKind,
    pub platform_id: [u8; 16],
    pub key_id: [u8; 16],
    pub base_release_id: [u8; 16],
    pub target_release_id: [u8; 16],
    pub base_security_version: u64,
    pub target_security_version: u64,
    pub source_date_epoch: u64,
    pub payload_length: u64,
    pub base_image_length: u64,
    pub target_image_length: u64,
    pub base_image_digest: [u8; 32],
    pub target_image_digest: [u8; 32],
    pub payload_digest: [u8; 32],
    pub update_id: [u8; 16],
    pub signature: [u8; UPDATE_MANIFEST_SIGNATURE_SIZE],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateManifestError {
    Length,
    Magic,
    Version,
    Algorithm,
    Reserved,
    PayloadKind,
    Identity,
    SecurityVersion,
    Epoch,
    PayloadLength,
    FullPayloadMismatch,
    Signature,
    NonCanonical,
}

impl UpdateManifest {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        payload_kind: UpdatePayloadKind,
        platform_id: [u8; 16],
        key_id: [u8; 16],
        base_release_id: [u8; 16],
        target_release_id: [u8; 16],
        base_security_version: u64,
        target_security_version: u64,
        source_date_epoch: u64,
        payload_length: u64,
        base_image_length: u64,
        target_image_length: u64,
        base_image_digest: [u8; 32],
        target_image_digest: [u8; 32],
        payload_digest: [u8; 32],
        update_id: [u8; 16],
    ) -> Self {
        Self {
            payload_kind,
            platform_id,
            key_id,
            base_release_id,
            target_release_id,
            base_security_version,
            target_security_version,
            source_date_epoch,
            payload_length,
            base_image_length,
            target_image_length,
            base_image_digest,
            target_image_digest,
            payload_digest,
            update_id,
            signature: [0; UPDATE_MANIFEST_SIGNATURE_SIZE],
        }
    }

    pub fn with_signature(mut self, signature: [u8; UPDATE_MANIFEST_SIGNATURE_SIZE]) -> Self {
        self.signature = signature;
        self
    }

    pub fn encode_unsigned(
        &self,
    ) -> Result<[u8; UPDATE_MANIFEST_SIGNED_SIZE], UpdateManifestError> {
        self.validate(false)?;
        let mut bytes = [0u8; UPDATE_MANIFEST_SIGNED_SIZE];
        bytes[0..8].copy_from_slice(&UPDATE_MANIFEST_MAGIC);
        put_u16(&mut bytes, 8, UPDATE_MANIFEST_VERSION);
        put_u16(&mut bytes, 10, UPDATE_MANIFEST_SIGNED_SIZE as u16);
        put_u32(&mut bytes, 12, UPDATE_MANIFEST_SIZE as u32);
        bytes[16] = self.payload_kind as u8;
        bytes[17] = 1; // SHA-256
        bytes[18] = 1; // Ed25519
        bytes[19] = 0; // no package-level compression
        bytes[32..48].copy_from_slice(&self.platform_id);
        bytes[48..64].copy_from_slice(&self.key_id);
        bytes[64..80].copy_from_slice(&self.base_release_id);
        bytes[80..96].copy_from_slice(&self.target_release_id);
        put_u64(&mut bytes, 96, self.base_security_version);
        put_u64(&mut bytes, 104, self.target_security_version);
        put_u64(&mut bytes, 112, self.source_date_epoch);
        put_u64(&mut bytes, 120, self.payload_length);
        put_u64(&mut bytes, 128, self.base_image_length);
        put_u64(&mut bytes, 136, self.target_image_length);
        bytes[144..176].copy_from_slice(&self.base_image_digest);
        bytes[176..208].copy_from_slice(&self.target_image_digest);
        bytes[208..240].copy_from_slice(&self.payload_digest);
        bytes[240..256].copy_from_slice(&self.update_id);
        Ok(bytes)
    }

    pub fn encode_signed(&self) -> Result<[u8; UPDATE_MANIFEST_SIZE], UpdateManifestError> {
        self.validate(true)?;
        let unsigned = self.encode_unsigned()?;
        let mut bytes = [0u8; UPDATE_MANIFEST_SIZE];
        bytes[..UPDATE_MANIFEST_SIGNED_SIZE].copy_from_slice(&unsigned);
        bytes[UPDATE_MANIFEST_SIGNED_SIZE..].copy_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, UpdateManifestError> {
        if bytes.len() != UPDATE_MANIFEST_SIZE {
            return Err(UpdateManifestError::Length);
        }
        if bytes[0..8] != UPDATE_MANIFEST_MAGIC {
            return Err(UpdateManifestError::Magic);
        }
        if get_u16(bytes, 8) != UPDATE_MANIFEST_VERSION
            || usize::from(get_u16(bytes, 10)) != UPDATE_MANIFEST_SIGNED_SIZE
            || usize::try_from(get_u32(bytes, 12)).map_err(|_| UpdateManifestError::Length)?
                != UPDATE_MANIFEST_SIZE
        {
            return Err(UpdateManifestError::Version);
        }
        if bytes[17] != 1 || bytes[18] != 1 || bytes[19] != 0 {
            return Err(UpdateManifestError::Algorithm);
        }
        if bytes[20..32].iter().any(|byte| *byte != 0) {
            return Err(UpdateManifestError::Reserved);
        }
        let manifest = Self {
            payload_kind: UpdatePayloadKind::try_from(bytes[16])?,
            platform_id: array_16(bytes, 32)?,
            key_id: array_16(bytes, 48)?,
            base_release_id: array_16(bytes, 64)?,
            target_release_id: array_16(bytes, 80)?,
            base_security_version: get_u64(bytes, 96),
            target_security_version: get_u64(bytes, 104),
            source_date_epoch: get_u64(bytes, 112),
            payload_length: get_u64(bytes, 120),
            base_image_length: get_u64(bytes, 128),
            target_image_length: get_u64(bytes, 136),
            base_image_digest: array_32(bytes, 144)?,
            target_image_digest: array_32(bytes, 176)?,
            payload_digest: array_32(bytes, 208)?,
            update_id: array_16(bytes, 240)?,
            signature: bytes[UPDATE_MANIFEST_SIGNED_SIZE..]
                .try_into()
                .map_err(|_| UpdateManifestError::Length)?,
        };
        manifest.validate(true)?;
        if manifest.encode_signed()?.as_slice() != bytes {
            return Err(UpdateManifestError::NonCanonical);
        }
        Ok(manifest)
    }

    pub fn signed_region(bytes: &[u8]) -> Result<&[u8], UpdateManifestError> {
        Self::decode(bytes)?;
        Ok(&bytes[..UPDATE_MANIFEST_SIGNED_SIZE])
    }

    fn validate(&self, signed: bool) -> Result<(), UpdateManifestError> {
        if zero(&self.platform_id)
            || zero(&self.key_id)
            || zero(&self.base_release_id)
            || zero(&self.target_release_id)
            || self.base_release_id == self.target_release_id
            || zero(&self.base_image_digest)
            || zero(&self.target_image_digest)
            || zero(&self.payload_digest)
            || zero(&self.update_id)
        {
            return Err(UpdateManifestError::Identity);
        }
        if self.base_security_version == 0
            || self.target_security_version <= self.base_security_version
        {
            return Err(UpdateManifestError::SecurityVersion);
        }
        if self.source_date_epoch == 0 {
            return Err(UpdateManifestError::Epoch);
        }
        if self.payload_length == 0
            || self.base_image_length == 0
            || self.target_image_length == 0
            || self.payload_length > MAX_IMAGE_SIZE
            || self.base_image_length > MAX_IMAGE_SIZE
            || self.target_image_length > MAX_IMAGE_SIZE
        {
            return Err(UpdateManifestError::PayloadLength);
        }
        if self.payload_kind == UpdatePayloadKind::Full
            && (self.payload_length != self.target_image_length
                || self.payload_digest != self.target_image_digest)
        {
            return Err(UpdateManifestError::FullPayloadMismatch);
        }
        if signed && zero(&self.signature) {
            return Err(UpdateManifestError::Signature);
        }
        Ok(())
    }
}

fn zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

fn array_16(bytes: &[u8], offset: usize) -> Result<[u8; 16], UpdateManifestError> {
    bytes[offset..offset + 16]
        .try_into()
        .map_err(|_| UpdateManifestError::Length)
}

fn array_32(bytes: &[u8], offset: usize) -> Result<[u8; 32], UpdateManifestError> {
    bytes[offset..offset + 32]
        .try_into()
        .map_err(|_| UpdateManifestError::Length)
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
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap_or([0; 2]))
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap_or([0; 4]))
}

fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap_or([0; 8]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(kind: UpdatePayloadKind) -> UpdateManifest {
        let target_digest = [10; 32];
        UpdateManifest::new(
            kind,
            [1; 16],
            [2; 16],
            [3; 16],
            [4; 16],
            7,
            8,
            1_785_465_257,
            if kind == UpdatePayloadKind::Full {
                128 * 1024 * 1024
            } else {
                4096
            },
            128 * 1024 * 1024,
            128 * 1024 * 1024,
            [9; 32],
            target_digest,
            if kind == UpdatePayloadKind::Full {
                target_digest
            } else {
                [11; 32]
            },
            [12; 16],
        )
        .with_signature([13; 64])
    }

    #[test]
    fn canonical_full_and_delta_manifests_roundtrip() {
        for kind in [UpdatePayloadKind::Full, UpdatePayloadKind::Delta] {
            let manifest = sample(kind);
            let encoded = manifest.encode_signed().unwrap();
            assert_eq!(UpdateManifest::decode(&encoded), Ok(manifest));
            assert_eq!(&encoded[..8], b"ENRUPD01");
            assert_eq!(UpdateManifest::signed_region(&encoded).unwrap().len(), 256);
        }
    }

    #[test]
    fn rollback_reserved_signature_and_full_payload_mismatch_fail_closed() {
        let mut manifest = sample(UpdatePayloadKind::Full);
        manifest.target_security_version = manifest.base_security_version;
        assert_eq!(
            manifest.encode_signed(),
            Err(UpdateManifestError::SecurityVersion)
        );

        manifest = sample(UpdatePayloadKind::Full);
        manifest.payload_digest[0] ^= 1;
        assert_eq!(
            manifest.encode_signed(),
            Err(UpdateManifestError::FullPayloadMismatch)
        );

        manifest = sample(UpdatePayloadKind::Delta);
        manifest.signature = [0; 64];
        assert_eq!(
            manifest.encode_signed(),
            Err(UpdateManifestError::Signature)
        );

        let mut encoded = sample(UpdatePayloadKind::Delta).encode_signed().unwrap();
        encoded[20] = 1;
        assert_eq!(
            UpdateManifest::decode(&encoded),
            Err(UpdateManifestError::Reserved)
        );
    }
}
