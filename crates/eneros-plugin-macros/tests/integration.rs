//! eneros-plugin-macros 集成测试
//!
//! 过程宏 crate 的 `#[cfg(test)]` 单元测试不会被编译到测试二进制中，
//! 因此使用 `tests/` 目录的集成测试来验证宏逻辑。
//! 本测试通过在 mock 插件上应用 `#[eneros_plugin]` 宏，验证生成的 C ABI 入口函数。

use eneros_plugin::{Plugin, PluginMetadata, PluginType, PluginResult};
use std::ffi::CStr;
use std::sync::OnceLock;

/// 测试用 mock 插件（unit struct，便于 `Box::new` 构造）
struct MockPlugin;

/// 静态元数据存储（OnceLock 保证线程安全初始化）
static MOCK_METADATA: OnceLock<PluginMetadata> = OnceLock::new();

/// 应用 `#[eneros_plugin]` 宏 — 自动生成 C ABI 入口函数
#[eneros_plugin_macros::eneros_plugin(
    name = "test-plugin",
    version = "1.0.0",
    api_version = "0.27.0",
    plugin_type = "protocol",
    author = "EnerOS Team",
    description = "Mock plugin for testing"
)]
#[async_trait::async_trait]
impl Plugin for MockPlugin {
    fn metadata(&self) -> &PluginMetadata {
        MOCK_METADATA.get_or_init(|| PluginMetadata {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            api_version: "0.27.0".to_string(),
            plugin_type: PluginType::Protocol,
            description: "Mock plugin for testing".to_string(),
        })
    }

    fn plugin_type(&self) -> PluginType {
        PluginType::Protocol
    }

    async fn init(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn start(&mut self) -> PluginResult<()> {
        Ok(())
    }

    async fn stop(&mut self) -> PluginResult<()> {
        Ok(())
    }
}

/// 测试 1：宏成功展开并生成 C ABI 入口函数
///
/// 验证宏标注后能编译通过（即宏展开成功），且生成的
/// `eneros_plugin_create` / `eneros_plugin_destroy` 函数可正常调用。
#[test]
fn test_macro_expands() {
    // 调用宏生成的 create 函数
    let ptr = eneros_plugin_create();
    assert!(!ptr.is_null(), "eneros_plugin_create 应返回非空指针");

    // 调用宏生成的 destroy 函数释放实例
    // eneros_plugin_destroy 是安全的 extern "C" fn，内部自行处理 unsafe 操作
    eneros_plugin_destroy(ptr);

    // 验证 destroy 对 null 指针安全（no-op）
    eneros_plugin_destroy(std::ptr::null_mut());
}

/// 测试 2：宏生成的 metadata 函数返回正确的 JSON
///
/// 验证 `eneros_plugin_metadata` 返回的 C 字符串包含正确的字段值，
/// 且 plugin_type 被规范为首字母大写形式（匹配 PluginType 枚举的 serde 序列化）。
#[test]
fn test_metadata_generation() {
    // SAFETY: eneros_plugin_metadata 返回有效的 C 字符串指针
    let ptr = eneros_plugin_metadata();
    assert!(!ptr.is_null(), "eneros_plugin_metadata 应返回非空指针");

    let cstr = unsafe { CStr::from_ptr(ptr) };
    let json = cstr.to_str().expect("metadata 应为有效 UTF-8");

    // 验证 JSON 包含正确的字段
    assert!(
        json.contains(r#""name":"test-plugin""#),
        "JSON 应包含 name 字段: {}",
        json
    );
    assert!(
        json.contains(r#""version":"1.0.0""#),
        "JSON 应包含 version 字段: {}",
        json
    );
    assert!(
        json.contains(r#""api_version":"0.27.0""#),
        "JSON 应包含 api_version 字段: {}",
        json
    );
    assert!(
        json.contains(r#""author":"EnerOS Team""#),
        "JSON 应包含 author 字段: {}",
        json
    );
    assert!(
        json.contains(r#""description":"Mock plugin for testing""#),
        "JSON 应包含 description 字段: {}",
        json
    );

    // 注意：metadata 函数返回的指针由 CString::into_raw 创建，内存泄漏在测试中可接受
}

/// 测试 3：验证 plugin_type 规范化与 metadata 反序列化
///
/// 宏接受小写 plugin_type（如 "protocol"），但生成的 metadata JSON 中
/// 应使用首字母大写形式（"Protocol"），以匹配 PluginType 枚举的 serde 序列化。
/// 同时验证 metadata JSON 可被反序列化为 PluginMetadata。
#[test]
fn test_plugin_type_validation() {
    // SAFETY: eneros_plugin_metadata 返回有效的 C 字符串指针
    let ptr = eneros_plugin_metadata();
    let cstr = unsafe { CStr::from_ptr(ptr) };
    let json = cstr.to_str().expect("metadata 应为有效 UTF-8");

    // 验证 plugin_type 被规范为首字母大写形式
    assert!(
        json.contains(r#""plugin_type":"Protocol""#),
        "JSON 应包含规范化的 plugin_type (Protocol): {}",
        json
    );
    // 确保不是小写形式
    assert!(
        !json.contains(r#""plugin_type":"protocol""#),
        "JSON 不应包含小写 plugin_type: {}",
        json
    );

    // 验证 metadata JSON 可被反序列化为 PluginMetadata
    let metadata: PluginMetadata = serde_json::from_str(json)
        .expect("metadata JSON 应可反序列化为 PluginMetadata");
    assert_eq!(metadata.name, "test-plugin");
    assert_eq!(metadata.version, "1.0.0");
    assert_eq!(metadata.api_version, "0.27.0");
    assert_eq!(metadata.plugin_type, PluginType::Protocol);
}
