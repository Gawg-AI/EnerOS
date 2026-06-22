//! plugin-daemon IPC 客户端 — 连接独立守护进程加载插件（v0.28.0 Task 11）
//!
//! v0.28.0 将插件加载从同进程（Inline）改为独立 daemon 进程（Daemon），
//! 实现崩溃隔离：插件 panic 不会影响主进程。本模块提供 IPC 客户端，
//! 通过 JSON 行协议与 `plugin-daemon` 通信。
//!
//! # 协议
//!
//! 每行一个 JSON 请求/响应（以 `\n` 分隔）。
//!
//! 请求（`DaemonRequest`，内部标签枚举，`cmd` 字段区分命令）：
//! ```json
//! {"cmd": "load", "path": "/path/to/plugin.so", "skip_signature": false}
//! {"cmd": "unload", "name": "my-plugin"}
//! {"cmd": "list"}
//! {"cmd": "status"}
//! ```
//!
//! 响应（`DaemonResponse`）：
//! ```json
//! {"ok": true, "data": {...}}
//! {"ok": false, "error": "error message"}
//! ```
//!
//! # 传输层
//!
//! - Linux：Unix socket（默认 `/var/run/eneros/plugin-daemon.sock`）
//! - 跨平台回退：TCP `127.0.0.1:5410`
//!
//! # 示例
//!
//! ```no_run
//! use eneros_plugin::ipc::PluginDaemonClient;
//!
//! let client = PluginDaemonClient::new(PluginDaemonClient::default_addr());
//! match client.load("/var/lib/eneros/plugins/iec103/iec103.so", false) {
//!     Ok(resp) if resp.ok => println!("加载成功: {:?}", resp.data),
//!     Ok(resp) => eprintln!("加载失败: {:?}", resp.error),
//!     Err(e) => eprintln!("IPC 错误: {}", e),
//! }
//! ```

use crate::error::{PluginError, PluginResult};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[cfg(not(unix))]
use std::net::TcpStream;

/// IPC 请求命令（内部标签枚举，`cmd` 字段区分命令类型）
///
/// v0.28.0 Task 12：统一定义于 `eneros-plugin::ipc`，daemon 端直接复用，
/// 消除原先 `DaemonCommand`（daemon）与 `DaemonRequest`（client）的重复定义，
/// 通过往返测试保证两端序列化/反序列化兼容。
///
/// 序列化使用 `rename_all = "lowercase"` 保证 `Load` → `"load"` 等映射。
/// `Load.skip_signature` 标注 `#[serde(default)]`，允许客户端省略该字段
/// （默认 `false`），与 daemon 端历史行为保持兼容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum DaemonRequest {
    /// 加载插件
    Load {
        /// 插件动态库路径
        path: String,
        /// 是否跳过签名验证（受 daemon 配置 `allow_skip_signature` 限制）
        #[serde(default)]
        skip_signature: bool,
    },
    /// 卸载插件
    Unload {
        /// 插件名称
        name: String,
    },
    /// 列出已加载插件
    List,
    /// 查询插件信息
    Info {
        /// 插件名称
        name: String,
    },
    /// 启用插件
    Enable {
        /// 插件名称
        name: String,
    },
    /// 禁用插件
    Disable {
        /// 插件名称
        name: String,
    },
    /// 验证插件签名
    Verify {
        /// 插件动态库路径
        path: String,
    },
    /// 查询 daemon 状态
    Status,
}

/// IPC 响应
///
/// 成功时 `data` 存在、`error` 为 `None`；失败时 `error` 存在、`data` 为 `None`。
/// 反序列化时缺失的 `Option` 字段自动为 `None`，与 daemon 端 `skip_serializing_if` 配合。
///
/// v0.28.0 Task 12：统一定义于 `eneros-plugin::ipc`，daemon 端直接复用，
/// 消除原先在 daemon `main.rs` 中的重复定义。`ok()` / `error()` 构造方法
/// 供 daemon 端构造响应，client 端通过反序列化消费。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonResponse {
    /// 是否成功
    pub ok: bool,
    /// 成功时的响应数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// 失败时的错误信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DaemonResponse {
    /// 构造成功响应
    ///
    /// 将任意可序列化数据封装为 `data` 字段。序列化失败时回退为 `Value::Null`
    /// （理论上不会发生，仅作防御性处理）。
    pub fn ok<T: Serialize>(data: T) -> Self {
        Self {
            ok: true,
            data: Some(serde_json::to_value(data).unwrap_or(serde_json::Value::Null)),
            error: None,
        }
    }

    /// 构造错误响应
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// plugin-daemon IPC 客户端
///
/// 连接独立运行的 `plugin-daemon` 进程，通过 JSON 行协议发送命令并接收响应。
/// 客户端为无状态设计，每次请求建立新连接（简化实现，适合低频管理操作）。
pub struct PluginDaemonClient {
    /// IPC 地址（Unix socket 路径或 TCP 地址）
    addr: String,
    /// 读写超时
    timeout: Duration,
}

impl PluginDaemonClient {
    /// 创建客户端（不立即连接）
    ///
    /// `addr` 为 Unix socket 路径（以 `/` 开头，仅 Linux）或 TCP 地址（`host:port`）。
    pub fn new(addr: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            timeout: Duration::from_secs(5),
        }
    }

    /// 设置读写超时（链式调用）
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 跨平台默认 IPC 地址
    ///
    /// - Linux：`/var/run/eneros/plugin-daemon.sock`（Unix socket）
    /// - 其他平台：`127.0.0.1:5410`（TCP 回退）
    pub fn default_addr() -> &'static str {
        #[cfg(unix)]
        {
            "/var/run/eneros/plugin-daemon.sock"
        }
        #[cfg(not(unix))]
        {
            "127.0.0.1:5410"
        }
    }

    /// 发送请求并接收响应
    ///
    /// 序列化请求为 JSON → 连接 daemon → 发送（以 `\n` 结尾）→ 读取一行响应 → 反序列化。
    /// 序列化/IO 错误通过 `PluginError::Serialization`/`PluginError::Io` 返回。
    fn send_request(&self, request: &DaemonRequest) -> PluginResult<DaemonResponse> {
        let json = serde_json::to_string(request)?;
        let response = self.connect_and_send(&json)?;
        let resp: DaemonResponse = serde_json::from_str(&response)?;
        Ok(resp)
    }

    /// 连接 daemon 并发送单行 JSON 请求，返回单行 JSON 响应
    ///
    /// Unix socket 为本机通信，`connect` 通常立即返回（ENOENT/ECONNREFUSED），
    /// 不存在 TCP 远程连接的长时间阻塞问题，故保留 `connect` 不加超时。
    #[cfg(unix)]
    fn connect_and_send(&self, json: &str) -> PluginResult<String> {
        let mut stream = UnixStream::connect(&self.addr)?;
        // 设置读写超时失败时记录警告（而非静默吞掉），便于排查超时配置问题
        if let Err(e) = stream.set_read_timeout(Some(self.timeout)) {
            tracing::warn!("IPC 设置读超时失败 '{}': {}", self.addr, e);
        }
        if let Err(e) = stream.set_write_timeout(Some(self.timeout)) {
            tracing::warn!("IPC 设置写超时失败 '{}': {}", self.addr, e);
        }
        stream.write_all(json.as_bytes())?;
        stream.write_all(b"\n")?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        // read_line 返回 0 表示 daemon 关闭了连接，应返回 IO 错误而非让上层解析空字符串
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(PluginError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "daemon 关闭了连接",
            )));
        }
        Ok(line.trim().to_string())
    }

    /// 连接 daemon 并发送单行 JSON 请求，返回单行 JSON 响应
    ///
    /// 使用 `connect_timeout` 限制连接阶段超时，避免 daemon 不可达时阻塞 60+ 秒。
    #[cfg(not(unix))]
    fn connect_and_send(&self, json: &str) -> PluginResult<String> {
        // 解析为 SocketAddr 以使用 connect_timeout（避免连接阶段无限阻塞）
        let socket_addr: std::net::SocketAddr = self.addr.parse().map_err(|e: std::net::AddrParseError| {
            PluginError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("IPC 地址解析失败 '{}': {}", self.addr, e),
            ))
        })?;
        let mut stream = TcpStream::connect_timeout(&socket_addr, self.timeout)?;
        // 设置读写超时失败时记录警告（而非静默吞掉），便于排查超时配置问题
        if let Err(e) = stream.set_read_timeout(Some(self.timeout)) {
            tracing::warn!("IPC 设置读超时失败 '{}': {}", self.addr, e);
        }
        if let Err(e) = stream.set_write_timeout(Some(self.timeout)) {
            tracing::warn!("IPC 设置写超时失败 '{}': {}", self.addr, e);
        }
        stream.write_all(json.as_bytes())?;
        stream.write_all(b"\n")?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        // read_line 返回 0 表示 daemon 关闭了连接，应返回 IO 错误而非让上层解析空字符串
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(PluginError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "daemon 关闭了连接",
            )));
        }
        Ok(line.trim().to_string())
    }

    /// 加载插件
    ///
    /// 请求 daemon 加载指定路径的插件动态库，可选跳过签名验证。
    pub fn load(&self, path: &str, skip_signature: bool) -> PluginResult<DaemonResponse> {
        self.send_request(&DaemonRequest::Load {
            path: path.to_string(),
            skip_signature,
        })
    }

    /// 卸载插件
    ///
    /// 请求 daemon 卸载指定名称的插件，释放动态库句柄。
    pub fn unload(&self, name: &str) -> PluginResult<DaemonResponse> {
        self.send_request(&DaemonRequest::Unload {
            name: name.to_string(),
        })
    }

    /// 列出已加载插件
    ///
    /// 返回 daemon 中所有已注册插件的列表。
    pub fn list(&self) -> PluginResult<DaemonResponse> {
        self.send_request(&DaemonRequest::List)
    }

    /// 查询插件信息
    ///
    /// 返回指定名称插件的详细信息（版本、状态、启用标志等）。
    pub fn info(&self, name: &str) -> PluginResult<DaemonResponse> {
        self.send_request(&DaemonRequest::Info {
            name: name.to_string(),
        })
    }

    /// 启用插件
    ///
    /// 将指定插件标记为启用状态。
    pub fn enable(&self, name: &str) -> PluginResult<DaemonResponse> {
        self.send_request(&DaemonRequest::Enable {
            name: name.to_string(),
        })
    }

    /// 禁用插件
    ///
    /// 将指定插件标记为禁用状态。
    pub fn disable(&self, name: &str) -> PluginResult<DaemonResponse> {
        self.send_request(&DaemonRequest::Disable {
            name: name.to_string(),
        })
    }

    /// 验证插件签名
    ///
    /// 请求 daemon 验证指定路径插件的签名，不加载插件。
    pub fn verify(&self, path: &str) -> PluginResult<DaemonResponse> {
        self.send_request(&DaemonRequest::Verify {
            path: path.to_string(),
        })
    }

    /// 查询 daemon 状态
    ///
    /// 返回 daemon 运行状态与已加载/已启用插件数量。
    pub fn status(&self) -> PluginResult<DaemonResponse> {
        self.send_request(&DaemonRequest::Status)
    }

    /// 检查 daemon 是否可达
    ///
    /// 发送 `status` 请求，成功返回 `true`，连接失败或超时返回 `false`。
    ///
    /// 使用线程 + 通道 + `recv_timeout(3s)` 实现整体超时控制：
    /// 即使 `connect_and_send` 因 daemon 不可达而阻塞（TCP 连接阶段），
    /// 也能在 3 秒内返回 `false`，避免调用方长时间等待。
    pub fn is_reachable(&self) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let addr = self.addr.clone();
        let timeout = self.timeout;
        std::thread::spawn(move || {
            // 在子线程中执行 status 请求，通过通道回传结果
            let client = PluginDaemonClient { addr, timeout };
            let _ = tx.send(client.status().is_ok());
        });
        // 3 秒内未收到结果视为不可达，返回 false
        rx.recv_timeout(Duration::from_secs(3)).unwrap_or(false)
    }
}

impl Default for PluginDaemonClient {
    fn default() -> Self {
        Self::new(Self::default_addr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 测试 1：创建 PluginDaemonClient 并验证地址与默认超时
    #[test]
    fn test_daemon_client_new() {
        let client = PluginDaemonClient::new("127.0.0.1:5410");
        assert_eq!(client.addr, "127.0.0.1:5410");
        assert_eq!(client.timeout, Duration::from_secs(5));
    }

    /// 测试 2：验证默认地址（Linux 为 Unix socket，其他平台为 TCP）
    #[test]
    fn test_daemon_client_default_addr() {
        let addr = PluginDaemonClient::default_addr();
        #[cfg(unix)]
        assert_eq!(addr, "/var/run/eneros/plugin-daemon.sock");
        #[cfg(not(unix))]
        assert_eq!(addr, "127.0.0.1:5410");
    }

    /// 测试 3：设置自定义超时
    #[test]
    fn test_daemon_client_with_timeout() {
        let client = PluginDaemonClient::new("127.0.0.1:5410")
            .with_timeout(Duration::from_secs(10));
        assert_eq!(client.timeout, Duration::from_secs(10));
    }

    /// 测试 4：序列化 load 请求，验证 cmd/path/skip_signature 字段
    #[test]
    fn test_daemon_request_load_serialize() {
        let req = DaemonRequest::Load {
            path: "/var/lib/eneros/plugins/iec103.so".to_string(),
            skip_signature: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"cmd\":\"load\""));
        assert!(json.contains("\"path\":\"/var/lib/eneros/plugins/iec103.so\""));
        assert!(json.contains("\"skip_signature\":false"));
    }

    /// 测试 5：序列化 unload 请求，验证 cmd/name 字段
    #[test]
    fn test_daemon_request_unload_serialize() {
        let req = DaemonRequest::Unload {
            name: "iec103-driver".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"cmd\":\"unload\""));
        assert!(json.contains("\"name\":\"iec103-driver\""));
    }

    /// 测试 6：序列化 list 请求，验证仅包含 cmd 字段
    #[test]
    fn test_daemon_request_list_serialize() {
        let req = DaemonRequest::List;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, "{\"cmd\":\"list\"}");
    }

    /// 测试 7：反序列化成功响应，验证 ok/data 字段
    #[test]
    fn test_daemon_response_ok_deserialize() {
        let json = r#"{"ok":true,"data":{"name":"iec103","version":"1.0.0"}}"#;
        let resp: DaemonResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert!(resp.error.is_none());
        let data = resp.data.unwrap();
        assert_eq!(data["name"], "iec103");
        assert_eq!(data["version"], "1.0.0");
    }

    /// 测试 8：反序列化错误响应，验证 ok/error 字段
    #[test]
    fn test_daemon_response_error_deserialize() {
        let json = r#"{"ok":false,"error":"plugin not loaded: unknown"}"#;
        let resp: DaemonResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.ok);
        assert!(resp.data.is_none());
        assert_eq!(resp.error.unwrap(), "plugin not loaded: unknown");
    }

    /// 测试 9：连接超时 — 连接不可达地址应在合理时间内返回错误（而非阻塞 60 秒）
    ///
    /// 验证 H1 修复：TCP 使用 `connect_timeout`，避免 daemon 不可达时阻塞 60+ 秒。
    /// Unix socket 连接不存在的路径立即返回 ENOENT，不存在阻塞问题。
    #[test]
    fn test_connect_timeout() {
        // 选择平台相关的不可达地址
        #[cfg(unix)]
        let addr = "/var/run/eneros/nonexistent-test-socket-e70e3f.sock";
        #[cfg(not(unix))]
        let addr = "192.0.2.1:1"; // TEST-NET-1（RFC 5737）不可路由地址

        let client = PluginDaemonClient::new(addr).with_timeout(Duration::from_secs(1));
        let start = std::time::Instant::now();
        let result = client.status();
        let elapsed = start.elapsed();

        assert!(result.is_err(), "连接不可达地址应返回错误");
        // 应在合理时间内返回（远小于 OS 默认的 60+ 秒连接超时）
        assert!(
            elapsed.as_secs() < 30,
            "应在 30 秒内返回，实际耗时: {:?}",
            elapsed
        );
    }

    /// 测试 10：read_line 返回 0 时应返回 IO 错误（连接关闭）而非解析错误
    ///
    /// 验证 H3 修复：daemon 关闭连接时 `read_line` 返回 `Ok(0)`，
    /// 客户端应返回 `PluginError::Io(UnexpectedEof)` 而非让上层解析空字符串报序列化错误。
    #[test]
    fn test_read_line_eof_returns_connection_closed() {
        use crate::error::PluginError;

        // 启动模拟 daemon：接受连接 → 读取请求 → 关闭连接（不发送任何响应）
        // 客户端 read_line 应返回 0，触发 UnexpectedEof 错误
        #[cfg(not(unix))]
        {
            use std::net::TcpListener;

            let listener = TcpListener::bind("127.0.0.1:0").expect("绑定 TCP 失败");
            let port = listener.local_addr().unwrap().port();

            std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    // 读取客户端请求后关闭连接，不发送任何响应
                    let mut buf = [0u8; 1024];
                    let _ = std::io::Read::read(&mut stream, &mut buf);
                    drop(stream);
                }
            });

            // 等待监听就绪
            std::thread::sleep(Duration::from_millis(100));

            let client = PluginDaemonClient::new(format!("127.0.0.1:{}", port))
                .with_timeout(Duration::from_secs(5));
            let result = client.status();

            assert!(result.is_err(), "daemon 关闭连接应返回错误");
            let err = result.unwrap_err();
            // 应返回 IO 错误（UnexpectedEof），而非序列化错误（解析空字符串）
            assert!(
                matches!(err, PluginError::Io(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof),
                "期望 IO 错误（UnexpectedEof），实际: {:?}",
                err
            );
        }

        #[cfg(unix)]
        {
            use std::os::unix::net::UnixListener;

            let sock_path = std::env::temp_dir().join(format!(
                "eneros-test-ipc-eof-{}-{}.sock",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let _ = std::fs::remove_file(&sock_path);

            let listener = UnixListener::bind(&sock_path).expect("绑定 Unix socket 失败");
            let path_clone = sock_path.clone();

            std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 1024];
                    let _ = std::io::Read::read(&mut stream, &mut buf);
                    drop(stream);
                }
                let _ = std::fs::remove_file(&path_clone);
            });

            std::thread::sleep(Duration::from_millis(100));

            let client =
                PluginDaemonClient::new(sock_path.to_string_lossy().to_string())
                    .with_timeout(Duration::from_secs(5));
            let result = client.status();

            assert!(result.is_err(), "daemon 关闭连接应返回错误");
            let err = result.unwrap_err();
            assert!(
                matches!(err, PluginError::Io(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof),
                "期望 IO 错误（UnexpectedEof），实际: {:?}",
                err
            );
        }
    }

    /// 测试 11：DaemonRequest 所有变体的序列化→反序列化往返测试
    ///
    /// v0.28.0 Task 12：验证 client 端序列化的请求能被 daemon 端反序列化
    /// （两端共用同一类型定义，往返后必须保持相等）。
    /// 覆盖全部 8 个变体：Load/Unload/List/Info/Enable/Disable/Verify/Status，
    /// 其中 Load 分别测试 `skip_signature=true` 与 `false` 两种取值。
    #[test]
    fn test_request_serialization_roundtrip() {
        let cases = vec![
            DaemonRequest::Load {
                path: "/var/lib/eneros/plugins/iec103.so".to_string(),
                skip_signature: false,
            },
            DaemonRequest::Load {
                path: "/tmp/test.so".to_string(),
                skip_signature: true,
            },
            DaemonRequest::Unload {
                name: "iec103-driver".to_string(),
            },
            DaemonRequest::List,
            DaemonRequest::Info {
                name: "modbus-rtu".to_string(),
            },
            DaemonRequest::Enable {
                name: "modbus-tcp".to_string(),
            },
            DaemonRequest::Disable {
                name: "iec104".to_string(),
            },
            DaemonRequest::Verify {
                path: "/var/lib/eneros/plugins/iec104.so".to_string(),
            },
            DaemonRequest::Status,
        ];

        for original in cases {
            let json = serde_json::to_string(&original)
                .unwrap_or_else(|_| panic!("序列化失败: {:?}", original));
            let roundtrip: DaemonRequest = serde_json::from_str(&json)
                .unwrap_or_else(|_| panic!("反序列化失败: {}", json));
            assert_eq!(
                original, roundtrip,
                "往返后值不一致，原始: {:?}, 往返: {:?}, JSON: {}",
                original, roundtrip, json
            );
        }
    }

    /// 测试 12：DaemonResponse 的序列化→反序列化往返测试
    ///
    /// v0.28.0 Task 12：验证 daemon 端序列化的响应能被 client 端反序列化
    /// （两端共用同一类型定义，往返后必须保持相等）。
    /// 覆盖成功响应（带 JSON 对象数据 / null 数据）与错误响应。
    ///
    /// 注意：`DaemonResponse::ok(Value::Null)` 构造 `data: Some(Null)`，序列化为
    /// `{"ok":true,"data":null}`，但 serde 将 JSON `null` 反序列化为 `None`
    /// （而非 `Some(Null)`），因此 null 数据用例验证关键字段而非完全相等。
    #[test]
    fn test_response_serialization_roundtrip() {
        // 成功响应：带 JSON 对象数据
        let resp_ok_obj = DaemonResponse::ok(serde_json::json!({
            "name": "iec103",
            "version": "1.0.0",
            "enabled": true
        }));
        let json = serde_json::to_string(&resp_ok_obj).unwrap();
        let roundtrip: DaemonResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp_ok_obj, roundtrip);
        assert!(roundtrip.ok);
        assert!(roundtrip.error.is_none());

        // 成功响应：null 数据
        // serde 将 JSON `null` 反序列化为 `None`，因此往返后 `data` 为 `None`
        let resp_ok_null = DaemonResponse::ok(serde_json::Value::Null);
        let json = serde_json::to_string(&resp_ok_null).unwrap();
        let roundtrip: DaemonResponse = serde_json::from_str(&json).unwrap();
        assert!(roundtrip.ok);
        assert!(roundtrip.error.is_none());
        assert!(roundtrip.data.is_none());

        // 错误响应
        let resp_err = DaemonResponse::error("插件未加载: unknown");
        let json = serde_json::to_string(&resp_err).unwrap();
        let roundtrip: DaemonResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp_err, roundtrip);
        assert!(!roundtrip.ok);
        assert!(roundtrip.data.is_none());
        assert_eq!(roundtrip.error.unwrap(), "插件未加载: unknown");
    }
}
