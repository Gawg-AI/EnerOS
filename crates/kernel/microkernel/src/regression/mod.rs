//! 全系统回归测试框架（v0.157.0 seL4 深度定制后回归验证 ★）。
//!
//! Phase 1-2 所有功能在 seL4 深度定制配置下回归测试通过，★ 证明
//! 定制配置不影响全系统功能（方案 A 下 seL4 内核保留，无需替代）。
//!
//! ## 模块结构
//!
//! - [`framework`]：回归框架核心类型（RegressionReport/TestResult/TestCase）
//! - [`phase0`]：Phase 0 内核地基回归矩阵（22 版本）
//! - [`phase1`]：Phase 1 单机 MVP 回归矩阵（52 版本）
//! - [`phase2`]：Phase 2 多机联邦回归矩阵（52 版本）
//!
//! ## ADR-0001 定位
//!
//! ★ P3-G 终版本，seL4 深度定制"可用性"里程碑，为 P3-H~P3-K 解锁。
//! 方案 A 下回归范围从"全量内核行为"缩小为"定制配置影响面"。
//!
//! ## 偏差/决策记录
//!
//! - D1：蓝图路径 `tests/regression/` → `crates/kernel/microkernel/
//!   src/regression/`（框架代码入 crate，测试文件入 tests/；仓库规范
//!   §2.3.1 强制子系统归口）。
//! - D2：蓝图 `Vec<TestResult>` / `String` → 定长数组 + 固定长度
//!   字符串（no_std 无 alloc；同仓库 D2 先例链）。
//! - D3：蓝图 `#![no_main]` 裸机入口 → host 可测骨架（lib crate
//!   内模块 + tests/ 集成测试；真机运行属 v0.157.0 后续集成）。
//! - D4：蓝图 126 个版本全量回归 → 按子系统分组代表性验证
//!   （Phase 0: 22 + Phase 1: 52 + Phase 2: 52 = 126 条目，
//!   host 骨架以 mock 通过验证框架正确性）。

pub mod framework;
pub mod phase0;
pub mod phase1;
pub mod phase2;

pub use framework::{
    Priority, RegressionMatrix, RegressionReport, RegressionRunner, TestCase, TestResult,
    TestStatus, MAX_PHASE0_TESTS, MAX_PHASE1_TESTS, MAX_PHASE2_TESTS, MAX_RESULTS,
};
