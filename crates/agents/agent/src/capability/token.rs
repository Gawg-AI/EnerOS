//! 能力令牌（Capability Token）核心数据结构 (v0.39.0).
//!
//! 实现 Agent 访问控制的能力令牌，包含：
//! - [`CapabilityToken`] — 令牌主体（9 字段 + SM2 签名）
//! - [`ResourceTarget`] — 目标资源（5 变体）
//! - [`PermissionSet`] — 权限位集（手动 bitflags，6 种权限）
//! - [`ConstraintPack`] / [`ConstraintType`] — 电力约束包
//!
//! # 偏差声明 (D1~D13)
//! - D1: `build_and_sign` 接受 `now: u64` + `rng: &mut CsRng`（no_std 无系统时钟/RNG）
//! - D2: token_id 由 `rng.fill_bytes()` 生成（no_std 无 `crate::rng::next_u64()`）
//! - D3: SM2 签名使用 `sm2_sign(data, &sk, &pk, rng)`（需公钥计算 Z 值 + RNG）
//! - D4: 验签使用 `sm2_verify(data, &sig, &pk)`（`sm2_verify_hash` 不存在）
//! - D5: `signature: [u8; 64]`（SM2 固定 64 字节 r‖s，非 `Vec<u8>`）
//! - D6: 手动 `PermissionSet(u32)` bitflags（不依赖 `bitflags` crate）
//! - D7: 自定义 `SocketAddr { ipv4: u32, port: u16 }`（no_std 无 `std::net::SocketAddr`）
//! - D8: 自定义 `DeviceId(pub u64)`（agent crate 中不存在此类型）
//! - D9: 跳过 `ConstraintPack::check(cmd)`（`ControlCommand` 尚未定义）
//! - D10: `verify` 返回 `Result<(), AgentError>`（Ok(())=有效，非 `Result<bool, _>`）
//! - D11: `Sm2Signature::from_bytes(&self.signature)` 兼容 `[u8; 64]` 字段
//! - D12: 模块路径 `crates/agents/agent/src/capability/`
//! - D13: 引入 `eneros-crypto` 依赖（agent crate 首个外部依赖）
//!
//! # no_std 合规
//! 仅使用 `alloc::*` / `core::*`，不依赖 `std::*`。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_crypto::{sm2_verify, Sm2PublicKey, Sm2Signature};

use crate::error::AgentError;
use crate::id::AgentId;

// ============================================================
// 支持类型: DeviceId / SocketAddr / SystemResource
// ============================================================

/// 设备 ID.
///
/// 用于标识物理或虚拟设备，作为能力令牌的目标资源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub u64);

/// 网络套接字地址（no_std 版本）.
///
/// 仅包含 IPv4 地址和端口号，不依赖 `std::net::SocketAddr`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SocketAddr {
    /// IPv4 地址（大端序 4 字节，如 192.168.1.1 = 0xC0A80101）
    pub ipv4: u32,
    /// 端口号
    pub port: u16,
}

/// 系统资源类型.
///
/// 标识 Agent 可访问的系统级资源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemResource {
    /// CPU
    Cpu,
    /// 内存
    Memory,
    /// 存储
    Storage,
    /// 网络
    Network,
    /// GPIO
    Gpio,
    /// 定时器
    Timer,
    /// 系统总线
    SystemBus,
}

// ============================================================
// ResourceTarget: 目标资源枚举
// ============================================================

/// 能力令牌的目标资源.
///
/// 指定令牌授权访问的具体资源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceTarget {
    /// 设备
    Device(DeviceId),
    /// Agent
    Agent(AgentId),
    /// 文件路径
    File(String),
    /// 网络地址
    Network(SocketAddr),
    /// 系统资源
    SystemResource(SystemResource),
}

impl ResourceTarget {
    /// 序列化到缓冲区（确定性，用于签名）.
    fn serialize(&self, buf: &mut Vec<u8>) {
        match self {
            ResourceTarget::Device(id) => {
                buf.push(0);
                buf.extend_from_slice(&id.0.to_be_bytes());
            }
            ResourceTarget::Agent(id) => {
                buf.push(1);
                buf.extend_from_slice(&id.0.to_be_bytes());
            }
            ResourceTarget::File(path) => {
                buf.push(2);
                let bytes = path.as_bytes();
                buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                buf.extend_from_slice(bytes);
            }
            ResourceTarget::Network(addr) => {
                buf.push(3);
                buf.extend_from_slice(&addr.ipv4.to_be_bytes());
                buf.extend_from_slice(&addr.port.to_be_bytes());
            }
            ResourceTarget::SystemResource(res) => {
                buf.push(4);
                buf.push(*res as u8);
            }
        }
    }
}

// ============================================================
// PermissionSet: 权限位集（手动 bitflags, D6）
// ============================================================

/// 权限位集.
///
/// 使用 `u32` 底层位表示，手动实现 bitflags 操作（不依赖 `bitflags` crate）。
/// 支持 6 种权限：READ/WRITE/EXECUTE/CONTROL/CONFIG/ADMIN。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PermissionSet(pub u32);

impl PermissionSet {
    /// 读权限
    pub const READ: PermissionSet = PermissionSet(0x01);
    /// 写权限
    pub const WRITE: PermissionSet = PermissionSet(0x02);
    /// 执行权限
    pub const EXECUTE: PermissionSet = PermissionSet(0x04);
    /// 控制权限
    pub const CONTROL: PermissionSet = PermissionSet(0x08);
    /// 配置权限
    pub const CONFIG: PermissionSet = PermissionSet(0x10);
    /// 管理权限
    pub const ADMIN: PermissionSet = PermissionSet(0x20);
    /// 无权限
    pub const NONE: PermissionSet = PermissionSet(0x00);
    /// 全部权限
    pub const ALL: PermissionSet = PermissionSet(0x3F);

    /// 获取底层位表示.
    pub fn bits(&self) -> u32 {
        self.0
    }

    /// 从位表示构造.
    pub fn from_bits(bits: u32) -> Self {
        PermissionSet(bits)
    }

    /// 检查是否包含指定权限.
    pub fn contains(&self, other: PermissionSet) -> bool {
        (self.0 & other.0) == other.0
    }

    /// 插入权限.
    pub fn insert(&mut self, other: PermissionSet) {
        self.0 |= other.0;
    }

    /// 是否为空权限.
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// 是否包含全部权限.
    pub fn is_all(&self) -> bool {
        self.0 == Self::ALL.0
    }
}

impl core::ops::BitOr for PermissionSet {
    type Output = PermissionSet;
    fn bitor(self, rhs: PermissionSet) -> PermissionSet {
        PermissionSet(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for PermissionSet {
    fn bitor_assign(&mut self, rhs: PermissionSet) {
        self.0 |= rhs.0;
    }
}

// ============================================================
// ConstraintType / ConstraintPack: 电力约束
// ============================================================

/// 约束类型.
///
/// 指定约束检查的维度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConstraintType {
    /// 最大功率
    MaxPower,
    /// 最小功率
    MinPower,
    /// SOC 下限
    SocMin,
    /// SOC 上限
    SocMax,
    /// 电压下限
    VoltageMin,
    /// 电压上限
    VoltageMax,
    /// 频率下限
    FreqMin,
    /// 频率上限
    FreqMax,
}

/// 电力约束包.
///
/// 包含功率、SOC、电压、频率约束，用于能力令牌的安全限制。
/// 每个约束为 `(min, max)` 元组，表示允许范围。
#[derive(Clone, Debug)]
pub struct ConstraintPack {
    /// 最大功率（kW）
    pub max_power: f32,
    /// 最小功率（kW）
    pub min_power: f32,
    /// SOC 约束 `(min, max)`（百分比 0~100）
    pub soc_limit: (f32, f32),
    /// 电压约束 `(min, max)`（V）
    pub voltage_limit: (f32, f32),
    /// 频率约束 `(min, max)`（Hz）
    pub frequency_limit: (f32, f32),
}

impl ConstraintPack {
    /// 检查给定值是否满足约束.
    ///
    /// 返回 `true` 表示值在约束范围内，`false` 表示违反约束。
    pub fn check_constraint(&self, value: f32, ctype: ConstraintType) -> bool {
        match ctype {
            ConstraintType::MaxPower => value <= self.max_power,
            ConstraintType::MinPower => value >= self.min_power,
            ConstraintType::SocMin => value >= self.soc_limit.0,
            ConstraintType::SocMax => value <= self.soc_limit.1,
            ConstraintType::VoltageMin => value >= self.voltage_limit.0,
            ConstraintType::VoltageMax => value <= self.voltage_limit.1,
            ConstraintType::FreqMin => value >= self.frequency_limit.0,
            ConstraintType::FreqMax => value <= self.frequency_limit.1,
        }
    }

    /// 将值截断到约束边界.
    ///
    /// 如果值超出约束范围，返回边界值；否则返回原值。
    pub fn clamp(&self, value: f32, ctype: ConstraintType) -> f32 {
        match ctype {
            ConstraintType::MaxPower => value.min(self.max_power),
            ConstraintType::MinPower => value.max(self.min_power),
            ConstraintType::SocMin => value.max(self.soc_limit.0),
            ConstraintType::SocMax => value.min(self.soc_limit.1),
            ConstraintType::VoltageMin => value.max(self.voltage_limit.0),
            ConstraintType::VoltageMax => value.min(self.voltage_limit.1),
            ConstraintType::FreqMin => value.max(self.frequency_limit.0),
            ConstraintType::FreqMax => value.min(self.frequency_limit.1),
        }
    }

    /// 序列化到缓冲区（确定性，用于签名）.
    fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.max_power.to_be_bytes());
        buf.extend_from_slice(&self.min_power.to_be_bytes());
        buf.extend_from_slice(&self.soc_limit.0.to_be_bytes());
        buf.extend_from_slice(&self.soc_limit.1.to_be_bytes());
        buf.extend_from_slice(&self.voltage_limit.0.to_be_bytes());
        buf.extend_from_slice(&self.voltage_limit.1.to_be_bytes());
        buf.extend_from_slice(&self.frequency_limit.0.to_be_bytes());
        buf.extend_from_slice(&self.frequency_limit.1.to_be_bytes());
    }
}

impl Default for ConstraintPack {
    /// 默认约束包：全零（拒绝所有）.
    fn default() -> Self {
        ConstraintPack {
            max_power: 0.0,
            min_power: 0.0,
            soc_limit: (0.0, 0.0),
            voltage_limit: (0.0, 0.0),
            frequency_limit: (0.0, 0.0),
        }
    }
}

// ============================================================
// CapabilityToken: 能力令牌
// ============================================================

/// 能力令牌.
///
/// Agent 访问控制的核心载体，包含权限、约束和 SM2 签名。
/// 每个令牌授权持有者（`owner`）对目标资源（`target`）执行特定操作。
///
/// # 字段
/// - `token_id`: 令牌唯一 ID（CSRNG 随机生成）
/// - `owner`: 令牌持有者
/// - `target`: 目标资源
/// - `permissions`: 权限集
/// - `constraints`: 安全约束
/// - `issued_at`: 签发时间戳
/// - `expires_at`: 过期时间戳（None = 永不过期）
/// - `issuer`: 签发者
/// - `signature`: SM2 签名（r‖s，64 字节）
#[derive(Clone, Debug)]
pub struct CapabilityToken {
    /// 令牌唯一 ID
    pub token_id: u64,
    /// 令牌持有者
    pub owner: AgentId,
    /// 目标资源
    pub target: ResourceTarget,
    /// 权限集
    pub permissions: PermissionSet,
    /// 安全约束
    pub constraints: ConstraintPack,
    /// 签发时间戳
    pub issued_at: u64,
    /// 过期时间戳（None = 永不过期）
    pub expires_at: Option<u64>,
    /// 签发者
    pub issuer: AgentId,
    /// SM2 签名（r‖s，64 字节）
    pub signature: [u8; 64],
}

impl CapabilityToken {
    /// 检查令牌是否已过期.
    ///
    /// `now >= expires_at` 视为过期；无过期时间则永不过期。
    pub fn is_expired(&self, now: u64) -> bool {
        match self.expires_at {
            Some(exp) => now >= exp,
            None => false,
        }
    }

    /// 检查令牌是否包含指定权限.
    pub fn check_permission(&self, perm: PermissionSet) -> bool {
        self.permissions.contains(perm)
    }

    /// 检查值是否满足令牌约束.
    pub fn check_constraint(&self, value: f32, ctype: ConstraintType) -> bool {
        self.constraints.check_constraint(value, ctype)
    }

    /// 序列化未签名部分（用于签名/验签）.
    ///
    /// 将除 `signature` 外的所有字段按确定性顺序序列化为字节串。
    /// 同一令牌的两次序列化结果相同。
    pub fn serialize_unsigned(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.token_id.to_be_bytes());
        buf.extend_from_slice(&self.owner.0.to_be_bytes());
        self.target.serialize(&mut buf);
        buf.extend_from_slice(&self.permissions.0.to_be_bytes());
        self.constraints.serialize(&mut buf);
        buf.extend_from_slice(&self.issued_at.to_be_bytes());
        match self.expires_at {
            Some(exp) => {
                buf.push(1);
                buf.extend_from_slice(&exp.to_be_bytes());
            }
            None => {
                buf.push(0);
            }
        }
        buf.extend_from_slice(&self.issuer.0.to_be_bytes());
        buf
    }

    /// 验证令牌签名.
    ///
    /// # 参数
    /// - `issuer_pk`: 签发者公钥
    ///
    /// # 返回
    /// - `Ok(())`: 签名有效
    /// - `Err(TokenNotSigned)`: 签名为全零（未签名）
    /// - `Err(TokenSignatureInvalid)`: 签名验证失败
    pub fn verify(&self, issuer_pk: &Sm2PublicKey) -> Result<(), AgentError> {
        // 检查签名是否为全零（未签名）
        if self.signature.iter().all(|&b| b == 0) {
            return Err(AgentError::TokenNotSigned);
        }
        // 序列化未签名部分
        let data = self.serialize_unsigned();
        // 从 64 字节恢复 Sm2Signature
        let sig = Sm2Signature::from_bytes(&self.signature);
        // 验证签名
        match sm2_verify(&data, &sig, issuer_pk) {
            Ok(true) => Ok(()),
            Ok(false) => Err(AgentError::TokenSignatureInvalid),
            Err(_) => Err(AgentError::TokenSignatureInvalid),
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造已知字段值的能力令牌（用于测试）.
    fn make_test_token() -> CapabilityToken {
        CapabilityToken {
            token_id: 12345,
            owner: AgentId(1),
            target: ResourceTarget::Agent(AgentId(2)),
            permissions: PermissionSet::READ | PermissionSet::WRITE,
            constraints: ConstraintPack {
                max_power: 100.0,
                min_power: 10.0,
                soc_limit: (20.0, 80.0),
                voltage_limit: (200.0, 240.0),
                frequency_limit: (49.5, 50.5),
            },
            issued_at: 1000,
            expires_at: Some(2000),
            issuer: AgentId(1),
            signature: [0u8; 64],
        }
    }

    // ============================================================
    // PermissionSet 测试 (6)
    // ============================================================

    #[test]
    fn test_permission_set_bits() {
        assert_eq!(PermissionSet::READ.bits(), 1);
        assert_eq!(PermissionSet::WRITE.bits(), 2);
        assert_eq!(PermissionSet::ALL.bits(), 0x3F);
    }

    #[test]
    fn test_permission_set_contains() {
        let p = PermissionSet::READ | PermissionSet::WRITE;
        assert!(p.contains(PermissionSet::READ));
        assert!(p.contains(PermissionSet::WRITE));
        assert!(!p.contains(PermissionSet::EXECUTE));
    }

    #[test]
    fn test_permission_set_insert() {
        let mut p = PermissionSet::NONE;
        p.insert(PermissionSet::READ);
        assert!(p.contains(PermissionSet::READ));
        p.insert(PermissionSet::WRITE);
        assert!(p.contains(PermissionSet::WRITE));
        assert_eq!(p.bits(), 3);
    }

    #[test]
    fn test_permission_set_bitor() {
        let p = PermissionSet::READ | PermissionSet::WRITE | PermissionSet::EXECUTE;
        assert_eq!(p, PermissionSet(0x07));
    }

    #[test]
    fn test_permission_set_is_empty() {
        assert!(PermissionSet::NONE.is_empty());
        assert!(!PermissionSet::READ.is_empty());
    }

    #[test]
    fn test_permission_set_is_all() {
        assert!(PermissionSet::ALL.is_all());
        assert!(!(PermissionSet::READ | PermissionSet::WRITE).is_all());
        assert!(PermissionSet::NONE.is_empty());
    }

    // ============================================================
    // ConstraintPack 测试 (4)
    // ============================================================

    #[test]
    fn test_constraint_pack_check_max_power() {
        let pack = ConstraintPack {
            max_power: 100.0,
            ..Default::default()
        };
        assert!(pack.check_constraint(50.0, ConstraintType::MaxPower));
        assert!(!pack.check_constraint(150.0, ConstraintType::MaxPower));
    }

    #[test]
    fn test_constraint_pack_check_min_power() {
        let pack = ConstraintPack {
            min_power: 10.0,
            ..Default::default()
        };
        assert!(!pack.check_constraint(5.0, ConstraintType::MinPower));
        assert!(pack.check_constraint(20.0, ConstraintType::MinPower));
    }

    #[test]
    fn test_constraint_pack_check_soc_voltage_freq() {
        let pack = ConstraintPack {
            soc_limit: (20.0, 80.0),
            voltage_limit: (200.0, 240.0),
            frequency_limit: (49.5, 50.5),
            ..Default::default()
        };
        // SocMin: value >= 20.0
        assert!(pack.check_constraint(30.0, ConstraintType::SocMin));
        assert!(!pack.check_constraint(10.0, ConstraintType::SocMin));
        // SocMax: value <= 80.0
        assert!(pack.check_constraint(70.0, ConstraintType::SocMax));
        assert!(!pack.check_constraint(90.0, ConstraintType::SocMax));
        // VoltageMin: value >= 200.0
        assert!(pack.check_constraint(210.0, ConstraintType::VoltageMin));
        assert!(!pack.check_constraint(190.0, ConstraintType::VoltageMin));
        // VoltageMax: value <= 240.0
        assert!(pack.check_constraint(230.0, ConstraintType::VoltageMax));
        assert!(!pack.check_constraint(250.0, ConstraintType::VoltageMax));
        // FreqMin: value >= 49.5
        assert!(pack.check_constraint(50.0, ConstraintType::FreqMin));
        assert!(!pack.check_constraint(49.0, ConstraintType::FreqMin));
        // FreqMax: value <= 50.5
        assert!(pack.check_constraint(50.0, ConstraintType::FreqMax));
        assert!(!pack.check_constraint(51.0, ConstraintType::FreqMax));
    }

    #[test]
    fn test_constraint_pack_clamp() {
        let pack = ConstraintPack {
            max_power: 100.0,
            ..Default::default()
        };
        assert_eq!(pack.clamp(150.0, ConstraintType::MaxPower), 100.0);
        assert_eq!(pack.clamp(50.0, ConstraintType::MaxPower), 50.0);
    }

    // ============================================================
    // CapabilityToken 测试 (5)
    // ============================================================

    #[test]
    fn test_token_is_expired() {
        let mut token = make_test_token();
        token.expires_at = Some(1000);
        assert!(!token.is_expired(999));
        assert!(token.is_expired(1000));
        assert!(token.is_expired(1001));
        // 无过期时间则永不过期
        token.expires_at = None;
        assert!(!token.is_expired(u64::MAX));
    }

    #[test]
    fn test_token_check_permission() {
        let token = make_test_token(); // permissions = READ | WRITE
        assert!(token.check_permission(PermissionSet::READ));
        assert!(token.check_permission(PermissionSet::WRITE));
        assert!(!token.check_permission(PermissionSet::EXECUTE));
    }

    #[test]
    fn test_token_check_constraint() {
        let token = make_test_token(); // constraints.max_power = 100.0
        assert!(token.check_constraint(50.0, ConstraintType::MaxPower));
        assert!(!token.check_constraint(150.0, ConstraintType::MaxPower));
    }

    #[test]
    fn test_serialize_unsigned_deterministic() {
        let t1 = make_test_token();
        let t2 = make_test_token();
        assert_eq!(t1.serialize_unsigned(), t2.serialize_unsigned());
    }

    #[test]
    fn test_serialize_unsigned_different_tokens() {
        let t1 = make_test_token();
        let mut t2 = make_test_token();
        t2.token_id = 99999;
        assert_ne!(t1.serialize_unsigned(), t2.serialize_unsigned());
    }
}
