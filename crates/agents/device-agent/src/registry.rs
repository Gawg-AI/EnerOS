//! 设备注册表 — PointAccess 适配 + MockPointDevice + DeviceInfo + DeviceRegistry.
//!
//! ARCH-P02（ADR-0026/ADR-0027）：删除私有 `DeviceAdapter` 字符串点名旁路，
//! 统一经 [`eneros_protocol_abstract::access::PointAccess`] 访问设备点；
//! 字符串点名 → `PointId` 的映射由 [`DeviceInfo::point_map`] 承担
//! （真实协议场景由 `ProtocolPointMapping` 构建，见 ADR-0026 §5）。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;

use eneros_protocol_abstract::access::PointAccessWriter;
// Mock 类型专用导入（仅测试/仿真编译，release 由 compile_error! 熔断）。
#[cfg(any(test, feature = "mock"))]
use eneros_protocol_abstract::access::{
    AuthorizedWriter, PointAccess, PointAccessExt, WriteError, WriteOutcome,
};
#[cfg(any(test, feature = "mock"))]
use eneros_protocol_abstract::config::ProtocolType;
#[cfg(any(test, feature = "mock"))]
use eneros_protocol_abstract::error::ProtocolError;
#[cfg(any(test, feature = "mock"))]
use eneros_protocol_abstract::AuthorizedCommand;
#[cfg(any(test, feature = "mock"))]
use eneros_upa_model::{DataPoint, DataSource, PointQuality, PointType, PointValue};
use eneros_upa_model::{DeviceId, PointId};

use crate::device_type::DeviceType;

/// 适配器种类（持久化登记；与 [`DeviceType`] 一一对应，未知种类拒绝加载）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    Pcs,
    Battery,
    Bms,
    Meter,
    Temperature,
    Other,
}

impl AdapterKind {
    /// 行编码。
    pub fn code(self) -> u8 {
        match self {
            AdapterKind::Pcs => 0,
            AdapterKind::Battery => 1,
            AdapterKind::Bms => 2,
            AdapterKind::Meter => 3,
            AdapterKind::Temperature => 4,
            AdapterKind::Other => 5,
        }
    }

    /// 行编码 → 种类（未知码 → None，fail-closed 不猜）。
    pub fn from_code(code: u8) -> Option<AdapterKind> {
        match code {
            0 => Some(AdapterKind::Pcs),
            1 => Some(AdapterKind::Battery),
            2 => Some(AdapterKind::Bms),
            3 => Some(AdapterKind::Meter),
            4 => Some(AdapterKind::Temperature),
            5 => Some(AdapterKind::Other),
            _ => None,
        }
    }

    /// 从设备类型推导（未列出的类型 → Other）。
    pub fn from_device_type(t: &DeviceType) -> AdapterKind {
        match t {
            DeviceType::Pcs => AdapterKind::Pcs,
            DeviceType::Battery => AdapterKind::Battery,
            DeviceType::Bms => AdapterKind::Bms,
            DeviceType::Meter => AdapterKind::Meter,
            DeviceType::Temperature => AdapterKind::Temperature,
        }
    }
}

/// 端点引用（EIPC 服务名/协议端点地址；持久化登记）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointRef(pub String);

/// 点 schema 版本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PointSchemaVersion(pub u32);

/// 允许操作集合（位标志）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationSet(u8);

impl OperationSet {
    pub const READ: u8 = 1;
    pub const WRITE: u8 = 2;
    pub const READBACK: u8 = 4;

    /// 构造（非法位 → None）。
    pub fn new(bits: u8) -> Option<OperationSet> {
        if bits & !0b111 != 0 {
            return None;
        }
        Some(OperationSet(bits))
    }

    /// 默认：读 + 写 + 读回。
    pub fn read_write_readback() -> OperationSet {
        OperationSet(Self::READ | Self::WRITE | Self::READBACK)
    }

    /// 是否允许指定操作。
    pub fn allows(&self, op: u8) -> bool {
        self.0 & op != 0
    }

    /// 位值（持久化）。
    pub fn bits(&self) -> u8 {
        self.0
    }
}

/// 设备登记（蓝图 40 WP-40.10：持久化的设备元数据——不含任何运行期句柄）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRegistration {
    /// 设备 ID（信封路由）。
    pub device_id: DeviceId,
    /// 适配器种类。
    pub adapter_kind: AdapterKind,
    /// 端点引用。
    pub endpoint: EndpointRef,
    /// 点 schema 版本。
    pub point_schema: PointSchemaVersion,
    /// 允许操作。
    pub allowed_ops: OperationSet,
    /// 适配器能力摘要（SM3；运行期由装配注入，不持久化裸指针/CPtr）。
    pub capability_digest: [u8; 32],
    /// 配置代次（stale config 检测）。
    pub config_generation: u64,
}

/// 设备元信息（ARCH-P02：adapter 为 `PointAccess` + `AuthorizedWriter`，
/// 字符串点名经 point_map 桥接；蓝图/24 D3：写路由经 AuthorizedWriter）。
pub struct DeviceInfo {
    /// 设备类型.
    pub device_type: DeviceType,
    /// 设备 ID（信封路由：cmd.device_id() → 适配器）。
    pub device_id: DeviceId,
    /// 设备点访问适配器（读 seam + 授权写接缝）。
    pub adapter: Box<dyn PointAccessWriter>,
    /// 字符串点名 → PointId 映射（由 `ProtocolPointMapping` 构建或 mock 自动分配）.
    pub point_map: BTreeMap<String, PointId>,
    /// 蓝图 40 WP-40.10：持久化登记元数据（不含运行期句柄）。
    pub registration: DeviceRegistration,
    /// 蓝图 40 WP-40.10：适配器是否可用（factory 重建失败 → false，
    /// **不静默丢设备**；不可用设备路由时明确失败）。
    pub available: bool,
}

/// Mock 点访问设备（BTreeMap-backed，实现 `PointAccess`）.
///
/// 点值带质量位与时间戳（ADR-0027 canonical 点模型）；`set_offline` 后
/// 读点返回 `invalid` 品质（在线判断由质量位表达，而非独立布尔）。
#[cfg(any(test, feature = "mock"))]
pub struct MockPointDevice {
    /// 点 ID → 数据点.
    points: BTreeMap<PointId, DataPoint>,
    /// 下一个自增点 ID.
    next_id: PointId,
    /// 设备 ID.
    device_id: DeviceId,
    /// 离线标志（读点返回 invalid 品质）.
    offline: bool,
    /// 写失败开关（FIX-24.R3：模拟"遥测正常但写失败"——独立于离线，
    /// 离线同时影响读质量）。
    write_fail: bool,
}

#[cfg(any(test, feature = "mock"))]
impl MockPointDevice {
    /// 创建空设备（无点位，默认在线）。
    pub fn new(device_id: DeviceId) -> Self {
        Self {
            points: BTreeMap::new(),
            next_id: 1,
            device_id,
            offline: false,
            write_fail: false,
        }
    }

    /// 添加点位（自动分配 PointId，返回字符串点名 → PointId 映射项）。
    pub fn with_point(mut self, name: &str, value: f64) -> Self {
        let id = self.next_id;
        self.next_id += 1;
        self.points.insert(
            id,
            DataPoint {
                point_id: id,
                device_id: self.device_id,
                name: String::from(name),
                description: None,
                point_type: PointType::Analog,
                value: PointValue::Float(value),
                quality: PointQuality::good(),
                timestamp_ms: 0,
                source: DataSource::Internal,
                unit: None,
            },
        );
        self
    }

    /// 添加点位并显式指定 PointId（测试/仿真：对齐链策略/租约的 point_id；
    /// 与自动分配语义一致，重复 id 覆盖）。
    pub fn with_point_at(mut self, name: &str, value: f64, point_id: PointId) -> Self {
        self.next_id = self.next_id.max(point_id + 1);
        self.points.insert(
            point_id,
            DataPoint {
                point_id,
                device_id: self.device_id,
                name: String::from(name),
                description: None,
                point_type: PointType::Analog,
                value: PointValue::Float(value),
                quality: PointQuality::good(),
                timestamp_ms: 0,
                source: DataSource::Internal,
                unit: None,
            },
        );
        self
    }

    /// 收集点名为 name 的 PointId（供 DeviceInfo.point_map 使用）。
    pub fn point_id_of(&self, name: &str) -> Option<PointId> {
        self.points
            .values()
            .find(|p| p.name == name)
            .map(|p| p.point_id)
    }

    /// 设置点位值（按名称）。
    pub fn set_point(&mut self, name: &str, value: f64) {
        if let Some(id) = self.point_id_of(name) {
            if let Some(p) = self.points.get_mut(&id) {
                p.value = PointValue::Float(value);
            }
        }
    }

    /// 设置离线状态（离线时读点返回 invalid 品质）。
    pub fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    /// 设置写失败开关（FIX-24.R3：模拟协议写失败；读路径不受影响）。
    pub fn set_write_fail(&mut self, fail: bool) {
        self.write_fail = fail;
    }

    /// 全部点的时间戳设为 `ts_ms`（FIX-24.R3：CTL-010 真实反馈需要新鲜
    /// 时间戳——reconcile 对过时样本判 Stale → 不可信 → fail-safe deny）。
    pub fn touch(&mut self, ts_ms: u64) {
        for p in self.points.values_mut() {
            p.timestamp_ms = ts_ms;
        }
    }

    /// 全部点 ID 列表。
    pub fn point_ids(&self) -> alloc::vec::Vec<PointId> {
        self.points.keys().copied().collect()
    }
}

#[cfg(any(test, feature = "mock"))]
impl PointAccess for MockPointDevice {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        let mut dp = self
            .points
            .get(&point_id)
            .cloned()
            .ok_or(ProtocolError::PointNotFound)?;
        if self.offline {
            dp.quality = PointQuality::invalid();
        }
        Ok(dp)
    }

    // 蓝图/24 D1：写已移出 PointAccess（读写信任分级）。
    // TODO(WP-24.3)：为 MockPointDevice 实现 AuthorizedWriter
    // （write_authorized：设备/点匹配校验通过后才落写）。

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Internal
    }
}

#[cfg(any(test, feature = "mock"))]
impl PointAccessExt for MockPointDevice {
    fn read_device_points(
        &mut self,
        device_id: DeviceId,
    ) -> Result<alloc::vec::Vec<DataPoint>, ProtocolError> {
        if device_id != self.device_id {
            return Err(ProtocolError::PointNotFound);
        }
        Ok(self.points.values().cloned().collect())
    }
}

#[cfg(any(test, feature = "mock"))]
impl AuthorizedWriter for MockPointDevice {
    fn write_authorized(
        &mut self,
        cmd: &AuthorizedCommand,
        value: PointValue,
    ) -> Result<WriteOutcome, WriteError> {
        // 蓝图/24 §6.1：命令点/设备匹配校验通过后才落写；不匹配 → 拒绝。
        if cmd.device_id() != self.device_id {
            return Err(WriteError::CommandMismatch);
        }
        if !self.points.contains_key(&cmd.point_id()) {
            return Err(WriteError::CommandMismatch);
        }
        if self.offline || self.write_fail {
            return Err(WriteError::Protocol(ProtocolError::WriteFailed));
        }
        // 篡改防御：载荷类型与点类型矛盾 → 拒绝。
        let point_type_ok = self.points.get(&cmd.point_id()).is_some_and(|p| {
            matches!(
                (&p.point_type, &value),
                (PointType::Analog, PointValue::Float(_) | PointValue::Int(_))
                    | (
                        PointType::Digital | PointType::Control,
                        PointValue::Bool(_) | PointValue::Enum(_)
                    )
            )
        });
        if !point_type_ok {
            return Err(WriteError::Unauthorized);
        }
        let echoed = value.clone();
        if let Some(p) = self.points.get_mut(&cmd.point_id()) {
            p.value = value;
        }
        Ok(WriteOutcome {
            echoed: Some(echoed),
        })
    }
}

/// 不可用适配器占位（蓝图 40 WP-40.10：adapter factory 缺失时登记——
/// 设备保留、`available = false`，路由层明确失败，不静默丢设备）。
#[derive(Debug)]
pub struct UnavailableAdapter;

impl eneros_protocol_abstract::access::PointAccess for UnavailableAdapter {
    fn read_point(
        &mut self,
        _point_id: PointId,
    ) -> Result<eneros_upa_model::DataPoint, eneros_protocol_abstract::error::ProtocolError> {
        Err(eneros_protocol_abstract::error::ProtocolError::Unsupported)
    }

    fn protocol_type(&self) -> eneros_protocol_abstract::config::ProtocolType {
        eneros_protocol_abstract::config::ProtocolType::Internal
    }
}

impl eneros_protocol_abstract::access::AuthorizedWriter for UnavailableAdapter {
    fn write_authorized(
        &mut self,
        _cmd: &eneros_protocol_abstract::AuthorizedCommand,
        _value: eneros_upa_model::PointValue,
    ) -> Result<
        eneros_protocol_abstract::access::WriteOutcome,
        eneros_protocol_abstract::access::WriteError,
    > {
        Err(eneros_protocol_abstract::access::WriteError::Protocol(
            eneros_protocol_abstract::error::ProtocolError::Unsupported,
        ))
    }
}

/// 设备注册表（BTreeMap-backed）.
pub struct DeviceRegistry {
    /// 设备名 → 设备信息映射.
    devices: BTreeMap<String, DeviceInfo>,
}

impl DeviceRegistry {
    /// 创建空注册表.
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }

    /// 注册设备（adapter 为统一 `PointAccess` + `AuthorizedWriter`；
    /// point_map 为字符串点名 → PointId；device_id 供信封路由）。
    ///
    /// 蓝图 40 WP-40.10：登记元数据取默认（endpoint = 设备名、ops =
    /// 读+写+读回、schema v1、generation 0、能力摘要全零占位——完整
    /// 元数据经 [`Self::register_with_registration`]）。
    pub fn register(
        &mut self,
        name: &str,
        device_type: DeviceType,
        device_id: DeviceId,
        adapter: Box<dyn PointAccessWriter>,
        point_map: BTreeMap<String, PointId>,
    ) {
        let registration = DeviceRegistration {
            device_id,
            adapter_kind: AdapterKind::from_device_type(&device_type),
            endpoint: EndpointRef(String::from(name)),
            point_schema: PointSchemaVersion(1),
            allowed_ops: OperationSet::read_write_readback(),
            capability_digest: [0u8; 32],
            config_generation: 0,
        };
        self.register_with_registration(name, device_type, registration, adapter, point_map);
    }

    /// 注册设备（完整登记元数据——持久化加载/服务装配路径）。
    pub fn register_with_registration(
        &mut self,
        name: &str,
        device_type: DeviceType,
        registration: DeviceRegistration,
        adapter: Box<dyn PointAccessWriter>,
        point_map: BTreeMap<String, PointId>,
    ) {
        let available = true;
        self.devices.insert(
            String::from(name),
            DeviceInfo {
                device_type,
                device_id: registration.device_id,
                adapter,
                point_map,
                registration,
                available,
            },
        );
    }

    /// 标记设备可用性（adapter factory 重建失败 → false；不静默丢设备）。
    pub fn set_available(&mut self, name: &str, available: bool) -> bool {
        let Some(info) = self.devices.get_mut(name) else {
            return false;
        };
        info.available = available;
        true
    }

    /// 更新设备元数据（类型/ID/点映射；适配器保持——运行期句柄不重建）。
    ///
    /// 返回 Err = 设备不存在（明确失败，不静默）。
    pub fn update(
        &mut self,
        name: &str,
        device_type: DeviceType,
        device_id: DeviceId,
        point_map: BTreeMap<String, PointId>,
    ) -> Result<(), String> {
        let Some(info) = self.devices.get_mut(name) else {
            return Err(format!("unknown device '{name}'"));
        };
        info.device_type = device_type;
        info.device_id = device_id;
        info.point_map = point_map;
        info.registration.device_id = device_id;
        info.registration.adapter_kind = AdapterKind::from_device_type(&device_type);
        info.registration.config_generation = info.registration.config_generation.saturating_add(1);
        Ok(())
    }

    /// 移除设备。返回是否移除成功。
    pub fn remove(&mut self, name: &str) -> bool {
        self.devices.remove(name).is_some()
    }

    /// 设备是否存在。
    pub fn contains(&self, name: &str) -> bool {
        self.devices.contains_key(name)
    }

    /// 获取设备可变引用.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut DeviceInfo> {
        self.devices.get_mut(name)
    }

    /// 按设备 ID 路由（蓝图/24 §6.2 状态机步骤 3：device_id → 适配器）。
    pub fn find_by_device_id(&mut self, device_id: DeviceId) -> Option<&mut DeviceInfo> {
        self.devices.values_mut().find(|d| d.device_id == device_id)
    }

    /// 设备数量.
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// 可变迭代.
    pub fn iter_mut(&mut self) -> alloc::collections::btree_map::IterMut<'_, String, DeviceInfo> {
        self.devices.iter_mut()
    }

    /// 只读迭代（v0.227.0，注册表持久化需要）.
    pub fn iter(&self) -> alloc::collections::btree_map::Iter<'_, String, DeviceInfo> {
        self.devices.iter()
    }
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
