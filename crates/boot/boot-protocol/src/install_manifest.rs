pub const INSTALL_MANIFEST_MAGIC: [u8; 8] = *b"ENRINST1";
pub const INSTALL_MANIFEST_VERSION: u16 = 1;
pub const INSTALL_MANIFEST_SIGNED_SIZE: usize = 160;
pub const INSTALL_MANIFEST_SIGNATURE_SIZE: usize = 64;
pub const INSTALL_MANIFEST_SIZE: usize = 256;

const MIN_PAYLOAD_SIZE: u64 = 16 * 1024 * 1024;
const MAX_PAYLOAD_SIZE: u64 = 1024 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallManifest {
    pub platform_id: [u8; 16],
    pub key_id: [u8; 16],
    pub security_version: u64,
    pub source_date_epoch: u64,
    pub payload_length: u64,
    pub sector_size: u32,
    pub disk_guid: [u8; 16],
    pub release_id: [u8; 16],
    pub payload_digest: [u8; 32],
    pub signature: [u8; INSTALL_MANIFEST_SIGNATURE_SIZE],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallManifestError {
    Length,
    Magic,
    Version,
    Reserved,
    Identity,
    SecurityVersion,
    Epoch,
    PayloadLength,
    SectorSize,
    Signature,
}

impl InstallManifest {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        platform_id: [u8; 16],
        key_id: [u8; 16],
        security_version: u64,
        source_date_epoch: u64,
        payload_length: u64,
        sector_size: u32,
        disk_guid: [u8; 16],
        release_id: [u8; 16],
        payload_digest: [u8; 32],
    ) -> Self {
        Self {
            platform_id,
            key_id,
            security_version,
            source_date_epoch,
            payload_length,
            sector_size,
            disk_guid,
            release_id,
            payload_digest,
            signature: [0; INSTALL_MANIFEST_SIGNATURE_SIZE],
        }
    }

    pub fn with_signature(mut self, signature: [u8; INSTALL_MANIFEST_SIGNATURE_SIZE]) -> Self {
        self.signature = signature;
        self
    }

    pub fn encode_unsigned(
        &self,
    ) -> Result<[u8; INSTALL_MANIFEST_SIGNED_SIZE], InstallManifestError> {
        self.validate(false)?;
        let mut bytes = [0u8; INSTALL_MANIFEST_SIGNED_SIZE];
        bytes[0..8].copy_from_slice(&INSTALL_MANIFEST_MAGIC);
        put_u16(&mut bytes, 8, INSTALL_MANIFEST_VERSION);
        put_u16(&mut bytes, 10, INSTALL_MANIFEST_SIGNED_SIZE as u16);
        put_u32(&mut bytes, 12, INSTALL_MANIFEST_SIZE as u32);
        bytes[32..48].copy_from_slice(&self.platform_id);
        bytes[48..64].copy_from_slice(&self.key_id);
        put_u64(&mut bytes, 64, self.security_version);
        put_u64(&mut bytes, 72, self.source_date_epoch);
        put_u64(&mut bytes, 80, self.payload_length);
        put_u32(&mut bytes, 88, self.sector_size);
        bytes[96..112].copy_from_slice(&self.disk_guid);
        bytes[112..128].copy_from_slice(&self.release_id);
        bytes[128..160].copy_from_slice(&self.payload_digest);
        Ok(bytes)
    }

    pub fn encode_signed(&self) -> Result<[u8; INSTALL_MANIFEST_SIZE], InstallManifestError> {
        self.validate(true)?;
        let unsigned = self.encode_unsigned()?;
        let mut bytes = [0u8; INSTALL_MANIFEST_SIZE];
        bytes[..INSTALL_MANIFEST_SIGNED_SIZE].copy_from_slice(&unsigned);
        bytes[INSTALL_MANIFEST_SIGNED_SIZE
            ..INSTALL_MANIFEST_SIGNED_SIZE + INSTALL_MANIFEST_SIGNATURE_SIZE]
            .copy_from_slice(&self.signature);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, InstallManifestError> {
        if bytes.len() != INSTALL_MANIFEST_SIZE {
            return Err(InstallManifestError::Length);
        }
        if bytes[0..8] != INSTALL_MANIFEST_MAGIC {
            return Err(InstallManifestError::Magic);
        }
        if get_u16(bytes, 8) != INSTALL_MANIFEST_VERSION
            || usize::from(get_u16(bytes, 10)) != INSTALL_MANIFEST_SIGNED_SIZE
            || usize::try_from(get_u32(bytes, 12)).map_err(|_| InstallManifestError::Length)?
                != INSTALL_MANIFEST_SIZE
        {
            return Err(InstallManifestError::Version);
        }
        if !is_zero(&bytes[16..32])
            || !is_zero(&bytes[92..96])
            || !is_zero(&bytes[224..INSTALL_MANIFEST_SIZE])
        {
            return Err(InstallManifestError::Reserved);
        }
        let manifest = Self {
            platform_id: array(bytes, 32)?,
            key_id: array(bytes, 48)?,
            security_version: get_u64(bytes, 64),
            source_date_epoch: get_u64(bytes, 72),
            payload_length: get_u64(bytes, 80),
            sector_size: get_u32(bytes, 88),
            disk_guid: array(bytes, 96)?,
            release_id: array(bytes, 112)?,
            payload_digest: bytes[128..160]
                .try_into()
                .map_err(|_| InstallManifestError::Length)?,
            signature: bytes[160..224]
                .try_into()
                .map_err(|_| InstallManifestError::Length)?,
        };
        manifest.validate(true)?;
        if manifest.encode_signed()?.as_slice() != bytes {
            return Err(InstallManifestError::Reserved);
        }
        Ok(manifest)
    }

    pub fn signed_region(bytes: &[u8]) -> Result<&[u8], InstallManifestError> {
        Self::decode(bytes)?;
        Ok(&bytes[..INSTALL_MANIFEST_SIGNED_SIZE])
    }

    fn validate(&self, signed: bool) -> Result<(), InstallManifestError> {
        if is_zero(&self.platform_id)
            || is_zero(&self.key_id)
            || is_zero(&self.disk_guid)
            || is_zero(&self.release_id)
            || is_zero(&self.payload_digest)
        {
            return Err(InstallManifestError::Identity);
        }
        if self.security_version == 0 {
            return Err(InstallManifestError::SecurityVersion);
        }
        if self.source_date_epoch == 0 {
            return Err(InstallManifestError::Epoch);
        }
        if !matches!(self.sector_size, 512 | 4096) {
            return Err(InstallManifestError::SectorSize);
        }
        if !(MIN_PAYLOAD_SIZE..=MAX_PAYLOAD_SIZE).contains(&self.payload_length)
            || self.payload_length % u64::from(self.sector_size) != 0
        {
            return Err(InstallManifestError::PayloadLength);
        }
        if signed && is_zero(&self.signature) {
            return Err(InstallManifestError::Signature);
        }
        Ok(())
    }
}

fn is_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

fn array(bytes: &[u8], offset: usize) -> Result<[u8; 16], InstallManifestError> {
    bytes[offset..offset + 16]
        .try_into()
        .map_err(|_| InstallManifestError::Length)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> InstallManifest {
        InstallManifest::new(
            [1; 16],
            [2; 16],
            7,
            1_785_465_257,
            128 * 1024 * 1024,
            512,
            [3; 16],
            [4; 16],
            [5; 32],
        )
        .with_signature([6; 64])
    }

    #[test]
    fn canonical_install_manifest_roundtrips() {
        let encoded = sample().encode_signed().unwrap();
        assert_eq!(InstallManifest::decode(&encoded).unwrap(), sample());
        assert_eq!(&encoded[..8], b"ENRINST1");
        assert_eq!(InstallManifest::signed_region(&encoded).unwrap().len(), 160);
    }

    #[test]
    fn reserved_identity_size_sector_and_signature_fail_closed() {
        let encoded = sample().encode_signed().unwrap();
        for index in [16, 31, 92, 95, 224, 255] {
            let mut changed = encoded;
            changed[index] ^= 1;
            assert!(InstallManifest::decode(&changed).is_err());
        }
        let mut bad = sample();
        bad.payload_length += 1;
        assert_eq!(
            bad.encode_signed(),
            Err(InstallManifestError::PayloadLength)
        );
        bad = sample();
        bad.signature = [0; 64];
        assert_eq!(bad.encode_signed(), Err(InstallManifestError::Signature));
    }
}
