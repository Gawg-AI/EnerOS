//! Phase 0 内核地基回归矩阵（v0.157.0，22 版本）。
//!
//! 蓝图 §4.2 P0-内核地基：双分区隔离/调度/IPC/SMP。
//! 版本区间：v0.1.0 ~ v0.22.0（含 4 个刚性子版本）。

use super::framework::{Priority, RegressionMatrix, TestCase, TestStatus};

/// 默认通过的 mock 测试函数（host 骨架 D4）。
fn mock_pass() -> TestStatus {
    TestStatus::Pass
}

/// 填充 Phase 0 回归矩阵（22 条目）。
pub fn populate(matrix: &mut RegressionMatrix) {
    let cases: [(&[u8], &[u8], Priority, bool); 22] = [
        (b"v0.1.0", b"toolchain-setup", Priority::P0, true),
        (b"v0.2.0", b"ci-pipeline", Priority::P0, true),
        (b"v0.3.0", b"hal-trait", Priority::P0, true),
        (b"v0.4.0", b"board-qemu", Priority::P0, true),
        (b"v0.5.0", b"uart-serial", Priority::P0, true),
        (b"v0.6.0", b"gicv3-interrupt", Priority::P0, true),
        (b"v0.7.0", b"generic-timer", Priority::P0, true),
        (b"v0.8.0", b"kernel-entry", Priority::P0, true),
        (b"v0.9.0", b"partition-isolation", Priority::P0, true),
        (b"v0.9.1", b"compliance-gate", Priority::P0, true),
        (b"v0.10.0", b"buddy-heap", Priority::P0, true),
        (b"v0.11.0", b"user-heap", Priority::P0, true),
        (b"v0.12.0", b"rtc-driver", Priority::P0, true),
        (b"v0.12.1", b"beidou-timing", Priority::P1, true),
        (b"v0.12.2", b"holdover-mode", Priority::P1, true),
        (b"v0.13.0", b"watchdog", Priority::P0, true),
        (b"v0.14.0", b"bootloader", Priority::P0, true),
        (b"v0.15.0", b"device-tree", Priority::P0, true),
        (b"v0.16.0", b"smp-boot", Priority::P0, true),
        (b"v0.17.0", b"multi-core-sched", Priority::P0, true),
        (b"v0.17.1", b"ipi-optimization", Priority::P1, true),
        (b"v0.22.0", b"fs-littlefs", Priority::P0, true),
    ];

    let mut i = 0;
    while i < 22 {
        matrix.phase0[i] =
            TestCase::new(cases[i].0, cases[i].1, mock_pass, cases[i].2, cases[i].3, 0);
        i += 1;
    }
    matrix.phase0_count = 22;
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// P0T1：Phase 0 矩阵填充 22 条目。
    #[test]
    fn p0t1_populate_count() {
        let mut matrix = RegressionMatrix::new();
        populate(&mut matrix);
        assert_eq!(matrix.phase0_count, 22);
    }

    /// P0T2：所有条目有效且 phase=0。
    #[test]
    fn p0t2_all_valid_phase0() {
        let mut matrix = RegressionMatrix::new();
        populate(&mut matrix);
        let mut i = 0;
        while i < 22 {
            assert!(matrix.phase0[i].valid);
            assert_eq!(matrix.phase0[i].phase, 0);
            i += 1;
        }
    }

    /// P0T3：所有 mock 测试通过。
    #[test]
    fn p0t3_all_pass() {
        let mut matrix = RegressionMatrix::new();
        populate(&mut matrix);
        let mut i = 0;
        while i < 22 {
            let result = matrix.phase0[i].run();
            assert_eq!(result.status, TestStatus::Pass);
            i += 1;
        }
    }
}
