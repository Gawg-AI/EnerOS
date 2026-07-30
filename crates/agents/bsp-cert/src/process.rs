//! BSP 认证流程引擎

use alloc::string::String;
use alloc::vec::Vec;

use crate::spec::default_spec;
use crate::testkit::{BspTestKit, MockTestKit};
use crate::types::{BspTestReport, CertPhase, Certificate, HardwareInfo, Outcome};
use crate::CertError;

/// 认证流程 trait
pub trait CertificationProcess {
    /// 提交硬件认证申请，返回案例 ID
    fn submit(&mut self, hw: HardwareInfo) -> Result<u64, CertError>;
    /// 对指定案例执行测试，返回测试报告
    fn run_tests(&mut self, case_id: u64) -> Result<BspTestReport, CertError>;
    /// 根据测试报告进行认证判定，返回证书或拒绝
    fn certify(&mut self, case_id: u64, report: &BspTestReport) -> Result<Certificate, CertError>;
}

/// BSP 认证引擎
#[derive(Debug)]
pub struct BspCertEngine {
    /// 认证案例列表: (case_id, 硬件信息, 阶段, 测试报告)
    pub cases: Vec<(u64, HardwareInfo, CertPhase, Option<BspTestReport>)>,
    next_id: u64,
    next_cert_id: u64,
    /// 测试套件是否全部通过（用于模拟）
    pass_all: bool,
}

impl BspCertEngine {
    /// 创建认证引擎
    pub fn new(pass_all: bool) -> Self {
        Self {
            cases: Vec::new(),
            next_id: 1,
            next_cert_id: 1000,
            pass_all,
        }
    }

    fn find_case(&self, case_id: u64) -> Option<usize> {
        self.cases.iter().position(|(id, _, _, _)| *id == case_id)
    }
}

impl CertificationProcess for BspCertEngine {
    fn submit(&mut self, hw: HardwareInfo) -> Result<u64, CertError> {
        let case_id = self.next_id;
        self.next_id += 1;
        self.cases.push((case_id, hw, CertPhase::Submitted, None));
        Ok(case_id)
    }

    fn run_tests(&mut self, case_id: u64) -> Result<BspTestReport, CertError> {
        let idx = self.find_case(case_id).ok_or(CertError::CaseNotFound)?;
        let phase = self.cases[idx].2;
        if phase != CertPhase::Submitted && phase != CertPhase::Testing {
            return Err(CertError::InvalidPhase);
        }
        // 转为 Testing
        self.cases[idx].2 = CertPhase::Testing;

        let spec = default_spec();
        let mut kit = MockTestKit::new(self.pass_all);
        let report = kit.run(&spec);

        // 存储报告，转为 Report 阶段
        self.cases[idx].3 = Some(report.clone());
        self.cases[idx].2 = CertPhase::Report;

        Ok(report)
    }

    fn certify(&mut self, case_id: u64, report: &BspTestReport) -> Result<Certificate, CertError> {
        let idx = self.find_case(case_id).ok_or(CertError::CaseNotFound)?;
        let phase = self.cases[idx].2;
        if phase != CertPhase::Report {
            return Err(CertError::InvalidPhase);
        }

        // 检查所有强制项是否通过
        let spec = default_spec();
        let mandatory_ids: Vec<&String> = spec
            .items
            .iter()
            .filter(|i| i.mandatory)
            .map(|i| &i.id)
            .collect();

        let all_mandatory_passed = mandatory_ids.iter().all(|mid| {
            report
                .results
                .iter()
                .find(|r| &r.case_id == *mid)
                .map(|r| r.outcome == Outcome::Pass)
                .unwrap_or(false)
        });

        if all_mandatory_passed {
            self.cases[idx].2 = CertPhase::Certified;
            let hw = self.cases[idx].1.clone();
            let cert_id = self.next_cert_id;
            self.next_cert_id += 1;
            Ok(Certificate {
                cert_id,
                hw_vendor: hw.vendor,
                hw_model: hw.model,
                spec_version: spec.version,
                issued_at: 1700000000,
                expires_at: 1731536000,
            })
        } else {
            self.cases[idx].2 = CertPhase::Rejected;
            Err(CertError::MandatoryFailed)
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    fn sample_hw() -> HardwareInfo {
        HardwareInfo {
            vendor: String::from("HiSilicon"),
            model: String::from("Kunpeng-920"),
            cpu_arch: String::from("aarch64"),
        }
    }

    #[test]
    fn test_full_certification_flow() {
        let mut engine = BspCertEngine::new(true);
        let case_id = engine.submit(sample_hw()).unwrap();
        assert_eq!(case_id, 1);

        let report = engine.run_tests(case_id).unwrap();
        assert_eq!(report.failed, 0);

        let cert = engine.certify(case_id, &report).unwrap();
        assert_eq!(cert.hw_vendor, "HiSilicon");
        assert_eq!(cert.hw_model, "Kunpeng-920");
        assert_eq!(cert.spec_version, "1.0.0");
        assert!(cert.cert_id >= 1000);

        // 验证阶段
        let (_, _, phase, _) = &engine.cases[0];
        assert_eq!(*phase, CertPhase::Certified);
    }

    #[test]
    fn test_rejection_on_failure() {
        let mut engine = BspCertEngine::new(false);
        let case_id = engine.submit(sample_hw()).unwrap();
        let report = engine.run_tests(case_id).unwrap();
        assert_eq!(report.failed, 1);

        let result = engine.certify(case_id, &report);
        assert_eq!(result, Err(CertError::MandatoryFailed));

        let (_, _, phase, _) = &engine.cases[0];
        assert_eq!(*phase, CertPhase::Rejected);
    }

    #[test]
    fn test_invalid_case_id() {
        let mut engine = BspCertEngine::new(true);
        let result = engine.run_tests(999);
        assert_eq!(result, Err(CertError::CaseNotFound));
    }

    #[test]
    fn test_invalid_phase_for_certify() {
        let mut engine = BspCertEngine::new(true);
        let case_id = engine.submit(sample_hw()).unwrap();
        // 未执行测试就尝试认证
        let report = BspTestReport {
            total: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            results: Vec::new(),
        };
        let result = engine.certify(case_id, &report);
        assert_eq!(result, Err(CertError::InvalidPhase));
    }

    #[test]
    fn test_multiple_cases() {
        let mut engine = BspCertEngine::new(true);
        let id1 = engine.submit(sample_hw()).unwrap();
        let hw2 = HardwareInfo {
            vendor: "Phytium".to_string(),
            model: "D2000".to_string(),
            cpu_arch: "aarch64".to_string(),
        };
        let id2 = engine.submit(hw2).unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(engine.cases.len(), 2);
    }

    #[test]
    fn test_run_tests_twice_invalid_phase() {
        let mut engine = BspCertEngine::new(true);
        let case_id = engine.submit(sample_hw()).unwrap();
        let _ = engine.run_tests(case_id).unwrap();
        // 再次执行测试应失败（已在 Report 阶段）
        let result = engine.run_tests(case_id);
        assert_eq!(result, Err(CertError::InvalidPhase));
    }
}
