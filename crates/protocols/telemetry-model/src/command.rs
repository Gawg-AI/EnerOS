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
/// 只能由 `ControlGateway::authorize` 产出（控制网关是唯一构造路径）。
/// `device_id` 为 upa-model `DeviceId(u16)`、`point_id` 为 `PointId(u32)`；
/// 信封 wire 布局不变（wire 上 device_id 仍占 4 字节，见 control-gateway
/// envelope 编解码；u32→u16 的收窄在 HMAC 验证之后、default-deny）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizedCommand {
    /// 目标设备 ID（upa-model `DeviceId`，u16）。
    pub device_id: eneros_upa_model::DeviceId,
    /// 目标点 ID（upa-model `PointId`，u32）。
    pub point_id: eneros_upa_model::PointId,
    /// 开关语义命令（Single/Double）。
    pub command: ControlCommand,
    /// 签发者身份类别。
    pub actor_kind: ActorKind,
    /// 签发者标识（固定 16 字节）。
    pub issuer: [u8; 16],
    /// 关联 ID（请求→结果链追踪）。
    pub correlation_id: [u8; 16],
    /// 序列号（签发方单调）。
    pub sequence: u64,
    /// 本命令所骑乘的租约 ID（CTL-006）。
    pub lease_id: [u8; 16],
    /// fencing token（单调；吊销围栏，CTL-006）。
    pub fencing_token: u64,
    /// 策略代次（PDP/PEP，CTL-005）。
    pub policy_version: u64,
    /// 幂等键（CTL-007 去重）。
    pub idempotency_key: [u8; 16],
}
