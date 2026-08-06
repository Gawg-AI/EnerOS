use core::cmp::min;

use crate::media::{BlockDevice, BlockReadError};

const GPT_SIGNATURE: [u8; 8] = *b"EFI PART";
const GPT_REVISION_1_0: u32 = 0x0001_0000;
const GPT_HEADER_SIZE: usize = 92;
const GPT_ENTRY_SIZE: usize = 128;
const GPT_ENTRY_COUNT: usize = 128;
const GPT_ENTRIES_BYTES: usize = GPT_ENTRY_SIZE * GPT_ENTRY_COUNT;
const PROTECTIVE_MBR_TYPE: u8 = 0xEE;

pub const GPT_MAX_PRODUCT_PARTITIONS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    pub const ZERO: Self = Self([0; 16]);

    pub fn parse(value: &str) -> Result<Self, GptError> {
        if value.len() != 36 {
            return Err(GptError::Guid);
        }
        let bytes = value.as_bytes();
        for index in [8usize, 13, 18, 23] {
            if bytes[index] != b'-' {
                return Err(GptError::Guid);
            }
        }
        let mut canonical = [0u8; 16];
        let mut nibble = 0usize;
        for (index, byte) in bytes.iter().enumerate() {
            if matches!(index, 8 | 13 | 18 | 23) {
                continue;
            }
            let value = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                _ => return Err(GptError::Guid),
            };
            if nibble & 1 == 0 {
                canonical[nibble / 2] = value << 4;
            } else {
                canonical[nibble / 2] |= value;
            }
            nibble += 1;
        }
        if canonical == [0; 16] {
            return Err(GptError::Guid);
        }
        Ok(Self(canonical))
    }

    fn to_disk_bytes(self) -> [u8; 16] {
        let source = self.0;
        [
            source[3], source[2], source[1], source[0], source[5], source[4], source[7], source[6],
            source[8], source[9], source[10], source[11], source[12], source[13], source[14],
            source[15],
        ]
    }

    fn from_disk_bytes(source: &[u8]) -> Self {
        Self([
            source[3], source[2], source[1], source[0], source[5], source[4], source[7], source[6],
            source[8], source[9], source[10], source[11], source[12], source[13], source[14],
            source[15],
        ])
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PartitionSpec<'a> {
    pub type_guid: Guid,
    pub unique_guid: Guid,
    pub first_lba: u64,
    pub last_lba: u64,
    pub attributes: u64,
    pub name: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedPartition {
    pub type_guid: Guid,
    pub unique_guid: Guid,
    pub first_lba: u64,
    pub last_lba: u64,
    pub attributes: u64,
    pub name: [u16; 36],
    pub name_len: u8,
}

impl ParsedPartition {
    const EMPTY: Self = Self {
        type_guid: Guid::ZERO,
        unique_guid: Guid::ZERO,
        first_lba: 0,
        last_lba: 0,
        attributes: 0,
        name: [0; 36],
        name_len: 0,
    };

    pub fn name_matches(&self, expected: &str) -> bool {
        let mut encoded = [0u16; 36];
        let mut count = 0usize;
        for unit in expected.encode_utf16() {
            if count == encoded.len() {
                return false;
            }
            encoded[count] = unit;
            count += 1;
        }
        count == usize::from(self.name_len) && self.name[..count] == encoded[..count]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gpt {
    pub sector_size: u32,
    pub total_lbas: u64,
    pub disk_guid: Guid,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub partitions: [ParsedPartition; GPT_MAX_PRODUCT_PARTITIONS],
    pub partition_count: u8,
}

impl Gpt {
    pub fn partition(&self, index: usize) -> Option<&ParsedPartition> {
        if index < usize::from(self.partition_count) {
            Some(&self.partitions[index])
        } else {
            None
        }
    }

    pub fn find_by_name(&self, name: &str) -> Option<&ParsedPartition> {
        self.partitions[..usize::from(self.partition_count)]
            .iter()
            .find(|partition| partition.name_matches(name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GptError {
    SectorSize,
    ImageLength,
    Guid,
    ProtectiveMbr,
    Header,
    HeaderCrc,
    EntryCrc,
    BackupMismatch,
    PartitionCount,
    PartitionBounds,
    PartitionOverlap,
    DuplicateGuid,
    Name,
    NonCanonical,
    Overflow,
    MediaRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Header {
    current_lba: u64,
    backup_lba: u64,
    first_usable_lba: u64,
    last_usable_lba: u64,
    disk_guid: Guid,
    entries_lba: u64,
    entries_crc: u32,
}

pub fn write_gpt(
    image: &mut [u8],
    sector_size: u32,
    disk_guid: Guid,
    partitions: &[PartitionSpec<'_>],
) -> Result<(), GptError> {
    let geometry = Geometry::new(image.len(), sector_size)?;
    if disk_guid == Guid::ZERO {
        return Err(GptError::Guid);
    }
    if partitions.is_empty() || partitions.len() > GPT_MAX_PRODUCT_PARTITIONS {
        return Err(GptError::PartitionCount);
    }
    validate_specs(partitions, geometry.first_usable, geometry.last_usable)?;

    let sector = usize::try_from(sector_size).map_err(|_| GptError::SectorSize)?;
    image[..sector].fill(0);
    write_protective_mbr(&mut image[..sector], geometry.total_lbas)?;

    let primary_entries_offset = geometry.byte_offset(geometry.primary_entries_lba)?;
    let primary_entries_end = primary_entries_offset
        .checked_add(GPT_ENTRIES_BYTES)
        .ok_or(GptError::Overflow)?;
    image[primary_entries_offset..primary_entries_end].fill(0);
    for (index, partition) in partitions.iter().enumerate() {
        let start = primary_entries_offset + index * GPT_ENTRY_SIZE;
        encode_entry(partition, &mut image[start..start + GPT_ENTRY_SIZE])?;
    }
    let entries_crc = crc32(&image[primary_entries_offset..primary_entries_end]);

    let backup_entries_offset = geometry.byte_offset(geometry.backup_entries_lba)?;
    let backup_entries_end = backup_entries_offset
        .checked_add(GPT_ENTRIES_BYTES)
        .ok_or(GptError::Overflow)?;
    image.copy_within(
        primary_entries_offset..primary_entries_end,
        backup_entries_offset,
    );
    if backup_entries_end > image.len() {
        return Err(GptError::ImageLength);
    }

    let primary_header_offset = geometry.byte_offset(1)?;
    write_header(
        &mut image[primary_header_offset..primary_header_offset + sector],
        Header {
            current_lba: 1,
            backup_lba: geometry.backup_header_lba,
            first_usable_lba: geometry.first_usable,
            last_usable_lba: geometry.last_usable,
            disk_guid,
            entries_lba: geometry.primary_entries_lba,
            entries_crc,
        },
    );
    let backup_header_offset = geometry.byte_offset(geometry.backup_header_lba)?;
    write_header(
        &mut image[backup_header_offset..backup_header_offset + sector],
        Header {
            current_lba: geometry.backup_header_lba,
            backup_lba: 1,
            first_usable_lba: geometry.first_usable,
            last_usable_lba: geometry.last_usable,
            disk_guid,
            entries_lba: geometry.backup_entries_lba,
            entries_crc,
        },
    );
    Ok(())
}

pub fn parse_gpt(image: &[u8], sector_size: u32) -> Result<Gpt, GptError> {
    let geometry = Geometry::new(image.len(), sector_size)?;
    let sector = usize::try_from(sector_size).map_err(|_| GptError::SectorSize)?;
    parse_regions(
        &geometry,
        &image[..sector],
        sector_at(image, &geometry, 1)?,
        entries_at(image, &geometry, geometry.primary_entries_lba)?,
        entries_at(image, &geometry, geometry.backup_entries_lba)?,
        sector_at(image, &geometry, geometry.backup_header_lba)?,
    )
}

/// Parse GPT directly from a bounded block device without allocating from
/// attacker-controlled lengths or reading the full disk into memory.
pub fn read_gpt<D: BlockDevice>(device: &mut D) -> Result<Gpt, GptError> {
    let sector_size = device.sector_size();
    let geometry = Geometry::from_lbas(device.total_lbas(), sector_size)?;
    let sector = usize::try_from(sector_size).map_err(|_| GptError::SectorSize)?;
    let mut mbr = [0u8; 4096];
    let mut primary_header = [0u8; 4096];
    let mut backup_header = [0u8; 4096];
    let mut primary_entries = [0u8; GPT_ENTRIES_BYTES];
    let mut backup_entries = [0u8; GPT_ENTRIES_BYTES];

    read_exact_blocks(device, 0, &mut mbr[..sector])?;
    read_exact_blocks(device, 1, &mut primary_header[..sector])?;
    read_exact_blocks(device, geometry.primary_entries_lba, &mut primary_entries)?;
    read_exact_blocks(device, geometry.backup_entries_lba, &mut backup_entries)?;
    read_exact_blocks(
        device,
        geometry.backup_header_lba,
        &mut backup_header[..sector],
    )?;

    parse_regions(
        &geometry,
        &mbr[..sector],
        &primary_header[..sector],
        &primary_entries,
        &backup_entries,
        &backup_header[..sector],
    )
}

fn read_exact_blocks<D: BlockDevice>(
    device: &mut D,
    lba: u64,
    destination: &mut [u8],
) -> Result<(), GptError> {
    device
        .read_blocks(lba, destination)
        .map_err(|error| match error {
            BlockReadError::SectorSize => GptError::SectorSize,
            BlockReadError::BufferSize | BlockReadError::OutOfBounds => GptError::ImageLength,
            BlockReadError::Overflow => GptError::Overflow,
            BlockReadError::Media => GptError::MediaRead,
        })
}

fn parse_regions(
    geometry: &Geometry,
    mbr: &[u8],
    primary_header: &[u8],
    primary_entries: &[u8],
    backup_entries: &[u8],
    backup_header: &[u8],
) -> Result<Gpt, GptError> {
    validate_protective_mbr(mbr, geometry.total_lbas)?;
    let primary = parse_header(primary_header)?;
    let backup = parse_header(backup_header)?;
    if primary.current_lba != 1
        || primary.backup_lba != geometry.backup_header_lba
        || primary.entries_lba != geometry.primary_entries_lba
        || backup.current_lba != geometry.backup_header_lba
        || backup.backup_lba != 1
        || backup.entries_lba != geometry.backup_entries_lba
        || primary.first_usable_lba != geometry.first_usable
        || primary.last_usable_lba != geometry.last_usable
        || backup.first_usable_lba != primary.first_usable_lba
        || backup.last_usable_lba != primary.last_usable_lba
        || backup.disk_guid != primary.disk_guid
        || backup.entries_crc != primary.entries_crc
    {
        return Err(GptError::BackupMismatch);
    }

    if primary_entries.len() != GPT_ENTRIES_BYTES || backup_entries.len() != GPT_ENTRIES_BYTES {
        return Err(GptError::ImageLength);
    }
    if crc32(primary_entries) != primary.entries_crc || crc32(backup_entries) != backup.entries_crc
    {
        return Err(GptError::EntryCrc);
    }
    if primary_entries != backup_entries {
        return Err(GptError::BackupMismatch);
    }

    let mut partitions = [ParsedPartition::EMPTY; GPT_MAX_PRODUCT_PARTITIONS];
    let mut count = 0usize;
    let mut saw_unused = false;
    let mut previous_last = None;
    for index in 0..GPT_ENTRY_COUNT {
        let entry = &primary_entries[index * GPT_ENTRY_SIZE..(index + 1) * GPT_ENTRY_SIZE];
        if entry[..16].iter().all(|byte| *byte == 0) {
            if entry.iter().any(|byte| *byte != 0) {
                return Err(GptError::NonCanonical);
            }
            saw_unused = true;
            continue;
        }
        if saw_unused {
            return Err(GptError::NonCanonical);
        }
        if count == GPT_MAX_PRODUCT_PARTITIONS {
            return Err(GptError::PartitionCount);
        }
        let partition = decode_entry(entry)?;
        if partition.first_lba < primary.first_usable_lba
            || partition.last_lba > primary.last_usable_lba
            || partition.first_lba > partition.last_lba
        {
            return Err(GptError::PartitionBounds);
        }
        if let Some(last) = previous_last {
            if partition.first_lba <= last {
                return Err(GptError::PartitionOverlap);
            }
        }
        if partitions[..count]
            .iter()
            .any(|existing| existing.unique_guid == partition.unique_guid)
        {
            return Err(GptError::DuplicateGuid);
        }
        previous_last = Some(partition.last_lba);
        partitions[count] = partition;
        count += 1;
    }
    if count == 0 {
        return Err(GptError::PartitionCount);
    }

    Ok(Gpt {
        sector_size: u32::try_from(geometry.sector_size).map_err(|_| GptError::SectorSize)?,
        total_lbas: geometry.total_lbas,
        disk_guid: primary.disk_guid,
        first_usable_lba: primary.first_usable_lba,
        last_usable_lba: primary.last_usable_lba,
        partitions,
        partition_count: u8::try_from(count).map_err(|_| GptError::PartitionCount)?,
    })
}

#[derive(Debug, Clone, Copy)]
struct Geometry {
    sector_size: usize,
    total_lbas: u64,
    primary_entries_lba: u64,
    backup_entries_lba: u64,
    backup_header_lba: u64,
    first_usable: u64,
    last_usable: u64,
}

impl Geometry {
    fn new(image_len: usize, sector_size: u32) -> Result<Self, GptError> {
        if !matches!(sector_size, 512 | 4096) {
            return Err(GptError::SectorSize);
        }
        let sector_size = usize::try_from(sector_size).map_err(|_| GptError::SectorSize)?;
        if image_len % sector_size != 0 {
            return Err(GptError::ImageLength);
        }
        let total_lbas = u64::try_from(image_len / sector_size).map_err(|_| GptError::Overflow)?;
        Self::from_lbas(
            total_lbas,
            u32::try_from(sector_size).map_err(|_| GptError::SectorSize)?,
        )
    }

    fn from_lbas(total_lbas: u64, sector_size: u32) -> Result<Self, GptError> {
        if !matches!(sector_size, 512 | 4096) {
            return Err(GptError::SectorSize);
        }
        let sector_size = usize::try_from(sector_size).map_err(|_| GptError::SectorSize)?;
        let entry_sectors = GPT_ENTRIES_BYTES.div_ceil(sector_size);
        let entry_sectors_u64 = u64::try_from(entry_sectors).map_err(|_| GptError::Overflow)?;
        let minimum_lbas = 2u64
            .checked_mul(entry_sectors_u64)
            .and_then(|value| value.checked_add(4))
            .ok_or(GptError::Overflow)?;
        if total_lbas < minimum_lbas {
            return Err(GptError::ImageLength);
        }
        let backup_header_lba = total_lbas - 1;
        let backup_entries_lba = backup_header_lba
            .checked_sub(entry_sectors_u64)
            .ok_or(GptError::Overflow)?;
        let first_usable = 2u64
            .checked_add(entry_sectors_u64)
            .ok_or(GptError::Overflow)?;
        let last_usable = backup_entries_lba
            .checked_sub(1)
            .ok_or(GptError::Overflow)?;
        Ok(Self {
            sector_size,
            total_lbas,
            primary_entries_lba: 2,
            backup_entries_lba,
            backup_header_lba,
            first_usable,
            last_usable,
        })
    }

    fn byte_offset(&self, lba: u64) -> Result<usize, GptError> {
        usize::try_from(lba)
            .map_err(|_| GptError::Overflow)?
            .checked_mul(self.sector_size)
            .ok_or(GptError::Overflow)
    }
}

fn validate_specs(
    partitions: &[PartitionSpec<'_>],
    first_usable: u64,
    last_usable: u64,
) -> Result<(), GptError> {
    let mut previous_last = None;
    for (index, partition) in partitions.iter().enumerate() {
        if partition.type_guid == Guid::ZERO || partition.unique_guid == Guid::ZERO {
            return Err(GptError::Guid);
        }
        if partition.first_lba < first_usable
            || partition.last_lba > last_usable
            || partition.first_lba > partition.last_lba
        {
            return Err(GptError::PartitionBounds);
        }
        if let Some(last) = previous_last {
            if partition.first_lba <= last {
                return Err(GptError::PartitionOverlap);
            }
        }
        if partitions[..index]
            .iter()
            .any(|previous| previous.unique_guid == partition.unique_guid)
        {
            return Err(GptError::DuplicateGuid);
        }
        validate_name(partition.name)?;
        previous_last = Some(partition.last_lba);
    }
    Ok(())
}

fn write_protective_mbr(mbr: &mut [u8], total_lbas: u64) -> Result<(), GptError> {
    if mbr.len() < 512 {
        return Err(GptError::SectorSize);
    }
    let entry = &mut mbr[446..462];
    entry[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    entry[4] = PROTECTIVE_MBR_TYPE;
    entry[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&1u32.to_le_bytes());
    let sectors = min(total_lbas.saturating_sub(1), u64::from(u32::MAX)) as u32;
    entry[12..16].copy_from_slice(&sectors.to_le_bytes());
    mbr[510..512].copy_from_slice(&[0x55, 0xAA]);
    Ok(())
}

fn validate_protective_mbr(mbr: &[u8], total_lbas: u64) -> Result<(), GptError> {
    if mbr.len() < 512
        || mbr[510..512] != [0x55, 0xAA]
        || mbr[450] != PROTECTIVE_MBR_TYPE
        || get_u32(mbr, 454) != 1
        || get_u32(mbr, 458) != min(total_lbas.saturating_sub(1), u64::from(u32::MAX)) as u32
    {
        return Err(GptError::ProtectiveMbr);
    }
    Ok(())
}

fn write_header(sector: &mut [u8], header: Header) {
    sector.fill(0);
    sector[..8].copy_from_slice(&GPT_SIGNATURE);
    put_u32(sector, 8, GPT_REVISION_1_0);
    put_u32(sector, 12, GPT_HEADER_SIZE as u32);
    put_u64(sector, 24, header.current_lba);
    put_u64(sector, 32, header.backup_lba);
    put_u64(sector, 40, header.first_usable_lba);
    put_u64(sector, 48, header.last_usable_lba);
    sector[56..72].copy_from_slice(&header.disk_guid.to_disk_bytes());
    put_u64(sector, 72, header.entries_lba);
    put_u32(sector, 80, GPT_ENTRY_COUNT as u32);
    put_u32(sector, 84, GPT_ENTRY_SIZE as u32);
    put_u32(sector, 88, header.entries_crc);
    let crc = crc32(&sector[..GPT_HEADER_SIZE]);
    put_u32(sector, 16, crc);
}

fn parse_header(sector: &[u8]) -> Result<Header, GptError> {
    if sector.len() < GPT_HEADER_SIZE
        || sector[..8] != GPT_SIGNATURE
        || get_u32(sector, 8) != GPT_REVISION_1_0
        || usize::try_from(get_u32(sector, 12)).map_err(|_| GptError::Header)? != GPT_HEADER_SIZE
        || get_u32(sector, 20) != 0
        || get_u32(sector, 80) != GPT_ENTRY_COUNT as u32
        || get_u32(sector, 84) != GPT_ENTRY_SIZE as u32
        || sector[GPT_HEADER_SIZE..].iter().any(|byte| *byte != 0)
    {
        return Err(GptError::Header);
    }
    let mut header_bytes = [0u8; GPT_HEADER_SIZE];
    header_bytes.copy_from_slice(&sector[..GPT_HEADER_SIZE]);
    let expected_crc = get_u32(&header_bytes, 16);
    put_u32(&mut header_bytes, 16, 0);
    if expected_crc == 0 || crc32(&header_bytes) != expected_crc {
        return Err(GptError::HeaderCrc);
    }
    let disk_guid = Guid::from_disk_bytes(&sector[56..72]);
    if disk_guid == Guid::ZERO {
        return Err(GptError::Guid);
    }
    Ok(Header {
        current_lba: get_u64(sector, 24),
        backup_lba: get_u64(sector, 32),
        first_usable_lba: get_u64(sector, 40),
        last_usable_lba: get_u64(sector, 48),
        disk_guid,
        entries_lba: get_u64(sector, 72),
        entries_crc: get_u32(sector, 88),
    })
}

fn encode_entry(partition: &PartitionSpec<'_>, entry: &mut [u8]) -> Result<(), GptError> {
    entry.fill(0);
    entry[..16].copy_from_slice(&partition.type_guid.to_disk_bytes());
    entry[16..32].copy_from_slice(&partition.unique_guid.to_disk_bytes());
    put_u64(entry, 32, partition.first_lba);
    put_u64(entry, 40, partition.last_lba);
    put_u64(entry, 48, partition.attributes);
    let mut count = 0usize;
    for unit in partition.name.encode_utf16() {
        if count == 36 || unit == 0 {
            return Err(GptError::Name);
        }
        put_u16(entry, 56 + count * 2, unit);
        count += 1;
    }
    if count == 0 {
        return Err(GptError::Name);
    }
    Ok(())
}

fn decode_entry(entry: &[u8]) -> Result<ParsedPartition, GptError> {
    let type_guid = Guid::from_disk_bytes(&entry[..16]);
    let unique_guid = Guid::from_disk_bytes(&entry[16..32]);
    if type_guid == Guid::ZERO || unique_guid == Guid::ZERO {
        return Err(GptError::Guid);
    }
    let mut name = [0u16; 36];
    let mut name_len = 0usize;
    let mut terminated = false;
    for (index, destination) in name.iter_mut().enumerate() {
        let unit = get_u16(entry, 56 + index * 2);
        if unit == 0 {
            terminated = true;
        } else {
            if terminated || (0xD800..=0xDFFF).contains(&unit) {
                return Err(GptError::Name);
            }
            *destination = unit;
            name_len += 1;
        }
    }
    if name_len == 0 {
        return Err(GptError::Name);
    }
    Ok(ParsedPartition {
        type_guid,
        unique_guid,
        first_lba: get_u64(entry, 32),
        last_lba: get_u64(entry, 40),
        attributes: get_u64(entry, 48),
        name,
        name_len: u8::try_from(name_len).map_err(|_| GptError::Name)?,
    })
}

fn validate_name(name: &str) -> Result<(), GptError> {
    let mut count = 0usize;
    for unit in name.encode_utf16() {
        if unit == 0 || count == 36 {
            return Err(GptError::Name);
        }
        count += 1;
    }
    if count == 0 {
        return Err(GptError::Name);
    }
    Ok(())
}

fn sector_at<'a>(image: &'a [u8], geometry: &Geometry, lba: u64) -> Result<&'a [u8], GptError> {
    let offset = geometry.byte_offset(lba)?;
    let end = offset
        .checked_add(geometry.sector_size)
        .ok_or(GptError::Overflow)?;
    image.get(offset..end).ok_or(GptError::ImageLength)
}

fn entries_at<'a>(image: &'a [u8], geometry: &Geometry, lba: u64) -> Result<&'a [u8], GptError> {
    let offset = geometry.byte_offset(lba)?;
    let end = offset
        .checked_add(GPT_ENTRIES_BYTES)
        .ok_or(GptError::Overflow)?;
    image.get(offset..end).ok_or(GptError::ImageLength)
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

fn crc32(bytes: &[u8]) -> u32 {
    // HYG-001：CRC32 唯一实现归一到 abi/ipc-wire。
    eneros_ipc_wire::crc32(bytes)
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;

    extern crate alloc;

    fn guid(value: &str) -> Guid {
        Guid::parse(value).unwrap()
    }

    fn specs(sector_size: u32) -> [PartitionSpec<'static>; 2] {
        let factor = 1_048_576u64 / u64::from(sector_size);
        [
            PartitionSpec {
                type_guid: guid("454e4552-0001-4001-8000-000000000001"),
                unique_guid: guid("454e4552-1001-4001-8000-000000000001"),
                first_lba: factor,
                last_lba: factor * 2 - 1,
                attributes: 1,
                name: "BOOTCTRL",
            },
            PartitionSpec {
                type_guid: guid("454e4552-0002-4001-8000-000000000002"),
                unique_guid: guid("454e4552-1002-4001-8000-000000000002"),
                first_lba: factor * 2,
                last_lba: factor * 4 - 1,
                attributes: 0,
                name: "BOOT_A",
            },
        ]
    }

    struct MemoryBlockDevice {
        sector_size: u32,
        bytes: Vec<u8>,
        fail_lba: Option<u64>,
        read_calls: usize,
    }

    impl BlockDevice for MemoryBlockDevice {
        fn sector_size(&self) -> u32 {
            self.sector_size
        }

        fn total_lbas(&self) -> u64 {
            u64::try_from(self.bytes.len() / self.sector_size as usize).unwrap()
        }

        fn read_blocks(&mut self, first_lba: u64, buffer: &mut [u8]) -> Result<(), BlockReadError> {
            let sector = self.sector_size as usize;
            if buffer.is_empty() || buffer.len() % sector != 0 {
                return Err(BlockReadError::BufferSize);
            }
            let block_count =
                u64::try_from(buffer.len() / sector).map_err(|_| BlockReadError::Overflow)?;
            let end_lba = first_lba
                .checked_add(block_count)
                .ok_or(BlockReadError::Overflow)?;
            if end_lba > self.total_lbas() {
                return Err(BlockReadError::OutOfBounds);
            }
            if self
                .fail_lba
                .is_some_and(|fault| fault >= first_lba && fault < end_lba)
            {
                return Err(BlockReadError::Media);
            }
            let start = usize::try_from(first_lba).unwrap() * sector;
            buffer.copy_from_slice(&self.bytes[start..start + buffer.len()]);
            self.read_calls += 1;
            Ok(())
        }
    }

    fn roundtrip(sector_size: u32) {
        let mut first = vec![0u8; 32 * 1024 * 1024];
        let mut second = vec![0u8; first.len()];
        let disk = guid("454e4552-4f53-4001-8000-000000000001");
        write_gpt(&mut first, sector_size, disk, &specs(sector_size)).unwrap();
        write_gpt(&mut second, sector_size, disk, &specs(sector_size)).unwrap();
        assert_eq!(first, second, "writer must be bit deterministic");
        let parsed = parse_gpt(&first, sector_size).unwrap();
        assert_eq!(parsed.disk_guid, disk);
        assert_eq!(parsed.partition_count, 2);
        assert!(parsed.find_by_name("BOOTCTRL").is_some());
        assert!(parsed.find_by_name("BOOT_A").is_some());
    }

    #[test]
    fn sector_512_roundtrip_is_deterministic() {
        roundtrip(512);
    }

    #[test]
    fn crc32_matches_the_independent_ieee_check_vector() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn sector_4096_roundtrip_is_deterministic() {
        roundtrip(4096);
    }

    #[test]
    fn bounded_device_reader_checks_both_gpt_copies_on_512_and_4096_media() {
        for sector_size in [512u32, 4096] {
            let mut bytes = vec![0u8; 32 * 1024 * 1024];
            let disk = guid("454e4552-4f53-4001-8000-000000000001");
            write_gpt(&mut bytes, sector_size, disk, &specs(sector_size)).unwrap();
            let geometry = Geometry::new(bytes.len(), sector_size).unwrap();

            let mut good = MemoryBlockDevice {
                sector_size,
                bytes: bytes.clone(),
                fail_lba: None,
                read_calls: 0,
            };
            let parsed = read_gpt(&mut good).unwrap();
            assert_eq!(parsed.disk_guid, disk);
            assert_eq!(parsed.partition_count, 2);
            assert_eq!(good.read_calls, 5, "reader has a fixed bounded I/O plan");

            for fault_lba in [
                0,
                1,
                geometry.primary_entries_lba,
                geometry.backup_entries_lba,
                geometry.backup_header_lba,
            ] {
                let mut faulted = MemoryBlockDevice {
                    sector_size,
                    bytes: bytes.clone(),
                    fail_lba: Some(fault_lba),
                    read_calls: 0,
                };
                assert_eq!(read_gpt(&mut faulted), Err(GptError::MediaRead));
            }
        }
    }

    #[test]
    fn bad_mbr_header_entries_and_backup_are_rejected() {
        let sector_size = 512;
        let mut image = vec![0u8; 32 * 1024 * 1024];
        write_gpt(
            &mut image,
            sector_size,
            guid("454e4552-4f53-4001-8000-000000000001"),
            &specs(sector_size),
        )
        .unwrap();

        let mut bad = image.clone();
        bad[510] ^= 1;
        assert_eq!(parse_gpt(&bad, sector_size), Err(GptError::ProtectiveMbr));

        let mut bad = image.clone();
        bad[512 + 24] ^= 1;
        assert_eq!(parse_gpt(&bad, sector_size), Err(GptError::HeaderCrc));

        let mut bad = image.clone();
        bad[2 * 512 + 64] ^= 1;
        assert_eq!(parse_gpt(&bad, sector_size), Err(GptError::EntryCrc));

        let mut bad = image;
        let backup_header = bad.len() - 512;
        bad[backup_header + 56] ^= 1;
        assert_eq!(parse_gpt(&bad, sector_size), Err(GptError::HeaderCrc));
    }

    #[test]
    fn overlap_duplicate_guid_and_invalid_name_are_rejected_by_writer() {
        let mut image = vec![0u8; 32 * 1024 * 1024];
        let disk = guid("454e4552-4f53-4001-8000-000000000001");
        let mut overlap = specs(512);
        overlap[1].first_lba = overlap[0].last_lba;
        assert_eq!(
            write_gpt(&mut image, 512, disk, &overlap),
            Err(GptError::PartitionOverlap)
        );

        let mut duplicate = specs(512);
        duplicate[1].unique_guid = duplicate[0].unique_guid;
        assert_eq!(
            write_gpt(&mut image, 512, disk, &duplicate),
            Err(GptError::DuplicateGuid)
        );

        let mut invalid_name = specs(512);
        invalid_name[0].name = "";
        assert_eq!(
            write_gpt(&mut image, 512, disk, &invalid_name),
            Err(GptError::Name)
        );
    }

    #[test]
    fn bounded_structural_mutations_never_panic() {
        let mut image = vec![0u8; 32 * 1024 * 1024];
        write_gpt(
            &mut image,
            512,
            guid("454e4552-4f53-4001-8000-000000000001"),
            &specs(512),
        )
        .unwrap();
        let mut rejected = 0;
        for index in (0..(64 * 1024)).step_by(257) {
            let mut bad = image.clone();
            bad[index] ^= 0x80;
            if parse_gpt(&bad, 512).is_err() {
                rejected += 1;
            }
        }
        assert!(rejected > 10);
        assert_eq!(
            parse_gpt(&image[..image.len() - 1], 512),
            Err(GptError::ImageLength)
        );
    }
}
