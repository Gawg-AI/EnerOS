//! MMS (Manufacturing Message Specification) protocol implementation for IEC 61850.
//!
//! This module implements the minimum MMS subset required for IEC 61850:
//! - BER (Basic Encoding Rules) for ASN.1 encoding/decoding
//! - ACSE (Association Control Service Element) for association management
//! - MMS Initiate/Read/Write/InformationReport
//!
//! References: ISO 9506 (MMS), ISO 8650 (ACSE), ISO 8823 (Presentation),
//!             ISO 8327 (Session), IEC 61850-8-1

use std::io;
use tracing::debug;

use super::cotp::CotpTransport;

// ============================================================================
// BER (Basic Encoding Rules) encoding/decoding
// ============================================================================

/// BER tag classes
#[allow(dead_code)]
const BER_UNIVERSAL: u8 = 0x00;
#[allow(dead_code)]
const BER_APPLICATION: u8 = 0x40;
const BER_CONTEXT: u8 = 0x80;
#[allow(dead_code)]
const BER_PRIVATE: u8 = 0xC0;
const BER_CONSTRUCTED: u8 = 0x20;

/// Common BER tags
const TAG_BOOLEAN: u8 = 0x01;
const TAG_INTEGER: u8 = 0x02;
const TAG_BIT_STRING: u8 = 0x03;
const TAG_OCTET_STRING: u8 = 0x04;
const TAG_NULL: u8 = 0x05;
const TAG_OID: u8 = 0x06;
const TAG_SEQUENCE: u8 = 0x30;
#[allow(dead_code)]
const TAG_SET: u8 = 0x31;

/// ACSE tags
const TAG_AARQ: u8 = 0x60;  // A-ASSOCIATE request (APPLICATION 0 CONSTRUCTED)
const TAG_AARE: u8 = 0x61;  // A-ASSOCIATE response (APPLICATION 1 CONSTRUCTED)
#[allow(dead_code)]
const TAG_RLRQ: u8 = 0x62;  // A-RELEASE request
#[allow(dead_code)]
const TAG_RLRE: u8 = 0x63;  // A-RELEASE response
#[allow(dead_code)]
const TAG_ABRT: u8 = 0x64;  // A-ABORT

/// BER encoder helper
pub struct BerEncoder {
    #[allow(dead_code)]
    buf: Vec<u8>,
}

impl BerEncoder {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Encode a tag + length + value
    pub fn encode_tl(tag: u8, value: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(2 + value.len());
        result.push(tag);
        Self::encode_length(&mut result, value.len());
        result.extend_from_slice(value);
        result
    }

    /// Encode length in BER format
    fn encode_length(buf: &mut Vec<u8>, len: usize) {
        if len < 128 {
            buf.push(len as u8);
        } else if len < 256 {
            buf.push(0x81);
            buf.push(len as u8);
        } else {
            buf.push(0x82);
            buf.push((len >> 8) as u8);
            buf.push((len & 0xFF) as u8);
        }
    }

    /// Encode an INTEGER value
    pub fn encode_integer(val: i32) -> Vec<u8> {
        if (-128..=127).contains(&val) {
            vec![TAG_INTEGER, 0x01, val as u8]
        } else if (-32768..=32767).contains(&val) {
            vec![TAG_INTEGER, 0x02, (val >> 8) as u8, (val & 0xFF) as u8]
        } else {
            vec![TAG_INTEGER, 0x04,
                 (val >> 24) as u8, ((val >> 16) & 0xFF) as u8,
                 ((val >> 8) & 0xFF) as u8, (val & 0xFF) as u8]
        }
    }

    /// Encode a BOOLEAN value
    pub fn encode_boolean(val: bool) -> Vec<u8> {
        vec![TAG_BOOLEAN, 0x01, if val { 0xFF } else { 0x00 }]
    }

    /// Encode an OCTET STRING
    pub fn encode_octet_string(data: &[u8]) -> Vec<u8> {
        Self::encode_tl(TAG_OCTET_STRING, data)
    }

    /// Encode a visible string (UTF8String tag 0x0C)
    pub fn encode_visible_string(s: &str) -> Vec<u8> {
        Self::encode_tl(0x0C, s.as_bytes())
    }

    /// Encode a context-specific tag [n] IMPLICIT
    pub fn encode_context(n: u8, value: &[u8]) -> Vec<u8> {
        let tag = BER_CONTEXT | n;
        Self::encode_tl(tag, value)
    }

    /// Encode a context-specific constructed tag [n] CONSTRUCTED
    pub fn encode_context_constructed(n: u8, value: &[u8]) -> Vec<u8> {
        let tag = BER_CONTEXT | BER_CONSTRUCTED | n;
        Self::encode_tl(tag, value)
    }

    /// Encode a SEQUENCE
    pub fn encode_sequence(content: &[u8]) -> Vec<u8> {
        Self::encode_tl(TAG_SEQUENCE, content)
    }

    /// Encode NULL
    pub fn encode_null() -> Vec<u8> {
        vec![TAG_NULL, 0x00]
    }

    /// Encode an OBJECT IDENTIFIER
    /// Components are u32 to support large OID arc values (e.g., 9506 for ISO 9506)
    pub fn encode_oid(components: &[u32]) -> Vec<u8> {
        if components.is_empty() {
            return vec![TAG_OID, 0x00];
        }
        // First two components encoded as 40*first + second
        let mut encoded = Vec::new();
        let first = components[0];
        let second = components.get(1).copied().unwrap_or(0);
        // Encode 40*first + second using base-128
        Self::encode_oid_arc(&mut encoded, 40 * first + second);
        for &c in components.iter().skip(2) {
            Self::encode_oid_arc(&mut encoded, c);
        }
        Self::encode_tl(TAG_OID, &encoded)
    }

    /// Encode a single OID arc value using base-128 encoding (high bit continuation)
    fn encode_oid_arc(buf: &mut Vec<u8>, val: u32) {
        if val == 0 {
            buf.push(0);
            return;
        }
        // Collect 7-bit groups from most significant to least significant
        let mut parts = Vec::new();
        let mut v = val;
        while v > 0 {
            parts.push((v & 0x7F) as u8);
            v >>= 7;
        }
        parts.reverse();
        for i in 0..parts.len() {
            if i < parts.len() - 1 {
                buf.push(parts[i] | 0x80); // Set high bit for continuation
            } else {
                buf.push(parts[i]); // Last byte has high bit clear
            }
        }
    }
}

impl Default for BerEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// BER decoder helper
pub struct BerDecoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> BerDecoder<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Check if there's more data to decode
    pub fn has_more(&self) -> bool {
        self.pos < self.buf.len()
    }

    /// Decode the next TLV (Tag-Length-Value)
    pub fn decode_tlv(&mut self) -> io::Result<(u8, &'a [u8])> {
        if self.pos >= self.buf.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "BER: end of buffer"));
        }

        let tag = self.buf[self.pos];
        self.pos += 1;

        let len = self.decode_length()?;
        if self.pos + len > self.buf.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "BER: value extends past buffer"));
        }

        let value = &self.buf[self.pos..self.pos + len];
        self.pos += len;

        Ok((tag, value))
    }

    /// Decode length field
    fn decode_length(&mut self) -> io::Result<usize> {
        if self.pos >= self.buf.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "BER: no length byte"));
        }

        let first = self.buf[self.pos];
        self.pos += 1;

        if first < 128 {
            Ok(first as usize)
        } else if first == 0x81 {
            if self.pos >= self.buf.len() {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "BER: no length byte"));
            }
            let len = self.buf[self.pos] as usize;
            self.pos += 1;
            Ok(len)
        } else if first == 0x82 {
            if self.pos + 1 >= self.buf.len() {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "BER: no length bytes"));
            }
            let len = ((self.buf[self.pos] as usize) << 8) | (self.buf[self.pos + 1] as usize);
            self.pos += 2;
            Ok(len)
        } else {
            Err(io::Error::new(io::ErrorKind::InvalidData, format!("BER: unsupported length format 0x{:02X}", first)))
        }
    }

    /// Decode an INTEGER value
    pub fn decode_integer(&mut self) -> io::Result<i32> {
        let (tag, value) = self.decode_tlv()?;
        if tag != TAG_INTEGER {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("Expected INTEGER tag, got 0x{:02X}", tag)));
        }
        match value.len() {
            1 => Ok(value[0] as i8 as i32),
            2 => Ok(i16::from_be_bytes([value[0], value[1]]) as i32),
            4 => Ok(i32::from_be_bytes([value[0], value[1], value[2], value[3]])),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, format!("Unsupported integer length: {}", value.len()))),
        }
    }

    /// Peek at the next tag without consuming it
    pub fn peek_tag(&self) -> Option<u8> {
        if self.pos < self.buf.len() {
            Some(self.buf[self.pos])
        } else {
            None
        }
    }

    /// Skip the next TLV
    pub fn skip_tlv(&mut self) -> io::Result<()> {
        let _ = self.decode_tlv()?;
        Ok(())
    }
}

// ============================================================================
// ISO Session Layer (ISO 8327) — minimal implementation
// ============================================================================

/// ISO Session layer PDU types
const SESSION_CN: u8 = 0x13;  // Connect
const SESSION_AC: u8 = 0x0E;  // Accept
const SESSION_FN: u8 = 0x09;  // Finish
const SESSION_DN: u8 = 0x0C;  // Disconnect
const SESSION_DT: u8 = 0x01;  // Data
#[allow(dead_code)]
const SESSION_AB: u8 = 0x19;  // Abort

/// Build ISO Session Connect (CN) SPDU
fn build_session_connect(user_data: &[u8]) -> Vec<u8> {
    let mut spdu = Vec::with_capacity(16 + user_data.len());
    spdu.push(SESSION_CN);        // SPDU type
    // Session user data tag
    spdu.push(0x0A);              // Parameter id: session user data
    // Length of user data
    BerEncoder::encode_length(&mut spdu, user_data.len());
    spdu.extend_from_slice(user_data);
    spdu
}

/// Build ISO Session Data (DT) SPDU
fn build_session_data(user_data: &[u8]) -> Vec<u8> {
    let mut spdu = Vec::with_capacity(4 + user_data.len());
    spdu.push(SESSION_DT);        // SPDU type
    spdu.push(0x01);              // Parameter id
    spdu.push(user_data.len() as u8); // Length
    spdu.extend_from_slice(user_data);
    spdu
}

/// Parse ISO Session response — extract user data from AC or FN
fn parse_session_response(data: &[u8]) -> io::Result<Vec<u8>> {
    if data.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Empty session response"));
    }

    let spdu_type = data[0];
    match spdu_type {
        SESSION_AC => {
            // Accept: skip header to find user data
            // Minimal parsing: find the presentation data after session header
            let mut pos = 1;
            while pos + 2 < data.len() {
                let param_id = data[pos];
                if param_id == 0x0A || param_id == 0x0C {
                    // Session user data or session disconnect
                    let len = data[pos + 1] as usize;
                    if pos + 2 + len <= data.len() {
                        return Ok(data[pos + 2..pos + 2 + len].to_vec());
                    }
                }
                pos += 1;
            }
            // Fallback: return everything after the first few bytes
            if data.len() > 4 {
                Ok(data[4..].to_vec())
            } else {
                Err(io::Error::new(io::ErrorKind::InvalidData, "Cannot parse session AC"))
            }
        }
        SESSION_FN | SESSION_DN => {
            Err(io::Error::new(io::ErrorKind::ConnectionReset, "Session refused/disconnected"))
        }
        _ => {
            Err(io::Error::new(io::ErrorKind::InvalidData, format!("Unexpected session SPDU: 0x{:02X}", spdu_type)))
        }
    }
}

// ============================================================================
// ISO Presentation Layer (ISO 8823) — minimal implementation
// ============================================================================

/// Build ISO Presentation Connect PPDU (CP-type)
fn build_presentation_connect(session_data: &[u8]) -> Vec<u8> {
    // Presentation CP PPDU: mode-selector + normal-mode-parameters
    let mut ppdu = Vec::new();

    // Mode selector: choose normal mode (X.410-1984 mode = 1, normal = 2)
    ppdu.push(0x02);  // Context 2 [CONSTRUCTED]
    ppdu.push(0x01);  // Length 1
    ppdu.push(0x02);  // Normal mode

    // Normal mode parameters (context 0 constructed)
    let mut normal_params = Vec::new();

    // Called presentation selector (context 0)
    normal_params.extend_from_slice(&BerEncoder::encode_context(0, &[0x01]));

    // Calling presentation selector (context 2)
    normal_params.extend_from_slice(&BerEncoder::encode_context(2, &[0x01]));

    // Presentation context definition list (context 4 constructed)
    let mut context_list = Vec::new();

    // Context 1: MMS (ISO 9506)
    let mut ctx1 = Vec::new();
    ctx1.extend_from_slice(&BerEncoder::encode_integer(1));  // context id = 1
    ctx1.extend_from_slice(&BerEncoder::encode_integer(1));  // transfer syntax = 1 (BER)
    ctx1.extend_from_slice(&BerEncoder::encode_oid(&[1u32, 0, 9506, 2, 3])); // MMS OID
    context_list.extend_from_slice(&BerEncoder::encode_sequence(&ctx1));

    // Context 2: ACSE (ISO 8650)
    let mut ctx2 = Vec::new();
    ctx2.extend_from_slice(&BerEncoder::encode_integer(2));
    ctx2.extend_from_slice(&BerEncoder::encode_integer(1));
    ctx2.extend_from_slice(&BerEncoder::encode_oid(&[1u32, 0, 8650, 1, 1])); // ACSE OID
    context_list.extend_from_slice(&BerEncoder::encode_sequence(&ctx2));

    normal_params.extend_from_slice(&BerEncoder::encode_context_constructed(4, &context_list));

    // User data (context 30 constructed) — contains the ACSE AARQ
    normal_params.extend_from_slice(&BerEncoder::encode_context_constructed(30, session_data));

    ppdu.extend_from_slice(&BerEncoder::encode_context_constructed(0, &normal_params));

    ppdu
}

/// Parse ISO Presentation response — extract ACSE user data
fn parse_presentation_response(data: &[u8]) -> io::Result<Vec<u8>> {
    if data.len() < 4 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Presentation response too short"));
    }

    // Try to find the ACSE data within the presentation response
    // The structure is: CPA-type → mode-selector + normal-mode-parameters → user-data
    // For simplicity, search for the AARE tag (0x61)
    for i in 0..data.len().saturating_sub(1) {
        if data[i] == TAG_AARE {
            // Found AARE — return from this point
            let mut decoder = BerDecoder::new(&data[i..]);
            let (_tag, value) = decoder.decode_tlv()?;
            return Ok(value.to_vec());
        }
    }

    // Fallback: return the raw data for further parsing
    Ok(data.to_vec())
}

// ============================================================================
// ACSE (Association Control Service Element)
// ============================================================================

/// MMS OID components: 1.0.9506.2.3
const MMS_OID: &[u32] = &[1, 0, 9506, 2, 3];
/// ACSE OID components: 1.0.8650.1.1
const ACSE_OID: &[u32] = &[1, 0, 8650, 1, 1];

/// Build ACSE A-ASSOCIATE request (AARQ)
fn build_aarq() -> Vec<u8> {
    let mut aarq = Vec::new();

    // Application context name (context 0)
    let mut app_context = Vec::new();
    app_context.extend_from_slice(&BerEncoder::encode_oid(ACSE_OID));
    aarq.extend_from_slice(&BerEncoder::encode_context_constructed(0, &app_context));

    // Called AP title (context 1) — optional
    // Calling AP title (context 2) — optional

    // User information (context 30 constructed) — contains MMS InitiateRequest
    let mms_init = build_mms_initiate_request();
    let mut user_info = Vec::new();
    // DIRECT-REFERENCE (context 0): object identifier for MMS
    user_info.extend_from_slice(&BerEncoder::encode_context(0, &BerEncoder::encode_oid(MMS_OID)));
    // Single-ASN1-type (context 1): the MMS PDU
    user_info.extend_from_slice(&BerEncoder::encode_context_constructed(1, &mms_init));
    aarq.extend_from_slice(&BerEncoder::encode_context_constructed(30, &user_info));

    BerEncoder::encode_tl(TAG_AARQ, &aarq)
}

/// Parse ACSE A-ASSOCIATE response (AARE)
fn parse_aare(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoder = BerDecoder::new(data);

    // Look for result (context 2) and user information (context 30)
    while decoder.has_more() {
        let tag = decoder.peek_tag().unwrap_or(0);
        match tag {
            0xA2 => {
                // Result [2] — check if accepted
                let (_, value) = decoder.decode_tlv()?;
                // value should contain: INTEGER (result) + INTEGER (diagnostic)
                // result = 0 means accepted
                let mut inner = BerDecoder::new(value);
                if inner.has_more() {
                    let (_, result_val) = inner.decode_tlv()?;
                    if !result_val.is_empty() && result_val[0] != 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::ConnectionRefused,
                            format!("ACSE association rejected: result={}", result_val[0]),
                        ));
                    }
                }
            }
            0xBE => {
                // User information [30] — contains MMS InitiateResponse
                let (_, value) = decoder.decode_tlv()?;
                return Ok(value.to_vec());
            }
            _ => {
                decoder.skip_tlv()?;
            }
        }
    }

    Ok(Vec::new())
}

// ============================================================================
// MMS (Manufacturing Message Specification)
// ============================================================================

/// Build MMS InitiateRequest PDU
fn build_mms_initiate_request() -> Vec<u8> {
    let mut initiate = Vec::new();

    // localDetailCalling [0] — max segment size
    initiate.extend_from_slice(&BerEncoder::encode_context(0, &BerEncoder::encode_integer(65535)));

    // proposedMaxServOutstandingCalling [1]
    initiate.extend_from_slice(&BerEncoder::encode_context(1, &BerEncoder::encode_integer(16)));

    // proposedMaxServOutstandingCalled [2]
    initiate.extend_from_slice(&BerEncoder::encode_context(2, &BerEncoder::encode_integer(16)));

    // proposedDataStructureNesting [3]
    initiate.extend_from_slice(&BerEncoder::encode_context(3, &BerEncoder::encode_integer(10)));

    // initRequestDetail [4]
    let mut detail = Vec::new();
    // proposedVersion [0]
    detail.extend_from_slice(&BerEncoder::encode_context(0, &BerEncoder::encode_integer(1)));
    // proposedSupportedFeatures [1]
    detail.extend_from_slice(&BerEncoder::encode_context(1, &BerEncoder::encode_bit_string_features()));
    // servicesSupportedCalling [2] — bit string
    detail.extend_from_slice(&BerEncoder::encode_context(2, &BerEncoder::encode_bit_string_features()));
    initiate.extend_from_slice(&BerEncoder::encode_context_constructed(4, &detail));

    // InitiateRequest is context [0] CONSTRUCTED within MMS-PDU
    BerEncoder::encode_context_constructed(0, &initiate)
}

/// Build MMS Read request
pub fn build_mms_read(domain: &str, variable: &str) -> Vec<u8> {
    let mut read_req = Vec::new();

    // specificationWithResult [0] = FALSE
    read_req.extend_from_slice(&BerEncoder::encode_context(0, &BerEncoder::encode_boolean(false)));

    // variableAccessSpecification [1] — list of variable names
    let mut var_spec = Vec::new();
    // listOfVariable [0] CONSTRUCTED
    let mut var_list = Vec::new();
    // Single variable
    let mut var_entry = Vec::new();
    // variableSpecification [0] — domain-specific
    let mut var_spec_inner = Vec::new();
    // domainSpecific [1] CONSTRUCTED
    let mut domain_spec = Vec::new();
    domain_spec.extend_from_slice(&BerEncoder::encode_visible_string(domain));  // domainId
    domain_spec.extend_from_slice(&BerEncoder::encode_visible_string(variable)); // itemId
    var_spec_inner.extend_from_slice(&BerEncoder::encode_context_constructed(1, &domain_spec));
    var_entry.extend_from_slice(&BerEncoder::encode_context_constructed(0, &var_spec_inner));
    var_list.extend_from_slice(&BerEncoder::encode_sequence(&var_entry));
    var_spec.extend_from_slice(&BerEncoder::encode_context_constructed(0, &var_list));
    read_req.extend_from_slice(&BerEncoder::encode_context_constructed(1, &var_spec));

    // Read is context [2] CONSTRUCTED within ConfirmedServiceRequest
    BerEncoder::encode_context_constructed(2, &read_req)
}

/// Build MMS Write request
pub fn build_mms_write(domain: &str, variable: &str, value: &[u8]) -> Vec<u8> {
    let mut write_req = Vec::new();

    // variableAccessSpecification [0]
    let mut var_spec = Vec::new();
    let mut var_list = Vec::new();
    let mut var_entry = Vec::new();
    let mut var_spec_inner = Vec::new();
    let mut domain_spec = Vec::new();
    domain_spec.extend_from_slice(&BerEncoder::encode_visible_string(domain));
    domain_spec.extend_from_slice(&BerEncoder::encode_visible_string(variable));
    var_spec_inner.extend_from_slice(&BerEncoder::encode_context_constructed(1, &domain_spec));
    var_entry.extend_from_slice(&BerEncoder::encode_context_constructed(0, &var_spec_inner));
    var_list.extend_from_slice(&BerEncoder::encode_sequence(&var_entry));
    var_spec.extend_from_slice(&BerEncoder::encode_context_constructed(0, &var_list));
    write_req.extend_from_slice(&BerEncoder::encode_context_constructed(0, &var_spec));

    // listOfData [1]
    let mut data_list = Vec::new();
    data_list.extend_from_slice(&BerEncoder::encode_context_constructed(1, value));
    write_req.extend_from_slice(&BerEncoder::encode_context_constructed(1, &data_list));

    // Write is context [5] CONSTRUCTED within ConfirmedServiceRequest
    BerEncoder::encode_context_constructed(5, &write_req)
}

/// Parse MMS Read response — extract variable data
pub fn parse_mms_read_response(data: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoder = BerDecoder::new(data);
    while decoder.has_more() {
        let tag = decoder.peek_tag().unwrap_or(0);
        // Look for accessResult [1] or listOfAccessResult
        if tag == 0xA1 || tag == 0xA2 {
            let (_, value) = decoder.decode_tlv()?;
            return Ok(value.to_vec());
        }
        decoder.skip_tlv()?;
    }
    Ok(data.to_vec())
}

// ============================================================================
// MMS Client — high-level interface
// ============================================================================

/// MMS client for IEC 61850 communication
pub struct MmsClient {
    transport: CotpTransport,
    invoke_id: u32,
}

impl MmsClient {
    /// Connect to an IEC 61850 server via COTP + ACSE + MMS
    pub async fn connect(
        addr: &str,
        local_tsap: u16,
        remote_tsap: u16,
    ) -> io::Result<Self> {
        // Step 1: COTP connection
        let mut transport = CotpTransport::connect(addr, local_tsap, remote_tsap).await?;

        // Step 2: Build ACSE AARQ wrapped in Presentation + Session
        let aarq = build_aarq();
        let pres_connect = build_presentation_connect(&aarq);
        let session_connect = build_session_connect(&pres_connect);

        // Send COTP DT with session connect
        transport.send_data(&session_connect).await?;

        // Receive response
        let response = transport.recv_data().await?;

        // Parse session response
        let session_data = parse_session_response(&response)?;

        // Parse presentation response → get ACSE AARE
        let aare_data = parse_presentation_response(&session_data)?;

        // Parse ACSE AARE → get MMS InitiateResponse
        let _mms_init_resp = parse_aare(&aare_data)?;

        debug!("MMS client connected and associated");
        Ok(Self {
            transport,
            invoke_id: 1,
        })
    }

    /// Read a named variable from the server
    pub async fn read_variable(&mut self, domain: &str, variable: &str) -> io::Result<Vec<u8>> {
        // Build MMS Read request wrapped in ConfirmedServiceRequest → MMS-PDU
        let read_req = build_mms_read(domain, variable);

        // ConfirmedServiceRequest [2] CONSTRUCTED
        let csr = BerEncoder::encode_context_constructed(2, &read_req);

        // Confirmed-RequestPDU [0] CONSTRUCTED
        let mut req_pdu = Vec::new();
        req_pdu.extend_from_slice(&BerEncoder::encode_context(0, &BerEncoder::encode_integer(self.invoke_id as i32)));
        req_pdu.extend_from_slice(&csr);
        let mms_pdu = BerEncoder::encode_context_constructed(0, &req_pdu);

        // Wrap in Session DT
        let session_dt = build_session_data(&mms_pdu);
        self.transport.send_data(&session_dt).await?;

        // Receive response
        let response = self.transport.recv_data().await?;

        // Parse session DT to get MMS response
        let mms_resp = if response.len() > 3 && response[0] == SESSION_DT {
            &response[3..]
        } else {
            &response
        };

        self.invoke_id += 1;
        parse_mms_read_response(mms_resp)
    }

    /// Write a value to a named variable on the server
    pub async fn write_variable(&mut self, domain: &str, variable: &str, value: &[u8]) -> io::Result<()> {
        let write_req = build_mms_write(domain, variable, value);

        let csr = BerEncoder::encode_context_constructed(2, &write_req);
        let mut req_pdu = Vec::new();
        req_pdu.extend_from_slice(&BerEncoder::encode_context(0, &BerEncoder::encode_integer(self.invoke_id as i32)));
        req_pdu.extend_from_slice(&csr);
        let mms_pdu = BerEncoder::encode_context_constructed(0, &req_pdu);

        let session_dt = build_session_data(&mms_pdu);
        self.transport.send_data(&session_dt).await?;

        // Receive and discard write response
        let _ = self.transport.recv_data().await?;

        self.invoke_id += 1;
        Ok(())
    }

    /// Disconnect from the server
    pub async fn disconnect(&mut self) -> io::Result<()> {
        self.transport.disconnect().await
    }
}

// ============================================================================
// BER encoder extensions for MMS
// ============================================================================

impl BerEncoder {
    /// Encode a BIT STRING with common MMS features
    fn encode_bit_string_features() -> Vec<u8> {
        // Bit string with all features supported (simplified)
        let bs = vec![TAG_BIT_STRING, 0x03, 0x00, 0xFF, 0xFF];
        bs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ber_encode_integer() {
        let enc = BerEncoder::encode_integer(42);
        assert_eq!(enc[0], TAG_INTEGER);
        assert_eq!(enc[1], 1);
        assert_eq!(enc[2], 42);
    }

    #[test]
    fn test_ber_encode_integer_negative() {
        let enc = BerEncoder::encode_integer(-1);
        assert_eq!(enc[0], TAG_INTEGER);
        assert_eq!(enc[2], 0xFF);
    }

    #[test]
    fn test_ber_encode_boolean() {
        let enc = BerEncoder::encode_boolean(true);
        assert_eq!(enc[0], TAG_BOOLEAN);
        assert_eq!(enc[2], 0xFF);

        let enc = BerEncoder::encode_boolean(false);
        assert_eq!(enc[2], 0x00);
    }

    #[test]
    fn test_ber_encode_sequence() {
        let inner = BerEncoder::encode_integer(1);
        let seq = BerEncoder::encode_sequence(&inner);
        assert_eq!(seq[0], TAG_SEQUENCE);
    }

    #[test]
    fn test_ber_decode_integer() {
        let enc = BerEncoder::encode_integer(42);
        let mut dec = BerDecoder::new(&enc);
        let val = dec.decode_integer().unwrap();
        assert_eq!(val, 42);
    }

    #[test]
    fn test_ber_decode_tlv() {
        let enc = BerEncoder::encode_tl(0x02, &[42]);
        let mut dec = BerDecoder::new(&enc);
        let (tag, value) = dec.decode_tlv().unwrap();
        assert_eq!(tag, 0x02);
        assert_eq!(value, &[42]);
    }

    #[test]
    fn test_ber_encode_context() {
        let enc = BerEncoder::encode_context(5, &[0x01]);
        assert_eq!(enc[0], BER_CONTEXT | 5);
    }

    #[test]
    fn test_ber_encode_oid() {
        let enc = BerEncoder::encode_oid(&[1u32, 0, 9506]);
        assert_eq!(enc[0], TAG_OID);
    }

    #[test]
    fn test_build_mms_read() {
        let read = build_mms_read("LD0", "GGIO1.AnIn1.mag");
        assert!(!read.is_empty());
        // Should start with context [2] CONSTRUCTED
        assert_eq!(read[0], BER_CONTEXT | BER_CONSTRUCTED | 2);
    }

    #[test]
    fn test_build_mms_write() {
        let write = build_mms_write("LD0", "GGIO1.AnIn1.mag", &[0x01]);
        assert!(!write.is_empty());
        // Should start with context [5] CONSTRUCTED
        assert_eq!(write[0], BER_CONTEXT | BER_CONSTRUCTED | 5);
    }

    #[test]
    fn test_ber_roundtrip_large_integer() {
        let enc = BerEncoder::encode_integer(1000);
        let mut dec = BerDecoder::new(&enc);
        let val = dec.decode_integer().unwrap();
        assert_eq!(val, 1000);
    }

    #[test]
    fn test_ber_encode_visible_string() {
        let enc = BerEncoder::encode_visible_string("LD0");
        assert_eq!(enc[0], 0x0C); // UTF8String tag
    }
}
