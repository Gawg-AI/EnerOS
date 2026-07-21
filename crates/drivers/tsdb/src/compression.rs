//! Compression backends for TSDB columnar data.
//!
//! # Backend Decision (v0.25.0)
//!
//! `snap` v1.1.1 was tested and is **not** no_std compatible — it requires
//! `std` (uses `std::convert::TryInto`, the `vec!`/`write!` macros, and does
//! not declare `#![no_std]`). Cross-compiling to `aarch64-unknown-none` fails
//! with `E0463: can't find crate for std`.
//!
//! Fallback: **`lz4_flex`** (pure Rust, no_std with `default-features = false`).
//! The [`Compressor`] trait abstracts the backend, so upper layers (writer,
//! reader) are unaffected by this choice.
//!
//! # Ratio Semantics
//!
//! [`Compressor::ratio`] returns **original / compressed** (≥ 1.0 indicates
//! space savings). If no data has been compressed yet, `1.0` is returned.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::error::TsdbError;
use crate::schema::CompressionType;

/// Abstract compression interface.
///
/// Implementations are responsible for tracking their own compression-ratio
/// statistics via interior mutability.
pub trait Compressor {
    /// Compresses `data` and returns the compressed bytes.
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>, TsdbError>;

    /// Decompresses `data` previously produced by [`compress`](Self::compress).
    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>, TsdbError>;

    /// Returns the cumulative compression ratio (original / compressed).
    ///
    /// A value ≥ 1.0 indicates the compressor is reducing size on average.
    fn ratio(&self) -> f32;
}

/// LZ4-based compressor (backs `CompressionType::Snappy`).
///
/// Uses `lz4_flex` with size-prepended framing so that decompression does
/// not need an out-of-band length hint.
pub struct SnappyCompressor {
    total_input: AtomicU64,
    total_compressed: AtomicU64,
}

impl SnappyCompressor {
    /// Creates a new compressor with zero statistics.
    pub fn new() -> Self {
        Self {
            total_input: AtomicU64::new(0),
            total_compressed: AtomicU64::new(0),
        }
    }
}

impl Default for SnappyCompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor for SnappyCompressor {
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>, TsdbError> {
        let compressed = lz4_flex::compress_prepend_size(data);
        self.total_input
            .fetch_add(data.len() as u64, Ordering::Relaxed);
        self.total_compressed
            .fetch_add(compressed.len() as u64, Ordering::Relaxed);
        Ok(compressed)
    }

    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>, TsdbError> {
        lz4_flex::decompress_size_prepended(data).map_err(|_| TsdbError::DecompressFailed)
    }

    fn ratio(&self) -> f32 {
        let input = self.total_input.load(Ordering::Relaxed);
        let compressed = self.total_compressed.load(Ordering::Relaxed);
        if compressed == 0 {
            return 1.0;
        }
        input as f32 / compressed as f32
    }
}

/// Pass-through compressor (backs `CompressionType::None`).
pub struct NoopCompressor;

impl Compressor for NoopCompressor {
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>, TsdbError> {
        Ok(data.to_vec())
    }

    fn decompress(&self, data: &[u8]) -> Result<Vec<u8>, TsdbError> {
        Ok(data.to_vec())
    }

    fn ratio(&self) -> f32 {
        1.0
    }
}

/// Factory: creates a compressor matching the given [`CompressionType`].
pub fn make_compressor(ct: CompressionType) -> Box<dyn Compressor> {
    match ct {
        CompressionType::None => Box::new(NoopCompressor),
        CompressionType::Snappy => Box::new(SnappyCompressor::new()),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    #[test]
    fn test_snappy_compress_decompress_roundtrip() {
        let compressor = SnappyCompressor::new();
        let input = b"hello world hello world hello world hello world";
        let compressed = compressor.compress(input).expect("compress");
        let decompressed = compressor.decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_snappy_empty_input_roundtrip() {
        let compressor = SnappyCompressor::new();
        let compressed = compressor.compress(b"").expect("compress");
        let decompressed = compressor.decompress(&compressed).expect("decompress");
        assert!(decompressed.is_empty());
    }

    #[test]
    fn test_snappy_compresses_repetitive_data() {
        let compressor = SnappyCompressor::new();
        // Highly repetitive data should compress well.
        let mut input = Vec::with_capacity(4096);
        for _ in 0..256 {
            input.extend_from_slice(b"AAAAAAAAAAAAAAAA");
        }
        let compressed = compressor.compress(&input).expect("compress");
        assert!(
            compressed.len() < input.len(),
            "compressed {} should be smaller than input {}",
            compressed.len(),
            input.len()
        );
    }

    #[test]
    fn test_snappy_ratio_after_compression() {
        let compressor = SnappyCompressor::new();
        // Before any compression, ratio is 1.0.
        assert_eq!(compressor.ratio(), 1.0);

        let mut input = Vec::with_capacity(4096);
        for _ in 0..256 {
            input.extend_from_slice(b"BBBBBBBBBBBBBBBB");
        }
        let _compressed = compressor.compress(&input).expect("compress");
        // Repetitive data → ratio > 1.0.
        assert!(
            compressor.ratio() > 1.0,
            "ratio {} should be > 1.0 for repetitive data",
            compressor.ratio()
        );
    }

    #[test]
    fn test_snappy_decompress_invalid_data_fails() {
        let compressor = SnappyCompressor::new();
        let garbage = [0xFFu8; 16];
        let result = compressor.decompress(&garbage);
        assert!(matches!(result, Err(TsdbError::DecompressFailed)));
    }

    #[test]
    fn test_noop_compress_decompress_roundtrip() {
        let compressor = NoopCompressor;
        let input = b"some data that should not be compressed";
        let compressed = compressor.compress(input).expect("compress");
        assert_eq!(compressed, input);
        let decompressed = compressor.decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, input);
    }

    #[test]
    fn test_noop_ratio_is_always_one() {
        let compressor = NoopCompressor;
        let _ = compressor.compress(b"some data").unwrap();
        assert_eq!(compressor.ratio(), 1.0);
    }

    #[test]
    fn test_make_compressor_none() {
        let compressor = make_compressor(CompressionType::None);
        let data = b"test";
        let compressed = compressor.compress(data).expect("compress");
        assert_eq!(compressed, data);
        assert_eq!(compressor.ratio(), 1.0);
    }

    #[test]
    fn test_make_compressor_snappy() {
        let compressor = make_compressor(CompressionType::Snappy);
        let data = b"test test test test test test";
        let compressed = compressor.compress(data).expect("compress");
        let decompressed = compressor.decompress(&compressed).expect("decompress");
        assert_eq!(decompressed, data);
    }
}
