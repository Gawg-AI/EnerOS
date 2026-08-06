//! 站点验收测试 (OPS-011).
//!
//! MASTER §21.14 OPS-011: "site acceptance test — 签名验收报告"
//!
//! SAT 是设备部署到客户现场后的最终验收测试。
//! 必须由客户和集成商双方签字确认，生成签名验收报告。
//! 任一检查项失败则 SAT 不通过。

use alloc::string::String;
use alloc::vec::Vec;

/// SAT 检查项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SatCheck {
    /// 设备启动正常。
    BootOk,
    /// 所有协议通信正常（Modbus/IEC104/IEC61850）。
    ProtocolCommOk,
    /// 控制回路响应时间达标。
    ControlLoopLatencyOk,
    /// 安全功能验证（紧急停机/保护）。
    SafetyFunctionOk,
    /// 数据记录与历史完整。
    DataLoggingOk,
    /// 远程管理可达。
    RemoteManagementOk,
    /// 备份/恢复验证。
    BackupRestoreOk,
    /// 告警系统正常。
    AlarmSystemOk,
}

impl SatCheck {
    pub fn label(self) -> &'static str {
        match self {
            SatCheck::BootOk => "BOOT_OK",
            SatCheck::ProtocolCommOk => "PROTOCOL_COMM_OK",
            SatCheck::ControlLoopLatencyOk => "CONTROL_LOOP_LATENCY_OK",
            SatCheck::SafetyFunctionOk => "SAFETY_FUNCTION_OK",
            SatCheck::DataLoggingOk => "DATA_LOGGING_OK",
            SatCheck::RemoteManagementOk => "REMOTE_MANAGEMENT_OK",
            SatCheck::BackupRestoreOk => "BACKUP_RESTORE_OK",
            SatCheck::AlarmSystemOk => "ALARM_SYSTEM_OK",
        }
    }

    /// 所有检查项。
    pub fn all() -> &'static [SatCheck] {
        &[
            SatCheck::BootOk,
            SatCheck::ProtocolCommOk,
            SatCheck::ControlLoopLatencyOk,
            SatCheck::SafetyFunctionOk,
            SatCheck::DataLoggingOk,
            SatCheck::RemoteManagementOk,
            SatCheck::BackupRestoreOk,
            SatCheck::AlarmSystemOk,
        ]
    }
}

/// 单项检查结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SatResult {
    pub check: SatCheck,
    pub passed: bool,
    /// 证据（禁止无证据通过）。
    pub evidence: String,
}

impl SatResult {
    pub fn pass(check: SatCheck, evidence: &str) -> Self {
        Self {
            check,
            passed: true,
            evidence: String::from(evidence),
        }
    }

    pub fn fail(check: SatCheck, evidence: &str) -> Self {
        Self {
            check,
            passed: false,
            evidence: String::from(evidence),
        }
    }
}

/// SAT 检查清单。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SatChecklist {
    /// 设备 ID。
    pub device_id: String,
    /// 站点名称。
    pub site_name: String,
    /// 测试时间（Unix 秒）。
    pub test_date: u64,
    /// 检查结果。
    pub results: Vec<SatResult>,
}

impl SatChecklist {
    pub fn new(device_id: &str, site_name: &str, test_date: u64) -> Self {
        Self {
            device_id: String::from(device_id),
            site_name: String::from(site_name),
            test_date,
            results: Vec::new(),
        }
    }

    /// 添加检查结果。
    pub fn add(&mut self, result: SatResult) {
        self.results.retain(|r| r.check != result.check);
        self.results.push(result);
    }

    /// 是否全部通过。
    pub fn all_passed(&self) -> bool {
        !self.results.is_empty() && self.results.iter().all(|r| r.passed)
    }

    /// 失败项数。
    pub fn failure_count(&self) -> usize {
        self.results.iter().filter(|r| !r.passed).count()
    }

    /// 已完成检查项数。
    pub fn completed_count(&self) -> usize {
        self.results.len()
    }

    /// 是否所有检查项都已完成。
    pub fn is_complete(&self) -> bool {
        self.completed_count() == SatCheck::all().len()
    }
}

/// SAT 验收报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SatReport {
    /// 报告 ID。
    pub report_id: String,
    /// 检查清单。
    pub checklist: SatChecklist,
    /// 客户代表签名（SM2 指纹）。
    pub customer_signature: String,
    /// 集成商代表签名（SM2 指纹）。
    pub integrator_signature: String,
    /// 是否通过（必须 all_passed + 双方签字）。
    pub passed: bool,
}

impl SatReport {
    /// 生成验收报告。
    ///
    /// 必须满足：
    /// 1. 检查清单完整（8 项全部完成）。
    /// 2. 所有检查项通过。
    /// 3. 客户和集成商都签字。
    pub fn generate(
        report_id: &str,
        checklist: SatChecklist,
        customer_signature: &str,
        integrator_signature: &str,
    ) -> Result<Self, &'static str> {
        if report_id.is_empty() {
            return Err("report_id is empty");
        }
        if !checklist.is_complete() {
            return Err("checklist incomplete — must run all 8 checks");
        }
        if !checklist.all_passed() {
            return Err("checklist has failures — cannot generate passed report");
        }
        if customer_signature.is_empty() {
            return Err("customer signature missing");
        }
        if integrator_signature.is_empty() {
            return Err("integrator signature missing");
        }

        Ok(Self {
            report_id: String::from(report_id),
            checklist,
            customer_signature: String::from(customer_signature),
            integrator_signature: String::from(integrator_signature),
            passed: true,
        })
    }

    /// 生成未通过报告（有失败项时）。
    pub fn generate_failed(
        report_id: &str,
        checklist: SatChecklist,
        customer_signature: &str,
        integrator_signature: &str,
    ) -> Result<Self, &'static str> {
        if !checklist.is_complete() {
            return Err("checklist incomplete");
        }
        if checklist.all_passed() {
            return Err("checklist all passed — use generate() instead");
        }

        Ok(Self {
            report_id: String::from(report_id),
            checklist,
            customer_signature: String::from(customer_signature),
            integrator_signature: String::from(integrator_signature),
            passed: false,
        })
    }
}

/// SAT 执行器。
pub struct SiteAcceptanceTest {
    checklist: SatChecklist,
}

impl SiteAcceptanceTest {
    pub fn new(device_id: &str, site_name: &str, test_date: u64) -> Self {
        Self {
            checklist: SatChecklist::new(device_id, site_name, test_date),
        }
    }

    /// 执行单项检查。
    pub fn run_check(&mut self, check: SatCheck, passed: bool, evidence: &str) -> &SatResult {
        let result = if passed {
            SatResult::pass(check, evidence)
        } else {
            SatResult::fail(check, evidence)
        };
        self.checklist.add(result);
        self.checklist.results.last().unwrap()
    }

    /// 执行全部检查（调用方提供每项的通过状态和证据）。
    pub fn run_all<F>(&mut self, mut check_fn: F) -> &SatChecklist
    where
        F: FnMut(SatCheck) -> (bool, String),
    {
        for &check in SatCheck::all() {
            let (passed, evidence) = check_fn(check);
            self.run_check(check, passed, &evidence);
        }
        &self.checklist
    }

    /// 生成报告。
    pub fn generate_report(
        &self,
        report_id: &str,
        customer_sig: &str,
        integrator_sig: &str,
    ) -> Result<SatReport, &'static str> {
        if self.checklist.all_passed() {
            SatReport::generate(
                report_id,
                self.checklist.clone(),
                customer_sig,
                integrator_sig,
            )
        } else {
            SatReport::generate_failed(
                report_id,
                self.checklist.clone(),
                customer_sig,
                integrator_sig,
            )
        }
    }

    /// 检查清单引用。
    pub fn checklist(&self) -> &SatChecklist {
        &self.checklist
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_sat_success() {
        let mut sat = SiteAcceptanceTest::new("dev-001", "Site-A", 1000);
        sat.run_all(|check| match check {
            SatCheck::BootOk => (true, String::from("boot log verified, 0 errors")),
            SatCheck::ProtocolCommOk => {
                (true, String::from("Modbus/IEC104/IEC61850 all responded"))
            }
            SatCheck::ControlLoopLatencyOk => (true, String::from("P99=4ms < 8ms deadline")),
            SatCheck::SafetyFunctionOk => (true, String::from("emergency stop verified")),
            SatCheck::DataLoggingOk => (true, String::from("SOE/trend logs complete")),
            SatCheck::RemoteManagementOk => (true, String::from("management API reachable")),
            SatCheck::BackupRestoreOk => (true, String::from("backup+restore drill passed")),
            SatCheck::AlarmSystemOk => (true, String::from("alarm hierarchy verified")),
        });

        assert!(sat.checklist().all_passed());
        assert!(sat.checklist().is_complete());

        let report = sat
            .generate_report("sat-001", "cust-sig", "integ-sig")
            .unwrap();
        assert!(report.passed);
        assert_eq!(report.customer_signature, "cust-sig");
        assert_eq!(report.integrator_signature, "integ-sig");
    }

    #[test]
    fn sat_with_failure_generates_failed_report() {
        let mut sat = SiteAcceptanceTest::new("dev-001", "Site-A", 1000);
        sat.run_all(|check| match check {
            SatCheck::ControlLoopLatencyOk => (false, String::from("P99=12ms > 8ms deadline")),
            _ => (true, String::from("ok")),
        });

        assert!(!sat.checklist().all_passed());
        assert_eq!(sat.checklist().failure_count(), 1);

        let report = sat
            .generate_report("sat-002", "cust-sig", "integ-sig")
            .unwrap();
        assert!(!report.passed);
    }

    #[test]
    fn incomplete_checklist_rejected() {
        let mut sat = SiteAcceptanceTest::new("dev-001", "Site-A", 1000);
        sat.run_check(SatCheck::BootOk, true, "ok");

        // Only 1/8 checks done
        assert!(!sat.checklist().is_complete());

        let result = sat.generate_report("sat-003", "cust", "integ");
        assert!(result.is_err());
    }

    #[test]
    fn missing_signature_rejected() {
        let mut sat = SiteAcceptanceTest::new("dev-001", "Site-A", 1000);
        sat.run_all(|_| (true, String::from("ok")));

        let result = sat.generate_report("sat-004", "", "integ-sig");
        assert!(result.is_err());

        let result = sat.generate_report("sat-005", "cust-sig", "");
        assert!(result.is_err());
    }

    #[test]
    fn empty_report_id_rejected() {
        let mut sat = SiteAcceptanceTest::new("dev-001", "Site-A", 1000);
        sat.run_all(|_| (true, String::from("ok")));

        let result = sat.generate_report("", "cust", "integ");
        assert!(result.is_err());
    }

    #[test]
    fn generate_failed_rejects_all_passed() {
        let mut sat = SiteAcceptanceTest::new("dev-001", "Site-A", 1000);
        sat.run_all(|_| (true, String::from("ok")));

        let result = SatReport::generate_failed("sat-006", sat.checklist().clone(), "c", "i");
        assert!(result.is_err()); // should use generate() instead
    }

    #[test]
    fn eight_checks_total() {
        assert_eq!(SatCheck::all().len(), 8);
    }

    #[test]
    fn check_can_be_rerun() {
        let mut sat = SiteAcceptanceTest::new("dev-001", "Site-A", 1000);
        sat.run_check(SatCheck::BootOk, false, "first attempt failed");
        sat.run_check(SatCheck::BootOk, true, "second attempt passed");

        // Should have only 1 result (latest)
        assert_eq!(sat.checklist().completed_count(), 1);
        assert!(sat.checklist().results[0].passed);
    }
}
