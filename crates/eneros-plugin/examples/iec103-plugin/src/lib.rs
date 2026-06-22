//! IEC 60870-5-103 协议适配器示例插件
//!
//! 本 crate 编译为 cdylib 动态库，由 EnerOS 插件加载器通过 C ABI 加载。
//! 元数据通过同目录 `manifest.toml` 提供，故不导出 `eneros_plugin_metadata`。
//!
//! IEC 103 是基于串口的继电保护设备通信协议，本示例为 stub 实现，
//! connect/disconnect/read/write 返回占位数据，用于演示插件接口契约。

use eneros_plugin::protocol::{
    ProtocolAdapterInstance, ProtocolPlugin, ProtocolPluginConfig, PluginDataPoint,
    PluginDataQuality, PluginDataValue,
};
use async_trait::async_trait;
use std::ffi::c_void;
use std::time::{SystemTime, UNIX_EPOCH};

/// IEC 60870-5-103 协议适配器插件
pub struct Iec103Plugin;

#[async_trait]
impl ProtocolPlugin for Iec103Plugin {
    fn protocol_name(&self) -> &str {
        "iec103"
    }

    fn description(&self) -> &str {
        "IEC 60870-5-103 protocol adapter (stub)"
    }

    async fn create_adapter(
        &self,
        _config: &ProtocolPluginConfig,
    ) -> Result<Box<dyn ProtocolAdapterInstance>, eneros_plugin::PluginError> {
        Ok(Box::new(Iec103Adapter {
            connected: false,
            address: _config.address.clone(),
        }))
    }
}

/// IEC 103 适配器实例（stub）
pub struct Iec103Adapter {
    connected: bool,
    address: String,
}

#[async_trait]
impl ProtocolAdapterInstance for Iec103Adapter {
    async fn connect(&mut self) -> Result<(), eneros_plugin::PluginError> {
        // stub：串口连接占位
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), eneros_plugin::PluginError> {
        self.connected = false;
        Ok(())
    }

    async fn read(&self, address: &str) -> Result<PluginDataPoint, eneros_plugin::PluginError> {
        // stub：返回占位遥测数据
        Ok(PluginDataPoint {
            address: address.to_string(),
            value: PluginDataValue::Float32(0.0),
            timestamp: current_unix_millis(),
            quality: if self.connected {
                PluginDataQuality::Good
            } else {
                PluginDataQuality::Offline
            },
        })
    }

    async fn write(
        &mut self,
        _address: &str,
        _value: &PluginDataValue,
    ) -> Result<(), eneros_plugin::PluginError> {
        // stub：IEC 103 一般只读，写操作占位
        Ok(())
    }

    fn name(&self) -> &str {
        "iec103-adapter"
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

impl Iec103Adapter {
    /// 获取连接地址（用于调试）
    pub fn address(&self) -> &str {
        &self.address
    }
}

/// 获取当前 Unix 毫秒时间戳
fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// C ABI 入口：创建插件实例
///
/// 返回堆分配的 `Box<Iec103Plugin>` 裸指针，调用方负责通过
/// `eneros_plugin_destroy` 释放。
///
/// 注意：通过 C ABI 传递的是瘦指针（具体类型），加载器在需要时
/// 可将其包装为 `dyn ProtocolPlugin` trait object。
///
/// # Safety
///
/// 调用方必须保证返回的指针仅通过 `eneros_plugin_destroy` 释放一次，
/// 且在销毁前不得解引用为其他类型。
#[no_mangle]
pub unsafe extern "C" fn eneros_plugin_create() -> *mut c_void {
    let plugin: Box<Iec103Plugin> = Box::new(Iec103Plugin);
    Box::into_raw(plugin) as *mut c_void
}

/// C ABI 入口：销毁插件实例
///
/// 接收 `eneros_plugin_create` 返回的指针并释放其内存。
/// 传入空指针时为空操作。
///
/// # Safety
///
/// `ptr` 必须为 `eneros_plugin_create` 的返回值或空指针，
/// 且同一指针不得销毁超过一次。
#[no_mangle]
pub unsafe extern "C" fn eneros_plugin_destroy(ptr: *mut c_void) {
    if !ptr.is_null() {
        // SAFETY: ptr 由 eneros_plugin_create 通过 Box::into_raw 产生，
        // 调用方保证仅释放一次。
        let _ = Box::from_raw(ptr as *mut Iec103Plugin);
    }
}
