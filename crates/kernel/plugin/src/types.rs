//! 插件市场核心类型定义 (v0.166.0).
//!
//! 定义插件包、清单、权限、状态等核心数据结构。

use alloc::string::String;
use alloc::vec::Vec;

/// 插件唯一标识符.
pub type PluginId = u32;

/// 插件类型枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginType {
    /// 协议插件（Modbus/IEC104/61850 等）.
    Protocol,
    /// 驱动插件（硬件设备驱动）.
    Driver,
    /// Agent 插件（智能体扩展）.
    Agent,
    /// 算法插件（AI 推理/求解器扩展）.
    Algorithm,
    /// 通用插件.
    Generic,
}

/// 插件权限枚举.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// 网络访问权限.
    Network,
    /// 文件系统读取权限.
    FileRead,
    /// 文件系统写入权限.
    FileWrite,
    /// 设备访问权限.
    DeviceAccess,
    /// IPC 通信权限.
    Ipc,
    /// 定时器权限.
    Timer,
    /// 内存映射权限.
    MemoryMap,
}

/// 插件状态.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    /// 已加载（未启用）.
    Loaded,
    /// 已启用（运行中）.
    Enabled,
    /// 已禁用.
    Disabled,
    /// 已崩溃.
    Crashed,
}

/// 插件清单（Manifest）— 描述插件元数据.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    /// 插件名称.
    pub name: String,
    /// 插件版本（语义化版本号）.
    pub version: String,
    /// 作者.
    pub author: String,
    /// 插件类型.
    pub plugin_type: PluginType,
    /// 所需权限列表.
    pub permissions: Vec<Permission>,
    /// 最低 SDK 版本要求.
    pub min_sdk_version: String,
}

/// 签名者证书（简化版，包含 SM2 公钥）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    /// 证书持有者名称.
    pub subject: String,
    /// SM2 公钥（未压缩格式，65 字节：0x04 ‖ x ‖ y）.
    pub public_key: Vec<u8>,
    /// 证书有效期起始（Unix 时间戳）.
    pub not_before: u64,
    /// 证书有效期截止（Unix 时间戳）.
    pub not_after: u64,
}

/// 插件包 — 安装单元的完整数据.
#[derive(Debug, Clone)]
pub struct PluginPackage {
    /// 插件清单.
    pub manifest: PluginManifest,
    /// 插件二进制（ELF 格式）.
    pub binary: Vec<u8>,
    /// SM2 签名（64 字节：r ‖ s）.
    pub signature: Vec<u8>,
    /// 签名者证书.
    pub signer_cert: Certificate,
}

/// 已安装插件信息.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    /// 插件 ID.
    pub id: PluginId,
    /// 插件清单.
    pub manifest: PluginManifest,
    /// 当前状态.
    pub state: PluginState,
}

/// 当前 SDK 版本号.
pub const SDK_VERSION: &str = "0.166.0";

/// 插件包魔数（"EPP\0" = EnerOS Plugin Package）.
pub const PLUGIN_MAGIC: [u8; 4] = [0x45, 0x50, 0x50, 0x00];

/// 插件包格式版本.
pub const PLUGIN_FORMAT_VERSION: u8 = 1;

impl PluginManifest {
    /// 创建新的插件清单.
    pub fn new(
        name: String,
        version: String,
        author: String,
        plugin_type: PluginType,
        permissions: Vec<Permission>,
        min_sdk_version: String,
    ) -> Self {
        Self {
            name,
            version,
            author,
            plugin_type,
            permissions,
            min_sdk_version,
        }
    }
}

impl PluginInfo {
    /// 从已安装插件创建信息快照.
    pub fn new(id: PluginId, manifest: PluginManifest, state: PluginState) -> Self {
        Self {
            id,
            manifest,
            state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_type_eq() {
        assert_eq!(PluginType::Protocol, PluginType::Protocol);
        assert_ne!(PluginType::Driver, PluginType::Agent);
    }

    #[test]
    fn test_plugin_state_transitions() {
        let state = PluginState::Loaded;
        assert_eq!(state, PluginState::Loaded);
        let state = PluginState::Enabled;
        assert_eq!(state, PluginState::Enabled);
        let state = PluginState::Disabled;
        assert_eq!(state, PluginState::Disabled);
    }

    #[test]
    fn test_manifest_creation() {
        let manifest = PluginManifest::new(
            String::from("test-plugin"),
            String::from("1.0.0"),
            String::from("EnerOS Team"),
            PluginType::Protocol,
            alloc::vec![Permission::Network, Permission::Ipc],
            String::from("0.165.0"),
        );
        assert_eq!(manifest.name, "test-plugin");
        assert_eq!(manifest.permissions.len(), 2);
        assert_eq!(manifest.plugin_type, PluginType::Protocol);
    }

    #[test]
    fn test_plugin_info() {
        let manifest = PluginManifest::new(
            String::from("info-test"),
            String::from("0.1.0"),
            String::from("dev"),
            PluginType::Generic,
            alloc::vec![],
            String::from("0.166.0"),
        );
        let info = PluginInfo::new(42, manifest, PluginState::Loaded);
        assert_eq!(info.id, 42);
        assert_eq!(info.state, PluginState::Loaded);
    }
}
