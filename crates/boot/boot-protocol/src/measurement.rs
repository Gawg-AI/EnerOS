use sha2::{Digest, Sha256};

pub const MEASUREMENT_LOG_MAGIC: [u8; 8] = *b"ENRMEAS1";
pub const MEASUREMENT_LOG_VERSION: u16 = 1;
pub const MEASUREMENT_LOG_HEADER_SIZE: usize = 128;
pub const MEASUREMENT_RECORD_SIZE: usize = 96;
pub const MEASUREMENT_MAX_RECORDS: usize = 8;
pub const MEASUREMENT_LOG_SIZE: usize = 1024;

const CRC_OFFSET: usize = MEASUREMENT_LOG_SIZE - 4;
const RECORD_FLAG_VERIFIED: u16 = 1;

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MeasurementKind {
    BootPolicy = 1,
    BootControl = 2,
    Manifest = 3,
    Kernel = 4,
    InitBundle = 5,
    Recovery = 6,
}

impl TryFrom<u16> for MeasurementKind {
    type Error = MeasurementError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::BootPolicy),
            2 => Ok(Self::BootControl),
            3 => Ok(Self::Manifest),
            4 => Ok(Self::Kernel),
            5 => Ok(Self::InitBundle),
            6 => Ok(Self::Recovery),
            _ => Err(MeasurementError::Kind),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasurementRecord {
    pub kind: MeasurementKind,
    pub security_version: u64,
    pub identity: [u8; 16],
    pub data_digest: [u8; 32],
    pub chain_digest: [u8; 32],
}

impl MeasurementRecord {
    const EMPTY: Self = Self {
        kind: MeasurementKind::BootPolicy,
        security_version: 0,
        identity: [0; 16],
        data_digest: [0; 32],
        chain_digest: [0; 32],
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeasurementLog {
    pub boot_nonce: [u8; 32],
    pub final_digest: [u8; 32],
    records: [MeasurementRecord; MEASUREMENT_MAX_RECORDS],
    count: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementError {
    Length,
    Magic,
    Version,
    Count,
    Reserved,
    Crc,
    Nonce,
    Kind,
    Order,
    Identity,
    SecurityVersion,
    Digest,
    Chain,
}

impl MeasurementLog {
    pub fn new(boot_nonce: [u8; 32]) -> Result<Self, MeasurementError> {
        if is_zero(&boot_nonce) {
            return Err(MeasurementError::Nonce);
        }
        Ok(Self {
            boot_nonce,
            final_digest: initial_digest(&boot_nonce),
            records: [MeasurementRecord::EMPTY; MEASUREMENT_MAX_RECORDS],
            count: 0,
        })
    }

    pub const fn record_count(&self) -> u16 {
        self.count
    }

    pub fn records(&self) -> &[MeasurementRecord] {
        &self.records[..usize::from(self.count)]
    }

    pub fn extend(
        &mut self,
        kind: MeasurementKind,
        security_version: u64,
        identity: [u8; 16],
        data_digest: [u8; 32],
    ) -> Result<(), MeasurementError> {
        let index = usize::from(self.count);
        if index >= MEASUREMENT_MAX_RECORDS {
            return Err(MeasurementError::Count);
        }
        if index != 0 && self.records[index - 1].kind >= kind {
            return Err(MeasurementError::Order);
        }
        if security_version == 0 {
            return Err(MeasurementError::SecurityVersion);
        }
        if is_zero(&identity) {
            return Err(MeasurementError::Identity);
        }
        if is_zero(&data_digest) {
            return Err(MeasurementError::Digest);
        }
        let chain_digest = extend_digest(
            &self.final_digest,
            kind,
            security_version,
            &identity,
            &data_digest,
        );
        self.records[index] = MeasurementRecord {
            kind,
            security_version,
            identity,
            data_digest,
            chain_digest,
        };
        self.final_digest = chain_digest;
        self.count += 1;
        Ok(())
    }

    pub fn quote_digest(&self, remote_nonce: &[u8; 32]) -> Result<[u8; 32], MeasurementError> {
        if is_zero(remote_nonce) || self.count == 0 {
            return Err(MeasurementError::Nonce);
        }
        let mut hash = Sha256::new();
        hash.update(b"EnerOS/measurement-quote/v1");
        hash.update(self.final_digest);
        hash.update(self.boot_nonce);
        hash.update(remote_nonce);
        Ok(hash.finalize().into())
    }

    pub fn encode(&self) -> Result<[u8; MEASUREMENT_LOG_SIZE], MeasurementError> {
        self.validate()?;
        let mut bytes = [0u8; MEASUREMENT_LOG_SIZE];
        bytes[0..8].copy_from_slice(&MEASUREMENT_LOG_MAGIC);
        put_u16(&mut bytes, 8, MEASUREMENT_LOG_VERSION);
        put_u16(&mut bytes, 10, MEASUREMENT_LOG_HEADER_SIZE as u16);
        put_u32(&mut bytes, 12, MEASUREMENT_LOG_SIZE as u32);
        put_u16(&mut bytes, 16, self.count);
        put_u16(&mut bytes, 18, MEASUREMENT_MAX_RECORDS as u16);
        bytes[32..64].copy_from_slice(&self.boot_nonce);
        bytes[64..96].copy_from_slice(&self.final_digest);
        for (index, record) in self.records().iter().enumerate() {
            let offset = MEASUREMENT_LOG_HEADER_SIZE + index * MEASUREMENT_RECORD_SIZE;
            encode_record(
                record,
                u32::try_from(index).map_err(|_| MeasurementError::Count)?,
                &mut bytes[offset..offset + MEASUREMENT_RECORD_SIZE],
            );
        }
        let crc = crc32(&bytes[..CRC_OFFSET]);
        put_u32(&mut bytes, CRC_OFFSET, crc);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MeasurementError> {
        if bytes.len() != MEASUREMENT_LOG_SIZE {
            return Err(MeasurementError::Length);
        }
        if bytes[0..8] != MEASUREMENT_LOG_MAGIC {
            return Err(MeasurementError::Magic);
        }
        if get_u16(bytes, 8) != MEASUREMENT_LOG_VERSION
            || usize::from(get_u16(bytes, 10)) != MEASUREMENT_LOG_HEADER_SIZE
            || usize::try_from(get_u32(bytes, 12)).map_err(|_| MeasurementError::Length)?
                != MEASUREMENT_LOG_SIZE
            || usize::from(get_u16(bytes, 18)) != MEASUREMENT_MAX_RECORDS
        {
            return Err(MeasurementError::Version);
        }
        if crc32(&bytes[..CRC_OFFSET]) != get_u32(bytes, CRC_OFFSET) {
            return Err(MeasurementError::Crc);
        }
        let count = usize::from(get_u16(bytes, 16));
        if count == 0 || count > MEASUREMENT_MAX_RECORDS {
            return Err(MeasurementError::Count);
        }
        if !is_zero(&bytes[20..32])
            || !is_zero(&bytes[96..MEASUREMENT_LOG_HEADER_SIZE])
            || !is_zero(
                &bytes[MEASUREMENT_LOG_HEADER_SIZE + count * MEASUREMENT_RECORD_SIZE..CRC_OFFSET],
            )
        {
            return Err(MeasurementError::Reserved);
        }
        let boot_nonce: [u8; 32] = bytes[32..64]
            .try_into()
            .map_err(|_| MeasurementError::Length)?;
        let expected_final: [u8; 32] = bytes[64..96]
            .try_into()
            .map_err(|_| MeasurementError::Length)?;
        let mut log = Self::new(boot_nonce)?;
        for index in 0..count {
            let offset = MEASUREMENT_LOG_HEADER_SIZE + index * MEASUREMENT_RECORD_SIZE;
            let record = decode_record(
                &bytes[offset..offset + MEASUREMENT_RECORD_SIZE],
                u32::try_from(index).map_err(|_| MeasurementError::Count)?,
            )?;
            log.extend(
                record.kind,
                record.security_version,
                record.identity,
                record.data_digest,
            )?;
            if log.records[index].chain_digest != record.chain_digest {
                return Err(MeasurementError::Chain);
            }
        }
        if log.final_digest != expected_final {
            return Err(MeasurementError::Chain);
        }
        if log.encode()?.as_slice() != bytes {
            return Err(MeasurementError::Reserved);
        }
        Ok(log)
    }

    fn validate(&self) -> Result<(), MeasurementError> {
        if self.count == 0 || usize::from(self.count) > MEASUREMENT_MAX_RECORDS {
            return Err(MeasurementError::Count);
        }
        if is_zero(&self.boot_nonce) {
            return Err(MeasurementError::Nonce);
        }
        let mut previous = initial_digest(&self.boot_nonce);
        let mut previous_kind = None;
        for record in self.records() {
            if previous_kind.is_some_and(|kind| kind >= record.kind) {
                return Err(MeasurementError::Order);
            }
            let expected = extend_digest(
                &previous,
                record.kind,
                record.security_version,
                &record.identity,
                &record.data_digest,
            );
            if expected != record.chain_digest {
                return Err(MeasurementError::Chain);
            }
            previous = expected;
            previous_kind = Some(record.kind);
        }
        if previous != self.final_digest {
            return Err(MeasurementError::Chain);
        }
        Ok(())
    }
}

fn encode_record(record: &MeasurementRecord, index: u32, bytes: &mut [u8]) {
    put_u16(bytes, 0, record.kind as u16);
    put_u16(bytes, 2, RECORD_FLAG_VERIFIED);
    put_u32(bytes, 4, index);
    put_u64(bytes, 8, record.security_version);
    bytes[16..32].copy_from_slice(&record.identity);
    bytes[32..64].copy_from_slice(&record.data_digest);
    bytes[64..96].copy_from_slice(&record.chain_digest);
}

fn decode_record(bytes: &[u8], expected_index: u32) -> Result<MeasurementRecord, MeasurementError> {
    if get_u16(bytes, 2) != RECORD_FLAG_VERIFIED || get_u32(bytes, 4) != expected_index {
        return Err(MeasurementError::Order);
    }
    let record = MeasurementRecord {
        kind: MeasurementKind::try_from(get_u16(bytes, 0))?,
        security_version: get_u64(bytes, 8),
        identity: bytes[16..32]
            .try_into()
            .map_err(|_| MeasurementError::Length)?,
        data_digest: bytes[32..64]
            .try_into()
            .map_err(|_| MeasurementError::Length)?,
        chain_digest: bytes[64..96]
            .try_into()
            .map_err(|_| MeasurementError::Length)?,
    };
    if record.security_version == 0 {
        return Err(MeasurementError::SecurityVersion);
    }
    if is_zero(&record.identity) {
        return Err(MeasurementError::Identity);
    }
    if is_zero(&record.data_digest) || is_zero(&record.chain_digest) {
        return Err(MeasurementError::Digest);
    }
    Ok(record)
}

fn initial_digest(boot_nonce: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"EnerOS/measurement-chain/initial/v1");
    hash.update(boot_nonce);
    hash.finalize().into()
}

fn extend_digest(
    previous: &[u8; 32],
    kind: MeasurementKind,
    security_version: u64,
    identity: &[u8; 16],
    data_digest: &[u8; 32],
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"EnerOS/measurement-chain/extend/v1");
    hash.update(previous);
    hash.update((kind as u16).to_le_bytes());
    hash.update(security_version.to_le_bytes());
    hash.update(identity);
    hash.update(data_digest);
    hash.finalize().into()
}

fn crc32(bytes: &[u8]) -> u32 {
    // HYG-001：CRC32 唯一实现归一到 abi/ipc-wire（IEEE 802.3 0xEDB88320）。
    eneros_ipc_wire::crc32(bytes)
}

fn is_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
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
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MeasurementLog {
        let mut log = MeasurementLog::new([0x5a; 32]).unwrap();
        for (kind, value) in [
            (MeasurementKind::BootPolicy, 1),
            (MeasurementKind::BootControl, 2),
            (MeasurementKind::Manifest, 3),
            (MeasurementKind::Kernel, 4),
            (MeasurementKind::InitBundle, 5),
        ] {
            log.extend(kind, 7, [value; 16], [value; 32]).unwrap();
        }
        log
    }

    #[test]
    fn canonical_five_record_chain_roundtrips() {
        let log = sample();
        let bytes = log.encode().unwrap();
        assert_eq!(bytes.len(), MEASUREMENT_LOG_SIZE);
        assert_eq!(MeasurementLog::decode(&bytes).unwrap(), log);
        assert_eq!(log.record_count(), 5);
    }

    #[test]
    fn remote_nonce_is_bound_to_the_quote() {
        let log = sample();
        let first = log.quote_digest(&[1; 32]).unwrap();
        let second = log.quote_digest(&[2; 32]).unwrap();
        assert_ne!(first, second);
        assert_eq!(log.quote_digest(&[0; 32]), Err(MeasurementError::Nonce));
    }

    #[test]
    fn every_encoded_byte_mutation_is_rejected() {
        let bytes = sample().encode().unwrap();
        for index in 0..bytes.len() {
            let mut mutated = bytes;
            mutated[index] ^= 1;
            assert!(
                MeasurementLog::decode(&mutated).is_err(),
                "accepted byte {index}"
            );
        }
    }

    #[test]
    fn order_identity_digest_and_count_fail_closed() {
        let mut log = MeasurementLog::new([7; 32]).unwrap();
        log.extend(MeasurementKind::Manifest, 1, [1; 16], [1; 32])
            .unwrap();
        assert_eq!(
            log.extend(MeasurementKind::BootPolicy, 1, [2; 16], [2; 32]),
            Err(MeasurementError::Order)
        );
        assert_eq!(
            log.extend(MeasurementKind::Kernel, 1, [0; 16], [3; 32]),
            Err(MeasurementError::Identity)
        );
        assert_eq!(MeasurementLog::new([0; 32]), Err(MeasurementError::Nonce));
    }
}
