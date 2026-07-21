//! EnerOS v0.115.0 mTLS 双向认证通信安全（P2-I 安全体系第 3 版）.
//!
//! 联邦跨域通信缺乏双向身份认证与国密加密通道，存在中间人攻击与窃听风险。
//! 本 crate 实现 mTLS 双向认证 + SM2/SM3/SM4 全国密通信加密，满足信创合规与
//! 「抓包全加密」出口判定（蓝图 §1），为 v0.116.0 模型签名与联邦可信验证奠基。
//!
//! # 核心类型
//!
//! - [`SmCipherSuite`] / [`KeyExchange`] / [`Cipher`] / [`MacAlgorithm`] /
//!   [`negotiate`] — SM 密码套件：SM2-DHE 密钥交换 + SM4-GCM/SM4-CBC +
//!   SM3-HMAC；服务端优先顺序选首个交集，无交集返回
//!   [`TlsError::NoCommonCipherSuite`]
//! - [`CertManager`] — 证书管理：链式验签（复用 eneros-crypto
//!   `verify_signature`）→ 有效期 → CRL 吊销检查，顺序固定，错误显式传播
//! - [`MtlsContext`] / [`HandshakeOutcome`] — mTLS 握手状态机：ClientHello →
//!   ServerHello+证书+CertRequest → 互验证书 → SM2 临时密钥交换（签名证明
//!   私钥持有）→ SM3-HMAC Finished → 派生会话密钥
//! - [`MtlsRecord`] — 记录层：SM4-GCM（或 SM4-CBC+SM3-HMAC）加密 + 单调
//!   序列号 nonce + AAD 绑序列号 + 64 位防重放滑动窗口
//! - [`TlsError`] / [`CertError`] / [`TlsStats`] — 错误模型与可观测统计
//! - [`MtlsTransport`] / [`MockMtlsTransport`] — 传输抽象（同步 trait，
//!   v0.110.0 SyncTransport / v0.114.0 AttestTransport 同先例）
//!
//! # 偏差声明（相对蓝图 §4.5）
//!
//! 蓝图 §4.5 为 GmSSL C FFI（extern "C" + NonNull + std::net::TcpStream）。
//! 按记忆 §4.3 no_std 硬性要求与 v0.113.0/v0.114.0 先例，FFI 移除，改为纯
//! Rust 实现 + [`MtlsTransport`] 同步 trait 抽象，主机可测；真实 GmSSL/
//! Tongsuo 适配器归属集成层。完整偏差表（D1~Dn）登记于
//! `docs/security/mtls-design.md`。
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `core::*` / `alloc::*`，唯一依赖 eneros-crypto（workspace 内 path
//! 依赖），零第三方依赖，零 unsafe，零 extern "C"，不调用 `panic!` /
//! `todo!` / `unimplemented!`，可交叉编译到 `aarch64-unknown-none`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

pub mod cert_mgr;
pub mod cipher_suite;
pub mod handshake;
pub mod record;

pub use cert_mgr::CertManager;
pub use cipher_suite::{negotiate, Cipher, KeyExchange, MacAlgorithm, SmCipherSuite};
pub use handshake::{HandshakeOutcome, MtlsContext};
pub use record::MtlsRecord;

/// 证书错误（mTLS 证书管理专用，Copy 对齐 v0.111.0/v0.113.0 错误模型惯例）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertError {
    /// 证书已过期（`now > not_after`）.
    Expired,
    /// 证书尚未生效（`now < not_before`）.
    NotYetValid,
    /// 证书已被吊销（序列号命中已加载 CRL）.
    Revoked,
    /// 证书签名验证失败（颁发者公钥验签不通过）.
    SignatureInvalid,
    /// 信任链断裂（信任根中找不到证书颁发者）.
    ChainBroken,
}

/// mTLS 错误（≥6 变体，Copy；`CertInvalid` 携带 [`CertError`] 保留拒绝原因）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsError {
    /// 双方密码套件列表无交集（协商失败）.
    NoCommonCipherSuite,
    /// 握手失败（对端密钥交换签名无效 / Finished HMAC 不匹配 / 对端中止）.
    HandshakeFailed,
    /// 对端证书无效（携带具体 [`CertError`] 原因）.
    CertInvalid(CertError),
    /// 记录解密失败（GCM tag 校验失败 / CBC 填充或 HMAC 校验失败 / 帧格式损坏）.
    DecryptFailed,
    /// 重放帧拒绝（序列号命中防重放窗口或已滑出窗口左界）.
    ReplayDetected,
    /// 传输通道故障（[`MtlsTransport`] 发送/接收错误或对端断连）.
    TransportError,
    /// 线上消息格式非法（帧头长度错误 / 消息类型与握手状态不符）.
    InvalidMessage,
    /// 内部错误（证书 DER 编解码失败等不应发生的情形）.
    InternalError,
}

impl From<CertError> for TlsError {
    /// 证书错误 → mTLS 错误映射：统一包装为 [`TlsError::CertInvalid`].
    fn from(e: CertError) -> Self {
        TlsError::CertInvalid(e)
    }
}

/// mTLS 可观测统计（蓝图 §9 落地；Copy 对齐 TlsStats 惯例）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TlsStats {
    /// 累计成功握手次数.
    pub handshakes: u32,
    /// 累计握手拒绝次数（证书无效 / Finished 不匹配 / 套件无交集等）.
    pub rejected: u32,
    /// 累计发送记录数.
    pub records_sent: u32,
    /// 累计接收记录数.
    pub records_recv: u32,
    /// 最近一次错误（无错误为 `None`）.
    pub last_error: Option<TlsError>,
}

/// mTLS 传输抽象（同步 trait；蓝图 `std::net::TcpStream` 的 no_std 替代）.
///
/// 帧语义：每次 `send` 发送一个完整消息帧，`recv` 返回一个完整消息帧。
/// 真实 TCP 适配器（流式切帧）归属集成层实现同一 trait。
pub trait MtlsTransport {
    /// 发送一个完整消息帧.
    fn send(&mut self, data: &[u8]) -> Result<(), TlsError>;
    /// 接收一个完整消息帧（阻塞至收到或失败）.
    fn recv(&mut self) -> Result<Vec<u8>, TlsError>;
}

/// 模拟 mTLS 传输（单线程回环对 + 故障注入 + 调用计数）.
///
/// 通过 [`MockMtlsTransport::pair`] 创建交叉连接的一对端点：一端 `send`
/// 的字节进入另一端 `recv` 队列。基于 `Rc<RefCell<…>>`，仅限单线程使用；
/// 多线程握手测试请在 `#[cfg(test)]` 内用 `std::sync::mpsc` 自行实现
/// [`MtlsTransport`]（见 `handshake.rs` 测试模块先例）。
pub struct MockMtlsTransport {
    /// 接收队列（对端 outbox 与本端 inbox 共享同一队列）.
    inbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
    /// 发送队列.
    outbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
    /// 故障注入：下一次 `send` 返回 [`TlsError::TransportError`].
    fail_next_send: bool,
    /// 故障注入：下一次 `recv` 返回 [`TlsError::TransportError`].
    fail_next_recv: bool,
    /// 累计 send + recv 调用次数.
    pub calls: u32,
}

impl MockMtlsTransport {
    /// 创建交叉连接的一对模拟传输端点.
    pub fn pair() -> (Self, Self) {
        let q_ab: Rc<RefCell<VecDeque<Vec<u8>>>> = Rc::new(RefCell::new(VecDeque::new()));
        let q_ba: Rc<RefCell<VecDeque<Vec<u8>>>> = Rc::new(RefCell::new(VecDeque::new()));
        let a = Self {
            inbox: Rc::clone(&q_ba),
            outbox: Rc::clone(&q_ab),
            fail_next_send: false,
            fail_next_recv: false,
            calls: 0,
        };
        let b = Self {
            inbox: Rc::clone(&q_ab),
            outbox: Rc::clone(&q_ba),
            fail_next_send: false,
            fail_next_recv: false,
            calls: 0,
        };
        (a, b)
    }

    /// 注入：下一次 `send` 失败.
    pub fn fail_next_send(&mut self) {
        self.fail_next_send = true;
    }

    /// 注入：下一次 `recv` 失败.
    pub fn fail_next_recv(&mut self) {
        self.fail_next_recv = true;
    }

    /// 对端队列中待接收的帧数（测试观察用）.
    pub fn pending(&self) -> usize {
        self.inbox.borrow().len()
    }
}

impl MtlsTransport for MockMtlsTransport {
    fn send(&mut self, data: &[u8]) -> Result<(), TlsError> {
        self.calls += 1;
        if self.fail_next_send {
            self.fail_next_send = false;
            return Err(TlsError::TransportError);
        }
        self.outbox.borrow_mut().push_back(data.to_vec());
        Ok(())
    }

    fn recv(&mut self) -> Result<Vec<u8>, TlsError> {
        self.calls += 1;
        if self.fail_next_recv {
            self.fail_next_recv = false;
            return Err(TlsError::TransportError);
        }
        self.inbox
            .borrow_mut()
            .pop_front()
            .ok_or(TlsError::TransportError)
    }
}

// ============================================================
// 集成测试 INT18~INT19 + 性能测试 PERF20
// （std 线程 + mpsc 通道传输，测试模块内允许 std）
// ============================================================

#[cfg(test)]
mod tests {
    use alloc::vec;
    use std::sync::mpsc;
    use std::thread;

    use eneros_crypto::{
        build_certificate, build_self_signed, CertRequest, CsRng, DistinguishedName, Sm2KeyPair,
        SubjectPublicKey, X509Certificate,
    };

    use super::*;

    /// 固定测试时间戳（2023-11-14 22:13:20 UTC）.
    const NOW: u64 = 1_700_000_000;

    /// 默认套件：SM2-DHE + SM4-GCM + SM3-HMAC.
    const SUITE: SmCipherSuite =
        SmCipherSuite::new(KeyExchange::Sm2Dhe, Cipher::Sm4Gcm, MacAlgorithm::Sm3Hmac);

    /// 测试 PKI：CA + 服务端/客户端证书与密钥对.
    struct TestPki {
        ca_cert: X509Certificate,
        server_cert: X509Certificate,
        server_kp: Sm2KeyPair,
        client_cert: X509Certificate,
        client_kp: Sm2KeyPair,
    }

    /// 构建两级测试 PKI（自签名 CA → 双端叶子证书）.
    fn make_pki(now: u64) -> TestPki {
        let mut rng = CsRng::new();
        let ca_kp = Sm2KeyPair::generate(&mut rng).expect("CA 密钥对");
        let ca_req = CertRequest::new(
            DistinguishedName::new("EnerOS mTLS INT CA")
                .with_o("EnerOS")
                .with_c("CN"),
            SubjectPublicKey::Sm2(ca_kp.public_key),
        );
        let ca_cert = build_self_signed(
            &ca_req,
            &ca_kp.private_key,
            &ca_kp.public_key,
            now,
            &mut rng,
        )
        .expect("CA 证书");

        let server_kp = Sm2KeyPair::generate(&mut rng).expect("服务端密钥对");
        let server_req = CertRequest::new(
            DistinguishedName::new("mtls-int-server")
                .with_o("EnerOS")
                .with_c("CN"),
            SubjectPublicKey::Sm2(server_kp.public_key),
        )
        .with_validity_days(365);
        let server_cert = build_certificate(
            &server_req,
            &ca_cert.subject,
            &ca_kp.private_key,
            &ca_kp.public_key,
            &[0x11],
            now,
            &mut rng,
        )
        .expect("服务端证书");

        let client_kp = Sm2KeyPair::generate(&mut rng).expect("客户端密钥对");
        let client_req = CertRequest::new(
            DistinguishedName::new("mtls-int-client")
                .with_o("EnerOS")
                .with_c("CN"),
            SubjectPublicKey::Sm2(client_kp.public_key),
        )
        .with_validity_days(365);
        let client_cert = build_certificate(
            &client_req,
            &ca_cert.subject,
            &ca_kp.private_key,
            &ca_kp.public_key,
            &[0x21],
            now,
            &mut rng,
        )
        .expect("客户端证书");

        TestPki {
            ca_cert,
            server_cert,
            server_kp,
            client_cert,
            client_kp,
        }
    }

    /// 基于 std::sync::mpsc 的双向通道传输（可选中间人篡改注入）.
    ///
    /// 中间人模型：转发第 `mitm_frame` 帧（本端 send 计数，从 1 起）时翻转
    /// 其 `mitm_offset` 处字节（`usize::MAX` 表示最后一字节）。
    struct ChannelTransport {
        tx: mpsc::Sender<Vec<u8>>,
        rx: mpsc::Receiver<Vec<u8>>,
        forwarded: usize,
        mitm_frame: Option<usize>,
        mitm_offset: usize,
    }

    impl ChannelTransport {
        fn new(tx: mpsc::Sender<Vec<u8>>, rx: mpsc::Receiver<Vec<u8>>) -> Self {
            Self {
                tx,
                rx,
                forwarded: 0,
                mitm_frame: None,
                mitm_offset: 0,
            }
        }
    }

    impl MtlsTransport for ChannelTransport {
        fn send(&mut self, data: &[u8]) -> Result<(), TlsError> {
            let mut d = data.to_vec();
            self.forwarded += 1;
            if self.mitm_frame == Some(self.forwarded) && !d.is_empty() {
                let idx = if self.mitm_offset == usize::MAX {
                    d.len() - 1
                } else {
                    self.mitm_offset % d.len()
                };
                d[idx] ^= 0xFF;
            }
            self.tx.send(d).map_err(|_| TlsError::TransportError)
        }

        fn recv(&mut self) -> Result<Vec<u8>, TlsError> {
            self.rx.recv().map_err(|_| TlsError::TransportError)
        }
    }

    /// 篡改注入侧：客户端上行帧 / 服务端下行帧.
    enum TamperSide {
        Client,
        Server,
    }

    /// 运行一次双向握手，可选在一侧注入中间人篡改，返回（客户端结果，服务端结果）.
    fn run_handshake(
        tamper: Option<(TamperSide, usize, usize)>,
    ) -> (
        Result<HandshakeOutcome, TlsError>,
        Result<HandshakeOutcome, TlsError>,
    ) {
        let pki = make_pki(NOW);
        let server_ctx = MtlsContext::new(
            pki.server_cert.clone(),
            pki.server_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );
        let mut client_ctx = MtlsContext::new(
            pki.client_cert.clone(),
            pki.client_kp.private_key.clone(),
            vec![pki.ca_cert.clone()],
            true,
            vec![SUITE],
        );

        let (c2s_tx, c2s_rx) = mpsc::channel::<Vec<u8>>();
        let (s2c_tx, s2c_rx) = mpsc::channel::<Vec<u8>>();

        let server_tamper = match tamper {
            Some((TamperSide::Server, frame, offset)) => Some((frame, offset)),
            _ => None,
        };
        let handle = thread::spawn(move || {
            let mut transport = ChannelTransport::new(s2c_tx, c2s_rx);
            if let Some((frame, offset)) = server_tamper {
                transport.mitm_frame = Some(frame);
                transport.mitm_offset = offset;
            }
            let mut ctx = server_ctx;
            let mut rng = CsRng::new();
            ctx.handshake_server(&mut transport, &mut rng, NOW)
        });

        let mut transport = ChannelTransport::new(c2s_tx, s2c_rx);
        if let Some((TamperSide::Client, frame, offset)) = tamper {
            transport.mitm_frame = Some(frame);
            transport.mitm_offset = offset;
        }
        let mut rng = CsRng::new();
        let client_res = client_ctx.handshake_client(&mut transport, &mut rng, NOW);
        // 显式断开客户端→服务端通道：中间人篡改导致客户端先拒绝时不再发帧，
        // 释放 c2s_tx 使服务端阻塞的 recv 立即返回 TransportError，防测试死锁
        drop(transport);
        let server_res = handle.join().expect("服务端线程");
        (client_res, server_res)
    }

    // ========================================================
    // INT18：端到端双向 mTLS 加密通信（蓝图 §7.1）
    // ========================================================

    /// INT18 快乐路径：双向握手成功 → 双方向记录层加密往返 → 明文一致，
    /// 且线上字节不含明文（抓包全加密），统计计数正确.
    #[test]
    fn int18_end_to_end_encrypted_channel() {
        let (client_res, server_res) = run_handshake(None);
        let client_out = client_res.expect("客户端握手应成功");
        let server_out = server_res.expect("服务端握手应成功");
        assert_eq!(
            client_out.session_key, server_out.session_key,
            "双方会话密钥必须一致"
        );

        // 双方由各自握手产物构造记录层（共享密钥与套件）
        let mut client_rec = MtlsRecord::new(&client_out);
        let mut server_rec = MtlsRecord::new(&server_out);

        // 客户端 → 服务端：调度指令
        let msg_c = b"dispatch: P=+50MW @15:00";
        let wire_c = client_rec.seal(msg_c);
        assert!(
            wire_c.windows(msg_c.len()).all(|w| w != &msg_c[..]),
            "线上字节不得包含明文（抓包全加密）"
        );
        let opened_c = server_rec.open(&wire_c).expect("服务端解密应成功");
        assert_eq!(opened_c, &msg_c[..], "解密明文必须与原始一致");

        // 服务端 → 客户端：量测数据
        let msg_s = b"telemetry: U=10.2kV I=301A";
        let wire_s = server_rec.seal(msg_s);
        assert!(
            wire_s.windows(msg_s.len()).all(|w| w != &msg_s[..]),
            "线上字节不得包含明文（抓包全加密）"
        );
        let opened_s = client_rec.open(&wire_s).expect("客户端解密应成功");
        assert_eq!(opened_s, &msg_s[..], "解密明文必须与原始一致");

        assert_eq!(client_rec.records_sent, 1);
        assert_eq!(client_rec.records_recv, 1);
        assert_eq!(server_rec.records_sent, 1);
        assert_eq!(server_rec.records_recv, 1);
    }

    // ========================================================
    // INT19：中间人篡改全线拒绝（蓝图 §7.3）
    // ========================================================

    /// INT19 中间人攻击：转发中篡改任一握手帧（ClientHello / ServerHello /
    /// Certificate / Finished）→ 握手失败；篡改已建连记录字节 → DecryptFailed；
    /// 重放记录 → ReplayDetected。全线拒绝，无一漏网.
    #[test]
    fn int19_mitm_tamper_all_rejected() {
        // ① 篡改 ClientHello（客户端上行第 1 帧，random_c 区域）
        let (c, s) = run_handshake(Some((TamperSide::Client, 1, 10)));
        assert!(c.is_err() && s.is_err(), "篡改 ClientHello 必须双方失败");

        // ② 篡改 ServerHello（服务端下行第 1 帧，random_s 区域）
        let (c, s) = run_handshake(Some((TamperSide::Server, 1, 10)));
        assert!(c.is_err() && s.is_err(), "篡改 ServerHello 必须双方失败");

        // ③ 篡改 Certificate（服务端下行第 2 帧，证书 DER 区域）
        let (c, s) = run_handshake(Some((TamperSide::Server, 2, 100)));
        assert!(c.is_err() && s.is_err(), "篡改 Certificate 必须双方失败");

        // ④ 篡改 Finished（客户端上行第 4 帧，最后一字节 HMAC）
        let (c, s) = run_handshake(Some((TamperSide::Client, 4, usize::MAX)));
        assert_eq!(
            s,
            Err(TlsError::HandshakeFailed),
            "服务端 Finished 校验失败应返回 HandshakeFailed"
        );
        assert!(c.is_err(), "服务端中止后客户端必失败");

        // ⑤ 建连后篡改记录字节 → DecryptFailed；重放 → ReplayDetected
        let (client_res, server_res) = run_handshake(None);
        let client_out = client_res.expect("客户端握手应成功");
        let server_out = server_res.expect("服务端握手应成功");
        let mut client_rec = MtlsRecord::new(&client_out);
        let mut server_rec = MtlsRecord::new(&server_out);

        let wire = client_rec.seal(b"firmware-block-0001");
        let mut tampered = wire.clone();
        let n = tampered.len();
        tampered[n - 1] ^= 0xFF; // 翻转 GCM tag 末字节
        assert_eq!(
            server_rec.open(&tampered),
            Err(TlsError::DecryptFailed),
            "篡改记录必须解密失败"
        );

        // 重放同一帧：第一次接受，第二次拒绝
        let wire2 = client_rec.seal(b"firmware-block-0002");
        server_rec.open(&wire2).expect("首次接收应成功");
        assert_eq!(
            server_rec.open(&wire2),
            Err(TlsError::ReplayDetected),
            "重放帧必须拒绝"
        );
    }

    // ========================================================
    // PERF20：握手性能（蓝图 §6.3/§7.2，cfg(test) Instant 口径，
    // 同 v0.113.0 PERF20 / v0.114.0 PERF22 先例）
    // ========================================================

    /// PERF20 单次双向握手（含双向验签 + SM2 密钥交换 + HMAC Finished）计时：
    /// debug 仅打印；release 默认打印，设 `ENEROS_PERF_GATE=1` 时断言
    /// < 200ms（门禁面向目标硬件 SM2 加速 / 性能 CI 场景，偏差见设计文档 D 表）.
    #[test]
    fn perf20_handshake_under_200ms() {
        let start = std::time::Instant::now();
        let (client_res, server_res) = run_handshake(None);
        let elapsed = start.elapsed();
        assert!(client_res.is_ok() && server_res.is_ok(), "握手必须成功");
        #[cfg(debug_assertions)]
        eprintln!("[PERF20] 单次双向握手耗时: {:?}", elapsed);
        #[cfg(not(debug_assertions))]
        {
            // 仅当变量值恰为 "1" 时启用门禁：规避终端残留空串变量误激活
            if std::env::var("ENEROS_PERF_GATE").as_deref() == Ok("1") {
                assert!(
                    elapsed.as_millis() < 200,
                    "PERF20 双向握手耗时 {:?} 超过 200ms 上限",
                    elapsed
                );
            } else {
                eprintln!(
                    "[PERF20] 单次双向握手耗时: {:?}（设 ENEROS_PERF_GATE=1 启用 <200ms 断言）",
                    elapsed
                );
            }
        }
    }
}
