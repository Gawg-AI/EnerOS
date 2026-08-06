//! EnerOS BSP 认证体系 (workspace v0.192.0)
//!
//! 提供标准化 BSP 认证能力，包括认证规范定义、测试套件执行与认证流程管理。
//!
//! # 模块
//! - [`types`][]: 核心数据结构（BspCategory / BspSpec / BspTestReport / Certificate）
//! - [`spec`][]: 认证规范管理（默认规范生成 / 规范校验）
//! - [`testkit`][]: 测试套件（BspTestKit trait / MockTestKit）
//! - [`process`][]: 认证流程引擎（提交 / 测试 / 认证判定）

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod process;
pub mod spec;
pub mod testkit;
pub mod types;

pub use process::{BspCertEngine, CertificationProcess};
pub use spec::{default_spec, validate_spec};
pub use testkit::{run_tests, BspTestKit, MockTestKit};
pub use types::{
    BspCategory, BspItem, BspSpec, BspTestReport, CertPhase, Certificate, HardwareInfo, Outcome,
    TestCaseResult,
};

/// BSP 认证错误类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertError {
    /// 案例不存在
    CaseNotFound,
    /// 当前阶段不允许此操作
    InvalidPhase,
    /// 强制项测试未通过
    MandatoryFailed,
    /// 认证规范非法
    InvalidSpec,
    /// 已认证，不可重复操作
    AlreadyCertified,
}
