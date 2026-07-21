//! v0.98.0 跨域通信通道：Edge Coordinator 间 mTLS 双向认证 + 国密 SM3/SM4 加密通话。
//!
//! ## 设计要点
//!
//! - **确定性握手**（D6）：hello 帧 `MAGIC[4]‖cert_len:u32be‖client_cert‖nonce[32]`
//!   经 [`SecureTransport`] 发出 → 对端应答帧 `MAGIC‖cert_len‖cert` → 复用 v0.97.0
//!   [`CertVerifier`](crate::discovery::CertVerifier) 验证对端证书 → 双方独立派生
//!   会话密钥 `SM3("eneros-ch-enc"‖init_cert‖resp_cert‖nonce)[..16]`（同序拼接可复算，
//!   [`derive_session_key`] / [`handle_hello`] 公开辅助供应答方与回环双端测试）。
//! - **加密通话**（D7）：SM4-GCM AEAD（eneros-crypto 既有 `Sm4Gcm`），帧
//!   `seq:u64be‖ciphertext‖tag[16]`；nonce = `0u32‖seq_be`（12 字节，逐 seq 唯一，
//!   GCM 安全）；aad = `node_id_be‖seq_be`。
//! - **可观测**（D12）：4 个 pub 计数器 `connect_count` / `call_count` /
//!   `handshake_fail_count` / `crypto_fail_count`。
//! - **依赖注入**：`Box<dyn SecureTransport>` / `Box<dyn CertVerifier>`，真实 gRPC/TLS
//!   栈由集成阶段 Agent Runtime 适配层注入（接口先行，同 v0.97.0 D5/D6）。
//!
//! ## D1~D12 偏差表（简版，相对蓝图 v0.98.0 原文）
//!
//! | 编号 | 偏差 |
//! |------|------|
//! | D1 | 既有 crate 单模块 `channel.rs`（tls/grpc_service 语义并入：TlsConfig 纯数据 + SecureTransport 服务抽象，不过度拆分） |
//! | D2 | `node_id: u64` / `connect(u64, SocketAddr)`，无堆字符串标识（v0.97.0 D2 惯例） |
//! | D3 | sync 方法 + sync `SecureTransport`（no_std 硬规则禁 async；tonic 依赖 std/tokio 无法交叉编译 aarch64-unknown-none） |
//! | D4 | `TlsConfig` 纯数据 + `validate()` 非空校验（PEM 解析/真实 TLS 握手后置集成） |
//! | D5 | 复用 v0.97.0 `CertVerifier` 验证对端证书（§5.5 防重复造轮子） |
//! | D6 | 确定性握手语义 + SM3 域分离会话密钥派生（双方同序拼接可独立复算） |
//! | D7 | SM4-GCM 认证加密替代 TLS record 层；nonce 逐 seq 唯一 |
//! | D8 | `use_sm` 纯配置占位：本版本仅国密路径，无分支行为差异 |
//! | D9 | 零新增第三方依赖，仅 path 依赖 eneros-crypto（SBOM 不变） |
//! | D10 | `ChannelError` 6 变体最小完备（握手/证书/拒绝/未知节点/加解密/传输） |
//! | D11 | crate 内嵌 `#[cfg(test)]` ~40 测试（Mock 故障注入覆盖握手失败/证书拒绝/篡改） |
//! | D12 | 4 个 pub 计数器替代外部连接状态 metric |

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::net::SocketAddr;

use eneros_crypto::rng::CsRng;
use eneros_crypto::sm3::Sm3Hasher;
use eneros_crypto::sm4::gcm::Sm4Gcm;

use crate::discovery::CertVerifier;

/// 握手帧魔数 "ECH0"（EnerOS Cross-domain Handshake v0）
const MAGIC: [u8; 4] = *b"ECH0";
/// SM4-GCM 认证标签长度（字节）
const TAG_LEN: usize = 16;
/// 握手 nonce 长度（字节）
const NONCE_LEN: usize = 32;

/// 跨域通道错误（6 变体最小完备，D10）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// 握手失败（应答帧缺失或格式错误）
    HandshakeFailed,
    /// 证书无效（本地 TLS 配置为空，或对端证书被 CertVerifier 拒绝）
    CertInvalid,
    /// 对端拒绝连接（握手/通话发送失败）
    ConnectionRefused,
    /// 未知节点（endpoint 不存在）
    UnknownNode,
    /// 加解密失败（应答帧过短或 GCM 标签校验不通过）
    CryptoFailed,
    /// 传输失败（接收失败）
    TransportFailed,
}

/// TLS 配置（纯数据，D4；PEM 解析与真实 TLS 握手后置集成）
#[derive(Debug, Clone, PartialEq)]
pub struct TlsConfig {
    /// CA 根证书字节
    pub ca_cert: Vec<u8>,
    /// 本端客户端证书字节
    pub client_cert: Vec<u8>,
    /// 本端客户端私钥字节
    pub client_key: Vec<u8>,
    /// 国密开关（D8：纯配置占位，本版本仅国密路径，无分支行为差异）
    pub use_sm: bool,
}

impl TlsConfig {
    /// 非空校验：ca_cert / client_cert / client_key 任一空 → `Err(CertInvalid)`
    pub fn validate(&self) -> Result<(), ChannelError> {
        if self.ca_cert.is_empty() || self.client_cert.is_empty() || self.client_key.is_empty() {
            return Err(ChannelError::CertInvalid);
        }
        Ok(())
    }
}

/// 已建立的跨域端点
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    /// 对端节点 id
    pub node_id: u64,
    /// 对端网络地址
    pub addr: SocketAddr,
    /// 握手是否已建立
    pub established: bool,
    /// 会话密钥（SM3 域分离派生，16 字节）
    pub session_key: [u8; 16],
    /// 发送序号（每 call 递增，GCM nonce/aad 组成部分）
    pub send_seq: u64,
}

/// 安全传输抽象（sync，无 Send+Sync，D3）
pub trait SecureTransport {
    /// 向指定节点发送数据
    fn send(&mut self, node_id: u64, data: &[u8]) -> Result<(), ChannelError>;
    /// 从指定节点接收数据
    fn recv(&mut self, node_id: u64) -> Result<Vec<u8>, ChannelError>;
}

/// Mock 安全传输（故障注入，D11）
#[derive(Debug, Clone, Default)]
pub struct MockSecureTransport {
    /// 已成功发送的记录 `(node_id, data)`
    pub sent: Vec<(u64, Vec<u8>)>,
    /// 按节点分桶的待接收队列（每次 recv 弹队首）
    pub inbox: BTreeMap<u64, Vec<Vec<u8>>>,
    /// 剩余 send 应失败次数（>0 → `Err(ConnectionRefused)` 并递减）
    pub fail_send_times: u32,
    /// 剩余 recv 应失败次数（>0 → `Err(TransportFailed)` 并递减）
    pub fail_recv_times: u32,
}

impl MockSecureTransport {
    /// 创建空 Mock 传输（无故障注入）
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecureTransport for MockSecureTransport {
    fn send(&mut self, node_id: u64, data: &[u8]) -> Result<(), ChannelError> {
        if self.fail_send_times > 0 {
            self.fail_send_times -= 1;
            return Err(ChannelError::ConnectionRefused);
        }
        self.sent.push((node_id, data.to_vec()));
        Ok(())
    }

    fn recv(&mut self, node_id: u64) -> Result<Vec<u8>, ChannelError> {
        if self.fail_recv_times > 0 {
            self.fail_recv_times -= 1;
            return Err(ChannelError::TransportFailed);
        }
        match self.inbox.get_mut(&node_id) {
            Some(queue) if !queue.is_empty() => Ok(queue.remove(0)),
            _ => Err(ChannelError::TransportFailed),
        }
    }
}

/// 解析握手应答帧 `MAGIC‖cert_len:u32be‖cert`，返回对端证书
fn parse_reply_frame(frame: &[u8]) -> Result<Vec<u8>, ChannelError> {
    if frame.len() < 8 || !frame.starts_with(&MAGIC) {
        return Err(ChannelError::HandshakeFailed);
    }
    let cert_len = u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
    if frame.len() != 8 + cert_len {
        return Err(ChannelError::HandshakeFailed);
    }
    Ok(frame[8..].to_vec())
}

/// SM3 域分离会话密钥派生（D6）：`SM3("eneros-ch-enc"‖init_cert‖resp_cert‖nonce)[..16]`
///
/// 发起方与应答方按同序拼接可独立复算出同一会话密钥。
pub fn derive_session_key(init_cert: &[u8], resp_cert: &[u8], nonce: &[u8; 32]) -> [u8; 16] {
    let mut hasher = Sm3Hasher::new();
    hasher.update(b"eneros-ch-enc");
    hasher.update(init_cert);
    hasher.update(resp_cert);
    hasher.update(nonce);
    let digest = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

/// 应答方辅助：解析 hello 帧 `MAGIC‖cert_len‖cert‖nonce[32]`，
/// 返回 `(对端证书, nonce, 应答帧 MAGIC‖cert_len‖own_cert)`；帧格式错 → `Err(HandshakeFailed)`
// 签名与 spec 保持字面一致，不引入类型别名
#[allow(clippy::type_complexity)]
pub fn handle_hello(
    hello: &[u8],
    own_cert: &[u8],
) -> Result<(Vec<u8>, [u8; 32], Vec<u8>), ChannelError> {
    if hello.len() < 8 + NONCE_LEN || !hello.starts_with(&MAGIC) {
        return Err(ChannelError::HandshakeFailed);
    }
    let cert_len = u32::from_be_bytes([hello[4], hello[5], hello[6], hello[7]]) as usize;
    if hello.len() != 8 + cert_len + NONCE_LEN {
        return Err(ChannelError::HandshakeFailed);
    }
    let peer_cert = hello[8..8 + cert_len].to_vec();
    let mut nonce = [0u8; NONCE_LEN];
    nonce.copy_from_slice(&hello[8 + cert_len..]);
    let mut reply = Vec::with_capacity(8 + own_cert.len());
    reply.extend_from_slice(&MAGIC);
    reply.extend_from_slice(&(own_cert.len() as u32).to_be_bytes());
    reply.extend_from_slice(own_cert);
    Ok((peer_cert, nonce, reply))
}

/// 构造 GCM nonce：`0u32‖seq_be`（12 字节，逐 seq 唯一，D7）
fn gcm_nonce(seq: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&seq.to_be_bytes());
    nonce
}

/// 构造 GCM aad：`node_id_be‖seq_be`（16 字节，D7）
fn gcm_aad(node_id: u64, seq: u64) -> [u8; 16] {
    let mut aad = [0u8; 16];
    aad[..8].copy_from_slice(&node_id.to_be_bytes());
    aad[8..].copy_from_slice(&seq.to_be_bytes());
    aad
}

/// 跨域通信通道（v0.98.0）：mTLS 双向认证握手 + SM4-GCM 加密通话 + 4 计数器
pub struct FederationChannel {
    /// TLS 配置（纯数据）
    pub tls: TlsConfig,
    /// 证书验证器（复用 v0.97.0 CertVerifier，D5）
    pub verifier: Box<dyn CertVerifier>,
    /// 安全传输层（依赖注入，D3）
    pub transport: Box<dyn SecureTransport>,
    /// 随机数生成器（握手 nonce 来源；测试用 `CsRng::new()` 确定性）
    pub rng: CsRng,
    /// 已建立端点列表
    pub endpoints: Vec<Endpoint>,
    /// 成功握手计数
    pub connect_count: u64,
    /// 成功加密通话计数
    pub call_count: u64,
    /// 握手失败计数（发送失败/接收失败/帧格式错/证书拒绝）
    pub handshake_fail_count: u64,
    /// 加解密失败计数（应答帧过短/GCM 标签校验失败）
    pub crypto_fail_count: u64,
}

impl FederationChannel {
    /// 创建通道：endpoints 为空，4 计数器全零
    pub fn new(
        tls: TlsConfig,
        verifier: Box<dyn CertVerifier>,
        transport: Box<dyn SecureTransport>,
        rng: CsRng,
    ) -> Self {
        Self {
            tls,
            verifier,
            transport,
            rng,
            endpoints: Vec::new(),
            connect_count: 0,
            call_count: 0,
            handshake_fail_count: 0,
            crypto_fail_count: 0,
        }
    }

    /// 建立到指定节点的跨域连接（确定性握手，D6）
    ///
    /// 流程：tls.validate() → rng nonce[32] → hello 帧发送 → 接收应答帧 →
    /// 解析对端证书 → CertVerifier 验证 → 派生会话密钥 → push Endpoint。
    pub fn connect(&mut self, node_id: u64, addr: SocketAddr) -> Result<(), ChannelError> {
        // validate 失败直接返回，不计数
        self.tls.validate()?;

        let mut nonce = [0u8; NONCE_LEN];
        self.rng.fill_bytes(&mut nonce);

        // hello 帧：MAGIC‖cert_len:u32be‖client_cert‖nonce[32]
        let mut hello = Vec::with_capacity(8 + self.tls.client_cert.len() + NONCE_LEN);
        hello.extend_from_slice(&MAGIC);
        hello.extend_from_slice(&(self.tls.client_cert.len() as u32).to_be_bytes());
        hello.extend_from_slice(&self.tls.client_cert);
        hello.extend_from_slice(&nonce);

        if self.transport.send(node_id, &hello).is_err() {
            // 忽略底层错误细节，统一为 ConnectionRefused
            self.handshake_fail_count += 1;
            return Err(ChannelError::ConnectionRefused);
        }

        let reply = match self.transport.recv(node_id) {
            Ok(r) => r,
            Err(_) => {
                self.handshake_fail_count += 1;
                return Err(ChannelError::HandshakeFailed);
            }
        };
        let peer_cert = match parse_reply_frame(&reply) {
            Ok(c) => c,
            Err(e) => {
                self.handshake_fail_count += 1;
                return Err(e);
            }
        };

        if self.verifier.verify(&peer_cert).is_err() {
            self.handshake_fail_count += 1;
            return Err(ChannelError::CertInvalid);
        }

        let session_key = derive_session_key(&self.tls.client_cert, &peer_cert, &nonce);
        self.endpoints.push(Endpoint {
            node_id,
            addr,
            established: true,
            session_key,
            send_seq: 0,
        });
        self.connect_count += 1;
        Ok(())
    }

    /// 加密通话：SM4-GCM 加密请求 → 发送 → 接收应答帧 → 解密（D7）
    pub fn call(&mut self, node_id: u64, plaintext: &[u8]) -> Result<Vec<u8>, ChannelError> {
        let idx = match self.endpoints.iter().position(|e| e.node_id == node_id) {
            Some(i) => i,
            None => return Err(ChannelError::UnknownNode),
        };
        if !self.endpoints[idx].established {
            return Err(ChannelError::ConnectionRefused);
        }
        self.endpoints[idx].send_seq += 1;
        let seq = self.endpoints[idx].send_seq;
        let session_key = self.endpoints[idx].session_key;

        let (ct, tag) =
            Sm4Gcm::new(&session_key).encrypt(&gcm_nonce(seq), plaintext, &gcm_aad(node_id, seq));

        // 请求帧：seq:u64be‖ct‖tag
        let mut frame = Vec::with_capacity(8 + ct.len() + TAG_LEN);
        frame.extend_from_slice(&seq.to_be_bytes());
        frame.extend_from_slice(&ct);
        frame.extend_from_slice(&tag);

        if self.transport.send(node_id, &frame).is_err() {
            return Err(ChannelError::ConnectionRefused);
        }
        let reply = match self.transport.recv(node_id) {
            Ok(r) => r,
            Err(_) => return Err(ChannelError::TransportFailed),
        };

        // 应答帧：seq:u64be‖ct‖tag（至少 8+16 字节）
        if reply.len() < 8 + TAG_LEN {
            self.crypto_fail_count += 1;
            return Err(ChannelError::CryptoFailed);
        }
        let rseq = u64::from_be_bytes([
            reply[0], reply[1], reply[2], reply[3], reply[4], reply[5], reply[6], reply[7],
        ]);
        let rct = &reply[8..reply.len() - TAG_LEN];
        let mut rtag = [0u8; TAG_LEN];
        rtag.copy_from_slice(&reply[reply.len() - TAG_LEN..]);

        // aad/nonce 按应答帧中的 seq 同规则重建
        match Sm4Gcm::new(&session_key).decrypt(
            &gcm_nonce(rseq),
            rct,
            &gcm_aad(node_id, rseq),
            &rtag,
        ) {
            Ok(pt) => {
                self.call_count += 1;
                Ok(pt)
            }
            Err(_) => {
                self.crypto_fail_count += 1;
                Err(ChannelError::CryptoFailed)
            }
        }
    }

    /// 断开连接：找到则移除并返回 true，否则 false
    pub fn disconnect(&mut self, node_id: u64) -> bool {
        match self.endpoints.iter().position(|e| e.node_id == node_id) {
            Some(idx) => {
                self.endpoints.remove(idx);
                true
            }
            None => false,
        }
    }

    /// 断连重连（蓝图 §9 可靠）：按已存 addr 移除旧端点后重新握手
    pub fn reconnect(&mut self, node_id: u64) -> Result<(), ChannelError> {
        let idx = match self.endpoints.iter().position(|e| e.node_id == node_id) {
            Some(i) => i,
            None => return Err(ChannelError::UnknownNode),
        };
        let addr = self.endpoints[idx].addr;
        self.endpoints.remove(idx);
        self.connect(node_id, addr)
    }
}

#[cfg(test)]
mod tests {
    use core::net::{IpAddr, Ipv4Addr};

    use eneros_crypto::rng::CsRng;
    use eneros_crypto::sm4::gcm::Sm4Gcm;

    use super::*;
    use crate::discovery::MockCertVerifier;

    // ------------------------------------------------------------
    // 测试辅助
    // ------------------------------------------------------------

    const CA_CERT: &[u8] = b"eneros-test-ca-cert";
    const CLIENT_CERT: &[u8] = b"eneros-test-client-cert";
    const CLIENT_KEY: &[u8] = b"eneros-test-client-key";
    const PEER_CERT: &[u8] = b"eneros-test-peer-cert";

    fn make_tls() -> TlsConfig {
        TlsConfig {
            ca_cert: CA_CERT.to_vec(),
            client_cert: CLIENT_CERT.to_vec(),
            client_key: CLIENT_KEY.to_vec(),
            use_sm: true,
        }
    }

    fn make_reply_frame(cert: &[u8]) -> Vec<u8> {
        let mut f = Vec::with_capacity(8 + cert.len());
        f.extend_from_slice(&MAGIC);
        f.extend_from_slice(&(cert.len() as u32).to_be_bytes());
        f.extend_from_slice(cert);
        f
    }

    /// 用指定会话密钥构造 call 应答帧 `seq‖ct‖tag`（GCM 加密，aad/nonce 同规则）
    fn make_call_reply(
        session_key: &[u8; 16],
        node_id: u64,
        seq: u64,
        plaintext: &[u8],
    ) -> Vec<u8> {
        let (ct, tag) =
            Sm4Gcm::new(session_key).encrypt(&gcm_nonce(seq), plaintext, &gcm_aad(node_id, seq));
        let mut frame = Vec::with_capacity(8 + ct.len() + TAG_LEN);
        frame.extend_from_slice(&seq.to_be_bytes());
        frame.extend_from_slice(&ct);
        frame.extend_from_slice(&tag);
        frame
    }

    fn test_addr(n: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, n)), 8443)
    }

    fn make_channel(accept: bool, transport: MockSecureTransport) -> FederationChannel {
        FederationChannel::new(
            make_tls(),
            Box::new(MockCertVerifier::new(accept)),
            Box::new(transport),
            CsRng::new(),
        )
    }

    /// 预算 CsRng::new() 第 1、2 次 fill_bytes(32) 输出（确定性）
    fn preset_nonces() -> ([u8; 32], [u8; 32]) {
        let mut rng = CsRng::new();
        let mut n1 = [0u8; 32];
        rng.fill_bytes(&mut n1);
        let mut n2 = [0u8; 32];
        rng.fill_bytes(&mut n2);
        (n1, n2)
    }

    /// 手工构造已建立 endpoint（绕过 connect，供 call 系列测试）
    fn channel_with_endpoint(transport: MockSecureTransport, key: [u8; 16]) -> FederationChannel {
        let mut ch = make_channel(true, transport);
        ch.endpoints.push(Endpoint {
            node_id: 2,
            addr: test_addr(2),
            established: true,
            session_key: key,
            send_seq: 0,
        });
        ch
    }

    // ------------------------------------------------------------
    // TC1~TC6：ChannelError / TlsConfig
    // ------------------------------------------------------------

    // TC1: ChannelError 派生（Debug/Clone/Copy/PartialEq/Eq），6 变体互不等
    #[test]
    fn tc01_channel_error_derive() {
        let errs = [
            ChannelError::HandshakeFailed,
            ChannelError::CertInvalid,
            ChannelError::ConnectionRefused,
            ChannelError::UnknownNode,
            ChannelError::CryptoFailed,
            ChannelError::TransportFailed,
        ];
        for (i, a) in errs.iter().enumerate() {
            for (j, b) in errs.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b);
                }
            }
        }
        let e = ChannelError::CryptoFailed;
        let e2 = e; // Copy 语义
        assert_eq!(e, e2);
        assert_eq!(format!("{:?}", e), "CryptoFailed"); // Debug
    }

    // TC2: TlsConfig 派生（Debug/Clone/PartialEq）与字段读写
    #[test]
    fn tc02_tls_config_derive_and_fields() {
        let tls = make_tls();
        let tls2 = tls.clone();
        assert_eq!(tls, tls2);
        assert_eq!(tls.ca_cert, CA_CERT);
        assert_eq!(tls.client_cert, CLIENT_CERT);
        assert_eq!(tls.client_key, CLIENT_KEY);
        assert!(tls.use_sm);
        let mut tls3 = make_tls();
        tls3.use_sm = false;
        assert_ne!(tls, tls3);
        let _ = format!("{:?}", tls); // Debug 可用
    }

    // TC3: validate 空 ca_cert → Err(CertInvalid)
    #[test]
    fn tc03_tls_validate_empty_ca() {
        let mut tls = make_tls();
        tls.ca_cert = Vec::new();
        assert_eq!(tls.validate(), Err(ChannelError::CertInvalid));
    }

    // TC4: validate 空 client_cert → Err(CertInvalid)
    #[test]
    fn tc04_tls_validate_empty_cert() {
        let mut tls = make_tls();
        tls.client_cert = Vec::new();
        assert_eq!(tls.validate(), Err(ChannelError::CertInvalid));
    }

    // TC5: validate 空 client_key → Err(CertInvalid)
    #[test]
    fn tc05_tls_validate_empty_key() {
        let mut tls = make_tls();
        tls.client_key = Vec::new();
        assert_eq!(tls.validate(), Err(ChannelError::CertInvalid));
    }

    // TC6: validate 全非空 → Ok
    #[test]
    fn tc06_tls_validate_ok() {
        assert_eq!(make_tls().validate(), Ok(()));
    }

    // ------------------------------------------------------------
    // TC7~TC12：MockSecureTransport
    // ------------------------------------------------------------

    // TC7: send 成功 → Ok 且入 sent
    #[test]
    fn tc07_mock_send_success() {
        let mut t = MockSecureTransport::new();
        assert_eq!(t.send(2, b"hello"), Ok(()));
        assert_eq!(t.sent.len(), 1);
        assert_eq!(t.sent[0], (2, b"hello".to_vec()));
    }

    // TC8: send 故障注入 fail_send_times=2 → 2 次 Err(ConnectionRefused) 递减后成功入 sent
    #[test]
    fn tc08_mock_send_fail_injection() {
        let mut t = MockSecureTransport {
            fail_send_times: 2,
            ..MockSecureTransport::new()
        };
        assert_eq!(t.send(2, b"a"), Err(ChannelError::ConnectionRefused));
        assert_eq!(t.fail_send_times, 1);
        assert_eq!(t.send(2, b"b"), Err(ChannelError::ConnectionRefused));
        assert_eq!(t.fail_send_times, 0);
        assert!(t.sent.is_empty()); // 失败期间不记录
        assert_eq!(t.send(2, b"c"), Ok(()));
        assert_eq!(t.sent.len(), 1);
        assert_eq!(t.sent[0], (2, b"c".to_vec()));
    }

    // TC9: recv 成功 → 按队列顺序弹队首
    #[test]
    fn tc09_mock_recv_success_fifo() {
        let mut t = MockSecureTransport::new();
        t.inbox
            .insert(2, vec![b"first".to_vec(), b"second".to_vec()]);
        assert_eq!(t.recv(2), Ok(b"first".to_vec()));
        assert_eq!(t.recv(2), Ok(b"second".to_vec()));
    }

    // TC10: recv 故障注入 fail_recv_times=1 → 先 Err(TransportFailed) 递减后成功
    #[test]
    fn tc10_mock_recv_fail_injection() {
        let mut t = MockSecureTransport {
            fail_recv_times: 1,
            ..MockSecureTransport::new()
        };
        t.inbox.insert(2, vec![b"data".to_vec()]);
        assert_eq!(t.recv(2), Err(ChannelError::TransportFailed));
        assert_eq!(t.fail_recv_times, 0);
        assert_eq!(t.recv(2), Ok(b"data".to_vec()));
    }

    // TC11: recv 空 inbox → Err(TransportFailed)
    #[test]
    fn tc11_mock_recv_empty_inbox() {
        let mut t = MockSecureTransport::new();
        assert_eq!(t.recv(2), Err(ChannelError::TransportFailed));
        t.inbox.insert(2, Vec::new()); // 队列存在但为空
        assert_eq!(t.recv(2), Err(ChannelError::TransportFailed));
    }

    // TC12: recv 按 node_id 路由，各节点队列互不影响
    #[test]
    fn tc12_mock_recv_routing_by_node() {
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![b"for-2".to_vec()]);
        t.inbox.insert(3, vec![b"for-3".to_vec()]);
        assert_eq!(t.recv(3), Ok(b"for-3".to_vec()));
        assert_eq!(t.recv(2), Ok(b"for-2".to_vec()));
        assert_eq!(t.recv(2), Err(ChannelError::TransportFailed)); // 已弹空
        assert_eq!(t.recv(99), Err(ChannelError::TransportFailed)); // 未知节点
    }

    // ------------------------------------------------------------
    // TC13~TC24：connect 握手
    // ------------------------------------------------------------

    // TC13: connect 成功路径 → connect_count==1、established、send_seq==0、字段正确
    #[test]
    fn tc13_connect_success() {
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![make_reply_frame(PEER_CERT)]);
        let mut ch = make_channel(true, t);
        assert_eq!(ch.connect(2, test_addr(2)), Ok(()));
        assert_eq!(ch.connect_count, 1);
        assert_eq!(ch.endpoints.len(), 1);
        let ep = &ch.endpoints[0];
        assert_eq!(ep.node_id, 2);
        assert_eq!(ep.addr, test_addr(2));
        assert!(ep.established);
        assert_eq!(ep.send_seq, 0);
        assert_eq!(ch.handshake_fail_count, 0);
    }

    // TC14: tls validate 失败 → Err(CertInvalid)，计数器全零、endpoints 空、未发送
    #[test]
    fn tc14_connect_validate_fail_not_counted() {
        let mut ch = make_channel(true, MockSecureTransport::new());
        ch.tls.ca_cert = Vec::new();
        assert_eq!(ch.connect(2, test_addr(2)), Err(ChannelError::CertInvalid));
        assert_eq!(ch.connect_count, 0);
        assert_eq!(ch.call_count, 0);
        assert_eq!(ch.handshake_fail_count, 0);
        assert_eq!(ch.crypto_fail_count, 0);
        assert!(ch.endpoints.is_empty());
    }

    // TC15: connect send 失败 → Err(ConnectionRefused) + handshake_fail_count==1
    #[test]
    fn tc15_connect_send_fail() {
        let t = MockSecureTransport {
            fail_send_times: 1,
            ..MockSecureTransport::new()
        };
        let mut ch = make_channel(true, t);
        assert_eq!(
            ch.connect(2, test_addr(2)),
            Err(ChannelError::ConnectionRefused)
        );
        assert_eq!(ch.handshake_fail_count, 1);
        assert_eq!(ch.connect_count, 0);
        assert!(ch.endpoints.is_empty());
    }

    // TC16: connect recv 失败（空 inbox）→ Err(HandshakeFailed) + handshake_fail_count==1
    #[test]
    fn tc16_connect_recv_fail() {
        let mut ch = make_channel(true, MockSecureTransport::new());
        assert_eq!(
            ch.connect(2, test_addr(2)),
            Err(ChannelError::HandshakeFailed)
        );
        assert_eq!(ch.handshake_fail_count, 1);
        assert_eq!(ch.connect_count, 0);
        assert!(ch.endpoints.is_empty());
    }

    // TC17: 应答帧格式错（坏 MAGIC / 长度不符）→ Err(HandshakeFailed) + handshake_fail_count==1
    #[test]
    fn tc17_connect_bad_reply_frame() {
        // 坏 MAGIC
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![b"BAD!not-a-frame".to_vec()]);
        let mut ch = make_channel(true, t);
        assert_eq!(
            ch.connect(2, test_addr(2)),
            Err(ChannelError::HandshakeFailed)
        );
        assert_eq!(ch.handshake_fail_count, 1);
        // cert_len 与实际长度不符
        let mut bad = make_reply_frame(PEER_CERT);
        bad.truncate(bad.len() - 2);
        let mut t2 = MockSecureTransport::new();
        t2.inbox.insert(3, vec![bad]);
        let mut ch2 = make_channel(true, t2);
        assert_eq!(
            ch2.connect(3, test_addr(3)),
            Err(ChannelError::HandshakeFailed)
        );
        assert_eq!(ch2.handshake_fail_count, 1);
    }

    // TC18: verifier reject → Err(CertInvalid) + handshake_fail_count==1 + endpoints 空
    #[test]
    fn tc18_connect_verifier_reject() {
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![make_reply_frame(PEER_CERT)]);
        let mut ch = make_channel(false, t);
        assert_eq!(ch.connect(2, test_addr(2)), Err(ChannelError::CertInvalid));
        assert_eq!(ch.handshake_fail_count, 1);
        assert_eq!(ch.connect_count, 0);
        assert!(ch.endpoints.is_empty());
    }

    // TC19: 会话密钥 == derive_session_key(client_cert, peer_cert, nonce) 复算（确定性 nonce）
    #[test]
    fn tc19_connect_session_key_matches_derive() {
        let (nonce, _) = preset_nonces();
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![make_reply_frame(PEER_CERT)]);
        let mut ch = make_channel(true, t);
        ch.connect(2, test_addr(2)).unwrap();
        let expected = derive_session_key(CLIENT_CERT, PEER_CERT, &nonce);
        assert_eq!(ch.endpoints[0].session_key, expected);
    }

    // TC20: handle_hello 应答方复算同密钥（双向同密钥）
    #[test]
    fn tc20_handle_hello_responder_same_key() {
        let (nonce, _) = preset_nonces();
        // 重建发起方 hello 帧（与 connect 发出格式一致）
        let mut hello = Vec::new();
        hello.extend_from_slice(&MAGIC);
        hello.extend_from_slice(&(CLIENT_CERT.len() as u32).to_be_bytes());
        hello.extend_from_slice(CLIENT_CERT);
        hello.extend_from_slice(&nonce);
        // 应答方处理 hello
        let (peer_cert, got_nonce, reply) = handle_hello(&hello, PEER_CERT).unwrap();
        assert_eq!(peer_cert, CLIENT_CERT);
        assert_eq!(got_nonce, nonce);
        assert_eq!(reply, make_reply_frame(PEER_CERT));
        // 应答方：init=对端（发起方）证书，resp=自身证书
        let responder_key = derive_session_key(&peer_cert, PEER_CERT, &got_nonce);
        // 发起方：init=自身证书，resp=对端证书
        let initiator_key = derive_session_key(CLIENT_CERT, PEER_CERT, &nonce);
        assert_eq!(responder_key, initiator_key);
    }

    // TC21: handle_hello 格式错 → Err(HandshakeFailed)
    #[test]
    fn tc21_handle_hello_bad_format() {
        // 空帧
        assert_eq!(
            handle_hello(b"", PEER_CERT).unwrap_err(),
            ChannelError::HandshakeFailed
        );
        // 坏 MAGIC
        let mut bad_magic = Vec::new();
        bad_magic.extend_from_slice(b"BAD!");
        bad_magic.extend_from_slice(&1u32.to_be_bytes());
        bad_magic.extend_from_slice(b"x");
        bad_magic.extend_from_slice(&[0u8; 32]);
        assert_eq!(
            handle_hello(&bad_magic, PEER_CERT).unwrap_err(),
            ChannelError::HandshakeFailed
        );
        // 缺 nonce（长度不足）
        let mut short = Vec::new();
        short.extend_from_slice(&MAGIC);
        short.extend_from_slice(&1u32.to_be_bytes());
        short.extend_from_slice(b"x");
        assert_eq!(
            handle_hello(&short, PEER_CERT).unwrap_err(),
            ChannelError::HandshakeFailed
        );
        // cert_len 与实际不符
        let mut wrong_len = Vec::new();
        wrong_len.extend_from_slice(&MAGIC);
        wrong_len.extend_from_slice(&100u32.to_be_bytes());
        wrong_len.extend_from_slice(b"xy");
        wrong_len.extend_from_slice(&[0u8; 32]);
        assert_eq!(
            handle_hello(&wrong_len, PEER_CERT).unwrap_err(),
            ChannelError::HandshakeFailed
        );
    }

    // TC22: 二次 connect 同 node_id → push 第二个 endpoint，connect_count==2
    #[test]
    fn tc22_connect_twice_same_node() {
        let mut t = MockSecureTransport::new();
        t.inbox.insert(
            2,
            vec![make_reply_frame(PEER_CERT), make_reply_frame(PEER_CERT)],
        );
        let mut ch = make_channel(true, t);
        ch.connect(2, test_addr(2)).unwrap();
        ch.connect(2, test_addr(2)).unwrap();
        assert_eq!(ch.connect_count, 2);
        assert_eq!(ch.endpoints.len(), 2);
        assert!(ch.endpoints.iter().all(|e| e.node_id == 2 && e.established));
    }

    // TC23: 多端点 connect（2 个 node 各自建立）
    #[test]
    fn tc23_connect_multiple_nodes() {
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![make_reply_frame(PEER_CERT)]);
        t.inbox.insert(3, vec![make_reply_frame(PEER_CERT)]);
        let mut ch = make_channel(true, t);
        ch.connect(2, test_addr(2)).unwrap();
        ch.connect(3, test_addr(3)).unwrap();
        assert_eq!(ch.connect_count, 2);
        assert_eq!(ch.endpoints.len(), 2);
        assert_eq!(ch.endpoints[0].node_id, 2);
        assert_eq!(ch.endpoints[1].node_id, 3);
        assert_eq!(ch.endpoints[1].addr, test_addr(3));
    }

    // TC24: 两次 connect 的 nonce 不同 → 会话密钥互异（nonce 逐握手唯一）
    #[test]
    fn tc24_connect_nonces_unique_keys_differ() {
        let (n1, n2) = preset_nonces();
        assert_ne!(n1, n2);
        let mut t = MockSecureTransport::new();
        t.inbox.insert(
            2,
            vec![make_reply_frame(PEER_CERT), make_reply_frame(PEER_CERT)],
        );
        let mut ch = make_channel(true, t);
        ch.connect(2, test_addr(2)).unwrap();
        ch.connect(2, test_addr(2)).unwrap();
        assert_eq!(
            ch.endpoints[0].session_key,
            derive_session_key(CLIENT_CERT, PEER_CERT, &n1)
        );
        assert_eq!(
            ch.endpoints[1].session_key,
            derive_session_key(CLIENT_CERT, PEER_CERT, &n2)
        );
        assert_ne!(ch.endpoints[0].session_key, ch.endpoints[1].session_key);
    }

    // ------------------------------------------------------------
    // TC25~TC34：call 加密通话
    // ------------------------------------------------------------

    // TC25: call 成功路径 → Ok(明文)、call_count==1、send_seq==1
    #[test]
    fn tc25_call_success() {
        let key = [0x42u8; 16];
        let mut t = MockSecureTransport::new();
        t.inbox
            .insert(2, vec![make_call_reply(&key, 2, 1, b"pong")]);
        let mut ch = channel_with_endpoint(t, key);
        assert_eq!(ch.call(2, b"ping"), Ok(b"pong".to_vec()));
        assert_eq!(ch.call_count, 1);
        assert_eq!(ch.endpoints[0].send_seq, 1);
        assert_eq!(ch.crypto_fail_count, 0);
    }

    // TC26: call 未知节点 → Err(UnknownNode)，计数器不变
    #[test]
    fn tc26_call_unknown_node() {
        let mut ch = make_channel(true, MockSecureTransport::new());
        assert_eq!(ch.call(99, b"ping"), Err(ChannelError::UnknownNode));
        assert_eq!(ch.call_count, 0);
        assert_eq!(ch.crypto_fail_count, 0);
    }

    // TC27: established=false → Err(ConnectionRefused)（手工构造 endpoint）
    #[test]
    fn tc27_call_not_established() {
        let mut ch = make_channel(true, MockSecureTransport::new());
        ch.endpoints.push(Endpoint {
            node_id: 2,
            addr: test_addr(2),
            established: false,
            session_key: [0x42u8; 16],
            send_seq: 0,
        });
        assert_eq!(ch.call(2, b"ping"), Err(ChannelError::ConnectionRefused));
        assert_eq!(ch.call_count, 0);
        assert_eq!(ch.endpoints[0].send_seq, 0); // 未递增
    }

    // TC28: 应答帧过短（< 8+16）→ Err(CryptoFailed) + crypto_fail_count==1
    #[test]
    fn tc28_call_reply_too_short() {
        let key = [0x42u8; 16];
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![vec![0u8; 10]]);
        let mut ch = channel_with_endpoint(t, key);
        assert_eq!(ch.call(2, b"ping"), Err(ChannelError::CryptoFailed));
        assert_eq!(ch.crypto_fail_count, 1);
        assert_eq!(ch.call_count, 0);
    }

    // TC29: 篡改应答帧 tag → Err(CryptoFailed) + crypto_fail_count==1
    #[test]
    fn tc29_call_tampered_tag() {
        let key = [0x42u8; 16];
        let mut reply = make_call_reply(&key, 2, 1, b"pong");
        let last = reply.len() - 1;
        reply[last] ^= 0x01; // 篡改 tag 末字节
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![reply]);
        let mut ch = channel_with_endpoint(t, key);
        assert_eq!(ch.call(2, b"ping"), Err(ChannelError::CryptoFailed));
        assert_eq!(ch.crypto_fail_count, 1);
    }

    // TC30: 篡改应答帧密文 → Err(CryptoFailed)
    #[test]
    fn tc30_call_tampered_ciphertext() {
        let key = [0x42u8; 16];
        let mut reply = make_call_reply(&key, 2, 1, b"pong");
        reply[8] ^= 0x01; // 篡改 ct 首字节
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![reply]);
        let mut ch = channel_with_endpoint(t, key);
        assert_eq!(ch.call(2, b"ping"), Err(ChannelError::CryptoFailed));
        assert_eq!(ch.crypto_fail_count, 1);
    }

    // TC31: aad/nonce 不匹配（应答帧头 seq 与加密时 seq 不一致）→ Err(CryptoFailed)
    #[test]
    fn tc31_call_wrong_seq_reply() {
        let key = [0x42u8; 16];
        // 用 seq=1 加密，但帧头写 seq=99 → 接收方按 99 重建 aad/nonce → 解密失败
        let mut reply = make_call_reply(&key, 2, 1, b"pong");
        reply[..8].copy_from_slice(&99u64.to_be_bytes());
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![reply]);
        let mut ch = channel_with_endpoint(t, key);
        assert_eq!(ch.call(2, b"ping"), Err(ChannelError::CryptoFailed));
        assert_eq!(ch.crypto_fail_count, 1);
    }

    // TC32: call send 失败 → Err(ConnectionRefused)
    #[test]
    fn tc32_call_send_fail() {
        let t = MockSecureTransport {
            fail_send_times: 1,
            ..MockSecureTransport::new()
        };
        let mut ch = channel_with_endpoint(t, [0x42u8; 16]);
        assert_eq!(ch.call(2, b"ping"), Err(ChannelError::ConnectionRefused));
        assert_eq!(ch.call_count, 0);
        assert_eq!(ch.crypto_fail_count, 0);
    }

    // TC33: call recv 失败 → Err(TransportFailed)
    #[test]
    fn tc33_call_recv_fail() {
        let t = MockSecureTransport {
            fail_recv_times: 1,
            ..MockSecureTransport::new()
        };
        let mut ch = channel_with_endpoint(t, [0x42u8; 16]);
        assert_eq!(ch.call(2, b"ping"), Err(ChannelError::TransportFailed));
        assert_eq!(ch.call_count, 0);
    }

    // TC34: 连续 2 次 call → seq 递增、nonce 不同（密文互异）、call_count==2
    #[test]
    fn tc34_call_twice_seq_increment() {
        let key = [0x42u8; 16];
        let mut t = MockSecureTransport::new();
        t.inbox.insert(
            2,
            vec![
                make_call_reply(&key, 2, 1, b"pong-1"),
                make_call_reply(&key, 2, 2, b"pong-2"),
            ],
        );
        let mut ch = channel_with_endpoint(t, key);
        assert_eq!(ch.call(2, b"ping"), Ok(b"pong-1".to_vec()));
        assert_eq!(ch.call(2, b"ping"), Ok(b"pong-2".to_vec()));
        assert_eq!(ch.call_count, 2);
        assert_eq!(ch.endpoints[0].send_seq, 2);
        // nonce 逐 seq 唯一：seq=1 与 seq=2 的 GCM nonce 不同
        assert_ne!(gcm_nonce(1), gcm_nonce(2));
    }

    // ------------------------------------------------------------
    // TC35~TC37：disconnect / reconnect
    // ------------------------------------------------------------

    // TC35: disconnect 存在的节点 → true，endpoint 被移除
    #[test]
    fn tc35_disconnect_existing() {
        let mut ch = channel_with_endpoint(MockSecureTransport::new(), [0x42u8; 16]);
        assert!(ch.disconnect(2));
        assert!(ch.endpoints.is_empty());
    }

    // TC36: disconnect 不存在 → false；移除后 call → Err(UnknownNode)
    #[test]
    fn tc36_disconnect_unknown_then_call_unknown() {
        let mut ch = channel_with_endpoint(MockSecureTransport::new(), [0x42u8; 16]);
        assert!(!ch.disconnect(99)); // 不存在 → false
        assert_eq!(ch.endpoints.len(), 1);
        assert!(ch.disconnect(2)); // 移除
        assert_eq!(ch.call(2, b"ping"), Err(ChannelError::UnknownNode));
    }

    // TC37: reconnect 未知节点 → Err(UnknownNode)；reconnect 成功 → addr 复用、新握手、established
    #[test]
    fn tc37_reconnect_unknown_and_success() {
        let (n1, n2) = preset_nonces();
        let mut t = MockSecureTransport::new();
        t.inbox.insert(
            2,
            vec![make_reply_frame(PEER_CERT), make_reply_frame(PEER_CERT)],
        );
        let mut ch = make_channel(true, t);
        assert_eq!(ch.reconnect(2), Err(ChannelError::UnknownNode)); // 未知节点
        ch.connect(2, test_addr(2)).unwrap();
        assert_eq!(
            ch.endpoints[0].session_key,
            derive_session_key(CLIENT_CERT, PEER_CERT, &n1)
        );
        ch.endpoints[0].send_seq = 5; // 模拟已通话
        assert_eq!(ch.reconnect(2), Ok(()));
        assert_eq!(ch.endpoints.len(), 1); // 旧端点被移除替换
        let ep = &ch.endpoints[0];
        assert_eq!(ep.addr, test_addr(2)); // addr 复用
        assert!(ep.established);
        assert_eq!(ep.send_seq, 0); // 新握手序号归零
        assert_eq!(
            ep.session_key,
            derive_session_key(CLIENT_CERT, PEER_CERT, &n2)
        ); // 新会话密钥
        assert_eq!(ch.connect_count, 2);
    }

    // ------------------------------------------------------------
    // TC38~TC40：计数器累计 / 全链路 / use_sm
    // ------------------------------------------------------------

    // TC38: 计数器跨多次调用累计正确
    #[test]
    fn tc38_counters_accumulate() {
        let mut t = MockSecureTransport::new();
        t.inbox.insert(2, vec![make_reply_frame(PEER_CERT)]); // connect(2) 握手应答
        t.inbox.insert(3, vec![make_reply_frame(PEER_CERT)]); // connect(3) 握手应答
        let mut ch = make_channel(true, t);
        ch.connect(2, test_addr(2)).unwrap(); // connect_count=1
        ch.connect(3, test_addr(3)).unwrap(); // connect_count=2
                                              // 一次握手失败（recv 空 inbox）
        assert_eq!(
            ch.connect(9, test_addr(9)),
            Err(ChannelError::HandshakeFailed)
        ); // handshake_fail=1
           // 一次 call 成功（node 2，seq=1，密钥为第一次 connect 派生）
        let (n1, _) = preset_nonces();
        let key2 = ch.endpoints[0].session_key;
        assert_eq!(key2, derive_session_key(CLIENT_CERT, PEER_CERT, &n1));
        // 用同密钥手工构造新 channel 验证 call/crypto 计数累计
        let mut t2 = MockSecureTransport::new();
        t2.inbox.insert(
            2,
            vec![
                make_call_reply(&key2, 2, 1, b"ok"),
                vec![0u8; 4], // 过短应答帧 → CryptoFailed
            ],
        );
        let mut ch2 = channel_with_endpoint(t2, key2);
        assert_eq!(ch2.call(2, b"a"), Ok(b"ok".to_vec())); // call_count=1
        assert_eq!(ch2.call(2, b"b"), Err(ChannelError::CryptoFailed)); // crypto_fail=1
        assert_eq!(ch2.call_count, 1);
        assert_eq!(ch2.crypto_fail_count, 1);
        // 累计断言（connect 通道侧）
        assert_eq!(ch.connect_count, 2);
        assert_eq!(ch.handshake_fail_count, 1);
    }

    // TC39: 全链路 connect→call→disconnect→reconnect→call
    #[test]
    fn tc39_full_lifecycle() {
        let (n1, n2) = preset_nonces();
        let key1 = derive_session_key(CLIENT_CERT, PEER_CERT, &n1);
        let key2 = derive_session_key(CLIENT_CERT, PEER_CERT, &n2);
        let mut t = MockSecureTransport::new();
        t.inbox.insert(
            2,
            vec![
                make_reply_frame(PEER_CERT),             // connect 握手应答
                make_call_reply(&key1, 2, 1, b"pong-1"), // 第一次 call 应答
                make_reply_frame(PEER_CERT),             // reconnect 握手应答
                make_call_reply(&key2, 2, 1, b"pong-2"), // 重连后 call 应答（新密钥、seq 归零）
            ],
        );
        let mut ch = make_channel(true, t);
        ch.connect(2, test_addr(2)).unwrap();
        assert_eq!(ch.call(2, b"ping"), Ok(b"pong-1".to_vec()));
        assert!(ch.disconnect(2));
        assert_eq!(ch.call(2, b"ping"), Err(ChannelError::UnknownNode));
        assert_eq!(ch.reconnect(2), Err(ChannelError::UnknownNode)); // disconnect 后已无端点
                                                                     // 重新 connect 恢复（等价 reconnect 语义：addr 复用由上层保证）
        ch.connect(2, test_addr(2)).unwrap();
        assert_eq!(ch.endpoints[0].session_key, key2);
        assert_eq!(ch.call(2, b"ping"), Ok(b"pong-2".to_vec()));
        assert_eq!(ch.connect_count, 2);
        assert_eq!(ch.call_count, 2);
    }

    // TC40: use_sm 字段不影响行为（D8，true/false 两条路径均成功）
    #[test]
    fn tc40_use_sm_flag_no_behavior_diff() {
        for use_sm in [true, false] {
            let (n1, _) = preset_nonces();
            let key = derive_session_key(CLIENT_CERT, PEER_CERT, &n1);
            let mut t = MockSecureTransport::new();
            t.inbox.insert(
                2,
                vec![
                    make_reply_frame(PEER_CERT),
                    make_call_reply(&key, 2, 1, b"pong"),
                ],
            );
            let mut ch = make_channel(true, t);
            ch.tls.use_sm = use_sm;
            assert_eq!(ch.connect(2, test_addr(2)), Ok(()));
            assert_eq!(ch.call(2, b"ping"), Ok(b"pong".to_vec()));
            assert_eq!(ch.connect_count, 1);
            assert_eq!(ch.call_count, 1);
        }
    }
}
