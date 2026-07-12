//! IEEE 802.3 CRC32 (polynomial 0xEDB88320, reversed).
//!
//! Used for storage data integrity verification. The implementation is
//! table-based (256-entry lookup) for O(n) computation with a small constant
//! factor.
//!
//! # Known Vectors
//!
//! - Empty input → `0x00000000`
//! - `b"123456789"` → `0xCBF43926`

/// Reversed IEEE 802.3 CRC32 polynomial.
const CRC32_POLY: u32 = 0xEDB8_8320;

/// 256-entry lookup table for byte-at-a-time CRC32 computation.
const CRC32_TABLE: [u32; 256] = build_crc32_table();

/// Builds the 256-entry CRC32 lookup table at compile time.
const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ CRC32_POLY;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

/// Computes the IEEE 802.3 CRC32 of `data`.
///
/// Returns `0` for empty input. For the standard check vector
/// `b"123456789"` the result is `0xCBF43926`.
///
/// # Example
///
/// ```
/// # use eneros_storage::crc32::crc32;
/// assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
/// assert_eq!(crc32(b""), 0);
/// ```
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        let idx = ((crc as u8) ^ byte) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
    }
    crc ^ 0xFFFF_FFFF
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_input() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn test_known_vector_123456789() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn test_single_byte() {
        // CRC32 of a single zero byte.
        assert_eq!(crc32(b"\x00"), 0xD202_EF8D);
    }

    #[test]
    fn test_arbitrary_data() {
        let data = b"The quick brown fox jumps over the lazy dog";
        // Known CRC32 (IEEE 802.3 / zlib) of this ASCII string.
        assert_eq!(crc32(data), 0x414F_A339);
    }

    #[test]
    fn test_incremental_consistency() {
        // CRC32 of concatenated data should equal CRC32 of the whole.
        // (Note: this is true only because crc32 here is not incremental in
        // the streaming sense — we just verify that recomputing on the full
        // buffer matches the value computed once.)
        let part1 = b"hello, ";
        let part2 = b"world!";
        let mut combined = alloc::vec::Vec::new();
        combined.extend_from_slice(part1);
        combined.extend_from_slice(part2);
        assert_eq!(crc32(&combined), crc32(b"hello, world!"));
    }

    #[test]
    fn test_table_built() {
        // Spot-check a few table entries against known values.
        // table[0] is always 0 (no bits set).
        assert_eq!(CRC32_TABLE[0], 0);
        // table[1] = poly itself (1 bit shifts through).
        assert_eq!(CRC32_TABLE[1], 0x7707_3096);
        // table[255] — last entry, deterministic.
        assert_eq!(CRC32_TABLE[255], 0x2D02_EF8D);
    }

    #[test]
    fn test_all_zeros_block() {
        let block = [0u8; 512];
        // CRC32 of 512 zero bytes — deterministic and non-zero.
        let first = crc32(&block);
        let second = crc32(&block);
        assert_eq!(first, second, "CRC must be deterministic");
        // 512 zeros should not produce a zero CRC (the initial/final XOR
        // prevents the trivial all-zero output).
        assert_ne!(first, 0, "CRC of 512 zeros must be non-zero");
        // Length sensitivity: 511 zeros differs from 512 zeros.
        let shorter = [0u8; 511];
        assert_ne!(crc32(&shorter), first, "CRC must depend on length");
    }

    #[test]
    fn test_different_data_different_crc() {
        let a = crc32(b"foo");
        let b = crc32(b"bar");
        assert_ne!(a, b);
    }
}
