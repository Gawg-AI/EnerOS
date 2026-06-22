//! eneros-plugin-macros — `#[eneros_plugin]` 过程宏
//!
//! 为 EnerOS 插件自动生成 C ABI 入口函数（eneros_plugin_create /
//! eneros_plugin_destroy / eneros_plugin_metadata），
//! 简化插件开发。开发者只需在 `impl Plugin for MyPlugin` 上标注
//! `#[eneros_plugin(...)]` 即可。
//!
//! # 示例
//!
//! ```ignore
//! use eneros_plugin::{Plugin, PluginMetadata, PluginType, PluginResult};
//!
//! struct MyPlugin;
//!
//! #[eneros_plugin_macros::eneros_plugin(
//!     name = "my-plugin",
//!     version = "1.0.0",
//!     api_version = "0.27.0",
//!     plugin_type = "protocol",
//!     author = "EnerOS Team",
//!     description = "My custom plugin"
//! )]
//! impl Plugin for MyPlugin {
//!     // ... trait 方法实现
//! }
//! ```

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    Ident, ItemImpl, LitStr, Token,
    parse::{Parse, ParseStream},
    parse2,
};

/// 过程宏属性参数
///
/// 对应 `#[eneros_plugin(name = "...", version = "...", ...)]` 中的键值对。
struct PluginArgs {
    /// 插件名称（唯一标识）
    name: String,
    /// 插件版本
    version: String,
    /// 插件 API 版本（与 EnerOS API 版本兼容性检查）
    api_version: String,
    /// 插件类型（protocol | agent | analysis）
    plugin_type: String,
    /// 插件作者（可选）
    author: Option<String>,
    /// 插件描述（可选）
    description: Option<String>,
}

impl Parse for PluginArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut api_version = None;
        let mut plugin_type = None;
        let mut author = None;
        let mut description = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value: LitStr = input.parse()?;

            match key.to_string().as_str() {
                "name" => name = Some(value.value()),
                "version" => version = Some(value.value()),
                "api_version" => api_version = Some(value.value()),
                "plugin_type" => plugin_type = Some(value.value()),
                "author" => author = Some(value.value()),
                "description" => description = Some(value.value()),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "未知属性 `{}`，支持的属性: name, version, api_version, plugin_type, author, description",
                            other
                        ),
                    ));
                }
            }

            // 可选逗号分隔
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(PluginArgs {
            name: name.ok_or_else(|| syn::Error::new(Span::call_site(), "缺少必需属性 `name`"))?,
            version: version.ok_or_else(|| syn::Error::new(Span::call_site(), "缺少必需属性 `version`"))?,
            api_version: api_version
                .ok_or_else(|| syn::Error::new(Span::call_site(), "缺少必需属性 `api_version`"))?,
            plugin_type: plugin_type
                .ok_or_else(|| syn::Error::new(Span::call_site(), "缺少必需属性 `plugin_type`"))?,
            author,
            description,
        })
    }
}

/// 验证 plugin_type 是否合法
///
/// 仅接受小写形式：`protocol` | `agent` | `analysis`。
fn validate_plugin_type(plugin_type: &str) -> Result<(), syn::Error> {
    match plugin_type {
        "protocol" | "agent" | "analysis" => Ok(()),
        other => Err(syn::Error::new(
            Span::call_site(),
            format!(
                "无效的 plugin_type `{}`，期望: protocol | agent | analysis",
                other
            ),
        )),
    }
}

/// 将小写 plugin_type 转为 PluginType 枚举的 serde 序列化形式（首字母大写）
///
/// `PluginType` 枚举序列化为 `"Protocol"` / `"Agent"` / `"Analysis"`，
/// 因此 metadata JSON 中必须使用首字母大写形式。
fn canonical_plugin_type(plugin_type: &str) -> &str {
    match plugin_type {
        "protocol" => "Protocol",
        "agent" => "Agent",
        "analysis" => "Analysis",
        // 验证后不应到达此处，返回原值作为兜底
        other => other,
    }
}

/// JSON 字符串转义
///
/// 转义双引号、反斜杠和控制字符，确保生成的 JSON 字符串合法。
/// 符合 RFC 8259 规范：所有控制字符（U+0000 ~ U+001F）必须被转义。
fn json_escape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => result.push_str(r#"\""#),
            '\\' => result.push_str(r"\\"),
            '\n' => result.push_str(r"\n"),
            '\r' => result.push_str(r"\r"),
            '\t' => result.push_str(r"\t"),
            // 转义控制字符（0x00-0x1F），符合 RFC 8259 规范
            c if (c as u32) < 0x20 => result.push_str(&format!("\\u{:04x}", c as u32)),
            c => result.push(c),
        }
    }
    result
}

/// 生成 metadata JSON 字符串
///
/// 格式与 `eneros_plugin::manifest::PluginMetadata` 的 serde 反序列化兼容。
/// `author` 字段为前向兼容保留（PluginMetadata 当前无此字段，但 serde 忽略未知字段）。
fn generate_metadata_json(args: &PluginArgs) -> String {
    let pt = canonical_plugin_type(&args.plugin_type);
    format!(
        r#"{{"name":"{}","version":"{}","api_version":"{}","plugin_type":"{}","author":"{}","description":"{}"}}"#,
        json_escape(&args.name),
        json_escape(&args.version),
        json_escape(&args.api_version),
        pt,
        json_escape(args.author.as_deref().unwrap_or("")),
        json_escape(args.description.as_deref().unwrap_or("")),
    )
}

/// 宏内部实现（使用 proc_macro2::TokenStream）
fn eneros_plugin_impl(
    args: proc_macro2::TokenStream,
    input: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    // 解析属性参数
    let plugin_args = match parse2::<PluginArgs>(args) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error(),
    };

    // 解析 impl 块
    let item_impl = match parse2::<ItemImpl>(input) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error(),
    };

    // 验证 plugin_type
    if let Err(e) = validate_plugin_type(&plugin_args.plugin_type) {
        return e.to_compile_error();
    }

    // 提取实现类型
    let self_ty = &item_impl.self_ty;

    // 生成 metadata JSON
    let metadata_json = generate_metadata_json(&plugin_args);

    quote! {
        // 保留原始 impl 块
        #item_impl

        /// C ABI 入口：创建插件实例
        ///
        /// 返回 `Box<#self_ty>` 的裸指针（以 `*mut c_void` 形式），
        /// 调用方负责通过 `eneros_plugin_destroy` 释放。
        #[no_mangle]
        pub extern "C" fn eneros_plugin_create() -> *mut std::ffi::c_void {
            let plugin: Box<#self_ty> = Box::new(#self_ty);
            Box::into_raw(plugin) as *mut std::ffi::c_void
        }

        /// C ABI 入口：销毁插件实例
        ///
        /// 接收 `eneros_plugin_create` 返回的裸指针并释放内存。
        /// 传入 null 指针时安全返回（no-op）。
        #[no_mangle]
        pub extern "C" fn eneros_plugin_destroy(ptr: *mut std::ffi::c_void) {
            if !ptr.is_null() {
                // SAFETY: ptr 来自 eneros_plugin_create 的 Box::into_raw，
                // 类型为 *mut #self_ty，Box::from_raw 还原后 drop 释放内存。
                unsafe {
                    drop(Box::from_raw(ptr as *mut #self_ty));
                }
            }
        }

        /// C ABI 入口：获取插件元数据（JSON 字符串）
        ///
        /// 返回 null 结尾的 C 字符串指针，内容为 JSON 格式的元数据。
        /// 指针指向静态缓冲区（OnceLock<CString>），无需释放，零泄漏。
        /// 调用方仅读取内容，不持有指针所有权。
        #[no_mangle]
        pub extern "C" fn eneros_plugin_metadata() -> *const std::ffi::c_char {
            use std::ffi::CString;
            use std::sync::OnceLock;
            static METADATA: OnceLock<CString> = OnceLock::new();
            let metadata = METADATA.get_or_init(|| {
                CString::new(#metadata_json)
                    .unwrap_or_else(|_| CString::new("").unwrap())
            });
            metadata.as_ptr() as *const std::ffi::c_char
        }
    }
}

/// `#[eneros_plugin]` 过程宏 — 自动生成 C ABI 入口函数
///
/// 标注在 `impl Plugin for MyPlugin` 块上，自动生成：
/// - `eneros_plugin_create` — 创建插件实例
/// - `eneros_plugin_destroy` — 销毁插件实例
/// - `eneros_plugin_metadata` — 返回元数据 JSON 字符串
///
/// # 属性参数
///
/// - `name`（必需）— 插件名称
/// - `version`（必需）— 插件版本
/// - `api_version`（必需）— 插件 API 版本
/// - `plugin_type`（必需）— 插件类型：`protocol` | `agent` | `analysis`
/// - `author`（可选）— 插件作者
/// - `description`（可选）— 插件描述
#[proc_macro_attribute]
pub fn eneros_plugin(args: TokenStream, input: TokenStream) -> TokenStream {
    eneros_plugin_impl(args.into(), input.into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_escape_control_chars() {
        // null 字节必须被转义为 \u0000，避免 CString::new panic 与 JSON 非法
        assert_eq!(json_escape("\0"), r"\u0000");
        // 其他控制字符（0x01）被转义为 \u0001
        assert_eq!(json_escape("\x01"), r"\u0001");
        // 控制字符 0x1F（单元分隔符）被转义为 \u001f
        assert_eq!(json_escape("\x1f"), r"\u001f");
        // 混合控制字符
        assert_eq!(json_escape("a\0b\x01c"), r"a\u0000b\u0001c");
    }

    #[test]
    fn test_json_escape_normal_chars() {
        // 正常 ASCII 字符不被转义
        assert_eq!(json_escape("hello world"), "hello world");
        // 数字与字母混合
        assert_eq!(json_escape("plugin123"), "plugin123");
        // 中文等多字节字符不被转义
        assert_eq!(json_escape("插件"), "插件");
        // 空字符串
        assert_eq!(json_escape(""), "");
    }

    #[test]
    fn test_json_escape_special_chars() {
        // 双引号转义
        assert_eq!(json_escape(r#"""#), r#"\""#);
        // 反斜杠转义
        assert_eq!(json_escape(r"\"), r"\\");
        // 换行符转义
        assert_eq!(json_escape("\n"), r"\n");
        // 回车符转义
        assert_eq!(json_escape("\r"), r"\r");
        // 制表符转义
        assert_eq!(json_escape("\t"), r"\t");
        // 混合特殊字符：输入 "a\"b\\c\n"（即 a " b \ c 换行），
        // 期望输出 a\"b\\c\n（字面字符序列）
        assert_eq!(json_escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
    }

    #[test]
    fn test_json_escape_rfc8259_compliance() {
        // 验证所有控制字符（0x00-0x1F）均被转义，符合 RFC 8259
        for code in 0u32..=0x1F {
            let c = char::from_u32(code).unwrap();
            // 跳过已被显式分支处理的字符（\n \r \t），它们使用短转义形式
            if matches!(c, '\n' | '\r' | '\t') {
                continue;
            }
            let input: String = std::iter::once(c).collect();
            let escaped = json_escape(&input);
            assert!(
                escaped.starts_with("\\u"),
                "控制字符 U+{:04X} 未被转义为 \\uXXXX 形式，得到: {:?}",
                code,
                escaped
            );
        }
    }
}
