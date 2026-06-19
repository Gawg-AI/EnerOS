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
