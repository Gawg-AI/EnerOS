use crate::gpt::ParsedPartition;

/// Minimal synchronous media contract shared by firmware and host-side fault tests.
/// Implementations must either fill the entire buffer or return an error.
pub trait BlockDevice {
    fn sector_size(&self) -> u32;
    fn total_lbas(&self) -> u64;
    fn read_blocks(&mut self, first_lba: u64, buffer: &mut [u8]) -> Result<(), BlockReadError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReadError {
    SectorSize,
    BufferSize,
    OutOfBounds,
    Overflow,
    Media,
}

/// A byte-addressed, partition-bounded view over a block device.
///
/// Callers supply one sector of scratch storage so the implementation stays
/// allocator-free and never trusts an on-media length to allocate memory.
pub struct PartitionReader<'a, D: BlockDevice> {
    device: &'a mut D,
    first_lba: u64,
    sector_count: u64,
}

impl<'a, D: BlockDevice> PartitionReader<'a, D> {
    pub fn new(device: &'a mut D, partition: &ParsedPartition) -> Result<Self, BlockReadError> {
        let sector_count = partition
            .last_lba
            .checked_sub(partition.first_lba)
            .and_then(|value| value.checked_add(1))
            .ok_or(BlockReadError::Overflow)?;
        if partition.last_lba >= device.total_lbas() {
            return Err(BlockReadError::OutOfBounds);
        }
        validate_sector_size(device.sector_size())?;
        Ok(Self {
            device,
            first_lba: partition.first_lba,
            sector_count,
        })
    }

    pub fn len(&self) -> Result<u64, BlockReadError> {
        self.sector_count
            .checked_mul(u64::from(self.device.sector_size()))
            .ok_or(BlockReadError::Overflow)
    }

    pub fn is_empty(&self) -> bool {
        self.sector_count == 0
    }

    pub fn read_exact(
        &mut self,
        offset: u64,
        destination: &mut [u8],
        scratch_sector: &mut [u8],
    ) -> Result<(), BlockReadError> {
        let sector_size =
            usize::try_from(self.device.sector_size()).map_err(|_| BlockReadError::SectorSize)?;
        validate_sector_size(self.device.sector_size())?;
        if scratch_sector.len() != sector_size {
            return Err(BlockReadError::BufferSize);
        }
        let length = u64::try_from(destination.len()).map_err(|_| BlockReadError::Overflow)?;
        let end = offset.checked_add(length).ok_or(BlockReadError::Overflow)?;
        if end > self.len()? {
            return Err(BlockReadError::OutOfBounds);
        }
        if destination.is_empty() {
            return Ok(());
        }

        let sector_size_u64 = u64::from(self.device.sector_size());
        let mut media_lba = self
            .first_lba
            .checked_add(offset / sector_size_u64)
            .ok_or(BlockReadError::Overflow)?;
        let mut in_sector =
            usize::try_from(offset % sector_size_u64).map_err(|_| BlockReadError::Overflow)?;
        let mut written = 0usize;

        while written < destination.len() {
            self.device.read_blocks(media_lba, scratch_sector)?;
            let available = sector_size - in_sector;
            let count = core::cmp::min(available, destination.len() - written);
            destination[written..written + count]
                .copy_from_slice(&scratch_sector[in_sector..in_sector + count]);
            written += count;
            media_lba = media_lba.checked_add(1).ok_or(BlockReadError::Overflow)?;
            in_sector = 0;
        }
        Ok(())
    }
}

pub(crate) fn validate_sector_size(sector_size: u32) -> Result<(), BlockReadError> {
    if matches!(sector_size, 512 | 4096) {
        Ok(())
    } else {
        Err(BlockReadError::SectorSize)
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;
    use crate::Guid;

    struct MemoryDevice {
        sector_size: u32,
        bytes: Vec<u8>,
        fail_lba: Option<u64>,
        reads: usize,
    }

    impl BlockDevice for MemoryDevice {
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
            let blocks =
                u64::try_from(buffer.len() / sector).map_err(|_| BlockReadError::Overflow)?;
            let end_lba = first_lba
                .checked_add(blocks)
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
            self.reads += 1;
            Ok(())
        }
    }

    fn partition(first_lba: u64, last_lba: u64) -> ParsedPartition {
        ParsedPartition {
            type_guid: Guid([1; 16]),
            unique_guid: Guid([2; 16]),
            first_lba,
            last_lba,
            attributes: 0,
            name: [0; 36],
            name_len: 1,
        }
    }

    #[test]
    fn cross_sector_reads_are_exact_and_bounded() {
        for sector_size in [512u32, 4096] {
            let mut bytes = vec![0u8; sector_size as usize * 8];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = (index % 251) as u8;
            }
            let mut device = MemoryDevice {
                sector_size,
                bytes: bytes.clone(),
                fail_lba: None,
                reads: 0,
            };
            let mut reader = PartitionReader::new(&mut device, &partition(2, 5)).unwrap();
            let mut scratch = vec![0u8; sector_size as usize];
            let mut destination = vec![0u8; sector_size as usize + 37];
            reader
                .read_exact(17, &mut destination, &mut scratch)
                .unwrap();
            let start = sector_size as usize * 2 + 17;
            assert_eq!(destination, bytes[start..start + destination.len()]);
            assert_eq!(
                reader.read_exact(reader.len().unwrap() - 3, &mut [0; 4], &mut scratch),
                Err(BlockReadError::OutOfBounds)
            );
        }
    }

    #[test]
    fn media_fault_is_propagated_without_partial_success() {
        let mut device = MemoryDevice {
            sector_size: 512,
            bytes: vec![0x5a; 4096],
            fail_lba: Some(3),
            reads: 0,
        };
        let mut reader = PartitionReader::new(&mut device, &partition(2, 5)).unwrap();
        let mut scratch = [0u8; 512];
        let mut destination = [0xa5; 700];
        assert_eq!(
            reader.read_exact(400, &mut destination, &mut scratch),
            Err(BlockReadError::Media)
        );
        assert!(destination[..112].iter().all(|byte| *byte == 0x5a));
        assert!(destination[112..].iter().all(|byte| *byte == 0xa5));
    }

    #[test]
    fn bad_sector_and_scratch_sizes_are_rejected() {
        let mut device = MemoryDevice {
            sector_size: 1024,
            bytes: vec![0; 8192],
            fail_lba: None,
            reads: 0,
        };
        assert!(matches!(
            PartitionReader::new(&mut device, &partition(1, 2)),
            Err(BlockReadError::SectorSize)
        ));

        let mut device = MemoryDevice {
            sector_size: 512,
            bytes: vec![0; 8192],
            fail_lba: None,
            reads: 0,
        };
        let mut reader = PartitionReader::new(&mut device, &partition(1, 2)).unwrap();
        assert_eq!(
            reader.read_exact(0, &mut [0; 1], &mut [0; 511]),
            Err(BlockReadError::BufferSize)
        );
    }
}
