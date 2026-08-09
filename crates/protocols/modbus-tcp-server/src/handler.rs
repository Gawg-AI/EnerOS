//! PDU handler for the Modbus TCP slave.
//!
//! Parses a PDU (the bytes after the unit_id in an MBAP frame) and produces
//! either a normal response PDU or a Modbus exception PDU. All parse paths
//! are length-bounded: a PDU whose length does not match the function code's
//! grammar is rejected with [`ExceptionCode::IllegalDataValue`] before any
//! field is trusted.
//!
//! # Write policy (default deny)
//!
//! Writes (function codes 0x05/0x06/0x0F/0x10) are **default-deny**. With
//! [`WritePolicy::DenyAll`] (the default) every write is answered with an
//! [`ExceptionCode::IllegalFunction`] response and no register is touched.
//! With [`WritePolicy::Authorized`] writes are only applied when the supplied
//! [`AuthorizedCommand`] matches the request's unit/address/value — this is
//! the control-gateway integration point (NET-010 纵向闭环).

use eneros_control_gateway::AuthorizedCommand;
use eneros_modbus_rtu::ExceptionCode;
use eneros_modbus_tcp::MbapHeader;

/// A normal (non-exception) response PDU, returned to the framer.
pub struct Response {
    /// The transaction id copied from the request MBAP header.
    pub transaction_id: u16,
    /// The unit id copied from the request MBAP header.
    pub unit_id: u8,
    /// The response PDU bytes (function code + data, no MBAP).
    pub pdu: Vec<u8>,
}

/// A Modbus exception response.
pub struct ExceptionResponse {
    pub transaction_id: u16,
    pub unit_id: u8,
    pub code: ExceptionCode,
}

/// Write policy. See the module docs for the default-deny contract.
pub enum WritePolicy<'a> {
    /// Reject every write with `IllegalFunction`. No register is modified.
    DenyAll,
    /// Allow a write only if it matches the supplied [`AuthorizedCommand`].
    /// The unit_id, start address, and value(s) must all match; otherwise the
    /// write is denied with `IllegalFunction`.
    Authorized(&'a AuthorizedCommand),
}

/// Backing register store. The slave serves holding registers out of this.
pub struct RegisterStore {
    /// Holding registers, indexed by address. address 0 -> regs`0`.
    pub regs: Vec<u16>,
}

impl RegisterStore {
    pub fn new(count: usize) -> Self {
        Self {
            regs: vec![0u16; count],
        }
    }

    /// Reads `quantity` registers starting at `start`. Returns `None` if the
    /// range is out of bounds (caller maps that to IllegalDataAddress).
    pub fn read(&self, start: u16, quantity: u16) -> Option<Vec<u16>> {
        let start = start as usize;
        let quantity = quantity as usize;
        if start.checked_add(quantity)? > self.regs.len() {
            return None;
        }
        Some(self.regs[start..start + quantity].to_vec())
    }

    /// Writes a single register. Returns `false` if out of bounds.
    pub fn write_single(&mut self, addr: u16, value: u16) -> bool {
        let addr = addr as usize;
        if addr >= self.regs.len() {
            return false;
        }
        self.regs[addr] = value;
        true
    }

    /// Writes multiple registers. Returns `false` if out of bounds.
    pub fn write_multiple(&mut self, start: u16, values: &[u16]) -> bool {
        let start = start as usize;
        if start
            .checked_add(values.len())
            .map_or(true, |end| end > self.regs.len())
        {
            return false;
        }
        for (i, v) in values.iter().enumerate() {
            self.regs[start + i] = *v;
        }
        true
    }
}

/// Outcome of handling one PDU.
pub enum HandleOutcome {
    Response(Response),
    Exception(ExceptionResponse),
    /// The frame was malformed at the MBAP level (bad protocol id / truncated).
    /// The connection should be closed without a response.
    CloseConnection,
}

/// Handles a single received MBAP frame. `buf` is the full frame including the
/// 7-byte MBAP header.
pub fn handle_frame(
    buf: &[u8],
    store: &mut RegisterStore,
    write_policy: WritePolicy<'_>,
) -> HandleOutcome {
    // 1. Decode MBAP header (needs at least 7 bytes).
    let header = match MbapHeader::decode(buf) {
        Ok(h) => h,
        Err(_) => return HandleOutcome::CloseConnection,
    };

    // 2. Validate the declared length against the actual buffer.
    // length field = unit_id (1) + PDU bytes.
    let declared = header.length as usize;
    if declared == 0 {
        return HandleOutcome::CloseConnection;
    }
    let pdu_len = declared - 1;
    let available = buf.len().saturating_sub(7);
    if pdu_len > available {
        // The peer declared more bytes than it sent — truncated frame.
        return HandleOutcome::CloseConnection;
    }

    let pdu = &buf[7..7 + pdu_len];
    if pdu.is_empty() {
        return exception(header, ExceptionCode::IllegalFunction);
    }

    let fc = pdu[0];
    // Echo the function code for normal responses; set high bit for exception.
    match fc {
        0x03 => handle_read_holding(header, pdu, store),
        0x06 => handle_write_single(header, pdu, store, write_policy),
        0x10 => handle_write_multiple(header, pdu, store, write_policy),
        _ => {
            // Unsupported function code (including 0x05/0x0F writes which we
            // do not serve even when authorized — only 0x06/0x10 are wired to
            // the control gateway).
            exception(header, ExceptionCode::IllegalFunction)
        }
    }
}

fn handle_read_holding(header: MbapHeader, pdu: &[u8], store: &RegisterStore) -> HandleOutcome {
    // PDU: fc(1) + start_addr(2) + quantity(2) = 5 bytes.
    if pdu.len() != 5 {
        return exception(header, ExceptionCode::IllegalDataValue);
    }
    let start = u16::from_be_bytes([pdu[1], pdu[2]]);
    let quantity = u16::from_be_bytes([pdu[3], pdu[4]]);
    // Modbus spec: quantity must be 1..=125 for read holding registers.
    if quantity == 0 || quantity > 125 {
        return exception(header, ExceptionCode::IllegalDataValue);
    }
    match store.read(start, quantity) {
        Some(values) => {
            let mut resp_pdu = Vec::with_capacity(2 + values.len() * 2);
            resp_pdu.push(0x03); // echo fc
            resp_pdu.push((values.len() * 2) as u8); // byte count
            for v in values {
                resp_pdu.extend_from_slice(&v.to_be_bytes());
            }
            HandleOutcome::Response(Response {
                transaction_id: header.transaction_id,
                unit_id: header.unit_id,
                pdu: resp_pdu,
            })
        }
        None => exception(header, ExceptionCode::IllegalDataAddress),
    }
}

fn handle_write_single(
    header: MbapHeader,
    pdu: &[u8],
    store: &mut RegisterStore,
    write_policy: WritePolicy<'_>,
) -> HandleOutcome {
    // PDU: fc(1) + addr(2) + value(2) = 5 bytes.
    if pdu.len() != 5 {
        return exception(header, ExceptionCode::IllegalDataValue);
    }
    let addr = u16::from_be_bytes([pdu[1], pdu[2]]);
    let value = u16::from_be_bytes([pdu[3], pdu[4]]);

    // DEFAULT DENY: writes only proceed under an exact-matching AuthorizedCommand.
    let allowed = match write_policy {
        WritePolicy::DenyAll => false,
        WritePolicy::Authorized(auth) => {
            auth.device_id == header.unit_id as u16
                && auth.point_id as u16 == addr
                && single_command_matches_on(&auth.command, value)
        }
    };
    if !allowed {
        return exception(header, ExceptionCode::IllegalFunction);
    }
    if !store.write_single(addr, value) {
        return exception(header, ExceptionCode::IllegalDataAddress);
    }
    // Normal response echoes the request PDU.
    HandleOutcome::Response(Response {
        transaction_id: header.transaction_id,
        unit_id: header.unit_id,
        pdu: pdu.to_vec(),
    })
}

fn handle_write_multiple(
    header: MbapHeader,
    pdu: &[u8],
    store: &mut RegisterStore,
    write_policy: WritePolicy<'_>,
) -> HandleOutcome {
    // PDU: fc(1) + start(2) + quantity(2) + byte_count(1) + values(quantity*2).
    if pdu.len() < 6 {
        return exception(header, ExceptionCode::IllegalDataValue);
    }
    let start = u16::from_be_bytes([pdu[1], pdu[2]]);
    let quantity = u16::from_be_bytes([pdu[3], pdu[4]]);
    let byte_count = pdu[5] as usize;
    if quantity == 0 || quantity > 123 {
        return exception(header, ExceptionCode::IllegalDataValue);
    }
    if byte_count != quantity as usize * 2 {
        return exception(header, ExceptionCode::IllegalDataValue);
    }
    if pdu.len() != 6 + byte_count {
        return exception(header, ExceptionCode::IllegalDataValue);
    }
    let mut values = Vec::with_capacity(quantity as usize);
    for i in 0..quantity as usize {
        let off = 6 + i * 2;
        values.push(u16::from_be_bytes([pdu[off], pdu[off + 1]]));
    }

    // DEFAULT DENY for multi-register writes: the control gateway authorizes
    // one point at a time, so a multi-point write is denied unless every
    // targeted register matches the single authorized point's address (i.e.
    // quantity == 1). Multi-point writes are rejected outright.
    let allowed = match write_policy {
        WritePolicy::DenyAll => false,
        WritePolicy::Authorized(auth) => {
            quantity == 1
                && auth.device_id == header.unit_id as u16
                && auth.point_id as u16 == start
                && single_command_matches_on(&auth.command, values[0])
        }
    };
    if !allowed {
        return exception(header, ExceptionCode::IllegalFunction);
    }
    if !store.write_multiple(start, &values) {
        return exception(header, ExceptionCode::IllegalDataAddress);
    }
    // Normal response: fc + start + quantity.
    let mut resp_pdu = Vec::with_capacity(5);
    resp_pdu.push(0x10);
    resp_pdu.extend_from_slice(&start.to_be_bytes());
    resp_pdu.extend_from_slice(&quantity.to_be_bytes());
    HandleOutcome::Response(Response {
        transaction_id: header.transaction_id,
        unit_id: header.unit_id,
        pdu: resp_pdu,
    })
}

/// Maps a SingleCommand::On to value 0xFF00 and Off to 0x0000 (Modbus coil
/// convention, also used for register writes that represent a single bit).
fn single_command_matches_on(
    cmd: &eneros_telemetry_model::command::ControlCommand,
    value: u16,
) -> bool {
    use eneros_telemetry_model::command::{ControlCommand, SingleCommand};
    match cmd {
        ControlCommand::Single(SingleCommand::On) => value == 0xFF00,
        ControlCommand::Single(SingleCommand::Off) => value == 0x0000,
        _ => false,
    }
}

fn exception(header: MbapHeader, code: ExceptionCode) -> HandleOutcome {
    HandleOutcome::Exception(ExceptionResponse {
        transaction_id: header.transaction_id,
        unit_id: header.unit_id,
        code,
    })
}

/// Encodes a normal [`Response`] into a full MBAP frame (7-byte header + PDU).
pub fn encode_response(resp: &Response) -> Vec<u8> {
    let header = MbapHeader::new(resp.transaction_id, resp.unit_id, resp.pdu.len() as u16);
    let mut out = Vec::with_capacity(7 + resp.pdu.len());
    out.extend_from_slice(&header.encode());
    out.extend_from_slice(&resp.pdu);
    out
}

/// Encodes an exception [`ExceptionResponse`] into a full MBAP frame. The PDU
/// is fc|0x80 followed by the exception code; the fc is taken from `request_fc`.
pub fn encode_exception(resp: &ExceptionResponse, request_fc: u8) -> Vec<u8> {
    let pdu = vec![request_fc | 0x80, resp.code as u8];
    let header = MbapHeader::new(resp.transaction_id, resp.unit_id, pdu.len() as u16);
    let mut out = Vec::with_capacity(7 + pdu.len());
    out.extend_from_slice(&header.encode());
    out.extend_from_slice(&pdu);
    out
}
