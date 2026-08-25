//! Device Agent — 设备管理 Agent（实现 AgentRuntime trait）.
//!
//! 周期性采集设备状态（SOC/电压/电流/温度/功率）并执行来自 `CommandSource` 的
//! 控制命令。作为 RTOS 控制层与 Agent 层之间的桥梁。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;

use eneros_agent::{AgentDescriptor, AgentState, AgentType};
#[cfg(feature = "agent-runtime")]
use eneros_agent::{AgentRuntime, AgentRuntimeError, HeartbeatStatus};
use eneros_protocol_abstract::access::PointAccess;
use eneros_upa_model::{DataPoint, PointId, PointValue};

use crate::command::CommandSource;
#[cfg(any(test, feature = "mock"))]
use crate::command::MockCommandSource;
#[cfg(any(test, feature = "mock"))]
use crate::device_type::DeviceType;
use crate::device_type::{DeviceSnapshot, DeviceState};
use crate::ingress::{EnvelopeIngress, IngressOutcome};
use crate::registry::DeviceRegistry;
#[cfg(any(test, feature = "mock"))]
use crate::registry::MockPointDevice;

/// 设备管理 Agent.
///
/// 持有 `DeviceRegistry`（多设备注册表）和 `CommandSource`（命令源），
/// 在 `on_tick` 中执行设备状态采集和命令执行。
pub struct DeviceAgent {
    /// Agent 描述符.
    pub descriptor: AgentDescriptor,
    /// 设备注册表.
    pub devices: DeviceRegistry,
    /// 命令源.
    pub command_source: Box<dyn CommandSource>,
    /// 最近状态快照.
    pub last_snapshot: DeviceSnapshot,
    /// 生命周期状态.
    pub state: AgentState,
    /// tick 计数.
    pub tick_count: u64,
    /// 旧匿名命令拒绝计数（蓝图/24 §6.2 双轨：收到不执行，仅留痕）。
    pub legacy_denied: u64,
    /// 信封输入收口点（蓝图/24 §6.2：唯一写裁决收口）。
    pub ingress: EnvelopeIngress,
}

impl DeviceAgent {
    /// 构造 Device Agent（D3：使用 AgentDescriptor::new）.
    ///
    /// `descriptor = AgentDescriptor::new(AgentType::Device, name, now_ms)`。
    /// `devices` 为空注册表，由调用方通过 `registry_mut()` 注册设备。
    pub fn new(name: &str, command_source: Box<dyn CommandSource>, now_ms: u64) -> Self {
        Self::new_with_ingress(
            name,
            command_source,
            now_ms,
            // 默认收口点：空策略/空安全点/零密钥 → 一切命令 default-deny
            // （fail-closed）；生产装配必须使用 EnvelopeIngress::new 注入。
            EnvelopeIngress::new(
                b"",
                [0u8; 16],
                eneros_control_gateway::lease::LeaseScope {
                    devices: alloc::vec::Vec::new(),
                    points: alloc::vec::Vec::new(),
                    max_class: 0,
                },
                0,
                0,
                eneros_control_gateway::policy::Policy::new(1),
                60,
                &[],
                now_ms,
            ),
        )
    }

    /// 构造设备 Agent 并注入信封收口点（蓝图/24 §7.4 装配检查点：
    /// HMAC 密钥/lease_id/重启计数经装配层传入 EnvelopeIngress::new）。
    pub fn new_with_ingress(
        name: &str,
        command_source: Box<dyn CommandSource>,
        now_ms: u64,
        ingress: EnvelopeIngress,
    ) -> Self {
        Self {
            descriptor: AgentDescriptor::new(AgentType::Device, name, now_ms),
            devices: DeviceRegistry::new(),
            command_source,
            last_snapshot: DeviceSnapshot::new(),
            state: AgentState::Created,
            tick_count: 0,
            legacy_denied: 0,
            ingress,
        }
    }

    /// 默认构造（预注册 3 个 Mock 设备：pcs/battery/meter）——
    /// 仅测试/仿真（蓝图/20 R2-1：生产必须显式注册真实设备）。
    #[cfg(any(test, feature = "mock"))]
    pub fn new_default(now_ms: u64) -> Self {
        let mut devices = DeviceRegistry::new();
        // PCS：功率变换系统
        {
            let dev = MockPointDevice::new(1)
                .with_point("soc", 0.5)
                .with_point("voltage", 400.0)
                .with_point("current", 100.0)
                .with_point("temperature", 35.0)
                .with_point("power", 0.0);
            let mut pm = BTreeMap::new();
            for n in ["soc", "voltage", "current", "temperature", "power"] {
                if let Some(id) = dev.point_id_of(n) {
                    pm.insert(String::from(n), id);
                }
            }
            devices.register("pcs", DeviceType::Pcs, 1, Box::new(dev), pm);
        }
        // Battery：电池
        {
            let dev = MockPointDevice::new(2)
                .with_point("soc", 0.65)
                .with_point("voltage", 48.0)
                .with_point("current", 50.0)
                .with_point("temperature", 28.0)
                .with_point("power", 25.0);
            let mut pm = BTreeMap::new();
            for n in ["soc", "voltage", "current", "temperature", "power"] {
                if let Some(id) = dev.point_id_of(n) {
                    pm.insert(String::from(n), id);
                }
            }
            devices.register("battery", DeviceType::Battery, 2, Box::new(dev), pm);
        }
        // Meter：电表
        {
            let dev = MockPointDevice::new(3)
                .with_point("voltage", 220.0)
                .with_point("current", 30.0)
                .with_point("power", 6600.0);
            let mut pm = BTreeMap::new();
            for n in ["voltage", "current", "power"] {
                if let Some(id) = dev.point_id_of(n) {
                    pm.insert(String::from(n), id);
                }
            }
            devices.register("meter", DeviceType::Meter, 3, Box::new(dev), pm);
        }
        Self::new_with_ingress(
            "device-agent",
            Box::new(MockCommandSource::new()),
            now_ms,
            EnvelopeIngress::new(
                b"",
                [0u8; 16],
                eneros_control_gateway::lease::LeaseScope {
                    devices: alloc::vec::Vec::new(),
                    points: alloc::vec::Vec::new(),
                    max_class: 0,
                },
                0,
                0,
                eneros_control_gateway::policy::Policy::new(1),
                60,
                &[],
                now_ms,
            ),
        )
        .with_devices(devices)
    }

    /// 装配辅助：注入预注册设备（new_default 用）。
    #[cfg(any(test, feature = "mock"))]
    fn with_devices(mut self, devices: DeviceRegistry) -> Self {
        self.devices = devices;
        self
    }

    /// 获取设备注册表可变引用（供测试注册设备）.
    pub fn registry_mut(&mut self) -> &mut DeviceRegistry {
        &mut self.devices
    }

    /// 获取最近状态快照.
    pub fn last_snapshot(&self) -> &DeviceSnapshot {
        &self.last_snapshot
    }

    /// 采集设备状态（D5：返回 DeviceSnapshot，存入 last_snapshot）.
    ///
    /// 遍历所有设备，经 `PointAccess` + `point_map` 桥接读取 soc/voltage/current/
    /// temperature/power（ARCH-P02：字符串点名 → PointId → DataPoint，读点带
    /// 质量位与时间戳）。读取失败或质量位 invalid 时标记 `online: false`，
    /// 不中断采集。
    fn poll_devices(&mut self, now_ms: u64) {
        let mut snapshot = DeviceSnapshot::new();
        for (name, info) in self.devices.iter_mut() {
            let adapter = &mut info.adapter;
            let read_f = |adapter: &mut dyn PointAccess,
                          point_map: &BTreeMap<String, PointId>,
                          key: &str| {
                point_map
                    .get(key)
                    .and_then(|&id| adapter.read_point(id).ok())
            };
            let soc = read_f(adapter, &info.point_map, "soc");
            let voltage = read_f(adapter, &info.point_map, "voltage");
            let current = read_f(adapter, &info.point_map, "current");
            let temperature = read_f(adapter, &info.point_map, "temperature");
            let power = read_f(adapter, &info.point_map, "power");

            let all_valid = [&soc, &voltage, &current, &temperature, &power]
                .iter()
                .all(|dp| match dp {
                    Some(p) => p.quality.valid,
                    None => false,
                });
            let f = |dp: &Option<DataPoint>| match dp {
                Some(p) => match &p.value {
                    PointValue::Float(v) => *v,
                    PointValue::Int(i) => *i as f64,
                    _ => 0.0,
                },
                None => 0.0,
            };
            let last_update = [&soc, &voltage, &current, &temperature, &power]
                .iter()
                .filter_map(|dp| dp.as_ref())
                .map(|p| p.timestamp_ms)
                .max()
                .unwrap_or(now_ms);
            snapshot.set(
                name,
                DeviceState {
                    soc: f(&soc),
                    voltage: f(&voltage),
                    current: f(&current),
                    temperature: f(&temperature),
                    power: f(&power),
                    online: all_valid,
                    last_update_ms: last_update,
                },
            );
        }
        self.last_snapshot = snapshot;
    }

    /// 执行命令（蓝图/24 §6.2 双轨过渡期临时处置）。
    ///
    /// 旧匿名 `CommandSource` 输入的 `DeviceCommand` **收到不执行**——
    /// 记 `legacy_denied` 计数后丢弃（§6.2 双轨：旧匿名路径只拒不执行）。
    /// 设备未找到同样跳过，不中断（D14 语义保留）。
    ///
    /// TODO(WP-24.3)：此处接信封输入状态机（帧解码 → ControlChain 复验 →
    /// device_id 路由 → point_id 匹配 → `write_authorized`）；`legacy_denied`
    /// 改为 `eneros_write_denied_legacy_total` 指标 + 审计事件（§6.6）。
    fn execute_commands(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError> {
        while let Some(_cmd) = self.command_source.try_read() {
            // 双轨（§6.2/D12）：匿名命令收到不执行，记 write_denied_legacy
            // 指标 + 审计 + 本地计数。
            self.legacy_denied += 1;
            self.ingress.record_legacy_denied(now_ms);
        }
        Ok(())
    }

    /// 处理一帧信封投递（EIPC 载荷 → 复验 → 路由 → 落写；§6.2 状态机）。
    ///
    /// 返回外部可观察结果（Applied/IdempotentHit/DryRun/Denied），
    /// 全部失败路径已有指标 + 审计，不 panic。
    pub fn process_envelope(&mut self, payload: &[u8], now_ms: u64) -> IngressOutcome {
        self.ingress.process(&mut self.devices, payload, now_ms)
    }
}

#[cfg(feature = "agent-runtime")]
impl AgentRuntime for DeviceAgent {
    fn descriptor(&self) -> &AgentDescriptor {
        &self.descriptor
    }

    fn on_start(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.state = AgentState::Running;
        Ok(())
    }

    fn on_tick(&mut self, now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.poll_devices(now_ms);
        self.execute_commands(now_ms)?;
        self.tick_count += 1;
        Ok(())
    }

    fn on_stop(&mut self, _now_ms: u64) -> Result<(), AgentRuntimeError> {
        self.state = AgentState::Dead;
        Ok(())
    }

    fn on_heartbeat(&self, _now_ms: u64) -> HeartbeatStatus {
        if self.state == AgentState::Running {
            HeartbeatStatus::Alive
        } else {
            HeartbeatStatus::Dead
        }
    }
}
