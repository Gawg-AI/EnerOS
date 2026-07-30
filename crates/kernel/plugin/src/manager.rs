//! 插件管理器 (v0.166.0).
//!
//! 插件生命周期的统一入口：安装 → 启用 → 禁用 → 卸载。
//! 集成签名验证、权限检查、沙箱创建、ELF 加载四重保护。
//!
//! # 安装流程（蓝图 §4.2）
//! 1. 解析 Manifest
//! 2. 验证签名（SM2/SM3）
//! 3. 检查 SDK 版本兼容
//! 4. 检查权限合理性
//! 5. 创建沙箱
//! 6. ELF 加载
//! 7. 注册到插件表

use alloc::vec::Vec;

use crate::loader::{LoadedPlugin, PluginLoader};
use crate::sandbox::PluginSandbox;
use crate::signature::{is_sdk_compatible, SignatureVerifier};
use crate::types::{PluginId, PluginInfo, PluginManifest, PluginPackage, PluginState};
use crate::PluginError;

/// 最大已安装插件数.
pub const MAX_PLUGINS: usize = 64;

/// 已安装插件内部记录.
#[derive(Debug)]
struct InstalledPlugin {
    /// 插件 ID.
    id: PluginId,
    /// 插件清单.
    manifest: PluginManifest,
    /// 当前状态.
    state: PluginState,
    /// 沙箱实例.
    sandbox: PluginSandbox,
    /// 加载信息.
    loaded: LoadedPlugin,
}

/// 插件管理器 — 管理插件完整生命周期.
///
/// 提供安装/卸载/启用/禁用/查询接口。
/// 所有操作均经过签名验证 + 权限检查 + 沙箱隔离。
pub struct PluginManagerImpl {
    /// 已安装插件列表.
    plugins: Vec<InstalledPlugin>,
    /// 下一个可用 ID.
    next_id: PluginId,
    /// 签名验证器.
    verifier: SignatureVerifier,
    /// ELF 加载器.
    loader: PluginLoader,
    /// 统计信息.
    stats: PluginStats,
}

/// 插件管理器统计（§9.6 可观测）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginStats {
    /// 累计安装次数.
    pub total_installed: u32,
    /// 累计卸载次数.
    pub total_uninstalled: u32,
    /// 累计拒绝次数（签名/权限/加载失败）.
    pub total_rejected: u32,
    /// 当前活跃插件数.
    pub active_count: u32,
}

/// 插件管理器 trait（蓝图 §3 接口类）.
pub trait PluginManager {
    /// 安装插件包.
    fn install(&mut self, package: &PluginPackage) -> Result<PluginId, PluginError>;
    /// 卸载插件.
    fn uninstall(&mut self, id: PluginId) -> Result<(), PluginError>;
    /// 启用插件.
    fn enable(&mut self, id: PluginId) -> Result<(), PluginError>;
    /// 禁用插件.
    fn disable(&mut self, id: PluginId) -> Result<(), PluginError>;
    /// 列出所有已安装插件.
    fn list(&self) -> Vec<PluginInfo>;
}

impl PluginManagerImpl {
    /// 创建插件管理器.
    ///
    /// - `verifier`: 签名验证器（含受信任根公钥）
    /// - `sandbox_base`: 沙箱内存池基地址
    /// - `sandbox_size`: 沙箱内存池大小
    pub fn new(verifier: SignatureVerifier, sandbox_base: usize, sandbox_size: usize) -> Self {
        Self {
            plugins: Vec::new(),
            next_id: 1,
            verifier,
            loader: PluginLoader::new(sandbox_base, sandbox_size),
            stats: PluginStats {
                total_installed: 0,
                total_uninstalled: 0,
                total_rejected: 0,
                active_count: 0,
            },
        }
    }

    /// 获取统计信息.
    pub fn stats(&self) -> PluginStats {
        self.stats
    }

    /// 获取指定插件的状态.
    pub fn get_state(&self, id: PluginId) -> Option<PluginState> {
        self.plugins.iter().find(|p| p.id == id).map(|p| p.state)
    }

    /// 按名称查找插件.
    pub fn find_by_name(&self, name: &str) -> Option<PluginInfo> {
        self.plugins
            .iter()
            .find(|p| p.manifest.name == name)
            .map(|p| PluginInfo::new(p.id, p.manifest.clone(), p.state))
    }

    /// 检查权限是否被系统策略允许.
    fn is_permission_allowed(&self, manifest: &PluginManifest) -> Result<(), PluginError> {
        // 当前策略：所有声明权限均允许（后续版本可接入策略引擎）
        // 但权限数量不能超过上限（防恶意插件声明过多权限）
        const MAX_PERMISSIONS: usize = 16;
        if manifest.permissions.len() > MAX_PERMISSIONS {
            return Err(PluginError::PermissionDenied);
        }
        Ok(())
    }

    /// 查找插件（内部可变引用）.
    fn find_plugin_mut(&mut self, id: PluginId) -> Result<&mut InstalledPlugin, PluginError> {
        self.plugins
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or(PluginError::NotFound)
    }
}

impl PluginManager for PluginManagerImpl {
    fn install(&mut self, package: &PluginPackage) -> Result<PluginId, PluginError> {
        // 容量检查
        if self.plugins.len() >= MAX_PLUGINS {
            self.stats.total_rejected += 1;
            return Err(PluginError::MaxPluginsReached);
        }

        // 1. 验证签名
        self.verifier.verify_package(package).map_err(|e| {
            self.stats.total_rejected += 1;
            e
        })?;

        // 2. 检查 SDK 版本兼容
        if !is_sdk_compatible(&package.manifest.min_sdk_version) {
            self.stats.total_rejected += 1;
            return Err(PluginError::IncompatibleSdk);
        }

        // 3. 检查权限
        self.is_permission_allowed(&package.manifest).map_err(|e| {
            self.stats.total_rejected += 1;
            e
        })?;

        // 4. 创建沙箱
        let sandbox = PluginSandbox::new(&package.manifest.permissions).map_err(|e| {
            self.stats.total_rejected += 1;
            e
        })?;

        // 5. ELF 加载
        let loaded = self.loader.load(&package.binary).map_err(|e| {
            self.stats.total_rejected += 1;
            e
        })?;

        // 6. 注册到插件表
        let id = self.next_id;
        self.next_id += 1;

        let plugin = InstalledPlugin {
            id,
            manifest: package.manifest.clone(),
            state: PluginState::Loaded,
            sandbox,
            loaded,
        };
        self.plugins.push(plugin);

        self.stats.total_installed += 1;

        Ok(id)
    }

    fn uninstall(&mut self, id: PluginId) -> Result<(), PluginError> {
        let idx = self
            .plugins
            .iter()
            .position(|p| p.id == id)
            .ok_or(PluginError::NotFound)?;

        // 停用沙箱
        self.plugins[idx].sandbox.deactivate();

        // 卸载 ELF
        let loaded = self.plugins[idx].loaded.clone();
        self.loader.unload(&loaded);

        // 移除记录
        self.plugins.remove(idx);
        self.stats.total_uninstalled += 1;
        if self.stats.active_count > 0 {
            self.stats.active_count -= 1;
        }

        Ok(())
    }

    fn enable(&mut self, id: PluginId) -> Result<(), PluginError> {
        let plugin = self.find_plugin_mut(id)?;
        match plugin.state {
            PluginState::Loaded | PluginState::Disabled => {
                plugin.state = PluginState::Enabled;
                plugin.sandbox.active = true;
                self.stats.active_count += 1;
                Ok(())
            }
            PluginState::Enabled => Ok(()), // 已启用，幂等
            PluginState::Crashed => Err(PluginError::PluginCrashed),
        }
    }

    fn disable(&mut self, id: PluginId) -> Result<(), PluginError> {
        let plugin = self.find_plugin_mut(id)?;
        match plugin.state {
            PluginState::Enabled => {
                plugin.state = PluginState::Disabled;
                plugin.sandbox.active = false;
                if self.stats.active_count > 0 {
                    self.stats.active_count -= 1;
                }
                Ok(())
            }
            PluginState::Loaded | PluginState::Disabled => Ok(()), // 幂等
            PluginState::Crashed => Err(PluginError::PluginCrashed),
        }
    }

    fn list(&self) -> Vec<PluginInfo> {
        self.plugins
            .iter()
            .map(|p| PluginInfo::new(p.id, p.manifest.clone(), p.state))
            .collect()
    }
}

/// 插件崩溃处理（由外部监控调用）.
impl PluginManagerImpl {
    /// 标记插件为崩溃状态.
    ///
    /// 插件崩溃不影响宿主系统（蓝图 §9.4），仅隔离该插件。
    pub fn mark_crashed(&mut self, id: PluginId) -> Result<(), PluginError> {
        let idx = self
            .plugins
            .iter()
            .position(|p| p.id == id)
            .ok_or(PluginError::NotFound)?;

        if self.plugins[idx].state == PluginState::Enabled && self.stats.active_count > 0 {
            self.stats.active_count -= 1;
        }
        self.plugins[idx].state = PluginState::Crashed;
        self.plugins[idx].sandbox.deactivate();
        Ok(())
    }

    /// 尝试重启崩溃的插件.
    pub fn restart(&mut self, id: PluginId) -> Result<(), PluginError> {
        let plugin = self.find_plugin_mut(id)?;
        if plugin.state != PluginState::Crashed {
            return Err(PluginError::InvalidStateTransition);
        }
        plugin.state = PluginState::Loaded;
        plugin.sandbox.active = true;
        plugin.sandbox.reset_quotas();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use eneros_crypto::rng::CsRng;
    use eneros_crypto::sm2::{sm2_sign, Sm2KeyPair};

    use super::*;
    use crate::types::{Certificate, Permission, PluginType};

    fn make_signed_package(
        keypair: &Sm2KeyPair,
        name: &str,
        perms: Vec<Permission>,
    ) -> PluginPackage {
        // 构建最小有效 ELF
        let binary = crate::loader::make_test_elf(0x0, &[(0x0, 64, 4096)]);

        let mut rng = CsRng::new();
        let sig = sm2_sign(&binary, &keypair.private_key, &keypair.public_key, &mut rng).unwrap();

        let cert = Certificate {
            subject: String::from("Test CA"),
            public_key: keypair.public_key.point.to_bytes_uncompressed().to_vec(),
            not_before: 0,
            not_after: u64::MAX,
        };

        PluginPackage {
            manifest: PluginManifest::new(
                String::from(name),
                String::from("1.0.0"),
                String::from("tester"),
                PluginType::Generic,
                perms,
                String::from("0.165.0"),
            ),
            binary,
            signature: sig.to_bytes().to_vec(),
            signer_cert: cert,
        }
    }

    fn setup_manager(keypair: &Sm2KeyPair) -> PluginManagerImpl {
        let verifier = SignatureVerifier::new(alloc::vec![keypair.public_key]);
        PluginManagerImpl::new(verifier, 0x4000_0000, 0x100_0000)
    }

    #[test]
    fn test_install_and_list() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut mgr = setup_manager(&kp);

        let pkg = make_signed_package(&kp, "my-plugin", alloc::vec![Permission::Ipc]);
        let id = mgr.install(&pkg).unwrap();
        assert_eq!(id, 1);

        let list = mgr.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].manifest.name, "my-plugin");
        assert_eq!(list[0].state, PluginState::Loaded);
    }

    #[test]
    fn test_enable_disable_lifecycle() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut mgr = setup_manager(&kp);

        let pkg = make_signed_package(&kp, "lifecycle", alloc::vec![]);
        let id = mgr.install(&pkg).unwrap();

        // Loaded → Enabled
        mgr.enable(id).unwrap();
        assert_eq!(mgr.get_state(id), Some(PluginState::Enabled));

        // Enabled → Disabled
        mgr.disable(id).unwrap();
        assert_eq!(mgr.get_state(id), Some(PluginState::Disabled));

        // Disabled → Enabled
        mgr.enable(id).unwrap();
        assert_eq!(mgr.get_state(id), Some(PluginState::Enabled));
    }

    #[test]
    fn test_uninstall() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut mgr = setup_manager(&kp);

        let pkg = make_signed_package(&kp, "to-remove", alloc::vec![]);
        let id = mgr.install(&pkg).unwrap();
        assert_eq!(mgr.list().len(), 1);

        mgr.uninstall(id).unwrap();
        assert_eq!(mgr.list().len(), 0);
        assert_eq!(mgr.get_state(id), None);
    }

    #[test]
    fn test_uninstall_not_found() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut mgr = setup_manager(&kp);
        assert_eq!(mgr.uninstall(999).unwrap_err(), PluginError::NotFound);
    }

    #[test]
    fn test_crash_and_restart() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut mgr = setup_manager(&kp);

        let pkg = make_signed_package(&kp, "crashable", alloc::vec![]);
        let id = mgr.install(&pkg).unwrap();
        mgr.enable(id).unwrap();

        // 模拟崩溃
        mgr.mark_crashed(id).unwrap();
        assert_eq!(mgr.get_state(id), Some(PluginState::Crashed));

        // 崩溃后无法 enable
        assert_eq!(mgr.enable(id).unwrap_err(), PluginError::PluginCrashed);

        // 重启
        mgr.restart(id).unwrap();
        assert_eq!(mgr.get_state(id), Some(PluginState::Loaded));

        // 重启后可再次 enable
        mgr.enable(id).unwrap();
        assert_eq!(mgr.get_state(id), Some(PluginState::Enabled));
    }

    #[test]
    fn test_incompatible_sdk_rejected() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut mgr = setup_manager(&kp);

        let mut pkg = make_signed_package(&kp, "future-plugin", alloc::vec![]);
        pkg.manifest.min_sdk_version = String::from("9.99.0"); // 未来版本

        assert_eq!(mgr.install(&pkg).unwrap_err(), PluginError::IncompatibleSdk);
        assert_eq!(mgr.stats().total_rejected, 1);
    }

    #[test]
    fn test_untrusted_signer_rejected() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let rogue_kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut mgr = setup_manager(&kp);

        // 用 rogue 密钥签名
        let pkg = make_signed_package(&rogue_kp, "rogue", alloc::vec![]);
        assert_eq!(mgr.install(&pkg).unwrap_err(), PluginError::UntrustedSigner);
    }

    #[test]
    fn test_find_by_name() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut mgr = setup_manager(&kp);

        let pkg = make_signed_package(&kp, "findme", alloc::vec![]);
        mgr.install(&pkg).unwrap();

        assert!(mgr.find_by_name("findme").is_some());
        assert!(mgr.find_by_name("nonexistent").is_none());
    }

    #[test]
    fn test_stats_tracking() {
        let mut rng = CsRng::new();
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();
        let mut mgr = setup_manager(&kp);

        let pkg = make_signed_package(&kp, "stats-test", alloc::vec![]);
        let id = mgr.install(&pkg).unwrap();
        mgr.enable(id).unwrap();

        let stats = mgr.stats();
        assert_eq!(stats.total_installed, 1);
        assert_eq!(stats.active_count, 1);

        mgr.disable(id).unwrap();
        assert_eq!(mgr.stats().active_count, 0);

        mgr.uninstall(id).unwrap();
        assert_eq!(mgr.stats().total_uninstalled, 1);
    }
}
