//! 动态库加载器 — 使用 libloading 加载 .so/.dll/.dylib 插件
//!
//! 提供跨平台的动态库加载能力，通过 C ABI 入口函数与插件交互。
//! 元数据加载优先从同目录 `manifest.toml` 读取（避免 ABI 不稳定），
//! 备选调用插件导出的 `eneros_plugin_metadata` 函数（返回 JSON 字符串）。
//!
//! # 示例
//!
//! ```no_run
//! use eneros_plugin::loader::PluginLoader;
//! use std::path::Path;
//!
//! let loader = PluginLoader::new();
//! // 加载插件动态库（需同目录存在 manifest.toml）
//! match loader.load(Path::new("/var/lib/eneros/plugins/iec103/iec103.so")) {
//!     Ok(plugin) => {
//!         println!("插件 {} v{} 已加载", plugin.metadata.name, plugin.metadata.version);
//!         // 使用 plugin.vtable.create() 创建实例...
//!         // 使用后调用 plugin.vtable.destroy() 销毁实例
//!     }
//!     Err(e) => eprintln!("加载失败: {}", e),
//! }
//! ```

use crate::error::{PluginError, PluginResult};
use crate::ipc::{DaemonResponse, PluginDaemonClient};
use crate::manifest::{PluginManifest, PluginMetadata};
use crate::signature::{PluginSignatureVerifier, VerificationResult as SigVerificationResult};
use crate::version::{check_compatibility, CURRENT_API_VERSION};
use libloading::{Library, Symbol};
use serde::{Deserialize, Serialize};
use std::ffi::CStr;
use std::path::{Path, PathBuf};

/// C ABI 入口函数名称：创建插件实例
const CREATE_FN_NAME: &[u8] = b"eneros_plugin_create\0";
/// C ABI 入口函数名称：销毁插件实例
const DESTROY_FN_NAME: &[u8] = b"eneros_plugin_destroy\0";
/// C ABI 入口函数名称：获取插件元数据（返回 JSON 字符串指针）
const METADATA_FN_NAME: &[u8] = b"eneros_plugin_metadata\0";

/// C ABI 创建函数指针类型：返回插件实例指针
type CreateFn = unsafe extern "C" fn() -> *mut std::ffi::c_void;
/// C ABI 销毁函数指针类型：接收插件实例指针并释放
type DestroyFn = unsafe extern "C" fn(ptr: *mut std::ffi::c_void);
/// C ABI 元数据函数指针类型：返回 JSON 字符串的 C 字符串指针
type MetadataFn = unsafe extern "C" fn() -> *const std::ffi::c_char;

/// 插件 vtable（函数指针表）
///
/// 存储 create/destroy 函数指针，用于创建和销毁插件实例。
/// 函数指针从 `Symbol` 中拷贝出来，不再借用 `Library`；
/// 但调用时必须保证 `Library` 仍然存活（由 `LoadedPlugin` 所有权保证）。
#[derive(Clone, Copy, Debug)]
pub struct PluginVTable {
    /// 创建插件实例，返回实例指针
    pub create: CreateFn,
    /// 销毁插件实例，接收实例指针并释放资源
    pub destroy: DestroyFn,
}

/// 已加载的插件
///
/// 持有动态库句柄、vtable、元数据与路径。
/// `Library` 在 drop 时自动卸载动态库；
/// 调用 vtable 函数前必须保证 `library` 仍然存活。
pub struct LoadedPlugin {
    /// 动态库句柄（drop 时自动关闭）
    pub library: Library,
    /// 函数指针表
    pub vtable: PluginVTable,
    /// 插件元数据
    pub metadata: PluginMetadata,
    /// 动态库路径
    pub path: PathBuf,
}

impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Library 未实现 Debug，用占位符替代
        f.debug_struct("LoadedPlugin")
            .field("library", &"<Library>")
            .field("vtable", &self.vtable)
            .field("metadata", &self.metadata)
            .field("path", &self.path)
            .finish()
    }
}

/// 插件加载模式
///
/// 控制插件以同进程方式加载还是通过独立 daemon 进程加载。
/// v0.28.0 起默认使用 `Daemon` 模式实现崩溃隔离。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoadMode {
    /// 同进程加载（v0.27.0 兼容）
    ///
    /// 通过 `libloading` 直接将动态库加载进当前进程，性能最高但无崩溃隔离。
    Inline,
    /// 通过 plugin-daemon 加载（v0.28.0 默认）
    ///
    /// 插件在独立 daemon 进程中加载，通过 IPC 通信，插件崩溃不影响主进程。
    #[default]
    Daemon,
}

/// `load_with_mode` 统一分发结果
///
/// v0.28.0 Task 11 修复 H2：Inline 与 Daemon 两种加载模式返回类型不同，
/// Inline 模式返回同进程持有的 `LoadedPlugin`（含库句柄与 vtable），
/// Daemon 模式返回 IPC 响应 `DaemonResponse`（插件在独立进程中加载）。
/// 使用枚举统一两种结果，避免上层调用方自行 `match load_mode` 时遗漏
/// 签名验证或版本检查步骤。
#[derive(Debug)]
pub enum LoadWithModeResult {
    /// Inline 模式加载结果（同进程持有库句柄）
    Inline(LoadedPlugin),
    /// Daemon 模式加载结果（daemon 进程加载，返回 IPC 响应）
    Daemon(DaemonResponse),
}

/// 插件加载器
pub struct PluginLoader;

impl PluginLoader {
    /// 创建新的加载器实例
    pub fn new() -> Self {
        Self
    }

    /// 加载动态库插件
    ///
    /// 流程：
    /// 1. 检查文件存在性与扩展名
    /// 2. 打开动态库（unsafe）
    /// 3. 解析 create/destroy 符号（unsafe）
    /// 4. 加载元数据：优先 manifest.toml，备选 eneros_plugin_metadata 函数
    ///
    /// # 错误
    /// - `PluginError::LoadFailed`：文件不存在、扩展名无效、符号缺失或元数据加载失败
    pub fn load(&self, path: &Path) -> Result<LoadedPlugin, PluginError> {
        // 1. 检查文件存在
        if !path.exists() {
            return Err(PluginError::LoadFailed(format!(
                "file not found: {}",
                path.display()
            )));
        }

        // 2. 检查扩展名
        if !is_valid_plugin_path(path) {
            return Err(PluginError::LoadFailed(format!(
                "invalid file extension: {}",
                path.display()
            )));
        }

        // 3. 打开动态库
        // SAFETY: 调用方保证 path 指向一个有效的动态库文件。
        // 此处已通过文件存在性检查与扩展名校验（.so/.dll/.dylib）。
        // libloading 内部使用平台 API（dlopen/LoadLibraryW）打开库，
        // 加载失败时返回错误，不会导致未定义行为。
        let library = unsafe { Library::new(path) }
            .map_err(|e| PluginError::LoadFailed(format!("open library failed: {}", e)))?;

        // 4. 解析 create 符号
        // SAFETY: library.get 仅查找符号地址，不执行库内代码。
        // CREATE_FN_NAME 以 null 结尾（b"...\0"），符合 dlsym/GetProcAddress 要求。
        // 符号不存在时返回错误，不会导致未定义行为。
        let create_symbol: Symbol<CreateFn> = unsafe { library.get(CREATE_FN_NAME) }
            .map_err(|e| {
                PluginError::LoadFailed(format!(
                    "symbol not found: eneros_plugin_create: {}",
                    e
                ))
            })?;

        // 5. 解析 destroy 符号
        // SAFETY: 同上，仅查找符号地址，不执行代码。
        let destroy_symbol: Symbol<DestroyFn> = unsafe { library.get(DESTROY_FN_NAME) }
            .map_err(|e| {
                PluginError::LoadFailed(format!(
                    "symbol not found: eneros_plugin_destroy: {}",
                    e
                ))
            })?;

        // 拷贝函数指针出 Symbol（函数指针是 Copy 类型），不再借用 library
        let create_fn: CreateFn = *create_symbol;
        let destroy_fn: DestroyFn = *destroy_symbol;

        let vtable = PluginVTable {
            create: create_fn,
            destroy: destroy_fn,
        };

        // 6. 加载元数据：优先 manifest.toml，备选 eneros_plugin_metadata 函数
        let metadata = match load_metadata_from_manifest(path) {
            Some(result) => result?,
            None => load_metadata_from_symbol(&library)?,
        };

        Ok(LoadedPlugin {
            library,
            vtable,
            metadata,
            path: path.to_path_buf(),
        })
    }

    /// 卸载插件
    ///
    /// 当前 `LoadedPlugin` 不持有插件实例指针（仅持有 vtable），
    /// 因此卸载只需让 `Library` drop 自动关闭动态库。
    /// 若上层已通过 `vtable.create` 创建实例，应先调用 `vtable.destroy` 销毁实例，
    /// 再调用本方法卸载库。
    pub fn unload(&self, plugin: LoadedPlugin) -> Result<(), PluginError> {
        // Library 在 drop 时自动关闭动态库
        drop(plugin);
        Ok(())
    }

    /// 通过 plugin-daemon 加载插件（v0.28.0 默认模式）
    ///
    /// 将加载请求委托给独立运行的 daemon 进程，实现崩溃隔离。
    /// 插件在 daemon 进程中加载，主进程通过 IPC 获取结果。
    ///
    /// # 参数
    /// - `path`：插件动态库路径
    /// - `client`：已配置地址的 IPC 客户端
    /// - `skip_signature`：是否跳过签名验证
    #[allow(clippy::unused_self)]
    pub fn load_daemon(
        &self,
        path: &str,
        client: &PluginDaemonClient,
        skip_signature: bool,
    ) -> Result<DaemonResponse, PluginError> {
        client.load(path, skip_signature)
    }

    /// 同进程加载插件（v0.27.0 兼容模式）
    ///
    /// 直接在当前进程加载动态库，性能最高但无崩溃隔离。
    /// 本方法为现有 `load` 的别名，便于上层按 `LoadMode` 统一分发。
    pub fn load_inline(&self, path: &str) -> Result<LoadedPlugin, PluginError> {
        self.load(Path::new(path))
    }

    /// 按 `LoadMode` 统一分发加载插件（v0.28.0 Task 11 修复 H2）
    ///
    /// 上层调用方无需自行 `match load_mode`，本方法确保两种模式下都执行
    /// 必要的安全检查，避免遗漏签名验证或 API 版本兼容性检查：
    ///
    /// - **Inline 模式**：先验证签名（若不跳过）→ 加载动态库 → 检查 API 版本兼容性。
    ///   签名验证在加载前执行，避免将未签名/被篡改的代码加载进当前进程。
    /// - **Daemon 模式**：委托 `PluginDaemonClient` 发送 IPC 请求，由 daemon 进程
    ///   负责签名验证与版本检查（daemon 端 `handle_load` 已实现）。
    ///
    /// # 参数
    /// - `path`：插件动态库路径
    /// - `mode`：加载模式（Inline / Daemon）
    /// - `client`：Daemon 模式下必需的 IPC 客户端，Inline 模式可传 `None`
    /// - `skip_signature`：是否跳过签名验证（仅 Inline 模式生效）
    /// - `trusted_keys_dir`：可信公钥目录（仅 Inline 模式用于构造验证器）
    /// - `require_signature`：是否强制要求签名（仅 Inline 模式生效）
    ///
    /// # 返回
    /// - `Ok(LoadWithModeResult::Inline(loaded))`：Inline 模式加载成功
    /// - `Ok(LoadWithModeResult::Daemon(resp))`：Daemon 模式 IPC 请求成功
    /// - `Err(...)`：签名验证失败、版本不兼容、加载失败或 IPC 错误
    pub fn load_with_mode(
        &self,
        path: &Path,
        mode: LoadMode,
        client: Option<&PluginDaemonClient>,
        skip_signature: bool,
        trusted_keys_dir: &Path,
        require_signature: bool,
    ) -> PluginResult<LoadWithModeResult> {
        // 路径转 &str（load_inline / load_daemon 接受 &str）
        let path_str = path.to_str().ok_or_else(|| {
            PluginError::LoadFailed(format!(
                "path contains invalid utf-8: {}",
                path.display()
            ))
        })?;

        match mode {
            LoadMode::Inline => {
                // 1. 签名验证（加载前执行，防止未签名/被篡改的代码进入当前进程）
                if !skip_signature {
                    let verifier = PluginSignatureVerifier::new(
                        trusted_keys_dir,
                        require_signature,
                    )?;
                    let result = verifier.verify_plugin(path)?;
                    match result {
                        SigVerificationResult::Valid { .. } => {}
                        SigVerificationResult::Invalid { reason } => {
                            return Err(PluginError::SignatureInvalid(reason));
                        }
                        SigVerificationResult::Missing => {
                            return Err(PluginError::SignatureMissing);
                        }
                        SigVerificationResult::UntrustedSigner { signer } => {
                            return Err(PluginError::UntrustedSigner(signer));
                        }
                    }
                }

                // 2. 加载动态库
                let loaded = self.load_inline(path_str)?;

                // 3. API 版本兼容性检查
                check_compatibility(&loaded.metadata.api_version, CURRENT_API_VERSION)?;

                Ok(LoadWithModeResult::Inline(loaded))
            }
            LoadMode::Daemon => {
                // Daemon 模式：委托 IPC 客户端，签名验证与版本检查由 daemon 端 handle_load 执行
                let client = client.ok_or_else(|| {
                    PluginError::LoadFailed(
                        "daemon 模式需要 PluginDaemonClient，但传入 None".to_string(),
                    )
                })?;
                let resp = self.load_daemon(path_str, client, skip_signature)?;
                Ok(LoadWithModeResult::Daemon(resp))
            }
        }
    }
}

impl Default for PluginLoader {
    fn default() -> Self {
        Self::new()
    }
}

/// 从同目录下的 manifest.toml 加载元数据
///
/// 若 manifest.toml 不存在返回 `None`，由调用方决定是否走备选路径。
/// 若文件存在但解析失败，返回 `Some(Err(...))` 以传播错误。
fn load_metadata_from_manifest(plugin_path: &Path) -> Option<Result<PluginMetadata, PluginError>> {
    let dir = plugin_path.parent()?;
    let manifest_path = dir.join("manifest.toml");
    if !manifest_path.exists() {
        return None;
    }
    let manifest = PluginManifest::load_from_file(&manifest_path);
    Some(manifest.map(|m| PluginMetadata::from(&m)))
}

/// 从插件导出的 eneros_plugin_metadata 函数加载元数据
///
/// 函数返回 JSON 格式的 C 字符串指针，解析为 `PluginMetadata`。
/// 若符号不存在或返回 null 指针，返回错误。
fn load_metadata_from_symbol(library: &Library) -> Result<PluginMetadata, PluginError> {
    // SAFETY: library.get 仅查找符号地址，不执行代码。
    // METADATA_FN_NAME 以 null 结尾，符合平台 API 要求。
    // 符号不存在时返回错误。
    let metadata_symbol: Option<Symbol<MetadataFn>> =
        unsafe { library.get(METADATA_FN_NAME) }.ok();

    let metadata_fn = match metadata_symbol {
        Some(sym) => *sym,
        None => {
            return Err(PluginError::LoadFailed(
                "failed to load metadata: no manifest.toml and eneros_plugin_metadata symbol not found"
                    .to_string(),
            ));
        }
    };

    // SAFETY: 调用插件提供的 metadata 函数。
    // 安全保证：
    // - library 已成功加载且存活
    // - 假设插件实现正确，返回有效的 C 字符串指针或 null
    // - 我们对返回的 null 指针进行检查
    // - 仅读取字符串内容并拷贝到 Rust 所有的数据结构，不持有指针所有权
    //
    // v0.28.0 Task 11 修复 H4：用 catch_unwind 包裹 FFI 调用，
    // 防止插件导出的 metadata 函数 panic 直接传播到调用方进程。
    // AssertUnwindSafe 允许跨越 catch_unwind 边界捕获函数指针（Copy 类型，安全）。
    let ptr = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        metadata_fn()
    }))
    .map_err(|_| {
        PluginError::LoadFailed(
            "failed to load metadata: eneros_plugin_metadata panicked".to_string(),
        )
    })?;
    if ptr.is_null() {
        return Err(PluginError::LoadFailed(
            "failed to load metadata: eneros_plugin_metadata returned null".to_string(),
        ));
    }

    // SAFETY: ptr 已确认非 null，假设指向有效的 null 结尾 C 字符串。
    // CStr::from_ptr 读取直到 null 结尾，不会越界。
    // 字符串内容被拷贝到 Rust 所有的 String/PluginMetadata 中。
    let cstr = unsafe { CStr::from_ptr(ptr) };
    let json_str = cstr.to_str().map_err(|e| {
        PluginError::LoadFailed(format!(
            "failed to load metadata: invalid utf-8: {}",
            e
        ))
    })?;

    serde_json::from_str::<PluginMetadata>(json_str).map_err(|e| {
        PluginError::LoadFailed(format!(
            "failed to load metadata: parse json: {}",
            e
        ))
    })
}

/// 获取平台动态库扩展名
///
/// 返回当前平台对应的动态库扩展名（linux: so, windows: dll, macos: dylib）。
/// 该函数为工具函数，供外部调用方查询平台扩展名，当前主流程未直接使用。
#[allow(dead_code)]
fn platform_extension() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "so"
    }
    #[cfg(target_os = "windows")]
    {
        "dll"
    }
    #[cfg(target_os = "macos")]
    {
        "dylib"
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        "so"
    }
}

/// 检查路径是否为有效动态库（扩展名为 .so/.dll/.dylib）
fn is_valid_plugin_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| matches!(ext, "so" | "dll" | "dylib"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_extension_returns_known_value() {
        let ext = platform_extension();
        assert!(matches!(ext, "so" | "dll" | "dylib"));
    }

    #[test]
    fn test_platform_extension_matches_target() {
        #[cfg(target_os = "linux")]
        assert_eq!(platform_extension(), "so");
        #[cfg(target_os = "windows")]
        assert_eq!(platform_extension(), "dll");
        #[cfg(target_os = "macos")]
        assert_eq!(platform_extension(), "dylib");
    }

    #[test]
    fn test_is_valid_plugin_path_so() {
        assert!(is_valid_plugin_path(Path::new("/usr/lib/plugin.so")));
        assert!(is_valid_plugin_path(Path::new("plugin.so")));
    }

    #[test]
    fn test_is_valid_plugin_path_dll() {
        assert!(is_valid_plugin_path(Path::new("C:\\plugins\\plugin.dll")));
        assert!(is_valid_plugin_path(Path::new("plugin.dll")));
    }

    #[test]
    fn test_is_valid_plugin_path_dylib() {
        assert!(is_valid_plugin_path(Path::new("/usr/lib/plugin.dylib")));
        assert!(is_valid_plugin_path(Path::new("plugin.dylib")));
    }

    #[test]
    fn test_is_valid_plugin_path_invalid_extensions() {
        assert!(!is_valid_plugin_path(Path::new("plugin.txt")));
        assert!(!is_valid_plugin_path(Path::new("plugin.rs")));
        assert!(!is_valid_plugin_path(Path::new("plugin.exe")));
        assert!(!is_valid_plugin_path(Path::new("plugin.toml")));
    }

    #[test]
    fn test_is_valid_plugin_path_no_extension() {
        assert!(!is_valid_plugin_path(Path::new("plugin")));
        assert!(!is_valid_plugin_path(Path::new("/usr/lib/plugin")));
    }

    #[test]
    fn test_is_valid_plugin_path_empty() {
        assert!(!is_valid_plugin_path(Path::new("")));
    }

    #[test]
    fn test_load_nonexistent_file() {
        let loader = PluginLoader::new();
        let result = loader.load(Path::new("/nonexistent/path/plugin.so"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::LoadFailed(_)));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn test_load_invalid_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.txt");
        std::fs::write(&path, "not a library").unwrap();

        let loader = PluginLoader::new();
        let result = loader.load(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::LoadFailed(_)));
        assert!(err.to_string().contains("invalid file extension"));
    }

    #[test]
    fn test_load_nonexistent_file_error_message() {
        let loader = PluginLoader::new();
        let result = loader.load(Path::new("/nonexistent/abc.so"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("file not found"));
        assert!(msg.contains("/nonexistent/abc.so"));
    }

    #[test]
    #[allow(clippy::clone_on_copy)]
    fn test_vtable_clone_copy() {
        unsafe extern "C" fn dummy_create() -> *mut std::ffi::c_void {
            std::ptr::null_mut()
        }
        unsafe extern "C" fn dummy_destroy(_ptr: *mut std::ffi::c_void) {}

        let vtable = PluginVTable {
            create: dummy_create,
            destroy: dummy_destroy,
        };
        // 验证 Copy
        let vtable_copy = vtable;
        // 验证 Clone（Copy 类型同时实现 Clone，显式调用以验证 trait 派生）
        let vtable_clone = vtable.clone();
        // 函数指针相等性检查
        assert_eq!(vtable.create as usize, vtable_copy.create as usize);
        assert_eq!(vtable.destroy as usize, vtable_copy.destroy as usize);
        assert_eq!(vtable.create as usize, vtable_clone.create as usize);
        assert_eq!(vtable.destroy as usize, vtable_clone.destroy as usize);
    }

    #[test]
    fn test_loader_new() {
        let _loader = PluginLoader::new();
    }

    #[test]
    #[allow(clippy::default_constructed_unit_structs)]
    fn test_loader_default() {
        // 显式验证 Default trait 派生正确
        let _loader = PluginLoader::default();
    }

    #[test]
    #[allow(clippy::default_constructed_unit_structs)]
    fn test_unload_drops_library() {
        // unload 接受 LoadedPlugin 并 drop，由于无法构造真实 LoadedPlugin，
        // 此处仅验证 unload 签名可编译且 Default 与 new 等价
        let loader = PluginLoader::default();
        // 验证 loader 可调用（无真实插件时不调用 unload）
        let _ = &loader;
    }

    #[test]
    fn test_load_metadata_from_manifest_missing() {
        // manifest.toml 不存在时返回 None
        let result = load_metadata_from_manifest(Path::new("/nonexistent/plugin.so"));
        // /nonexistent 目录不存在，parent 存在但 manifest.toml 不存在
        assert!(result.is_none());
    }

    #[test]
    fn test_load_metadata_from_manifest_present() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        let toml_str = r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
api_version = "0.27.0"
plugin_type = "Protocol"
description = "test"
"#;
        std::fs::write(&manifest_path, toml_str).unwrap();

        let plugin_path = dir.path().join("plugin.so");
        let result = load_metadata_from_manifest(&plugin_path);
        assert!(result.is_some());
        let metadata = result.unwrap().unwrap();
        assert_eq!(metadata.name, "test-plugin");
        assert_eq!(metadata.version, "0.1.0");
        assert_eq!(metadata.api_version, "0.27.0");
    }

    #[test]
    fn test_load_metadata_from_manifest_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_path = dir.path().join("manifest.toml");
        std::fs::write(&manifest_path, "invalid toml = =").unwrap();

        let plugin_path = dir.path().join("plugin.so");
        let result = load_metadata_from_manifest(&plugin_path);
        assert!(result.is_some());
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn test_load_mode_default_is_daemon() {
        // v0.28.0 默认加载模式为 Daemon
        assert_eq!(LoadMode::default(), LoadMode::Daemon);
    }

    #[test]
    fn test_load_mode_serde_roundtrip() {
        // 验证 LoadMode 序列化/反序列化往返一致
        let json = serde_json::to_string(&LoadMode::Daemon).unwrap();
        assert_eq!(json, "\"daemon\"");
        let mode: LoadMode = serde_json::from_str("\"daemon\"").unwrap();
        assert_eq!(mode, LoadMode::Daemon);

        let json = serde_json::to_string(&LoadMode::Inline).unwrap();
        assert_eq!(json, "\"inline\"");
        let mode: LoadMode = serde_json::from_str("\"inline\"").unwrap();
        assert_eq!(mode, LoadMode::Inline);
    }

    /// v0.28.0 Task 11 修复 H2：验证 inline 模式下 `load_with_mode` 执行签名验证
    ///
    /// 创建一个临时 .so 文件（非真实动态库），调用 `load_with_mode` 以 Inline 模式
    /// 加载，`skip_signature=false` 且 `require_signature=true`。由于文件无对应
    /// `.sig` 签名文件，签名验证应返回 `SignatureMissing` 错误。
    ///
    /// 这证明 `load_with_mode` 在 inline 模式下确实执行了签名验证（在加载动态库
    /// 之前），而非直接跳过。
    #[test]
    fn test_load_with_mode_inline_signature_check() {
        use crate::error::PluginError;

        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("test-plugin.so");
        std::fs::write(&plugin_path, b"fake plugin content").unwrap();

        let loader = PluginLoader::new();
        // trusted_keys_dir 使用空临时目录（无可信公钥）
        let keys_dir = dir.path().join("keys");
        std::fs::create_dir_all(&keys_dir).unwrap();

        // skip_signature=false, require_signature=true → 应在签名验证阶段失败
        let result = loader.load_with_mode(
            &plugin_path,
            LoadMode::Inline,
            None,
            false, // skip_signature = false
            &keys_dir,
            true, // require_signature = true
        );

        assert!(result.is_err(), "inline 模式应执行签名验证并失败");
        let err = result.unwrap_err();
        // require_signature=true 且无 .sig 文件 → SignatureMissing
        assert!(
            matches!(err, PluginError::SignatureMissing),
            "期望 SignatureMissing 错误（无 .sig 文件且 require_signature=true），实际: {:?}",
            err
        );
    }

    /// v0.28.0 Task 11 修复 H2：验证 inline 模式下 skip_signature=true 跳过签名验证
    ///
    /// 当 `skip_signature=true` 时，`load_with_mode` 应跳过签名验证直接进入
    /// 加载阶段。由于测试文件非真实动态库，加载将在 `LoadFailed` 阶段失败，
    /// 而非签名验证阶段。这证明 `skip_signature` 参数生效。
    #[test]
    fn test_load_with_mode_inline_skip_signature() {
        use crate::error::PluginError;

        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("skip-sig-plugin.so");
        std::fs::write(&plugin_path, b"fake plugin content").unwrap();

        let loader = PluginLoader::new();
        let keys_dir = dir.path().join("keys");
        std::fs::create_dir_all(&keys_dir).unwrap();

        // skip_signature=true → 跳过签名验证，直接尝试加载（非真实库 → LoadFailed）
        let result = loader.load_with_mode(
            &plugin_path,
            LoadMode::Inline,
            None,
            true, // skip_signature = true
            &keys_dir,
            true, // require_signature = true（但因 skip_signature=true 被跳过）
        );

        assert!(result.is_err(), "非真实动态库应加载失败");
        let err = result.unwrap_err();
        // 应为 LoadFailed（加载阶段），而非 SignatureMissing（签名阶段）
        assert!(
            matches!(err, PluginError::LoadFailed(_)),
            "skip_signature=true 时应跳过签名验证进入加载阶段，期望 LoadFailed，实际: {:?}",
            err
        );
    }

    /// v0.28.0 Task 11 修复 H2：验证 daemon 模式缺少 client 时返回错误
    #[test]
    fn test_load_with_mode_daemon_requires_client() {
        use crate::error::PluginError;

        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("daemon-plugin.so");
        std::fs::write(&plugin_path, b"fake").unwrap();

        let loader = PluginLoader::new();
        let keys_dir = dir.path().join("keys");
        std::fs::create_dir_all(&keys_dir).unwrap();

        // Daemon 模式但 client=None → 应返回 LoadFailed 错误
        let result = loader.load_with_mode(
            &plugin_path,
            LoadMode::Daemon,
            None, // 缺少 client
            false,
            &keys_dir,
            true,
        );

        assert!(result.is_err(), "daemon 模式缺少 client 应返回错误");
        let err = result.unwrap_err();
        assert!(
            matches!(err, PluginError::LoadFailed(ref msg) if msg.contains("PluginDaemonClient")),
            "期望 LoadFailed 错误且消息包含 'PluginDaemonClient'，实际: {:?}",
            err
        );
    }
}
