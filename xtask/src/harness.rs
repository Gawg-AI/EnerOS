//! 42 — 零硬件验收 Harness 抽象。
//!
//! 蓝图 42 §6/§7 定义独立 peer 合约与 soak harness adapter。这里把这些
//! 接口落成 Rust trait，供后续接入真实 peer、QEMU/软件 adapter 或 L4
//! lab/hardware adapter 使用。当前只定义契约与最小类型，不代替产品实现。

// 44.2 收口登记：本模块为 42 卷「零硬件验收 Harness 抽象」契约/最小类型库，
// 当前未接线（51/52 卷 bring-up 接入真实 peer/QEMU adapter 后启用）；
// dead_code 定向豁免，解冻条件见 44.4 冻结登记。
#![allow(dead_code)]

use std::fmt;

/// 对端身份：必须是可审计字符串，禁止匿名对端。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIdentity(pub String);

/// 对端软件版本，用于证据记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version(pub String);

/// 测试网络描述：当前支持软件网络，后续可扩展为物理端口/VLAN。
#[derive(Debug, Clone)]
pub struct TestNetwork {
    pub name: String,
    pub endpoint: Option<String>,
}

/// 单个互操作用例。
#[derive(Debug, Clone)]
pub struct InteropCase {
    pub id: String,
    pub description: String,
}

/// 独立 peer 对单个用例的观察结果。
#[derive(Debug, Clone)]
pub struct PeerObservation {
    pub case_id: String,
    pub ok: bool,
    pub detail: String,
}

/// Harness 错误：统一 fail-closed，禁止把错误转成空成功。
#[derive(Debug)]
pub enum HarnessError {
    NotAvailable(String),
    Timeout(String),
    PeerMismatch(String),
    Io(String),
    Other(String),
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAvailable(msg) => write!(f, "not-available: {msg}"),
            Self::Timeout(msg) => write!(f, "timeout: {msg}"),
            Self::PeerMismatch(msg) => write!(f, "peer-mismatch: {msg}"),
            Self::Io(msg) => write!(f, "io: {msg}"),
            Self::Other(msg) => write!(f, "other: {msg}"),
        }
    }
}

impl std::error::Error for HarnessError {}

/// 蓝图 42 WP-42.7：独立互操作 peer 合约。
///
/// 允许独立开源实现、独立自写 raw peer、实验室商业设备 adapter。
/// 禁止调用同一 EnerOS codec 两端自证并标记为第三方互操作。
pub trait InteropPeer {
    fn identity(&self) -> PeerIdentity;
    fn version(&self) -> Version;
    fn start(&mut self, network: &TestNetwork) -> Result<(), HarnessError>;
    fn execute(&mut self, case: &InteropCase) -> Result<PeerObservation, HarnessError>;
    fn stop(&mut self) -> Result<(), HarnessError>;
}

/// 候选引用：必须绑定 candidate lineage。
#[derive(Debug, Clone)]
pub struct CandidateRef {
    pub candidate_id: String,
}

/// 部署结果。
#[derive(Debug, Clone)]
pub struct Deployment {
    pub deployment_id: String,
}

/// 工作负载计划。
#[derive(Debug, Clone)]
pub struct WorkloadPlan {
    pub plan_id: String,
    pub description: String,
}

/// 已启动工作负载 ID。
#[derive(Debug, Clone)]
pub struct WorkloadId(pub String);

/// 故障规格：必须有注入确认与期望 oracle。
#[derive(Debug, Clone)]
pub struct FaultSpec {
    pub fault_id: String,
    pub kind: String,
    pub detail: String,
}

/// 已注入故障 ID。
#[derive(Debug, Clone)]
pub struct FaultId(pub String);

/// 观测样本。
#[derive(Debug, Clone)]
pub struct ObservationSample {
    pub timestamp_ms: u64,
    pub key: String,
    pub value: String,
}

/// 观测批次。
#[derive(Debug, Clone)]
pub struct ObservationBatch {
    pub samples: Vec<ObservationSample>,
}

/// 蓝图 42 WP-42.9：Soak Harness Adapter。
///
/// 42 实现 QEMU/software adapter；43 增加 lab/hardware adapter。
/// 未配置 adapter 时必须返回 `HarnessError::NotAvailable`，禁止空执行器。
pub trait SoakHarnessAdapter {
    fn deploy(&mut self, candidate: CandidateRef) -> Result<Deployment, HarnessError>;
    fn start_workload(&mut self, plan: WorkloadPlan) -> Result<WorkloadId, HarnessError>;
    fn inject_fault(&mut self, fault: FaultSpec) -> Result<FaultId, HarnessError>;
    fn observe(&mut self) -> Result<ObservationBatch, HarnessError>;
    fn stop(&mut self) -> Result<(), HarnessError>;
}

/// 蓝图 42 WP-42.9：QEMU/software Soak Adapter。
///
/// 该适配器把 `cargo xtask qa soak-run` 封装为 `SoakHarnessAdapter`，
/// 作为 42 的 L2/L3 软件 soak 执行器。未支持任意故障注入时返回
/// `HarnessError::NotAvailable`，禁止把“未注入”当成成功。
pub struct QemuSoakAdapter {
    image: std::path::PathBuf,
    vars_template: Option<std::path::PathBuf>,
    out: std::path::PathBuf,
    cycles: usize,
    min_duration_days: u64,
    last_campaign: Option<serde_json::Value>,
}

impl QemuSoakAdapter {
    pub fn new(
        image: std::path::PathBuf,
        vars_template: Option<std::path::PathBuf>,
        out: std::path::PathBuf,
        cycles: usize,
        min_duration_days: u64,
    ) -> Self {
        Self {
            image,
            vars_template,
            out,
            cycles,
            min_duration_days,
            last_campaign: None,
        }
    }
}

impl SoakHarnessAdapter for QemuSoakAdapter {
    fn deploy(&mut self, candidate: CandidateRef) -> Result<Deployment, HarnessError> {
        if candidate.candidate_id.is_empty() {
            return Err(HarnessError::Other("candidate_id is empty".into()));
        }
        if !self.image.is_file() {
            return Err(HarnessError::NotAvailable(format!(
                "QEMU image not found: {}",
                self.image.display()
            )));
        }
        Ok(Deployment {
            deployment_id: format!(
                "qemu-{}",
                candidate.candidate_id.chars().take(12).collect::<String>()
            ),
        })
    }

    fn start_workload(&mut self, plan: WorkloadPlan) -> Result<WorkloadId, HarnessError> {
        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("xtask").arg("qa").arg("soak-run");
        cmd.arg("--cycles").arg(self.cycles.to_string());
        cmd.arg("--image").arg(&self.image);
        if let Some(vars) = &self.vars_template {
            cmd.arg("--vars-template").arg(vars);
        }
        cmd.arg("--min-duration-days")
            .arg(self.min_duration_days.to_string());
        cmd.arg("--out").arg(&self.out);
        let output = cmd
            .output()
            .map_err(|e| HarnessError::Io(format!("spawn soak-run: {e}")))?;
        if !output.status.success() {
            return Err(HarnessError::Other(format!(
                "soak-run failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        self.last_campaign = std::fs::read(&self.out)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Ok(WorkloadId(format!("{}-{}", plan.plan_id, self.cycles)))
    }

    fn inject_fault(&mut self, fault: FaultSpec) -> Result<FaultId, HarnessError> {
        // QEMU software adapter 当前不支持任意 fault injection；明确返回
        // NotAvailable，避免把“未注入”当成 PASS。
        Err(HarnessError::NotAvailable(format!(
            "QemuSoakAdapter does not support fault kind '{}' yet",
            fault.kind
        )))
    }

    fn observe(&mut self) -> Result<ObservationBatch, HarnessError> {
        let campaign = self
            .last_campaign
            .as_ref()
            .ok_or_else(|| HarnessError::NotAvailable("no soak campaign has been run".into()))?;
        let mut samples = Vec::new();
        if let Some(cycles) = campaign.get("metric_samples").and_then(|v| v.as_u64()) {
            samples.push(ObservationSample {
                timestamp_ms: 0,
                key: "metric_samples".into(),
                value: cycles.to_string(),
            });
        }
        if let Some(duration) = campaign.get("duration_days").and_then(|v| v.as_u64()) {
            samples.push(ObservationSample {
                timestamp_ms: 0,
                key: "duration_days".into(),
                value: duration.to_string(),
            });
        }
        if let Some(restarts) = campaign.get("restarts").and_then(|v| v.as_array()) {
            samples.push(ObservationSample {
                timestamp_ms: 0,
                key: "restarts".into(),
                value: restarts.len().to_string(),
            });
        }
        Ok(ObservationBatch { samples })
    }

    fn stop(&mut self) -> Result<(), HarnessError> {
        Ok(())
    }
}

/// 未配置 peer 的 fail-closed 实现。
///
/// 当没有真实/独立 peer 可用时，`InteropPeer` 应返回 `NotAvailable`，
/// 不能返回空成功或伪造 PASS。
pub struct UnavailableInteropPeer;

impl InteropPeer for UnavailableInteropPeer {
    fn identity(&self) -> PeerIdentity {
        PeerIdentity("unavailable".into())
    }

    fn version(&self) -> Version {
        Version("0.0.0-unavailable".into())
    }

    fn start(&mut self, _network: &TestNetwork) -> Result<(), HarnessError> {
        Err(HarnessError::NotAvailable(
            "no independent peer configured".into(),
        ))
    }

    fn execute(&mut self, _case: &InteropCase) -> Result<PeerObservation, HarnessError> {
        Err(HarnessError::NotAvailable(
            "no independent peer configured".into(),
        ))
    }

    fn stop(&mut self) -> Result<(), HarnessError> {
        Ok(())
    }
}

/// 外部脚本互操作 peer 适配器。
///
/// 用于接入独立 peer 脚本（如 `tests/interop/<peer>.sh` 或实验室脚本）。
/// 脚本必须自身产生可验证的 PASS 语义；本适配器不会把退出 0 直接当成
/// PASS，而是要求输出中包含 `RESULT=PASS` 或 JSON `"verdict":"PASS"`。
/// 未配置脚本或脚本失败时返回错误，禁止空成功。
pub struct ScriptInteropPeer {
    identity: PeerIdentity,
    version: Version,
    script: std::path::PathBuf,
    envs: Vec<(String, String)>,
    last_output: Option<String>,
}

impl ScriptInteropPeer {
    pub fn new(
        identity: PeerIdentity,
        version: Version,
        script: std::path::PathBuf,
        envs: Vec<(String, String)>,
    ) -> Self {
        Self {
            identity,
            version,
            script,
            envs,
            last_output: None,
        }
    }

    fn output_has_pass(output: &str) -> bool {
        output.contains("RESULT=PASS")
            || output.contains("\"verdict\":\"PASS\"")
            || output.contains("\"verdict\": \"PASS\"")
    }
}

impl InteropPeer for ScriptInteropPeer {
    fn identity(&self) -> PeerIdentity {
        self.identity.clone()
    }

    fn version(&self) -> Version {
        self.version.clone()
    }

    fn start(&mut self, _network: &TestNetwork) -> Result<(), HarnessError> {
        if !self.script.is_file() {
            return Err(HarnessError::NotAvailable(format!(
                "interop script not found: {}",
                self.script.display()
            )));
        }
        Ok(())
    }

    fn execute(&mut self, case: &InteropCase) -> Result<PeerObservation, HarnessError> {
        if !self.script.is_file() {
            return Err(HarnessError::NotAvailable(format!(
                "interop script not found: {}",
                self.script.display()
            )));
        }
        let mut cmd = std::process::Command::new("bash");
        cmd.arg(&self.script).arg(&case.id);
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }
        let output = cmd
            .output()
            .map_err(|e| HarnessError::Io(format!("spawn interop script: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        self.last_output = Some(format!("{stdout}\n{stderr}"));
        if !output.status.success() {
            return Err(HarnessError::Other(format!(
                "interop script failed: {}",
                stderr.trim()
            )));
        }
        if !Self::output_has_pass(&stdout) {
            return Err(HarnessError::Other(
                "interop script exited 0 but no RESULT=PASS or verdict=PASS found".into(),
            ));
        }
        Ok(PeerObservation {
            case_id: case.id.clone(),
            ok: true,
            detail: stdout,
        })
    }

    fn stop(&mut self) -> Result<(), HarnessError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakePeer;

    impl InteropPeer for FakePeer {
        fn identity(&self) -> PeerIdentity {
            PeerIdentity("fake-peer".into())
        }
        fn version(&self) -> Version {
            Version("0.0.0-test".into())
        }
        fn start(&mut self, _network: &TestNetwork) -> Result<(), HarnessError> {
            Ok(())
        }
        fn execute(&mut self, case: &InteropCase) -> Result<PeerObservation, HarnessError> {
            Ok(PeerObservation {
                case_id: case.id.clone(),
                ok: true,
                detail: "fake observation".into(),
            })
        }
        fn stop(&mut self) -> Result<(), HarnessError> {
            Ok(())
        }
    }

    struct FakeSoak;

    impl SoakHarnessAdapter for FakeSoak {
        fn deploy(&mut self, _candidate: CandidateRef) -> Result<Deployment, HarnessError> {
            Ok(Deployment {
                deployment_id: "dep-1".into(),
            })
        }
        fn start_workload(&mut self, _plan: WorkloadPlan) -> Result<WorkloadId, HarnessError> {
            Ok(WorkloadId("wl-1".into()))
        }
        fn inject_fault(&mut self, _fault: FaultSpec) -> Result<FaultId, HarnessError> {
            Ok(FaultId("fault-1".into()))
        }
        fn observe(&mut self) -> Result<ObservationBatch, HarnessError> {
            Ok(ObservationBatch { samples: vec![] })
        }
        fn stop(&mut self) -> Result<(), HarnessError> {
            Ok(())
        }
    }

    #[test]
    fn interop_peer_trait_is_object_safe_and_usable() {
        let mut peer: Box<dyn InteropPeer> = Box::new(FakePeer);
        let net = TestNetwork {
            name: "loopback".into(),
            endpoint: Some("127.0.0.1:1".into()),
        };
        peer.start(&net).unwrap();
        let obs = peer
            .execute(&InteropCase {
                id: "read".into(),
                description: "read".into(),
            })
            .unwrap();
        assert!(obs.ok);
        peer.stop().unwrap();
        assert_eq!(peer.identity().0, "fake-peer");
    }

    #[test]
    fn soak_adapter_trait_is_object_safe_and_usable() {
        let mut adapter: Box<dyn SoakHarnessAdapter> = Box::new(FakeSoak);
        let dep = adapter
            .deploy(CandidateRef {
                candidate_id: "cand".into(),
            })
            .unwrap();
        assert_eq!(dep.deployment_id, "dep-1");
        let wl = adapter
            .start_workload(WorkloadPlan {
                plan_id: "p".into(),
                description: "plan".into(),
            })
            .unwrap();
        assert_eq!(wl.0, "wl-1");
        let _ = adapter.observe().unwrap();
        adapter.stop().unwrap();
    }

    #[test]
    fn qemu_soak_adapter_fails_closed_for_missing_image_and_unsupported_fault() {
        let mut adapter = QemuSoakAdapter::new(
            std::path::PathBuf::from("/definitely/missing/raw"),
            None,
            std::path::PathBuf::from("target/soak-adapter-test.json"),
            1,
            0,
        );
        let dep = adapter.deploy(CandidateRef {
            candidate_id: "cand".into(),
        });
        assert!(matches!(dep, Err(HarnessError::NotAvailable(_))));

        let fault = adapter.inject_fault(FaultSpec {
            fault_id: "f".into(),
            kind: "power-cut".into(),
            detail: "not supported".into(),
        });
        assert!(matches!(fault, Err(HarnessError::NotAvailable(_))));
    }

    #[test]
    fn unavailable_interop_peer_fails_closed() {
        let mut peer = UnavailableInteropPeer;
        let net = TestNetwork {
            name: "none".into(),
            endpoint: None,
        };
        assert!(matches!(
            peer.start(&net),
            Err(HarnessError::NotAvailable(_))
        ));
        assert!(matches!(
            peer.execute(&InteropCase {
                id: "read".into(),
                description: "read".into(),
            }),
            Err(HarnessError::NotAvailable(_))
        ));
        peer.stop().unwrap();
    }

    #[test]
    fn script_interop_peer_fails_closed_without_pass_marker() {
        let dir = std::env::temp_dir().join(format!("eneros-script-peer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("peer.sh");
        std::fs::write(
            &script,
            "#!/usr/bin/env bash
echo 'no pass marker'
exit 0
",
        )
        .unwrap();
        let mut peer = ScriptInteropPeer::new(
            PeerIdentity("script-peer".into()),
            Version("1.0".into()),
            script.clone(),
            vec![],
        );
        let net = TestNetwork {
            name: "loopback".into(),
            endpoint: None,
        };
        peer.start(&net).unwrap();
        let r = peer.execute(&InteropCase {
            id: "read".into(),
            description: "read".into(),
        });
        assert!(r.is_err(), "script without PASS marker must fail closed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
