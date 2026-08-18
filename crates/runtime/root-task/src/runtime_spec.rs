//! WP-37.1（落地）：运行时服务规格（蓝图数据模型）与 `ServiceRuntime`
//! trait。蓝图 `RuntimeServiceSpec` 的 type 化表达：requirement/criticality/
//! priority/restart/readiness/capabilities/resource_budget。
//!
//! 命名纪律：seL4 实现 = target adapter（`Sel4ServiceRuntime`，接线时落地）；
//! host model 明确命名 `TestServiceRuntime`（禁止 `RealServiceRuntime`）。

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::bundle::{Criticality, Requirement};
use crate::supervisor::CapabilityGrantSet;

/// 服务标识（bundle 名）。
pub type ServiceId = String;

/// 服务 ELF 摘要（sha256）。
pub type ServiceDigest = [u8; 32];

/// 重启策略（蓝图 `RestartPolicy`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// 永不重启（root/系统关键服务）。
    Never,
    /// 总是重启（按重启预算/退避）。
    Always,
    /// 仅故障时重启（与 supervisor 退避语义一致）。
    OnFailure,
}

/// readiness 探测方式（蓝图 `ReadinessContract`：readiness 必须来自
/// IPC handshake/health probe，非"spawn 返回成功"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessProbe {
    /// IPC handshake：服务启动后必须回包握手。
    IpcHandshake,
    /// 周期健康轮询（间隔毫秒）。
    HealthPoll { interval_ms: u64 },
}

/// readiness 契约：探测方式 + 超时（超时 → `ServiceFault::ReadinessTimeout`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadinessContract {
    /// 探测方式。
    pub probe: ReadinessProbe,
    /// 超时（毫秒）——超时即失败，不无限等待。
    pub timeout_ms: u64,
}

impl ReadinessContract {
    /// 默认契约：IPC handshake + 5s 超时。
    pub fn default_handshake() -> Self {
        Self {
            probe: ReadinessProbe::IpcHandshake,
            timeout_ms: 5_000,
        }
    }

    /// 超时校验：`elapsed > timeout_ms` → 超时（fail-closed）。
    pub fn timed_out(&self, elapsed_ms: u64) -> bool {
        elapsed_ms > self.timeout_ms
    }
}

/// 资源预算（蓝图 `ResourceBudget`；mem 为 KiB，cpu 为百分比）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceBudget {
    /// 内存预算（KiB）。
    pub mem_kb: usize,
    /// CPU 预算（百分比，0.0..=100.0）。
    pub cpu_pct: f32,
}

/// WP-37.1：运行时服务规格（蓝图数据模型）——spawn/supervisor 的输入事实。
#[derive(Debug, Clone)]
pub struct RuntimeServiceSpec {
    /// 服务 id（bundle 名）。
    pub id: ServiceId,
    /// ELF 摘要（绑定介质事实）。
    pub digest: ServiceDigest,
    /// 必需性（required 阻断 / optional 降级）。
    pub requirement: Requirement,
    /// 关键性（36 compose flags 编码映射）。
    pub criticality: Criticality,
    /// 启动优先级（越小越先；DAG 排序后同层内按此序）。
    pub priority: u8,
    /// 重启策略。
    pub restart: RestartPolicy,
    /// readiness 契约。
    pub readiness: ReadinessContract,
    /// 能力授权集（唯一授权来源）。
    pub capabilities: CapabilityGrantSet,
    /// 资源预算。
    pub resource_budget: ResourceBudget,
}

/// 服务句柄（spawn/生命周期操作标识）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceHandle {
    /// 服务 id。
    pub id: ServiceId,
    /// 装配序号（区分同 id 重启实例）。
    pub instance: u64,
}

/// 服务配置（configure 阶段输入；当前为最小占位，能力/寄存器注入后续）。
#[derive(Debug, Clone, Default)]
pub struct ServiceConfig {
    /// 配置标签（如 "svc-hello/0"）。
    pub label: ServiceId,
}

/// readiness 轮询结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// 已就绪（handshake 通过 / health OK）。
    Ready,
    /// 尚未就绪（继续轮询）。
    NotReady,
}

/// 服务运行时故障（poll_readiness/stop 错误面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceFault {
    /// readiness 超时（契约超时内未 ready）。
    ReadinessTimeout { service: u16 },
    /// 服务已停止/不存在。
    NotRunning,
}

/// WP-37.1：服务运行时接口（蓝图建议接口）。seL4 实现 = target adapter
/// （接线时落地）；host model 见 [`TestServiceRuntime`]。
pub trait ServiceRuntime {
    /// 创建服务对象（TCB/VSpace/CNode 等；target adapter 持有能力所有权）。
    fn create(
        &mut self,
        spec: &RuntimeServiceSpec,
        digest: ServiceDigest,
    ) -> Result<ServiceHandle, crate::spawn::SpawnError>;
    /// 配置（能力/寄存器/优先级注入）。
    fn configure(
        &mut self,
        handle: &mut ServiceHandle,
        cfg: &ServiceConfig,
    ) -> Result<(), crate::spawn::SpawnError>;
    /// 启动（tcb_resume）。
    fn start(&mut self, handle: &mut ServiceHandle) -> Result<(), crate::spawn::SpawnError>;
    /// readiness 轮询（契约超时 → `ServiceFault::ReadinessTimeout`）。
    fn poll_readiness(
        &mut self,
        handle: &mut ServiceHandle,
        now_ms: u64,
    ) -> Result<Readiness, ServiceFault>;
    /// 停止（终态）。
    fn stop(&mut self, handle: ServiceHandle) -> Result<(), ServiceFault>;
}

/// WP-37.1：host model（**明确 Test 命名**——test-support，不冒充 target）。
/// 状态机：Created → Configured → Started → Ready（poll 推进）；stop 终态。
/// 契约超时由调用方以 `ReadinessContract.timeout_ms` 判定（Test 只模拟
/// "第 N 次 poll 后 ready" 的行为）。
#[derive(Debug, Default)]
pub struct TestServiceRuntime {
    /// 已创建的服务（id → 状态）。
    created: alloc::collections::BTreeMap<String, u8>,
    /// 下次 poll 返回 Ready 的次数（模拟握手延迟；0 = 立即 ready）。
    next_ready_after_polls: u8,
}

impl TestServiceRuntime {
    /// 新建（`polls_until_ready` = 多少次 poll 后才 Ready）。
    pub fn new(polls_until_ready: u8) -> Self {
        Self {
            created: BTreeMap::new(),
            next_ready_after_polls: polls_until_ready,
        }
    }

    /// 已创建服务数（测试断言）。
    pub fn created_count(&self) -> usize {
        self.created.len()
    }
}

impl ServiceRuntime for TestServiceRuntime {
    fn create(
        &mut self,
        spec: &RuntimeServiceSpec,
        _digest: ServiceDigest,
    ) -> Result<ServiceHandle, crate::spawn::SpawnError> {
        let instance = self.created.len() as u64 + 1;
        self.created.insert(spec.id.clone(), 0); // 0 = Created
        Ok(ServiceHandle {
            id: spec.id.clone(),
            instance,
        })
    }

    fn configure(
        &mut self,
        handle: &mut ServiceHandle,
        _cfg: &ServiceConfig,
    ) -> Result<(), crate::spawn::SpawnError> {
        if let Some(state) = self.created.get_mut(&handle.id) {
            *state = 1; // Configured
            Ok(())
        } else {
            Err(crate::spawn::SpawnError::MissingBundleModule)
        }
    }

    fn start(&mut self, handle: &mut ServiceHandle) -> Result<(), crate::spawn::SpawnError> {
        if let Some(state) = self.created.get_mut(&handle.id) {
            *state = 2; // Started
            Ok(())
        } else {
            Err(crate::spawn::SpawnError::MissingBundleModule)
        }
    }

    fn poll_readiness(
        &mut self,
        handle: &mut ServiceHandle,
        _now_ms: u64,
    ) -> Result<Readiness, ServiceFault> {
        let Some(state) = self.created.get_mut(&handle.id) else {
            return Err(ServiceFault::NotRunning);
        };
        if *state == 2 {
            if self.next_ready_after_polls > 0 {
                self.next_ready_after_polls -= 1;
                return Ok(Readiness::NotReady);
            }
            *state = 3; // Ready
            return Ok(Readiness::Ready);
        }
        Err(ServiceFault::NotRunning)
    }

    fn stop(&mut self, handle: ServiceHandle) -> Result<(), ServiceFault> {
        if self.created.remove(&handle.id).is_some() {
            Ok(())
        } else {
            Err(ServiceFault::NotRunning)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::Criticality as C;
    use crate::bundle::Requirement as R;

    fn spec(id: &str) -> RuntimeServiceSpec {
        RuntimeServiceSpec {
            id: id.to_string(),
            digest: [0u8; 32],
            requirement: R::Required,
            criticality: C::Support,
            priority: 10,
            restart: RestartPolicy::OnFailure,
            readiness: ReadinessContract::default_handshake(),
            capabilities: CapabilityGrantSet::new(),
            resource_budget: ResourceBudget {
                mem_kb: 4096,
                cpu_pct: 10.0,
            },
        }
    }

    /// WP-37.1：ReadinessContract 超时校验（fail-closed）。
    #[test]
    fn readiness_contract_timeout_semantics() {
        let c = ReadinessContract::default_handshake();
        assert!(!c.timed_out(5_000), "恰为超时不算超时");
        assert!(c.timed_out(5_001), "超过超时即失败");
        assert_eq!(c.probe, ReadinessProbe::IpcHandshake);
    }

    /// WP-37.1：TestServiceRuntime 全流程（create→configure→start→
    /// poll→ready→stop）与 NotRunning 负例。
    #[test]
    fn test_service_runtime_full_lifecycle() {
        let mut rt = TestServiceRuntime::new(1); // 1 次 poll 后 ready
        let s = spec("svc-hello");
        let mut handle = rt.create(&s, [1u8; 32]).expect("create");
        assert_eq!(rt.created_count(), 1);
        rt.configure(&mut handle, &ServiceConfig::default())
            .expect("configure");
        rt.start(&mut handle).expect("start");
        // 第 1 次 poll：NotReady；第 2 次：Ready。
        assert_eq!(
            rt.poll_readiness(&mut handle, 0).unwrap(),
            Readiness::NotReady
        );
        assert_eq!(rt.poll_readiness(&mut handle, 0).unwrap(), Readiness::Ready);
        // 停止后 NotRunning。
        rt.stop(handle.clone()).expect("stop");
        assert!(matches!(
            rt.poll_readiness(&mut handle, 0),
            Err(ServiceFault::NotRunning)
        ));
        // 未创建服务 → NotRunning。
        let mut ghost = ServiceHandle {
            id: "ghost".to_string(),
            instance: 0,
        };
        assert!(matches!(
            rt.poll_readiness(&mut ghost, 0),
            Err(ServiceFault::NotRunning)
        ));
    }

    /// WP-37.1：criticality 映射（36 compose 编码）已在 bundle.rs 测试；
    /// 此处验证 RuntimeServiceSpec 携带的字段一致。
    #[test]
    fn runtime_spec_carries_contract_fields() {
        let s = spec("svc-x");
        assert_eq!(s.requirement, R::Required);
        assert_eq!(s.criticality, C::Support);
        assert_eq!(s.priority, 10);
        assert_eq!(s.restart, RestartPolicy::OnFailure);
        assert_eq!(s.resource_budget.mem_kb, 4096);
    }
}
