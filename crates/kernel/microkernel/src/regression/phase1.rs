//! Phase 1 单机 MVP 回归矩阵（v0.157.0，52 版本）。
//!
//! 蓝图 §4.2 分类：P1-Agent(8) + P1-设备协议(9) + P1-四遥SOE(2) +
//! P1-RTOS(5) + P1-LLM(5) + P1-Solver(5) + P1-双脑(3) + P1-MVP(3) +
//! 其余基础设施(12) = 52。
//! 版本区间：v0.23.0 ~ v0.74.0（含 8 个刚性子版本）。

use super::framework::{Priority, RegressionMatrix, TestCase, TestStatus};

/// 默认通过的 mock 测试函数（host 骨架 D4）。
fn mock_pass() -> TestStatus {
    TestStatus::Pass
}

/// 填充 Phase 1 回归矩阵（52 条目）。
pub fn populate(matrix: &mut RegressionMatrix) {
    let cases: [(&[u8], &[u8], Priority, bool); 52] = [
        // 基础设施（12）
        (b"v0.23.0", b"tcpip-smoltcp", Priority::P0, true),
        (b"v0.24.0", b"fs-vfs-layer", Priority::P0, true),
        (b"v0.24.1", b"fs-wear-level", Priority::P1, true),
        (b"v0.25.0", b"dds-cyclone", Priority::P1, true),
        (b"v0.26.0", b"socket-bsd", Priority::P0, true),
        (b"v0.27.0", b"tls-mtls", Priority::P0, true),
        (b"v0.28.0", b"tcpip-advanced", Priority::P0, true),
        (b"v0.29.0", b"cellular-4g", Priority::P1, true),
        (b"v0.30.0", b"cloud-sync", Priority::P1, true),
        (b"v0.30.1", b"cloud-aggregator", Priority::P1, true),
        (b"v0.30.2", b"cloud-coordinator", Priority::P1, true),
        (b"v0.31.0", b"sm2-sm3-sm4", Priority::P0, true),
        // P1-Agent（8）
        (b"v0.33.0", b"agent-lifecycle", Priority::P0, true),
        (b"v0.34.0", b"agent-capability", Priority::P0, true),
        (b"v0.35.0", b"agent-heartbeat", Priority::P0, true),
        (b"v0.36.0", b"agent-coordinator", Priority::P0, true),
        (b"v0.37.0", b"device-agent", Priority::P0, true),
        (b"v0.38.0", b"alarm-agent", Priority::P0, true),
        (b"v0.39.0", b"capability-token", Priority::P0, true),
        (b"v0.40.0", b"agent-sandbox", Priority::P0, true),
        // P1-设备协议（9）
        (b"v0.41.0", b"modbus-rtu", Priority::P1, true),
        (b"v0.42.0", b"modbus-tcp", Priority::P1, true),
        (b"v0.42.1", b"iec104-master", Priority::P1, true),
        (b"v0.43.0", b"iec104-slave", Priority::P1, true),
        (b"v0.44.0", b"iec61850-mms", Priority::P1, true),
        (b"v0.45.0", b"iec61850-goose", Priority::P1, true),
        (b"v0.46.0", b"iec61850-sv", Priority::P1, true),
        (b"v0.47.0", b"can-open", Priority::P1, true),
        (b"v0.48.0", b"grid-points", Priority::P1, true),
        // P1-四遥SOE（2）
        (b"v0.49.0", b"telemetry-telesignal", Priority::P0, true),
        (b"v0.50.0", b"telecontrol-soe", Priority::P0, true),
        // P1-RTOS（5）
        (b"v0.51.0", b"control-loop", Priority::P0, true),
        (b"v0.51.1", b"sampling-jitter", Priority::P0, true),
        (b"v0.51.2", b"degradation-mode", Priority::P0, true),
        (b"v0.52.0", b"pcc-control", Priority::P0, true),
        (b"v0.53.0", b"power-sequence", Priority::P0, true),
        // P1-LLM（5）
        (b"v0.53.1", b"gguf-loader", Priority::P1, true),
        (b"v0.53.2", b"llm-engine", Priority::P1, true),
        (b"v0.54.0", b"infer-scheduler", Priority::P1, true),
        (b"v0.55.0", b"prompt-template", Priority::P1, true),
        (b"v0.56.0", b"safety-validator", Priority::P1, true),
        // P1-Solver（5）
        (b"v0.57.0", b"solver-core", Priority::P1, true),
        (b"v0.58.0", b"solver-milp", Priority::P1, true),
        (b"v0.59.0", b"llm-rust-wrap", Priority::P1, true),
        (b"v0.60.0", b"solver-model", Priority::P1, true),
        (b"v0.61.0", b"solver-warm", Priority::P1, true),
        // P1-双脑（3）
        (b"v0.62.0", b"intent-contract", Priority::P0, true),
        (b"v0.63.0", b"intent-parser", Priority::P0, true),
        (b"v0.64.0", b"dual-brain", Priority::P0, true),
        // P1-MVP（3）
        (b"v0.72.0", b"energy-market", Priority::P0, true),
        (b"v0.73.0", b"mvp-scenario", Priority::P0, true),
        (b"v0.74.0", b"e2e-storage", Priority::P0, true),
    ];

    let mut i = 0;
    while i < 52 {
        matrix.phase1[i] =
            TestCase::new(cases[i].0, cases[i].1, mock_pass, cases[i].2, cases[i].3, 1);
        i += 1;
    }
    matrix.phase1_count = 52;
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// P1T1：Phase 1 矩阵填充 52 条目。
    #[test]
    fn p1t1_populate_count() {
        let mut matrix = RegressionMatrix::new();
        populate(&mut matrix);
        assert_eq!(matrix.phase1_count, 52);
    }

    /// P1T2：所有条目有效且 phase=1。
    #[test]
    fn p1t2_all_valid_phase1() {
        let mut matrix = RegressionMatrix::new();
        populate(&mut matrix);
        let mut i = 0;
        while i < 52 {
            assert!(matrix.phase1[i].valid);
            assert_eq!(matrix.phase1[i].phase, 1);
            i += 1;
        }
    }

    /// P1T3：所有 mock 测试通过。
    #[test]
    fn p1t3_all_pass() {
        let mut matrix = RegressionMatrix::new();
        populate(&mut matrix);
        let mut i = 0;
        while i < 52 {
            let result = matrix.phase1[i].run();
            assert_eq!(result.status, TestStatus::Pass);
            i += 1;
        }
    }
}
