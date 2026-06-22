# EnerOS 部署运维指南

## 1. 系统要求

- **OS**: Linux (Ubuntu 22.04+ / Debian 12+ 推荐) 或 Docker
- **Rust**: 1.75+ (仅源码编译需要)
- **内存**: 最低 512MB，推荐 2GB+
- **磁盘**: 最低 1GB（含时序数据存储）
- **网络**: 开放 8080 端口（API），可选 16686（Jaeger UI）、9090（Prometheus）、3000（Grafana）

## 2. 快速开始

### 2.1 Docker Compose 部署（推荐）

```bash
# 克隆代码
git clone <repo-url> eneros && cd eneros

# 编辑配置
cp eneros.toml eneros.toml.local
# 修改 eneros.toml.local 中的配置项...

# 启动
docker compose -f deploy/docker/docker-compose.yml up -d

# 查看日志
docker compose -f deploy/docker/docker-compose.yml logs -f eneros

# 健康检查
curl http://localhost:8080/health
```

### 2.2 带监控和追踪的完整部署

```bash
# 启动核心服务 + Prometheus + Grafana
docker compose -f deploy/docker/docker-compose.yml --profile monitoring up -d

# 启动核心服务 + Jaeger 追踪
docker compose -f deploy/docker/docker-compose.yml --profile tracing up -d

# 全部启动
docker compose -f deploy/docker/docker-compose.yml --profile monitoring --profile tracing up -d
```

### 2.3 源码编译部署

```bash
# 开发模式
./deploy/scripts/dev.sh

# 生产构建
./deploy/scripts/build.sh v0.9.0

# 直接运行 release 二进制
./target/release/eneros-api run --config eneros.toml
```

> **Windows 用户注意**：`deploy/scripts/` 下的 `.sh` 脚本为 bash 脚本，需在 Git Bash 或 WSL 环境下运行。原生 PowerShell 用户可直接使用 `cargo run --package eneros-api -- run --config eneros.toml` 等命令替代。

## 3. 配置管理

### 3.1 配置文件

主配置文件 `eneros.toml`，支持环境变量覆盖（格式：`ENEROS_<SECTION>__<FIELD>`）。

```bash
# 示例：通过环境变量覆盖 API 端口
ENEROS_API__PORT=9090 ./target/release/eneros-api run

# 示例：通过环境变量启用 IEC 104
ENEROS_SCADA__SOURCE=iec104 \
ENEROS_SCADA__IEC104_ADDR=192.168.1.100:2404 \
./target/release/eneros-api run --config eneros.toml
```

### 3.2 配置热重载（v0.9.0）

EnerOS 支持运行时配置热重载，无需重启服务：

- **自动热重载**：修改 `eneros.toml` 后，2 秒内自动生效
- **手动热重载**：`POST /api/config/reload`
- **查看当前配置**：`GET /api/config`（敏感字段已脱敏）

**支持热重载的字段**：
| 字段 | 说明 |
|------|------|
| `observability.log_level` | 日志级别（立即生效） |
| `observability.enable_metrics` | 指标开关 |
| `scada.fast_interval_ms` | 快速扫描间隔（下次 pipeline 重启生效） |
| `scada.normal_interval_ms` | 常规扫描间隔（下次 pipeline 重启生效） |
| `emergency.*` | 紧急阈值参数 |
| `powerflow.tolerance` | 潮流计算容差 |
| `powerflow.max_iterations` | 潮流计算最大迭代次数 |

**不支持热重载的字段**（需重启）：
- `api.host` / `api.port`（绑定地址）
- `api.enable_tls` / `api.tls_cert_path`（TLS 配置）
- `network.source` / `network.path`（电网模型）
- `devices`（设备连接列表）
- `scada.source` / `scada.iec104_addr`（数据源类型）
- `security.jwt_secret`（JWT 密钥）
- `eventbus.max_queue_size`（事件总线容量）

## 4. 可观测性

### 4.1 日志

- **格式**：JSON（默认）或纯文本
- **级别**：`trace` / `debug` / `info` / `warn` / `error`
- **动态调整**：`POST /api/log-level` — 无需重启
- **Span 追踪**：`enable_tracing=true` 时，JSON 日志包含 span 创建/关闭事件

```bash
# 动态修改日志级别
curl -X POST http://localhost:8080/api/log-level \
  -H "Content-Type: application/json" \
  -d '{"level": "debug"}'
```

### 4.2 指标（Prometheus）

```bash
# 启动带 Prometheus 的部署
docker compose -f deploy/docker/docker-compose.yml --profile monitoring up -d

# 访问 Prometheus UI
open http://localhost:9090

# 访问 Grafana（默认 admin/admin）
open http://localhost:3000
```

指标端点：`GET /metrics`（Prometheus 格式）

### 4.3 分布式追踪（v0.9.0）

```toml
[observability]
enable_tracing = true
otel_endpoint = "http://jaeger:4317"
otel_service_name = "eneros"
```

```bash
# 启动带 Jaeger 的部署
docker compose -f deploy/docker/docker-compose.yml --profile tracing up -d

# 访问 Jaeger UI
open http://localhost:16686
```

关键 handler 已添加 `#[tracing::instrument]` 注解，span 上下文贯穿 API → Pipeline → Gateway 链路。

## 5. SCADA 数据采集

### 5.1 双扫描组（DualScanGroup）

EnerOS 使用双扫描组机制分离快速/常规测点：

- **快速组**（默认 100ms）：频率、电压、断路器位置、继电器状态
- **常规组**（默认 1000ms）：功率、温度、设备状态

配置示例：
```toml
[scada]
source = "simulated"          # 或 "iec104"
fast_interval_ms = 100        # 快速组间隔
normal_interval_ms = 1000     # 常规组间隔
iec104_addr = "127.0.0.1:2404"
iec104_asdu = 1
```

### 5.2 IEC 104 连接

```toml
[scada]
source = "iec104"
iec104_addr = "192.168.1.100:2404"
iec104_asdu = 1
```

连接特性：
- 自动 TESTFR 心跳保活
- 非活跃状态时保留最后有效缓存（瞬态断连保护）
- 优雅断开（Ctrl+C 时自动断开所有设备连接）

## 6. 安全

### 6.1 认证

- **JWT**：HS256 签名，默认 TTL 3600 秒
- **API Key**：支持多密钥配置
- **RBAC**：4 种角色（Supervisor / Operator / Analyst / Viewer）

```toml
[security]
enable_auth = true
jwt_secret = "your-secret-key-here"
jwt_ttl_secs = 3600
api_keys = ["key1", "key2"]
```

### 6.2 TLS

```toml
[api]
enable_tls = true
tls_cert_path = "/path/to/cert.pem"
tls_key_path = "/path/to/key.pem"
```

### 6.3 审计日志

```toml
[security]
enable_audit = true
audit_log_path = "/var/log/eneros/audit.jsonl"
```

## 7. 运维操作

### 7.1 健康检查

```bash
# 基本健康检查
curl http://localhost:8080/health

# 使用脚本
./deploy/scripts/healthcheck.sh
```

### 7.2 优雅关停

按 `Ctrl+C` 触发优雅关停：
1. SCADA 双扫描 pipeline 完成当前采集周期后退出
2. Agent 决策循环停止
3. 设备连接断开（防止 RTU 连接泄漏）
4. 配置文件监听器停止
5. HTTP 服务器关闭

### 7.3 备份

```bash
# 备份时序数据
docker exec eneros-api tar czf - /var/lib/eneros/timeseries > timeseries-backup.tar.gz

# 备份 Agent 记忆
docker exec eneros-api tar czf - /var/lib/eneros/memory > memory-backup.tar.gz

# 备份配置
cp eneros.toml eneros.toml.backup.$(date +%Y%m%d)
```

## 8. 故障排查

### 8.1 IEC 104 连接失败

```
[SCADA] WARNING: IEC 104 connection failed: Connection refused
```

**排查步骤**：
1. 确认 RTU 服务器地址和端口正确
2. 检查网络连通性：`telnet 192.168.1.100 2404`
3. 确认 ASDU 地址匹配
4. 查看日志中的 TESTFR 心跳状态

### 8.2 潮流计算不收敛

```
[PowerFlow] Failed to converge after 50 iterations
```

**排查步骤**：
1. 调整 `powerflow.tolerance`（默认 1e-6）
2. 增加 `powerflow.max_iterations`（默认 50）
3. 检查网络模型参数是否合理（特别是变压器变比和线路参数）

### 8.3 配置热重载不生效

```bash
# 手动触发重载
curl -X POST http://localhost:8080/api/config/reload

# 查看哪些字段被应用/跳过
curl http://localhost:8080/api/config/reload -X POST | jq
```

## 9. 版本管理

EnerOS 遵循语义化版本 2.0.0：
- **MAJOR**：不兼容的 API 变更
- **MINOR**：向后兼容的功能新增
- **PATCH**：向后兼容的缺陷修复

变更记录见 `CHANGELOG.md`，未来规划见 `ROADMAP.md`。

## 10. plugin-daemon 部署

plugin-daemon 是 v0.28.0 引入的插件宿主进程，以进程隔离方式加载第三方插件，崩溃不影响主进程。

### 10.1 安装

plugin-daemon 包含在 eneros-plugin crate 中，编译 workspace 时自动构建：

```bash
cargo build --release -p plugin-daemon
```

产物：`target/release/plugin-daemon`

镜像部署时，plugin-daemon 已包含在 rootfs 中，二进制位于 `/usr/bin/plugin-daemon`。

### 10.2 配置

plugin-daemon 的配置位于 `/etc/eneros/plugin.toml`：

```toml
[plugin]
require_signature = true              # 生产环境必须为 true
default_mode = "daemon"               # daemon（默认）或 inline
plugin_dir = "/var/lib/eneros/plugins"
keys_dir = "/etc/eneros/keys"

[plugin.sandbox]
enable_seccomp = true
enable_quota = true
default_cpu_percent = 50
default_memory_mb = 256
allowed_paths = ["/var/lib/eneros/data"]
denied_paths = ["/etc/shadow"]
allowed_network = ["tcp:2404"]
```

| 配置项 | 说明 |
|--------|------|
| `require_signature` | 是否强制签名验证（生产环境必须 true） |
| `default_mode` | 默认加载模式（daemon / inline） |
| `plugin_dir` | 插件动态库目录 |
| `keys_dir` | 可信公钥目录 |
| `enable_seccomp` | 启用 seccomp 沙箱 |
| `enable_quota` | 启用 cgroups 资源配额 |
| `default_cpu_percent` | 默认 CPU 上限 |
| `default_memory_mb` | 默认内存上限 |

### 10.3 启动

```bash
# 通过 enerosctl 启动
enerosctl service start plugin-daemon

# 或直接运行
plugin-daemon --config /etc/eneros/plugin.toml

# 查看状态
enerosctl service status plugin-daemon
```

plugin-daemon 启动后会监听 IPC 通道，等待主进程的加载请求。

### 10.4 日志

plugin-daemon 的日志输出到 syslog 的 `protocol` 分类：

```bash
# 查看 plugin-daemon 日志
enerosctl log tail protocol -f

# 搜索插件加载错误
enerosctl log search "plugin" --level error
```

### 10.5 插件管理

```bash
# 列出已加载插件
enerosctl plugin list

# 加载插件
enerosctl plugin load /var/lib/eneros/plugins/libmy_plugin.so

# 卸载插件
enerosctl plugin unload my-plugin

# 查看插件详情
enerosctl plugin info my-plugin
```

详细插件开发与部署流程见 [插件开发指南](./plugin-development.md)。

## 11. 模拟器部署

EnerOS 仿真器（eneros-simulator）用于验证 Agent 决策、测试保护逻辑、回归测试分析模块，支持 TOML 场景脚本描述时序事件。

### 11.1 eneros-simulator crate

仿真器作为 eneros-simulator crate 提供，编译 workspace 时自动构建：

```bash
cargo build --release -p eneros-simulator
```

仿真器核心能力：

- **场景脚本引擎**：TOML 描述事件时间线（故障注入、负荷变化、跳闸等）
- **电网模型**：支持 IEEE 标准网络与自定义拓扑
- **故障注入**：短路、接地、发电机/线路跳闸
- **观察记录**：在指定时间点记录系统状态快照

### 11.2 场景脚本

场景脚本为 TOML 格式，包含场景元数据、事件时间线与初始状态：

```toml
name = "ieee14-line-trip"
description = "IEEE 14 节点系统线路跳闸场景"
duration = 60.0
time_step = 0.1

[[timeline]]
time = 0.0
action = { type = "observe" }
params = { label = "steady_state" }

[[timeline]]
time = 10.0
action = { type = "line_trip" }
params = { line = "L1-2" }

[[timeline]]
time = 30.0
action = { type = "observe" }
params = { label = "post_fault" }

[initial_state]
load_level = 0.8
```

支持的动作类型：

| 动作 | 说明 |
|------|------|
| `inject_fault` | 注入故障 |
| `clear_fault` | 清除故障 |
| `load_change` | 负荷变化 |
| `generator_trip` | 发电机跳闸 |
| `line_trip` | 线路跳闸 |
| `load_shed` | 负荷切除 |
| `observe` | 观察记录点 |

场景脚本格式详见 [ADR-0004](./adr/0004-simulator-scenario-engine.md)。

### 11.3 enerosctl simulator 命令

```bash
# 验证场景脚本格式
enerosctl simulator validate ./scenarios/ieee14-line-trip.toml

# 运行场景
enerosctl simulator run ./scenarios/ieee14-line-trip.toml

# 列出可用场景
enerosctl simulator list-scenarios
```

### 11.4 部署建议

- 仿真器主要用于开发、测试与验证环境，生产部署通常不需要
- 场景脚本建议纳入版本控制，便于回归测试
- 复杂场景可拆分为多个脚本，通过 `initial_state` 参数化复用

## 12. SDK 应用打包

eneros-sdk 是面向第三方开发者的 SDK，封装 Agent/协议/插件开发的常用类型与辅助函数。

### 12.1 eneros-sdk 依赖

在应用项目的 `Cargo.toml` 中添加 SDK 依赖：

```toml
[dependencies]
eneros-sdk = { path = "../eneros/crates/eneros-sdk", features = ["full"] }
```

SDK 通过 feature 门控按需启用模块：

| feature | 启用模块 | 用途 |
|---------|----------|------|
| `agent` | agent | Agent 开发 |
| `protocol` | protocol | 协议开发 |
| `plugin` | plugin | 插件开发 |
| `full`（默认） | 全部 | 全部模块 |

### 12.2 编译

```bash
# 编译 SDK 应用
cargo build --release

# 交叉编译（aarch64 目标）
cargo build --release --target aarch64-unknown-linux-gnu
```

### 12.3 分发

SDK 应用的分发方式取决于应用类型：

| 应用类型 | 产物 | 分发方式 |
|----------|------|----------|
| 插件 | 动态库（.so/.dll/.dylib） | 签名后部署到 `/var/lib/eneros/plugins/` |
| Agent | 可执行文件 | 部署到 `/usr/bin/`，在 `init.toml` 中注册 |
| 独立工具 | 可执行文件 | 按需部署 |

插件分发需附带：

- 动态库文件
- `manifest.toml` 清单
- `.sig` 签名文件
- 公钥（首次部署时添加到 `/etc/eneros/keys/`）

详细打包与签名流程见 [插件开发指南 — 插件市场发布流程](./plugin-development.md#9-插件市场发布流程)。
