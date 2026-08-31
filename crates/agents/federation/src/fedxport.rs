//! # Phase 7 真实 mTLS 联邦运输（蓝图 61 §12 §2.4）
//!
//! `svc-federation` 与 E3 证据使用的真实网络传输：TLS 1.3/mTLS
//! （[`eneros_tls`] 基于 rustls，证书链验证无自签绕过）。
//!
//! - 帧协议：`FED1` ‖ u32 len ‖ [`FedFrame`]（与 [`crate::fednet`]
//!   模拟网络同构：数据面与传输面可互换，协议层行为一致）；
//! - 收发都是**有界**的（[`MAX_FRAME`]），防恶意超大帧；
//! - 会话实体身份由消息层 SM2 签名 + 注册表确立；TLS 层承担信道保密、
//!   完整性、双向证书认证。本运输层不提供任何 `dangerous()`
//!   降级验证路径。
//!
//! 本模块 std-only，由 `tls` feature 开启；不进入 no_std 站内闭包。
#![cfg(feature = "tls")]

use std::io::{Read, Write};

use crate::offer::{FedEnvelope, FedFrame, WireError, MAX_FRAME};

/// 帧头：`FED1` ‖ u32 length。
const FRAME_MAGIC: [u8; 4] = *b"FED1";
const HEADER_LEN: usize = 8;

/// 写一帧（magic + len + canonical frame），flush。
pub fn send_frame<W: Write>(w: &mut W, env: &FedEnvelope) -> std::io::Result<()> {
    let frame = FedFrame::encode(env)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "wire encode failed"))?;
    if frame.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "frame oversized",
        ));
    }
    let mut header = [0u8; HEADER_LEN];
    header[..4].copy_from_slice(&FRAME_MAGIC);
    header[4..].copy_from_slice(&(frame.len() as u32).to_be_bytes());
    w.write_all(&header)?;
    w.write_all(&frame)?;
    w.flush()
}

/// 读一帧（有界）。
pub fn recv_frame<R: Read>(r: &mut R) -> Result<FedEnvelope, RecvError> {
    let mut header = [0u8; HEADER_LEN];
    r.read_exact(&mut header).map_err(RecvError::Io)?;
    if header[..4] != FRAME_MAGIC {
        return Err(RecvError::BadMagic);
    }
    let len = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
    if len > MAX_FRAME {
        return Err(RecvError::Oversized);
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).map_err(RecvError::Io)?;
    FedFrame::decode(&buf).map_err(RecvError::Wire)
}

/// 接收错误（fail-closed 可观测）。
#[derive(Debug)]
pub enum RecvError {
    Io(std::io::Error),
    BadMagic,
    Oversized,
    Wire(WireError),
}

impl core::fmt::Display for RecvError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::BadMagic => write!(f, "bad frame magic"),
            Self::Oversized => write!(f, "frame oversized"),
            Self::Wire(e) => write!(f, "wire: {e:?}"),
        }
    }
}

/// PEM 解析辅助（证书链/私钥/根 CA）。
pub use eneros_tls::{build_root_store, parse_cert_chain_pem, parse_private_key_pem};

/// mTLS 服务端配置（证书 + 私钥 + 客户端证书验证）。
pub fn server_mtls(
    cert_pem: &[u8],
    key_pem: &[u8],
    ca_pem: &[u8],
) -> Result<eneros_tls::TlsServerConfig, eneros_tls::TlsError> {
    eneros_tls::TlsServerConfig::new_mtls(cert_pem, key_pem, ca_pem)
}

/// mTLS 客户端配置（根 CA + 客户端证书）。
pub fn client_mtls(
    ca_pem: &[u8],
    cert_pem: &[u8],
    key_pem: &[u8],
) -> Result<eneros_tls::TlsClientConfig, eneros_tls::TlsError> {
    eneros_tls::TlsClientConfig::new_mtls(ca_pem, cert_pem, key_pem)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use std::net::TcpListener;
    use std::thread;

    use eneros_crypto::rng::CsRng;
    use eneros_crypto::sm2::Sm2KeyPair;
    use eneros_tls::{TlsAcceptor, TlsClientConfig, TlsConnector, TlsServerConfig};
    use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};

    use super::*;
    use crate::negotiation::{SiteSigner, Sm2PublicKeyAlias};
    use crate::offer::*;

    fn kp() -> (Sm2KeyPair, CsRng) {
        let mut rng = CsRng::new();
        (Sm2KeyPair::generate(&mut rng).expect("kp"), rng)
    }

    /// 并行测试下确定性安装 rustls 默认 crypto provider（蓝 图/20 R2 先例）。
    fn ensure_provider() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    /// (ca, ca_key, server_cert, server_key, client_cert, client_key) PEM。
    type PemBundle = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);
    fn test_pki() -> PemBundle {
        let mut ca_params = CertificateParams::new(vec!["fed-test-ca".to_string()]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca_kp = KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_kp).unwrap();
        let mk = |cn: &str| {
            let mut p = CertificateParams::new(vec![cn.to_string()]).unwrap();
            p.is_ca = IsCa::NoCa;
            let leaf_kp = KeyPair::generate().unwrap();
            let cert = p.signed_by(&leaf_kp, &ca, &ca_kp).unwrap();
            (
                cert.pem().into_bytes(),
                leaf_kp.serialize_pem().into_bytes(),
            )
        };
        let (s_cert, s_key) = mk("coord-0");
        let (c_cert, c_key) = mk("site-9");
        (
            ca.pem().into_bytes(),
            ca_kp.serialize_pem().into_bytes(),
            s_cert,
            s_key,
            c_cert,
            c_key,
        )
    }

    fn offer_env(signer: &mut SiteSigner) -> FedEnvelope {
        signer.sign(
            &FedBody::Offer(CapabilityOffer {
                offer_id: [9; 16],
                interval_start_ns: 0,
                interval_end_ns: 3_600_000_000_000,
                p_min_kw: 0.0,
                p_max_kw: 50.0,
                energy_min_kwh: 0.0,
                energy_max_kwh: 50.0,
                ramp_kw_s: 10.0,
                reserve_up_kw: 1.0,
                reserve_down_kw: 1.0,
                price_curve_ref: BlobRefV1::new(1, b"p"),
                capability_root: [3u8; 32],
                valid_until_ns: u64::MAX,
                revision: 1,
            }),
            &EnvelopeHdr {
                epoch: 1,
                sequence: 1,
                tenant_id: 1,
                site_id: 9,
                created_ns: 1_000,
                deadline_ns: 100_000,
                signer_key_id: 101,
            },
        )
    }

    #[test]
    fn real_tls_roundtrip_offer() {
        ensure_provider();
        let (ca_pem, _ca_key, s_cert, s_key, c_cert, c_key) = test_pki();
        let server_cfg = TlsServerConfig::new_mtls(&s_cert, &s_key, &ca_pem).expect("srv cfg");
        let client_cfg = TlsClientConfig::new_mtls(&ca_pem, &c_cert, &c_key).expect("cli cfg");

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let acceptor = TlsAcceptor::new(listener, server_cfg);

        let (site_kp, rng) = kp();
        let mut signer = SiteSigner::new(101, site_kp, rng);
        let env = offer_env(&mut signer);
        let (srv_kp, rng_srv) = kp();
        let mut srv_signer = SiteSigner::new(202, srv_kp, rng_srv);
        let server = thread::spawn(move || {
            let mut stream = acceptor.accept().expect("accept");
            let recv = recv_frame(&mut stream).expect("recv");
            assert_eq!(recv.kind, FedKind::Offer);
            // 双向：回 Ack。
            let ack_env = srv_signer.sign(
                &FedBody::Ack(CommitmentAck {
                    commitment_id: [1; 16],
                    acked_at_ns: 2_000,
                }),
                &EnvelopeHdr {
                    epoch: 1,
                    sequence: 1,
                    tenant_id: 1,
                    site_id: 0,
                    created_ns: 2_000,
                    deadline_ns: 100_000,
                    signer_key_id: 202,
                },
            );
            send_frame(&mut stream, &ack_env).expect("send ack");
            recv
        });

        let mut stream = TlsConnector::new(client_cfg, "coord-0".try_into().expect("name"))
            .connect(format!("127.0.0.1:{port}").parse().expect("addr"))
            .expect("connect");
        send_frame(&mut stream, &env).expect("send");
        let ack = recv_frame(&mut stream).expect("recv ack");
        assert_eq!(ack.kind, FedKind::Ack);

        let sent = server.join().expect("srv thread");
        assert_eq!(sent.body, env.body);
        let _: &Sm2PublicKeyAlias = signer.public_key();
    }

    #[test]
    fn client_cert_missing_rejected() {
        ensure_provider();
        let (ca_pem, _ca_key, s_cert, s_key, _c_cert, _c_key) = test_pki();
        let server_cfg = TlsServerConfig::new_mtls(&s_cert, &s_key, &ca_pem).expect("srv cfg");
        let client_no_cert = TlsClientConfig::new_server_only(&ca_pem).expect("cli cfg");
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let acceptor = TlsAcceptor::new(listener, server_cfg);
        let server = thread::spawn(move || {
            let mut stream = acceptor.accept().expect("accept");
            let r = recv_frame(&mut stream).map(|_| ());
            assert!(r.is_err(), "无客户端证书必须拒绝");
        });
        let (site_kp, rng) = kp();
        let mut signer = SiteSigner::new(101, site_kp, rng);
        let env = offer_env(&mut signer);
        let mut stream = TlsConnector::new(client_no_cert, "coord-0".try_into().expect("name"))
            .connect(format!("127.0.0.1:{port}").parse().expect("addr"))
            .expect("connect");
        // 帧发送在握手之后；握手失败在首 IO 暴露（服务端断言 err）。
        let _ = send_frame(&mut stream, &env);
        server.join().expect("join");
    }
}
