//! Phase 7 E3 证据：真实 mTLS 端到端 offer→propose→commitment→ack→evidence
//! （蓝图 61 §12 §2.4「真实 mTLS transport」；E5/HIL 对端不在本环境，
//! E3 真实 TCP+TLS+国密签名协议为当前可复现证据，E5 登记 BLOCKED）。
//!
//! 进程内双线程：coordinator 线程（TLS 服务端）；site 线程（TLS 客户端）。
//! 全部消息经真实 TCP 环回 + rustls TLS 1.3 mTLS + SM2 协议签名。

#![allow(clippy::disallowed_macros)]
#![cfg(feature = "tls")]

use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::Sm2KeyPair;
use eneros_federation::allocation::{
    AllocationInput, AllocationResult, AllocationSolver, SiteOfferView, TargetProfile,
};
use eneros_federation::fedxport::{recv_frame, send_frame};
use eneros_federation::negotiation::{
    CoordinatorNegotiator, SiteNegotiator, SiteSigner, StepOutcome,
};
use eneros_federation::offer::{BlobRefV1, CapabilityOffer, ProgressEvidence, SettlementEvidence};
use eneros_tls::{TlsAcceptor, TlsClientConfig, TlsConnector, TlsServerConfig};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};

const NOW: u64 = 2_000_000_000u64;

fn ensure_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// u32 → 16 字节 id。
fn fed_id16(v: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&v.to_be_bytes());
    out
}

fn gen_kp() -> (Sm2KeyPair, CsRng) {
    let mut rng = CsRng::new();
    (Sm2KeyPair::generate(&mut rng).expect("kp"), rng)
}

/// (ca, ca_key, server_cert, server_key, client_cert, client_key) PEM 字节。
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

fn site_offer(p_max: f64) -> CapabilityOffer {
    CapabilityOffer {
        offer_id: [9; 16],
        interval_start_ns: 0,
        interval_end_ns: 3_600_000_000_000,
        p_min_kw: 0.0,
        p_max_kw: p_max,
        energy_min_kwh: 0.0,
        energy_max_kwh: p_max,
        ramp_kw_s: p_max / 10.0,
        reserve_up_kw: p_max * 0.05,
        reserve_down_kw: p_max * 0.05,
        price_curve_ref: BlobRefV1::new(1, b"price"),
        capability_root: [9; 32],
        valid_until_ns: u64::MAX,
        revision: 1,
    }
}

#[test]
fn real_tls_full_negotiation() {
    ensure_provider();
    let (ca_pem, _ca_key, s_cert, s_key, c_cert, c_key) = test_pki();
    let server_cfg = TlsServerConfig::new_mtls(&s_cert, &s_key, &ca_pem).expect("srv cfg");
    let client_cfg = TlsClientConfig::new_mtls(&ca_pem, &c_cert, &c_key).expect("cli cfg");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let acceptor = TlsAcceptor::new(listener, server_cfg);

    // 共享密钥身份：站点 key（site 持有私钥）登记到协调器；协调器 key 登记到站点。
    let (kp_site, rng_site) = gen_kp();
    let (kp_coord, rng_coord) = gen_kp();

    // ---- 协调器线程 ----
    let coord_pub = kp_coord.public_key;
    let coord = thread::spawn(move || {
        let mut coord = CoordinatorNegotiator::new(0, 1, 1, 1);
        coord
            .register_site_key(101, &kp_site.public_key, NOW)
            .expect("reg site key");
        let mut signer = SiteSigner::new(202, kp_coord, rng_coord);
        let mut stream = acceptor.accept().expect("accept");
        stream
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(15)))
            .ok();
        let mut committed = false;
        let mut evidence = 0u32;
        loop {
            let env = match recv_frame(&mut stream) {
                Ok(e) => e,
                // 对端正常关闭（站点会话完成）→ 优雅收尾。
                Err(eneros_federation::fedxport::RecvError::Io(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(e) => panic!("coord recv: {e}"),
            };
            let (out, outbox) = coord.on_message(&env, NOW + 100, &mut signer);
            match out {
                StepOutcome::OfferRegistered { .. } => {
                    // 求解：单站目标 = 容量 50%。
                    let views: Vec<SiteOfferView> = coord
                        .sessions()
                        .iter()
                        .map(|(sid, s)| {
                            let o = s.offer().expect("offer");
                            SiteOfferView {
                                site_id: *sid,
                                offer_id: o.offer_id,
                                revision: o.revision,
                                p_min_kw: o.p_min_kw,
                                p_max_kw: o.p_max_kw,
                                energy_min_kwh: o.energy_min_kwh,
                                energy_max_kwh: o.energy_max_kwh,
                                ramp_kw_s: o.ramp_kw_s,
                                reserve_up_kw: o.reserve_up_kw,
                                reserve_down_kw: o.reserve_down_kw,
                                price_rank: *sid,
                            }
                        })
                        .collect();
                    let target = views.iter().map(|v| v.p_max_kw).sum::<f64>() * 0.5;
                    let inp = AllocationInput {
                        interval_start_ns: 0,
                        interval_ns: 3_600_000_000_000,
                        sites: views,
                        target: TargetProfile {
                            p_target_kw: vec![target],
                            reserve_up_kw: 0.0,
                            reserve_down_kw: 0.0,
                        },
                    };
                    let alloc_targets = match AllocationSolver::solve(&inp).expect("solve") {
                        AllocationResult::Feasible { targets, .. } => targets,
                        other => panic!("coord: {other:?}"),
                    };
                    for t in alloc_targets {
                        let o = coord
                            .session(t.site_id)
                            .and_then(|s| s.offer())
                            .expect("offer");
                        let proposal = AllocationSolver::to_proposal(
                            &t,
                            o,
                            fed_id16(t.site_id),
                            1,
                            0,
                            3_600_000_000_000,
                        )
                        .expect("proposal");
                        let (_, penv) = coord
                            .propose(t.site_id, &proposal, NOW + 200, &mut signer)
                            .expect("propose");
                        send_frame(&mut stream, &penv).expect("send proposal");
                    }
                }
                StepOutcome::CommitmentConfirmed { .. } => {
                    committed = true;
                    for e in outbox {
                        send_frame(&mut stream, &e).expect("send ack");
                    }
                }
                StepOutcome::EvidenceRecorded { .. } => {
                    evidence += 1;
                }
                StepOutcome::ReplayRejected { .. }
                | StepOutcome::StaleLeaderRejected { .. }
                | StepOutcome::SignatureRejected
                | StepOutcome::TenantMismatch => panic!("coord rejected: {out:?}"),
                _ => {}
            }
        }
        assert!(committed, "coordinator 必须确认 commitment");
        assert!(evidence >= 2, "coordinator 必须收到 progress+settlement");
        coord
    });

    // ---- 站点线程 ----
    let site = thread::spawn(move || {
        let mut site = SiteNegotiator::new(9, 1, 1, 100);
        site.register_peer_key_with_id(202, &coord_pub, NOW)
            .expect("reg coord key");
        let mut signer = SiteSigner::new(101, kp_site, rng_site);
        let mut stream = TlsConnector::new(client_cfg, "coord-0".try_into().expect("name"))
            .connect(format!("127.0.0.1:{port}").parse().expect("addr"))
            .expect("connect");
        // 1. 发布 offer。
        let (_, env) = site
            .publish_offer(&site_offer(100.0), NOW, &mut signer)
            .expect("publish");
        send_frame(&mut stream, &env).expect("send offer");
        // 2. 收 proposal → accept → 签 commitment。
        let env = recv_frame(&mut stream).expect("recv proposal");
        let (out, outbox) = site.on_message(&env, NOW + 300, &mut signer);
        assert!(
            matches!(out, StepOutcome::ProposalAccepted { .. }),
            "site: {out:?}"
        );
        assert_eq!(outbox.len(), 1);
        send_frame(&mut stream, &outbox[0]).expect("send commitment");
        // 3. 收 Ack。
        let env = recv_frame(&mut stream).expect("recv ack");
        let (out, _) = site.on_message(&env, NOW + 400, &mut signer);
        assert!(matches!(out, StepOutcome::Acknowledged { .. }), "{out:?}");
        // 4. 提交聚合进度 + 结算证据。
        let cid = site.commitment().expect("cmt").commitment.commitment_id;
        let prog = ProgressEvidence {
            commitment_id: cid,
            site_id: 9,
            interval_start_ns: 0,
            interval_end_ns: 3_600_000_000_000,
            delivered_kwh: 50.0,
            deviation_kw: 0.2,
            evidence_ref: BlobRefV1::new(3, b"ev"),
            seq: 1,
        };
        let (_, pv) = site
            .submit_progress(&prog, NOW + 500, &mut signer)
            .expect("prog");
        send_frame(&mut stream, &pv).expect("send progress");
        let set = SettlementEvidence {
            commitment_id: cid,
            site_id: 9,
            settle_start_ns: 0,
            settle_end_ns: 3_600_000_000_000,
            measured_milli_kwh: 50_000,
            quality_bp: 9_900,
            note: b"e2e".to_vec(),
        };
        let (_, sv) = site
            .submit_settlement(&set, NOW + 600, &mut signer)
            .expect("settle");
        send_frame(&mut stream, &sv).expect("send settlement");
        // 5. 成功退出。
        site
    });

    let coord = coord.join().expect("coord thread");
    let site = site.join().expect("site thread");
    assert_eq!(coord.sessions().len(), 1);
    assert_eq!(coord.session(9).expect("s9").evidence_progress.len(), 1);
    assert_eq!(coord.session(9).expect("s9").evidence_settlement.len(), 1);
    assert!(site.commitment().expect("cmt").acked, "站点必须已受 Ack");
    eprintln!("E3 证据：真实 mTLS 端到端 offer→commitment→ack→evidence 完成");
}
