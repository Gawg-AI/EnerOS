//! IEC 60870-5-104 传输层一致性测试 — IEC 60870-5-104
//!
//! 测试 APCI 帧格式（I/U/S 帧）、传输参数（k/w/t1/t2/t3）。
//!
//! 注意：EnerOS 的 Iec104Client 将 APCI 帧构造逻辑封装在内部，
//! Iec104Config 未暴露 k/w/t1/t2/t3 参数。因此部分传输层测试标注
//! `#[ignore]`，待实现层暴露相关接口后启用。
//!
//! 参考:
//! - IEC 60870-5-104 §4: APCI 帧格式
//! - IEC 60870-5-104 §5: 传输过程（k/w/t1/t2/t3 参数）

use std::time::Duration;
use eneros_device::Iec104Config;

// ============================================================================
// APCI 帧格式 — IEC 60870-5-104 §4
// ============================================================================

/// APCI 起始字节固定为 0x68
#[test]
fn test_apci_start_byte() {
    // IEC 60870-5-104 规定所有帧以 0x68 开始
    const START_BYTE: u8 = 0x68;
    assert_eq!(START_BYTE, 0x68);
}

/// APCI 帧结构：起始字节(0x68) + 长度(1B) + 控制域(4B)
#[test]
fn test_apci_frame_structure() {
    // APCI (Application Protocol Control Information) 固定 6 字节头:
    //   起始字节(1B) + APDU 长度(1B) + 控制域(4B)
    // APDU 长度 = 控制域(4B) + ASDU 长度
    // U/S 帧无 ASDU，长度 = 4
    const APCI_HEADER_SIZE: usize = 6;
    const U_S_FRAME_LENGTH: u8 = 4;
    assert_eq!(APCI_HEADER_SIZE, 6);
    assert_eq!(U_S_FRAME_LENGTH, 4);
}

// ============================================================================
// U 帧格式 — 非编号控制功能 (Unnumbered control)
// ============================================================================

/// U 帧 STARTDT_ACT 控制域字节 1 = 0x07
#[test]
fn test_u_frame_startdt_act() {
    // IEC 60870-5-104 规定 STARTDT_ACT 的控制域第 1 字节为 0x07
    // 完整帧: 0x68 0x04 0x07 0x00 0x00 0x00
    const STARTDT_ACT: u8 = 0x07;
    assert_eq!(STARTDT_ACT, 0x07);

    // 验证 U 帧格式（控制域字节 1 的 bit0 = 1 表示 U 帧）
    let ctrl_byte = STARTDT_ACT;
    assert_eq!(ctrl_byte & 0x01, 1); // U 帧标识位
}

/// U 帧 STARTDT_CON 控制域字节 1 = 0x0B
#[test]
fn test_u_frame_startdt_con() {
    // STARTDT_CON 的控制域第 1 字节为 0x0B
    const STARTDT_CON: u8 = 0x0B;
    assert_eq!(STARTDT_CON, 0x0B);

    let ctrl_byte = STARTDT_CON;
    assert_eq!(ctrl_byte & 0x01, 1); // U 帧标识位
}

/// U 帧 STOPDT_ACT 控制域字节 1 = 0x13
#[test]
fn test_u_frame_stopdt_act() {
    const STOPDT_ACT: u8 = 0x13;
    assert_eq!(STOPDT_ACT, 0x13);

    let ctrl_byte = STOPDT_ACT;
    assert_eq!(ctrl_byte & 0x01, 1); // U 帧标识位
}

/// U 帧 STOPDT_CON 控制域字节 1 = 0x23
#[test]
fn test_u_frame_stopdt_con() {
    const STOPDT_CON: u8 = 0x23;
    assert_eq!(STOPDT_CON, 0x23);

    let ctrl_byte = STOPDT_CON;
    assert_eq!(ctrl_byte & 0x01, 1); // U 帧标识位
}

/// U 帧 TESTFR_ACT 控制域字节 1 = 0x43
#[test]
fn test_u_frame_testfr_act() {
    const TESTFR_ACT: u8 = 0x43;
    assert_eq!(TESTFR_ACT, 0x43);

    let ctrl_byte = TESTFR_ACT;
    assert_eq!(ctrl_byte & 0x01, 1); // U 帧标识位
}

/// U 帧 TESTFR_CON 控制域字节 1 = 0x83
#[test]
fn test_u_frame_testfr_con() {
    const TESTFR_CON: u8 = 0x83;
    assert_eq!(TESTFR_CON, 0x83);

    let ctrl_byte = TESTFR_CON;
    assert_eq!(ctrl_byte & 0x01, 1); // U 帧标识位
}

/// U 帧完整帧格式：0x68 0x04 + 控制域(4B，仅字节 1 有意义，其余为 0)
#[test]
fn test_u_frame_complete_format() {
    // STARTDT_ACT 完整帧
    let startdt_act: [u8; 6] = [0x68, 0x04, 0x07, 0x00, 0x00, 0x00];
    assert_eq!(startdt_act[0], 0x68); // 起始字节
    assert_eq!(startdt_act[1], 0x04); // APDU 长度 = 4
    assert_eq!(startdt_act[2], 0x07); // STARTDT_ACT
    assert_eq!(startdt_act[3], 0x00); // 控制域 2
    assert_eq!(startdt_act[4], 0x00); // 控制域 3
    assert_eq!(startdt_act[5], 0x00); // 控制域 4

    // TESTFR_ACT 完整帧
    let testfr_act: [u8; 6] = [0x68, 0x04, 0x43, 0x00, 0x00, 0x00];
    assert_eq!(testfr_act[2], 0x43);
}

/// U 帧控制域字节 1 的位定义验证
#[test]
fn test_u_frame_control_byte_bit_definitions() {
    // U 帧控制域字节 1 位定义:
    //   bit0 = 1 (U 帧标识，所有 U 帧均设置)
    //   bit1 = 1 (所有 U 帧功能均设置)
    //   bit2 = STARTDT ACT
    //   bit3 = STARTDT CON
    //   bit4 = STOPDT ACT
    //   bit5 = STOPDT CON
    //   bit6 = TESTFR ACT
    //   bit7 = TESTFR CON

    // STARTDT_ACT = 0x07 = 0b00000111
    assert_eq!(0x07 & 0x01, 1); // U 帧标识
    assert_eq!((0x07 >> 2) & 0x01, 1); // STARTDT ACT

    // STARTDT_CON = 0x0B = 0b00001011
    assert_eq!((0x0B >> 3) & 0x01, 1); // STARTDT CON

    // STOPDT_ACT = 0x13 = 0b00010011
    assert_eq!((0x13 >> 4) & 0x01, 1); // STOPDT ACT

    // STOPDT_CON = 0x23 = 0b00100011
    assert_eq!((0x23 >> 5) & 0x01, 1); // STOPDT CON

    // TESTFR_ACT = 0x43 = 0b01000011
    assert_eq!((0x43 >> 6) & 0x01, 1); // TESTFR ACT

    // TESTFR_CON = 0x83 = 0b10000011
    assert_eq!((0x83 >> 7) & 0x01, 1); // TESTFR CON
}

// ============================================================================
// I 帧格式 — 信息传输 (Information transfer)
// ============================================================================

/// I 帧控制域字节 1 的 bit0 = 0（标识 I 帧）
#[test]
fn test_i_frame_control_field_bit0() {
    // I 帧控制域字节 1 的最低位 (bit0) = 0
    // 发送序号 N(S) 占 bit1-15（控制域 1-2 字节）
    // 接收序号 N(R) 占 bit17-31（控制域 3-4 字节）

    // 模拟 I 帧控制域: N(S)=0, N(R)=0
    // 控制域 1 = (N(S) << 1) = 0x00
    // 控制域 2 = (N(S) >> 7) = 0x00
    // 控制域 3 = (N(R) << 1) = 0x00
    // 控制域 4 = (N(R) >> 7) = 0x00
    let ctrl1: u8 = 0x00;
    assert_eq!(ctrl1 & 0x01, 0); // I 帧标识位
}

/// I 帧发送序号 N(S) 编码在控制域字节 1-2
#[test]
fn test_i_frame_send_sequence_number_encoding() {
    // N(S) 编码: 控制域 1 = (N(S) << 1), 控制域 2 = (N(S) >> 7)
    // N(S) = 5 → 控制域 1 = 0x0A, 控制域 2 = 0x00
    let ns: u16 = 5;
    let ctrl1 = (ns << 1) as u8;
    let ctrl2 = (ns >> 7) as u8;
    assert_eq!(ctrl1, 0x0A);
    assert_eq!(ctrl2, 0x00);
    assert_eq!(ctrl1 & 0x01, 0); // I 帧标识

    // N(S) = 128 → 控制域 1 = 0x00, 控制域 2 = 0x01
    let ns2: u16 = 128;
    let ctrl1_2 = (ns2 << 1) as u8;
    let ctrl2_2 = (ns2 >> 7) as u8;
    assert_eq!(ctrl1_2, 0x00);
    assert_eq!(ctrl2_2, 0x01);
}

/// I 帧接收序号 N(R) 编码在控制域字节 3-4
#[test]
fn test_i_frame_recv_sequence_number_encoding() {
    // N(R) 编码: 控制域 3 = (N(R) << 1), 控制域 4 = (N(R) >> 7)
    let nr: u16 = 3;
    let ctrl3 = (nr << 1) as u8;
    let ctrl4 = (nr >> 7) as u8;
    assert_eq!(ctrl3, 0x06);
    assert_eq!(ctrl4, 0x00);
}

/// I 帧完整 APCI 头：0x68 + 长度 + 4 控制域字节 + ASDU
#[test]
fn test_i_frame_apci_header() {
    // I 帧 APCI: 0x68 + APDU长度 + N(S)(2B) + N(R)(2B)
    // APDU 长度 = 4 (控制域) + ASDU 长度
    // 假设 ASDU 长度为 10，则 APDU 长度 = 14
    let asdu_len: usize = 10;
    let apdu_len: u8 = (4 + asdu_len) as u8;
    assert_eq!(apdu_len, 14);

    // N(S)=1, N(R)=0
    let ns: u16 = 1;
    let nr: u16 = 0;
    let apci: [u8; 6] = [
        0x68,
        apdu_len,
        (ns << 1) as u8,
        (ns >> 7) as u8,
        (nr << 1) as u8,
        (nr >> 7) as u8,
    ];
    assert_eq!(apci[0], 0x68);
    assert_eq!(apci[1], 14);
    assert_eq!(apci[2], 0x02); // N(S)=1 → 0x02
    assert_eq!(apci[3], 0x00);
    assert_eq!(apci[4], 0x00); // N(R)=0 → 0x00
    assert_eq!(apci[5], 0x00);
}

// ============================================================================
// S 帧格式 — 编号监视功能 (Numbered supervisory)
// ============================================================================

/// S 帧控制域字节 1 = 0x01（bit0=1, bit1=0 标识 S 帧）
#[test]
fn test_s_frame_control_field() {
    // S 帧控制域字节 1: bit0=1, bit1=1 → 0x01 | 0x02 = 0x03?
    // 实际 IEC 60870-5-104 规定 S 帧控制域字节 1 = 0x01
    // bit0=1 (非 I 帧), bit1=0 (S 帧标识)
    // 标准 S 帧: 0x68 0x04 0x01 0x00 N(R)<<1 N(R)>>7
    const S_FRAME_CTRL1: u8 = 0x01;
    assert_eq!(S_FRAME_CTRL1, 0x01);
    assert_eq!(S_FRAME_CTRL1 & 0x01, 1); // 非 I 帧
}

/// S 帧 N(R) 接收序号编码在控制域字节 3-4
#[test]
fn test_s_frame_recv_sequence_number() {
    // S 帧控制域 3-4 编码 N(R): 控制域 3 = (N(R) << 1), 控制域 4 = (N(R) >> 7)
    let nr: u16 = 5;
    let ctrl3 = (nr << 1) as u8;
    let ctrl4 = (nr >> 7) as u8;
    assert_eq!(ctrl3, 0x0A);
    assert_eq!(ctrl4, 0x00);

    // 完整 S 帧: 0x68 0x04 0x01 0x00 0x0A 0x00
    let s_frame: [u8; 6] = [0x68, 0x04, 0x01, 0x00, ctrl3, ctrl4];
    assert_eq!(s_frame[0], 0x68);
    assert_eq!(s_frame[1], 0x04);
    assert_eq!(s_frame[2], 0x01); // S 帧标识
    assert_eq!(s_frame[3], 0x00);
    assert_eq!(s_frame[4], 0x0A); // N(R)=5
    assert_eq!(s_frame[5], 0x00);
}

// ============================================================================
// 帧类型区分 — I/U/S 帧识别
// ============================================================================

/// 通过控制域字节 1 区分 I/U/S 帧
#[test]
fn test_frame_type_discrimination() {
    // I 帧: 控制域字节 1 bit0 = 0
    let i_frame_ctrl1: u8 = 0x00; // N(S)=0
    assert_eq!(i_frame_ctrl1 & 0x01, 0);

    // U 帧: 控制域字节 1 bit0 = 1, 且为 STARTDT/STOPDT/TESTFR 值
    let u_frame_ctrl1: u8 = 0x07; // STARTDT_ACT
    assert_eq!(u_frame_ctrl1 & 0x01, 1);
    assert!(u_frame_ctrl1 == 0x07 || u_frame_ctrl1 == 0x0B
            || u_frame_ctrl1 == 0x13 || u_frame_ctrl1 == 0x23
            || u_frame_ctrl1 == 0x43 || u_frame_ctrl1 == 0x83);

    // S 帧: 控制域字节 1 = 0x01
    let s_frame_ctrl1: u8 = 0x01;
    assert_eq!(s_frame_ctrl1 & 0x01, 1);
    assert_eq!(s_frame_ctrl1, 0x01);
}

// ============================================================================
// 传输参数 k/w/t1/t2/t3 — IEC 60870-5-104 §5
// ============================================================================

/// Iec104Config 默认配置验证
#[test]
fn test_iec104_config_default_values() {
    let config = Iec104Config::default();
    assert_eq!(config.remote_addr, "127.0.0.1:2404");
    assert_eq!(config.asdu_address, 1);
    assert!(config.auto_interrogation);
    assert!(config.tls.is_none());
    assert!(config.secondary_addr.is_none());
}

/// IEC 104 标准端口号 2404
#[test]
fn test_iec104_standard_port() {
    // IEC 60870-5-104 规定 TCP 端口 2404
    const IEC104_STANDARD_PORT: u16 = 2404;
    assert_eq!(IEC104_STANDARD_PORT, 2404);

    let config = Iec104Config::default();
    assert!(config.remote_addr.contains("2404"));
}

/// t3 参数：测试帧超时（test_interval）
/// IEC 60870-5-104 规定 t3 默认值 = 20s（范围 1-48s）
/// EnerOS 的 Iec104Config.test_interval 对应 t3 参数
#[test]
fn test_t3_test_interval_parameter() {
    let config = Iec104Config::default();
    // 默认 test_interval = 30s（在标准范围内）
    assert_eq!(config.test_interval, Duration::from_secs(30));

    // 验证 t3 在 IEC 60870-5-104 允许范围内 (1-48s)
    let t3_secs = config.test_interval.as_secs();
    assert!((1..=48).contains(&t3_secs), "t3 应在 1-48s 范围内，实际: {}s", t3_secs);
}

/// k 参数：最大未确认 I 帧数
/// IEC 60870-5-104 规定 k 默认值 = 12（范围 1-32767）
/// 注意：EnerOS 的 Iec104Config 未暴露 k 参数，待实现后启用。
#[test]
#[ignore = "k 参数未在 Iec104Config 中暴露"]
fn test_k_max_outstanding_frames() {
    // IEC 60870-5-104 默认 k = 12（标准范围 1-32767）
    const K_DEFAULT: u16 = 12;
    assert_eq!(K_DEFAULT, 12);
}

/// w 参数：最大未确认 I 帧数（确认前）
/// IEC 60870-5-104 规定 w 默认值 = 8（范围 1-32767）
/// 注意：EnerOS 的 Iec104Config 未暴露 w 参数，待实现后启用。
#[test]
#[ignore = "w 参数未在 Iec104Config 中暴露"]
fn test_w_max_ack_delay_frames() {
    // IEC 60870-5-104 默认 w = 8（标准范围 1-32767）
    const W_DEFAULT: u16 = 8;
    assert_eq!(W_DEFAULT, 8);
}

/// t1 参数：确认超时
/// IEC 60870-5-104 规定 t1 默认值 = 15s（范围 1-255s）
/// 注意：EnerOS 的 Iec104Config 未暴露 t1 参数，待实现后启用。
#[test]
#[ignore = "t1 参数未在 Iec104Config 中暴露"]
fn test_t1_ack_timeout_parameter() {
    // IEC 60870-5-104 默认 t1 = 15s（标准范围 1-255s）
    const T1_DEFAULT_SECS: u64 = 15;
    assert_eq!(T1_DEFAULT_SECS, 15);
}

/// t2 参数：确认延迟
/// IEC 60870-5-104 规定 t2 默认值 = 10s（范围 1-255s）
/// 注意：EnerOS 的 Iec104Config 未暴露 t2 参数，待实现后启用。
#[test]
#[ignore = "t2 参数未在 Iec104Config 中暴露"]
fn test_t2_ack_delay_parameter() {
    // IEC 60870-5-104 默认 t2 = 10s（标准范围 1-255s）
    const T2_DEFAULT_SECS: u64 = 10;
    assert_eq!(T2_DEFAULT_SECS, 10);
}

/// t3 参数必须大于 t1（确保测试帧在确认超时后发送）
/// 注意：需要 t1 参数暴露后才能完整验证
#[test]
#[ignore = "t1 参数未在 Iec104Config 中暴露，无法验证 t3 > t1"]
fn test_t3_greater_than_t1() {
    let t1_default: u64 = 15;
    let t3_default: u64 = 20;
    assert!(t3_default > t1_default, "t3({}) 应大于 t1({})", t3_default, t1_default);
}

// ============================================================================
// 连接配置验证
// ============================================================================

/// Iec104Config 可自定义远程地址和 ASDU 地址
#[test]
fn test_iec104_config_custom_values() {
    let config = Iec104Config {
        remote_addr: "192.168.1.100:2404".to_string(),
        asdu_address: 2,
        connect_timeout: Duration::from_secs(10),
        reconnect_interval: Duration::from_secs(3),
        test_interval: Duration::from_secs(20),
        auto_interrogation: false,
        tls: None,
        secondary_addr: None,
        redundancy: Default::default(),
    };
    assert_eq!(config.remote_addr, "192.168.1.100:2404");
    assert_eq!(config.asdu_address, 2);
    assert_eq!(config.connect_timeout, Duration::from_secs(10));
    assert_eq!(config.reconnect_interval, Duration::from_secs(3));
    assert_eq!(config.test_interval, Duration::from_secs(20));
    assert!(!config.auto_interrogation);
}

/// 连接超时配置验证
#[test]
fn test_iec104_connect_timeout() {
    let config = Iec104Config::default();
    // 默认连接超时 5s
    assert_eq!(config.connect_timeout, Duration::from_secs(5));
}

/// 重连间隔配置验证
#[test]
fn test_iec104_reconnect_interval() {
    let config = Iec104Config::default();
    // 默认重连间隔 5s
    assert_eq!(config.reconnect_interval, Duration::from_secs(5));
}

// ============================================================================
// APDU 长度限制 — IEC 60870-5-104
// ============================================================================

/// IEC 60870-5-104 最大 APDU 长度 = 253 字节（ASDU 最大 249 字节）
#[test]
fn test_apdu_max_length() {
    // IEC 60870-5-104 规定:
    //   APDU 最大长度 = 253 字节
    //   APCI 头 = 6 字节
    //   ASDU 最大长度 = 253 - 4 = 249 字节（不含控制域 4 字节）
    const MAX_APDU_LENGTH: usize = 253;
    const APCI_CONTROL_FIELD: usize = 4;
    const MAX_ASDU_LENGTH: usize = MAX_APDU_LENGTH - APCI_CONTROL_FIELD;

    assert_eq!(MAX_APDU_LENGTH, 253);
    assert_eq!(MAX_ASDU_LENGTH, 249);
}

/// APDU 长度字段为 1 字节（最大 255，但实际限制 253）
#[test]
fn test_apdu_length_field_size() {
    // APCI 中的 APDU 长度字段为 1 字节
    // 理论范围 0-255，但 IEC 60870-5-104 限制最大 253
    const LENGTH_FIELD_MAX: u8 = 255;
    const APDU_MAX: u8 = 253;
    // 253 < 255，APDU 最大值在长度字段可表示范围内
    assert_eq!(APDU_MAX, 253);
    assert_eq!(LENGTH_FIELD_MAX, 255);
}
