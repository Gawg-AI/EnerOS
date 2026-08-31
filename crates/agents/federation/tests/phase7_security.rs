//! Phase 7 集成证据：100 站压力、故障注入、跨租户、恶意云目标（蓝图 61 §12 §2.7）。
//!
//! 覆盖蓝图 §2.7 测试清单：
//! 3/10/100 站 offer/commit/revision；网络分区；协调器重启；旧 leader；
//! 证书撤销；重放；站内能力突降；承诺不可行（default-fill 禁令）；
//! 部分站离线；跨租户/站数据隔离；云发恶意超范围目标→站点拒绝且站内继续。

#![allow(clippy::disallowed_macros)]
use eneros_crypto::rng::CsRng;
use eneros_crypto::sm2::{Sm2KeyPair, Sm2PublicKey};
use eneros_federation::allocation::{
    AllocationInput, AllocationResult, AllocationSolver, SiteOfferView, TargetProfile,
};
use eneros_federation::fednet::FedNet;
use eneros_federation::negotiation::{
    CoordinatorNegotiator, SiteNegotiator, SiteSigner, StepOutcome,
};
use eneros_federation::offer::{
    BlobRefV1, CapabilityOffer, EnvelopeHdr, FedBody, ProgressEvidence, RejectReason,
    SettlementEvidence,
};

const NOW: u64 = 1_000_000_000u64;

/// u32 → 16 字节 id（确定性）。
fn id16(v: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&v.to_be_bytes());
    out
}

/// u32 → 32 字节 digest。
fn id32(v: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..4].copy_from_slice(&v.to_be_bytes());
    out
}

fn gen_kp() -> (Sm2KeyPair, CsRng) {
    let mut rng = CsRng::new();
    (Sm2KeyPair::generate(&mut rng).expect("kp"), rng)
}

/// 站点能力包络（按 site_id 确定性变化：能力突降场景可缩小）。
fn site_offer(site_id: u32, revision: u64, p_max_kw: f64) -> CapabilityOffer {
    CapabilityOffer {
        offer_id: id16(site_id),
        interval_start_ns: 0,
        interval_end_ns: 3_600_000_000_000,
        p_min_kw: 0.0,
        p_max_kw,
        energy_min_kwh: 0.0,
        energy_max_kwh: p_max_kw * 1.0,
        ramp_kw_s: p_max_kw / 10.0,
        reserve_up_kw: p_max_kw * 0.05,
        reserve_down_kw: p_max_kw * 0.05,
        price_curve_ref: BlobRefV1::new(1, b"price"),
        capability_root: id32(site_id),
        valid_until_ns: u64::MAX,
        revision,
    }
}

/// 站点侧对象：negotiator + signer。
struct SiteNode {
    neg: SiteNegotiator,
    signer: SiteSigner,
    offer: CapabilityOffer,
}

/// 构造 N 站联邦（协调器 + 站点），key 全局 id 空间。
fn build_federation(n: u32) -> (CoordinatorNegotiator, Vec<SiteNode>, Vec<Sm2PublicKey>) {
    let mut coord = CoordinatorNegotiator::new(0, 1, 7, 1);
    let mut sites = Vec::new();
    let mut pubkeys = Vec::new();
    for i in 1..=n {
        let key_id = 1000 + i;
        let (site_kp, rng) = gen_kp();
        coord
            .register_site_key(key_id, &site_kp.public_key, NOW)
            .expect("reg site key");
        pubkeys.push(site_kp.public_key);
        let mut neg = SiteNegotiator::new(i, 1, 1, 100 * i as u64);
        let (kp_c, _rng_c) = gen_kp();
        neg.register_peer_key_with_id(42, &kp_c.public_key, NOW)
            .expect("reg coord key");
        sites.push(SiteNode {
            neg,
            signer: SiteSigner::new(key_id, site_kp, rng),
            offer: site_offer(i, 1, 10.0 + (i % 90) as f64),
        });
    }
    (coord, sites, pubkeys)
}

/// 每站 p_max 容量（kW）。
fn site_pmax(site_id: u32) -> f64 {
    10.0 + (site_id % 90) as f64
}

// ---------------------------------------------------------------
// 1) 100 站 offer/commit/revision 全流程（FedNet 确定性驱动）
// ---------------------------------------------------------------

#[test]
fn hundred_sites_offer_commit_revision() {
    let n = 100u32;
    let (mut coord, mut sites, _) = build_federation(n);
    let mut net = FedNet::new(4096);
    net.register(0);
    for i in 1..=n {
        net.register(i);
    }
    let mut coord_signer = {
        let (kp, rng) = gen_kp();
        // 协调器 key 与站点侧登记的 id 一致。
        SiteSigner::new(42, kp, rng)
    };
    let _ = &mut coord_signer;

    // —— 所有站点发布 offer ——
    for s in sites.iter_mut() {
        let (_, env) = s
            .neg
            .publish_offer(&s.offer, NOW, &mut s.signer)
            .expect("publish");
        net.send(s.neg.site_id, 0, env).expect("send offer");
    }
    net.advance(1);
    net.drain_due();
    let mut offer_count = 0u32;
    for env in net.take_all(0) {
        let (out, _) = coord.on_message(&env, NOW + 1, &mut coord_signer);
        assert!(
            matches!(out, StepOutcome::OfferRegistered { .. }),
            "{out:?}"
        );
        offer_count += 1;
    }
    assert_eq!(offer_count, n);
    assert_eq!(coord.sessions().len(), n as usize);

    // —— 协调器求解并逐站提案 ——
    let target: Vec<f64> = vec![sum_capacity(&coord) * 0.5];
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
    let input = AllocationInput {
        interval_start_ns: 0,
        interval_ns: 3_600_000_000_000,
        sites: views,
        target: TargetProfile {
            p_target_kw: target.clone(),
            reserve_up_kw: 0.0,
            reserve_down_kw: 0.0,
        },
    };
    let alloc = match AllocationSolver::solve(&input).expect("solve") {
        AllocationResult::Feasible { targets, .. } => targets,
        other => panic!("expected feasible, got {other:?}"),
    };
    assert_eq!(alloc.len(), n as usize, "100 站全部分配");
    for t in &alloc {
        let o = coord
            .session(t.site_id)
            .and_then(|s| s.offer())
            .expect("offer");
        let (_, env) = coord
            .propose(
                t.site_id,
                &AllocationSolver::to_proposal(
                    &t.clone(),
                    o,
                    id16(t.site_id),
                    1,
                    0,
                    3_600_000_000_000,
                )
                .expect("proposal"),
                NOW + 2,
                &mut coord_signer,
            )
            .expect("propose");
        net.send(0, t.site_id, env).expect("send proposal");
    }
    net.advance(1);
    net.drain_due();
    let mut commitments = 0u32;
    for (i, s) in sites.iter_mut().enumerate() {
        let sid = (i + 1) as u32;
        let mut got = false;
        while let Some(env) = net.pop(sid) {
            let (out, outbox) = s.neg.on_message(&env, NOW + 3, &mut s.signer);
            assert!(
                matches!(out, StepOutcome::ProposalAccepted { .. }),
                "site {sid}: {out:?}"
            );
            for e in outbox {
                net.send(sid, 0, e).expect("send commitment");
            }
            got = true;
        }
        assert!(got, "site {sid} 必须收到提案");
        commitments += 1;
    }
    net.advance(1);
    net.drain_due();
    let mut acked = 0u32;
    for env in net.take_all(0) {
        let (out, ackbox) = coord.on_message(&env, NOW + 4, &mut coord_signer);
        assert!(
            matches!(out, StepOutcome::CommitmentConfirmed { .. }),
            "{out:?}"
        );
        assert_eq!(ackbox.len(), 1);
        for a in ackbox {
            net.send(0, env.site_id, a).expect("send ack");
        }
        acked += 1;
    }
    assert_eq!(commitments, n);
    assert_eq!(acked, n);
    // 所有站点收到 Ack。
    let mut acked_sites = 0u32;
    for (i, s) in sites.iter_mut().enumerate() {
        let sid = (i + 1) as u32;
        while let Some(env) = net.pop(sid) {
            let (out, _) = s.neg.on_message(&env, NOW + 5, &mut s.signer);
            assert!(
                matches!(out, StepOutcome::Acknowledged { .. }),
                "site {sid}: {out:?}"
            );
            acked_sites += 1;
        }
    }
    assert_eq!(acked_sites, n);

    // —— 修订波：1/3 站点能力突降（revision 2）→ 协调器记录新包络 ——
    let mut revised = 0u32;
    for (i, s) in sites.iter_mut().enumerate() {
        let sid = (i + 1) as u32;
        if sid % 3 == 0 {
            s.offer = site_offer(sid, 2, site_pmax(sid) * 0.5);
            let (_, env) = s
                .neg
                .publish_offer(&s.offer, NOW + 10, &mut s.signer)
                .expect("revise");
            net.send(sid, 0, env).expect("send revise");
            revised += 1;
        }
    }
    net.advance(1);
    net.drain_due();
    let mut superseded_or_registered = 0u32;
    for env in net.take_all(0) {
        let (out, _) = coord.on_message(&env, NOW + 11, &mut coord_signer);
        assert!(
            matches!(out, StepOutcome::OfferRegistered { revision: 2, .. }),
            "site {}: {out:?}",
            env.site_id
        );
        superseded_or_registered += 1;
    }
    assert_eq!(superseded_or_registered, revised);
    // 所有会话 offer revision == 2 或 1，且每个站点都有 offer。
    for (sid, s) in coord.sessions() {
        let o = s.offer().expect("offer 必须存在");
        assert!(o.revision == 1 || o.revision == 2);
        let _ = sid;
    }
    // 修订后重新求解：目标仍在容量内 → 可行；已有 commitment 不影响。
    let views2: Vec<SiteOfferView> = coord
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
    let target2 = sum_capacity_views(&views2) * 0.5;
    let inp2 = AllocationInput {
        interval_start_ns: 0,
        interval_ns: 3_600_000_000_000,
        sites: views2,
        target: TargetProfile {
            p_target_kw: vec![target2],
            reserve_up_kw: 0.0,
            reserve_down_kw: 0.0,
        },
    };
    // 目标 = 修订后容量 50% → 必须可行（不超过 Σp_max）。
    match AllocationSolver::solve(&inp2).expect("solve2") {
        AllocationResult::Feasible { .. } => {}
        other => panic!("修订后容量内目标必须可行: {other:?}"),
    }
    // 反例：目标超过修订后总容量 → 结构化 Infeasible，绝不 default-fill。
    let inp3 = AllocationInput {
        sites: inp2.sites.clone(),
        target: TargetProfile {
            p_target_kw: vec![sum_capacity_views(&inp2.sites) * 2.0],
            ..inp2.target
        },
        ..inp2
    };
    match AllocationSolver::solve(&inp3).expect("solve3") {
        AllocationResult::Infeasible(_) => {}
        other => panic!("超容量必须 Infeasible: {other:?}"),
    }
}

fn sum_capacity(coord: &CoordinatorNegotiator) -> f64 {
    coord
        .sessions()
        .values()
        .map(|s| s.offer().map(|o| o.p_max_kw).unwrap_or(0.0))
        .sum()
}

fn sum_capacity_views(views: &[SiteOfferView]) -> f64 {
    views.iter().map(|v| v.p_max_kw).sum()
}

// ---------------------------------------------------------------
// 2) 网络分区 + 部分站离线 + 站内继续
// ---------------------------------------------------------------

#[test]
fn partition_and_partial_offline_site_continues() {
    let n = 10u32;
    let (mut coord, mut sites, _) = build_federation(n);
    let mut net = FedNet::new(4096);
    net.register(0);
    for i in 1..=n {
        net.register(i);
    }
    let (kp_c, rng_c) = gen_kp();
    let mut coord_signer = SiteSigner::new(42, kp_c, rng_c);
    // 分区：站点 5 网络不可达（不注册）。
    net.partition(5);
    let mut sent = 0u32;
    for (i, s) in sites.iter_mut().enumerate() {
        let sid = (i + 1) as u32;
        if sid == 5 {
            continue; // 离线站点不发布 offer
        }
        let (_, env) = s
            .neg
            .publish_offer(&s.offer, NOW, &mut s.signer)
            .expect("pub");
        net.send(sid, 0, env).expect("send");
        sent += 1;
    }
    net.advance(1);
    net.drain_due();
    for env in net.take_all(0) {
        let (out, _) = coord.on_message(&env, NOW + 1, &mut coord_signer);
        assert!(matches!(out, StepOutcome::OfferRegistered { .. }));
    }
    assert_eq!(sent, n - 1);
    // 求解：9 站容量内目标可行。
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
    let target = sum_capacity_views(&views) * 0.5;
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
        other => panic!("{other:?}"),
    };
    assert_eq!(alloc_targets.len(), (n - 1) as usize, "离线站点不参与分配");
    // 向每个在线站点提案。
    for t in &alloc_targets {
        let o = coord
            .session(t.site_id)
            .and_then(|s| s.offer())
            .expect("offer");
        let (_, env) = coord
            .propose(
                t.site_id,
                &AllocationSolver::to_proposal(
                    &t.clone(),
                    o,
                    id16(t.site_id),
                    1,
                    0,
                    3_600_000_000_000,
                )
                .expect("proposal"),
                NOW + 2,
                &mut coord_signer,
            )
            .expect("propose");
        net.send(0, t.site_id, env).expect("send proposal");
    }
    net.advance(1);
    net.drain_due();
    // 在线站点接受。
    let mut accepted = 0u32;
    for (i, s) in sites.iter_mut().enumerate() {
        let sid = (i + 1) as u32;
        if sid == 5 {
            continue;
        }
        while let Some(env) = net.pop(sid) {
            let (out, outbox) = s.neg.on_message(&env, NOW + 3, &mut s.signer);
            assert!(
                matches!(out, StepOutcome::ProposalAccepted { .. }),
                "site {sid}: {out:?}"
            );
            for e in outbox {
                net.send(sid, 0, e).expect("send cmt");
            }
            accepted += 1;
        }
    }
    assert_eq!(accepted, n - 1);
    // 站点 5（离线站）独立自治：无协调器状态下本地会话可继续发布/修订
    // offer（站内控制权在站内，不依赖云）。
    let s5 = &mut sites[4];
    let (out, _) = s5
        .neg
        .publish_offer(&s5.offer.clone(), NOW + 9, &mut s5.signer)
        .expect("publish offline");
    assert!(
        matches!(out, StepOutcome::OfferRegistered { revision: 1, .. }),
        "{out:?}"
    );
}

// ---------------------------------------------------------------
// 3) 协调器重启 + 旧 leader 拒绝
// ---------------------------------------------------------------

#[test]
fn coordinator_restart_stale_leader_rejected() {
    let (mut coord, mut sites, _) = build_federation(3);
    let (kp_c, rng_c) = gen_kp();
    let mut coord_signer = SiteSigner::new(42, kp_c, rng_c);

    // 站点发布 offer。
    for s in sites.iter_mut() {
        let (_, env) = s
            .neg
            .publish_offer(&s.offer, NOW, &mut s.signer)
            .expect("pub");
        let (out, _) = coord.on_message(&env, NOW + 1, &mut coord_signer);
        assert!(matches!(out, StepOutcome::OfferRegistered { .. }));
    }
    // 记录 journal 供恢复。
    let journal = coord.journal().clone();
    let keys = coord.keys().clone();

    // 崩溃 → 重启（epoch 前跳）。
    let max_epoch = journal.records.iter().map(|r| r.epoch).max().unwrap_or(0);
    let mut coord2 = CoordinatorNegotiator::new(0, 1, max_epoch + 1, 1000);
    coord2.restore(&journal, keys);
    assert_eq!(coord2.sessions().len(), 3);
    assert!(coord2.session(1).expect("s1").offer().is_some());

    // 旧 leader 拒绝路径：站点注册协调器 key（kp_c3）；同一把 key 分别以
    // 新纪元（coord2.epoch=8，先到）与旧纪元（7，后到）签名——窗口必须
    // 拒绝旧纪元。
    let (kp_site, rng_site) = gen_kp();
    let (kp_c3, rng_c3) = gen_kp();
    let mut fresh_site = SiteNegotiator::new(9, 1, 9, 100);
    fresh_site
        .register_peer_key_with_id(42, &kp_c3.public_key, NOW)
        .expect("reg");
    let mut fresh_signer = SiteSigner::new(1009, kp_site, rng_site);
    let mut new_epoch_signer = SiteSigner::new(42, kp_c3.clone(), CsRng::new());
    let mut stale_signer = SiteSigner::new(42, kp_c3, rng_c3);
    // 重启后的协调器需要恢复该新站点的密钥登记（PKI 静态配置）。
    coord2
        .register_site_key(1009, &fresh_signer.public_key().clone(), NOW)
        .expect("reg 1009");
    let (_, env) = fresh_site
        .publish_offer(&site_offer(9, 1, 50.0), NOW, &mut fresh_signer)
        .expect("pub");
    // 新协调器先收 offer（epoch = max+1，即 8）。
    let (out, _) = coord2.on_message(&env, NOW + 2, &mut new_epoch_signer);
    assert!(
        matches!(out, StepOutcome::OfferRegistered { .. }),
        "{out:?}"
    );
    // 站点先见新纪元 proposal（epoch 8，窗口锚定）。
    let alloc = eneros_federation::allocation::SiteAllocation {
        site_id: 9,
        points_kw: vec![20.0],
        energy_kwh: 20.0,
        reserve_up_kw: 1.0,
        reserve_down_kw: 1.0,
        within_envelope: true,
    };
    let proposal = AllocationSolver::to_proposal(
        &alloc,
        &site_offer(9, 1, 50.0),
        [2; 16],
        1,
        0,
        3_600_000_000_000,
    )
    .expect("p");
    let new_epoch = coord2.epoch.max(8); // 新纪元必须高于旧纪元（这里旧=7）
    let fresh_env = new_epoch_signer.sign(
        &FedBody::Proposal(proposal.clone()),
        &EnvelopeHdr {
            epoch: new_epoch,
            sequence: 6,
            tenant_id: 1,
            site_id: 0,
            created_ns: NOW,
            deadline_ns: NOW + 3_600_000_000_000,
            signer_key_id: 42,
        },
    );
    let (out, _) = fresh_site.on_message(&fresh_env, NOW + 3, &mut fresh_signer);
    assert!(
        matches!(out, StepOutcome::ProposalAccepted { .. }),
        "{out:?}"
    );
    // 旧纪元 leader（epoch 7，同 key）→ 站点窗口拒绝。
    let stale_env = stale_signer.sign(
        &FedBody::Proposal(proposal),
        &EnvelopeHdr {
            epoch: 7, // 旧纪元
            sequence: 5,
            tenant_id: 1,
            site_id: 0,
            created_ns: NOW,
            deadline_ns: NOW + 3_600_000_000_000,
            signer_key_id: 42,
        },
    );
    let (out, _) = fresh_site.on_message(&stale_env, NOW + 4, &mut fresh_signer);
    assert!(
        matches!(out, StepOutcome::StaleLeaderRejected { .. }),
        "{out:?}"
    );
}

// ---------------------------------------------------------------
// 4) 证书撤销：撤销后站点消息被拒，其他站点不受影响
// ---------------------------------------------------------------

#[test]
fn cert_revocation_isolated() {
    let n = 5u32;
    let (mut coord, mut sites, _) = build_federation(n);
    let (kp_c, rng_c) = gen_kp();
    let mut coord_signer = SiteSigner::new(42, kp_c, rng_c);
    // 站点发布 offer。
    for s in sites.iter_mut() {
        let (_, env) = s
            .neg
            .publish_offer(&s.offer, NOW, &mut s.signer)
            .expect("pub");
        let (out, _) = coord.on_message(&env, NOW + 1, &mut coord_signer);
        assert!(matches!(out, StepOutcome::OfferRegistered { .. }));
    }
    // 撤销站点 2 的 key。
    coord.revoke_site_key(1002, NOW + 2).expect("revoke");
    // 站点 2 再次发消息（修订 offer）→ 拒。
    let s2 = &mut sites[1];
    let (_, env) = s2
        .neg
        .publish_offer(&site_offer(2, 2, 20.0), NOW + 3, &mut s2.signer)
        .expect("pub");
    let (out, _) = coord.on_message(&env, NOW + 4, &mut coord_signer);
    assert!(matches!(out, StepOutcome::SignatureRejected), "{out:?}");
    assert_eq!(coord.signature_rejected, 1);
    // 其他站点不受影响：站点 1 正常修订被接受。
    let s1 = &mut sites[0];
    let (_, env) = s1
        .neg
        .publish_offer(&site_offer(1, 2, 30.0), NOW + 5, &mut s1.signer)
        .expect("pub");
    let (out, _) = coord.on_message(&env, NOW + 6, &mut coord_signer);
    assert!(
        matches!(out, StepOutcome::OfferRegistered { .. }),
        "{out:?}"
    );
}

// ---------------------------------------------------------------
// 5) 跨租户隔离
// ---------------------------------------------------------------

#[test]
fn cross_tenant_isolation() {
    let (kp_site, rng_site) = gen_kp();
    let (kp_c, rng_c) = gen_kp();
    let mut coord = CoordinatorNegotiator::new(0, 1, 1, 1);
    coord
        .register_site_key(111, &kp_site.public_key, NOW)
        .expect("reg");
    let mut site = SiteNegotiator::new(9, 2, 1, 100); // 租户 2
    site.register_peer_key_with_id(42, &kp_c.public_key, NOW)
        .expect("reg coord");
    let mut site_signer = SiteSigner::new(111, kp_site, rng_site);
    let mut coord_signer = SiteSigner::new(42, kp_c, rng_c);
    // 租户 2 站点向租户 1 协调器发消息 → 拒。
    let (_, env) = site
        .publish_offer(&site_offer(9, 1, 50.0), NOW, &mut site_signer)
        .expect("pub");
    let (out, _) = coord.on_message(&env, NOW + 1, &mut coord_signer);
    assert!(matches!(out, StepOutcome::TenantMismatch), "{out:?}");
    assert_eq!(coord.tenant_rejected, 1);
    // 反向：协调器租户消息被站点拒绝。
    let alloc = eneros_federation::allocation::SiteAllocation {
        site_id: 9,
        points_kw: vec![10.0],
        energy_kwh: 10.0,
        reserve_up_kw: 0.0,
        reserve_down_kw: 0.0,
        within_envelope: true,
    };
    let proposal = AllocationSolver::to_proposal(
        &alloc,
        &site_offer(9, 1, 50.0),
        [3; 16],
        1,
        0,
        3_600_000_000_000,
    )
    .expect("p");
    let env2 = coord_signer.sign(
        &FedBody::Proposal(proposal),
        &EnvelopeHdr {
            epoch: 1,
            sequence: 1,
            tenant_id: 1,
            site_id: 0,
            created_ns: NOW,
            deadline_ns: NOW + 3_600_000_000_000,
            signer_key_id: 42,
        },
    );
    let (out, _) = site.on_message(&env2, NOW + 2, &mut site_signer);
    assert!(matches!(out, StepOutcome::TenantMismatch), "{out:?}");
}

// ---------------------------------------------------------------
// 6) 云发恶意超范围目标 → 站点拒绝且站内继续
// ---------------------------------------------------------------

#[test]
fn malicious_cloud_target_rejected_site_continues() {
    let (mut coord, mut sites, _) = build_federation(1);
    let (kp_c, rng_c) = gen_kp();
    let mut coord_signer = SiteSigner::new(42, kp_c, rng_c);
    let s = &mut sites[0];
    let (_, env) = s
        .neg
        .publish_offer(&s.offer, NOW, &mut s.signer)
        .expect("pub");
    // 站点包络 p_max = site_pmax(1) = 10 + 1%90 = 11。
    assert_eq!(site_pmax(1), 11.0);
    let (out, _) = coord.on_message(&env, NOW + 1, &mut coord_signer);
    assert!(matches!(out, StepOutcome::OfferRegistered { .. }));
    // 恶意云目标：800 kW >> 包络。
    let alloc = eneros_federation::allocation::SiteAllocation {
        site_id: 1,
        points_kw: vec![800.0],
        energy_kwh: 800.0,
        reserve_up_kw: 0.0,
        reserve_down_kw: 0.0,
        within_envelope: true,
    };
    let proposal =
        AllocationSolver::to_proposal(&alloc, &s.offer.clone(), [4; 16], 1, 0, 3_600_000_000_000)
            .expect("p");
    let (_, penv) = coord
        .propose(1, &proposal, NOW + 2, &mut coord_signer)
        .expect("propose");
    let (out, outbox) = s.neg.on_message(&penv, NOW + 3, &mut s.signer);
    match out {
        StepOutcome::ProposalRejected {
            reason: RejectReason::OutOfEnvelope,
            ..
        } => {}
        other => panic!("恶意目标必须结构化拒绝: {other:?}"),
    }
    assert_eq!(outbox.len(), 1, "结构化拒绝必须出站");
    // 站内继续：同一站点随后可接受合法目标；拒绝事件被记录且不影响本地状态。
    assert!(s.neg.current_offer().is_some());
    let legal = eneros_federation::allocation::SiteAllocation {
        site_id: 1,
        points_kw: vec![5.0],
        energy_kwh: 5.0,
        reserve_up_kw: 0.2,
        reserve_down_kw: 0.2,
        within_envelope: true,
    };
    let p2 =
        AllocationSolver::to_proposal(&legal, &s.offer.clone(), [5; 16], 1, 0, 3_600_000_000_000)
            .expect("p2");
    let (_, penv2) = coord
        .propose(1, &p2, NOW + 4, &mut coord_signer)
        .expect("propose2");
    let (out, _) = s.neg.on_message(&penv2, NOW + 5, &mut s.signer);
    assert!(
        matches!(out, StepOutcome::ProposalAccepted { .. }),
        "{out:?}"
    );
}

// ---------------------------------------------------------------
// 7) 重放 + 幂等：重复消息不双签承诺
// ---------------------------------------------------------------

#[test]
fn replay_no_double_commitment() {
    let (mut coord, mut sites, _) = build_federation(3);
    let (kp_c, rng_c) = gen_kp();
    let mut coord_signer = SiteSigner::new(42, kp_c, rng_c);
    let mut proposals: Vec<_> = Vec::new();
    for (i, s) in sites.iter_mut().enumerate() {
        let sid = (i + 1) as u32;
        let (_, env) = s
            .neg
            .publish_offer(&s.offer, NOW, &mut s.signer)
            .expect("pub");
        let (out, _) = coord.on_message(&env, NOW + 1, &mut coord_signer);
        assert!(matches!(out, StepOutcome::OfferRegistered { .. }));
        let alloc = eneros_federation::allocation::SiteAllocation {
            site_id: sid,
            points_kw: vec![5.0],
            energy_kwh: 5.0,
            reserve_up_kw: 0.0,
            reserve_down_kw: 0.0,
            within_envelope: true,
        };
        let o = s.offer.clone();
        let proposalt =
            AllocationSolver::to_proposal(&alloc, &o, [sid as u8; 16], 1, 0, 3_600_000_000_000)
                .expect("p");
        let (_, penv) = coord
            .propose(sid, &proposalt, NOW + 2, &mut coord_signer)
            .expect("propose");
        proposals.push(penv.clone());
        // 站点接受。
        let (out, outbox) = s.neg.on_message(&penv, NOW + 3, &mut s.signer);
        assert!(matches!(out, StepOutcome::ProposalAccepted { .. }));
        assert_eq!(outbox.len(), 1);
        // 协调器确认。
        let (out, ackbox) = coord.on_message(&outbox[0], NOW + 4, &mut coord_signer);
        assert!(matches!(out, StepOutcome::CommitmentConfirmed { .. }));
        assert_eq!(ackbox.len(), 1);
        // 重放同一 commitment → 协调器拒（不重复确认、不重复 ack）。
        let (out, ackbox2) = coord.on_message(&outbox[0], NOW + 5, &mut coord_signer);
        assert!(matches!(out, StepOutcome::ReplayRejected { .. }), "{out:?}");
        assert!(ackbox2.is_empty());
        // 站点侧重放同一 proposal → 拒（commitment 不重复签发）。
        let (out, outbox2) = s.neg.on_message(&proposals[i], NOW + 6, &mut s.signer);
        assert!(matches!(out, StepOutcome::ReplayRejected { .. }), "{out:?}");
        assert!(outbox2.is_empty());
    }
}

// ---------------------------------------------------------------
// 8) 证据链：progress/settlement 只上聚合（数据最小化）
// ---------------------------------------------------------------

#[test]
fn evidence_aggregate_only() {
    let (mut coord, mut sites, _) = build_federation(1);
    let (kp_c, rng_c) = gen_kp();
    let mut coord_signer = SiteSigner::new(42, kp_c, rng_c);
    let s = &mut sites[0];
    let (_, env) = s
        .neg
        .publish_offer(&s.offer, NOW, &mut s.signer)
        .expect("pub");
    let (out, _) = coord.on_message(&env, NOW + 1, &mut coord_signer);
    assert!(matches!(out, StepOutcome::OfferRegistered { .. }));
    let alloc = eneros_federation::allocation::SiteAllocation {
        site_id: 1,
        points_kw: vec![5.0],
        energy_kwh: 5.0,
        reserve_up_kw: 0.0,
        reserve_down_kw: 0.0,
        within_envelope: true,
    };
    let proposal =
        AllocationSolver::to_proposal(&alloc, &s.offer.clone(), [6; 16], 1, 0, 3_600_000_000_000)
            .expect("p");
    let (_, penv) = coord
        .propose(1, &proposal, NOW + 2, &mut coord_signer)
        .expect("propose");
    let (_, cbox) = s.neg.on_message(&penv, NOW + 3, &mut s.signer);
    let (_, _) = coord.on_message(&cbox[0], NOW + 4, &mut coord_signer);
    // 站点提交进度 + 结算。
    let cid = s.neg.commitment().expect("cmt").commitment.commitment_id;
    let prog = ProgressEvidence {
        commitment_id: cid,
        site_id: 1,
        interval_start_ns: 0,
        interval_end_ns: 3_600_000_000_000,
        delivered_kwh: 5.0,
        deviation_kw: 0.0,
        evidence_ref: BlobRefV1::new(3, b"ev"),
        seq: 1,
    };
    let (_, pv) = s
        .neg
        .submit_progress(&prog, NOW + 5, &mut s.signer)
        .expect("prog");
    // wire 大小与点位无关（数据最小化）——progress 帧长度固定。
    let frame_len = eneros_federation::offer::FedFrame::encode(&pv)
        .expect("enc")
        .len();
    let (out, _) = coord.on_message(&pv, NOW + 6, &mut coord_signer);
    assert!(matches!(out, StepOutcome::EvidenceRecorded { .. }));
    let set = SettlementEvidence {
        commitment_id: cid,
        site_id: 1,
        settle_start_ns: 0,
        settle_end_ns: 3_600_000_000_000,
        measured_milli_kwh: 5_000,
        quality_bp: 10_000,
        note: b"x".to_vec(),
    };
    let (_, sv) = s
        .neg
        .submit_settlement(&set, NOW + 7, &mut s.signer)
        .expect("set");
    let (out, _) = coord.on_message(&sv, NOW + 8, &mut coord_signer);
    assert!(matches!(out, StepOutcome::EvidenceRecorded { .. }));
    assert_eq!(coord.session(1).expect("s").evidence_progress.len(), 1);
    assert_eq!(coord.session(1).expect("s").evidence_settlement.len(), 1);
    // 记录帧长（寄存器）：证据帧为常值（BlobRef 定长），断言其有界。
    assert!(frame_len < 512, "progress 帧有界");
}

// ---------------------------------------------------------------
// 9) 距离/时延：100 站一轮往返的确定性边界（压力健康度）
// ---------------------------------------------------------------

#[test]
fn hundred_site_roundtrip_bounded() {
    let n = 100u32;
    let (mut coord, mut sites, _) = build_federation(n);
    let mut net = FedNet::new(4096);
    net.register(0);
    for i in 1..=n {
        net.register(i);
    }
    let (kp_c, rng_c) = gen_kp();
    let mut coord_signer = SiteSigner::new(42, kp_c, rng_c);
    let t0 = std::time::Instant::now();
    for s in sites.iter_mut() {
        let (_, env) = s
            .neg
            .publish_offer(&s.offer, NOW, &mut s.signer)
            .expect("pub");
        net.send(s.neg.site_id, 0, env).expect("send");
    }
    net.advance(1);
    net.drain_due();
    let mut offers = net.take_all(0);
    assert_eq!(offers.len(), n as usize);
    for env in offers.drain(..) {
        let (out, _) = coord.on_message(&env, NOW + 1, &mut coord_signer);
        assert!(matches!(out, StepOutcome::OfferRegistered { .. }));
    }
    // 全部提案。
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
    let target = sum_capacity_views(&views) * 0.5;
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
    let alloc = match AllocationSolver::solve(&inp).expect("solve") {
        AllocationResult::Feasible { targets, .. } => targets,
        other => panic!("{other:?}"),
    };
    for t in &alloc {
        let o = coord
            .session(t.site_id)
            .and_then(|s| s.offer())
            .expect("offer");
        let (_, env) = coord
            .propose(
                t.site_id,
                &AllocationSolver::to_proposal(
                    &t.clone(),
                    o,
                    id16(t.site_id),
                    1,
                    0,
                    3_600_000_000_000,
                )
                .expect("proposal"),
                NOW + 2,
                &mut coord_signer,
            )
            .expect("propose");
        net.send(0, t.site_id, env).expect("send p");
    }
    net.advance(1);
    net.drain_due();
    for (i, s) in sites.iter_mut().enumerate() {
        let sid = (i + 1) as u32;
        while let Some(env) = net.pop(sid) {
            let (_, outbox) = s.neg.on_message(&env, NOW + 3, &mut s.signer);
            for e in outbox {
                net.send(sid, 0, e).expect("send c");
            }
        }
    }
    let elapsed = t0.elapsed();
    // 100 站全流程（含 SM2 签名×N）在确定性内完成（机器学习无界等待禁令）。
    assert!(
        elapsed.as_secs() < 60,
        "100 站全流程必须在 60s 内（实测 {}ms）",
        elapsed.as_millis()
    );
}
