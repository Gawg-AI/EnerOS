//! EnerOS 协议一致性测试套件
//!
//! 本 crate 对 EnerOS 支持的三种电力协议进行基于协议标准的黑盒一致性测试：
//!
//! - **IEC 61850**（MMS / GOOSE / SV）— IEC 61850-8-1 / IEC 61850-9-2 LE
//! - **Modbus**（TCP / RTU）— Modbus Application Protocol v1.1b3 / Modbus over Serial Line v1.02
//! - **IEC 60870-5-104**（ASDU / 传输层）— IEC 60870-5-104 / IEC 60870-5-101
//!
//! 测试聚焦于帧格式、字段编码、类型标识等协议标准一致性，
//! 不依赖真实设备——所有测试均为纯函数级别的编解码验证。
//!
//! # 模块组织
//!
//! ```text
//! src/
//! ├── lib.rs                      — 模块声明
//! ├── iec61850/
//! │   ├── mod.rs
//! │   ├── mms_conformance.rs      — MMS 服务一致性
//! │   ├── goose_conformance.rs    — GOOSE 报文一致性
//! │   └── sv_conformance.rs       — SV 采样值一致性
//! ├── modbus/
//! │   ├── mod.rs
//! │   ├── tcp_conformance.rs      — Modbus TCP 功能码
//! │   ├── rtu_conformance.rs      — Modbus RTU 帧格式
//! │   └── exception_codes.rs      — 异常码响应
//! └── iec104/
//!     ├── mod.rs
//!     ├── asdu_conformance.rs     — ASDU 类型一致性
//!     └── transport_conformance.rs— 传输层一致性
//! ```

// 本 crate 为纯测试套件，所有代码仅在 #[test] 函数中使用。
// cargo build 时 #[test] 函数不编译，导致导入和函数出现"未使用"警告。
// 此处抑制这些警告，避免 clippy -D warnings 失败。
#![allow(unused_imports)]
#![allow(dead_code)]

pub mod iec61850;
pub mod modbus;
pub mod iec104;
