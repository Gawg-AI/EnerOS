//! Socket abstraction layer (v0.29.0).
//!
//! Unified Socket API built on top of the v0.28.0 TCP/IP stack. Provides a
//! [`SocketManager`] that centrally owns the [`NetworkInterface`] and all
//! sockets, exposing standardized read/write/close operations, non-blocking
//! IO, and `poll`-style multiplexing.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────┐
//! │  Caller (v0.46.0 Modbus TCP / v0.48.0 IEC 104 / Phase 2) │
//! └─────────────┬────────────────────────────────────────────┘
//!               │  SocketManager API (tcp_connect/read/write/...)
//! ┌─────────────▼────────────────────────────────────────────┐
//! │  socket::SocketManager<D: NetDevice>  (this module)       │
//! │  ┌──────────────────────────────────────────────────┐    │
//! │  │  sockets: BTreeMap<SocketId, SocketEntry>         │    │
//! │  │  poll:    Poll (registry: BTreeMap<Id, Interest>) │    │
//! │  │  iface:   NetworkInterface<D> (owns smoltcp)      │    │
//! │  └──────────────────────────────────────────────────┘    │
//! └─────────────┬────────────────────────────────────────────┘
//!               │  SocketHandle -> smoltcp tcp/udp Socket
//! ┌─────────────▼────────────────────────────────────────────┐
//! │  tcpip::NetworkInterface<D> + SocketSet  (v0.28.0)        │
//! └─────────────┬────────────────────────────────────────────┘
//!               │  NetDevice trait
//! ┌─────────────▼────────────────────────────────────────────┐
//! │  mac::MacController (v0.27.0 Ethernet driver)            │
//! └──────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! use eneros_net::{SocketManager, InterfaceConfig, ipv4_addr, ipv4_cidr};
//!
//! let dev = MyNetDevice::new();
//! let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
//!     .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24));
//! let mut mgr = SocketManager::new(dev, config);
//!
//! // TCP connect
//! let stream = mgr.tcp_connect(remote, 50000).expect("connect failed");
//! mgr.write(stream.id(), b"hello").expect("write failed");
//!
//! // Non-blocking poll
//! mgr.register(stream.id(), Interest::all_readable()).ok();
//! let events = mgr.poll_once();
//! for ev in events {
//!     // handle ev.socket_id readiness
//! }
//! ```
//!
//! # Design Decisions (Karpathy 四原则)
//!
//! - **Think Before Coding**: SocketManager 集中拥有 NetworkInterface 和所有
//!   Socket，不使用 `Box<dyn Socket>` 存储。smoltcp 的 SocketSet 架构要求集中
//!   式 socket 所有权；若实现 `Socket` trait 需 `Rc<RefCell<>>`（no_std 不友好）
//!   或 unsafe 全局指针，违背 Simplicity First。
//! - **Simplicity First**: `TcpStream` / `TcpListener` / `UdpSocket` 是
//!   `SocketId`（`usize`）的零成本 newtype 句柄；操作通过
//!   `mgr.read(stream.id(), buf)` 完成，无 `Rc`/`RefCell`/`Box`。
//! - **Surgical Changes**: 不修改 v0.27.0（6 文件）和 v0.28.0（7 文件）源文件，
//!   仅在 lib.rs 添加 `pub mod socket;`。
//! - **Goal-Driven Execution**: 测试覆盖 SocketManager 生命周期 + IO + Poll +
//!   错误转换，使用 MockNetDevice 不依赖真实网络。
//!
//! # 偏差声明
//!
//! [`Socket`] trait 定义保留（用于文档和未来扩展），但不为 smoltcp 后端实现。
//! 原因：smoltcp 的 socket 数据存储在 `SocketSet`（在 `NetworkInterface` 内部），
//! 要实现 `Socket` trait 的 `fn read(&mut self, buf: &mut [u8])` 签名，TcpStream
//! 必须能独立访问 NetworkInterface —— 这需要 `Rc<RefCell<>>`（no_std 不友好）或
//! unsafe 全局指针（复杂且不安全）。SocketManager 方法 API 提供等价功能，代码更
//! 简单。后续如需多态可引入 trait 对象。
//!
//! # no_std 合规
//!
//! crate 根 (`lib.rs`) 声明 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`，
//! 覆盖所有子模块。socket/* 文件使用 `alloc::collections::BTreeMap`（不用 HashMap），
//! 无 `use std::*`。

pub mod api;
pub mod event;
pub mod manager;
pub mod poll;

// Re-export all public types for convenience.
pub use api::{Socket, SocketError, SocketId, SocketKind, TcpListener, TcpStream, UdpSocket};
pub use event::Event;
pub use manager::SocketManager;
pub use poll::{Interest, Poll, Readiness};
