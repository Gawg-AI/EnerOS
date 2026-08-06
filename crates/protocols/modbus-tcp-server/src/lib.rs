//! EnerOS Modbus TCP slave/server (NET-010 首发协议 1).
//!
//! A real `std::net::TcpListener` based Modbus TCP slave used for
//! independent-peer interoperability evidence. It speaks real Modbus TCP over
//! a real TCP socket — not an in-memory loopback. The integration test in
//! `tests/interop.rs` connects with a raw `std::net::TcpStream` client that
//! hand-builds MBAP+PDU bytes (sharing no framing code with this crate), then
//! runs positive and negative interop cases.
//!
//! # Write path (control-gateway 纵向闭环)
//!
//! Writes are **default-deny**: with no [`AuthorizedCommand`] installed every
//! write (0x05/0x06/0x0F/0x10) is answered with `IllegalFunction` and no
//! register is modified. A caller that has obtained an `AuthorizedCommand`
//! from the control gateway installs it with
//! [`ModbusTcpServer::set_authorized_write`]; the next matching write is then
//! applied. This is the single chokepoint through which writes reach the
//! device.
//!
//! This is a host-side std crate (real TCP). The seL4 deployment will reuse
//! the [`handler`] module over a smoltcp transport.

pub mod handler;
pub mod server;

pub use handler::{HandleOutcome, RegisterStore, Response, WritePolicy};
pub use server::ModbusTcpServer;
