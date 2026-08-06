//! NET-010 independent-peer interoperability evidence.
//!
//! This test connects to the slave with a RAW `std::net::TcpStream` client
//! that builds Modbus TCP frames (MBAP + PDU) by hand. The client shares NO
//! framing code with the server crate — it is a genuinely independent peer
//! implementation, satisfying the "no in-library loopback masquerading as
//! interop" requirement.
//!
//! Cases:
//! - **Positive**: read holding registers (0x03) returns the seeded values.
//! - **Negative**: illegal function code, illegal address, illegal data value,
//!   malformed MBAP (wrong protocol id), and write-default-deny all produce
//!   the correct Modbus exception or connection close.
//! - **Control-gateway 闭环**: with no `AuthorizedCommand` installed, a write
//!   (0x06) is denied with `IllegalFunction`; after installing a matching
//!   `AuthorizedCommand` (obtained from the real control gateway), the same
//!   write is applied and the register changes.

// Test module: a direct `panic!` is used for the "server sent bytes for a
// malformed frame" branch (a real bug if it ever fires). The workspace
// clippy.toml disallows `panic!` in non-test code; test modules are
// explicitly exempted ("测试模块请用 #[allow] 豁免").
#![allow(clippy::disallowed_macros)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use eneros_control_gateway::{ControlGateway, EnvelopeBuilder};
use eneros_telemetry_model::command::{ControlCommand, SingleCommand};

const KEY: &[u8] = b"interop-test-key-0123456789aa";

/// Builds a raw Modbus TCP frame (MBAP + PDU) — independent framing.
fn build_frame(txn: u16, unit: u8, pdu: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(7 + pdu.len());
    buf.extend_from_slice(&txn.to_be_bytes()); // transaction_id
    buf.extend_from_slice(&0u16.to_be_bytes()); // protocol_id = 0
    let length = (pdu.len() as u16) + 1; // +1 for unit_id
    buf.extend_from_slice(&length.to_be_bytes());
    buf.push(unit);
    buf.extend_from_slice(pdu);
    buf
}

/// Sends a frame and reads one full response (MBAP + PDU).
fn exchange(stream: &mut TcpStream, frame: &[u8]) -> Vec<u8> {
    stream.write_all(frame).expect("write request");
    stream.flush().expect("flush");
    // Read 7-byte MBAP header.
    let mut header = [0u8; 7];
    stream.read_exact(&mut header).expect("read header");
    let length = u16::from_be_bytes([header[4], header[5]]) as usize;
    assert!(length >= 1, "response length must include unit_id");
    let pdu_len = length - 1;
    let mut pdu = vec![0u8; pdu_len];
    stream.read_exact(&mut pdu).expect("read pdu");
    let mut full = header.to_vec();
    full.extend_from_slice(&pdu);
    full
}

/// Starts a server on an ephemeral port in a background thread and returns
/// its address plus a join handle. The server handles exactly one connection
/// (all frames on that connection) then the thread ends.
fn start_server(
    register_count: usize,
    seed: &[(usize, u16)],
    authorized: Option<eneros_control_gateway::AuthorizedCommand>,
) -> (SocketAddr, std::thread::JoinHandle<()>) {
    use eneros_modbus_tcp_server::ModbusTcpServer;
    let mut server = ModbusTcpServer::new("127.0.0.1:0", register_count).expect("bind");
    for &(addr, val) in seed {
        server.seed_register(addr, val);
    }
    if let Some(cmd) = authorized {
        server.set_authorized_write(cmd);
    }
    let addr = server.local_addr().expect("local_addr");
    let handle = std::thread::spawn(move || {
        let _ = server.serve_one();
    });
    (addr, handle)
}

#[test]
fn test_positive_read_holding_registers() {
    let (addr, handle) = start_server(16, &[(0, 0x1111), (1, 0x2222), (2, 0x3333)], None);

    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // FC 0x03: read 3 registers starting at address 0.
    let pdu = [0x03, 0x00, 0x00, 0x00, 0x03];
    let req = build_frame(0x0001, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);

    // MBAP: txn=0x0001, proto=0, length=1+1+1+6=9, unit=0x01
    assert_eq!(&resp[0..2], &[0x00, 0x01]); // txn
    assert_eq!(&resp[2..4], &[0x00, 0x00]); // proto
    assert_eq!(resp[6], 0x01); // unit
                               // PDU: fc=0x03, byte_count=6, then 3 registers big-endian.
    assert_eq!(resp[7], 0x03);
    assert_eq!(resp[8], 6);
    assert_eq!(&resp[9..15], &[0x11, 0x11, 0x22, 0x22, 0x33, 0x33]);

    drop(stream);
    handle.join().unwrap();
}

#[test]
fn test_negative_illegal_function_code() {
    let (addr, handle) = start_server(16, &[], None);
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // FC 0x02 (read discrete inputs) — unsupported by this slave.
    let pdu = [0x02, 0x00, 0x00, 0x00, 0x01];
    let req = build_frame(0x0002, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);

    // Exception response: fc|0x80 = 0x82, code = 0x01 (IllegalFunction).
    assert_eq!(resp[7], 0x82);
    assert_eq!(resp[8], 0x01);

    drop(stream);
    handle.join().unwrap();
}

#[test]
fn test_negative_illegal_address() {
    let (addr, handle) = start_server(4, &[], None); // only 4 registers
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // Read 2 registers starting at address 3 -> range [3,5) exceeds 4.
    let pdu = [0x03, 0x00, 0x03, 0x00, 0x02];
    let req = build_frame(0x0003, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);

    assert_eq!(resp[7], 0x83); // fc|0x80
    assert_eq!(resp[8], 0x02); // IllegalDataAddress

    drop(stream);
    handle.join().unwrap();
}

#[test]
fn test_negative_illegal_data_value_quantity_zero() {
    let (addr, handle) = start_server(16, &[], None);
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // quantity = 0 -> IllegalDataValue.
    let pdu = [0x03, 0x00, 0x00, 0x00, 0x00];
    let req = build_frame(0x0004, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);

    assert_eq!(resp[7], 0x83);
    assert_eq!(resp[8], 0x03); // IllegalDataValue
    drop(stream);
    handle.join().unwrap();
}

#[test]
fn test_negative_illegal_data_value_quantity_too_large() {
    let (addr, handle) = start_server(16, &[], None);
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // quantity = 200 (> 125) -> IllegalDataValue.
    let pdu = [0x03, 0x00, 0x00, 0x00, 0xC8];
    let req = build_frame(0x0005, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);

    assert_eq!(resp[7], 0x83);
    assert_eq!(resp[8], 0x03);
    drop(stream);
    handle.join().unwrap();
}

#[test]
fn test_negative_malformed_mbap_closes_connection() {
    let (addr, handle) = start_server(16, &[], None);
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // Send a frame with protocol_id = 1 (not Modbus). The server must close
    // the connection without a response. We accept EITHER a clean EOF
    // (read returns Ok(0)) OR a connection-reset error — both are valid
    // platform-level signals that the peer closed the socket. On Linux a
    // graceful close yields Ok(0); on Windows a non-graceful close yields
    // WSAECONNRESET. The invariant under test is "no Modbus response is
    // sent for a malformed MBAP frame", which both outcomes satisfy.
    let bad = vec![0x00, 0x09, 0x00, 0x01, 0x00, 0x02, 0x01, 0x03, 0x00];
    stream.write_all(&bad).expect("write bad frame");
    stream.flush().expect("flush");

    let mut buf = [0u8; 16];
    let closed = match stream.read(&mut buf) {
        Ok(0) => true, // graceful EOF (Linux)
        Ok(n) => {
            // Unexpected: the server sent bytes back for a
            // malformed MBAP frame — that would be a real bug.
            panic!(
                "server sent {} bytes for a malformed MBAP frame: {:?}",
                n,
                &buf[..n]
            );
        }
        Err(e) => {
            e.kind() == std::io::ErrorKind::ConnectionReset
                || e.kind() == std::io::ErrorKind::ConnectionAborted
        }
    };
    assert!(
        closed,
        "server should have closed the connection (got bytes or unexpected error)"
    );

    drop(stream);
    handle.join().unwrap();
}

#[test]
fn test_write_is_default_denied_without_authorization() {
    let (addr, handle) = start_server(16, &[(5, 0x0000)], None);
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // FC 0x06 write single register: addr=5, value=0x1234. No AuthorizedCommand
    // is installed, so this MUST be denied with IllegalFunction (0x01).
    let pdu = [0x06, 0x00, 0x05, 0x12, 0x34];
    let req = build_frame(0x0006, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);

    assert_eq!(resp[7], 0x86); // fc|0x80
    assert_eq!(resp[8], 0x01); // IllegalFunction (default deny)

    // Verify via a follow-up read that the register was NOT modified.
    let pdu = [0x03, 0x00, 0x05, 0x00, 0x01];
    let req = build_frame(0x0007, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);
    assert_eq!(resp[7], 0x03);
    assert_eq!(&resp[9..11], &[0x00, 0x00]); // still 0x0000

    drop(stream);
    handle.join().unwrap();
}

#[test]
fn test_write_succeeds_through_control_gateway() {
    // Build a real AuthorizedCommand via the control gateway, then install it
    // on the server. The matching write must be applied; the register changes.
    let mut gw = ControlGateway::new(KEY, 5, 64);
    let now: u64 = 1_700_000_000;
    let envelope = EnvelopeBuilder::new(
        [9u8; 16],
        1, // device_id -> matches unit_id 1
        5, // point_id -> matches register address 5
        ControlCommand::Single(SingleCommand::On),
        now,
        now + 30,
        [0xEE; 16],
    )
    .lease([3u8; 16], 7)
    .policy_version(3)
    .build(KEY);
    let authorized = gw.authorize(&envelope.encode(), now).expect("authorize");

    let (addr, handle) = start_server(16, &[(5, 0x0000)], Some(authorized));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // FC 0x06: addr=5, value=0xFF00 (SingleCommand::On convention). This
    // matches the authorized command, so the write MUST succeed.
    let pdu = [0x06, 0x00, 0x05, 0xFF, 0x00];
    let req = build_frame(0x0008, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);

    // Normal response echoes the request PDU.
    assert_eq!(resp[7], 0x06);
    assert_eq!(&resp[8..12], &[0x00, 0x05, 0xFF, 0x00]);

    // Verify the register actually changed.
    let pdu = [0x03, 0x00, 0x05, 0x00, 0x01];
    let req = build_frame(0x0009, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);
    assert_eq!(resp[7], 0x03);
    assert_eq!(&resp[9..11], &[0xFF, 0x00]); // now 0xFF00

    drop(stream);
    handle.join().unwrap();
}

#[test]
fn test_write_with_wrong_value_is_denied_even_when_authorized() {
    // An AuthorizedCommand for value 0xFF00 (On) must NOT permit a write of
    // 0x0001 (an arbitrary non-matching value). This proves the gateway binds
    // the exact value, not just the address.
    let mut gw = ControlGateway::new(KEY, 5, 64);
    let now: u64 = 1_700_000_000;
    let envelope = EnvelopeBuilder::new(
        [10u8; 16],
        1,
        5,
        ControlCommand::Single(SingleCommand::On),
        now,
        now + 30,
        [0xEE; 16],
    )
    .lease([3u8; 16], 7)
    .policy_version(3)
    .build(KEY);
    let authorized = gw.authorize(&envelope.encode(), now).expect("authorize");

    let (addr, handle) = start_server(16, &[(5, 0x0000)], Some(authorized));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // Write value 0x0001 — does NOT match SingleCommand::On (0xFF00).
    let pdu = [0x06, 0x00, 0x05, 0x00, 0x01];
    let req = build_frame(0x000A, 0x01, &pdu);
    let resp = exchange(&mut stream, &req);

    assert_eq!(resp[7], 0x86); // exception
    assert_eq!(resp[8], 0x01); // IllegalFunction (denied — value mismatch)

    drop(stream);
    handle.join().unwrap();
}
