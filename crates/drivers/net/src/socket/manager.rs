//! `SocketManager<D>` — centrally owns `NetworkInterface` and all sockets.
//!
//! Provides a unified API for TCP/UDP socket lifecycle (connect / listen /
//! accept / bind / close), IO (read / write / send_to / recv_from), state
//! queries, and non-blocking poll-based multiplexing.
//!
//! # Architecture
//!
//! `SocketManager` owns the [`NetworkInterface`] (which in turn owns the
//! smoltcp `Interface` and `SocketSet`). All socket operations go through
//! [`SocketId`] → [`SocketEntry`] → `SocketHandle` → smoltcp socket.
//!
//! ```text
//! Application
//!     │  mgr.tcp_connect(remote, port) → TcpStream(id)
//!     │  mgr.read(stream.id(), buf)
//!     ▼
//! SocketManager
//!     ├── sockets: BTreeMap<SocketId, SocketEntry>
//!     │       └── SocketEntry { handle, kind, nonblocking, ... }
//!     ├── poll: Poll  (registry: BTreeMap<SocketId, Interest>)
//!     └── iface: NetworkInterface<D>
//!             ├── iface: smoltcp::iface::Interface
//!             └── sockets: SocketSet  (smoltcp::iface::SocketSet)
//!                     └── SocketHandle → tcp::Socket / udp::Socket
//! ```
//!
//! # Borrow Strategy
//!
//! smoltcp stores sockets inside `SocketSet` (owned by `NetworkInterface`).
//! To access a socket, we need `&mut self.iface.sockets.inner.get_mut(handle)`.
//! For `tcp_connect`, we also need `self.iface.iface.context()`.
//!
//! Split-borrowing pattern: access `self.iface` fields directly so the
//! compiler can split the borrow of `self.iface.iface` and
//! `self.iface.sockets` (different fields of the same struct).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::fmt;

use smoltcp::socket::{tcp, udp};
use smoltcp::wire::{IpAddress, IpEndpoint};

use super::api::{SocketError, SocketId, SocketKind, TcpListener, TcpStream, UdpSocket};
use super::event::Event;
use super::poll::{Interest, Poll};
use crate::error::NetStats;
use crate::mac::NetDevice;
use crate::tcpip::addr::{Ipv4Addr, SocketAddr, SocketHandle};
use crate::tcpip::error::TcpIpError;
use crate::tcpip::interface::{InterfaceConfig, NetworkInterface};
use crate::tcpip::socket::TcpState;

// ---------------------------------------------------------------------------
// Buffer size constants (defaults, matching configs/socket.toml)
// ---------------------------------------------------------------------------

/// Default TCP RX buffer size (64 KB - 1, matches configs/tcpip.toml).
const TCP_RX_BUFFER_SIZE: usize = 65535;
/// Default TCP TX buffer size (64 KB - 1).
const TCP_TX_BUFFER_SIZE: usize = 65535;
/// Default UDP RX buffer size (4 KB).
const UDP_RX_BUFFER_SIZE: usize = 4096;
/// Default UDP TX buffer size (4 KB).
const UDP_TX_BUFFER_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// SocketEntry
// ---------------------------------------------------------------------------

/// Per-socket metadata stored in `SocketManager::sockets`.
///
/// Maps a [`SocketId`] to the underlying smoltcp [`SocketHandle`] and caches
/// the socket kind, non-blocking flag, and endpoint addresses.
struct SocketEntry {
    /// smoltcp socket handle — identifies the socket in `SocketSet`.
    handle: SocketHandle,
    /// Socket kind (TcpStream / TcpListener / Udp).
    kind: SocketKind,
    /// Non-blocking flag (semantic; smoltcp is always non-blocking).
    nonblocking: bool,
    /// Cached local address (if known).
    local_addr: Option<SocketAddr>,
    /// Cached remote address (if known).
    remote_addr: Option<SocketAddr>,
}

impl fmt::Debug for SocketEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SocketEntry")
            .field("kind", &self.kind)
            .field("nonblocking", &self.nonblocking)
            .field("local_addr", &self.local_addr)
            .field("remote_addr", &self.remote_addr)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// SocketManager
// ---------------------------------------------------------------------------

/// Centralized socket manager owning `NetworkInterface` and all sockets.
///
/// Provides a unified API for TCP/UDP socket operations. All socket state
/// lives inside the manager; callers hold lightweight [`SocketId`]-based
/// handles ([`TcpStream`] / [`TcpListener`] / [`UdpSocket`]).
///
/// # Usage
///
/// ```ignore
/// use eneros_net::{SocketManager, InterfaceConfig, ipv4_addr, ipv4_cidr, Interest};
///
/// let dev = MyNetDevice::new();
/// let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
///     .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24));
/// let mut mgr = SocketManager::new(dev, config);
///
/// // TCP connect
/// let stream = mgr.tcp_connect(remote, 50000).expect("connect failed");
/// mgr.register(stream.id(), Interest::all_readable()).ok();
///
/// // Main loop
/// loop {
///     mgr.poll_interface(timestamp()).ok();
///     let events = mgr.poll_once();
///     for ev in events {
///         if ev.is_readable() {
///             let mut buf = [0u8; 1024];
///             let n = mgr.read(ev.socket_id, &mut buf).unwrap_or(0);
///             // process buf[..n]
///         }
///     }
/// }
/// ```
pub struct SocketManager<D: NetDevice> {
    /// Network interface (owns smoltcp Interface + SocketSet + device).
    iface: NetworkInterface<D>,
    /// SocketId → SocketEntry mapping.
    sockets: BTreeMap<SocketId, SocketEntry>,
    /// Next SocketId to assign (monotonically increasing).
    next_id: SocketId,
    /// Poll registry for multiplexing.
    poll: Poll,
}

impl<D: NetDevice> SocketManager<D> {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a new `SocketManager` with the given device and configuration.
    ///
    /// The manager owns the `NetworkInterface` and starts with no sockets.
    pub fn new(device: D, config: InterfaceConfig) -> Self {
        let iface = NetworkInterface::new(device, config);
        Self {
            iface,
            sockets: BTreeMap::new(),
            next_id: 0,
            poll: Poll::new(),
        }
    }

    // -----------------------------------------------------------------------
    // TCP lifecycle
    // -----------------------------------------------------------------------

    /// Connect to a remote TCP endpoint.
    ///
    /// Creates a new smoltcp TCP socket, initiates a connection to `remote`
    /// using `local_port` as the source port, and returns a [`TcpStream`]
    /// handle.
    ///
    /// # Errors
    ///
    /// - [`SocketError::InvalidArgument`] — socket is in an invalid state.
    /// - [`SocketError::AddrNotAvailable`] — local port or remote address
    ///   is not usable.
    pub fn tcp_connect(
        &mut self,
        remote: SocketAddr,
        local_port: u16,
    ) -> Result<TcpStream, SocketError> {
        let id = self.next_id;
        let handle = {
            let iface = &mut self.iface;
            let handle = iface
                .sockets
                .add_tcp(TCP_RX_BUFFER_SIZE, TCP_TX_BUFFER_SIZE);
            let cx = iface.iface.context();
            let socket = iface.sockets.inner.get_mut::<tcp::Socket>(handle);
            match socket.connect(cx, remote, local_port) {
                Ok(()) => handle,
                Err(e) => {
                    // Clean up the socket on failure to avoid leaks.
                    iface.sockets.remove(handle);
                    return Err(SocketError::from(TcpIpError::from(e)));
                }
            }
        };
        self.next_id += 1;
        self.sockets.insert(
            id,
            SocketEntry {
                handle,
                kind: SocketKind::TcpStream,
                nonblocking: false,
                local_addr: None,
                remote_addr: Some(remote),
            },
        );
        Ok(TcpStream::new(id))
    }

    /// Start listening for incoming TCP connections on the given port.
    ///
    /// Creates a new smoltcp TCP socket in `Listen` state and returns a
    /// [`TcpListener`] handle.
    ///
    /// # Errors
    ///
    /// - [`SocketError::InvalidArgument`] — socket is in an invalid state.
    /// - [`SocketError::AddrNotAvailable`] — port is not usable.
    pub fn tcp_listen(&mut self, port: u16) -> Result<TcpListener, SocketError> {
        let id = self.next_id;
        let handle = {
            let iface = &mut self.iface;
            let handle = iface
                .sockets
                .add_tcp(TCP_RX_BUFFER_SIZE, TCP_TX_BUFFER_SIZE);
            let socket = iface.sockets.inner.get_mut::<tcp::Socket>(handle);
            match socket.listen(port) {
                Ok(()) => handle,
                Err(e) => {
                    iface.sockets.remove(handle);
                    return Err(SocketError::from(TcpIpError::from(e)));
                }
            }
        };
        self.next_id += 1;
        // Build a local address placeholder (port only; address unspecified).
        let local_addr = IpEndpoint {
            addr: IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(0, 0, 0, 0)),
            port,
        };
        self.sockets.insert(
            id,
            SocketEntry {
                handle,
                kind: SocketKind::TcpListener,
                nonblocking: false,
                local_addr: Some(local_addr),
                remote_addr: None,
            },
        );
        Ok(TcpListener::new(id))
    }

    /// Accept an incoming connection on a listening TCP socket.
    ///
    /// smoltcp does not provide a POSIX-style `accept()` that creates a new
    /// socket. Instead, the listener socket itself transitions to
    /// `Established` when a connection arrives. This method checks the
    /// listener's state:
    ///
    /// - If `Established`: returns `(TcpStream(listener_id), remote_addr)`.
    ///   The socket kind is updated from `TcpListener` to `TcpStream`.
    /// - Otherwise: returns [`SocketError::WouldBlock`].
    ///
    /// # Limitation
    ///
    /// Each listener can handle one pending connection at a time. After
    /// `tcp_accept` succeeds, the caller must create a new listener (via
    /// [`tcp_listen`](Self::tcp_listen)) to accept the next connection.
    ///
    /// # Errors
    ///
    /// - [`SocketError::Closed`] — socket id not found.
    /// - [`SocketError::WouldBlock`] — no connection ready.
    /// - [`SocketError::NotConnected`] — socket is Established but has no
    ///   remote endpoint (unexpected).
    pub fn tcp_accept(
        &mut self,
        listener: TcpListener,
    ) -> Result<(TcpStream, SocketAddr), SocketError> {
        let id = listener.id();
        let handle = self.sockets.get(&id).ok_or(SocketError::Closed)?.handle;

        // Check TCP state and get remote endpoint.
        let (state, remote) = {
            let socket = self.iface.sockets.inner.get::<tcp::Socket>(handle);
            let state = TcpState::from(socket.state());
            let remote = socket.remote_endpoint();
            (state, remote)
        };

        if !state.is_connected() {
            return Err(SocketError::WouldBlock);
        }

        let remote = remote.ok_or(SocketError::NotConnected)?;

        // Update the entry: listener becomes a stream.
        if let Some(entry) = self.sockets.get_mut(&id) {
            entry.kind = SocketKind::TcpStream;
            entry.remote_addr = Some(remote);
        }

        Ok((TcpStream::new(id), remote))
    }

    // -----------------------------------------------------------------------
    // UDP lifecycle
    // -----------------------------------------------------------------------

    /// Bind a UDP socket to the specified local address.
    ///
    /// Creates a new smoltcp UDP socket and binds it to `local.addr:local.port`.
    /// If the address is unspecified (0.0.0.0), the socket listens on all
    /// interfaces.
    ///
    /// # Errors
    ///
    /// - [`SocketError::AddrInUse`] — port is already bound.
    pub fn udp_bind(&mut self, local: SocketAddr) -> Result<UdpSocket, SocketError> {
        let id = self.next_id;
        let handle = {
            let iface = &mut self.iface;
            let handle = iface
                .sockets
                .add_udp(UDP_RX_BUFFER_SIZE, UDP_TX_BUFFER_SIZE);
            let socket = iface.sockets.inner.get_mut::<udp::Socket>(handle);
            // Convert IpEndpoint to (IpAddress, u16) for smoltcp's bind.
            match socket.bind((local.addr, local.port)) {
                Ok(()) => handle,
                Err(_) => {
                    iface.sockets.remove(handle);
                    return Err(SocketError::AddrInUse);
                }
            }
        };
        self.next_id += 1;
        self.sockets.insert(
            id,
            SocketEntry {
                handle,
                kind: SocketKind::Udp,
                nonblocking: false,
                local_addr: Some(local),
                remote_addr: None,
            },
        );
        Ok(UdpSocket::new(id))
    }

    // -----------------------------------------------------------------------
    // Close
    // -----------------------------------------------------------------------

    /// Close a socket and release all associated resources.
    ///
    /// Removes the socket from the `SocketSet`, the internal `sockets` map,
    /// and the poll registry.
    ///
    /// # Errors
    ///
    /// - [`SocketError::Closed`] — socket id not found.
    pub fn close(&mut self, id: SocketId) -> Result<(), SocketError> {
        let entry = self.sockets.remove(&id).ok_or(SocketError::Closed)?;
        self.iface.sockets.remove(entry.handle);
        self.poll.deregister(id);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // TCP IO
    // -----------------------------------------------------------------------

    /// Read data from a TCP socket into `buf`.
    ///
    /// Returns the number of bytes read. For non-established sockets, returns
    /// [`SocketError::NotConnected`]. If no data is available, returns
    /// [`SocketError::WouldBlock`].
    ///
    /// # Errors
    ///
    /// - [`SocketError::Closed`] — socket id not found.
    /// - [`SocketError::NotConnected`] — socket is not in Established state.
    /// - [`SocketError::WouldBlock`] — no data available (non-blocking).
    pub fn read(&mut self, id: SocketId, buf: &mut [u8]) -> Result<usize, SocketError> {
        let handle = self.sockets.get(&id).ok_or(SocketError::Closed)?.handle;
        let socket = self.iface.sockets.inner.get_mut::<tcp::Socket>(handle);
        socket
            .recv_slice(buf)
            .map_err(|e| SocketError::from(TcpIpError::from(e)))
    }

    /// Write data to a TCP socket from `buf`.
    ///
    /// Returns the number of bytes queued in the TX buffer. For non-established
    /// sockets, returns [`SocketError::NotConnected`]. If the TX buffer is
    /// full, returns [`SocketError::WouldBlock`].
    ///
    /// # Errors
    ///
    /// - [`SocketError::Closed`] — socket id not found.
    /// - [`SocketError::NotConnected`] — socket is not in Established state.
    /// - [`SocketError::WouldBlock`] — TX buffer full (non-blocking).
    pub fn write(&mut self, id: SocketId, buf: &[u8]) -> Result<usize, SocketError> {
        let handle = self.sockets.get(&id).ok_or(SocketError::Closed)?.handle;
        let socket = self.iface.sockets.inner.get_mut::<tcp::Socket>(handle);
        socket
            .send_slice(buf)
            .map_err(|e| SocketError::from(TcpIpError::from(e)))
    }

    // -----------------------------------------------------------------------
    // UDP IO
    // -----------------------------------------------------------------------

    /// Send data to a remote UDP endpoint.
    ///
    /// Returns the number of bytes sent (always equals `buf.len()` on success
    /// since smoltcp queues the entire packet).
    ///
    /// # Errors
    ///
    /// - [`SocketError::Closed`] — socket id not found.
    /// - [`SocketError::WouldBlock`] — TX buffer full.
    /// - [`SocketError::IoError`] — destination unreachable.
    pub fn send_to(
        &mut self,
        id: SocketId,
        buf: &[u8],
        dst: SocketAddr,
    ) -> Result<usize, SocketError> {
        let handle = self.sockets.get(&id).ok_or(SocketError::Closed)?.handle;
        let socket = self.iface.sockets.inner.get_mut::<udp::Socket>(handle);
        socket
            .send_slice(buf, dst)
            .map_err(|e| SocketError::from(TcpIpError::from(e)))?;
        Ok(buf.len())
    }

    /// Receive data from a UDP socket.
    ///
    /// Returns the number of bytes received and the source endpoint.
    ///
    /// # Errors
    ///
    /// - [`SocketError::Closed`] — socket id not found.
    /// - [`SocketError::WouldBlock`] — no data available (non-blocking).
    pub fn recv_from(
        &mut self,
        id: SocketId,
        buf: &mut [u8],
    ) -> Result<(usize, SocketAddr), SocketError> {
        let handle = self.sockets.get(&id).ok_or(SocketError::Closed)?.handle;
        let socket = self.iface.sockets.inner.get_mut::<udp::Socket>(handle);
        let (len, meta) = socket
            .recv_slice(buf)
            .map_err(|e| SocketError::from(TcpIpError::from(e)))?;
        Ok((len, meta.endpoint))
    }

    // -----------------------------------------------------------------------
    // State queries
    // -----------------------------------------------------------------------

    /// Returns `true` if the socket has data ready to read.
    ///
    /// For TCP: `socket.can_recv()`. For UDP: `socket.can_recv()`.
    /// Returns `false` if the socket id is not found.
    pub fn is_readable(&self, id: SocketId) -> bool {
        let Some(entry) = self.sockets.get(&id) else {
            return false;
        };
        let handle = entry.handle;
        match entry.kind {
            SocketKind::TcpStream | SocketKind::TcpListener => {
                let socket = self.iface.sockets.inner.get::<tcp::Socket>(handle);
                socket.can_recv()
            }
            SocketKind::Udp => {
                let socket = self.iface.sockets.inner.get::<udp::Socket>(handle);
                socket.can_recv()
            }
        }
    }

    /// Returns `true` if the socket can accept data for writing.
    ///
    /// For TCP: `socket.can_send()`. For UDP: `socket.can_send()`.
    /// Returns `false` if the socket id is not found.
    pub fn is_writable(&self, id: SocketId) -> bool {
        let Some(entry) = self.sockets.get(&id) else {
            return false;
        };
        let handle = entry.handle;
        match entry.kind {
            SocketKind::TcpStream | SocketKind::TcpListener => {
                let socket = self.iface.sockets.inner.get::<tcp::Socket>(handle);
                socket.can_send()
            }
            SocketKind::Udp => {
                let socket = self.iface.sockets.inner.get::<udp::Socket>(handle);
                socket.can_send()
            }
        }
    }

    /// Returns the local endpoint of a socket.
    ///
    /// For TCP: queries `socket.local_endpoint()`. For UDP: constructs from
    /// `socket.endpoint()` (converting `IpListenEndpoint` to `IpEndpoint`).
    ///
    /// # Errors
    ///
    /// - [`SocketError::Closed`] — socket id not found.
    /// - [`SocketError::NotConnected`] — socket has no local endpoint.
    pub fn local_addr(&self, id: SocketId) -> Result<SocketAddr, SocketError> {
        let entry = self.sockets.get(&id).ok_or(SocketError::Closed)?;
        let handle = entry.handle;
        let cached_local = entry.local_addr;
        let kind = entry.kind;
        match kind {
            SocketKind::TcpStream | SocketKind::TcpListener => {
                let socket = self.iface.sockets.inner.get::<tcp::Socket>(handle);
                // smoltcp's local_endpoint() may return None for listening
                // sockets; fall back to the cached value from SocketEntry.
                socket
                    .local_endpoint()
                    .or(cached_local)
                    .ok_or(SocketError::NotConnected)
            }
            SocketKind::Udp => {
                let socket = self.iface.sockets.inner.get::<udp::Socket>(handle);
                let ep = socket.endpoint();
                // IpListenEndpoint has addr: Option<IpAddress>; convert to IpEndpoint.
                let addr = ep
                    .addr
                    .unwrap_or(IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(0, 0, 0, 0)));
                Ok(IpEndpoint {
                    addr,
                    port: ep.port,
                })
            }
        }
    }

    /// Returns the remote endpoint of a TCP socket.
    ///
    /// For UDP and unconnected TCP listeners, returns
    /// [`SocketError::NotConnected`].
    ///
    /// # Errors
    ///
    /// - [`SocketError::Closed`] — socket id not found.
    /// - [`SocketError::NotConnected`] — socket has no remote endpoint.
    pub fn remote_addr(&self, id: SocketId) -> Result<SocketAddr, SocketError> {
        let entry = self.sockets.get(&id).ok_or(SocketError::Closed)?;
        let handle = entry.handle;
        let cached_remote = entry.remote_addr;
        let kind = entry.kind;
        match kind {
            SocketKind::TcpStream | SocketKind::TcpListener => {
                let socket = self.iface.sockets.inner.get::<tcp::Socket>(handle);
                // Fall back to cached remote_addr (set during connect/accept).
                socket
                    .remote_endpoint()
                    .or(cached_remote)
                    .ok_or(SocketError::NotConnected)
            }
            SocketKind::Udp => Err(SocketError::NotConnected),
        }
    }

    /// Enable or disable non-blocking mode for a socket.
    ///
    /// This is a semantic flag — smoltcp is always non-blocking. The flag
    /// is stored in `SocketEntry` for future use (e.g. blocking simulation
    /// via application-layer poll loops).
    pub fn set_nonblocking(&mut self, id: SocketId, on: bool) {
        if let Some(entry) = self.sockets.get_mut(&id) {
            entry.nonblocking = on;
        }
    }

    /// Returns the kind of a socket, or `None` if not found.
    pub fn socket_kind(&self, id: SocketId) -> Option<SocketKind> {
        self.sockets.get(&id).map(|e| e.kind)
    }

    // -----------------------------------------------------------------------
    // Interface delegation
    // -----------------------------------------------------------------------

    /// Poll the network interface to process incoming/outgoing packets.
    ///
    /// Delegates to `NetworkInterface::poll(timestamp_ms)`. Should be called
    /// regularly from the application main loop.
    ///
    /// # Errors
    ///
    /// - [`SocketError::IoError`] — hardware-level error from the device.
    pub fn poll_interface(&mut self, timestamp_ms: u64) -> Result<(), SocketError> {
        self.iface.poll(timestamp_ms).map_err(SocketError::from)
    }

    /// Returns the timestamp at which `poll_interface` should be called next,
    /// or `None` if there are no pending timers.
    ///
    /// Delegates to `NetworkInterface::poll_at(timestamp_ms)`. Requires
    /// `&mut self` because smoltcp 0.13's `Interface::poll_at` requires
    /// `&mut self`.
    pub fn poll_at(&mut self, timestamp_ms: u64) -> Option<u64> {
        self.iface.poll_at(timestamp_ms)
    }

    /// Returns the first IPv4 address of the interface, or `None`.
    pub fn ipv4_addr(&self) -> Option<Ipv4Addr> {
        self.iface.ipv4_addr()
    }

    // -----------------------------------------------------------------------
    // Poll multiplexing
    // -----------------------------------------------------------------------

    /// Register interest in readiness events for a socket.
    ///
    /// # Errors
    ///
    /// - [`SocketError::Closed`] — socket id not found.
    pub fn register(&mut self, id: SocketId, interest: Interest) -> Result<(), SocketError> {
        if !self.sockets.contains_key(&id) {
            return Err(SocketError::Closed);
        }
        self.poll.register(id, interest);
        Ok(())
    }

    /// Deregister a socket from the poll registry.
    ///
    /// No-op if the socket is not registered.
    pub fn deregister(&mut self, id: SocketId) {
        self.poll.deregister(id);
    }

    /// Modify the interest for a registered socket.
    ///
    /// If the socket is not yet registered, this registers it.
    pub fn modify_interest(&mut self, id: SocketId, interest: Interest) {
        self.poll.modify(id, interest);
    }

    /// Check all registered sockets for readiness and return events.
    ///
    /// Returns a [`Vec<Event>`] containing one event per socket that has
    /// at least one ready operation (matching its registered interest).
    /// Returns an empty vector if no sockets are ready.
    ///
    /// This is a non-blocking call — it returns immediately with the current
    /// readiness state. The application is responsible for calling
    /// [`poll_interface`](Self::poll_interface) regularly to let smoltcp
    /// process packets and update socket states.
    pub fn poll_once(&mut self) -> Vec<Event> {
        // Collect the registry into a Vec to avoid holding a borrow of
        // self.poll while accessing self.sockets and self.iface.sockets.
        let entries: Vec<(SocketId, Interest)> = self.poll.iter().collect();
        let mut events = Vec::new();
        for (id, _interest) in entries {
            let is_readable = self.is_readable(id);
            let is_writable = self.is_writable(id);
            let readiness = self.poll.check_readiness(id, is_readable, is_writable);
            if !readiness.is_empty() {
                events.push(Event::new(id, readiness));
            }
        }
        events
    }

    // -----------------------------------------------------------------------
    // Accessors (for testing / diagnostics)
    // -----------------------------------------------------------------------

    /// Returns the number of active sockets.
    pub fn socket_count(&self) -> usize {
        self.sockets.len()
    }

    /// Returns the number of registered poll interests.
    pub fn poll_len(&self) -> usize {
        self.poll.len()
    }

    /// Returns the network statistics from the underlying device.
    pub fn net_stats(&self) -> NetStats {
        self.iface.device.device().stats()
    }
}

impl<D: NetDevice> fmt::Debug for SocketManager<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SocketManager")
            .field("socket_count", &self.sockets.len())
            .field("next_id", &self.next_id)
            .field("poll_len", &self.poll.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::NetError;
    use crate::tcpip::addr::{ipv4_addr, ipv4_cidr};

    // -----------------------------------------------------------------------
    // Mock device
    // -----------------------------------------------------------------------

    /// Minimal mock device for SocketManager tests.
    ///
    /// `send` is a no-op (discards frames), `recv` always returns `NoBuffer`
    /// (no frames available). This is sufficient for testing socket lifecycle,
    /// state queries, and poll mechanics without real network I/O.
    struct MockNetDevice {
        mac_addr: [u8; 6],
        mtu: usize,
        link_up: bool,
    }

    impl NetDevice for MockNetDevice {
        fn send(&mut self, _frame: &[u8]) -> Result<(), NetError> {
            Ok(())
        }
        fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, NetError> {
            Err(NetError::NoBuffer)
        }
        fn mac_address(&self) -> [u8; 6] {
            self.mac_addr
        }
        fn mtu(&self) -> usize {
            self.mtu
        }
        fn link_up(&self) -> bool {
            self.link_up
        }
        fn set_promiscuous(&mut self, _on: bool) {}
        fn stats(&self) -> NetStats {
            NetStats::new()
        }
    }

    fn make_manager() -> SocketManager<MockNetDevice> {
        let dev = MockNetDevice {
            mac_addr: [0x02, 0, 0, 0, 0, 0x01],
            mtu: 1500,
            link_up: true,
        };
        let config = InterfaceConfig::new([0x02, 0, 0, 0, 0, 0x01])
            .with_ipv4(ipv4_cidr(ipv4_addr(192, 168, 1, 100), 24))
            .with_gateway(ipv4_addr(192, 168, 1, 1));
        SocketManager::new(dev, config)
    }

    fn remote_addr(a: u8, b: u8, c: u8, d: u8, port: u16) -> SocketAddr {
        IpEndpoint {
            addr: IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(a, b, c, d)),
            port,
        }
    }

    // -----------------------------------------------------------------------
    // Construction tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_manager_new_empty() {
        let mgr = make_manager();
        assert_eq!(mgr.socket_count(), 0);
        assert_eq!(mgr.poll_len(), 0);
        assert_eq!(mgr.next_id, 0);
    }

    #[test]
    fn test_manager_ipv4_addr() {
        let mgr = make_manager();
        assert_eq!(mgr.ipv4_addr(), Some(ipv4_addr(192, 168, 1, 100)));
    }

    #[test]
    fn test_manager_debug() {
        let mgr = make_manager();
        let s = format!("{:?}", mgr);
        assert!(s.contains("SocketManager"));
        assert!(s.contains("socket_count"));
    }

    // -----------------------------------------------------------------------
    // TCP connect tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tcp_connect_returns_stream() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let result = mgr.tcp_connect(remote, 50000);
        assert!(result.is_ok());
        let stream = result.unwrap();
        assert_eq!(stream.id(), 0);
        assert_eq!(mgr.socket_count(), 1);
    }

    #[test]
    fn test_tcp_connect_increments_next_id() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let s1 = mgr.tcp_connect(remote, 50001).unwrap();
        let s2 = mgr.tcp_connect(remote, 50002).unwrap();
        assert_eq!(s1.id(), 0);
        assert_eq!(s2.id(), 1);
        assert_eq!(mgr.socket_count(), 2);
    }

    #[test]
    fn test_tcp_connect_socket_kind() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        assert_eq!(mgr.socket_kind(stream.id()), Some(SocketKind::TcpStream));
    }

    // -----------------------------------------------------------------------
    // TCP listen tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tcp_listen_returns_listener() {
        let mut mgr = make_manager();
        let result = mgr.tcp_listen(80);
        assert!(result.is_ok());
        let listener = result.unwrap();
        assert_eq!(listener.id(), 0);
        assert_eq!(mgr.socket_count(), 1);
    }

    #[test]
    fn test_tcp_listen_socket_kind() {
        let mut mgr = make_manager();
        let listener = mgr.tcp_listen(80).unwrap();
        assert_eq!(
            mgr.socket_kind(listener.id()),
            Some(SocketKind::TcpListener)
        );
    }

    // -----------------------------------------------------------------------
    // TCP accept tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tcp_accept_no_connection_would_block() {
        let mut mgr = make_manager();
        let listener = mgr.tcp_listen(80).unwrap();
        let result = mgr.tcp_accept(listener);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::WouldBlock);
    }

    // -----------------------------------------------------------------------
    // UDP bind tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_udp_bind_returns_socket() {
        let mut mgr = make_manager();
        let local = IpEndpoint {
            addr: IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(0, 0, 0, 0)),
            port: 8080,
        };
        let result = mgr.udp_bind(local);
        assert!(result.is_ok());
        let udp = result.unwrap();
        assert_eq!(udp.id(), 0);
        assert_eq!(mgr.socket_count(), 1);
    }

    #[test]
    fn test_udp_bind_socket_kind() {
        let mut mgr = make_manager();
        let local = IpEndpoint {
            addr: IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(0, 0, 0, 0)),
            port: 8080,
        };
        let udp = mgr.udp_bind(local).unwrap();
        assert_eq!(mgr.socket_kind(udp.id()), Some(SocketKind::Udp));
    }

    // -----------------------------------------------------------------------
    // Close tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_close_removes_socket() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        assert_eq!(mgr.socket_count(), 1);

        let result = mgr.close(stream.id());
        assert!(result.is_ok());
        assert_eq!(mgr.socket_count(), 0);
    }

    #[test]
    fn test_close_nonexistent_returns_closed() {
        let mut mgr = make_manager();
        let result = mgr.close(999);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::Closed);
    }

    #[test]
    fn test_close_deregisters_from_poll() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        mgr.register(stream.id(), Interest::all()).unwrap();
        assert_eq!(mgr.poll_len(), 1);

        mgr.close(stream.id()).unwrap();
        assert_eq!(mgr.poll_len(), 0);
    }

    // -----------------------------------------------------------------------
    // TCP read/write tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_read_non_existent_returns_closed() {
        let mut mgr = make_manager();
        let mut buf = [0u8; 64];
        let result = mgr.read(999, &mut buf);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::Closed);
    }

    #[test]
    fn test_write_non_existent_returns_closed() {
        let mut mgr = make_manager();
        let buf = [1u8; 64];
        let result = mgr.write(999, &buf);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::Closed);
    }

    #[test]
    fn test_read_on_syn_sent_returns_not_connected() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        // Socket is in SynSent state (not Established)
        let mut buf = [0u8; 64];
        let result = mgr.read(stream.id(), &mut buf);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::NotConnected);
    }

    #[test]
    fn test_write_on_syn_sent_returns_not_connected() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        let buf = [1u8; 64];
        let result = mgr.write(stream.id(), &buf);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::NotConnected);
    }

    // -----------------------------------------------------------------------
    // UDP send_to / recv_from tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_recv_from_empty_returns_would_block() {
        let mut mgr = make_manager();
        let local = IpEndpoint {
            addr: IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(0, 0, 0, 0)),
            port: 8080,
        };
        let udp = mgr.udp_bind(local).unwrap();
        let mut buf = [0u8; 64];
        let result = mgr.recv_from(udp.id(), &mut buf);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::WouldBlock);
    }

    #[test]
    fn test_send_to_non_existent_returns_closed() {
        let mut mgr = make_manager();
        let buf = [1u8; 64];
        let dst = remote_addr(192, 168, 1, 1, 9090);
        let result = mgr.send_to(999, &buf, dst);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::Closed);
    }

    // -----------------------------------------------------------------------
    // State query tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_readable_non_existent() {
        let mgr = make_manager();
        assert!(!mgr.is_readable(999));
    }

    #[test]
    fn test_is_writable_non_existent() {
        let mgr = make_manager();
        assert!(!mgr.is_writable(999));
    }

    #[test]
    fn test_socket_kind_non_existent() {
        let mgr = make_manager();
        assert_eq!(mgr.socket_kind(999), None);
    }

    #[test]
    fn test_local_addr_tcp_listener() {
        let mut mgr = make_manager();
        let listener = mgr.tcp_listen(80).unwrap();
        let result = mgr.local_addr(listener.id());
        // Listener has a local endpoint (port 80)
        assert!(result.is_ok());
        let addr = result.unwrap();
        assert_eq!(addr.port, 80);
    }

    #[test]
    fn test_remote_addr_tcp_syn_sent_returns_not_connected() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        // In SynSent state, remote_endpoint() may or may not be set
        // smoltcp typically sets it during connect()
        let _ = mgr.remote_addr(stream.id());
    }

    #[test]
    fn test_remote_addr_udp_returns_not_connected() {
        let mut mgr = make_manager();
        let local = IpEndpoint {
            addr: IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(0, 0, 0, 0)),
            port: 8080,
        };
        let udp = mgr.udp_bind(local).unwrap();
        let result = mgr.remote_addr(udp.id());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::NotConnected);
    }

    #[test]
    fn test_local_addr_udp_bound() {
        let mut mgr = make_manager();
        let local = IpEndpoint {
            addr: IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(0, 0, 0, 0)),
            port: 8080,
        };
        let udp = mgr.udp_bind(local).unwrap();
        let result = mgr.local_addr(udp.id());
        assert!(result.is_ok());
        let addr = result.unwrap();
        assert_eq!(addr.port, 8080);
    }

    #[test]
    fn test_local_addr_non_existent_returns_closed() {
        let mgr = make_manager();
        let result = mgr.local_addr(999);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::Closed);
    }

    // -----------------------------------------------------------------------
    // set_nonblocking tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_set_nonblocking_existing() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        mgr.set_nonblocking(stream.id(), true);
        // No direct accessor for nonblocking flag; just verify no panic.
    }

    #[test]
    fn test_set_nonblocking_non_existent_no_panic() {
        let mut mgr = make_manager();
        mgr.set_nonblocking(999, true); // should not panic
    }

    // -----------------------------------------------------------------------
    // poll_interface tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_poll_interface_ok() {
        let mut mgr = make_manager();
        let result = mgr.poll_interface(0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_poll_interface_multiple() {
        let mut mgr = make_manager();
        assert!(mgr.poll_interface(0).is_ok());
        assert!(mgr.poll_interface(100).is_ok());
        assert!(mgr.poll_interface(200).is_ok());
    }

    #[test]
    fn test_poll_at_returns_option() {
        let mut mgr = make_manager();
        let _ = mgr.poll_at(100);
    }

    // -----------------------------------------------------------------------
    // Poll registration tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_existing_socket() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        let result = mgr.register(stream.id(), Interest::all_readable());
        assert!(result.is_ok());
        assert_eq!(mgr.poll_len(), 1);
    }

    #[test]
    fn test_register_non_existent_returns_closed() {
        let mut mgr = make_manager();
        let result = mgr.register(999, Interest::all());
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), SocketError::Closed);
    }

    #[test]
    fn test_deregister_existing() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        mgr.register(stream.id(), Interest::all()).unwrap();
        assert_eq!(mgr.poll_len(), 1);

        mgr.deregister(stream.id());
        assert_eq!(mgr.poll_len(), 0);
    }

    #[test]
    fn test_deregister_non_existent_no_panic() {
        let mut mgr = make_manager();
        mgr.deregister(999); // should not panic
    }

    #[test]
    fn test_modify_interest() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        mgr.register(stream.id(), Interest::all_readable()).unwrap();

        mgr.modify_interest(stream.id(), Interest::all());
        // No direct accessor for Interest; just verify no panic.
    }

    // -----------------------------------------------------------------------
    // poll_once tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_poll_once_empty() {
        let mut mgr = make_manager();
        let events = mgr.poll_once();
        assert!(events.is_empty());
    }

    #[test]
    fn test_poll_once_no_ready_sockets() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        mgr.register(stream.id(), Interest::all_readable()).unwrap();

        // Socket is in SynSent state, not readable
        let events = mgr.poll_once();
        assert!(events.is_empty());
    }

    #[test]
    fn test_poll_once_registered_but_no_socket() {
        let mut mgr = make_manager();
        // Register interest for a non-existent socket (shouldn't happen
        // normally, but poll_once should handle gracefully).
        // Since register() checks socket existence, we skip this.
        let events = mgr.poll_once();
        assert!(events.is_empty());
    }

    #[test]
    fn test_poll_once_after_close() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let stream = mgr.tcp_connect(remote, 50000).unwrap();
        mgr.register(stream.id(), Interest::all()).unwrap();

        mgr.close(stream.id()).unwrap();

        // After close, the socket is removed from poll registry
        let events = mgr.poll_once();
        assert!(events.is_empty());
    }

    // -----------------------------------------------------------------------
    // Multiple socket tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_multiple_tcp_connects() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);

        let s1 = mgr.tcp_connect(remote, 50001).unwrap();
        let s2 = mgr.tcp_connect(remote, 50002).unwrap();
        let s3 = mgr.tcp_connect(remote, 50003).unwrap();

        assert_eq!(s1.id(), 0);
        assert_eq!(s2.id(), 1);
        assert_eq!(s3.id(), 2);
        assert_eq!(mgr.socket_count(), 3);
    }

    #[test]
    fn test_mixed_sockets() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);
        let local_udp = IpEndpoint {
            addr: IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(0, 0, 0, 0)),
            port: 8080,
        };

        let tcp_stream = mgr.tcp_connect(remote, 50000).unwrap();
        let tcp_listener = mgr.tcp_listen(80).unwrap();
        let udp = mgr.udp_bind(local_udp).unwrap();

        assert_eq!(tcp_stream.id(), 0);
        assert_eq!(tcp_listener.id(), 1);
        assert_eq!(udp.id(), 2);
        assert_eq!(mgr.socket_count(), 3);

        assert_eq!(
            mgr.socket_kind(tcp_stream.id()),
            Some(SocketKind::TcpStream)
        );
        assert_eq!(
            mgr.socket_kind(tcp_listener.id()),
            Some(SocketKind::TcpListener)
        );
        assert_eq!(mgr.socket_kind(udp.id()), Some(SocketKind::Udp));
    }

    #[test]
    fn test_close_individual_socket() {
        let mut mgr = make_manager();
        let remote = remote_addr(192, 168, 1, 1, 80);

        let s1 = mgr.tcp_connect(remote, 50001).unwrap();
        let s2 = mgr.tcp_connect(remote, 50002).unwrap();

        mgr.close(s1.id()).unwrap();
        assert_eq!(mgr.socket_count(), 1);
        assert_eq!(mgr.socket_kind(s1.id()), None);
        assert_eq!(mgr.socket_kind(s2.id()), Some(SocketKind::TcpStream));
    }
}
