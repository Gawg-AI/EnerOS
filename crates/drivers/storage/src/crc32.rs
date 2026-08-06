//! IEEE 802.3 CRC32 (polynomial 0xEDB88320, reversed).
//!
//! Used for storage data integrity verification.
//!
//! HYG-001：实现已归一到 [`eneros_ipc_wire::crc32`]（`abi/ipc-wire` 唯一
//! 实现）——本模块仅为兼容 re-export，不再维护第二份表实现。
//!
//! # Known Vectors
//!
//! - Empty input → `0x00000000`
//! - `b"123456789"` → `0xCBF43926`

/// Computes the IEEE 802.3 CRC32 of `data`（转发 abi/ipc-wire 唯一实现）。
///
/// # Example
///
/// ```
/// # use eneros_storage::crc32::crc32;
/// assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
/// assert_eq!(crc32(b""), 0);
/// ```
pub fn crc32(data: &[u8]) -> u32 {
    eneros_ipc_wire::crc32(data)
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
    fn test_forwarded_matches_known_vector() {
        // HYG-001：实现已归一到 eneros-ipc-wire——KAT 向量验证转发一致性
        // （旧表实现与 ipc-wire 同为 IEEE 802.3 0xEDB88320）。
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
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
