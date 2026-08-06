//! Modbus TCP slave server — a real `std::net::TcpListener` based server.
//!
//! This is the host-side reference slave used for independent-peer
//! interoperability evidence (NET-010). It speaks real Modbus TCP over a real
//! TCP socket — not an in-memory channel. The integration test connects with
//! a raw `std::net::TcpStream` client that builds MBAP+PDU frames by hand, so
//! the peer is a genuinely independent implementation (it shares no framing
//! code with this crate).
//!
//! # Write path (control-gateway 纵向闭环)
//!
//! By default the server holds no authorized command, so every write is
//! default-denied at the handler level (see [`crate::handler::WritePolicy`]).
//! A caller that has obtained an [`AuthorizedCommand`] from the control
//! gateway installs it with [`ModbusTcpServer::set_authorized_write`]; the
//! next matching write is then applied. This is the single chokepoint through
//! which writes reach the register store.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use eneros_control_gateway::AuthorizedCommand;
use eneros_modbus_tcp::MbapHeader;

use crate::handler::{
    encode_exception, encode_response, handle_frame, HandleOutcome, RegisterStore, WritePolicy,
};

/// Modbus TCP slave server.
pub struct ModbusTcpServer {
    listener: TcpListener,
    store: RegisterStore,
    /// The currently-authorized write command, if any. When `None`, all
    /// writes are default-denied.
    authorized: Option<AuthorizedCommand>,
}

impl ModbusTcpServer {
    /// Binds a new server to `addr` with an all-zero register store of
    /// `register_count` holding registers.
    pub fn new(addr: &str, register_count: usize) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        Ok(Self {
            listener,
            store: RegisterStore::new(register_count),
            authorized: None,
        })
    }

    /// Returns the address actually bound (useful when binding to `127.0.0.1:0`
    /// for an ephemeral port in tests).
    pub fn local_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.listener.local_addr()
    }

    /// Installs an [`AuthorizedCommand`] so the next matching write is applied
    /// instead of default-denied. This is the control-gateway handoff point.
    pub fn set_authorized_write(&mut self, cmd: AuthorizedCommand) {
        self.authorized = Some(cmd);
    }

    /// Clears any pending authorized write (re-arm default-deny).
    pub fn clear_authorized_write(&mut self) {
        self.authorized = None;
    }

    /// Read-only access to the backing register store (for test assertions).
    pub fn registers(&self) -> &[u16] {
        &self.store.regs
    }

    /// Writes a value directly into the backing store (test seed / internal
    /// setup only — NOT a protocol write path).
    pub fn seed_register(&mut self, addr: usize, value: u16) {
        if addr < self.store.regs.len() {
            self.store.regs[addr] = value;
        }
    }

    /// Accepts and serves exactly one connection until the peer closes it or
    /// sends a malformed frame. Returns the number of frames processed.
    pub fn serve_one(&mut self) -> std::io::Result<usize> {
        let (mut stream, _) = self.listener.accept()?;
        self.handle_connection(&mut stream)
    }

    /// Serves connections forever (blocks). Each connection is handled
    /// sequentially in this thread.
    pub fn serve_forever(&mut self) -> std::io::Result<()> {
        loop {
            let (mut stream, _) = self.listener.accept()?;
            let _ = self.handle_connection(&mut stream);
        }
    }

    fn handle_connection(&mut self, stream: &mut TcpStream) -> std::io::Result<usize> {
        // A modest read timeout so a misbehaving peer cannot stall the server
        // forever on a partial frame.
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));

        let mut frames = 0usize;
        loop {
            // 1. Read the 7-byte MBAP header.
            let mut header_buf = [0u8; 7];
            match read_exact_or_eof(stream, &mut header_buf) {
                Ok(true) => {}
                Ok(false) => return Ok(frames), // peer closed cleanly
                Err(e) => return Err(e),
            }

            let header = match MbapHeader::decode(&header_buf) {
                Ok(h) => h,
                Err(_) => {
                    // Bad MBAP (e.g. wrong protocol id) — close the connection.
                    return Ok(frames);
                }
            };

            // 2. Read the declared PDU bytes (length - 1 for unit_id).
            if header.length == 0 {
                return Ok(frames);
            }
            let pdu_len = (header.length - 1) as usize;
            let mut frame = header_buf.to_vec();
            if pdu_len > 0 {
                let mut pdu = vec![0u8; pdu_len];
                match read_exact_or_eof(stream, &mut pdu) {
                    Ok(true) => {}
                    Ok(false) => return Ok(frames),
                    Err(e) => return Err(e),
                }
                frame.extend_from_slice(&pdu);
            }

            // 3. Capture the request function code for exception encoding.
            let request_fc = if frame.len() > 7 { frame[7] } else { 0 };

            // 4. Dispatch.
            let policy = match &self.authorized {
                Some(cmd) => WritePolicy::Authorized(cmd),
                None => WritePolicy::DenyAll,
            };
            let outcome = handle_frame(&frame, &mut self.store, policy);
            frames += 1;

            // 5. Encode and send the response.
            match outcome {
                HandleOutcome::Response(resp) => {
                    let out = encode_response(&resp);
                    stream.write_all(&out)?;
                }
                HandleOutcome::Exception(exc) => {
                    let out = encode_exception(&exc, request_fc);
                    stream.write_all(&out)?;
                }
                HandleOutcome::CloseConnection => {
                    // Drop the connection without a response.
                    return Ok(frames);
                }
            }
        }
    }
}

/// Reads exactly `buf.len()` bytes, returning `Ok(false)` on a clean EOF
/// (peer closed before any byte of this read) and `Ok(true)` when the buffer
/// is filled. A partial read after some bytes is an error (truncated frame).
fn read_exact_or_eof(stream: &mut TcpStream, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = stream.read(&mut buf[filled..])?;
        if n == 0 {
            if filled == 0 {
                return Ok(false); // clean close at frame boundary
            }
            // Partial frame — treat as a truncated/failed read.
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated modbus frame",
            ));
        }
        filled += n;
    }
    Ok(true)
}
