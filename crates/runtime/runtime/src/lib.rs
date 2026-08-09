//! EnerOS user-space runtime library (workspace v0.192.0)
//!
//! Provides `print!`/`println!` macros and console initialization for
//! seL4 user-space Rust components. Depends on `eneros-sel4-sys` for
//! syscall FFI bindings.

#![no_std]
#![allow(clippy::empty_loop)]

pub mod console;
pub mod oom;
pub mod serial;

pub use console::init;
