//! EnerOS Memory Management — ARM64 page tables and virtual address spaces.
//!
//! This crate implements the ARMv8-A four-level page table (48-bit VA,
//! 4KB granule), `Vspace`/`Vregion` abstractions, and the `AddressSpace`
//! trait for virtual memory mapping/unmapping/translation.
//!
//! v0.9.0 adds `Partition`/`DmaGuard` for memory isolation and DMA protection.

#![cfg_attr(not(test), no_std)]

pub mod dma_guard;
pub mod isolation;
pub mod page_table;
pub mod partition;
pub mod vregion;
pub mod vspace;
