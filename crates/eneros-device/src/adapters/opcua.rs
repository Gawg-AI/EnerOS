//! OPC UA (Open Platform Communications Unified Architecture) client adapter.
//!
//! OPC UA is the dominant protocol for renewable energy plants (solar, wind)
//! and industrial automation. This adapter provides a client implementation
//! supporting node browsing, attribute reading, subscriptions, and method
//! calls.
//!
//! # Architecture
//!
//! ```text
//! OPC UA Server (SCADA / PLC / Gateway)
//!         │
//!         │ TCP (port 4840) + OPC UA Binary
//!         ▼
//! OpcUaClient (this file)
//!   ├── Hello/Acknowledge handshake
//!   ├── OpenSecureChannel
//!   ├── CreateSession / ActivateSession
//!   ├── Read / Write / Browse / Call
//!   └── CreateSubscription / CreateMonitoredItems
//!         │
//!         ▼
//! OpcUaAdapter → ProtocolAdapter trait
//! ```
//!
//! # Address Format
//!
//! `ns=<namespace>;<identifier>` (e.g., "ns=2;s=Temperature.Sensor1")
//! Also supports numeric: `ns=2;i=1234`

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use eneros_core::Result;
use crate::adapter::{
    ProtocolAdapter, ConnectionConfig, DataPoint, DataValue, DataQuality,
    SharedState, new_shared_state,
};
use crate::protocol::ProtocolType;

/// OPC UA node identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OpcUaNodeId {
    pub namespace: u16,
    pub identifier: NodeIdType,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeIdType {
    /// Numeric identifier
    Numeric(u32),
    /// String identifier
    String(String),
    /// GUID identifier (as hex string)
    Guid(String),
    /// ByteString identifier (as hex)
    ByteString(String),
}

impl OpcUaNodeId {
    /// Parse from standard OPC UA address format.
    ///
    /// Examples:
    /// - "ns=2;s=Temperature" → String identifier
    /// - "ns=0;i=85" → Numeric identifier (ObjectsFolder)
    /// - "i=2258" → Server.ServerStatus.CurrentTime (ns=0)
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        let mut namespace: u16 = 0;
        let mut identifier: Option<NodeIdType> = None;

        for part in s.split(';') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix("ns=") {
                namespace = rest.parse().map_err(|_| format!("invalid namespace: {}", rest))?;
            } else if let Some(rest) = part.strip_prefix("s=") {
                identifier = Some(NodeIdType::String(rest.to_string()));
            } else if let Some(rest) = part.strip_prefix("i=") {
                let n: u32 = rest.parse().map_err(|_| format!("invalid numeric id: {}", rest))?;
                identifier = Some(NodeIdType::Numeric(n));
            } else if let Some(rest) = part.strip_prefix("g=") {
                identifier = Some(NodeIdType::Guid(rest.to_string()));
            } else if let Some(rest) = part.strip_prefix("b=") {
                identifier = Some(NodeIdType::ByteString(rest.to_string()));
            }
        }

        match identifier {
            Some(id) => Ok(Self { namespace, identifier: id }),
            None => Err(format!("no identifier found in '{}'", s)),
        }
    }

    /// Format as standard OPC UA address string.
    ///
    /// Note: This is a convenience method. Prefer `format!("{}", node_id)`
    /// which uses the `Display` implementation.
    pub fn to_address_string(&self) -> String {
        match &self.identifier {
            NodeIdType::Numeric(n) => format!("ns={};i={}", self.namespace, n),
            NodeIdType::String(s) => format!("ns={};s={}", self.namespace, s),
            NodeIdType::Guid(g) => format!("ns={};g={}", self.namespace, g),
            NodeIdType::ByteString(b) => format!("ns={};b={}", self.namespace, b),
        }
    }
}

impl std::fmt::Display for OpcUaNodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.identifier {
            NodeIdType::Numeric(n) => write!(f, "ns={};i={}", self.namespace, n),
            NodeIdType::String(s) => write!(f, "ns={};s={}", self.namespace, s),
            NodeIdType::Guid(g) => write!(f, "ns={};g={}", self.namespace, g),
            NodeIdType::ByteString(b) => write!(f, "ns={};b={}", self.namespace, b),
        }
    }
}

/// OPC UA configuration.
#[derive(Debug, Clone)]
pub struct OpcUaConfig {
    pub endpoint_url: String,
    pub security_policy: String,
    pub security_mode: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub request_timeout_ms: u64,
    pub session_name: String,
}

impl Default for OpcUaConfig {
    fn default() -> Self {
        Self {
            endpoint_url: "opc.tcp://localhost:4840".to_string(),
            security_policy: "None".to_string(),
            security_mode: "None".to_string(),
            username: None,
            password: None,
            request_timeout_ms: 5000,
            session_name: "EnerOS-Client".to_string(),
        }
    }
}

/// OPC UA attribute identifier (standard attribute IDs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributeId {
    Value = 13,
    NodeId = 1,
    NodeClass = 2,
    BrowseName = 3,
    DisplayName = 4,
    Description = 5,
    DataType = 12,
    AccessLevel = 14,
}

/// OPC UA node class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeClass {
    Object = 1,
    Variable = 2,
    Method = 4,
    ObjectType = 8,
    VariableType = 16,
    ReferenceType = 32,
    DataType = 64,
    View = 128,
    Unknown = 0,
}

impl NodeClass {
    pub fn from_u32(val: u32) -> Self {
        match val {
            1 => Self::Object,
            2 => Self::Variable,
            4 => Self::Method,
            8 => Self::ObjectType,
            16 => Self::VariableType,
            32 => Self::ReferenceType,
            64 => Self::DataType,
            128 => Self::View,
            _ => Self::Unknown,
        }
    }
}

/// OPC UA variant — the universal data container.
#[derive(Debug, Clone, PartialEq)]
pub enum OpcUaVariant {
    Null,
    Boolean(bool),
    SByte(i8),
    Byte(u8),
    Int16(i16),
    UInt16(u16),
    Int32(i32),
    UInt32(u32),
    Int64(i64),
    UInt64(u64),
    Float(f32),
    Double(f64),
    String(String),
    ByteString(Vec<u8>),
    DateTime(i64),
    Array(Vec<OpcUaVariant>),
}

impl OpcUaVariant {
    /// Convert to DataValue for the ProtocolAdapter interface.
    pub fn to_data_value(&self) -> DataValue {
        match self {
            Self::Null => DataValue::Bytes(Vec::new()),
            Self::Boolean(v) => DataValue::Bool(*v),
            Self::SByte(v) => DataValue::Int16(*v as i16),
            Self::Byte(v) => DataValue::Int32(*v as i32),
            Self::Int16(v) => DataValue::Int16(*v),
            Self::UInt16(v) => DataValue::Int32(*v as i32),
            Self::Int32(v) => DataValue::Int32(*v),
            Self::UInt32(v) => DataValue::Int64(*v as i64),
            Self::Int64(v) => DataValue::Int64(*v),
            Self::UInt64(v) => DataValue::Int64(*v as i64),
            Self::Float(v) => DataValue::Float32(*v),
            Self::Double(v) => DataValue::Float64(*v),
            Self::String(v) => DataValue::String(v.clone()),
            Self::ByteString(v) => DataValue::Bytes(v.clone()),
            Self::DateTime(v) => DataValue::Int64(*v),
            Self::Array(arr) => {
                if arr.is_empty() {
                    DataValue::Bytes(Vec::new())
                } else {
                    arr[0].to_data_value()
                }
            }
        }
    }

    /// Get the OPC UA type ID (built-in type encoding number).
    pub fn type_id(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Boolean(_) => 1,
            Self::SByte(_) => 2,
            Self::Byte(_) => 3,
            Self::Int16(_) => 4,
            Self::UInt16(_) => 5,
            Self::Int32(_) => 6,
            Self::UInt32(_) => 7,
            Self::Int64(_) => 8,
            Self::UInt64(_) => 9,
            Self::Float(_) => 10,
            Self::Double(_) => 11,
            Self::String(_) => 12,
            Self::DateTime(_) => 13,
            Self::ByteString(_) => 15,
            Self::Array(_) => 22,
        }
    }
}

/// Browse result for a single reference.
#[derive(Debug, Clone)]
pub struct BrowseResult {
    pub node_id: OpcUaNodeId,
    pub browse_name: String,
    pub display_name: String,
    pub node_class: NodeClass,
}

/// OPC UA client — low-level TCP connection with OPC UA binary protocol.
///
/// This implementation provides the core OPC UA binary protocol:
/// - Hello/Acknowledge
/// - OpenSecureChannel
/// - CreateSession / ActivateSession
/// - Read / Write
/// - Browse
/// - CreateSubscription / CreateMonitoredItems
pub struct OpcUaClient {
    config: OpcUaConfig,
    stream: Option<TcpStream>,
    /// Channel ID from OpenSecureChannel response
    channel_id: u32,
    /// Token ID from OpenSecureChannel response
    token_id: u32,
    /// Session ID from CreateSession response
    session_id: Vec<u8>,
    /// Authentication token from ActivateSession
    auth_token: Vec<u8>,
    /// Subscription ID (created on demand)
    subscription_id: Option<u32>,
    /// Sequence number for messages
    sequence_number: u32,
    /// Request handle counter
    request_handle: u32,
}

impl OpcUaClient {
    /// Create a new OPC UA client with the given configuration.
    pub fn new(config: OpcUaConfig) -> Self {
        Self {
            config,
            stream: None,
            channel_id: 0,
            token_id: 0,
            session_id: Vec::new(),
            auth_token: Vec::new(),
            subscription_id: None,
            sequence_number: 1,
            request_handle: 1,
        }
    }

    /// Connect to the OPC UA server.
    pub async fn connect(&mut self) -> std::result::Result<(), String> {
        // Parse endpoint URL to extract host:port
        let (host, port) = parse_endpoint(&self.config.endpoint_url)?;

        let stream = tokio::time::timeout(
            std::time::Duration::from_millis(self.config.request_timeout_ms),
            TcpStream::connect((host.as_str(), port)),
        )
        .await
        .map_err(|_| format!("connect timeout to {}:{}", host, port))?
        .map_err(|e| format!("TCP connect failed: {}", e))?;

        self.stream = Some(stream);

        // Send Hello
        self.send_hello().await?;
        // Receive Acknowledge
        self.recv_acknowledge().await?;
        // OpenSecureChannel
        self.open_secure_channel().await?;
        // CreateSession
        self.create_session().await?;
        // ActivateSession
        self.activate_session().await?;

        Ok(())
    }

    /// Disconnect from the server.
    pub async fn disconnect(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            // Best-effort close
            let _ = stream.shutdown().await;
        }
    }

    /// Check if connected.
    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Read a node's value attribute.
    ///
    /// Sends a ReadRequest (service ID 631) over the secure channel and
    /// parses the ReadResponse to extract the value.
    pub async fn read_value(&mut self, node_id: &OpcUaNodeId) -> std::result::Result<OpcUaVariant, String> {
        if self.stream.is_none() {
            return Err("not connected".into());
        }

        use crate::adapters::opcua_codec::*;

        // Build ReadRequest body
        let mut body = BinaryWriter::new();
        // Type ID (ExpandedNodeId for ReadRequest)
        body.write_u8(0x00); // Two-byte NodeId encoding
        body.write_u8(ID_READ_REQ as u8); // Service ID (assumes < 256)
        // RequestHeader
        let req_handle = self.next_request_handle();
        body.write_request_header(&self.auth_token, req_handle);
        // MaxAge
        body.write_f64(0.0);
        // TimestampsToReturn (0 = Neither)
        body.write_u32(0);
        // NodesToRead array length
        body.write_i32(1);
        // ReadValueId:
        body.write_node_id(node_id); // NodeId
        body.write_u32(13); // AttributeId = Value (13)
        body.write_string(""); // IndexRange (null)
        // QualifiedName (DataEncoding): namespace=0, name=null
        body.write_u16(0);
        body.write_i32(-1); // null string

        let frame = build_msg_frame(
            self.channel_id,
            self.token_id,
            self.next_sequence_number(),
            1,
            body.as_bytes(),
        );

        self.send_message(&frame).await?;

        // Parse response
        let resp = self.recv_message().await?;
        let mut reader = BinaryReader::new(&resp);
        // Skip response header
        let (_, _, status_code) = reader.read_response_header()?;
        if status_code != 0 {
            return Err(format!("Read service failed: status 0x{:08x}", status_code));
        }
        // Results array length
        let num_results = reader.read_i32()?;
        if num_results < 1 {
            return Err("Read returned no results".into());
        }
        // DataValue: encoding mask
        let mask = reader.read_u8()?;
        let has_value = (mask & 0x01) != 0;
        let has_status = (mask & 0x02) != 0;
        if has_value {
            let variant = reader.read_variant()?;
            if has_status {
                let _status = reader.read_u32()?;
            }
            Ok(variant)
        } else {
            Ok(OpcUaVariant::Null)
        }
    }

    /// Write a node's value attribute.
    ///
    /// Sends a WriteRequest (service ID 673) over the secure channel.
    pub async fn write_value(
        &mut self,
        node_id: &OpcUaNodeId,
        value: OpcUaVariant,
    ) -> std::result::Result<(), String> {
        if self.stream.is_none() {
            return Err("not connected".into());
        }

        use crate::adapters::opcua_codec::*;

        // Build WriteRequest body
        let mut body = BinaryWriter::new();
        // Type ID (ExpandedNodeId for WriteRequest)
        body.write_u8(0x00);
        body.write_u8(ID_WRITE_REQ as u8);
        // RequestHeader
        let req_handle = self.next_request_handle();
        body.write_request_header(&self.auth_token, req_handle);
        // NodesToWrite array length
        body.write_i32(1);
        // WriteValue:
        body.write_node_id(node_id); // NodeId
        body.write_u32(13); // AttributeId = Value (13)
        body.write_string(""); // IndexRange (null)
        // DataValue (encoding mask = 0x01 = has value)
        body.write_u8(0x01);
        body.write_variant(&value);

        let frame = build_msg_frame(
            self.channel_id,
            self.token_id,
            self.next_sequence_number(),
            1,
            body.as_bytes(),
        );

        self.send_message(&frame).await?;

        // Parse response
        let resp = self.recv_message().await?;
        let mut reader = BinaryReader::new(&resp);
        let (_, _, status_code) = reader.read_response_header()?;
        if status_code != 0 {
            return Err(format!("Write service failed: status 0x{:08x}", status_code));
        }
        // Results array
        let num_results = reader.read_i32()?;
        if num_results >= 1 {
            let write_status = reader.read_u32()?;
            if write_status != 0 {
                return Err(format!("Write failed for node: status 0x{:08x}", write_status));
            }
        }
        Ok(())
    }

    /// Browse a node's children.
    ///
    /// Sends a BrowseRequest (service ID 527) to enumerate references
    /// from the given node.
    pub async fn browse(
        &mut self,
        node_id: &OpcUaNodeId,
    ) -> std::result::Result<Vec<BrowseResult>, String> {
        if self.stream.is_none() {
            return Err("not connected".into());
        }

        use crate::adapters::opcua_codec::*;

        // Build BrowseRequest body
        let mut body = BinaryWriter::new();
        body.write_u8(0x00);
        body.write_u8(ID_BROWSE_REQ as u8);
        let req_handle = self.next_request_handle();
        body.write_request_header(&self.auth_token, req_handle);
        // ViewDescription: ViewId (null), Timestamp, ViewVersion
        body.write_u8(0x00); // null NodeId
        body.write_u8(0);
        body.write_i64(0); // timestamp
        body.write_u32(0); // view version
        // MaxReferencesToReturn
        body.write_u32(0); // 0 = no limit
        // NodesToBrowse array length
        body.write_i32(1);
        // BrowseDescription:
        body.write_node_id(node_id); // NodeId
        body.write_u32(0); // BrowseDirection = Forward
        // ReferenceTypeId (null NodeId = all references)
        body.write_u8(0x00);
        body.write_u8(0);
        body.write_bool(true); // IncludeSubtypes
        body.write_u32(63); // NodeClassMask = all
        body.write_bool(true); // ResultMask = all

        let frame = build_msg_frame(
            self.channel_id,
            self.token_id,
            self.next_sequence_number(),
            1,
            body.as_bytes(),
        );

        self.send_message(&frame).await?;
        let resp = self.recv_message().await?;
        let mut reader = BinaryReader::new(&resp);
        let (_, _, status_code) = reader.read_response_header()?;
        if status_code != 0 {
            return Err(format!("Browse service failed: status 0x{:08x}", status_code));
        }

        // Results array
        let num_results = reader.read_i32()?;
        let mut browse_results = Vec::new();
        if num_results >= 1 {
            // BrowseResult:
            let _status = reader.read_u32()?; // StatusCode
            let _continuation = reader.read_u32()?; // ContinuationPoint
            let num_refs = reader.read_i32()?; // References array length
            for _ in 0..num_refs {
                // ReferenceDescription:
                let _ref_type_id = reader.read_node_id()?;
                let _is_forward = reader.read_bool()?;
                let target_node_id = reader.read_node_id()?;
                let _browse_name_ns = reader.read_u16()?;
                let _browse_name = reader.read_string()?;
                let _display_name_ns = reader.read_u16()?;
                let display_name = reader.read_string()?;
                let node_class_val = reader.read_u32()?;
                let _type_def = reader.read_node_id()?;

                browse_results.push(BrowseResult {
                    node_id: target_node_id,
                    browse_name: String::new(),
                    display_name,
                    node_class: NodeClass::from_u32(node_class_val),
                });
            }
        }
        Ok(browse_results)
    }

    /// Create a subscription for monitored items.
    ///
    /// Sends a CreateSubscriptionRequest (service ID 787) and returns
    /// the subscription ID.
    pub async fn create_subscription(
        &mut self,
        publishing_interval_ms: f64,
    ) -> std::result::Result<u32, String> {
        if self.stream.is_none() {
            return Err("not connected".into());
        }

        use crate::adapters::opcua_codec::*;

        // Build CreateSubscriptionRequest body
        let mut body = BinaryWriter::new();
        body.write_u8(0x00);
        body.write_u8(ID_CREATE_SUBSCRIPTION_REQ as u8);
        let req_handle = self.next_request_handle();
        body.write_request_header(&self.auth_token, req_handle);
        // RequestedPublishingInterval
        body.write_f64(publishing_interval_ms);
        // RequestedLifetimeCount
        body.write_u32(10000);
        // RequestedMaxKeepAliveCount
        body.write_u32(3000);
        // MaxNotificationsPerPublish
        body.write_u32(0);
        // PublishingEnabled
        body.write_bool(true);
        // Priority
        body.write_u8(0);

        let frame = build_msg_frame(
            self.channel_id,
            self.token_id,
            self.next_sequence_number(),
            1,
            body.as_bytes(),
        );

        self.send_message(&frame).await?;
        let resp = self.recv_message().await?;
        let mut reader = BinaryReader::new(&resp);
        let (_, _, status_code) = reader.read_response_header()?;
        if status_code != 0 {
            return Err(format!("CreateSubscription failed: status 0x{:08x}", status_code));
        }

        // Response body: SubscriptionId, RevisedPublishingInterval, RevisedLifetimeCount, RevisedMaxKeepAliveCount
        let subscription_id = reader.read_u32()?;
        let _revised_interval = reader.read_f64()?;
        let _revised_lifetime = reader.read_u32()?;
        let _revised_keepalive = reader.read_u32()?;

        self.subscription_id = Some(subscription_id);
        Ok(subscription_id)
    }

    async fn send_hello(&mut self) -> std::result::Result<(), String> {
        let stream = self.stream.as_mut().ok_or("not connected")?;
        // OPC UA Hello message
        let mut msg = Vec::with_capacity(56);
        // Message type: "HEL"
        msg.extend_from_slice(b"HEL");
        // Chunk type: 'F' (final)
        msg.push(b'F');
        // Message size (placeholder)
        let size_pos = msg.len();
        msg.extend_from_slice(&0u32.to_le_bytes());
        // Protocol version
        msg.extend_from_slice(&0u32.to_le_bytes());
        // Receive buffer size
        msg.extend_from_slice(&65535u32.to_le_bytes());
        // Send buffer size
        msg.extend_from_slice(&65535u32.to_le_bytes());
        // Max message size
        msg.extend_from_slice(&0u32.to_le_bytes());
        // Max chunk count
        msg.extend_from_slice(&0u32.to_le_bytes());
        // Endpoint URL
        let url = self.config.endpoint_url.as_bytes();
        msg.extend_from_slice(&(url.len() as u32).to_le_bytes());
        msg.extend_from_slice(url);

        // Update message size
        let total = msg.len() as u32;
        msg[size_pos..size_pos + 4].copy_from_slice(&total.to_le_bytes());

        stream.write_all(&msg).await.map_err(|e| format!("send hello: {}", e))?;
        Ok(())
    }

    async fn recv_acknowledge(&mut self) -> std::result::Result<(), String> {
        let stream = self.stream.as_mut().ok_or("not connected")?;
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).await.map_err(|e| format!("recv ack: {}", e))?;

        if &header[0..3] != b"ACK" {
            return Err(format!("expected ACK, got {:?}", &header[0..3]));
        }
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if size > 8 {
            let mut rest = vec![0u8; size - 8];
            stream.read_exact(&mut rest).await.map_err(|e| format!("recv ack body: {}", e))?;
        }
        Ok(())
    }

    async fn open_secure_channel(&mut self) -> std::result::Result<(), String> {
        use crate::adapters::opcua_codec::*;

        // Build OpenSecureChannelRequest body
        let mut body = BinaryWriter::new();
        // Type ID (ExpandedNodeId for OpenSecureChannelRequest)
        body.write_u8(0x00); // Two-byte NodeId encoding
        body.write_u8(ID_OPEN_SECURE_CHANNEL_REQ as u8);
        // RequestHeader (no auth token for OPN)
        body.write_request_header(&[], self.next_request_handle());
        // ClientProtocolVersion
        body.write_u32(0);
        // RequestType (0 = Issue)
        body.write_u32(0);
        // SecurityMode (1 = None)
        body.write_u32(1);
        // ClientNonce (null bytestring)
        body.write_i32(-1);
        // RequestedLifetime (ms)
        body.write_u32(600000);

        let frame = build_opn_frame(0, self.next_sequence_number(), 1, body.as_bytes());

        // Send OPN frame
        let stream = self.stream.as_mut().ok_or("not connected")?;
        stream.write_all(&frame).await.map_err(|e| format!("send OPN: {}", e))?;

        // Receive OPN response
        let resp_header = [0u8; 8];
        let mut header_buf = resp_header;
        stream.read_exact(&mut header_buf).await.map_err(|e| format!("recv OPN header: {}", e))?;

        if &header_buf[0..3] != b"OPN" {
            return Err(format!("expected OPN response, got {:?}", &header_buf[0..3]));
        }
        let resp_size = u32::from_le_bytes([header_buf[4], header_buf[5], header_buf[6], header_buf[7]]) as usize;
        if resp_size > 8 {
            let mut resp_body = vec![0u8; resp_size - 8];
            stream.read_exact(&mut resp_body).await.map_err(|e| format!("recv OPN body: {}", e))?;

            // Parse the OPN response to extract channel_id and token_id
            // Skip security header (varies by policy, but for "None" it's:
            //   security_policy_uri (string) + sender_certificate (bytestring) + receiver_thumbprint (bytestring)
            let mut reader = BinaryReader::new(&resp_body);
            // Channel ID
            self.channel_id = reader.read_u32()?;
            // Security policy URI
            let _uri = reader.read_string()?;
            // Sender certificate
            let _cert = reader.read_bytestring()?;
            // Receiver thumbprint
            let _thumb = reader.read_bytestring()?;
            // Sequence number
            let _seq = reader.read_u32()?;
            // Request ID
            let _req = reader.read_u32()?;
            // Service response: type ID
            let _type_id = reader.read_node_id()?;
            // Response header
            let (_, _, _status) = reader.read_response_header()?;
            // OpenSecureChannelResponse body:
            // ServerProtocolVersion (u32), SecurityToken:
            //   ChannelId, TokenId, CreatedAt, ModifiedAt
            let _server_protocol = reader.read_u32()?;
            self.channel_id = reader.read_u32()?;
            self.token_id = reader.read_u32()?;
        }

        Ok(())
    }

    async fn create_session(&mut self) -> std::result::Result<(), String> {
        use crate::adapters::opcua_codec::*;

        // Build CreateSessionRequest body
        let mut body = BinaryWriter::new();
        body.write_u8(0x00);
        body.write_u8(ID_CREATE_SESSION_REQ as u8);
        let req_handle = self.next_request_handle();
        body.write_request_header(&self.auth_token, req_handle);
        // ClientDescription (ApplicationDescription):
        //   ApplicationUri, ProductUri, ApplicationName (LocalizedText),
        //   ApplicationType, GatewayServerUri, DiscoveryProfileUri, DiscoveryUrls
        body.write_string(""); // ApplicationUri
        body.write_string(""); // ProductUri
        // LocalizedText: encoding=0x02 (has text), locale=null, text=...
        body.write_u8(0x02);
        body.write_i32(-1); // locale null
        body.write_string("EnerOS"); // text
        body.write_u32(1); // ApplicationType = Client
        body.write_i32(-1); // GatewayServerUri null
        body.write_i32(-1); // DiscoveryProfileUri null
        body.write_i32(0); // DiscoveryUrls array length = 0
        // ServerUri (null)
        body.write_i32(-1);
        // EndpointUrl
        body.write_string(&self.config.endpoint_url);
        // SessionName
        body.write_string(&self.config.session_name);
        // ClientNonce (32 bytes)
        let nonce: Vec<u8> = (0..32).map(|i| i as u8).collect();
        body.write_bytestring(&nonce);
        // ClientCertificate (null)
        body.write_i32(-1);
        // RequestedSessionTimeout
        body.write_f64(1200000.0); // 20 minutes
        // MaxResponseMessageSize
        body.write_u32(0); // 0 = no limit

        let frame = build_msg_frame(
            self.channel_id,
            self.token_id,
            self.next_sequence_number(),
            1,
            body.as_bytes(),
        );

        self.send_message(&frame).await?;
        let resp = self.recv_message().await?;
        let mut reader = BinaryReader::new(&resp);
        let (_, _, status_code) = reader.read_response_header()?;
        if status_code != 0 {
            return Err(format!("CreateSession failed: status 0x{:08x}", status_code));
        }

        // CreateSessionResponse body:
        // SessionId (NodeId), AuthenticationToken (NodeId), RevisedSessionTimeout,
        // ServerNonce, ServerCertificate, ServerEndpoints[], ServerSoftwareCertificates[],
        // ServerSignature, MaxRequestMessageSize
        let session_node_id = reader.read_node_id()?;
        let auth_node_id = reader.read_node_id()?;
        let _revised_timeout = reader.read_f64()?;
        let _server_nonce = reader.read_bytestring()?;
        let _server_cert = reader.read_bytestring()?;

        // Store session ID and auth token as bytes
        let mut id_writer = BinaryWriter::new();
        id_writer.write_node_id(&session_node_id);
        self.session_id = id_writer.into_bytes();

        let mut token_writer = BinaryWriter::new();
        token_writer.write_node_id(&auth_node_id);
        self.auth_token = token_writer.into_bytes();

        Ok(())
    }

    async fn activate_session(&mut self) -> std::result::Result<(), String> {
        use crate::adapters::opcua_codec::*;

        // Build ActivateSessionRequest body
        let mut body = BinaryWriter::new();
        body.write_u8(0x00);
        body.write_u8(ID_ACTIVATE_SESSION_REQ as u8);
        let req_handle = self.next_request_handle();
        body.write_request_header(&self.auth_token, req_handle);
        // ClientSignature (SignatureData): Algorithm null, Signature null
        body.write_i32(-1);
        body.write_i32(-1);
        // ClientSoftwareCertificates array (empty)
        body.write_i32(0);
        // LocaleIds array (empty)
        body.write_i32(0);
        // UserIdentityToken (ExtensionObject):
        //   TypeId = AnonymousIdentityToken (ns=0, i=321)
        //   321 > 255, so use four-byte NodeId encoding (0x01)
        body.write_u8(0x01); // Four-byte NodeId
        body.write_u8(0); // namespace = 0
        body.write_u16(321); // AnonymousIdentityToken
        body.write_u8(0x01); // Encoding = has binary body
        // AnonymousIdentityToken body: PolicyId (string)
        // Body length = 4 bytes (i32 length for empty string = 4, with -1 for null)
        body.write_i32(4); // body length: just the PolicyId string
        body.write_i32(-1); // PolicyId = null string
        // UserTokenSignature (SignatureData): Algorithm null, Signature null
        body.write_i32(-1);
        body.write_i32(-1);

        let frame = build_msg_frame(
            self.channel_id,
            self.token_id,
            self.next_sequence_number(),
            1,
            body.as_bytes(),
        );

        self.send_message(&frame).await?;
        let resp = self.recv_message().await?;
        let mut reader = BinaryReader::new(&resp);
        let (_, _, status_code) = reader.read_response_header()?;
        if status_code != 0 {
            return Err(format!("ActivateSession failed: status 0x{:08x}", status_code));
        }

        // ActivateSessionResponse: ServerNonce, Results[], DiagnosticInfos[]
        let _server_nonce = reader.read_bytestring()?;
        Ok(())
    }

    /// Send a message frame over the TCP connection.
    async fn send_message(&mut self, frame: &[u8]) -> std::result::Result<(), String> {
        let stream = self.stream.as_mut().ok_or("not connected")?;
        stream.write_all(frame).await.map_err(|e| format!("send: {}", e))?;
        Ok(())
    }

    /// Receive a MSG response and return just the body (after the 8-byte
    /// type/chunk/size header, 4-byte channel ID, 4-byte token ID,
    /// 4-byte sequence number, and 4-byte request ID).
    async fn recv_message(&mut self) -> std::result::Result<Vec<u8>, String> {
        let stream = self.stream.as_mut().ok_or("not connected")?;

        // Read 8-byte header
        let mut header = [0u8; 8];
        stream.read_exact(&mut header).await.map_err(|e| format!("recv header: {}", e))?;

        let msg_type = &header[0..3];
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;

        if size < 8 {
            return Err(format!("invalid message size: {}", size));
        }

        let mut rest = vec![0u8; size - 8];
        stream.read_exact(&mut rest).await.map_err(|e| format!("recv body: {}", e))?;

        if msg_type == b"MSG" {
            // Skip: channel_id(4) + token_id(4) + sequence_number(4) + request_id(4) = 16 bytes
            if rest.len() < 16 {
                return Err("MSG response too short".into());
            }
            Ok(rest[16..].to_vec())
        } else if msg_type == b"ERR" {
            // Error message: error code (u32) + reason (string)
            let mut reader = crate::adapters::opcua_codec::BinaryReader::new(&rest);
            let error_code = reader.read_u32().unwrap_or(0);
            let reason = reader.read_string().unwrap_or_default();
            Err(format!("OPC UA error (code 0x{:08x}): {}", error_code, reason))
        } else {
            // For OPN responses, skip the security header
            // For simplicity, return the full body
            Ok(rest)
        }
    }

    fn next_request_handle(&mut self) -> u32 {
        let h = self.request_handle;
        self.request_handle += 1;
        h
    }

    fn next_sequence_number(&mut self) -> u32 {
        let n = self.sequence_number;
        self.sequence_number += 1;
        n
    }
}

/// Parse an OPC UA endpoint URL into (host, port).
fn parse_endpoint(url: &str) -> std::result::Result<(String, u16), String> {
    let url = url.strip_prefix("opc.tcp://")
        .or_else(|| url.strip_prefix("opc.https://"))
        .ok_or_else(|| format!("invalid endpoint URL: {}", url))?;

    let (host_port, _path) = url.split_once('/').unwrap_or((url, ""));
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().map_err(|_| format!("invalid port: {}", p))?),
        None => (host_port.to_string(), 4840),
    };
    Ok((host, port))
}

/// OPC UA protocol adapter.
pub struct OpcUaAdapter {
    client: Arc<Mutex<OpcUaClient>>,
    shared_state: SharedState,
    name: String,
    config: OpcUaConfig,
    /// Cache for read values (updated by subscription callbacks)
    cache: Arc<RwLock<HashMap<String, OpcUaVariant>>>,
    /// Subscription callbacks
    callbacks: Arc<RwLock<Vec<Box<dyn Fn(DataPoint) + Send + Sync>>>>,
}

impl OpcUaAdapter {
    /// Create a new OPC UA adapter.
    pub fn new(name: &str, config: OpcUaConfig) -> Self {
        let client = OpcUaClient::new(config.clone());
        Self {
            client: Arc::new(Mutex::new(client)),
            shared_state: new_shared_state(),
            name: name.to_string(),
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            callbacks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Inject a value into the cache (for testing).
    pub async fn inject_value(&self, node_id: &str, value: OpcUaVariant) {
        self.cache.write().await.insert(node_id.to_string(), value.clone());

        let dp = DataPoint {
            address: node_id.to_string(),
            value: value.to_data_value(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            quality: DataQuality::Good,
        };

        let cbs = self.callbacks.read().await;
        for cb in cbs.iter() {
            cb(dp.clone());
        }
    }

    /// Get a reference to the underlying client.
    pub fn client(&self) -> Arc<Mutex<OpcUaClient>> {
        self.client.clone()
    }
}

#[async_trait]
impl ProtocolAdapter for OpcUaAdapter {
    async fn connect(&mut self, config: &ConnectionConfig) -> Result<()> {
        self.shared_state.set_state(crate::adapter::ConnectionState::Connecting);

        // Build OpcUaConfig from ConnectionConfig
        let endpoint = format!("opc.tcp://{}:{}", config.host, config.port);
        let opc_config = OpcUaConfig {
            endpoint_url: endpoint,
            username: config.credentials.as_ref().map(|c| c.username.clone()),
            password: config.credentials.as_ref().map(|c| c.password.clone()),
            ..self.config.clone()
        };

        let mut client = self.client.lock().await;
        *client = OpcUaClient::new(opc_config);

        client.connect().await.map_err(|e| {
            self.shared_state.mark_disconnected();
            eneros_core::EnerOSError::Device(format!("OPC UA connect failed: {}", e))
        })?;

        self.shared_state.mark_connected();
        tracing::info!("OPC UA adapter '{}' connected", self.name);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.client.lock().await.disconnect().await;
        self.shared_state.mark_disconnected();
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<DataPoint> {
        let node_id = OpcUaNodeId::parse(address).map_err(|e| {
            eneros_core::EnerOSError::Device(format!("invalid OPC UA address: {}", e))
        })?;

        // Check cache first
        if let Some(var) = self.cache.read().await.get(address) {
            return Ok(DataPoint {
                address: address.to_string(),
                value: var.to_data_value(),
                timestamp: chrono::Utc::now().timestamp_millis(),
                quality: DataQuality::Good,
            });
        }

        // Read from server
        let mut client = self.client.lock().await;
        match client.read_value(&node_id).await {
            Ok(var) => {
                self.cache.write().await.insert(address.to_string(), var.clone());
                Ok(DataPoint {
                    address: address.to_string(),
                    value: var.to_data_value(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    quality: DataQuality::Good,
                })
            }
            Err(e) => {
                tracing::warn!("OPC UA read failed for {}: {}", address, e);
                Ok(DataPoint {
                    address: address.to_string(),
                    value: DataValue::Bytes(Vec::new()),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    quality: DataQuality::Bad,
                })
            }
        }
    }

    async fn write(&mut self, address: &str, value: &DataValue) -> Result<()> {
        let node_id = OpcUaNodeId::parse(address).map_err(|e| {
            eneros_core::EnerOSError::Device(format!("invalid OPC UA address: {}", e))
        })?;

        let variant = match value {
            DataValue::Bool(v) => OpcUaVariant::Boolean(*v),
            DataValue::Int16(v) => OpcUaVariant::Int16(*v),
            DataValue::Int32(v) => OpcUaVariant::Int32(*v),
            DataValue::Int64(v) => OpcUaVariant::Int64(*v),
            DataValue::Float32(v) => OpcUaVariant::Float(*v),
            DataValue::Float64(v) => OpcUaVariant::Double(*v),
            DataValue::String(v) => OpcUaVariant::String(v.clone()),
            DataValue::Bytes(v) => OpcUaVariant::ByteString(v.clone()),
        };

        let mut client = self.client.lock().await;
        client.write_value(&node_id, variant).await.map_err(|e| {
            eneros_core::EnerOSError::Device(format!("OPC UA write failed: {}", e))
        })?;

        self.shared_state.record_sent(32);
        Ok(())
    }

    async fn subscribe(
        &mut self,
        addresses: Vec<String>,
        callback: Box<dyn Fn(DataPoint) + Send + Sync>,
    ) -> Result<()> {
        // Register callback
        self.callbacks.write().await.push(callback);

        // Create a subscription on the server (1000ms publishing interval)
        let mut client = self.client.lock().await;
        if client.is_connected() {
            match client.create_subscription(1000.0).await {
                Ok(sub_id) => {
                    tracing::info!(
                        "OPC UA adapter '{}' created subscription {} for {} addresses",
                        self.name, sub_id, addresses.len()
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "OPC UA adapter '{}' subscription creation failed ({}); \
                         callback registered for cache-based updates",
                        self.name, e
                    );
                }
            }
        } else {
            tracing::info!(
                "OPC UA adapter '{}' not connected; callback registered for cache-based updates ({} addresses)",
                self.name,
                addresses.len()
            );
        }
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::OpcUa
    }

    fn is_connected(&self) -> bool {
        self.shared_state.state() == crate::adapter::ConnectionState::Connected
    }

    fn shared_state(&self) -> SharedState {
        self.shared_state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id_parse_string() {
        let id = OpcUaNodeId::parse("ns=2;s=Temperature.Sensor1").unwrap();
        assert_eq!(id.namespace, 2);
        assert_eq!(id.identifier, NodeIdType::String("Temperature.Sensor1".into()));
    }

    #[test]
    fn test_node_id_parse_numeric() {
        let id = OpcUaNodeId::parse("ns=0;i=85").unwrap();
        assert_eq!(id.namespace, 0);
        assert_eq!(id.identifier, NodeIdType::Numeric(85));
    }

    #[test]
    fn test_node_id_parse_default_ns() {
        let id = OpcUaNodeId::parse("i=2258").unwrap();
        assert_eq!(id.namespace, 0);
        assert_eq!(id.identifier, NodeIdType::Numeric(2258));
    }

    #[test]
    fn test_node_id_parse_guid() {
        let id = OpcUaNodeId::parse("ns=1;g=12345678-1234-1234-1234-123456789abc").unwrap();
        assert_eq!(id.namespace, 1);
        assert!(matches!(id.identifier, NodeIdType::Guid(_)));
    }

    #[test]
    fn test_node_id_parse_invalid() {
        assert!(OpcUaNodeId::parse("invalid").is_err());
        assert!(OpcUaNodeId::parse("ns=abc;s=test").is_err());
        assert!(OpcUaNodeId::parse("ns=1").is_err()); // no identifier
    }

    #[test]
    fn test_node_id_to_string() {
        let id = OpcUaNodeId {
            namespace: 2,
            identifier: NodeIdType::String("Test".into()),
        };
        assert_eq!(id.to_string(), "ns=2;s=Test");

        let id2 = OpcUaNodeId {
            namespace: 0,
            identifier: NodeIdType::Numeric(85),
        };
        assert_eq!(id2.to_string(), "ns=0;i=85");
    }

    #[test]
    fn test_variant_to_data_value() {
        assert_eq!(OpcUaVariant::Boolean(true).to_data_value(), DataValue::Bool(true));
        assert_eq!(OpcUaVariant::Int32(42).to_data_value(), DataValue::Int32(42));
        assert_eq!(OpcUaVariant::Double(1.5).to_data_value(), DataValue::Float64(1.5));
        assert_eq!(
            OpcUaVariant::String("hello".into()).to_data_value(),
            DataValue::String("hello".into())
        );
    }

    #[test]
    fn test_variant_type_id() {
        assert_eq!(OpcUaVariant::Null.type_id(), 0);
        assert_eq!(OpcUaVariant::Boolean(false).type_id(), 1);
        assert_eq!(OpcUaVariant::Int32(0).type_id(), 6);
        assert_eq!(OpcUaVariant::Double(0.0).type_id(), 11);
        assert_eq!(OpcUaVariant::String(String::new()).type_id(), 12);
    }

    #[test]
    fn test_parse_endpoint() {
        let (h, p) = parse_endpoint("opc.tcp://192.168.1.1:4840").unwrap();
        assert_eq!(h, "192.168.1.1");
        assert_eq!(p, 4840);

        let (h, p) = parse_endpoint("opc.tcp://localhost").unwrap();
        assert_eq!(h, "localhost");
        assert_eq!(p, 4840);

        let (h, p) = parse_endpoint("opc.tcp://host:9999/path").unwrap();
        assert_eq!(h, "host");
        assert_eq!(p, 9999);

        assert!(parse_endpoint("http://localhost").is_err());
    }

    #[test]
    fn test_node_class_from_u32() {
        assert_eq!(NodeClass::from_u32(1), NodeClass::Object);
        assert_eq!(NodeClass::from_u32(2), NodeClass::Variable);
        assert_eq!(NodeClass::from_u32(4), NodeClass::Method);
        assert_eq!(NodeClass::from_u32(999), NodeClass::Unknown);
    }

    #[test]
    fn test_opcua_config_default() {
        let config = OpcUaConfig::default();
        assert_eq!(config.endpoint_url, "opc.tcp://localhost:4840");
        assert_eq!(config.security_policy, "None");
        assert!(config.username.is_none());
    }

    #[test]
    fn test_client_creation() {
        let client = OpcUaClient::new(OpcUaConfig::default());
        assert!(!client.is_connected());
        assert_eq!(client.sequence_number, 1);
        assert_eq!(client.request_handle, 1);
    }

    #[tokio::test]
    async fn test_adapter_creation() {
        let adapter = OpcUaAdapter::new("test-opcua", OpcUaConfig::default());
        assert_eq!(adapter.name(), "test-opcua");
        assert_eq!(adapter.protocol_type(), ProtocolType::OpcUa);
        assert!(!adapter.is_connected());
    }

    #[tokio::test]
    async fn test_adapter_read_cached() {
        let mut adapter = OpcUaAdapter::new("test", OpcUaConfig::default());
        adapter.shared_state.mark_connected();

        adapter
            .inject_value("ns=2;s=Temp", OpcUaVariant::Double(25.5))
            .await;

        let dp = adapter.read("ns=2;s=Temp").await.unwrap();
        assert_eq!(dp.value, DataValue::Float64(25.5));
        assert_eq!(dp.quality, DataQuality::Good);
    }

    #[tokio::test]
    async fn test_adapter_read_not_connected() {
        let adapter = OpcUaAdapter::new("test", OpcUaConfig::default());
        // Not connected — read returns Bad quality
        let dp = adapter.read("ns=2;s=Temp").await.unwrap();
        assert_eq!(dp.quality, DataQuality::Bad);
    }

    #[tokio::test]
    async fn test_adapter_subscribe_callback() {
        let mut adapter = OpcUaAdapter::new("test", OpcUaConfig::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_clone = received.clone();

        adapter
            .subscribe(vec!["ns=2;s=Temp".into()], Box::new(move |dp| {
                received_clone.try_lock().unwrap().push(dp);
            }))
            .await
            .unwrap();

        adapter
            .inject_value("ns=2;s=Temp", OpcUaVariant::Double(25.5))
            .await;

        let msgs = received.lock().await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].value, DataValue::Float64(25.5));
    }

    #[tokio::test]
    async fn test_adapter_write_unsupported_type() {
        let mut adapter = OpcUaAdapter::new("test", OpcUaConfig::default());
        adapter.shared_state.mark_connected();

        // Int16 is supported
        let result = adapter.write("ns=2;s=Test", &DataValue::Int16(42)).await;
        // Will fail because not connected to real server, but should parse address
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_adapter_write_invalid_address() {
        let mut adapter = OpcUaAdapter::new("test", OpcUaConfig::default());
        adapter.shared_state.mark_connected();

        let result = adapter.write("invalid", &DataValue::Int16(42)).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_browse_result() {
        let result = BrowseResult {
            node_id: OpcUaNodeId::parse("ns=2;s=Sensor1").unwrap(),
            browse_name: "Sensor1".to_string(),
            display_name: "Temperature Sensor 1".to_string(),
            node_class: NodeClass::Variable,
        };
        assert_eq!(result.node_class, NodeClass::Variable);
        assert_eq!(result.display_name, "Temperature Sensor 1");
    }

    #[test]
    fn test_array_variant_to_data_value() {
        let arr = OpcUaVariant::Array(vec![
            OpcUaVariant::Double(1.0),
            OpcUaVariant::Double(2.0),
        ]);
        assert_eq!(arr.to_data_value(), DataValue::Float64(1.0));

        let empty = OpcUaVariant::Array(vec![]);
        assert_eq!(empty.to_data_value(), DataValue::Bytes(Vec::new()));
    }
}
