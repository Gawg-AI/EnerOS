//! 虚拟 PCS 设备（蓝图 40 Exit #10 "external virtual device" 设备侧）。
//!
//! 进程内虚拟设备模型（产品装配代码，非 test mock）：写命令产生**真实
//! 内部副作用**（功率寄存器状态迁移 + 写计数），读回走独立
//! [`PointAccess::read_point`] 操作（非回显）。遥测点（直流母线电压/
//! SOC/并网/过流/过温）随读时刷新时间戳——模拟持续采集的设备模型，
//! 供 ingress `build_feedback` 构建新鲜可信反馈。
//!
//! 装配边界（诚实声明）：真实协议设备（Modbus/IEC104）依赖 39 的
//! transport 注入（CVR-39-02 设备句柄移交，41 Entry 前置）；本设备用于
//! 40 的产品闭环实弹——写/读回为真实寄存器语义，串口证据行
//! `VIRTUAL-PCS` 为外部可观察面。profile 声明 `virtual-pcs` 能力时装配。

use eneros_protocol_abstract::access::{AuthorizedWriter, PointAccess, WriteError, WriteOutcome};
use eneros_protocol_abstract::error::ProtocolError;
use eneros_protocol_abstract::AuthorizedCommand;
use eneros_upa_model::{
    DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue,
};

/// 虚拟 PCS 设备 ID（与 dispatch/device-agent 装配同源）。
pub const VIRTUAL_PCS_DEVICE_ID: DeviceId = 1;
/// 直流母线电压点（遥测）。
pub const POINT_DC_BUS_V: PointId = 1;
/// SOC 点（遥测）。
pub const POINT_SOC: PointId = 4;
/// 并网状态点（遥信）。
pub const POINT_GRID_UP: PointId = 5;
/// 过流/过温告警点（遥信，复用点号）。
pub const POINT_ALARM: PointId = 6;
/// 有功功率点（**写点**——控制目标）。
pub const POINT_POWER: PointId = 10;

/// 点名 → 点号（registry `point_map` 装配源）。
pub const VIRTUAL_PCS_POINTS: &[(&str, PointId)] = &[
    ("dc_bus_v", POINT_DC_BUS_V),
    ("soc_pct", POINT_SOC),
    ("grid_up", POINT_GRID_UP),
    ("overcurrent", POINT_ALARM),
    ("overtemp", POINT_ALARM),
    ("active_power", POINT_POWER),
];

/// 虚拟 PCS 设备模型。
pub struct VirtualPcsDevice {
    device_id: DeviceId,
    /// 功率寄存器（写命令的真实副作用落点）。
    power_kw: f64,
    /// 写命令计数（外部可观察证据）。
    write_count: u64,
    /// 单调毫秒时钟（读点时间戳刷新）。
    now_ms: fn() -> u64,
    /// SOC 读计数（SPEC-E WP-E.3：确定性振荡相位）。
    soc_reads: u64,
}

impl VirtualPcsDevice {
    /// 构造（`now_ms` = 单调毫秒时钟注入）。
    pub fn new(device_id: DeviceId, now_ms: fn() -> u64) -> Self {
        Self {
            device_id,
            power_kw: 0.0,
            write_count: 0,
            now_ms,
            soc_reads: 0,
        }
    }

    /// 写命令计数（写副作用的最小外部证据）。
    pub fn write_count(&self) -> u64 {
        self.write_count
    }

    /// 当前功率寄存器值（诊断）。
    pub fn power_kw(&self) -> f64 {
        self.power_kw
    }

    fn point_name(point: PointId) -> &'static str {
        match point {
            POINT_DC_BUS_V => "dc_bus_v",
            POINT_SOC => "soc_pct",
            POINT_GRID_UP => "grid_up",
            POINT_ALARM => "alarm",
            POINT_POWER => "active_power",
            _ => "unknown",
        }
    }

    fn read_value(&mut self, point: PointId) -> Option<PointValue> {
        match point {
            POINT_DC_BUS_V => Some(PointValue::Float(400.0)),
            POINT_SOC => {
                // SPEC-E WP-E.3：确定性双值振荡 0.47↔0.53（逐读翻转）。
                // 跨界值持续喂死区学习闭环（|Δsoc|=0.06 噪声序列），并使
                // System1 在学习前后对同域 SOC 给出不同判定（C3 前后对照帧）；
                // 相位确定性 ⇒ transcript 逐字节可复现。
                self.soc_reads = self.soc_reads.wrapping_add(1);
                let v = if self.soc_reads & 1 == 0 { 0.47 } else { 0.53 };
                Some(PointValue::Float(v))
            }
            POINT_GRID_UP => Some(PointValue::Bool(true)),
            POINT_ALARM => Some(PointValue::Bool(false)),
            POINT_POWER => Some(PointValue::Float(self.power_kw)),
            _ => None,
        }
    }
}

impl PointAccess for VirtualPcsDevice {
    fn protocol_type(&self) -> eneros_protocol_abstract::config::ProtocolType {
        eneros_protocol_abstract::config::ProtocolType::Internal
    }

    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        let value = self
            .read_value(point_id)
            .ok_or(ProtocolError::PointNotFound)?;
        let point_type = match &value {
            PointValue::Bool(_) => PointType::Digital,
            _ => PointType::Analog,
        };
        Ok(DataPoint {
            point_id,
            device_id: self.device_id,
            name: alloc::string::String::from(Self::point_name(point_id)),
            description: None,
            point_type,
            value,
            quality: PointQuality::good(),
            // 读时刷新（模拟持续采集——反馈新鲜度由该时间戳承载）。
            timestamp_ms: (self.now_ms)(),
            source: DataSource::Internal,
            unit: None,
        })
    }
}

impl AuthorizedWriter for VirtualPcsDevice {
    fn write_authorized(
        &mut self,
        cmd: &AuthorizedCommand,
        value: PointValue,
    ) -> Result<WriteOutcome, WriteError> {
        // 设备/点匹配二次校验（与协议 adapter 同契约：不匹配零副作用）。
        if cmd.device_id() != self.device_id || cmd.point_id() != POINT_POWER {
            return Err(WriteError::CommandMismatch);
        }
        let v = match value {
            PointValue::Float(f) => f,
            PointValue::Int(i) => i as f64,
            _ => return Err(WriteError::Unauthorized),
        };
        if !v.is_finite() {
            return Err(WriteError::Unauthorized);
        }
        self.power_kw = v;
        self.write_count += 1;
        #[cfg(target_os = "none")]
        eneros_runtime::println!(
            "[virtual-pcs] WRITE point={POINT_POWER} value={v} total={}",
            self.write_count
        );
        // echoed=None：读回由 ingress 独立 read_point 完成（WP-40.11——
        // 读回不得回显刚写 payload）。
        Ok(WriteOutcome { echoed: None })
    }
}

// PointAccessWriter 为 blanket impl（PointAccess + AuthorizedWriter 自动获得）。

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> u64 {
        1_700_000_000_000
    }

    #[test]
    fn telemetry_points_read_fresh_good_quality() {
        let mut dev = VirtualPcsDevice::new(VIRTUAL_PCS_DEVICE_ID, now);
        for (name, point) in VIRTUAL_PCS_POINTS {
            let dp = dev.read_point(*point).expect("read");
            assert_eq!(dp.point_id, *point, "{name}");
            assert!(dp.quality.valid, "遥测点品质必须 Good");
            assert_eq!(dp.timestamp_ms, now(), "读时刷新时间戳");
        }
    }

    #[test]
    fn unknown_point_rejected() {
        let mut dev = VirtualPcsDevice::new(1, now);
        assert!(dev.read_point(999).is_err());
    }

    #[test]
    fn write_has_real_side_effect_and_independent_readback() {
        let mut dev = VirtualPcsDevice::new(VIRTUAL_PCS_DEVICE_ID, now);
        // AuthorizedCommand 私有构造——经 from_parts（#[doc(hidden)]
        // 测试构造口，生产唯一构造点是 ControlGateway::authorize）。
        let cmd = test_command(VIRTUAL_PCS_DEVICE_ID, POINT_POWER);
        dev.write_authorized(&cmd, PointValue::Float(5.0))
            .expect("write");
        assert_eq!(dev.write_count(), 1);
        // 独立读回：read_point 是独立操作，值来自寄存器（非回显参数）。
        let dp = dev.read_point(POINT_POWER).expect("readback");
        assert_eq!(dp.value, PointValue::Float(5.0));
        assert!(dev.write_count() >= 1, "写副作用可观察");
    }

    #[test]
    fn write_mismatch_and_bad_payload_zero_side_effect() {
        let mut dev = VirtualPcsDevice::new(VIRTUAL_PCS_DEVICE_ID, now);
        let wrong_point = test_command(VIRTUAL_PCS_DEVICE_ID, POINT_SOC);
        assert!(matches!(
            dev.write_authorized(&wrong_point, PointValue::Float(1.0)),
            Err(WriteError::CommandMismatch)
        ));
        let cmd = test_command(VIRTUAL_PCS_DEVICE_ID, POINT_POWER);
        assert!(matches!(
            dev.write_authorized(&cmd, PointValue::Bool(true)),
            Err(WriteError::Unauthorized)
        ));
        assert!(matches!(
            dev.write_authorized(&cmd, PointValue::Float(f64::NAN)),
            Err(WriteError::Unauthorized)
        ));
        assert_eq!(dev.write_count(), 0, "拒绝路径零副作用");
        assert_eq!(dev.power_kw(), 0.0);
    }

    fn test_command(device: DeviceId, point: PointId) -> AuthorizedCommand {
        use eneros_telemetry_model::command::{ActorKind, ControlCommand, SingleCommand};
        AuthorizedCommand::from_parts(
            device,
            point,
            ControlCommand::Single(SingleCommand::On),
            ActorKind::Ai,
            [0u8; 16],
            [9u8; 16],
            1,
            [0u8; 16],
            0,
            1,
            [0u8; 16],
        )
    }
}
