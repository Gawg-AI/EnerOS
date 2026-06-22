//! IEC 60870-5-104 ASDU 类型一致性测试 — IEC 60870-5-101/104
//!
//! 测试 ASDU 类型标识、可变结构限定词、传送原因、公共地址、信息体格式
//! 是否符合 IEC 60870-5-101 标准。
//!
//! 参考:
//! - IEC 60870-5-101 §7: ASDU 定义
//! - IEC 60870-5-104 §8: ASDU 传输

use eneros_device::adapters::iec104::asdu::{
    parse_asdu, build_single_command, build_double_command,
    build_interrogation_command, build_setpoint_short_float,
    build_clock_sync_command, build_parameter_float, build_parameter_scaled,
    TypeId, CauseOfTransmission, InformationObject,
    DoublePointValue, SinglePointQuality, MeasuredQuality,
};

// ============================================================================
// M_SP_NA_1 — 单点信息 (Type 1, 0x01)
// ============================================================================

/// M_SP_NA_1 类型标识 = 0x01
#[test]
fn test_m_sp_na_1_type_identifier() {
    assert_eq!(TypeId::SinglePoint.to_u8(), 0x01);
    assert_eq!(TypeId::from_u8(0x01), TypeId::SinglePoint);
}

/// M_SP_NA_1 ASDU 解析：SIQ 字节编码 SPI(bit0) + BL/SB/NT/IV
#[test]
fn test_m_sp_na_1_asdu_parsing() {
    // 构造 M_SP_NA_1 ASDU:
    //   TI=0x01, SQ=0/Num=1, COT=3(自发), OA=0, ASDU地址=1
    //   IOA=100(3B小端), SIQ=0x01(SPI=1, 质量好)
    let buf: Vec<u8> = vec![
        0x01, 0x01, 0x03, 0x00, 0x01, 0x00,
        0x64, 0x00, 0x00, // IOA = 100
        0x01,             // SIQ: SPI=1, 质量好
    ];
    let asdu = parse_asdu(&buf).expect("ASDU 解析失败");
    assert_eq!(asdu.type_id, TypeId::SinglePoint);
    assert_eq!(asdu.num_objects, 1);
    assert_eq!(asdu.cot, CauseOfTransmission::Spontaneous);
    assert_eq!(asdu.oa, 0);
    assert_eq!(asdu.asdu_address, 1);
    assert_eq!(asdu.objects.len(), 1);

    match &asdu.objects[0] {
        InformationObject::SinglePoint { ioa, value, quality } => {
            assert_eq!(*ioa, 100);
            assert!(*value, "SPI 应为 true");
            assert!(quality.spi);
            assert!(quality.is_valid());
        }
        _ => panic!("期望 SinglePoint 信息体"),
    }
}

/// M_SP_NA_1 SIQ 质量描述词位定义
#[test]
fn test_m_sp_na_1_siq_quality_descriptor_bits() {
    // SIQ 格式: SPI(bit0) + 保留(bit1-3) + BL(bit4) + SB(bit5) + NT(bit6) + IV(bit7)
    let q_good = SinglePointQuality::from_u8(0x01);
    assert!(q_good.spi);
    assert!(!q_good.bl);
    assert!(!q_good.sb);
    assert!(!q_good.nt);
    assert!(!q_good.iv);
    assert!(q_good.is_valid());

    // BL=1 (blocked) — is_valid() 仅检查 IV 和 NT
    let q_blocked = SinglePointQuality::from_u8(0x11); // SPI=1, BL=1
    assert!(q_blocked.bl);
    // is_valid() 返回 true 因为 iv=false, nt=false（BL 不影响 is_valid）
    assert!(q_blocked.is_valid());

    // IV=1 (invalid) — is_valid() 返回 false
    let q_invalid = SinglePointQuality::from_u8(0x81); // SPI=1, IV=1
    assert!(q_invalid.iv);
    assert!(!q_invalid.is_valid());

    // NT=1 (not topical) — is_valid() 返回 false
    let q_nt = SinglePointQuality::from_u8(0x41); // SPI=1, NT=1
    assert!(q_nt.nt);
    assert!(!q_nt.is_valid());
}

// ============================================================================
// M_DP_NA_1 — 双点信息 (Type 3, 0x03)
// ============================================================================

/// M_DP_NA_1 类型标识 = 0x03
#[test]
fn test_m_dp_na_1_type_identifier() {
    assert_eq!(TypeId::DoublePoint.to_u8(), 0x03);
    assert_eq!(TypeId::from_u8(0x03), TypeId::DoublePoint);
}

/// M_DP_NA_1 ASDU 解析：DIQ 字节编码 DPV(bit0-1) + BL/SB/NT/IV
#[test]
fn test_m_dp_na_1_asdu_parsing() {
    // 构造 M_DP_NA_1 ASDU:
    //   TI=0x03, SQ=0/Num=1, COT=3(自发), OA=0, ASDU地址=1
    //   IOA=100(3B小端), DIQ=0x02(DPV=On, 质量好)
    let buf: Vec<u8> = vec![
        0x03, 0x01, 0x03, 0x00, 0x01, 0x00,
        0x64, 0x00, 0x00, // IOA = 100
        0x02,             // DIQ: DPV=2(ON), 质量好
    ];
    let asdu = parse_asdu(&buf).expect("ASDU 解析失败");
    assert_eq!(asdu.type_id, TypeId::DoublePoint);
    assert_eq!(asdu.num_objects, 1);

    match &asdu.objects[0] {
        InformationObject::DoublePoint { ioa, value, quality } => {
            assert_eq!(*ioa, 100);
            assert_eq!(*value, DoublePointValue::On);
            assert!(quality.is_valid());
        }
        _ => panic!("期望 DoublePoint 信息体"),
    }
}

/// M_DP_NA_1 双点值枚举：0=不定, 1=OFF, 2=ON, 3=不定
#[test]
fn test_m_dp_na_1_double_point_values() {
    assert_eq!(DoublePointValue::from_u8(0x00), DoublePointValue::Indeterminate);
    assert_eq!(DoublePointValue::from_u8(0x01), DoublePointValue::Off);
    assert_eq!(DoublePointValue::from_u8(0x02), DoublePointValue::On);
    assert_eq!(DoublePointValue::from_u8(0x03), DoublePointValue::Indeterminate2);
}

// ============================================================================
// M_ME_NA_1 — 归一化测量值 (Type 9, 0x09)
// ============================================================================

/// M_ME_NA_1 类型标识 = 0x09
/// 注意：EnerOS 当前未实现 M_ME_NA_1（归一化测量值）类型，
/// TypeId 枚举中不包含此类型。待实现后启用此测试。
#[test]
#[ignore = "M_ME_NA_1 (Type 9) 未在 eneros-device TypeId 中实现"]
fn test_m_me_na_1_type_identifier() {
    // IEC 60870-5-101 规定 M_ME_NA_1 类型标识为 0x09
    // 当前 TypeId::from_u8(0x09) 返回 Unknown(9)
    let tid = TypeId::from_u8(0x09);
    assert_eq!(tid.to_u8(), 0x09);
}

/// M_ME_NA_1 ASDU 结构：IOA(3B) + 归一化值(2B) + QDS(1B)
/// 注意：EnerOS 当前未实现 M_ME_NA_1 解析，待实现后启用。
#[test]
#[ignore = "M_ME_NA_1 (Type 9) 解析未在 eneros-device 中实现"]
fn test_m_me_na_1_asdu_parsing() {
    // M_ME_NA_1: 归一化值范围 -1.0 ~ +1.0，编码为 16 位有符号整数
    // NVA = -1.0 → 0x8000, NVA = 0 → 0x0000, NVA = +1.0 → 0x7FFF
    let buf: Vec<u8> = vec![
        0x09, 0x01, 0x03, 0x00, 0x01, 0x00,
        0xC8, 0x00, 0x00, // IOA = 200
        0xFF, 0x7F,       // NVA = 0x7FFF (+1.0)
        0x00,             // QDS: 质量好
    ];
    let asdu = parse_asdu(&buf);
    // 当前实现返回 Some 但 type_id 为 Unknown，对象列表为空
    if let Some(a) = asdu {
        assert_eq!(a.type_id.to_u8(), 0x09);
    }
}

// ============================================================================
// M_ME_NB_1 — 标度化测量值 (Type 11, 0x0B)
// ============================================================================

/// M_ME_NB_1 类型标识 = 0x0B
/// 注意：EnerOS 当前未实现 M_ME_NB_1（标度化测量值）类型，
/// TypeId 枚举中不包含此类型。待实现后启用此测试。
#[test]
#[ignore = "M_ME_NB_1 (Type 11) 未在 eneros-device TypeId 中实现"]
fn test_m_me_nb_1_type_identifier() {
    // IEC 60870-5-101 规定 M_ME_NB_1 类型标识为 0x0B
    let tid = TypeId::from_u8(0x0B);
    assert_eq!(tid.to_u8(), 0x0B);
}

/// M_ME_NB_1 ASDU 结构：IOA(3B) + 标度化值(2B,有符号) + QDS(1B)
/// 注意：EnerOS 当前未实现 M_ME_NB_1 解析，待实现后启用。
#[test]
#[ignore = "M_ME_NB_1 (Type 11) 解析未在 eneros-device 中实现"]
fn test_m_me_nb_1_asdu_parsing() {
    // M_ME_NB_1: 标度化值为 16 位有符号整数，范围 -32768 ~ +32767
    let buf: Vec<u8> = vec![
        0x0B, 0x01, 0x03, 0x00, 0x01, 0x00,
        0xC8, 0x00, 0x00, // IOA = 200
        0x64, 0x00,       // SVA = 100
        0x00,             // QDS: 质量好
    ];
    let asdu = parse_asdu(&buf);
    if let Some(a) = asdu {
        assert_eq!(a.type_id.to_u8(), 0x0B);
    }
}

// ============================================================================
// M_ME_NC_1 — 短浮点测量值 (Type 13, 0x0D)
// ============================================================================

/// M_ME_NC_1 类型标识 = 0x0D
#[test]
fn test_m_me_nc_1_type_identifier() {
    assert_eq!(TypeId::MeasuredShortFloat.to_u8(), 0x0D);
    assert_eq!(TypeId::from_u8(0x0D), TypeId::MeasuredShortFloat);
}

/// M_ME_NC_1 ASDU 解析：IEEE 754 短浮点(4B小端) + QDS(1B)
#[test]
fn test_m_me_nc_1_asdu_parsing() {
    let value: f32 = 220.5;
    let vb = value.to_le_bytes();
    let buf: Vec<u8> = vec![
        0x0D, 0x01, 0x01, 0x00, 0x01, 0x00,
        0xC8, 0x00, 0x00, // IOA = 200
        vb[0], vb[1], vb[2], vb[3], // IEEE 754 短浮点
        0x00,             // QDS: 质量好
    ];
    let asdu = parse_asdu(&buf).expect("ASDU 解析失败");
    assert_eq!(asdu.type_id, TypeId::MeasuredShortFloat);
    assert_eq!(asdu.num_objects, 1);

    match &asdu.objects[0] {
        InformationObject::MeasuredShortFloat { ioa, value: v, quality } => {
            assert_eq!(*ioa, 200);
            assert!((v - 220.5f32).abs() < 0.001);
            assert!(quality.is_valid());
        }
        _ => panic!("期望 MeasuredShortFloat 信息体"),
    }
}

/// M_ME_NC_1 QDS 质量描述词：OV(bit0) + BL(bit4) + SB(bit5) + NT(bit6) + IV(bit7)
#[test]
fn test_m_me_nc_1_qds_quality_descriptor() {
    let q_good = MeasuredQuality::from_u8(0x00);
    assert!(!q_good.ov);
    assert!(q_good.is_valid());

    let q_overflow = MeasuredQuality::from_u8(0x01);
    assert!(q_overflow.ov);
    assert!(!q_overflow.is_valid());

    let q_invalid = MeasuredQuality::from_u8(0x80);
    assert!(q_invalid.iv);
    assert!(!q_invalid.is_valid());
}

// ============================================================================
// C_SC_NA_1 — 单点命令 (Type 45, 0x2D)
// ============================================================================

/// C_SC_NA_1 类型标识 = 0x2D (45)
#[test]
fn test_c_sc_na_1_type_identifier() {
    assert_eq!(TypeId::SingleCommand.to_u8(), 0x2D);
    assert_eq!(TypeId::from_u8(0x2D), TypeId::SingleCommand);
    assert_eq!(0x2D, 45);
}

/// C_SC_NA_1 命令构造：SCO 字节 = SCS(bit0) + QU(bit1-5) + S/E(bit7)
#[test]
fn test_c_sc_na_1_command_construction() {
    let cmd = build_single_command(1, 1001, true, 0, false);
    assert_eq!(cmd[0], 0x2D); // TI = 45
    assert_eq!(cmd[1], 0x01); // SQ=0, Num=1
    assert_eq!(cmd[2], CauseOfTransmission::Activation.to_u8()); // COT = 6
    assert_eq!(cmd[3], 0x00); // OA = 0
    assert_eq!(cmd[4], 0x01); // ASDU 地址低字节
    assert_eq!(cmd[5], 0x00); // ASDU 地址高字节

    // IOA = 1001 (3 字节小端)
    let ioa = cmd[6] as u32 | (cmd[7] as u32) << 8 | (cmd[8] as u32) << 16;
    assert_eq!(ioa, 1001);

    // SCO: SCS=1(ON), QU=0, S/E=0
    assert_eq!(cmd[9] & 0x01, 1); // SCS = ON
    assert_eq!((cmd[9] >> 1) & 0x1F, 0); // QU = 0
    assert_eq!(cmd[9] & 0x80, 0); // S/E = 0 (执行)
}

/// C_SC_NA_1 S/E 位：0=执行, 1=选择
#[test]
fn test_c_sc_na_1_select_execute_flag() {
    // 执行命令
    let exec_cmd = build_single_command(1, 1001, true, 0, false);
    assert_eq!(exec_cmd[9] & 0x80, 0);

    // 选择命令
    let sel_cmd = build_single_command(1, 1001, true, 0, true);
    assert_eq!(sel_cmd[9] & 0x80, 0x80);
}

// ============================================================================
// C_DC_NA_1 — 双点命令 (Type 46, 0x2E)
// ============================================================================

/// C_DC_NA_1 类型标识 = 0x2E (46)
#[test]
fn test_c_dc_na_1_type_identifier() {
    assert_eq!(TypeId::DoubleCommand.to_u8(), 0x2E);
    assert_eq!(TypeId::from_u8(0x2E), TypeId::DoubleCommand);
    assert_eq!(0x2E, 46);
}

/// C_DC_NA_1 命令构造：DCO 字节 = DCS(bit0-1) + QU(bit2-6) + S/E(bit7)
#[test]
fn test_c_dc_na_1_command_construction() {
    let cmd = build_double_command(1, 1001, 2, 0, false); // DCS=2(ON)
    assert_eq!(cmd[0], 0x2E); // TI = 46
    assert_eq!(cmd[1], 0x01); // SQ=0, Num=1
    assert_eq!(cmd[2], CauseOfTransmission::Activation.to_u8());
    assert_eq!(cmd[3], 0x00); // OA
    assert_eq!(cmd[4], 0x01); // ASDU 地址低字节
    assert_eq!(cmd[5], 0x00); // ASDU 地址高字节

    // IOA = 1001
    let ioa = cmd[6] as u32 | (cmd[7] as u32) << 8 | (cmd[8] as u32) << 16;
    assert_eq!(ioa, 1001);

    // DCO: DCS=2(ON), QU=0, S/E=0
    assert_eq!(cmd[9] & 0x03, 2); // DCS = ON
    assert_eq!((cmd[9] >> 2) & 0x1F, 0); // QU = 0
    assert_eq!(cmd[9] & 0x80, 0); // S/E = 0
}

/// C_DC_NA_1 DCS 值：1=OFF, 2=ON（0 和 3 不允许）
#[test]
fn test_c_dc_na_1_dcs_values() {
    // DCS=1 (OFF)
    let off_cmd = build_double_command(1, 1001, 1, 0, false);
    assert_eq!(off_cmd[9] & 0x03, 1);

    // DCS=2 (ON)
    let on_cmd = build_double_command(1, 1001, 2, 0, false);
    assert_eq!(on_cmd[9] & 0x03, 2);
}

// ============================================================================
// ASDU 结构一致性 — IEC 60870-5-101 §7
// ============================================================================

/// ASDU 头部结构：类型标识(1B) + 可变结构限定词(1B) + 传送原因(2B) + 公共地址(2B) = 6B
#[test]
fn test_asdu_header_structure() {
    let cmd = build_interrogation_command(1, 0);
    assert!(cmd.len() >= 6);

    // 字节 0: 类型标识
    assert_eq!(cmd[0], TypeId::InterrogationCommand.to_u8());
    assert_eq!(cmd[0], 100); // C_IC_NA_1 = 100

    // 字节 1: 可变结构限定词 (SQ=1bit + Num=7bits)
    let sq_and_num = cmd[1];
    let sq = (sq_and_num >> 7) & 0x01;
    let num = sq_and_num & 0x7F;
    assert_eq!(num, 1); // 1 个信息体
    assert_eq!(sq, 0);  // 非序列

    // 字节 2-3: 传送原因 (COT, 2 字节)
    assert_eq!(cmd[2], CauseOfTransmission::Activation.to_u8());

    // 字节 4-5: 公共地址 (ASDU 地址, 2 字节小端)
    let asdu_addr = cmd[4] as u16 | (cmd[5] as u16) << 8;
    assert_eq!(asdu_addr, 1);
}

/// ASDU 可变结构限定词：SQ 位决定信息体地址排列方式
#[test]
fn test_asdu_variable_structure_qualifier() {
    // SQ=0: 每个信息体自带 IOA（非序列方式）
    // SQ=1: 仅第一个信息体有 IOA，后续顺序递增（序列方式）
    let cmd = build_single_command(1, 1001, true, 0, false);
    let sq_and_num = cmd[1];
    let sq = (sq_and_num >> 7) & 0x01;
    let num = sq_and_num & 0x7F;
    assert_eq!(sq, 0);  // 单点命令使用非序列方式
    assert_eq!(num, 1); // 1 个信息体
}

/// ASDU 传送原因 (COT) 常用值
#[test]
fn test_asdu_cause_of_transmission_values() {
    assert_eq!(CauseOfTransmission::Periodic.to_u8(), 1);
    assert_eq!(CauseOfTransmission::Background.to_u8(), 2);
    assert_eq!(CauseOfTransmission::Spontaneous.to_u8(), 3);
    assert_eq!(CauseOfTransmission::Initialized.to_u8(), 4);
    assert_eq!(CauseOfTransmission::Request.to_u8(), 5);
    assert_eq!(CauseOfTransmission::Activation.to_u8(), 6);
    assert_eq!(CauseOfTransmission::ActivationConfirmation.to_u8(), 7);
    assert_eq!(CauseOfTransmission::Deactivation.to_u8(), 8);
    assert_eq!(CauseOfTransmission::DeactivationConfirmation.to_u8(), 9);
    assert_eq!(CauseOfTransmission::InterrogatedByStation.to_u8(), 10);
}

/// ASDU 公共地址为 2 字节小端编码（IEC 60870-5-104）
#[test]
fn test_asdu_common_address_encoding() {
    let cmd = build_single_command(0x0102, 1001, true, 0, false);
    // ASDU 地址在字节 4-5（小端）
    assert_eq!(cmd[4], 0x02); // 低字节
    assert_eq!(cmd[5], 0x01); // 高字节
    let addr = cmd[4] as u16 | (cmd[5] as u16) << 8;
    assert_eq!(addr, 0x0102);
}

/// ASDU 信息体地址 (IOA) 为 3 字节小端编码
#[test]
fn test_asdu_ioa_encoding() {
    let cmd = build_single_command(1, 0x123456, true, 0, false);
    // IOA 在字节 6-8（3 字节小端）
    assert_eq!(cmd[6], 0x56);
    assert_eq!(cmd[7], 0x34);
    assert_eq!(cmd[8], 0x12);
    let ioa = cmd[6] as u32 | (cmd[7] as u32) << 8 | (cmd[8] as u32) << 16;
    assert_eq!(ioa, 0x123456);
}

/// ASDU 解析过短缓冲区返回 None
#[test]
fn test_asdu_parse_too_short() {
    assert!(parse_asdu(&[]).is_none());
    assert!(parse_asdu(&[0x01]).is_none());
    assert!(parse_asdu(&[0x01, 0x01, 0x03, 0x00, 0x01]).is_none()); // 5 字节 < 6
}

/// 多信息体 ASDU 解析（M_SP_NA_1，2 个单点）
#[test]
fn test_asdu_multiple_information_objects() {
    let buf: Vec<u8> = vec![
        0x01, 0x02, 0x03, 0x00, 0x01, 0x00, // TI=1, Num=2, COT=3, OA=0, ASDU=1
        0x64, 0x00, 0x00, 0x01, // IOA=100, SIQ=0x01(SPI=1)
        0x65, 0x00, 0x00, 0x00, // IOA=101, SIQ=0x00(SPI=0)
    ];
    let asdu = parse_asdu(&buf).expect("ASDU 解析失败");
    assert_eq!(asdu.type_id, TypeId::SinglePoint);
    assert_eq!(asdu.num_objects, 2);
    assert_eq!(asdu.objects.len(), 2);

    match &asdu.objects[0] {
        InformationObject::SinglePoint { ioa, value, .. } => {
            assert_eq!(*ioa, 100);
            assert!(*value);
        }
        _ => panic!("期望第一个 SinglePoint"),
    }
    match &asdu.objects[1] {
        InformationObject::SinglePoint { ioa, value, .. } => {
            assert_eq!(*ioa, 101);
            assert!(!*value);
        }
        _ => panic!("期望第二个 SinglePoint"),
    }
}

// ============================================================================
// 其他 ASDU 类型验证
// ============================================================================

/// C_IC_NA_1 总召唤命令 (Type 100) 结构
#[test]
fn test_c_ic_na_1_interrogation_command() {
    let cmd = build_interrogation_command(1, 0);
    assert_eq!(cmd[0], 100); // TI = C_IC_NA_1
    assert_eq!(cmd[1], 0x01); // Num=1
    assert_eq!(cmd[2], CauseOfTransmission::Activation.to_u8());
    // 字节 9 为召唤类型（0x14 = 站召唤）
    assert_eq!(cmd[9], 0x14);
}

/// C_SE_NC_1 设点命令-短浮点 (Type 50) 结构
#[test]
fn test_c_se_nc_1_setpoint_command() {
    let cmd = build_setpoint_short_float(1, 2001, 50.0f32, 0, false);
    assert_eq!(cmd[0], 50); // TI = C_SE_NC_1
    assert_eq!(cmd[1], 0x01); // Num=1

    // IOA = 2001
    let ioa = cmd[6] as u32 | (cmd[7] as u32) << 8 | (cmd[8] as u32) << 16;
    assert_eq!(ioa, 2001);

    // 值为 IEEE 754 短浮点（4 字节小端）
    let value = f32::from_le_bytes([cmd[9], cmd[10], cmd[11], cmd[12]]);
    assert!((value - 50.0f32).abs() < 0.001);

    // QOS: QU + S/E
    assert_eq!(cmd[13] & 0x80, 0); // S/E = 0 (执行)
}

/// C_CS_NA_1 时钟同步命令 (Type 103) 结构
#[test]
fn test_c_cs_na_1_clock_sync_command() {
    let cmd = build_clock_sync_command(1, 3600000); // 1 小时
    assert_eq!(cmd[0], 103); // TI = C_CS_NA_1
    assert_eq!(cmd.len(), 16); // 6 头 + 3 IOA + 7 CP56Time2a
}

/// P_PM_NA_1 参数-浮点 (Type 112) 结构
#[test]
fn test_p_pm_na_1_parameter_float() {
    let cmd = build_parameter_float(1, 1001, 50.5f32);
    assert_eq!(cmd[0], 112); // TI = P_PM_NA_1
    assert_eq!(cmd.len(), 13); // 6 头 + 3 IOA + 4 float
}

/// P_PM_NI_1 参数-标度化 (Type 111) 结构
#[test]
fn test_p_pm_ni_1_parameter_scaled() {
    let cmd = build_parameter_scaled(1, 1001, 100i16);
    assert_eq!(cmd[0], 111); // TI = P_PM_NI_1
    assert_eq!(cmd.len(), 11); // 6 头 + 3 IOA + 2 int16
}

/// TypeId Display 格式符合标准命名
#[test]
fn test_type_id_display_format() {
    assert_eq!(format!("{}", TypeId::SinglePoint), "M_SP_NA_1(1)");
    assert_eq!(format!("{}", TypeId::DoublePoint), "M_DP_NA_1(3)");
    assert_eq!(format!("{}", TypeId::MeasuredShortFloat), "M_ME_NC_1(13)");
    assert_eq!(format!("{}", TypeId::SingleCommand), "C_SC_NA_1(45)");
    assert_eq!(format!("{}", TypeId::DoubleCommand), "C_DC_NA_1(46)");
}

/// InformationObject::as_float() 类型转换一致性
#[test]
fn test_information_object_as_float_conversion() {
    // SinglePoint → 0.0 / 1.0
    let sp = InformationObject::SinglePoint {
        ioa: 1, value: true,
        quality: SinglePointQuality::from_u8(1),
    };
    assert!((sp.as_float().unwrap() - 1.0).abs() < 0.001);

    // MeasuredShortFloat → f32 值
    let mf = InformationObject::MeasuredShortFloat {
        ioa: 2, value: 220.5f32,
        quality: MeasuredQuality::from_u8(0),
    };
    assert!((mf.as_float().unwrap() - 220.5).abs() < 0.001);

    // DoublePoint On → 1.0, Off → 0.0
    let dp_on = InformationObject::DoublePoint {
        ioa: 3, value: DoublePointValue::On,
        quality: MeasuredQuality::from_u8(0),
    };
    assert!((dp_on.as_float().unwrap() - 1.0).abs() < 0.001);
}
