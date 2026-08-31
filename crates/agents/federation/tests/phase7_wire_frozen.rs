//! WP-7.3：RC 冻结面（蓝图 61 §12 §4.2「API/wire/config/catalog/model/skill
//! schema 冻结；只修 P0/P1」）。
//!
//! 本测试是联邦 wire schema 的冻结门：`WIRE_VERSION`、帧上限、消息种类
//! 清单、字段布局常量一旦改变（非 P0/P1 修复），本测试必须红——任何新增
//! 功能都只能通过 bump WIRE_VERSION + ADR 走开发轨道，不得静默改 v1。
//!
//! 冻结登记（v0.314.48 → v0.314.49 期间）：全部常量未变、无新消息种类、
//! 无新字段——由本测试的常量现值 + git diff 空共同证明。

use eneros_federation::offer::{
    FedFrame, FedKind, MAX_FRAME, MAX_INTERVALS, MAX_SITES_PER_PROPOSAL, WIRE_VERSION,
};

#[test]
fn wire_schema_frozen_at_v1() {
    // v0.314.49 冻结快照（任何改动须先过 ADR + bump version）。
    assert_eq!(WIRE_VERSION, 1, "wire 版本冻结于 1");
    assert_eq!(MAX_FRAME, 65_536, "帧上限冻结");
    assert_eq!(MAX_INTERVALS, 288, "区间数上限冻结（96/288 时段资格）");
    assert_eq!(MAX_SITES_PER_PROPOSAL, 1, "单 proposal 单站点冻结");
}

#[test]
fn message_kind_set_frozen() {
    // 消息种类清单冻结（§2.2 产物 + 流程消息；不得静默扩展）。
    let kinds = [
        FedKind::Offer,
        FedKind::Reject,
        FedKind::Proposal,
        FedKind::Commitment,
        FedKind::Ack,
        FedKind::Progress,
        FedKind::Settlement,
        FedKind::Revoke,
    ];
    for k in kinds {
        let b = k as u8;
        assert!((1..=8).contains(&b), "kind 域冻结在 1..=8");
        assert_eq!(FedKind::from_u8(b).expect("roundtrip"), k);
    }
    // 域外全部拒绝（fail-closed）。
    for b in [0u8, 9, 0x7F, 0xFF] {
        assert!(
            FedKind::from_u8(b).is_err(),
            "kind {b} 必须拒绝（reserved 非零拒绝纪律）"
        );
    }
}

#[test]
fn frame_layout_frozen() {
    // canonical 头字节序/字段顺序冻结：MAGIC(4)+ver(1)+kind(1)+epoch(8)
    // +seq(8)+tenant(4)+site(4)+created(8)+deadline(8)+bodylen(4)+digest(32)
    // +keyid(4) = 86 字节头 + body + 64 字节签名。
    // （布局由编码函数内联；此处用空 body 探测头长度，防布局漂移。）
    let canon =
        eneros_federation::offer::encode_canonical(FedKind::Offer, 0, 0, 0, 0, 0, 0, &[], 0)
            .expect("canonical");
    assert_eq!(canon.len(), 86, "canonical 头长度冻结");
    // 分层语义：帧层只验完整性（空 body digest 正确 → 解码通过）；
    // 体层业务校验拒绝（fail-closed 的第二层）。
    let frame = [&canon[..], &[0u8; 64]].concat();
    let env = FedFrame::decode(&frame).expect("帧层完整性通过");
    assert!(
        eneros_federation::offer::decode_body(env.kind, &env.body).is_err(),
        "空 body 在体层必须拒绝"
    );
}

#[test]
fn offer_field_set_frozen() {
    // 报价字段清单冻结：offer 现含 16 个定长字段 + 2 引用（价格曲线/
    // 能力根）+ revision——数据最小化测试已锚定定长体；此处锚定大小。
    let o = sample_offer();
    let bytes = o.encode().expect("encode");
    // 定长体：id16 + start8 + end8 + 7×f64(56) + ref41 + root32 + valid8
    //          + rev8 = 32+56+41+32+16 = 177。
    assert_eq!(bytes.len(), 177, "offer 定长体冻结（无点位字段）");
}

fn sample_offer() -> eneros_federation::offer::CapabilityOffer {
    use eneros_federation::offer::BlobRefV1;
    eneros_federation::offer::CapabilityOffer {
        offer_id: [1; 16],
        interval_start_ns: 0,
        interval_end_ns: 3_600_000_000_000,
        p_min_kw: 0.0,
        p_max_kw: 100.0,
        energy_min_kwh: 0.0,
        energy_max_kwh: 100.0,
        ramp_kw_s: 10.0,
        reserve_up_kw: 5.0,
        reserve_down_kw: 5.0,
        price_curve_ref: BlobRefV1::new(1, b"p"),
        capability_root: [2; 32],
        valid_until_ns: u64::MAX,
        revision: 1,
    }
}
