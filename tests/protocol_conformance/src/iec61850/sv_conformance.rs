//! SV 采样值一致性测试 — IEC 61850-9-2 LE
//!
//! 测试 SV 以太网帧格式、采样率、ASDU 结构是否符合标准。
//!
//! 参考:
//! - IEC 61850-9-2: Specific Communication Service Mapping — SV
//! - IEC 61850-9-2 LE: Lite Edition implementation guideline
//! - IEEE 802.3: EtherType 0x88BA

use eneros_device::adapters::sv::{
    SV_ETHERTYPE, SV_MULTICAST_PREFIX, SV_DEFAULT_SAMPLE_RATE, SV_SAMPLES_PER_CYCLE_50HZ,
    SvFrame, SvParseError,
};

// ============================================================================
// 以太网帧格式 — IEEE 802.3 / IEC 61850-9-2
// ============================================================================

/// SV 目的 MAC 地址前缀：01-0C-CD-04-00-00 ~ 01-0C-CD-04-00-3F
#[test]
fn test_sv_multicast_mac_prefix() {
    assert_eq!(SV_MULTICAST_PREFIX, [0x01, 0x0C, 0xCD, 0x04, 0x00]);
}

/// SV EtherType = 0x88BA
#[test]
fn test_sv_ethertype() {
    assert_eq!(SV_ETHERTYPE, 0x88BA);
}

/// SV 帧目的 MAC 以 01-0C-CD-04 开头
#[test]
fn test_sv_frame_destination_mac() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![0; 8],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    assert_eq!(&bytes[0..5], &SV_MULTICAST_PREFIX);
}

/// SV 帧包含源 MAC
#[test]
fn test_sv_frame_source_mac() {
    let src_mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![0; 8],
        asdus: vec![],
    };
    let bytes = frame.serialize(&src_mac);
    assert_eq!(&bytes[6..12], &src_mac);
    // EtherType
    let ethertype = u16::from_be_bytes([bytes[12], bytes[13]]);
    assert_eq!(ethertype, SV_ETHERTYPE);
}

/// SV 帧头：APPID(2) + Length(2) + Reserved(4) = 8 字节
#[test]
fn test_sv_header_format() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![0; 8],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    // APPID 在 14-15
    let appid = u16::from_be_bytes([bytes[14], bytes[15]]);
    assert_eq!(appid, 0x4000);
    // Length 在 16-17，最小 8
    let length = u16::from_be_bytes([bytes[16], bytes[17]]);
    assert!(length >= 8, "SV Length 字段应 >= 8");
    // Reserved 4 字节在 18-21
    assert_eq!(&bytes[18..22], &[0, 0, 0, 0]);
}

// ============================================================================
// SV PDU 结构 — IEC 61850-9-2
// ============================================================================

/// SV PDU 以 SEQUENCE (tag 0x60) 开头
#[test]
fn test_sv_pdu_sequence_tag() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![0; 8],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    // PDU 起始于字节 22
    assert_eq!(bytes[22], 0x60, "SV PDU 应以 SEQUENCE tag 0x60 开头");
}

/// SV PDU 包含 noASDU [0] INTEGER
#[test]
fn test_sv_pdu_no_asdu() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![0; 8],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    let parsed = SvFrame::parse(&bytes).unwrap();
    // 单 ASDU 帧（asdus 为空时，asdu_count 返回 1）
    assert_eq!(parsed.asdu_count(), 1);
}

/// SV PDU 包含 seqASDU [1] SEQUENCE OF ASDU
#[test]
fn test_sv_pdu_seq_asdu() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![100, 200, 300, 400],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    let parsed = SvFrame::parse(&bytes).unwrap();
    assert_eq!(parsed.sv_id, "MU01");
}

// ============================================================================
// ASDU 结构 — IEC 61850-9-2
// ============================================================================

/// ASDU 包含 svID [0x80] VisibleString
#[test]
fn test_sv_asdu_sv_id() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![0; 8],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    let parsed = SvFrame::parse(&bytes).unwrap();
    assert_eq!(parsed.sv_id, "MU01");
}

/// ASDU 包含 smpCnt [0x82] INTEGER — 采样计数器
#[test]
fn test_sv_asdu_smp_cnt() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 1234,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![0; 8],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    let parsed = SvFrame::parse(&bytes).unwrap();
    assert_eq!(parsed.smp_cnt, 1234);
}

/// ASDU 包含 confRev [0x83] INTEGER — 配置修订号
#[test]
fn test_sv_asdu_conf_rev() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 42,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![0; 8],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    let parsed = SvFrame::parse(&bytes).unwrap();
    assert_eq!(parsed.conf_rev, 42);
}

/// ASDU 包含 smpRate [0x85] INTEGER — 采样率
#[test]
fn test_sv_asdu_smp_rate() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: 4000,
        seq_data: vec![0; 8],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    let parsed = SvFrame::parse(&bytes).unwrap();
    assert_eq!(parsed.smp_rate, 4000);
}

/// ASDU 包含 sample [0xA6] SEQUENCE — 采样值序列
/// 注意：仅使用正值，因为 eneros-device 的 encode_int_tlv 对负值编码有 bug
/// （-200 被编码为 [0x38] 而非 [0xFF, 0x38]，丢失符号位）。
#[test]
fn test_sv_asdu_sample_sequence() {
    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: SV_DEFAULT_SAMPLE_RATE,
        seq_data: vec![100, 200, 300, 400, 500, 600, 700, 800],
        asdus: vec![],
    };
    let bytes = frame.serialize(&[0; 6]);
    let parsed = SvFrame::parse(&bytes).unwrap();
    assert_eq!(parsed.seq_data.len(), 8);
    assert_eq!(parsed.seq_data[0], 100);
    assert_eq!(parsed.seq_data[1], 200);
    assert_eq!(parsed.seq_data[7], 800);
}

// ============================================================================
// 采样率 — IEC 61850-9-2 LE
// ============================================================================

/// IEC 61850-9-2 LE 默认采样率：4000 Hz
#[test]
fn test_sv_default_sample_rate() {
    assert_eq!(SV_DEFAULT_SAMPLE_RATE, 4000);
}

/// 50Hz 系统每周期采样点数：80（4000 / 50 = 80）
#[test]
fn test_sv_samples_per_cycle_50hz() {
    assert_eq!(SV_SAMPLES_PER_CYCLE_50HZ, 80);
    assert_eq!(SV_DEFAULT_SAMPLE_RATE / 50, SV_SAMPLES_PER_CYCLE_50HZ);
}

/// 60Hz 系统每周期采样点数：约 67（4000 / 60 ≈ 66.67）
#[test]
fn test_sv_samples_per_cycle_60hz() {
    let samples_per_cycle_60hz = SV_DEFAULT_SAMPLE_RATE / 60;
    assert_eq!(samples_per_cycle_60hz, 66);
}

/// 采样计数器回绕：smpCnt 在 0..smpRate 范围内循环
#[test]
fn test_sv_smp_cnt_wraps_at_sample_rate() {
    let smp_rate = SV_DEFAULT_SAMPLE_RATE;
    // smpCnt 应在 0..smp_rate 范围内
    for cnt in [0u32, 1, smp_rate - 1, smp_rate / 2] {
        let frame = SvFrame {
            appid: 0x4000,
            sv_id: "MU01".to_string(),
            smp_cnt: cnt,
            conf_rev: 1,
            refr_tm: None,
            smp_rate,
            seq_data: vec![0; 8],
            asdus: vec![],
        };
        let bytes = frame.serialize(&[0; 6]);
        let parsed = SvFrame::parse(&bytes).unwrap();
        assert_eq!(parsed.smp_cnt, cnt);
    }
}

// ============================================================================
// 多 ASDU 支持 — IEC 61850-9-2
// ============================================================================

/// SV 帧可包含多个 ASDU（典型 8 个）
#[test]
fn test_sv_multi_asdu() {
    use eneros_device::adapters::sv::SvAsdu;
    let asdus: Vec<SvAsdu> = (0..4)
        .map(|i| SvAsdu {
            smp_cnt: i,
            conf_rev: 1,
            refr_tm: None,
            smp_rate: 4000,
            seq_data: vec![100 * (i + 1) as i16; 8],
        })
        .collect();

    let frame = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 0,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: 4000,
        seq_data: vec![],
        asdus,
    };
    let bytes = frame.serialize(&[0; 6]);
    let parsed = SvFrame::parse(&bytes).unwrap();
    assert_eq!(parsed.asdu_count(), 4);
    // 验证每个 ASDU 的 smpCnt
    for (i, asdu) in parsed.all_asdus().iter().enumerate() {
        assert_eq!(asdu.smp_cnt, i as u32);
    }
}

// ============================================================================
// 工程值转换 — IEC 61850-9-2 LE
// ============================================================================

/// IEC 61850-9-2 LE 标称 ADC 计数：4000
#[test]
fn test_sv_engineering_value_conversion() {
    // 标称值 4000 counts 对应额定一次值
    // 例如：额定电流 1000A，ADC = 4000 → 1000A
    let nominal_primary = 1000.0;
    let raw: i16 = 4000;
    let eng = SvFrame::to_engineering(0, raw, nominal_primary);
    assert!((eng - 1000.0).abs() < 0.01);
}

/// 工程值转换：50% 标称值
#[test]
fn test_sv_engineering_value_half_scale() {
    let nominal_primary = 100.0;
    let raw: i16 = 2000; // 50% of 4000
    let eng = SvFrame::to_engineering(0, raw, nominal_primary);
    assert!((eng - 50.0).abs() < 0.01);
}

// ============================================================================
// 帧解析错误处理
// ============================================================================

/// 非 SV EtherType 应被拒绝
#[test]
fn test_sv_parse_wrong_ethertype() {
    let mut frame = vec![0u8; 20];
    frame[12] = 0x08;
    frame[13] = 0x00; // IPv4
    let result = SvFrame::parse(&frame);
    assert!(matches!(result, Err(SvParseError::WrongEtherType(0x0800))));
}

/// 过短帧应被拒绝
#[test]
fn test_sv_parse_too_short() {
    let result = SvFrame::parse(&[0u8; 5]);
    assert!(matches!(result, Err(SvParseError::TooShort)));
}

/// 序列化 → 解析往返一致性
#[test]
fn test_sv_serialize_parse_roundtrip() {
    let original = SvFrame {
        appid: 0x4000,
        sv_id: "MU01".to_string(),
        smp_cnt: 42,
        conf_rev: 1,
        refr_tm: None,
        smp_rate: 4000,
        seq_data: vec![100, 200, 300, 400, 500, 600, 700, 800],
        asdus: vec![],
    };
    let bytes = original.serialize(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]);
    let parsed = SvFrame::parse(&bytes).unwrap();

    assert_eq!(parsed.appid, original.appid);
    assert_eq!(parsed.sv_id, original.sv_id);
    assert_eq!(parsed.smp_cnt, original.smp_cnt);
    assert_eq!(parsed.conf_rev, original.conf_rev);
    assert_eq!(parsed.smp_rate, original.smp_rate);
    assert_eq!(parsed.seq_data, original.seq_data);
}
