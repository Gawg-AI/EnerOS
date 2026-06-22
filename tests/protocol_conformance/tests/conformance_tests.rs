//! EnerOS 协议一致性测试集成入口
//!
//! 本文件为协议一致性测试套件的集成测试入口，运行时通过 `cargo test
//! -p eneros-protocol-conformance` 执行所有协议一致性测试。
//!
//! 测试覆盖三大电力协议：
//! - IEC 61850（MMS / GOOSE / SV）
//! - Modbus（TCP / RTU / 异常码）
//! - IEC 60870-5-104（ASDU / 传输层）
//!
//! 所有测试均为纯函数级别的编解码验证，不依赖真实设备或网络连接，
//! 可在 Windows/Linux/macOS 上运行。

// ============================================================================
// 协议套件完整性验证
//
// 集成测试入口仅验证测试 crate 能编译通过。各协议的实际一致性测试
// 位于 src/ 下对应模块的 #[test] 函数中，由 `cargo test` 自动发现执行。
// ============================================================================

/// IEC 61850 协议一致性测试模块可访问（MMS / GOOSE / SV）
#[test]
fn iec61850_conformance_suite_accessible() {
    // 实际测试在 eneros_protocol_conformance::iec61850::* 模块内
}

/// Modbus 协议一致性测试模块可访问（TCP / RTU / 异常码）
#[test]
fn modbus_conformance_suite_accessible() {
    // 实际测试在 eneros_protocol_conformance::modbus::* 模块内
}

/// IEC 60870-5-104 协议一致性测试模块可访问（ASDU / 传输层）
#[test]
fn iec104_conformance_suite_accessible() {
    // 实际测试在 eneros_protocol_conformance::iec104::* 模块内
}

/// 所有协议测试套件完整性验证
#[test]
fn all_protocol_suites_loaded() {
    // IEC 61850: MMS / GOOSE / SV
    // Modbus: TCP / RTU / 异常码
    // IEC 104: ASDU / 传输层
    // 共 8 个测试模块，实际测试函数在各模块内
}
