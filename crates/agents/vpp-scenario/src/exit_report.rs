//! Phase 2 出口标准验证报告：数据模型 + 证据注册 + 出口判定 + 报告渲染.
//!
//! D4（蓝图偏差）：蓝图 `run_exit_criteria` 第 4~10 项硬编码 `true` 必须替换为
//! [`ExitEvidence`] 证据注入——未注册证据的项一律 `passed=false` 且
//! `actual="未验证"`，杜绝"未验证却谎报通过"。第 1~3 项（节点数/聚合容量/DR
//! 响应时间）由 VPP 场景实时计算，经 [`ExitReportBuilder::evaluate`] 写入，
//! 不接受外部证据注入。

/// 单项出口标准评估结果.
#[derive(Debug, Clone, PartialEq)]
pub struct ExitCriterion {
    /// 出口标准编号（1~10）
    pub id: u32,
    /// 标准名称（≤ 64 字节）
    pub name: heapless::String<64>,
    /// 目标值描述（≤ 128 字节）
    pub target: heapless::String<128>,
    /// 实测值描述（≤ 128 字节）
    pub actual: heapless::String<128>,
    /// 是否通过
    pub passed: bool,
}

/// 出口标准外部验证证据（第 4~10 项经证据注入，D4：禁硬编码通过）.
#[derive(Debug, Clone, PartialEq)]
pub struct ExitEvidence {
    /// 实测值描述（≤ 128 字节）
    pub actual: heapless::String<128>,
    /// 是否通过
    pub passed: bool,
}

/// Phase 2 出口验证汇总报告.
#[derive(Debug, Clone, PartialEq)]
pub struct Phase2ExitReport {
    /// 阶段标识（如 "Phase 2"）
    pub phase: heapless::String<16>,
    /// 版本区间（如 "v0.75.0 ~ v0.126.0"）
    pub version_range: heapless::String<32>,
    /// 阶段版本总数（= 52）
    pub total_versions: u32,
    /// 出口标准总数
    pub exit_criteria_total: usize,
    /// 通过的出口标准数
    pub exit_criteria_passed: usize,
    /// 全部通过标记
    pub all_passed: bool,
    /// 逐项出口标准评估结果
    pub criteria: alloc::vec::Vec<ExitCriterion>,
}

/// 十项出口标准清单（蓝图 §4.5 `init_exit_criteria` 原样：序号/名称/目标）.
const EXIT_CRITERIA_DEFS: [(u32, &str, &str); 10] = [
    (1, "VPP 节点数", "≥ 5 台"),
    (2, "聚合容量", "≥ 100kW"),
    (3, "DR 响应时间", "< 30s"),
    (4, "联邦共识", "PBFT 2f+1"),
    (5, "时间同步", "< 1ms"),
    (6, "安全评估", "渗透测试通过"),
    (7, "可观测性", "指标+日志+追踪"),
    (8, "OTA 升级", "A/B 回滚通过"),
    (9, "Recovery Mode", "可恢复验证"),
    (10, "IEC 61850", "GOOSE < 4ms"),
];

/// 未注册证据项的实测占位文案（D4：不谎报）.
const UNVERIFIED: &str = "未验证";

/// 安全写入定界 heapless 字符串：整体写入，容量不足时按 UTF-8 字符边界截断.
fn write_bounded(dst: &mut heapless::String<128>, src: &str) {
    dst.clear();
    if dst.push_str(src).is_err() {
        for ch in src.chars() {
            if dst.push(ch).is_err() {
                break;
            }
        }
    }
}

/// 由字符串切片构造 heapless 字符串（容量不足时按 UTF-8 字符边界截断兜底）.
fn hstr<const N: usize>(s: &str) -> heapless::String<N> {
    let mut out = heapless::String::<N>::new();
    if out.push_str(s).is_err() {
        for ch in s.chars() {
            if out.push(ch).is_err() {
                break;
            }
        }
    }
    out
}

/// Phase 2 出口标准报告构建器.
///
/// - 第 1~3 项由 VPP 场景实时计算，经 [`ExitReportBuilder::evaluate`] 写入；
/// - 第 4~10 项由外部子系统验证证据注入，经
///   [`ExitReportBuilder::register_evidence`] 注册（D4）；
/// - [`ExitReportBuilder::build`] 汇总生成 [`Phase2ExitReport`]，未注册证据的
///   第 4~10 项一律标记 `actual="未验证"`、`passed=false`。
#[derive(Debug)]
pub struct ExitReportBuilder {
    /// 十项出口标准（蓝图 §4.5 清单原样初始化）
    criteria: heapless::Vec<ExitCriterion, 10>,
    /// 已注册的外部验证证据（仅第 4~10 项）
    evidence: heapless::Vec<(u32, ExitEvidence), 10>,
}

impl ExitReportBuilder {
    /// 创建构建器：初始化 10 项出口标准（passed 全 false，actual 为空）.
    #[must_use]
    pub fn new() -> Self {
        let mut criteria = heapless::Vec::<ExitCriterion, 10>::new();
        for (id, name, target) in EXIT_CRITERIA_DEFS.iter() {
            let _ = criteria.push(ExitCriterion {
                id: *id,
                name: hstr(name),
                target: hstr(target),
                actual: heapless::String::new(),
                passed: false,
            });
        }
        Self {
            criteria,
            evidence: heapless::Vec::new(),
        }
    }

    /// 只读访问十项出口标准当前状态.
    #[must_use]
    pub fn criteria(&self) -> &[ExitCriterion] {
        &self.criteria
    }

    /// 注册第 4~10 项出口标准的外部验证证据（D4：禁硬编码通过）.
    ///
    /// 返回 `true` 表示证据已接受；第 1~3 项由场景计算、忽略外部注入，
    /// id 越界（0 或 > 10）同样拒绝，均返回 `false`。重复注册同一 id 时覆盖旧证据。
    pub fn register_evidence(&mut self, id: u32, evidence: ExitEvidence) -> bool {
        if !(4..=10).contains(&id) {
            return false;
        }
        if let Some(slot) = self.evidence.iter_mut().find(|(eid, _)| *eid == id) {
            slot.1 = evidence;
            return true;
        }
        self.evidence.push((id, evidence)).is_ok()
    }

    /// 写入第 1~3 项出口标准的场景计算结果.
    ///
    /// `computed_actual` 超过 128 容量时按 UTF-8 字符边界截断。
    /// 返回 `true` 表示已写入；第 4~10 项由证据注入、忽略场景计算，返回 `false`。
    pub fn evaluate(&mut self, id: u32, computed_actual: &str, computed_passed: bool) -> bool {
        if !(1..=3).contains(&id) {
            return false;
        }
        match self.criteria.iter_mut().find(|c| c.id == id) {
            Some(c) => {
                write_bounded(&mut c.actual, computed_actual);
                c.passed = computed_passed;
                true
            }
            None => false,
        }
    }

    /// 汇总生成 Phase 2 出口验证报告.
    ///
    /// 第 4~10 项：已注册证据的应用证据的 actual/passed；未注册项
    /// `actual="未验证"`、`passed=false`。统计 total=10 / passed / all_passed。
    #[must_use]
    pub fn build(&self) -> Phase2ExitReport {
        let mut criteria: alloc::vec::Vec<ExitCriterion> =
            alloc::vec::Vec::with_capacity(self.criteria.len());
        for c in self.criteria.iter() {
            let mut item = c.clone();
            if (4..=10).contains(&item.id) {
                match self.evidence.iter().find(|(eid, _)| *eid == item.id) {
                    Some((_, ev)) => {
                        item.actual = ev.actual.clone();
                        item.passed = ev.passed;
                    }
                    None => {
                        item.actual = hstr(UNVERIFIED);
                        item.passed = false;
                    }
                }
            }
            criteria.push(item);
        }
        let passed = criteria.iter().filter(|c| c.passed).count();
        Phase2ExitReport {
            phase: hstr("Phase 2"),
            version_range: hstr("v0.75.0 ~ v0.126.0"),
            total_versions: 52,
            exit_criteria_total: criteria.len(),
            exit_criteria_passed: passed,
            all_passed: passed == criteria.len(),
            criteria,
        }
    }

    /// 渲染文本版出口验证报告（Phase 头 + 版本区间 + 10 行出口判定表 + 结论行，
    /// 表格格式对齐蓝图 §7.5 "Phase 2 出口标准达成判定"）。
    #[must_use]
    pub fn render(report: &Phase2ExitReport) -> alloc::string::String {
        use core::fmt::Write as _;

        let mut out = alloc::string::String::new();
        let _ = writeln!(out, "{} 出口标准验证报告", report.phase.as_str());
        let _ = writeln!(out, "版本区间: {}", report.version_range.as_str());
        let _ = writeln!(out, "版本总数: {}", report.total_versions);
        let _ = writeln!(out);
        let _ = writeln!(out, "| 序号 | 出口标准 | 目标 | 实测 | 通过 |");
        let _ = writeln!(out, "|------|----------|------|------|------|");
        for c in &report.criteria {
            let mark = if c.passed { "✅" } else { "❌" };
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                c.id,
                c.name.as_str(),
                c.target.as_str(),
                c.actual.as_str(),
                mark
            );
        }
        let _ = writeln!(out);
        if report.all_passed {
            let _ = writeln!(
                out,
                "结论: 全部通过（{}/{} 通过）",
                report.exit_criteria_passed, report.exit_criteria_total
            );
        } else {
            let _ = writeln!(
                out,
                "结论: {}/{} 通过",
                report.exit_criteria_passed, report.exit_criteria_total
            );
        }
        out
    }
}

impl Default for ExitReportBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条外部验证证据.
    fn make_evidence(actual: &str, passed: bool) -> ExitEvidence {
        ExitEvidence {
            actual: hstr(actual),
            passed,
        }
    }

    /// 注册第 4~10 项全部通过的证据（实测文案对齐蓝图 §7.5 判定表）.
    fn register_all_passing(b: &mut ExitReportBuilder) {
        let actuals = [
            (4u32, "PBFT 通过"),
            (5, "0.3ms"),
            (6, "30/30 通过"),
            (7, "三支柱就绪"),
            (8, "回滚通过"),
            (9, "可恢复"),
            (10, "2.1ms"),
        ];
        for (id, actual) in actuals.iter() {
            assert!(b.register_evidence(*id, make_evidence(actual, true)));
        }
    }

    /// 场景计算第 1~3 项并全部通过（实测文案对齐蓝图 §7.5 判定表）.
    fn evaluate_all_passing(b: &mut ExitReportBuilder) {
        assert!(b.evaluate(1, "8 台", true));
        assert!(b.evaluate(2, "350 kW", true));
        assert!(b.evaluate(3, "18000 ms", true));
    }

    /// RPT1：new() 后 10 项出口标准初始化正确（id 1~10、passed 全 false）.
    #[test]
    fn rpt1_new_initializes_ten_criteria() {
        let b = ExitReportBuilder::new();
        assert_eq!(b.criteria().len(), 10);
        for (idx, c) in b.criteria().iter().enumerate() {
            assert_eq!(c.id, idx as u32 + 1);
            assert!(!c.passed, "id={} 初始不应通过", c.id);
            assert!(c.actual.is_empty(), "id={} 初始 actual 应为空", c.id);
            assert!(!c.name.is_empty());
            assert!(!c.target.is_empty());
        }
        assert_eq!(b.criteria()[0].name.as_str(), "VPP 节点数");
        assert_eq!(b.criteria()[2].target.as_str(), "< 30s");
        assert_eq!(b.criteria()[9].name.as_str(), "IEC 61850");
    }

    /// RPT2：未注册证据不谎报——第 4~10 项 passed=false 且 actual="未验证"，
    /// all_passed=false.
    #[test]
    fn rpt2_unregistered_evidence_never_reports_pass() {
        let b = ExitReportBuilder::new();
        let report = b.build();
        assert_eq!(report.exit_criteria_total, 10);
        assert_eq!(report.exit_criteria_passed, 0);
        assert!(!report.all_passed);
        for c in report.criteria.iter().filter(|c| c.id >= 4) {
            assert!(!c.passed, "id={} 未注册证据不得通过", c.id);
            assert_eq!(c.actual.as_str(), UNVERIFIED);
        }
        assert_eq!(report.phase.as_str(), "Phase 2");
        assert_eq!(report.version_range.as_str(), "v0.75.0 ~ v0.126.0");
        assert_eq!(report.total_versions, 52);
    }

    /// RPT3：注册 4~10 证据 + evaluate 1~3 全过后 → 10/10、all_passed=true.
    #[test]
    fn rpt3_full_evidence_all_pass() {
        let mut b = ExitReportBuilder::new();
        evaluate_all_passing(&mut b);
        register_all_passing(&mut b);
        let report = b.build();
        assert_eq!(report.exit_criteria_total, 10);
        assert_eq!(report.exit_criteria_passed, 10);
        assert!(report.all_passed);
        // 证据 actual 原样落入报告
        assert_eq!(report.criteria[3].actual.as_str(), "PBFT 通过");
        assert_eq!(report.criteria[9].actual.as_str(), "2.1ms");
        // 场景计算 actual 原样落入报告
        assert_eq!(report.criteria[0].actual.as_str(), "8 台");
        assert_eq!(report.criteria[2].actual.as_str(), "18000 ms");
    }

    /// RPT4：register_evidence(1..=3) 与越界 id 被拒绝（返回 false 且不生效）；
    /// evaluate(4..=10) 被忽略.
    #[test]
    fn rpt4_injection_channel_isolation() {
        let mut b = ExitReportBuilder::new();
        for id in [0u32, 1, 2, 3, 11, 100] {
            assert!(
                !b.register_evidence(id, make_evidence("伪造证据", true)),
                "id={} 不应接受注册",
                id
            );
        }
        for id in 4..=10u32 {
            assert!(
                !b.evaluate(id, "伪造实测", true),
                "id={} 不应接受场景计算",
                id
            );
        }
        let report = b.build();
        // 第 1~3 项未被证据注入污染
        for c in report.criteria.iter().take(3) {
            assert!(!c.passed);
            assert!(c.actual.is_empty());
        }
        // 第 4~10 项未被场景计算污染
        for c in report.criteria.iter().skip(3) {
            assert!(!c.passed);
            assert_eq!(c.actual.as_str(), UNVERIFIED);
        }
        assert_eq!(report.exit_criteria_passed, 0);
        assert!(!report.all_passed);
    }

    /// RPT5：render 渲染含 10 行判定表与结论行（字符串 contains 校验关键行）.
    #[test]
    fn rpt5_render_contains_table_and_conclusion() {
        let mut b = ExitReportBuilder::new();
        evaluate_all_passing(&mut b);
        register_all_passing(&mut b);
        let report = b.build();
        let text = ExitReportBuilder::render(&report);
        assert!(text.contains("Phase 2"));
        assert!(text.contains("v0.75.0 ~ v0.126.0"));
        assert!(text.contains("| 序号 | 出口标准 | 目标 | 实测 | 通过 |"));
        // 10 行判定表逐行存在
        for id in 1..=10u32 {
            assert!(text.contains(&format!("| {} |", id)), "缺少第 {} 行", id);
        }
        assert!(text.contains("| 1 | VPP 节点数 | ≥ 5 台 | 8 台 | ✅ |"));
        assert!(text.contains("| 10 | IEC 61850 | GOOSE < 4ms | 2.1ms | ✅ |"));
        // 全过场景：无 ❌，结论行为"全部通过"
        assert!(!text.contains('❌'));
        assert!(text.contains("结论: 全部通过（10/10 通过）"));
    }

    /// RPT6：passed 计数正确——仅 7 项通过 → exit_criteria_passed=7、
    /// all_passed=false.
    #[test]
    fn rpt6_partial_pass_counts_correctly() {
        let mut b = ExitReportBuilder::new();
        // 第 1~3 项通过
        evaluate_all_passing(&mut b);
        // 仅注册第 4~7 项通过证据，第 8~10 项不注册
        for id in 4..=7u32 {
            assert!(b.register_evidence(id, make_evidence("通过", true)));
        }
        let report = b.build();
        assert_eq!(report.exit_criteria_passed, 7);
        assert_eq!(report.exit_criteria_total, 10);
        assert!(!report.all_passed);
        // 第 8~10 项未注册 → 未验证且不通过
        for c in report.criteria.iter().filter(|c| c.id >= 8) {
            assert!(!c.passed);
            assert_eq!(c.actual.as_str(), UNVERIFIED);
        }
        let text = ExitReportBuilder::render(&report);
        assert!(text.contains("结论: 7/10 通过"));
        assert!(text.contains('❌'));
        assert!(!text.contains("全部通过"));
    }
}
