//! 回归测试框架核心类型（v0.157.0）。
//!
//! 蓝图 §3 接口类 + §4.1 回归测试矩阵原样实现（D2 定长化）。

/// 测试结果最大记录数（定长，no_std 无堆 D2）。
pub const MAX_RESULTS: usize = 256;

/// Phase 0 最大测试用例数。
pub const MAX_PHASE0_TESTS: usize = 32;

/// Phase 1 最大测试用例数。
pub const MAX_PHASE1_TESTS: usize = 64;

/// Phase 2 最大测试用例数。
pub const MAX_PHASE2_TESTS: usize = 64;

/// 测试名称最大长度。
pub const MAX_NAME_LEN: usize = 48;

/// 测试优先级（蓝图 §4.2 回归测试分类）。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// P0：核心功能（内核地基/Agent/四遥/RTOS/双脑/MVP）。
    P0 = 0,
    /// P1：重要功能（设备协议/LLM/Solver/联邦）。
    P1 = 1,
    /// P2：扩展功能。
    P2 = 2,
}

/// 测试状态（蓝图 §3 接口类 `TestStatus` 原样）。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestStatus {
    /// 通过。
    Pass = 0,
    /// 失败。
    Fail = 1,
    /// 跳过。
    Skip = 2,
}

/// 单个测试结果（蓝图 §3 接口类 `TestResult` 原样，D2 定长化）。
#[derive(Clone, Copy, Debug)]
pub struct TestResult {
    /// 测试名称（固定长度，D2）。
    pub name: [u8; MAX_NAME_LEN],
    /// 测试名称有效长度。
    pub name_len: usize,
    /// 测试状态。
    pub status: TestStatus,
    /// 耗时（微秒，host 骨架恒 0）。
    pub duration_us: u64,
    /// 所属 Phase（0/1/2）。
    pub phase: u8,
    /// 是否有效（已填充）。
    pub valid: bool,
}

impl TestResult {
    /// 创建通过的测试结果。
    pub fn pass(name: &[u8], phase: u8) -> Self {
        let mut buf = [0u8; MAX_NAME_LEN];
        let len = name.len().min(MAX_NAME_LEN);
        let mut i = 0;
        while i < len {
            buf[i] = name[i];
            i += 1;
        }
        Self {
            name: buf,
            name_len: len,
            status: TestStatus::Pass,
            duration_us: 0,
            phase,
            valid: true,
        }
    }

    /// 创建失败的测试结果。
    pub fn fail(name: &[u8], phase: u8) -> Self {
        let mut r = Self::pass(name, phase);
        r.status = TestStatus::Fail;
        r
    }

    /// 创建跳过的测试结果。
    pub fn skip(name: &[u8], phase: u8) -> Self {
        let mut r = Self::pass(name, phase);
        r.status = TestStatus::Skip;
        r
    }

    /// 获取名称切片。
    pub fn name_str(&self) -> &[u8] {
        &self.name[..self.name_len]
    }
}

/// 单个回归测试用例（蓝图 §4.1 `TestCase` 原样，D2 定长化）。
#[derive(Clone, Copy)]
pub struct TestCase {
    /// 版本标识（如 "v0.33.0"）。
    pub version: [u8; 16],
    /// 版本标识有效长度。
    pub version_len: usize,
    /// 模块名称（如 "Agent Runtime"）。
    pub module: [u8; MAX_NAME_LEN],
    /// 模块名称有效长度。
    pub module_len: usize,
    /// 测试函数指针。
    pub test_fn: fn() -> TestStatus,
    /// 优先级。
    pub priority: Priority,
    /// 是否有 seL4 基线。
    pub sel4_baseline: bool,
    /// 所属 Phase。
    pub phase: u8,
    /// 是否有效。
    pub valid: bool,
}

impl TestCase {
    /// 创建测试用例。
    pub fn new(
        version: &[u8],
        module: &[u8],
        test_fn: fn() -> TestStatus,
        priority: Priority,
        sel4_baseline: bool,
        phase: u8,
    ) -> Self {
        let mut ver_buf = [0u8; 16];
        let ver_len = version.len().min(16);
        let mut i = 0;
        while i < ver_len {
            ver_buf[i] = version[i];
            i += 1;
        }

        let mut mod_buf = [0u8; MAX_NAME_LEN];
        let mod_len = module.len().min(MAX_NAME_LEN);
        i = 0;
        while i < mod_len {
            mod_buf[i] = module[i];
            i += 1;
        }

        Self {
            version: ver_buf,
            version_len: ver_len,
            module: mod_buf,
            module_len: mod_len,
            test_fn,
            priority,
            sel4_baseline,
            phase,
            valid: true,
        }
    }

    /// 执行测试用例，返回结果。
    pub fn run(&self) -> TestResult {
        let status = (self.test_fn)();
        let mut result = TestResult::pass(&self.module[..self.module_len], self.phase);
        result.status = status;
        result
    }
}

/// 回归测试报告（蓝图 §3 接口类 `RegressionReport` 原样，D2 定长化）。
#[derive(Clone, Copy)]
pub struct RegressionReport {
    /// 总测试数。
    pub total: usize,
    /// 通过数。
    pub passed: usize,
    /// 失败数。
    pub failed: usize,
    /// 跳过数。
    pub skipped: usize,
    /// 总耗时（微秒）。
    pub duration_us: u64,
    /// 结果数组（定长 D2）。
    pub results: [TestResult; MAX_RESULTS],
    /// 当前结果数。
    pub result_count: usize,
}

impl RegressionReport {
    /// 创建空报告。
    pub fn new() -> Self {
        Self {
            total: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            duration_us: 0,
            results: [TestResult::pass(b"", 0); MAX_RESULTS],
            result_count: 0,
        }
    }

    /// 添加测试结果。
    pub fn add(&mut self, result: TestResult) {
        if self.result_count < MAX_RESULTS {
            // 清除无效标记后存入。
            let mut r = result;
            r.valid = true;
            self.results[self.result_count] = r;
            self.result_count += 1;
            self.total += 1;
            match result.status {
                TestStatus::Pass => self.passed += 1,
                TestStatus::Fail => self.failed += 1,
                TestStatus::Skip => self.skipped += 1,
            }
        }
    }

    /// 判定是否全部通过（★ 出口判定）。
    pub fn all_passed(&self) -> bool {
        self.failed == 0 && self.total > 0
    }

    /// 获取指定 Phase 的通过数。
    pub fn phase_passed(&self, phase: u8) -> usize {
        let mut count = 0;
        let mut i = 0;
        while i < self.result_count {
            if self.results[i].phase == phase && self.results[i].status == TestStatus::Pass {
                count += 1;
            }
            i += 1;
        }
        count
    }

    /// 获取指定 Phase 的总数。
    pub fn phase_total(&self, phase: u8) -> usize {
        let mut count = 0;
        let mut i = 0;
        while i < self.result_count {
            if self.results[i].phase == phase {
                count += 1;
            }
            i += 1;
        }
        count
    }

    /// 通过率（百分比 × 100，避免浮点）。
    pub fn pass_rate_bp(&self) -> u32 {
        if self.total == 0 {
            return 0;
        }
        (self.passed as u32 * 10000) / self.total as u32
    }
}

impl Default for RegressionReport {
    fn default() -> Self {
        Self::new()
    }
}

/// 回归测试矩阵（蓝图 §4.1 `RegressionMatrix` 原样，D2 定长化）。
pub struct RegressionMatrix {
    /// Phase 0 测试用例（内核地基，22 版本）。
    pub phase0: [TestCase; MAX_PHASE0_TESTS],
    /// Phase 0 有效条目数。
    pub phase0_count: usize,
    /// Phase 1 测试用例（单机 MVP，52 版本）。
    pub phase1: [TestCase; MAX_PHASE1_TESTS],
    /// Phase 1 有效条目数。
    pub phase1_count: usize,
    /// Phase 2 测试用例（多机联邦，52 版本）。
    pub phase2: [TestCase; MAX_PHASE2_TESTS],
    /// Phase 2 有效条目数。
    pub phase2_count: usize,
}

impl RegressionMatrix {
    /// 创建空矩阵。
    pub fn new() -> Self {
        // 使用一个占位 TestCase 初始化数组。
        let placeholder = TestCase {
            version: [0u8; 16],
            version_len: 0,
            module: [0u8; MAX_NAME_LEN],
            module_len: 0,
            test_fn: || TestStatus::Skip,
            priority: Priority::P2,
            sel4_baseline: false,
            phase: 0,
            valid: false,
        };
        Self {
            phase0: [placeholder; MAX_PHASE0_TESTS],
            phase0_count: 0,
            phase1: [placeholder; MAX_PHASE1_TESTS],
            phase1_count: 0,
            phase2: [placeholder; MAX_PHASE2_TESTS],
            phase2_count: 0,
        }
    }

    /// 加载默认回归矩阵（Phase 0/1/2 全部条目）。
    pub fn load() -> Self {
        let mut matrix = Self::new();
        super::phase0::populate(&mut matrix);
        super::phase1::populate(&mut matrix);
        super::phase2::populate(&mut matrix);
        matrix
    }

    /// 总测试用例数。
    pub fn total_count(&self) -> usize {
        self.phase0_count + self.phase1_count + self.phase2_count
    }
}

impl Default for RegressionMatrix {
    fn default() -> Self {
        Self::new()
    }
}

/// 回归测试运行器（蓝图 §4.5 关键代码框架实现）。
///
/// 分层回归策略：Phase 0 → Phase 1 → Phase 2 → 端到端场景。
/// 前一层失败则不运行后续（蓝图 §5 技术交底）。
pub struct RegressionRunner {
    /// 回归矩阵。
    pub matrix: RegressionMatrix,
    /// 测试报告。
    pub report: RegressionReport,
    /// 是否在前层失败时停止。
    pub stop_on_phase_fail: bool,
}

impl RegressionRunner {
    /// 创建运行器。
    pub fn new(matrix: RegressionMatrix) -> Self {
        Self {
            matrix,
            report: RegressionReport::new(),
            stop_on_phase_fail: true,
        }
    }

    /// 运行全部回归测试（分层策略）。
    ///
    /// 返回最终报告。★ 全功能回归通过判定：`report.all_passed()`。
    pub fn run_all(&mut self) -> &RegressionReport {
        // Phase 0
        self.run_phase(0);
        if self.stop_on_phase_fail && self.report.phase_passed(0) < self.report.phase_total(0) {
            return &self.report;
        }

        // Phase 1
        self.run_phase(1);
        if self.stop_on_phase_fail && self.report.phase_passed(1) < self.report.phase_total(1) {
            return &self.report;
        }

        // Phase 2
        self.run_phase(2);

        &self.report
    }

    /// 运行指定 Phase 的测试。
    fn run_phase(&mut self, phase: u8) {
        let (cases, count) = match phase {
            0 => (&self.matrix.phase0[..], self.matrix.phase0_count),
            1 => (&self.matrix.phase1[..], self.matrix.phase1_count),
            _ => (&self.matrix.phase2[..], self.matrix.phase2_count),
        };

        let mut i = 0;
        while i < count {
            if cases[i].valid {
                let result = cases[i].run();
                self.report.add(result);
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    /// 通过的测试函数。
    fn pass_fn() -> TestStatus {
        TestStatus::Pass
    }

    /// 失败的测试函数。
    fn fail_fn() -> TestStatus {
        TestStatus::Fail
    }

    /// RF1：TestResult 创建与状态判定。
    #[test]
    fn rf1_test_result_creation() {
        let r = TestResult::pass(b"test_a", 0);
        assert_eq!(r.status, TestStatus::Pass);
        assert_eq!(r.name_str(), b"test_a");
        assert_eq!(r.phase, 0);

        let f = TestResult::fail(b"test_b", 1);
        assert_eq!(f.status, TestStatus::Fail);

        let s = TestResult::skip(b"test_c", 2);
        assert_eq!(s.status, TestStatus::Skip);
    }

    /// RF2：RegressionReport 累加与统计。
    #[test]
    fn rf2_report_accumulation() {
        let mut report = RegressionReport::new();
        report.add(TestResult::pass(b"a", 0));
        report.add(TestResult::pass(b"b", 0));
        report.add(TestResult::fail(b"c", 1));
        report.add(TestResult::skip(b"d", 2));

        assert_eq!(report.total, 4);
        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 1);
        assert_eq!(report.skipped, 1);
        assert!(!report.all_passed());
    }

    /// RF3：all_passed 判定。
    #[test]
    fn rf3_all_passed() {
        let mut report = RegressionReport::new();
        report.add(TestResult::pass(b"x", 0));
        report.add(TestResult::pass(b"y", 1));
        assert!(report.all_passed());
    }

    /// RF4：phase_passed / phase_total 分 Phase 统计。
    #[test]
    fn rf4_phase_stats() {
        let mut report = RegressionReport::new();
        report.add(TestResult::pass(b"a", 0));
        report.add(TestResult::pass(b"b", 0));
        report.add(TestResult::fail(b"c", 1));
        report.add(TestResult::pass(b"d", 2));

        assert_eq!(report.phase_total(0), 2);
        assert_eq!(report.phase_passed(0), 2);
        assert_eq!(report.phase_total(1), 1);
        assert_eq!(report.phase_passed(1), 0);
        assert_eq!(report.phase_total(2), 1);
        assert_eq!(report.phase_passed(2), 1);
    }

    /// RF5：pass_rate_bp 通过率计算。
    #[test]
    fn rf5_pass_rate() {
        let mut report = RegressionReport::new();
        report.add(TestResult::pass(b"a", 0));
        report.add(TestResult::pass(b"b", 0));
        report.add(TestResult::fail(b"c", 0));
        // 2/3 = 6666 bp
        assert_eq!(report.pass_rate_bp(), 6666);
    }

    /// RF6：TestCase 创建与执行。
    #[test]
    fn rf6_test_case_run() {
        let tc = TestCase::new(b"v0.1.0", b"toolchain", pass_fn, Priority::P0, true, 0);
        let result = tc.run();
        assert_eq!(result.status, TestStatus::Pass);
        assert_eq!(result.phase, 0);

        let tc2 = TestCase::new(b"v0.2.0", b"fail_mod", fail_fn, Priority::P1, false, 1);
        let result2 = tc2.run();
        assert_eq!(result2.status, TestStatus::Fail);
    }

    /// RF7：RegressionMatrix 创建与加载。
    #[test]
    fn rf7_matrix_load() {
        let matrix = RegressionMatrix::load();
        // Phase 0: 22 版本
        assert_eq!(matrix.phase0_count, 22);
        // Phase 1: 52 版本
        assert_eq!(matrix.phase1_count, 52);
        // Phase 2: 52 版本
        assert_eq!(matrix.phase2_count, 52);
        // 总计 126
        assert_eq!(matrix.total_count(), 126);
    }

    /// RF8：RegressionRunner 全通过场景。
    #[test]
    fn rf8_runner_all_pass() {
        let matrix = RegressionMatrix::load();
        let mut runner = RegressionRunner::new(matrix);
        let report = runner.run_all();
        // 所有 mock 测试函数返回 Pass。
        assert!(report.all_passed());
        assert_eq!(report.total, 126);
        assert_eq!(report.passed, 126);
        assert_eq!(report.failed, 0);
    }

    /// RF9：RegressionRunner 分层停止策略。
    #[test]
    fn rf9_runner_stop_on_fail() {
        let mut matrix = RegressionMatrix::new();
        // Phase 0 添加一个失败用例。
        matrix.phase0[0] = TestCase::new(b"v0.1.0", b"fail", fail_fn, Priority::P0, true, 0);
        matrix.phase0_count = 1;
        // Phase 1 添加一个通过用例。
        matrix.phase1[0] = TestCase::new(b"v0.23.0", b"pass", pass_fn, Priority::P0, true, 1);
        matrix.phase1_count = 1;

        let mut runner = RegressionRunner::new(matrix);
        runner.stop_on_phase_fail = true;
        let report = runner.run_all();

        // Phase 0 失败后停止，Phase 1 未运行。
        assert_eq!(report.total, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.phase_total(1), 0);
    }

    /// RF10：空报告 all_passed 为 false。
    #[test]
    fn rf10_empty_report_not_passed() {
        let report = RegressionReport::new();
        assert!(!report.all_passed());
    }
}
