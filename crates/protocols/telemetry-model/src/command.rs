//! 控制命令与执行状态（Command / ControlExecState）.
//!
//! 定义遥控命令的统一起义与执行状态机，对应 IEC 104 SingleCommand/DoubleCommand
//! 与 Modbus Function Code 5/6/15/16 的归一化语义。

/// 单位置命令（SingleCommand）。
///
/// 对应 IEC 104 SinglePoint 信息体（ON/OFF）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleCommand {
    /// 分（OFF）。
    Off,
    /// 合（ON）。
    On,
}

/// 双位置命令（DoubleCommand）。
///
/// 对应 IEC 104 DoublePoint 信息体（OFF/ON/Intermediate/Bad）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleCommand {
    /// 分（OFF）。
    Off,
    /// 合（ON）。
    On,
    /// 中间态。
    Intermediate,
    /// 错误。
    Bad,
}

/// 控制命令（统一遥控命令）。
///
/// 包装 `SingleCommand`（单位置）或 `DoubleCommand`（双位置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    /// 单位置命令。
    Single(SingleCommand),
    /// 双位置命令。
    Double(DoubleCommand),
}

/// 控制执行状态机（6 状态）。
///
/// 状态流转：
/// - 非 SBO：`Idle` → `Executing` → `Done`/`Failed`/`Timeout`
/// - SBO：`Idle` → `Selected` → `Executing` → `Done`/`Failed`/`Timeout`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlExecState {
    /// 空闲。
    Idle,
    /// 已选择（SBO 步骤 1）。
    Selected,
    /// 执行中。
    Executing,
    /// 执行完成（终态）。
    Done,
    /// 执行失败（终态）。
    Failed,
    /// 超时（终态）。
    Timeout,
}

impl ControlExecState {
    /// 返回是否为终态（`Done`/`Failed`/`Timeout`）。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ControlExecState::Done | ControlExecState::Failed | ControlExecState::Timeout
        )
    }

    /// 返回是否为活跃态（`Selected`/`Executing`）。
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            ControlExecState::Selected | ControlExecState::Executing
        )
    }
}

// ============================================================================
// 控制写接缝类型（蓝图/24 D1/D4：随 ControlCommand 下移共享模型层，
// 使 protocol-abstract 的 AuthorizedWriter 可引用而不破坏依赖方向）
// ============================================================================

/// 命令签发者身份类别。驱动策略（CTL-005）与审计。
///
/// 原定义于 control-gateway::envelope；蓝图/24 D4 类型化接缝时随
/// [`AuthorizedCommand`] 一并下移至共享模型层（control-gateway re-export）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ActorKind {
    /// 就地操作员。
    Operator = 0,
    /// 代理（Agent）。
    Agent = 1,
    /// AI 双脑。
    Ai = 2,
    /// 系统（计划任务等）。
    System = 3,
    /// 急停（Emergency）。
    Emergency = 4,
}

impl ActorKind {
    /// 从信封 wire 字节解码（非法编码返回 None，由解码器 default-deny）。
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(ActorKind::Operator),
            1 => Some(ActorKind::Agent),
            2 => Some(ActorKind::Ai),
            3 => Some(ActorKind::System),
            4 => Some(ActorKind::Emergency),
            _ => None,
        }
    }
}

/// 类型化命令值（蓝图 61 v0.314.25 §3.3 canonical typed payload）。
///
/// 变体即单位：`ActivePowerKw` 恒 kW、`VoltageSetpointV` 恒 V……
/// 跨 binary wire 用数字 tag + 固定 value bits（f64/u16 LE），不传
/// Rust enum 布局；`value_tag`/`value_bits` 在信封 V3 头部由 HMAC 覆盖。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommandValue {
    /// 开关位置（0=分/off，1=合/on）。
    SwitchPosition(u8),
    /// 有功功率设定（kW；连续 setpoint——PCS 点 10 的正确语义）。
    ActivePowerKw(f64),
    /// 无功功率设定（kvar）。
    ReactivePowerKvar(f64),
    /// 电压设定（V）。
    VoltageSetpointV(f64),
    /// 电流限值（A）。
    CurrentLimitA(f64),
    /// 模式码（u16；闭集由点表/合同定义）。
    ModeCode(u16),
}

impl CommandValue {
    /// wire tag（封闭数字码；未知 tag 解码 fail-closed）。
    pub fn value_tag(self) -> u8 {
        match self {
            CommandValue::SwitchPosition(_) => 1,
            CommandValue::ActivePowerKw(_) => 2,
            CommandValue::ReactivePowerKvar(_) => 3,
            CommandValue::VoltageSetpointV(_) => 4,
            CommandValue::CurrentLimitA(_) => 5,
            CommandValue::ModeCode(_) => 6,
        }
    }

    /// 8B value bits（LE；离散量在 bits[0..2]，其余清零）。
    pub fn value_bits(self) -> [u8; 8] {
        let mut b = [0u8; 8];
        match self {
            CommandValue::SwitchPosition(v) => b[0] = v,
            CommandValue::ModeCode(v) => b[0..2].copy_from_slice(&v.to_le_bytes()),
            CommandValue::ActivePowerKw(v) => b.copy_from_slice(&v.to_le_bytes()),
            CommandValue::ReactivePowerKvar(v) => b.copy_from_slice(&v.to_le_bytes()),
            CommandValue::VoltageSetpointV(v) => b.copy_from_slice(&v.to_le_bytes()),
            CommandValue::CurrentLimitA(v) => b.copy_from_slice(&v.to_le_bytes()),
        }
        b
    }

    /// tag + bits → 值（未知 tag / 非有限浮点 → None，default-deny）。
    pub fn from_tag_bits(tag: u8, bits: [u8; 8]) -> Option<Self> {
        let f = |b: [u8; 8]| -> Option<f64> {
            let v = f64::from_le_bytes(b);
            if v.is_finite() {
                Some(v)
            } else {
                None
            }
        };
        match tag {
            1 => {
                let v = bits[0];
                if v <= 1 {
                    Some(CommandValue::SwitchPosition(v))
                } else {
                    None
                }
            }
            2 => Some(CommandValue::ActivePowerKw(f(bits)?)),
            3 => Some(CommandValue::ReactivePowerKvar(f(bits)?)),
            4 => Some(CommandValue::VoltageSetpointV(f(bits)?)),
            5 => Some(CommandValue::CurrentLimitA(f(bits)?)),
            6 => Some(CommandValue::ModeCode(u16::from_le_bytes([
                bits[0], bits[1],
            ]))),
            _ => None,
        }
    }

    /// 单位代码（UnitCatalog 对应码；0=未知保留）。
    pub fn unit_code(self) -> u16 {
        match self {
            // SwitchPosition/ModeCode 无量纲。
            CommandValue::SwitchPosition(_) | CommandValue::ModeCode(_) => 0,
            CommandValue::ActivePowerKw(_) => 1,     // kW
            CommandValue::ReactivePowerKvar(_) => 2, // kvar
            CommandValue::VoltageSetpointV(_) => 3,  // V
            CommandValue::CurrentLimitA(_) => 4,     // A
        }
    }

    /// 转换为协议点值形态（writer 编码设备载荷的唯一取值途径；
    /// 61 v0.314.25：签名上移除独立 value 参数后由各适配器调用）。
    pub fn to_point_value(self) -> eneros_upa_model::PointValue {
        use eneros_upa_model::PointValue;
        match self {
            CommandValue::SwitchPosition(v) => PointValue::Int(i64::from(v)),
            CommandValue::ActivePowerKw(v) => PointValue::Float(v),
            CommandValue::ReactivePowerKvar(v) => PointValue::Float(v),
            CommandValue::VoltageSetpointV(v) => PointValue::Float(v),
            CommandValue::CurrentLimitA(v) => PointValue::Float(v),
            CommandValue::ModeCode(v) => PointValue::Int(i64::from(v)),
        }
    }

    /// 值对应的 legacy ControlCommand 视图（SwitchPosition → Single On/Off；
    /// 其余无 legacy 形态 → None——禁止把连续值降级解释为开关）。
    pub fn legacy_command(self) -> Option<ControlCommand> {
        match self {
            CommandValue::SwitchPosition(0) => Some(ControlCommand::Single(SingleCommand::Off)),
            CommandValue::SwitchPosition(1) => Some(ControlCommand::Single(SingleCommand::On)),
            _ => None,
        }
    }
}

/// 已通过控制链全部裁决、可施加于设备的写命令（蓝图/24 D4）。
///
/// **26-B WP-26.4（P1-1）封焊**：字段私有——生产代码中"字面量构造
/// `AuthorizedCommand { .. }`"编译失败（P1-1：旧全 pub 字段使任何依赖
/// telemetry-model 的 crate 可伪造已授权命令，绕过 HMAC/租约/防重放）。
/// 构造通道：
/// - 生产：`ControlGateway::authorize` 经 [`AuthorizedCommand::from_parts`]
///   （`#[doc(hidden)]`；WP-26.1 门禁执法封死其他生产调用）；
/// - 测试：`from_parts` 或契约套件 `protocol-abstract::contract::authorized_command`。
///
/// `device_id` 为 upa-model `DeviceId(u16)`、`point_id` 为 `PointId(u32)`；
/// 信封 wire 布局不变（wire 上 device_id 仍占 4 字节，见 control-gateway
/// envelope 编解码；u32→u16 的收窄在 HMAC 验证之后、default-deny）。
// Eq 不可用（CommandValue 含 f64；浮点相等语义由值位精确比较承担）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuthorizedCommand {
    /// 目标设备 ID（upa-model `DeviceId`，u16）。
    device_id: eneros_upa_model::DeviceId,
    /// 目标点 ID（upa-model `PointId`，u32）。
    point_id: eneros_upa_model::PointId,
    /// 签发者身份类别。
    actor_kind: ActorKind,
    /// 签发者标识（固定 16 字节）。
    issuer: [u8; 16],
    /// 关联 ID（请求→结果链追踪）。
    correlation_id: [u8; 16],
    /// 序列号（签发方单调）。
    sequence: u64,
    /// 本命令所骑乘的租约 ID（CTL-006）。
    lease_id: [u8; 16],
    /// fencing token（单调；吊销围栏，CTL-006）。
    fencing_token: u64,
    /// 策略代次（PDP/PEP，CTL-005）。
    policy_version: u64,
    /// 幂等键（CTL-007 去重）。
    idempotency_key: [u8; 16],
    /// 类型化命令值（61 v0.314.25：与 writer 执行完全同型的唯一数据源）。
    value: CommandValue,
    /// 命令摘要（61 v0.314.25）：SHA-256("eneros.authorized-command.v2" ‖
    /// 信封 V3 头部无 HMAC 字节)——writer 侧可从收到的信封字节重算验证。
    command_digest: [u8; 32],
}

impl AuthorizedCommand {
    /// 只读访问器（WP-26.4：字段私有后全字段经访问器读取）。
    pub fn device_id(&self) -> eneros_upa_model::DeviceId {
        self.device_id
    }
    pub fn point_id(&self) -> eneros_upa_model::PointId {
        self.point_id
    }
    /// legacy 开关语义视图（61 v0.314.25）：仅 SwitchPosition 有 legacy
    /// 形态；连续/模拟值返回 None（消费方必须显式处理，禁止把浮点
    /// 解释成开关）。
    pub fn command(&self) -> Option<ControlCommand> {
        self.value.legacy_command()
    }
    pub fn actor_kind(&self) -> ActorKind {
        self.actor_kind
    }
    pub fn issuer(&self) -> [u8; 16] {
        self.issuer
    }
    pub fn correlation_id(&self) -> [u8; 16] {
        self.correlation_id
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn lease_id(&self) -> [u8; 16] {
        self.lease_id
    }
    pub fn fencing_token(&self) -> u64 {
        self.fencing_token
    }
    pub fn policy_version(&self) -> u64 {
        self.policy_version
    }
    pub fn idempotency_key(&self) -> [u8; 16] {
        self.idempotency_key
    }
    /// 类型化命令值（唯一执行数据源；61 v0.314.25）。
    pub fn value(&self) -> CommandValue {
        self.value
    }
    /// 命令摘要（writer 侧重算比对依据；61 v0.314.25）。
    pub fn command_digest(&self) -> [u8; 32] {
        self.command_digest
    }

    /// 构造通道（`#[doc(hidden)]`，WP-26.4）。
    ///
    /// **生产唯一合法调用方**：`ControlGateway::authorize`（control-gateway
    /// gateway.rs）。WP-26.1 门禁执法：其他生产 crate 调用本函数 → 门禁
    /// FAIL（与字面量构造同样封死）。测试装配可用（测试不受门禁执法约束）。
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        device_id: eneros_upa_model::DeviceId,
        point_id: eneros_upa_model::PointId,
        actor_kind: ActorKind,
        issuer: [u8; 16],
        correlation_id: [u8; 16],
        sequence: u64,
        lease_id: [u8; 16],
        fencing_token: u64,
        policy_version: u64,
        idempotency_key: [u8; 16],
        value: CommandValue,
        command_digest: [u8; 32],
    ) -> Self {
        Self {
            device_id,
            point_id,
            actor_kind,
            issuer,
            correlation_id,
            sequence,
            lease_id,
            fencing_token,
            policy_version,
            idempotency_key,
            value,
            command_digest,
        }
    }
}
