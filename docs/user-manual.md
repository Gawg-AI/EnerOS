# EnerOS 用户手册

本手册面向 EnerOS 的运维人员与最终用户，说明系统的安装、配置、CLI 使用与故障排查方法。

## 1. 安装

EnerOS 支持三种安装方式：源码编译、镜像烧录、Docker 容器。

### 1.1 源码编译

适用于开发环境与定制化部署。

```bash
# 安装 Rust 1.75+
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 克隆代码
git clone <repo-url> eneros && cd eneros

# 编译整个 workspace
cargo build --workspace --release

# 运行
./target/release/eneros-api run --config eneros.toml
```

Linux 平台需要安装系统依赖：

```bash
sudo apt install -y build-essential pkg-config libseccomp-dev libssl-dev
```

### 1.2 镜像烧录

适用于生产部署到边缘设备（x86_64 / aarch64）。

```bash
# 构建系统镜像
cd os/image-builder
./build.sh --target aarch64 --output eneros.img

# 烧录到设备（以 /dev/sdX 为例）
dd if=eneros.img of=/dev/sdX bs=4M status=progress
sync
```

镜像构建流程详见 `os/image-builder/README.md`，包含分区创建、bootloader 安装、rootfs 注入等步骤。

### 1.3 Docker 容器

适用于快速体验与测试环境。

```bash
# 克隆代码
git clone <repo-url> eneros && cd eneros

# 编辑配置
cp eneros.toml eneros.toml.local

# 启动（推荐）
docker compose -f deploy/docker/docker-compose.yml up -d

# 带监控和追踪
docker compose -f deploy/docker/docker-compose.yml --profile monitoring --profile tracing up -d

# 查看日志
docker compose -f deploy/docker/docker-compose.yml logs -f eneros

# 健康检查
curl http://localhost:8080/health
```

详细部署说明见 [部署运维指南](./deployment.md)。

## 2. 配置文件

EnerOS 的配置文件位于 `/etc/eneros/` 目录，主要配置文件如下：

| 文件 | 说明 |
|------|------|
| `eneros.toml` | 主配置文件，API/SCADA/网络/安全/可观测性等 |
| `init.toml` | 系统初始化配置，Agent 启动项、服务编排 |
| `plugin.toml` | 插件系统配置，签名验证、沙箱、加载模式 |
| `ha.toml` | 高可用配置，集群节点、心跳、failover |
| `network.toml` | 网络配置，接口、bonding、防火墙 |
| `timesync.toml` | 时间同步配置，PTP/NTP 时钟源 |
| `syslog.toml` | 系统日志配置，级别、轮转、分类 |
| `audit.toml` | 审计日志配置 |
| `eneros-machine.yaml` | 机器标识配置 |

### 2.1 eneros.toml 主要配置段

```toml
[api]
host = "0.0.0.0"
port = 8080
enable_tls = false

[scada]
source = "simulated"            # 或 "iec104"
fast_interval_ms = 100
normal_interval_ms = 1000
iec104_addr = "127.0.0.1:2404"
iec104_asdu = 1

[powerflow]
tolerance = 1e-6
max_iterations = 50

[security]
enable_auth = true
jwt_secret = "your-secret-key"
jwt_ttl_secs = 3600
enable_audit = true
audit_log_path = "/var/log/eneros/audit.jsonl"

[observability]
log_level = "info"
log_format = "json"
enable_metrics = true
enable_tracing = false
```

支持环境变量覆盖，格式为 `ENEROS_<SECTION>__<FIELD>`：

```bash
ENEROS_API__PORT=9090 ./target/release/eneros-api run
```

配置支持热重载，修改 `eneros.toml` 后 2 秒内自动生效（部分字段需重启，详见 [部署运维指南](./deployment.md)）。

### 2.2 init.toml 主要配置段

`init.toml` 定义系统启动时的 Agent 进程与服务编排，位于 `os/rootfs/files/etc/eneros/init.toml`。包含 Agent 注册项（二进制路径、启动参数、资源配额）与服务依赖关系。

### 2.3 plugin.toml 主要配置段

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
```

### 2.4 ha.toml 主要配置段

```toml
[ha]
node_id = "node-1"
role = "primary"                      # primary / standby
heartbeat_interval_ms = 1000
heartbeat_timeout_ms = 3000

[ha.cluster]
nodes = ["node-1", "node-2"]
vip = "192.168.1.100"

[ha.failover]
auto_failover = true
drill_enabled = true
```

## 3. CLI 使用参考（enerosctl）

`enerosctl` 是 EnerOS 的管理 CLI，通过 TCP 控制通道（127.0.0.1:9876）或本地状态文件与内核交互。只读命令在 TCP 连接失败时回退到读取本地状态文件 `/var/run/eneros/agents.json`。

全局选项：

```
enerosctl [OPTIONS] <COMMAND>

选项：
  --socket <PATH>    IPC 控制 socket 路径（保留，当前使用 TCP）
  -v, --verbose      详细输出（启用 debug 日志）
  -h, --help         帮助
  -V, --version      版本
```

### 3.1 agent — Agent 进程管理

```
enerosctl agent <SUBCOMMAND>

子命令：
  list                列出所有注册的 Agent
  start <agent_id>    启动指定 Agent
  stop <agent_id>     停止指定 Agent
  status <agent_id>   查询指定 Agent 状态
  restart <agent_id>  重启指定 Agent
```

示例：

```bash
enerosctl agent list
enerosctl agent start dispatch-agent
enerosctl agent status forecast-agent
enerosctl agent restart operation-agent
```

### 3.2 eventbus — 事件总线管理

```
enerosctl eventbus <SUBCOMMAND>

子命令：
  status               查询 EventBusBroker 状态
  subscribe [topic]    订阅事件（实时打印，按 Ctrl+C 退出）
```

示例：

```bash
enerosctl eventbus status
enerosctl eventbus subscribe
enerosctl eventbus subscribe agent.command
```

### 3.3 system — 系统信息

```
enerosctl system <SUBCOMMAND>

子命令：
  info    显示系统信息（Agent 数量、状态分布、EventBus 连接状态）
```

### 3.4 network — 网络配置管理

```
enerosctl network <SUBCOMMAND>

子命令：
  status                 显示所有网络接口状态
  config [interface]     显示接口配置
  firewall <SUBCOMMAND>  显示/管理防火墙规则
  bond [interface]       显示 bonding 状态
```

防火墙子命令：

```
enerosctl network firewall <SUBCOMMAND>

子命令：
  list      列出防火墙规则
  add       添加防火墙规则
  delete    删除防火墙规则
```

### 3.5 log — 日志管理

```
enerosctl log <SUBCOMMAND>

子命令：
  tail [category] [-n LINES] [-f] [--json]   查看最近 N 行日志
  search <pattern> [OPTIONS]                 搜索日志
  level <level>                              设置日志级别
  export [OPTIONS]                           导出日志
  rotate                                     轮转日志
```

`tail` 选项：

- `category`：日志分类（system/agent/protocol/security/audit），可选
- `-n, --lines`：显示行数，默认 50
- `-f, --follow`：实时跟踪日志输出
- `--json`：输出原始 JSONL 行

`search` 选项：

- `-c, --category`：日志分类（指定 all 跨分类搜索）
- `-l, --level`：按级别过滤（trace/debug/info/warn/error）
- `--since`：起始时间（ISO 8601 或 YYYY-MM-DD）
- `--until`：结束时间

`export` 选项：

- `--start`：开始时间
- `--end`：结束时间
- `--format`：输出格式（json/text），默认 json
- `-c, --category`：日志分类

示例：

```bash
enerosctl log tail agent -n 100 -f
enerosctl log search "IEC 104" --level warn --since 2026-06-01
enerosctl log level debug
enerosctl log export --start 2026-06-01 --format json
```

### 3.6 device — 设备管理

```
enerosctl device <SUBCOMMAND>

子命令：
  list [-t TYPE]        列出所有设备（按类型过滤：serial/usb/gpio/i2c/spi/net）
  info <device>         显示设备详情
  config <device> [OPTIONS]   配置设备参数
  monitor               实时监控设备状态（按 Ctrl+C 退出）
```

`config` 选项：

- `--preset`：串口预设（iec104_ft12/modbus_rtu/modbus_rtu_high）
- `--baud`：波特率

### 3.7 audit — 审计日志管理

```
enerosctl audit <SUBCOMMAND>

子命令：
  list [OPTIONS]        列出审计日志
  verify                验证审计日志完整性
  search [OPTIONS]      搜索审计日志
```

`list` / `search` 选项：

- `--since`：起始时间
- `--until`：结束时间
- `--limit`：最大返回条数
- `--actor`：按操作者过滤（search）
- `--action`：按动作类型过滤（login/logout/config_change/agent_control/...）（search）
- `--result`：按结果过滤（success/failure/denied）（search）

### 3.8 time — 时间同步管理

```
enerosctl time <SUBCOMMAND>

子命令：
  status                 显示时间同步状态
  set-source <source>    设置时钟源（ptp/ntp/local）
  sync                   立即同步
```

### 3.9 update — OTA 更新管理

```
enerosctl update <SUBCOMMAND>

子命令：
  status              查询当前槽位状态
  apply <bundle>      应用 OTA 更新包
  rollback            回滚到上一已知良好槽位
  list                列出可用的更新包
  gen-keys [--output DIR]   生成 Ed25519 密钥对（默认 /etc/eneros/keys/）
```

EnerOS 采用 A/B 分区 OTA，支持原子切换与回滚。

### 3.10 protocol — 协议适配器管理

```
enerosctl protocol <SUBCOMMAND>

子命令：
  status    显示所有协议适配器状态（支持协议列表 + 传输层能力）
  list      列出已注册协议适配器及配置
  test <protocol> <address>   测试指定协议连通性
```

`test` 参数：

- `protocol`：协议类型（goose/sv/iec104/modbus_tcp/modbus_rtu/mqtt/opcua/dnp3/iec61850）
- `address`：目标地址（IP:Port / 串口设备 / 网卡名）

示例：

```bash
enerosctl protocol status
enerosctl protocol test iec104 192.168.1.100:2404
enerosctl protocol test modbus_rtu /dev/ttyS0
```

### 3.11 security — 安全管理

```
enerosctl security <SUBCOMMAND>

子命令：
  status    显示安全状态汇总（Secure Boot + 内核加固 + seccomp + 审计 + KMS）
  audit <SUBCOMMAND>    审计日志管理
  keys <SUBCOMMAND>     密钥管理
```

`security audit` 子命令：`list` / `search` / `verify`（选项同 audit 命令）。

`security keys` 子命令用于管理 KMS 密钥。

### 3.12 ha — 高可用管理

```
enerosctl ha <SUBCOMMAND>

子命令：
  status                显示 HA 状态（节点角色、心跳、同步、failover）
  nodes                 列出集群节点
  sync-status           显示同步状态
  failover-status       显示 failover 状态（状态机、VIP、上次切换）
  failover-trigger [--force]   手动触发 failover 切换
  failover-history      显示 failover 切换历史
  failover-drill [--scenario SCENARIO]   触发灾备演练
```

`failover-drill` 场景：`primary_down` / `network_partition` / `disk_failure`，默认 `primary_down`。

### 3.13 plugin — 插件管理

```
enerosctl plugin <SUBCOMMAND>

子命令：
  list                       列出已加载的插件
  load <path> [--skip-signature]   加载插件（验证签名 → 加载库 → 初始化 → 启动）
  unload <name>              卸载插件（停止 → 卸载库）
  info <name>                显示插件详情（manifest + state + statistics）
  verify <path> [--sig SIG]  验证插件签名（不加载）
  enable <name>              启用插件
  disable <name>             禁用插件
  gen-keys [--output DIR]    生成插件签名密钥对（Ed25519）
  sign <plugin> <key>        对插件文件签名
```

示例：

```bash
enerosctl plugin list
enerosctl plugin gen-keys --output /etc/eneros/keys/
enerosctl plugin sign ./my_plugin.so /etc/eneros/keys/private.key
enerosctl plugin verify ./my_plugin.so
enerosctl plugin load ./my_plugin.so
enerosctl plugin info my-plugin
enerosctl plugin unload my-plugin
```

### 3.14 shell — 交互式 REPL

```
enerosctl shell
```

启动交互式 shell，支持命令历史、自动补全。在 shell 内可直接输入子命令，无需 `enerosctl` 前缀。

### 3.15 completions — 生成补全脚本

```
enerosctl completions <SHELL>
```

支持的 shell：`bash` / `zsh` / `fish` / `powershell` / `elvish`。

示例：

```bash
# 生成 bash 补全
enerosctl completions bash > /etc/bash_completion.d/enerosctl

# 生成 PowerShell 补全
enerosctl completions powershell | Out-String | Invoke-Expression
```

### 3.16 config — 配置管理

```
enerosctl config <SUBCOMMAND>

子命令：
  get <key>          查看配置项（格式：file.field，如 plugin.require_signature）
  set <key> <value>  设置配置项
  edit <file>        编辑配置文件（file 为不含扩展名的文件名，如 plugin、syslog）
  list               列出所有配置文件
```

示例：

```bash
enerosctl config get plugin.require_signature
enerosctl config set plugin.require_signature true
enerosctl config edit plugin
enerosctl config list
```

### 3.17 service — 服务管理

```
enerosctl service <SUBCOMMAND>

子命令：
  start <name>    启动服务
  stop <name>     停止服务
  restart <name>  重启服务
  status <name>   查询服务状态
  list            列出所有服务
```

服务名称：`eneros-init` / `ha-daemon` / `plugin-daemon` / `eventbus-broker` / `gateway` 等。

### 3.18 doctor — 系统诊断

```
enerosctl doctor
```

执行系统级诊断，检查以下子系统状态：

- 内核与启动参数
- Agent 进程状态
- EventBus 连接
- 设备与协议
- 安全（Secure Boot / seccomp / 审计）
- 时间同步
- 网络配置
- 高可用状态
- 插件系统
- 磁盘与内存

输出各子系统健康状态与异常提示，是故障排查的首选工具。

### 3.19 simulator — 仿真器管理

```
enerosctl simulator <SUBCOMMAND>

子命令：
  run <scenario>          运行场景脚本
  validate <scenario>     验证场景脚本格式
  list-scenarios          列出可用场景
```

场景脚本为 TOML 格式，描述事件时间线与动作参数，详见 [ADR-0004](./adr/0004-simulator-scenario-engine.md)。

示例：

```bash
enerosctl simulator validate ./scenarios/ieee14-line-trip.toml
enerosctl simulator run ./scenarios/ieee14-line-trip.toml
enerosctl simulator list-scenarios
```

## 4. 故障排查

### 4.1 首选工具：enerosctl doctor

遇到问题时首先运行系统诊断：

```bash
enerosctl doctor
```

该命令会检查所有子系统状态并输出异常提示，多数问题可直接定位。

### 4.2 查看日志

```bash
# 查看最近 100 行系统日志
enerosctl log tail system -n 100

# 实时跟踪 Agent 日志
enerosctl log tail agent -f

# 搜索错误日志
enerosctl log search "error" --level error --since 2026-06-01

# 导出日志用于离线分析
enerosctl log export --start 2026-06-01 --format json > logs.json
```

### 4.3 常见问题

#### IEC 104 连接失败

```
[SCADA] WARNING: IEC 104 connection failed: Connection refused
```

排查步骤：

1. 确认 RTU 服务器地址和端口正确
2. 检查网络连通性：`telnet 192.168.1.100 2404`
3. 使用 `enerosctl protocol test iec104 192.168.1.100:2404` 测试
4. 确认 ASDU 地址匹配
5. 查看日志中的 TESTFR 心跳状态：`enerosctl log search "TESTFR"`

#### 潮流计算不收敛

```
[PowerFlow] Failed to converge after 50 iterations
```

排查步骤：

1. 调整 `powerflow.tolerance`（默认 1e-6）
2. 增加 `powerflow.max_iterations`（默认 50）
3. 检查网络模型参数是否合理（变压器变比、线路参数）
4. 使用 `enerosctl config set powerflow.tolerance 1e-4` 动态调整

#### 配置热重载不生效

```bash
# 手动触发重载
curl -X POST http://localhost:8080/api/config/reload

# 查看哪些字段被应用/跳过
curl -X POST http://localhost:8080/api/config/reload | jq
```

注意：`api.host`/`api.port`、`network.source`、`devices`、`security.jwt_secret` 等字段不支持热重载，需重启服务。

#### Agent 无法启动

1. 检查 Agent 列表：`enerosctl agent list`
2. 查看 Agent 日志：`enerosctl log tail agent`
3. 确认 `init.toml` 中 Agent 配置正确
4. 运行 `enerosctl doctor` 检查系统状态
5. 尝试手动启动：`enerosctl agent start <agent_id>`

#### 插件加载失败

1. 验证插件签名：`enerosctl plugin verify <path>`
2. 检查插件 manifest：`enerosctl plugin info <name>`
3. 查看插件日志：`enerosctl log search "plugin"`
4. 确认可信公钥已配置：检查 `/etc/eneros/keys/` 目录
5. 开发环境可使用 `--skip-signature` 跳过签名验证

#### 时间同步异常

1. 查看时间同步状态：`enerosctl time status`
2. 确认时钟源：`enerosctl time set-source ptp`
3. 手动同步：`enerosctl time sync`
4. 检查 PTP/NTP 服务器可达性

### 4.4 健康检查

```bash
# HTTP 健康检查
curl http://localhost:8080/health

# 使用脚本
./deploy/scripts/healthcheck.sh
```

### 4.5 优雅关停

按 `Ctrl+C` 触发优雅关停，系统按以下顺序退出：

1. SCADA 双扫描 pipeline 完成当前采集周期后退出
2. Agent 决策循环停止
3. 设备连接断开（防止 RTU 连接泄漏）
4. 配置文件监听器停止
5. HTTP 服务器关闭

## 5. 相关文档

- [部署运维指南](./deployment.md)
- [开发者指南](./developer-guide.md)
- [插件开发指南](./plugin-development.md)
- [贡献指南](../CONTRIBUTING.md)
- [架构决策记录](./adr/0001-record-architecture-decisions.md)
