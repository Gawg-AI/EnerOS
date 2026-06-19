//! OPC UA binary protocol codec.
//!
//! Implements the binary encoding/decoding for the core OPC UA service layer:
//! - Binary writer/reader for primitive types, NodeId, Variant, DataValue
//! - Service message framing (MSG/OPN/CLO)
//! - Request/response encoding for OpenSecureChannel, CreateSession,
//!   ActivateSession, Read, Write, Browse, CreateSubscription
//!
//! Reference: OPC UA Part 6 (Mappings), Table 1–13.

use crate::adapters::opcua::{NodeIdType, OpcUaNodeId, OpcUaVariant};

// ── OPC UA built-in type IDs (Part 6, Table 1) ─────────────────────────────

pub const TYPE_NULL: u8 = 0;
pub const TYPE_BOOLEAN: u8 = 1;
pub const TYPE_SBYTE: u8 = 2;
pub const TYPE_BYTE: u8 = 3;
pub const TYPE_INT16: u8 = 4;
pub const TYPE_UINT16: u8 = 5;
pub const TYPE_INT32: u8 = 6;
pub const TYPE_UINT32: u8 = 7;
pub const TYPE_INT64: u8 = 8;
pub const TYPE_UINT64: u8 = 9;
pub const TYPE_FLOAT: u8 = 10;
pub const TYPE_DOUBLE: u8 = 11;
pub const TYPE_STRING: u8 = 12;
pub const TYPE_DATETIME: u8 = 13;
pub const TYPE_GUID: u8 = 14;
pub const TYPE_BYTESTRING: u8 = 15;

// ── Service request/response IDs (Part 4, numeric identifiers) ─────────────

pub const ID_OPEN_SECURE_CHANNEL_REQ: u32 = 446;
pub const ID_OPEN_SECURE_CHANNEL_RES: u32 = 447;
pub const ID_CREATE_SESSION_REQ: u32 = 461;
pub const ID_CREATE_SESSION_RES: u32 = 462;
pub const ID_ACTIVATE_SESSION_REQ: u32 = 467;
pub const ID_ACTIVATE_SESSION_RES: u32 = 468;
pub const ID_READ_REQ: u32 = 631;
pub const ID_READ_RES: u32 = 632;
pub const ID_WRITE_REQ: u32 = 673;
pub const ID_WRITE_RES: u32 = 674;
pub const ID_BROWSE_REQ: u32 = 527;
pub const ID_BROWSE_RES: u32 = 528;
pub const ID_CREATE_SUBSCRIPTION_REQ: u32 = 787;
pub const ID_CREATE_SUBSCRIPTION_RES: u32 = 788;

// ── BinaryWriter ───────────────────────────────────────────────────────────

/// A little-endian binary encoder for OPC UA types.
pub struct BinaryWriter {
    buf: Vec<u8>,
}

impl BinaryWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn write_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn write_i8(&mut self, v: i8) {
        self.buf.push(v as u8);
    }

    pub fn write_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_i16(&mut self, v: i16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_f64(&mut self, v: f64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn write_bool(&mut self, v: bool) {
        self.buf.push(if v { 1 } else { 0 });
    }

    /// Write a UA String: i32 length (in bytes) + UTF-8 data. -1 = null.
    pub fn write_string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.write_i32(bytes.len() as i32);
        self.buf.extend_from_slice(bytes);
    }

    pub fn write_string_opt(&mut self, s: Option<&str>) {
        match s {
            Some(v) => self.write_string(v),
            None => self.write_i32(-1),
        }
    }

    /// Write a UA ByteString: i32 length + raw bytes. -1 = null.
    pub fn write_bytestring(&mut self, data: &[u8]) {
        self.write_i32(data.len() as i32);
        self.buf.extend_from_slice(data);
    }

    pub fn write_bytestring_opt(&mut self, data: Option<&[u8]>) {
        match data {
            Some(v) => self.write_bytestring(v),
            None => self.write_i32(-1),
        }
    }

    /// Write a UA DateTime: i64 (100ns intervals since Jan 1, 1601).
    pub fn write_datetime_now(&mut self) {
        // Convert Unix epoch (1970) to Windows epoch (1601):
        // 11644473600 = seconds between 1601-01-01 and 1970-01-01
        let now = chrono::Utc::now();
        let unix_secs = now.timestamp();
        let windows_secs = unix_secs + 11644473600;
        let ticks = windows_secs * 10_000_000 + (now.timestamp_subsec_nanos() as i64) / 100;
        self.write_i64(ticks);
    }

    /// Write a NodeId using the most compact encoding.
    pub fn write_node_id(&mut self, node_id: &OpcUaNodeId) {
        match &node_id.identifier {
            NodeIdType::Numeric(n) if node_id.namespace == 0 && *n <= 255 => {
                // Two-byte encoding: 0x00 + 1-byte identifier
                self.write_u8(0x00);
                self.write_u8(*n as u8);
            }
            NodeIdType::Numeric(n) if node_id.namespace <= 255 && *n <= 65535 => {
                // Four-byte encoding: 0x01 + 1-byte namespace + 2-byte identifier
                self.write_u8(0x01);
                self.write_u8(node_id.namespace as u8);
                self.write_u16(*n as u16);
            }
            NodeIdType::Numeric(n) => {
                // Numeric encoding: 0x02 + 2-byte namespace + 4-byte identifier
                self.write_u8(0x02);
                self.write_u16(node_id.namespace);
                self.write_u32(*n);
            }
            NodeIdType::String(s) => {
                // String encoding: 0x03 + 2-byte namespace + string
                self.write_u8(0x03);
                self.write_u16(node_id.namespace);
                self.write_string(s);
            }
            NodeIdType::Guid(g) => {
                // Guid encoding: 0x04 + 2-byte namespace + 16-byte GUID
                self.write_u8(0x04);
                self.write_u16(node_id.namespace);
                // Parse GUID hex string to 16 bytes
                let hex: String = g.chars().filter(|c| c.is_ascii_hexdigit()).collect();
                if hex.len() == 32 {
                    for i in 0..16 {
                        let byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                            .unwrap_or(0);
                        self.write_u8(byte);
                    }
                } else {
                    for _ in 0..16 {
                        self.write_u8(0);
                    }
                }
            }
            NodeIdType::ByteString(b) => {
                // ByteString encoding: 0x05 + 2-byte namespace + bytestring
                self.write_u8(0x05);
                self.write_u16(node_id.namespace);
                // Parse hex string to bytes
                let hex: String = b.chars().filter(|c| c.is_ascii_hexdigit()).collect();
                let bytes: Vec<u8> = (0..hex.len() / 2)
                    .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
                    .collect();
                self.write_bytestring(&bytes);
            }
        }
    }

    /// Write an ExpandedNodeId (NodeId + optional namespace URI + optional server index).
    pub fn write_expanded_node_id(&mut self, node_id: &OpcUaNodeId) {
        // The encoding byte's top 2 bits indicate presence of namespace URI
        // and server index. For simplicity, we write neither (both absent).
        self.write_node_id(node_id);
    }

    /// Write a Variant.
    pub fn write_variant(&mut self, variant: &OpcUaVariant) {
        match variant {
            OpcUaVariant::Null => {
                self.write_u8(TYPE_NULL);
            }
            OpcUaVariant::Boolean(v) => {
                self.write_u8(TYPE_BOOLEAN);
                self.write_bool(*v);
            }
            OpcUaVariant::SByte(v) => {
                self.write_u8(TYPE_SBYTE);
                self.write_i8(*v);
            }
            OpcUaVariant::Byte(v) => {
                self.write_u8(TYPE_BYTE);
                self.write_u8(*v);
            }
            OpcUaVariant::Int16(v) => {
                self.write_u8(TYPE_INT16);
                self.write_i16(*v);
            }
            OpcUaVariant::UInt16(v) => {
                self.write_u8(TYPE_UINT16);
                self.write_u16(*v);
            }
            OpcUaVariant::Int32(v) => {
                self.write_u8(TYPE_INT32);
                self.write_i32(*v);
            }
            OpcUaVariant::UInt32(v) => {
                self.write_u8(TYPE_UINT32);
                self.write_u32(*v);
            }
            OpcUaVariant::Int64(v) => {
                self.write_u8(TYPE_INT64);
                self.write_i64(*v);
            }
            OpcUaVariant::UInt64(v) => {
                self.write_u8(TYPE_UINT64);
                self.write_u64(*v);
            }
            OpcUaVariant::Float(v) => {
                self.write_u8(TYPE_FLOAT);
                self.write_f32(*v);
            }
            OpcUaVariant::Double(v) => {
                self.write_u8(TYPE_DOUBLE);
                self.write_f64(*v);
            }
            OpcUaVariant::String(v) => {
                self.write_u8(TYPE_STRING);
                self.write_string(v);
            }
            OpcUaVariant::ByteString(v) => {
                self.write_u8(TYPE_BYTESTRING);
                self.write_bytestring(v);
            }
            OpcUaVariant::DateTime(v) => {
                self.write_u8(TYPE_DATETIME);
                self.write_i64(*v);
            }
            OpcUaVariant::Array(arr) => {
                // Array encoding: type | 0x80, i32 length, elements
                if arr.is_empty() {
                    self.write_u8(TYPE_NULL | 0x80);
                    self.write_i32(0);
                } else {
                    let elem_type = variant_type_id(&arr[0]);
                    self.write_u8(elem_type | 0x80);
                    self.write_i32(arr.len() as i32);
                    for elem in arr {
                        self.write_variant_value(elem);
                    }
                }
            }
        }
    }

    /// Write only the value portion of a Variant (no type byte).
    fn write_variant_value(&mut self, variant: &OpcUaVariant) {
        match variant {
            OpcUaVariant::Null => {}
            OpcUaVariant::Boolean(v) => self.write_bool(*v),
            OpcUaVariant::SByte(v) => self.write_i8(*v),
            OpcUaVariant::Byte(v) => self.write_u8(*v),
            OpcUaVariant::Int16(v) => self.write_i16(*v),
            OpcUaVariant::UInt16(v) => self.write_u16(*v),
            OpcUaVariant::Int32(v) => self.write_i32(*v),
            OpcUaVariant::UInt32(v) => self.write_u32(*v),
            OpcUaVariant::Int64(v) => self.write_i64(*v),
            OpcUaVariant::UInt64(v) => self.write_u64(*v),
            OpcUaVariant::Float(v) => self.write_f32(*v),
            OpcUaVariant::Double(v) => self.write_f64(*v),
            OpcUaVariant::String(v) => self.write_string(v),
            OpcUaVariant::ByteString(v) => self.write_bytestring(v),
            OpcUaVariant::DateTime(v) => self.write_i64(*v),
            OpcUaVariant::Array(_) => {
                // Nested arrays not supported in value-only encoding
            }
        }
    }

    /// Write a RequestHeader (Part 4, Table 528).
    pub fn write_request_header(&mut self, auth_token: &[u8], request_handle: u32) {
        // AuthenticationToken (NodeId)
        if auth_token.is_empty() {
            // Null node ID
            self.write_u8(0x00);
            self.write_u8(0);
        } else {
            // If auth_token is 4 bytes, encode as numeric node ID
            if auth_token.len() == 4 {
                self.write_u8(0x02); // Numeric
                self.write_u16(0); // namespace 0
                let val = u32::from_le_bytes([
                    auth_token[0],
                    auth_token[1],
                    auth_token[2],
                    auth_token[3],
                ]);
                self.write_u32(val);
            } else {
                // Fallback: null node ID
                self.write_u8(0x00);
                self.write_u8(0);
            }
        }
        // Timestamp
        self.write_datetime_now();
        // RequestHandle
        self.write_u32(request_handle);
        // ReturnDiagnostics
        self.write_u32(0);
        // AuditEntryId (null string)
        self.write_i32(-1);
        // TimeoutHint
        self.write_u32(10000);
        // AdditionalHeader (ExtensionObject: null)
        self.write_u8(0x00); // Null NodeId
        self.write_u8(0);
        self.write_u8(0); // No encoding
    }
}

impl Default for BinaryWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the OPC UA built-in type ID for a variant.
fn variant_type_id(variant: &OpcUaVariant) -> u8 {
    match variant {
        OpcUaVariant::Null => TYPE_NULL,
        OpcUaVariant::Boolean(_) => TYPE_BOOLEAN,
        OpcUaVariant::SByte(_) => TYPE_SBYTE,
        OpcUaVariant::Byte(_) => TYPE_BYTE,
        OpcUaVariant::Int16(_) => TYPE_INT16,
        OpcUaVariant::UInt16(_) => TYPE_UINT16,
        OpcUaVariant::Int32(_) => TYPE_INT32,
        OpcUaVariant::UInt32(_) => TYPE_UINT32,
        OpcUaVariant::Int64(_) => TYPE_INT64,
        OpcUaVariant::UInt64(_) => TYPE_UINT64,
        OpcUaVariant::Float(_) => TYPE_FLOAT,
        OpcUaVariant::Double(_) => TYPE_DOUBLE,
        OpcUaVariant::String(_) => TYPE_STRING,
        OpcUaVariant::ByteString(_) => TYPE_BYTESTRING,
        OpcUaVariant::DateTime(_) => TYPE_DATETIME,
        OpcUaVariant::Array(_) => TYPE_NULL,
    }
}

// ── BinaryReader ───────────────────────────────────────────────────────────

/// A little-endian binary decoder for OPC UA types.
pub struct BinaryReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BinaryReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn skip(&mut self, n: usize) -> Result<(), String> {
        if self.pos + n > self.data.len() {
            return Err(format!(
                "skip({}): EOF at pos {} (len={})",
                n,
                self.pos,
                self.data.len()
            ));
        }
        self.pos += n;
        Ok(())
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.data.len() {
            return Err(format!(
                "read({}): EOF at pos {} (len={})",
                n,
                self.pos,
                self.data.len()
            ));
        }
        let result = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(result)
    }

    pub fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.read_bytes(1)?[0])
    }

    pub fn read_i8(&mut self) -> Result<i8, String> {
        Ok(self.read_u8()? as i8)
    }

    pub fn read_u16(&mut self) -> Result<u16, String> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn read_i16(&mut self) -> Result<i16, String> {
        Ok(self.read_u16()? as i16)
    }

    pub fn read_u32(&mut self) -> Result<u32, String> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_i32(&mut self) -> Result<i32, String> {
        Ok(self.read_u32()? as i32)
    }

    pub fn read_u64(&mut self) -> Result<u64, String> {
        let b = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_i64(&mut self) -> Result<i64, String> {
        Ok(self.read_u64()? as i64)
    }

    pub fn read_f32(&mut self) -> Result<f32, String> {
        let b = self.read_bytes(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_f64(&mut self) -> Result<f64, String> {
        let b = self.read_bytes(8)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_bool(&mut self) -> Result<bool, String> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_string(&mut self) -> Result<String, String> {
        let len = self.read_i32()?;
        if len < 0 {
            return Ok(String::new());
        }
        let len = len as usize;
        let bytes = self.read_bytes(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|e| format!("invalid UTF-8: {}", e))
    }

    pub fn read_bytestring(&mut self) -> Result<Vec<u8>, String> {
        let len = self.read_i32()?;
        if len < 0 {
            return Ok(Vec::new());
        }
        let len = len as usize;
        Ok(self.read_bytes(len)?.to_vec())
    }

    /// Read a NodeId.
    pub fn read_node_id(&mut self) -> Result<OpcUaNodeId, String> {
        let encoding = self.read_u8()?;
        match encoding {
            0x00 => {
                // Two-byte: namespace=0, 1-byte identifier
                let id = self.read_u8()? as u32;
                Ok(OpcUaNodeId {
                    namespace: 0,
                    identifier: NodeIdType::Numeric(id),
                })
            }
            0x01 => {
                // Four-byte: 1-byte namespace, 2-byte identifier
                let ns = self.read_u8()? as u16;
                let id = self.read_u16()? as u32;
                Ok(OpcUaNodeId {
                    namespace: ns,
                    identifier: NodeIdType::Numeric(id),
                })
            }
            0x02 => {
                // Numeric: 2-byte namespace, 4-byte identifier
                let ns = self.read_u16()?;
                let id = self.read_u32()?;
                Ok(OpcUaNodeId {
                    namespace: ns,
                    identifier: NodeIdType::Numeric(id),
                })
            }
            0x03 => {
                // String: 2-byte namespace, string
                let ns = self.read_u16()?;
                let s = self.read_string()?;
                Ok(OpcUaNodeId {
                    namespace: ns,
                    identifier: NodeIdType::String(s),
                })
            }
            0x04 => {
                // Guid: 2-byte namespace, 16-byte GUID
                let ns = self.read_u16()?;
                let _guid = self.read_bytes(16)?;
                Ok(OpcUaNodeId {
                    namespace: ns,
                    identifier: NodeIdType::Guid(String::new()),
                })
            }
            0x05 => {
                // ByteString: 2-byte namespace, bytestring
                let ns = self.read_u16()?;
                let bs = self.read_bytestring()?;
                let hex: String = bs.iter().map(|b| format!("{:02x}", b)).collect();
                Ok(OpcUaNodeId {
                    namespace: ns,
                    identifier: NodeIdType::ByteString(hex),
                })
            }
            other => Err(format!("unknown NodeId encoding: 0x{:02x}", other)),
        }
    }

    /// Read a Variant.
    pub fn read_variant(&mut self) -> Result<OpcUaVariant, String> {
        let encoding = self.read_u8()?;
        let type_id = encoding & 0x3f;
        let is_array = (encoding & 0x80) != 0;

        if is_array {
            let len = self.read_i32()?;
            if len < 0 {
                return Ok(OpcUaVariant::Null);
            }
            let len = len as usize;
            let mut arr = Vec::with_capacity(len);
            for _ in 0..len {
                arr.push(self.read_variant_value(type_id)?);
            }
            Ok(OpcUaVariant::Array(arr))
        } else {
            self.read_variant_value(type_id)
        }
    }

    fn read_variant_value(&mut self, type_id: u8) -> Result<OpcUaVariant, String> {
        match type_id {
            TYPE_NULL => Ok(OpcUaVariant::Null),
            TYPE_BOOLEAN => Ok(OpcUaVariant::Boolean(self.read_bool()?)),
            TYPE_SBYTE => Ok(OpcUaVariant::SByte(self.read_i8()?)),
            TYPE_BYTE => Ok(OpcUaVariant::Byte(self.read_u8()?)),
            TYPE_INT16 => Ok(OpcUaVariant::Int16(self.read_i16()?)),
            TYPE_UINT16 => Ok(OpcUaVariant::UInt16(self.read_u16()?)),
            TYPE_INT32 => Ok(OpcUaVariant::Int32(self.read_i32()?)),
            TYPE_UINT32 => Ok(OpcUaVariant::UInt32(self.read_u32()?)),
            TYPE_INT64 => Ok(OpcUaVariant::Int64(self.read_i64()?)),
            TYPE_UINT64 => Ok(OpcUaVariant::UInt64(self.read_u64()?)),
            TYPE_FLOAT => Ok(OpcUaVariant::Float(self.read_f32()?)),
            TYPE_DOUBLE => Ok(OpcUaVariant::Double(self.read_f64()?)),
            TYPE_STRING => Ok(OpcUaVariant::String(self.read_string()?)),
            TYPE_BYTESTRING => Ok(OpcUaVariant::ByteString(self.read_bytestring()?)),
            TYPE_DATETIME => Ok(OpcUaVariant::DateTime(self.read_i64()?)),
            other => Err(format!("unknown variant type: {}", other)),
        }
    }

    /// Read a ResponseHeader (Part 4, Table 529).
    /// Returns (timestamp, request_handle, status_code).
    pub fn read_response_header(&mut self) -> Result<(i64, u32, u32), String> {
        let timestamp = self.read_i64()?; // DateTime
        let request_handle = self.read_u32()?;
        let status_code = self.read_u32()?;
        // ServiceDiagnostics (DiagnosticInfo): read encoding byte
        let diag_encoding = self.read_u8()?;
        // Minimal DiagnosticInfo parsing
        if diag_encoding != 0 {
            // Skip diagnostic info based on encoding bits
            if (diag_encoding & 0x01) != 0 {
                let _ = self.read_string()?; // SymbolicId
            }
            if (diag_encoding & 0x02) != 0 {
                let _ = self.read_string()?; // NamespaceURI
            }
            if (diag_encoding & 0x04) != 0 {
                let _ = self.read_string()?; // Locale
            }
            if (diag_encoding & 0x08) != 0 {
                let _ = self.read_string()?; // LocalizedText
            }
            if (diag_encoding & 0x10) != 0 {
                let _ = self.read_i32()?; // AdditionalInfo
            }
            if (diag_encoding & 0x20) != 0 {
                let _ = self.read_u32()?; // InnerStatusCode
            }
            if (diag_encoding & 0x40) != 0 {
                // InnerDiagnosticInfo (recursive) — skip minimally
                let _ = self.read_u8()?;
            }
        }
        // StringTable (null array)
        let _string_table_len = self.read_i32()?;
        // AdditionalHeader (ExtensionObject)
        let _ = self.read_node_id()?;
        let _encoding = self.read_u8()?;
        if _encoding != 0 {
            let _body_len = self.read_i32()?;
            // Skip body
        }
        Ok((timestamp, request_handle, status_code))
    }
}

// ── Message framing ────────────────────────────────────────────────────────

/// Build a MSG-F (final message) frame with the given service body.
///
/// Frame layout:
///   "MSG" + 'F' + size(u32) + channel_id(u32) + token_id(u32)
///   + sequence_number(u32) + request_id(u32) + body
pub fn build_msg_frame(
    channel_id: u32,
    token_id: u32,
    sequence_number: u32,
    request_id: u32,
    body: &[u8],
) -> Vec<u8> {
    let total_size = 8 + 4 + 4 + 4 + 4 + body.len(); // header(8) + 4 u32s + body
    let mut frame = Vec::with_capacity(total_size);
    frame.extend_from_slice(b"MSG");
    frame.push(b'F');
    frame.extend_from_slice(&(total_size as u32).to_le_bytes());
    frame.extend_from_slice(&channel_id.to_le_bytes());
    frame.extend_from_slice(&token_id.to_le_bytes());
    frame.extend_from_slice(&sequence_number.to_le_bytes());
    frame.extend_from_slice(&request_id.to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

/// Build an OPN-F (open secure channel, final) frame.
///
/// Frame layout:
///   "OPN" + 'F' + size(u32) + channel_id(u32)
///   + security_policy_uri(string) + sender_certificate(bytestring)
///   + receiver_cert_thumbprint(bytestring)
///   + sequence_number(u32) + request_id(u32) + body
pub fn build_opn_frame(
    channel_id: u32,
    sequence_number: u32,
    request_id: u32,
    body: &[u8],
) -> Vec<u8> {
    let security_policy_uri = "http://opcfoundation.org/UA/SecurityPolicy#None";
    let uri_bytes = security_policy_uri.as_bytes();
    let security_header_size = 4 + uri_bytes.len() + 4 + 4; // uri len + uri + cert len(-1) + thumbprint len(-1)

    let total_size = 8 + 4 + security_header_size + 4 + 4 + body.len();
    let mut frame = Vec::with_capacity(total_size);
    frame.extend_from_slice(b"OPN");
    frame.push(b'F');
    frame.extend_from_slice(&(total_size as u32).to_le_bytes());
    frame.extend_from_slice(&channel_id.to_le_bytes());
    // Security header (None policy)
    frame.extend_from_slice(&(uri_bytes.len() as i32).to_le_bytes());
    frame.extend_from_slice(uri_bytes);
    frame.extend_from_slice(&(-1i32).to_le_bytes()); // Sender certificate: null
    frame.extend_from_slice(&(-1i32).to_le_bytes()); // Receiver certificate thumbprint: null
    // Sequence number + request ID
    frame.extend_from_slice(&sequence_number.to_le_bytes());
    frame.extend_from_slice(&request_id.to_le_bytes());
    // Body
    frame.extend_from_slice(body);
    frame
}

/// Parse a response message header.
/// Returns (message_type, chunk_type, size, body_offset).
pub fn parse_message_header(data: &[u8]) -> Result<([u8; 3], u8, u32, usize), String> {
    if data.len() < 8 {
        return Err("message too short for header".into());
    }
    let msg_type = [data[0], data[1], data[2]];
    let chunk_type = data[3];
    let size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    Ok((msg_type, chunk_type, size, 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_read_u32() {
        let mut w = BinaryWriter::new();
        w.write_u32(0x12345678);
        let bytes = w.into_bytes();
        let mut r = BinaryReader::new(&bytes);
        assert_eq!(r.read_u32().unwrap(), 0x12345678);
    }

    #[test]
    fn test_write_read_string() {
        let mut w = BinaryWriter::new();
        w.write_string("Hello, OPC UA!");
        let bytes = w.into_bytes();
        let mut r = BinaryReader::new(&bytes);
        assert_eq!(r.read_string().unwrap(), "Hello, OPC UA!");
    }

    #[test]
    fn test_write_read_node_id_numeric() {
        let node_id = OpcUaNodeId {
            namespace: 0,
            identifier: NodeIdType::Numeric(85),
        };
        let mut w = BinaryWriter::new();
        w.write_node_id(&node_id);
        let bytes = w.into_bytes();
        let mut r = BinaryReader::new(&bytes);
        let read_back = r.read_node_id().unwrap();
        assert_eq!(read_back, node_id);
    }

    #[test]
    fn test_write_read_variant_double() {
        let mut w = BinaryWriter::new();
        w.write_variant(&OpcUaVariant::Double(42.5));
        let bytes = w.into_bytes();
        let mut r = BinaryReader::new(&bytes);
        let v = r.read_variant().unwrap();
        assert_eq!(v, OpcUaVariant::Double(42.5));
    }

    #[test]
    fn test_build_msg_frame() {
        let body = vec![1u8, 2, 3, 4];
        let frame = build_msg_frame(1, 2, 3, 4, &body);
        assert_eq!(&frame[0..3], b"MSG");
        assert_eq!(frame[3], b'F');
        let size = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
        assert_eq!(size as usize, frame.len());
    }
}
