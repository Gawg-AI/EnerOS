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

/// 已通过控制链全部裁决、可施加于设备的写命令（蓝图/24 D4）。
///
/// **26-B WP-26.4（P1-1）封焊**：字段私有——生产代码中"字面量构造
/// `AuthorizedCommand { .. }`"编译失败（P1-1：旧全 pub 字段使任何依赖
/// telemetry-model 的 crate 可伪造已授权命令，绕过 HMAC/租约/防重放）。
/// 构造通道：
/// - 生产：`ControlGateway::authorize` 经 [`AuthorizedCommand::from_parts`]
///   （`#[doc(hidden)]`；WP-26.1 门禁执法封死其他生产调用）；
/// - 测试：`from_parts` 或契约套件 `protocol-abstract::contract::authorized_command`。
/// `device_id` 为 upa-model `DeviceId(u16)`、`point_id` 为 `PointId(u32)`；
/// 信封 wire 布局不变（wire 上 device_id 仍占 4 字节，见 control-gateway
/// envelope 编解码；u32→u16 的收窄在 HMAC 验证之后、default-deny）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizedCommand {
    /// 目标设备 ID（upa-model `DeviceId`，u16）。
    device_id: eneros_upa_model::DeviceId,
    /// 目标点 ID（upa-model `PointId`，u32）。
    point_id: eneros_upa_model::PointId,
    /// 开关语义命令（Single/Double）。
    command: ControlCommand,
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
}

impl AuthorizedCommand {
    /// 只读访问器（WP-26.4：字段私有后全字段经访问器读取）。
    pub fn device_id(&self) -> eneros_upa_model::DeviceId {
        self.device_id
    }
    pub fn point_id(&self) -> eneros_upa_model::PointId {
        self.point_id
    }
    pub fn command(&self) -> ControlCommand {
        self.command
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
        command: ControlCommand,
        actor_kind: ActorKind,
        issuer: [u8; 16],
        correlation_id: [u8; 16],
        sequence: u64,
        lease_id: [u8; 16],
        fencing_token: u64,
        policy_version: u64,
        idempotency_key: [u8; 16],
    ) -> Self {
        Self {
            device_id,
            point_id,
            command,
            actor_kind,
            issuer,
            correlation_id,
            sequence,
            lease_id,
            fencing_token,
            policy_version,
            idempotency_key,
        }
    }
}
