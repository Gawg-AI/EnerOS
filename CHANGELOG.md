# EnerOS 变更日志

本项目版本号遵循 [语义化版本 2.0.0](https://semver.org/lang/zh-CN/)。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)。

---

## [0.22.0] - 2026-06-19

### v0.22.0 部署与 OTA 更新（Deployment & OTA Updates）

> 让 EnerOS 从"能构建镜像"升级为"能安全部署 + 能远程 OTA 更新"。实现 A/B 分区原子更新、Ed25519 签名验证、声明式机器配置、eneros-imager v2 五分区布局、eneros-installer 交互式安装器 + PXE 配置生成、enerosctl update 子命令、eneros-init 启动成功检测与自动回滚。

#### 任务 1：ab_partition.rs 扩展 — 持久化 + boot count + health 状态

- **SlotStatus 扩展**：Active / Inactive / Trying / Good / Failed（新增 Trying/Good，移除 Unknown）
- **AbPartition 新增字段**：boot_count_a、boot_count_b、last_boot、last_update、state_file（#[serde(skip)]）
- **持久化**：load_from_file / save_to_file，槽位状态可保存到 /etc/eneros/slot-state.json，重启后恢复
- **switch_slot 自动持久化**：best-effort save_to_file，失败仅日志不传播
- **健康状态方法**：mark_trying（boot_count +1）、mark_good（重置 boot_count）、mark_failed、last_good_slot
- **容错**：文件不存在或损坏时默认 Slot A=Active+Good, Slot B=Inactive
- 新增 9 个测试

#### 任务 2：manifest.rs + signer.rs — Ed25519 签名更新包

- **UpdateManifest 结构体**：version / target_slot / image_version / images: Vec<ImageEntry> / created_at / signature
- **ImageEntry**：name / sha256 / size
- **signing_payload()**：用 \x1f 分隔符拼接字段（参考 audit.rs 防注入模式）
- **signer.rs**：SigningKey / VerifyingKey 封装 ed25519-dalek v2
  - generate_keypair()：平台特定随机源（Linux /dev/urandom，Windows RtlGenRandom FFI）
  - sign_manifest() / verify_manifest()
  - save/load 密钥文件（base64，Linux 0600 权限）
- **UpdateError 枚举**：Io / Config / SignatureFailed / HashMismatch / UnsupportedPlatform / BundleInvalid / SlotError / Serialize / HttpDownload / Key
- 新增 8 个测试（3 manifest + 5 signer）

#### 任务 3：ota.rs — OtaManager 完整 OTA 流程

- **OtaManager 结构体**：config: OtaConfig + ab_partition: AbPartition
- **download_bundle()**：reqwest::blocking HTTP 下载 .eneros-update 到 /data/updates/（临时文件 + rename 原子操作）
- **verify_bundle()**：解压 tar.gz → 读取 manifest.json → 验证 Ed25519 签名 → 验证每个 image SHA256
- **write_to_slot()**（Linux）：dd rootfs.img 到 /dev/sda2 或 /dev/sda3 + 复制 vmlinuz/initramfs.img 到 EFI 分区
- **switch_slot()**（Linux）：更新 GRUB grubenv next_slot + ab_partition.switch_slot + save
- **apply()**：完整流程编排 download → verify → write_to_slot(inactive) → switch_slot
- **rollback()**：切换到 last_good_slot
- **list_updates()**：列出 /data/updates/ 中的 .eneros-update
- **平台隔离**：download/verify/list 跨平台；write_to_slot/switch_slot/apply 为 Linux 特定，非 Linux 返回 UnsupportedPlatform
- 新增 7 个测试（update 模块共 45 个测试）

#### 任务 4：machine_config.rs — 声明式机器配置

- **MachineConfig 结构体**（serde_yaml）：hardware / partitions / network / boot / agents
  - HardwareSpec：arch / cpu_cores / memory_mb / disk_device
  - PartitionLayout：efi_size_mb / root_size_mb / data_size_mb / config_size_mb
  - NetworkSpec：hostname / interfaces: Vec<InterfaceConfig>
  - BootSpec：kernel_params / rt_config: RtConfig
  - agents: Vec<AgentSpec>（Agent 启用 + 资源配额 + 权限）
- **方法**：load_from_yaml / save_to_yaml / validate / generate_init_config（TOML）/ generate_network_config（TOML）/ generate_kernel_cmdline（RT 参数）
- **示例文件**：os/rootfs/files/etc/eneros/eneros-machine.yaml（含完整注释）
- 新增 17 个测试

#### 任务 5：eneros-imager v2 — 5 分区 A/B 布局 + 配置注入

- **create-partitions.sh**：5 分区布局（EFI 512MB FAT32 + RootA 1.5GB ext4 + RootB 1.5GB ext4 + Data 剩余 ext4 + Config 256MB ext4）
- **build.sh**：新增 --machine-config 参数，调用 inject-config.sh；RootA=Active/RootB=空；Config 分区写入 slot-state.json + eneros-machine.yaml + keys/；Data 分区创建 /data/updates/
- **inject-config.sh**（新建）：读取 eneros-machine.yaml，生成 init.toml + network.toml 注入 rootfs
- **grub.cfg**：3 菜单项（Slot A root=/dev/sda2 / Slot B root=/dev/sda3 / Recovery），加载 grubenv，next_slot 选择默认，boot_count >= 3 自动回退
- **grubenv**（新建）：1024 字节 GRUB 环境块，next_slot=A, boot_count=0
- **README.md**：5 分区布局说明 + A/B OTA 流程文档

#### 任务 6：eneros-installer 二进制 — 交互式 CLI + PXE 配置生成

- **新建 crates/eneros-os/bins/eneros-installer**：依赖 eneros-os + clap + tracing + serde_yaml + anyhow
- **CLI 参数**：--disk / --image / --machine-config / --generate-pxe / --output / --yes
- **cmd_install**（Linux）：10 步安装流程（列出磁盘 → 确认 → sgdisk 分区 → mkfs → 挂载 → dd/tar 写入镜像 → grub-install → 注入配置 → 创建 /data/updates/ → 卸载）
- **cmd_generate_pxe**（跨平台）：生成 pxelinux.cfg/default + dhcpd.conf 片段
- **GRUB 配置生成**：generate_grubenv()（1024 字节）+ generate_grub_cfg(&MachineConfig)
- **平台隔离**：main.rs 全文 #[cfg(target_os = "linux")]，非 Linux 编译为空 main + eprintln
- 新增 7 个测试

#### 任务 7：enerosctl update 子命令 + boot success detection

- **enerosctl Update 子命令**：Status / Apply / Rollback / List / GenKeys
  - cmd_update_status：表格输出槽位状态
  - cmd_update_apply：调用 OtaManager::apply()
  - cmd_update_rollback：调用 OtaManager::rollback()
  - cmd_update_list：列出可用更新
  - cmd_update_gen_keys：生成 Ed25519 密钥对
  - 全部 #[cfg(target_os = "linux")] 门控
- **eneros-init boot success detection**：
  - mark_boot_trying()：读取 ENEROS_BOOT_SLOT，加载 AbPartition，mark_trying（boot_count +1），boot_count > 3 → mark_failed + trigger_rollback
  - check_boot_success()：60 秒定时器 + 看门狗 keepalive（每 5 秒），服务就绪 → mark_good，服务失败 → mark_failed + trigger_rollback
  - trigger_rollback()：切换到 last_good_slot

#### 验证结果

- `cargo build --workspace` — 0 编译错误（含新增 eneros-installer 二进制）
- `cargo test -p eneros-os --lib` — 303 通过，0 失败（v0.22.0 新增 42 个测试）
- `cargo clippy -p eneros-os --all-targets` — 0 新警告（修复 3 个 "field assignment outside of initializer" 警告）
- `cargo clippy -p enerosctl --all-targets` — 0 新警告
- `cargo clippy -p eneros-installer --all-targets` — 0 新警告

#### 交付级修复（Delivery-Grade Hardening）

> 对 OTA 模块进行深度审计后修复 12 个严重问题，使其达到生产交付级质量。

**ab_partition.rs 修复**：
- **switch_slot 保留 Good 状态**：旧槽位从 `Inactive` 改为 `Good`（保留为回滚目标），修复 OTA 后回滚失效的致命 bug
- **新增 switch_to_trying 方法**：OTA 切换时新槽位设为 `Trying`（非 `Active`），旧槽位设为 `Good`，确保回滚目标可用
- 新增 2 个测试（test_switch_to_trying + test_ota_rollback_scenario）

**ota.rs 修复**：
- **流式 SHA256 校验**：`verify_bundle` 从 `std::fs::read`（全量读入内存）改为 `BufReader` + 64KB 缓冲区流式计算，避免 1.5GB rootfs.img 导致 OOM
- **镜像大小校验**：SHA256 校验前用 `metadata().len()` 对比 manifest 声明大小，防止截断/篡改
- **消除双重解压**：`verify_bundle` 返回 `(manifest, temp_dir)`，`write_to_slot` 接收解压目录而非重新解压，提升效率并消除一致性风险
- **块设备安全写入**：`write_to_slot` 从 `std::fs::copy` 改为 `OpenOptions::write(true)` + `std::io::copy` + `sync_all()`，确保数据落盘
- **GRUB grubenv 1024 字节格式**：`update_grubenv` 用 `#` 填充至 1024 字节 + `truncate(1024)`，修复 GRUB `load_env` 失败
- **grubenv 路径修正**：`/EFI/ENEROS/grubenv` → `/boot/efi/EFI/ENEROS/grubenv`（匹配 fstab 挂载点）
- **boot_count 重置**：切换槽位时 grubenv 中 `boot_count` 重置为 0
- **rollback 更新 GRUB**：`rollback` 现在同时更新 GRUB grubenv，修复回滚后 GRUB 仍启动失败槽位的问题
- **switch_to_trying 集成**：`OtaManager::switch_slot` 调用 `switch_to_trying` 而非 `switch_slot`
- **apply 校验 target_slot**：验证 manifest 声明的目标槽位与非活跃槽位匹配
- **下载安全**：`download_bundle` 新增 5 分钟超时 + 失败时清理 .tmp 文件 + 下载前清理残留 .tmp + `sync_all` 确保落盘

**signer.rs 修复**：
- **generate_keypair 返回 Result**：从 `.expect()` panic 改为 `Result` 返回，避免生产环境崩溃

**build.sh 修复**：
- **slot-state.json 格式匹配**：从自定义嵌套格式改为匹配 `AbPartition` serde 格式（`active_slot`/`slot_a_status`/`slot_b_status`/`boot_count_a`/`boot_count_b`/`last_boot`/`last_update`），修复 `load_from_file` 解析失败回退默认值的问题

---

## [0.20.2] - 2026-06-19

### v0.20.2 v0.20.0 功能完整性修复（Functional Completeness Fix）

> 修复 v0.20.0 时间同步、系统日志、审计日志三大模块的功能性缺陷，让功能真正可用。经四路并行深度审计发现 95 个功能性问题（16 Critical + 25 High），本次修复全部 Critical 和 High 级问题。

#### 任务 1+2：timesync.rs 核心修复 — 二进制 + 后台守护 + PTP 状态检测

- **新增 `eneros-timesync` 二进制**：加载配置 → apply() → 后台守护循环，SIGTERM/SIGINT 优雅退出
- **后台守护循环**：PTP 模式 try_wait 监控子进程 + 崩溃重启（指数退避 2s→30s）；NTP 模式按 poll_interval_secs 周期同步
- **pmc 轮询**：解析 `GET TIME_STATUS_NP` 获取 master_offset/port_state，`GET PARENT_DATASET` 获取 grandmasterIdentity；port_state == SLAVE 且 offset 稳定时 locked = true
- **phc2sys 修复**：添加 `-w` 等待 ptp4l 锁定，避免在 PHC 未校准时拉偏系统时钟
- **PTP 配置文件**：生成 /etc/ptp4l.conf（含 time_stamping hardware/software、接口段），ptp4l 用 `-f` 启动
- **Drop trait**：TimeSyncManager 退出时 kill + wait 子进程，避免孤儿
- **status 并发安全**：parking_lot::RwLock 保护，新增 last_error 字段
- **NTP 重试**：单服务器 3 次重试再切换；Transmit Timestamp 填充发送时刻
- **settimeofday 修复**：大偏差分支用 absolute_time 直接设置，精度无损
- **跨平台**：discover_phc/run_daemon/poll_ptp_status 非 Linux stub
- 新增 12 个测试（共 22 个 timesync 测试）

#### 任务 3+4：syslog.rs 线程安全 + 持久性 + 轮转 + 转发修复

- **线程安全**：LogWriter/LogForwarder/SyslogManager 内部 parking_lot::Mutex，log() 改为 &self
- **BufWriter 常驻**：HashMap<LogCategory, BufWriter<File>> 替代每次 open/close
- **fsync 策略**：ERROR 级别立即 sync_data，Audit 类强制 sync_all，其他按计数/时间 flush
- **TLS fail-fast**：配置加载阶段拒绝 TLS（非运行时静默丢日志）
- **按天轮转修复**：current_date 改为 HashMap 按分类独立跟踪，修复跨分类串扰 bug
- **max_files 清理**：按天数清理后再按 max_files 保留最新 N 个
- **gzip 失败处理**：检查 ExitStatus，失败时保留原文件并 tracing::warn
- **retry_interval 自动重传**：后台定时器周期调用 retry_cached
- **reload 热重载**：&mut self 方法，就地更新 config 保留 cache
- **retry_cached 毒丸修复**：失败条目移到队尾，不阻塞后续
- **缓存满加权保留**：ERROR/SECURITY/AUDIT 优先，DEBUG/INFO 丢弃新条目
- **RFC 5424 修复**：APP-NAME 放 source，PROCID 用 PID；SD-PARAM 转义 ] 字符
- **配置校验**：category_levels 未知 key 返回 Config 错误
- 新增 5 个测试（共 27 个 syslog 测试）

#### 任务 5+6：audit.rs 核心修复 — log() 参数 + 轮转 + 链式哈希 + 查询 + 签名

- **log() 参数修复**：增加 source_ip: Option<&str> 和 detail: &str 参数
- **审计日志轮转**：按 max_size_bytes 轮转为 audit.log.YYYYMMDD_HHMMSS，cleanup_old_files 真正生效
- **fsync 持久化**：每条审计记录 sync_all
- **recover_max_seq 修复**：读取失败返回错误而非静默返回 0
- **链式哈希**：AuditEntry 增加 prev_hash 字段，verify_integrity 检测 seq 间隙 + 链式哈希一致性
- **IntegrityViolation 结构**：返回 seq/line_number/violation_type/detail 详细信息
- **线程安全**：parking_lot::Mutex 保护，log()/query()/verify_integrity() 改为 &self
- **query 7 维过滤**：start/end/action/actor/result/target/limit
- **AuditAction 扩展**：新增 CommandExec 和 DataAccess 变体
- **签名分隔符修复**：用 \x1f（Unit Separator）替代 | 防注入
- **常量时间签名比较**：hmac::Mac::verify_slice
- **schema_version 字段**：AuditEntry 增加 schema_version: u32（默认 1）
- 新增 8 个测试（共 16 个 audit 测试）

#### 任务 7+8：enerosctl log/audit/time 命令修复与新增

- **log level 修复**：通过修改配置文件 + SIGHUP 通知 eneros-init 重载，真正生效（替代写死文件）
- **log level get**：无 level 参数时查询当前级别
- **log tail --follow**：实时跟踪模式（tokio::select! + ctrl_c）
- **log search 过滤**：--level/--since/--until/--source 选项 + --category all 跨分类搜索
- **log rotate 命令**：手动触发日志轮转
- **log export --output**：导出到文件 + BufReader 流式处理 + 时间戳严格过滤
- **log tail/search --json**：输出原始 JSONL
- **audit list/verify/search 子命令**：调用 AuditLogger API，专用格式化
- **time status/set-source/sync 子命令**：调用 TimeSyncManager API
- 文件不存在友好提示 + 帮助文本修正

#### 验证结果

- `cargo build --workspace` — 0 编译错误（含新增 eneros-timesync 二进制）
- `cargo test -p eneros-os --lib` — 261 通过，0 失败（v0.20.2 新增 25 个测试）
- `cargo clippy -p eneros-os --all-targets` — 0 新警告（10 个预存警告均在未修改的模块）
- `cargo clippy -p enerosctl --all-targets` — 0 警告

---

## [0.21.0] - 2026-06-19

### v0.21.0 设备管理与 HAL（Device Management & HAL）

> 实现完整的设备管理和硬件抽象层，支持电力设备热插拔、串口通信、USB/GPIO/I2C/SPI 设备接口。

#### 任务 1：devmgr 设备管理服务扩展

- 扩展 `DeviceType` 枚举：新增 Serial/Gpio/I2c/Spi 设备类型
- 新增 `DeviceStatus`（Online/Offline/Error）+ `DeviceInfo` 结构体，设备状态跟踪
- 新增设备枚举方法：`list_serial_devices`/`list_usb_devices`/`list_gpio_devices`/`list_i2c_devices`/`list_spi_devices`/`list_all_devices`
- 新增 `DeviceConfig`/`DeviceRule` 设备配置持久化（TOML）
- uevent 事件处理时自动更新设备状态
- 新增 11 个测试（共 17 个 devmgr 测试）

#### 任务 2：HAL 硬件抽象层完整实现

- **termios 串口配置**：`LinuxSerialPort::configure()` 完整实现——支持 8 种标准波特率（9600-921600）、CS5-CS8 数据位、1/2 停止位、None/Even/Odd 校验、None/Hardware(CRTSCTS)/Software(IXON|IXOFF) 流控、VMIN/VTIME 超时
- **串口超时**：`SerialConfig` 新增 `timeout_ms` 字段，`read()` 超时返回 `HalError::Timeout`
- **HAL trait 扩展**：新增 `GpioPin`/`I2cDevice`/`SpiDevice` trait + `GpioDirection`/`GpioEdge`/`SpiConfig` 类型
- **LinuxHal 实现**：GPIO（sysfs）、I2C（/dev/i2c-* + ioctl I2C_SLAVE）、SPI（/dev/spidev* + ioctl SPI_IOC_MESSAGE）
- 新增 10 个测试

#### 任务 3：串口设备管理（serial_mgr.rs）

- **串口配置模板**：`SerialPreset`（Iec104Ft12=9600/8/N/1、ModbusRtu=9600/8/E/1、ModbusRtuHigh=115200/8/N/1）
- **串口独占访问**：`SerialAccessControl`（Linux flock LOCK_EX|LOCK_NB）
- **串口故障检测**：`SerialMonitor`（错误计数 3→Degraded、10→Failed，成功重置 Healthy）
- 新增 13 个测试

#### 任务 4：USB 设备管理（usb_mgr.rs）

- **USB 白名单**：`UsbWhitelist`/`UsbWhitelistRule`（TOML 持久化、大小写不敏感匹配）
- **USB 串口适配器扫描**：`list_usb_serial_adapters()`（Linux 扫描 /sys/bus/usb/devices/）
- **USB 设备授权**：`authorize_usb_device()`（Linux 写 sysfs authorized 文件）
- 新增 9 个测试

#### 任务 5：GPIO 设备接口

- **GPIO 事件监听**：`GpioEventMonitor`（Linux sysfs poll POLLPRI，阻塞/超时两种模式）
- **GPIO 事件分发**：`GpioEventDispatcher`（跨平台回调机制）
- 新增 1 个测试

#### 任务 6：I2C/SPI 设备接口 + 传感器框架（sensor.rs）

- **传感器驱动框架**：`SensorDriver` trait + `SensorManager` + `SensorReading`/`SensorType`
- **LM75 I2C 温度传感器驱动**：寄存器 0x00，高 9 位有符号温度，分辨率 0.5°C
- **MCP3008 SPI ADC 驱动**：3 字节时序，10 位 ADC 值，3.3V 参考电压
- 新增 9 个测试（含 mock I2C/SPI 设备）

#### 任务 7：enerosctl device 子命令

- `enerosctl device list [--type <type>]`：列出所有设备（表格输出，按类型过滤）
- `enerosctl device info <device>`：显示设备详情（串口锁定/健康状态、USB 白名单状态）
- `enerosctl device config <device> [--preset <preset>] [--baud <rate>]`：配置设备参数
- `enerosctl device monitor`：实时监控设备状态（2 秒刷新，Ctrl+C 退出）

#### 验证

- `cargo build --workspace` — ✅ 0 错误
- `cargo test -p eneros-os --lib` — ✅ 236 passed; 0 failed（v0.21.0 新增 53 个测试）
- `cargo clippy -p eneros-os --all-targets` — ✅ 0 v0.21.0 新增警告（8 个既有警告不变）

---

## [0.20.1] - 2026-06-19

### v0.20.0 安全与正确性修复

> 对 v0.20.0 时间同步与日志模块进行深度代码审查后的修复，覆盖审计签名绕过、PTP 孤儿进程、NTP 崩溃、日志轮转错误、CLI 路径遍历等 Critical/High 级问题。

#### audit.rs — 审计日志安全修复

- **[A-1 Critical] 签名绕过修复**：`source_ip`/`detail` 纳入 HMAC 签名 payload，移除 `with_source_ip`/`with_detail` 方法（构造后不可修改已签名字段）。新增 `test_audit_entry_tamper_source_ip_detail` 测试验证篡改检测。
- **[A-2 Critical] 空密钥拒绝**：`AuditLogger::new` 对空 `hmac_secret` 返回错误，防止任何人伪造审计条目。
- **[A-3 High] seq 持久化恢复**：`log()` 首次调用时从 `audit.log` 扫描恢复 max seq，保证重启后序列号单调递增、防重放。
- **[A-4 High] 完整性校验增强**：`verify_integrity` 对不可解析行计入损坏列表（seq=0 标记），不再静默跳过。

#### timesync.rs — 时间同步正确性修复

- **[T-1 Critical] PTP 孤儿进程修复**：`TimeSyncManager` 保留 `ptp4l_child`/`phc2sys_child` 句柄，`start_ptp` 前先 kill 旧进程；PTP 启动后不立即标记 `locked=true`（需 pmc 轮询确认）。
- **[T-2 Critical] ptp4l 参数修正**：移除错误的 `-d <phc_device>`（`-d` 是 debug level），域号改用 `-D`（linuxptp 标准）。
- **[T-3 High] NTP 响应校验**：校验 response mode=4（server）、stratum≠0（Kiss-o'-Death）、transmit timestamp 非零，过滤 stray UDP 包。
- **[T-4 High] Duration 减法 panic 修复**：`recv_time - send_time` 改用 `saturating_sub`，防止时钟回拨时 panic。
- **[T-5 High] 负 tv_usec 归一化**：`apply_clock_offset` 对负 offset 归一化 `tv_usec` 到 `[0, 1_000_000)`，检查 `adjtime`/`settimeofday` 返回值并报错（CAP_SYS_TIME 缺失不再静默失败）。

#### syslog.rs — 日志系统修复

- **[S-1 Critical] 轮转大小修复**：`maybe_rotate` 用 `std::fs::metadata` 获取真实文件大小，替代跨分类累加的 `current_size`（protocol 流量不再误触发 system 轮转）。
- **[S-2 Critical] TLS 明文修复**：`Transport::Tls` 返回明确错误而非降级为明文 TCP，消除安全/审计日志明文泄露风险。
- **[S-3 High] RFC 5424 转义**：SD-PARAM 值转义 `"`→`\"`、`\`→`\\`；消息中 `\n` 替换为空格，避免 TCP 帧拆分。
- **[S-4 High] 多目标转发**：`forward` 失败后继续尝试剩余目标（主备日志服务器场景），仅缓存一次。
- **[S-5 High] 缓存元数据保留**：`retry_cached` 保留原始 `LogEntry`（含 level/category/source/message），不再降级为 `Info/System/"cached"`。

#### enerosctl log — CLI 修复

- **[Critical] 路径遍历防护**：`resolve_log_file` 校验 category 白名单（system/agent/protocol/security/audit），拒绝 `../` 注入。
- **[Critical] audit 路径对齐**：`--category audit` 指向 `/var/log/eneros/audit/audit.log`（与 audit.rs 实际写入路径一致）。
- **[High] grep 参数注入防护**：`grep -e <pattern> -- <file>`，`-e` 强制 pattern 为搜索模式，`--` 终止选项解析。
- **[High] grep 错误码检查**：退出码 2（错误）返回明确错误，不再误报为"无匹配"。
- **[Medium] format 参数校验**：`--format` 限制为 json/text，无效格式直接报错。
- **[Medium] 输出格式统一**：`format_log_line` 统一输出 `timestamp [level] [category] source — message`，三处命令复用。
- **[Medium] target 参数校验**：`log level` 校验 target 为 global 或合法分类名。
- **[Low] parse_time 移除不必要 cfg 门控**：纯函数无需 `#[cfg(target_os = "linux")]`。
- **[Low] tail 文档修正**："实时查看日志（tail -f）" → "查看最近 N 行日志"。

#### 验证

- `cargo build --workspace` — ✅ 0 错误
- `cargo test -p eneros-os --lib` — ✅ 187 测试通过（新增 1 个 source_ip/detail 篡改检测测试）
- `cargo clippy -p eneros-os --all-targets` — ✅ 0 v0.20.1 新增警告（8 个既有警告不变）

---

## [0.20.0] - 2026-06-19

### OS 系统服务：时间同步与日志（Time Sync & Logging）

> **目标**：实现精确时间同步（PTP < 100μs）和结构化日志系统
> **前置条件**：v0.19.0 网络配置完成

### 变更内容

#### Task 1+2：timesync 时间同步服务 + PTP 时钟管理

- **`crates/eneros-os/src/init/timesync.rs`**（新建，~460 行）：
  - `ClockSource` 枚举（Ptp/Ntp/LocalClock），优先级排序
  - `PtpConfig`（interface/domain/phc_device/hardware_timestamping）、`NtpConfig`（servers/poll_interval）、`TimeSyncConfig`（对应 `/etc/eneros/timesync.toml`）
  - `TimeSyncManager::apply()` Linux 下按优先级启动 PTP（ptp4l + phc2sys）或 NTP 同步
  - 自研 NTPv4 客户端（UDP 端口 123，解析 NTP 时间戳，计算偏差，adjtime/settimeofday 修正）
  - PHC 设备发现（扫描 `/sys/class/ptp/`），grandmaster ID 读取
  - 时间偏差监控（`check_offset_alert()`，阈值可配置，默认 1ms）
  - 配置热重载（`reload(path)`）
  - 11 个单元测试

#### Task 3+4：syslog 结构化日志 + 远程转发

- **`crates/eneros-os/src/init/syslog.rs`**（新建，~840 行）：
  - `LogLevel`（Trace/Debug/Info/Warn/Error）+ `LogCategory`（System/Agent/Protocol/Security/Audit）
  - `LogEntry` 结构化 JSON 日志条目（timestamp/level/category/source/message/fields）
  - `to_jsonl()` JSON 行序列化 + `to_rfc5424()` RFC 5424 格式转换
  - `LogWriter`：按分类分文件写入 + 轮转（Size/Daily/Both）+ gzip 压缩 + 过期清理（retention_days）
  - `LogForwarder`：RFC 5424 远程转发（TCP/TLS/UDP）+ 多目标 + 本地缓存（网络中断时 VecDeque 缓存）+ 重传
  - `SyslogManager`：组合写入器 + 转发器，动态级别调整（`set_global_level`/`set_category_level`）
  - 16 个单元测试

#### Task 5：审计日志增强

- **`crates/eneros-os/src/init/audit.rs`**（新建，~470 行）：
  - `AuditEntry` 带 HMAC-SHA256 签名（签名覆盖 seq/timestamp/action/actor/target/result）
  - `AuditAction` 枚举（Login/Logout/ConfigChange/AgentControl/PermissionChange/Update/Emergency/Other）
  - `AuditResult`（Success/Failure/Denied）
  - `AuditLogger::log()` 写入独立审计日志目录（`/var/log/eneros/audit/`）
  - 签名验证（`verify()`）+ 完整性校验（`verify_integrity()` 检测篡改）
  - 查询 API（`query()` 按时间范围 + 操作类型过滤）
  - 365 天保留 + 过期清理
  - 11 个单元测试

#### Task 6：enerosctl log 子命令

- **`crates/eneros-os/bins/enerosctl/src/main.rs`**（修改）：
  - `Commands` 枚举新增 `Log` 变体
  - `LogCommands` 枚举（Tail/Search/Level/Export）
  - `Commands::Log` match 分发
- **`crates/eneros-os/bins/enerosctl/src/commands.rs`**（修改）：
  - 4 个 Linux 专属 async 函数：`cmd_log_tail`（tail -n + JSON 解析格式化）、`cmd_log_search`（grep + 格式化）、`cmd_log_level`（写入控制文件）、`cmd_log_export`（时间范围过滤 + JSON/text 格式导出）
  - 4 个非 Linux stub 函数
  - `parse_time()` 辅助函数（ISO 8601 + YYYY-MM-DD 解析）

#### Task 7：编译+测试+clippy 验证

- `cargo build --workspace` — ✅ 通过（0 error）
- `cargo test -p eneros-os --lib` — ✅ 186 测试全部通过（含 39 个 v0.20.0 新增测试：timesync 11 + syslog 16 + audit 12）
- `cargo clippy -p eneros-os --all-targets` — ✅ 0 v0.20.0 新增警告（剩余 8 个均为既有代码）

### 模块接线

- **`crates/eneros-os/src/init/mod.rs`**（修改）：声明并导出 `timesync`/`syslog`/`audit` 三个新模块
- **`crates/eneros-os/Cargo.toml`**（修改）：添加 `hmac`/`sha2` 依赖（审计日志签名）

### 配置文件

- **`os/rootfs/files/etc/eneros/timesync.toml`**（新建）：PTP/NTP 时间同步配置（bond0 接口 + 域 0 + 硬件时间戳 + NTP 服务器列表）
- **`os/rootfs/files/etc/eneros/syslog.toml`**（新建）：syslog 配置（100MB/按天轮转 + 7 天保留 + gzip + 分类级别覆盖）

### 验证结果

- `cargo build --workspace` — ✅ 通过（0 error）
- `cargo test -p eneros-os --lib` — ✅ 186 passed; 0 failed
- `cargo clippy -p eneros-os --all-targets` — ✅ 0 v0.20.0 新增警告

---

## [0.19.0] - 2026-06-19

### OS 系统服务：网络配置（Network Configuration Service）

> **目标**：实现完整的网络配置服务，无 NetworkManager 依赖，支持电力通信网络需求。
> **前置条件**：v0.18.0 实时双执行域完成

### 变更内容

#### Task 1：netcfg 网络配置服务

- **`crates/eneros-os/src/init/netcfg.rs`**（新建，~655 行）：
  - `NetworkConfig`/`InterfaceConfig`/`IpConfig`（Static/Dhcp 枚举）/`BondConfig`/`BondMode`（ActiveBackup/Lacp/BalanceTlb）/`BridgeConfig`/`VlanConfig`/`DnsConfig`/`NetworkInterface`/`InterfaceType`/`BondStatus`/`NetworkError`
  - 静态 IP 配置（IPv4/IPv6）、VLAN（802.1Q）、网桥（bridge）
  - `NetworkConfig::load(path)` 解析 `/etc/eneros/network.toml`
  - `NetworkConfig::apply()` Linux 下调用 `ip` 命令应用配置（顺序：bonds → VLANs → bridges → interfaces → DNS）
  - `NetworkConfig::reload(path)` 支持 SIGHUP 触发热重载
  - `NetworkInterface::list()`/`get(name)` 读取 `/sys/class/net/` 枚举接口
  - `BondStatus::list()` 读取 `/proc/net/bonding/` 查询 bonding 状态
  - 11 个单元测试覆盖配置解析、序列化、DHCP、bond 模式、VLAN、DNS、非 Linux 平台 stub
- **`os/rootfs/files/etc/eneros/network.toml`**（新建）：电力通信网络配置示例（eth0 管理 + bond0 active-backup + VLAN 10 GOOSE + VLAN 20 SV + br0 网桥 + DNS）

#### Task 2：nftables 防火墙

- **`crates/eneros-os/src/init/firewall.rs`**（新建，~349 行）：
  - `FirewallError`/`FirewallRule`/`RuleDirection`（Input/Output）/`Protocol`（Tcp/Udp）/`Action`（Accept/Drop，默认 Drop）/`FirewallConfig`/`FirewallManager`
  - 默认安全策略：入站允许 TCP 22（SSH）/102（IEC 61850 MMS）/2404（IEC 104）/9876（EventBus）；出站允许 UDP 123（NTP）/319（PTP event）/320（PTP general）/514（syslog）；默认策略 Drop
  - `FirewallManager::load(path)`/`with_default_policy()`/`apply()`（Linux 下 `nft -f -`）/`save(path)`/`add_rule()`/`to_nftables_conf()`
  - 5 个单元测试覆盖默认策略端口、nftables 配置生成、序列化、添加规则、默认 Drop
- **`os/rootfs/files/etc/eneros/nftables.conf`**（新建）：nftables 规则集（input/output 链 + 默认 drop + IEC 104/61850/SSH/EventBus 入站规则 + NTP/PTP/syslog 出站规则）

#### Task 3：网络 bonding 与链路聚合

- **`crates/eneros-os/src/init/netcfg.rs`**：
  - `BondMode` 枚举支持 ActiveBackup/802.3ad LACP/BalanceTlb
  - `BondConfig` 含 `miimon_ms`（默认 100ms MII 监控）、`primary` 主接口
  - `apply_bond()` 写 `/sys/class/net/<bond>/bonding/mode` 和 `/sys/class/net/<bond>/bonding/miimon`
  - `BondStatus::list()` 解析 `/proc/net/bonding/<bond>` 获取活跃从接口与故障切换信息

#### Task 4：网络命名空间隔离

- **`crates/eneros-os/src/agentos/ipc.rs`**（修改，新增 ~180 行 + 测试）：
  - 新增 `NamespaceError`/`NetworkNamespaceConfig`/`NetworkNamespaceManager`
  - 8 个方法：`create`/`delete`/`create_veth_pair`/`attach_to_bridge`/`configure_ip`/`setup_agent_namespace`/`list`/`exists`
  - 所有 Linux 操作通过 `std::process::Command::new("ip")` 调用
  - 4 个单元测试覆盖序列化、create/exists/list 在非 Linux 平台的 stub 行为
- **`crates/eneros-os/src/agentos/mod.rs`**（修改）：导出 `NetworkNamespaceConfig`/`NetworkNamespaceManager`/`NamespaceError`

#### Task 5：DNS 配置与解析

- **`crates/eneros-os/src/init/netcfg.rs`**：
  - `DnsConfig` 含 `servers`（多 DNS 服务器故障切换）、`search` 域
  - `apply_dns()` 写 `/etc/resolv.conf`
  - `NetworkConfig::apply()` 末尾自动应用 DNS 配置

#### Task 6：网络热插拔支持

- **`crates/eneros-os/src/init/devmgr.rs`**（新建，~328 行）：
  - `DeviceError`/`DeviceType`（Net/Block/Usb/Unknown）/`HotplugAction`（Add/Remove/Change）/`HotplugEvent`/`DeviceManager`
  - Linux 下通过 `libc::socket(AF_NETLINK, SOCK_RAW, NETLINK_KOBJECT_UEVENT)` 监听 uevent
  - 解析 NULL 分隔的 KEY=VALUE 格式，识别 SUBSYSTEM=net/usb/block
  - `list_net_interfaces()` 读取 `/sys/class/net/`
  - 8 个单元测试覆盖序列化、DeviceManager、Linux 专属 `parse_uevent` 测试

#### Task 7：enerosctl network 子命令

- **`crates/eneros-os/bins/enerosctl/Cargo.toml`**（修改）：添加 `toml = { workspace = true }`
- **`crates/eneros-os/bins/enerosctl/src/main.rs`**（修改）：
  - `Commands` 枚举新增 `Network` 变体
  - 新增 `NetworkCommands` 枚举（Status/Config/Firewall/Bond）和 `FirewallCommands` 枚举（List/Policy）
  - 新增 `Commands::Network` match 分发到 `commands::cmd_network_*` 函数
- **`crates/eneros-os/bins/enerosctl/src/commands.rs`**（修改）：
  - 新增 `pad_right`/`format_table` 辅助函数（`#[cfg(target_os = "linux")]` 门控）
  - 5 个 Linux 专属 async 函数：`cmd_network_status`/`cmd_network_config`/`cmd_network_firewall_list`/`cmd_network_firewall_policy`/`cmd_network_bond_status`
  - 5 个非 Linux stub 函数返回 `Err(anyhow!("Network commands require Linux"))`

#### Task 8：编译+测试+clippy 验证

- `cargo build --workspace` — ✅ 通过（0 error）
- `cargo test -p eneros-os` — ✅ 147 测试全部通过（含 28 个 v0.19.0 新增测试：netcfg 11 + firewall 5 + devmgr 8 + namespace 4）
- `cargo clippy --workspace --all-targets` — ✅ 0 error（warning 均为既有代码）

### 模块接线

- **`crates/eneros-os/src/init/mod.rs`**（修改）：声明并导出 `netcfg`/`firewall`/`devmgr` 三个新模块

### 验证结果

- `cargo build --workspace` — ✅ 通过（0 error）
- `cargo test -p eneros-os` — ✅ 147 passed; 0 failed
- `cargo clippy --workspace --all-targets` — ✅ 0 error（warning 均为既有代码）

---

## [0.18.0] - 2026-06-19

### 实时双执行域（RT Execution Domain）

> **目标**：实现真正的 RT 调度，命令时延 P99 < 1ms。
> **前置条件**：v0.16.0 Gateway 进程化完成

### 变更内容

#### Task 1：eneros-rt 实时运行时接线

- **`crates/eneros-os/src/rt/runtime.rs`**：
  - 实现 `use_huge_pages`：写 `/proc/sys/vm/nr_hugepages` + `madvise(MADV_HUGEPAGE)`
  - 新增 `HugePageFailed` 错误变体
- **`crates/eneros-gateway/Cargo.toml`**：添加 `eneros-os` 依赖
- **`crates/eneros-gateway/src/rt_executor.rs`**：
  - 新增 `start_rt(rt_config)` 方法，用 `std::thread::Builder` 创建专用 RT 线程
  - 线程内调用 `RtRuntime::configure_current_thread()` 配置 SCHED_FIFO + CPU 隔离 + mlockall + huge pages
  - 然后构建 current_thread tokio runtime 运行循环
  - 提取 `run_loop()` 供 `start()` 和 `start_rt()` 共用

#### Task 2：rt/ipc.rs 真正无锁 SPSC

- **`crates/eneros-os/src/rt/ipc.rs`**：
  - 重写 `RtCommandQueue` 为真正无锁 SPSC：`UnsafeCell<MaybeUninit<T>>` + 原子索引 + Acquire/Release 内存序
  - 移除 `Mutex` 和 `T: Clone` 约束
  - 重写 `RtResultChannel` 为 seqlock 模式（双 `fetch_add` 版本号 + `UnsafeCell`）
  - 实现 Drop 正确清理未消费元素
  - 新增 2 个测试

#### Task 3：硬件看门狗集成

- **`crates/eneros-os/src/rt/watchdog.rs`**：
  - 新增 `WatchdogLogEntry`、`WatchdogLogger`（环形缓冲 100 条 + JSONL 持久化）
  - `HardwareWatchdog` 增加 `logger` 字段和 `open_with_logger()` 构造函数
  - `keepalive()` 失败自动记录日志
- **`crates/eneros-os/src/rt/mod.rs`**：重新导出 `WatchdogError`/`WatchdogLogger`/`WatchdogLogEntry`
- **`crates/eneros-os/bins/eneros-init/src/main.rs`**：
  - 创建 `HardwareWatchdog`（500ms 超时）
  - 主循环每 100ms 喂狗
  - 关闭时 disable，看门狗失败非致命

#### Task 4：内核启动参数验证

- **`os/boot/verify-boot-params.sh`**：新建 bash 脚本，检查 `/proc/cmdline` 的 `isolcpus`/`nohz_full`/`rcu_nocbs`/`irqaffinity` + `/sys/kernel/realtime` + `/sys/devices/system/cpu/isolated`
- **`os/tests/boot_params_test.rs`**：新建 Rust 集成测试，测试 `parse_cmdline()` 和 `check_rt_kernel()`
- **`os/tests/Cargo.toml`**：添加 `boot_params_test` `[[test]]` 条目

#### Task 5：实时性基准测试

- **`crates/eneros-gateway/tests/rt_benchmark.rs`**：新建 3 个基准测试
  - 延迟分布：10000 次命令，P50=1μs P99=12μs P999=22μs
  - 优先级对比：Critical vs Low 各 1000 次
  - SPSC 吞吐量：40M ops/sec

### 验证结果

- `cargo build --workspace` — ✅ 通过（0 error）
- `cargo test --workspace -- --test-threads=1` — ✅ 全部通过
- `cargo clippy --workspace --all-targets` — ✅ 0 error（warning 均为既有代码）

### 验收修复（21 项验收清单逐项检查后修复 2 项 FAIL）

#### 修复：RealtimeExecutor RT 线程独立喂狗

- **`crates/eneros-gateway/src/rt_executor.rs`**：
  - `RealtimeExecutor` 结构体新增 `watchdog: Option<Arc<Mutex<HardwareWatchdog>>>` 字段
  - 新增 `with_watchdog(watchdog)` 构建器方法，解耦 RT 域与 eneros-init 主线程的看门狗喂狗
  - `run_loop()` 新增 `maybe_keepalive()` 辅助方法，每 100ms 调用 `watchdog.lock().keepalive()`
  - `tokio::select!` 新增 `sleep(100ms)` 定时器分支，确保队列空闲时也能周期性喂狗
  - `new()` 和 `with_config()` 初始化 `watchdog: None`，向后兼容

#### 修复：SCHED_OTHER vs SCHED_FIFO 调度策略对比测试

- **`crates/eneros-gateway/tests/rt_benchmark.rs`**：
  - 新增 `test_rt_benchmark_sched_policy_comparison` 测试
  - Phase 1：SCHED_OTHER（默认）下执行 1000 次命令，记录 P50/P99
  - Phase 2：SCHED_FIFO（通过 RtRuntime 配置）下执行 1000 次命令，记录 P50/P99
  - 非 RT 内核：SCHED_FIFO 配置失败时退化，只断言成功执行
  - RT 内核：输出 P99 改善百分比

### 修复后验证

- `cargo build --workspace` — ✅ 通过
- `cargo test --workspace -- --test-threads=1` — ✅ 全部通过（0 failed）
- `cargo clippy --workspace --all-targets` — ✅ 0 error
- 21 项验收清单 — ✅ 21/21 PASS

---

## [0.16.1] - 2026-06-19

### v0.15.0 生产质量修复（真实场景可交付级）

> **目标**：按照真实场景可交付级、使用级标准，修复 v0.15.0 Agent 进程化代码中的生产质量问题。

### 变更内容

#### 修复：AgentProcess::run() 重连逻辑 + 服务初始化 + 错误退避

- **`crates/eneros-agent/src/process.rs`** 重写 `run()` 默认实现：
  - **重连逻辑**：外层重连循环，EventBusBroker 断连时指数退避重连（1s → 2s → 4s → ... → 30s 封顶），不再直接退出进程
  - **Ctrl+C 在退避期间也可响应**：`tokio::select!` 同时监听 `signal::ctrl_c()` 和 `tokio::time::sleep(backoff)`，确保任何时候都能优雅关闭
  - **服务初始化**：`tool_engine`、`memory`（`InMemoryMemory::default()`）、`reasoning`（`RuleBasedEngine::new()`）从 `None` 改为实际初始化，修复 DispatchAgent 等依赖 `ctx.remote.reasoning` 的 Agent 静默降级问题
  - **错误退避**：tick/handle_event 连续错误计数器，达到 10 次后暂停 5s 再继续，避免错误风暴刷爆日志
  - **Agent 实例仅创建一次**：`self.create_agent()` 在重连循环外调用，域状态（`last_dispatch`、`last_forecast` 等）在重连后保留
  - **代码结构**：提取 `connect_and_build()` 和 `run_tick_loop()` 为模块级自由函数，`TickLoopOutcome` 枚举区分 Shutdown/Disconnected
  - **`ipc_socket_dir` 字段**：添加文档注释说明为未来 IPC 预留，当前未使用

#### 修复：6 个 Agent 二进制 tracing + agent_id 一致性

- **6 个二进制**（`dispatch-agent`、`forecast-agent`、`self-healing-agent`、`operation-agent`、`planning-agent`、`trading-agent`）统一修复：
  - **EnvFilter**：`tracing_subscriber::fmt::init()` → `tracing_subscriber::fmt().with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))).init()`，支持 `RUST_LOG` 环境变量控制日志级别
  - **agent_id 一致性**：`DispatchAgentProcess { agent_id: args.agent_id }` → `agent_id: config.agent_id.clone()`，确保 `--config` 加载的配置文件中 `agent_id` 与进程结构体一致（之前 CLI 默认值会覆盖配置文件值）

#### 修复：RemoteGatewayClient 请求超时

- **`crates/eneros-gateway/src/client.rs`**：
  - `RemoteGatewayClient` 新增 `request_timeout: Duration` 字段，默认 10 秒
  - 新增 `with_timeout(addr, timeout)` 构造函数，支持自定义超时
  - `request()` 方法用 `tokio::time::timeout` 包装整个请求-响应周期（connect + write + read），超时返回明确的错误信息
  - 修复前：Gateway 进程挂起时 Agent 永久阻塞；修复后：10 秒超时返回错误，Agent 可记录并继续

### 验证

- `cargo build -p eneros-agent -p eneros-gateway` — ✅ 通过（0 error）
- `cargo build -p eneros-dispatch-agent -p eneros-forecast-agent -p eneros-self-healing-agent -p eneros-operation-agent -p eneros-planning-agent -p eneros-trading-agent` — ✅ 6 个二进制全部通过
- `cargo test -p eneros-agent -p eneros-gateway --lib` — ✅ 116 个测试全部通过
- `cargo test -p eneros-agent` — ✅ 全部通过（含 8 个 e2e_domain 测试）
- `cargo test -p eneros-gateway --test e2e_agentos` — ✅ 6 个端到端测试全部通过
- `cargo clippy -p eneros-agent -p eneros-gateway` — ✅ 新增代码 0 警告（预存 `eneros-device` 警告与本次修改无关）

---

## [0.16.0] - 2026-06-18

### Gateway 进程化（独立二进制 + 端到端 IPC 验证）

> **目标**：将 SafetyGateway/DecisionPipeline 从库迁移为独立进程，通过 TCP IPC 提供服务给 Agent 进程。
> **前置条件**：v0.15.0 Agent 进程化完成

### 变更内容

#### 新增：独立 Gateway 二进制

- **`crates/eneros-gateway/bins/gateway/Cargo.toml`**：新增 `eneros-gateway-bin` 包，`[[bin]]` 名为 `eneros-gateway`
- **`crates/eneros-gateway/bins/gateway/src/main.rs`**：
  - CLI 参数（clap）：`--bind`（默认 `127.0.0.1:9870`）、`--max-history`（默认 100）、`--log-level`（默认 `info`）
  - tracing 初始化：`EnvFilter` + `tracing_subscriber::fmt()`
  - Gateway 栈构建（`build_gateway_server()` 辅助函数）：
    - `PowerNetwork::from_ieee14()` → `Arc<parking_lot::RwLock>` → `NetworkSimulatorAdapter`
    - `FeasibilityProjector::new(simulator)` → `ConstraintEngine::new()` → `SafetyGateway::new(max_history)`
    - `ConstraintAwareValidator::with_default_interlocking(engine, gateway)` → `ConstrainedDecisionPipeline::new(projector, validator, gateway)`
    - `LocalGatewayClient::with_pipeline(gateway, Arc::new(pipeline))` → `GatewayServer::new(client, bind_addr)`
  - 运行：`tokio::select!` 在 `server.run()` 与 `ctrl_c()` 之间竞争，实现优雅关闭
  - 2 个单元测试：`test_cli_args_default`、`test_gateway_stack_construction`
- **`Cargo.toml`**（workspace）：新增 `crates/eneros-gateway/bins/gateway` 成员

#### 新增：端到端集成测试

- **`crates/eneros-gateway/tests/e2e_agentos.rs`**：6 个端到端测试
  - `test_e2e_validate_command`：RemoteGatewayClient → GatewayServer → SafetyGateway.validate_command → Ok
  - `test_e2e_execute_command`：RemoteGatewayClient → GatewayServer → SafetyGateway.execute_command → Ok(ExecutionResult)
  - `test_e2e_submit_command`：RemoteGatewayClient → GatewayServer → SafetyGateway.submit_command（带 SharedPriorityCommandQueue）→ Ok
  - `test_e2e_decide_with_pipeline`：RemoteGatewayClient → GatewayServer → ConstrainedDecisionPipeline.decide → Ok(DecisionResultCore)
  - `test_e2e_decide_without_pipeline_returns_error`：无管线的 GatewayServer → decide → Err("pipeline")
  - `test_e2e_connection_refused`：连接不存在的端口 → Err("connect")
  - 端口分配：`pick_free_port()` 先绑定 `127.0.0.1:0` 获取临时端口再释放，避免固定端口冲突

#### 已有基础设施（v0.15.0 交付，v0.16.0 复用）

- `crates/eneros-gateway/src/server.rs`：`GatewayServer` TCP 服务端（v0.15.0）
- `crates/eneros-gateway/src/client.rs`：`LocalGatewayClient` + `RemoteGatewayClient` + 线格式（v0.15.0）
- 4 个 IPC 接口：`execute_command`、`validate_command`、`submit_command`、`decide`（v0.15.0）
- `SafetyGateway`：per-device 锁池、safety_checks、command_history、SharedPriorityCommandQueue（已有）
- `ConstrainedDecisionPipeline`：7 阶段管线逻辑不变（precondition→project→validate→decide→execute→verify→rollback）

### 关键设计决策

1. **TCP 而非 Unix socket**：v0.15.0 选择 TCP 以支持 Windows 跨平台编译；v0.16.0 保持一致
2. **管线作为 Gateway 子服务**：ConstrainedDecisionPipeline 不单独拆进程，减少 IPC 跳数（Agent → Gateway → Pipeline 在同一进程内）
3. **DeviceManager 保留在 Gateway 进程**：方案 A（推荐），因为命令执行需要设备锁，IPC 化会增加延迟
4. **ObservationProvider 默认不配置**：独立二进制默认不注入 SCADA 观测提供者，后续可从 SCADA 进程拉取
5. **默认网络为 IEEE 14**：独立二进制使用 `PowerNetwork::from_ieee14()` 作为默认网络模型，生产环境可通过配置覆盖

### 验证

- `cargo build --workspace` — ✅ 通过（0 error，17.61s）
- `cargo test --workspace -- --test-threads=1` — ✅ 全部通过（0 FAILED）
- `cargo clippy -p eneros-gateway -p eneros-gateway-bin --all-targets` — ✅ v0.16.0 新增代码 0 警告
- `cargo test -p eneros-gateway --test e2e_agentos` — ✅ 6 个端到端测试全部通过
- `cargo test -p eneros-gateway-bin` — ✅ 2 个单元测试全部通过

---

## [0.15.0] - 2026-06-18

### 7 个专业 Agent 进程二进制（Task 2：AgentProcess::run 默认实现 + 6 个独立进程）

> **设计目标**：将 7 个专业 Agent（Dispatch / LoadForecast / Operation / SelfHealing / Planning / Trading）从库级 tokio 任务迁移为独立 OS 进程。每个 Agent 作为独立二进制运行，通过 EventBusBroker 与其他 Agent 通信，通过 GatewayServer 执行控制命令。域算法（economic_dispatch、calculate_ace、locate_fault_section、generate_isolation_sequence、find_restoration_path、single/double/holt_winters 指数平滑、evaluate_capacity、generate_expansion_plan、generate_bid、assess_risk 等）保持不变。

### 变更内容

#### eneros-agent

- **`src/process.rs`**：实现 `AgentProcess::run()` 默认方法（替换原 `todo!()`）
  - 步骤 1：连接 EventBusBroker 创建 publisher 客户端，包装为 `Arc<dyn EventBusPublisher>`（`RemoteEventBusPublisher`）
  - 步骤 2：连接 EventBusBroker 创建 subscriber 客户端（独立连接），调用 `subscribe(EventFilter::default())` 订阅全部事件，返回 `mpsc::Receiver<Event>` 包装为 `Arc<TokioMutex<Option<Receiver>>>`
  - 步骤 3：创建 `RemoteGatewayClient::new(gateway_addr)` 包装为 `Arc<dyn GatewayClient>`
  - 步骤 4：构建 `RemoteHandles`（`message_store = None`、`tool_engine = None`、`memory = None`、`reasoning = None`、`constraint_engine = None`、`network = PowerNetwork::from_ieee14()`、`system_state = Normal`、`audit_trail = Vec::new()`）
  - 步骤 5：构建 `LocalContext`（从 `AgentConfig` 复制 agent_id、authority、jurisdiction、tick_interval）
  - 步骤 6：构建 `Arc<AgentContext>`
  - 步骤 7：创建 `ActionDispatcher::new(event_bus, gateway_client)`
  - 步骤 8：调用 `self.create_agent(&config)` 创建 Agent 实例
  - 步骤 9：运行 `tokio::select!` tick 循环——Ctrl+C 优雅关闭 / 事件接收 → `handle_event()` → `dispatch()` / tick 定时器 → `tick()` → `dispatch()`
  - 事件接收器关闭（broker 断开）时打印 warn 并退出循环
- **新增 6 个独立二进制 crate**（`crates/eneros-agent/bins/`）：
  - `dispatch-agent/`：`eneros-dispatch-agent` — 经济调度 Agent 进程（`AgentType::Dispatcher`，`AuthorityLevel::Supervisor`）
  - `forecast-agent/`：`eneros-forecast-agent` — 负荷预测 Agent 进程（`AgentType::Custom("LoadForecast")`，`AuthorityLevel::Observer`，内置 `TimeSeriesEngine::new(86400)`）
  - `operation-agent/`：`eneros-operation-agent` — 运维 Agent 进程（`AgentType::Operator`，`AuthorityLevel::Operator`）
  - `self-healing-agent/`：`eneros-self-healing-agent` — 自愈 Agent 进程（`AgentType::Custom("SelfHealing")`，`AuthorityLevel::Emergency`，**RT 实时进程**，默认 tick 500ms）
  - `planning-agent/`：`eneros-planning-agent` — 规划 Agent 进程（`AgentType::Custom("Planning")`，`AuthorityLevel::Supervisor`）
  - `trading-agent/`：`eneros-trading-agent` — 交易 Agent 进程（`AgentType::Custom("Trading")`，`AuthorityLevel::Operator`）
- 每个二进制的 `main.rs` 实现：
  - `clap::Parser` 命令行参数：`--agent-id`、`--eventbus-addr`、`--gateway-addr`、`--tick-interval-ms`、`--config`（JSON 配置文件路径，可选）
  - `AgentProcess` 实现：`agent_id()`、`agent_type()`、`create_agent()`（使用各 Agent 的默认构造参数，域算法保持不变）
  - `#[tokio::main]` 入口：初始化 `tracing_subscriber`、解析参数、加载配置（JSON 文件优先，否则用命令行参数构造 `AgentConfig`）、调用 `process.run(config).await`
- `self-healing-agent` 的 `main.rs` 顶部包含 RT 调度说明注释——RT 调度（SCHED_FIFO）由 `eneros-init` 通过 `AgentScheduler` 外部应用，二进制本身无需特殊代码

#### 工作区配置

- **根 `Cargo.toml`**：`[workspace] members` 新增 6 个二进制路径
  - `crates/eneros-agent/bins/dispatch-agent`
  - `crates/eneros-agent/bins/forecast-agent`
  - `crates/eneros-agent/bins/operation-agent`
  - `crates/eneros-agent/bins/self-healing-agent`
  - `crates/eneros-agent/bins/planning-agent`
  - `crates/eneros-agent/bins/trading-agent`

### 验证

- `cargo build -p eneros-dispatch-agent -p eneros-forecast-agent -p eneros-operation-agent -p eneros-self-healing-agent -p eneros-planning-agent -p eneros-trading-agent` 通过，0 error
- `cargo run -p eneros-dispatch-agent -- --help` 正确输出 CLI 帮助
- `cargo run -p eneros-self-healing-agent -- --help` 正确输出 CLI 帮助（含 RT 进程标识）
- 域算法文件（`agents/*.rs`）零修改

---

### AgentOrchestrator 远程 Agent 协调支持

> **设计目标**：重构 `AgentOrchestrator` 支持两种运行模式——进程内模式（legacy/测试，直接调用 `agent.tick()`）和远程模式（v0.15.0 进程迁移，通过 `EventBusPublisher` 广播 tick 事件，Agent 进程独立订阅并执行）。

### 变更内容

#### eneros-core

- **`src/event.rs`**：
  - `EventType` 新增 `AgentTick` 变体——由 orchestrator 广播以触发所有 Agent 进程的 `tick()`
  - `EventPayload` 新增 `Tick` 变体——tick 广播事件的空 payload

#### eneros-agent

- **`src/orchestrator.rs`**：
  - `AgentOrchestrator` 结构体新增 `remote_mode: bool` 字段
  - 新增 `new_remote(ctx, dispatcher)` 构造函数——创建远程模式 orchestrator（`remote_mode = true`，`agents` 为空）
  - 新增 `is_remote_mode()` 查询方法
  - 现有 `new()`、`with_pipeline()`、`with_pipeline_and_feedback()` 构造函数均设置 `remote_mode = false`（进程内模式不变）
  - `tick_all()`：远程模式下广播 `EventType::AgentTick` 事件到 EventBusPublisher，返回空 `DispatchResult` 列表（Agent 进程各自执行 tick 并通过本地 `ActionDispatcher` 分发动作）；进程内模式保持原有 `join_all` 并发逻辑
  - `process_event()`：远程模式下将事件发布到 EventBusBroker，Agent 进程通过订阅独立处理；进程内模式保持原有拓扑路由逻辑
  - `route_action()`、`dispatch_via_pipeline()`、`retry_with_feedback()`：仅进程内模式使用，保持不变
  - `ConflictResolver`、`EmergencyResponsePipeline`、`TopologyAwareScheduler`：保持不变
- **`src/process.rs`**：修复预存编译错误——`EventBusClient::subscribe()` 签名已改为 `Option<EventFilter>`，调用处补充 `Some()` 包装

#### 测试

- 新增 4 个 orchestrator 测试：
  - `test_remote_mode_flag_and_empty_agents`：验证 `new_remote()` 设置 `remote_mode = true` 且 `agent_count() == 0`
  - `test_in_process_mode_flag_is_false`：验证 `new()` 设置 `remote_mode = false`
  - `test_remote_mode_tick_all_broadcasts_agent_tick`：验证远程模式 `tick_all()` 广播 `AgentTick` 事件
  - `test_remote_mode_process_event_publishes_event`：验证远程模式 `process_event()` 发布事件到 EventBus

### 验证

- `cargo build -p eneros-agent` 通过，0 error
- `cargo test -p eneros-agent` 全部通过（334 单元测试 + 15 集成测试 = 349 通过，0 失败）
- `cargo build --workspace` 通过，0 error

---

### GatewayClient 基础设施（Agent 进程迁移前置）

> **设计目标**：为 Agent 进程迁移（v0.15.0 主线）提供 Gateway 访问的统一客户端接口。Agent 进程通过 `GatewayClient` trait 访问 SafetyGateway 服务，无需关心 Gateway 是库级集成（`LocalGatewayClient`）还是独立进程（`RemoteGatewayClient` + `GatewayServer`）。

### 变更内容

#### eneros-core

- **`Cargo.toml`**：新增 `async-trait`、`anyhow` 工作区依赖
- **`src/gateway_client.rs`**（新增）：`GatewayClient` async trait，定义 4 个方法
  - `execute_command(cmd) -> Result<ExecutionResult>`：立即执行命令
  - `validate_command(&cmd) -> Result<()>`：仅校验不执行
  - `submit_command(cmd) -> Result<()>`：提交到优先级队列
  - `decide(action, ctx_core) -> Result<DecisionResultCore>`：运行决策管线
- **`src/lib.rs`**：声明 `pub mod gateway_client` 并 re-export `GatewayClient`

#### eneros-gateway

- **`Cargo.toml`**：新增 `anyhow`、`serde_json` 依赖
- **`src/client.rs`**（新增）：
  - `LocalGatewayClient`：包装 `Arc<SafetyGateway>`（可选 `Arc<ConstrainedDecisionPipeline>`），实现 `GatewayClient` + `Clone`
  - `RemoteGatewayClient`：TCP IPC 客户端，每次请求建立新连接
  - `GatewayRequest` / `GatewayResponse`：线格式消息枚举（`#[serde(tag = "type")]`）
  - `read_frame` / `write_frame`：4 字节 LE 长度前缀 + JSON payload（与 EventBusBroker 一致）
- **`src/server.rs`**（新增）：
  - `GatewayServer`：TCP IPC 服务端，每连接 `tokio::spawn` 独立任务
  - `handle_connection` / `handle_request`：请求-响应循环
  - 实现 `Clone`（通过 `LocalGatewayClient: Clone`）
- **`src/pipeline_types.rs`**：新增 `impl From<&DecisionContextCore> for DecisionContext`
  - `device_states` 字段默认为 `None`（不在 Core 中，调用方需显式注入）
- **`src/lib.rs`**：声明 `pub mod client`、`pub mod server`，re-export `LocalGatewayClient`、`RemoteGatewayClient`、`GatewayRequest`、`GatewayResponse`、`GatewayServer`

#### 测试

- **`tests/gateway_client.rs`**（新增）：14 个集成测试
  - LocalGatewayClient：execute_command / validate_command / submit_command / decide（含 pipeline 和无 pipeline）
  - RemoteGatewayClient + GatewayServer：TCP 往返测试（所有 4 个方法）
  - 并发连接测试（5 个并发客户端）
  - Local vs Remote 决策结果一致性测试

### 验证

- `cargo build -p eneros-core -p eneros-gateway` 通过，0 error
- `cargo test -p eneros-gateway` 全部通过（116 单元测试 + 22 决策管线测试 + 14 GatewayClient 测试 = 152 通过，0 失败）

---

### eneros-init 集成 Agent 进程启动（Task 6：AgentServiceConfig + spawn_all_agents）

> **设计目标**：让 eneros-init PID 1 在启动系统服务后自动 spawn 所有配置的 Agent 进程，并将其纳入 AgentOS 内核管理（AgentRegistry/AgentSupervisor/AgentScheduler/AuthorityEnforcer/ResourceQuota）。

### 变更内容

#### eneros-os

- **`src/init/config.rs`**：
  - 新增 `AgentServiceConfig` 结构体：`agent_id`、`agent_type`、`authority`、`binary`、`args`、`env`、`scheduling_policy`、`resource_quota`、`dependencies`
  - `InitConfig` 新增 `agents: Vec<AgentServiceConfig>` 字段（`#[serde(default)]` 向后兼容，无 `[[agents]]` 段的旧 TOML 仍可解析）
  - `load_default()` 新增 6 个默认 Agent 配置：`dispatch-1`/`forecast-1`/`operation-1`/`self-healing-1`（RT SCHED_FIFO，priority=80，cpus=[2,3]，lock_memory=true）/`planning-1`/`trading-1`
  - `validate()` 新增 Agent 配置校验：空 agent_id 拒绝、空 binary 拒绝、重复 agent_id 拒绝
  - 8 个新单元测试覆盖默认配置/RT 调度/TOML 解析/校验逻辑
- **`src/init/mod.rs`**：re-export `AgentServiceConfig`
- **`src/agentos/mod.rs`**：re-export `AgentSpawnConfig`（supervisor 模块）

#### eneros-init 二进制

- **`bins/eneros-init/src/main.rs`**：
  - 新增 `spawn_all_agents()`：遍历 agent_configs，通过 `AgentSupervisor::spawn()` 启动每个 Agent 进程，然后应用调度策略（`AgentScheduler::schedule()`）、授予权限（`AuthorityEnforcer::auto_grant()`）、设置资源配额（`ResourceQuota::set_quota()`）——所有 OS 级操作非致命，失败时 warn 日志并继续
  - 新增 `stop_all_agents()`：关停所有 Running/Degraded 状态的 Agent 进程
  - 新增 `restart_crashed_agents()`：主循环每次迭代调用，检查 Agent 健康状态，重启 Crashed 进程（复用 supervisor 的 5 次/分钟崩溃降级策略）
  - `main()` 流程更新：步骤 7 创建 5 个 AgentOS 内核组件（共享 `Arc<AgentRegistry>`），步骤 8 在系统服务启动后调用 `spawn_all_agents()`，步骤 9 `run_main_loop()` 接受 `&supervisor` + `&agent_configs` 参数并每轮调用 `restart_crashed_agents()`，步骤 10 关停时先 `stop_all_agents()` 再 `manager.stop_all()`
  - `run_main_loop()` 签名扩展：新增 `supervisor: &Arc<AgentSupervisor>` 和 `agent_configs: &[AgentServiceConfig]` 参数
  - 4 个新单元测试：空配置 spawn/stop/restart 幂等性、注册到 registry 验证（非 Linux 接受 Running 或 Crashed 状态）
  - `Cargo.toml` 新增 `[dev-dependencies] eneros-core`（测试使用 `AuthorityLevel::Supervisor`）

#### 生产配置

- **`os/rootfs/files/etc/eneros/init.toml`**：新增 6 个 `[[agents]]` 段
  - 每个 Agent 配置完整的 `agent_id`、`agent_type`、`authority`、`binary`、`args`、`dependencies`、`[agents.env]`（RUST_LOG）、`[agents.resource_quota]`（cpu_percent/memory_mb/max_pids）
  - `self-healing-1` 配置 `[agents.scheduling_policy.Realtime]`：`priority=80`、`cpus=[2,3]`、`lock_memory=true`

#### 测试修复

- **`crates/eneros-gateway/tests/decision_pipeline_verification.rs`**：修复 v0.15.0 重构后的 API 兼容性
  - `ActionDispatcher::with_pipeline(event_bus, gateway, pipeline)` → `ActionDispatcher::new_local(event_bus, gateway).with_pipeline(Arc::new(pipeline))`
  - `ActionDispatcher::new(event_bus, gateway)` → `ActionDispatcher::new_local(event_bus, gateway)`

### 验证

- `cargo build --workspace` 通过，0 error（30.05s）
- `cargo test --workspace -- --test-threads=1` 全部通过（0 失败）
- `cargo clippy --workspace --all-targets` 通过（exit code 0，仅 eventbus broker 的 `std::io::Error::other` 预存警告）

---

### BREAKING CHANGES

> v0.15.0 为破坏性版本，eneros-agent crate API 不兼容。以下变更需调用方迁移：

1. **`Agent` trait**：移除 `start()`/`stop()` 方法（由 `AgentSupervisor` 管理生命周期），保留 `handle_event()`/`tick()`/`handle_emergency()` 领域方法
2. **`AgentContext`**：拆分为 `LocalContext`（本地状态）+ `RemoteHandles`（远程服务句柄）。原 `Arc<EventBus>`/`Arc<SafetyGateway>` 字段替换为 `Arc<dyn EventBusPublisher>`/`Arc<dyn GatewayClient>` trait 对象
3. **`ActionDispatcher`**：
   - `new(event_bus, gateway)` → `new_local(event_bus, gateway)`（进程内模式，使用 `LocalEventBusPublisher` + `LocalGatewayClient` 包装）
   - `with_pipeline(pipeline)` 改为 builder 方法（返回 `Self`）
   - 原 `with_pipeline(event_bus, gateway, pipeline)` 三参数构造函数移除
4. **`AgentOrchestrator`**：新增 `new_remote(ctx, dispatcher)` 构造函数用于远程模式；原 `new()`/`with_pipeline()`/`with_pipeline_and_feedback()` 保持进程内模式（`remote_mode = false`）
5. **`SpawnedAgent`**：由 `AgentProcess` trait 替代。每个 Agent 作为独立二进制运行，通过 `AgentProcess::run(config)` 入口启动
6. **`EventBusClient::subscribe()`**：签名从 `subscribe(filter: EventFilter)` 改为 `subscribe(filter: Option<EventFilter>)`（`None` 等价于 `EventFilter::default()`）

### 迁移指南

- **进程内 Agent 集成**（测试/legacy 场景）：使用 `ActionDispatcher::new_local(event_bus, gateway)` 替代原 `new()`，其余 API 不变
- **远程 Agent 进程**：实现 `AgentProcess` trait，通过 `eneros-agent/bins/<name>-agent/` 模板创建独立二进制，由 `eneros-init` 通过 `[[agents]]` 配置段管理
- **EventBus 订阅**：将 `subscribe(EventFilter::default())` 改为 `subscribe(None)` 或 `subscribe(Some(EventFilter::default()))`

### 最终验证

- `cargo build --workspace` — ✅ 通过（0 error）
- `cargo test --workspace -- --test-threads=1` — ✅ 全部通过（0 FAILED，领域算法测试全部保留）
- `cargo clippy --workspace --all-targets` — ✅ 通过（exit code 0）

### 代码审查修复（Karpathy 原则排查）

> 基于 Karpathy「Think Before Coding / Surgical Changes / Goal-Driven Execution」原则对 v0.15.0 全局代码进行系统性审查，发现并修复以下问题：

#### 修复 1：`dispatcher.rs` 安全缺陷（严重）

- **问题**：`ActionDispatcher::dispatch_structured()` 在 `gateway_client.decide()` 返回 `Err` 时，原代码返回 `Ok(DispatchResult::CommandExecuted)` — 但实际未执行任何命令。在电力系统场景下，这会导致 Agent 误认为控制指令已执行，可能引发安全事故。
- **修复**：将错误路径改为 `Err(EnerOSError::Internal("gateway decide failed: ..."))`，正确传播错误。调用方（`AgentOrchestrator::dispatch_via_pipeline`）已通过 `has_pipeline()` 检查在先，不会触发此路径；直接调用 `dispatch_structured` 的测试已更新为期望错误。
- **影响文件**：`crates/eneros-agent/src/dispatcher.rs`、`crates/eneros-gateway/tests/decision_pipeline_verification.rs`

#### 修复 2：`eneros-init/main.rs` clippy 警告

- **问题**：`if let Some(info) = supervisor.health_check(&cfg.agent_id).ok()` 触发 clippy `matching on Some with ok() is redundant` 警告。
- **修复**：改为 `if let Ok(info) = supervisor.health_check(&cfg.agent_id)`。
- **影响文件**：`crates/eneros-os/bins/eneros-init/src/main.rs`

#### 修复 3：`supervisor.rs` 未使用变量

- **问题**：`AgentSupervisor::should_restart()` 中 `let info = self.registry.lookup(...)` 的 `info` 从未被读取（仅用于存在性校验），触发 `unused_variables` 警告。
- **修复**：改为 `let _info = ...`（保留 `?` 操作符的存在性校验语义）。
- **影响文件**：`crates/eneros-os/src/agentos/supervisor.rs`

#### 修复 4：`process.rs` 多余 clone

- **问题**：`AgentProcess::run()` 中 `authority: config.authority.clone()` 对 `Copy` 类型 `AuthorityLevel` 调用 `clone()`，触发 clippy `using clone on type which implements Copy` 警告。
- **修复**：改为 `authority: config.authority`（直接复制）。
- **影响文件**：`crates/eneros-agent/src/process.rs`

#### 审查结论

- v0.15.0 核心改动文件（`context.rs`、`dispatcher.rs`、`orchestrator.rs`、`process.rs`、`init/config.rs`、`client.rs`、`publisher.rs`）逻辑正确
- `EventBusClient::subscribe(Option<EventFilter>)` 签名变更的所有调用方已正确更新
- `ActionDispatcher::new()` 接受 trait 对象的设计正确（`new_local()` 包装具体类型，`new()` 接受 `Arc<dyn ...>`）
- `AgentOrchestrator` 双模（`new()`/`new_remote()`）实现正确，`remote_mode` 标志控制 tick/event 广播路径
- 7 个 Agent 二进制的 `AgentProcess::run()` 默认实现正确（EventBus 连接 → Gateway 连接 → tick 循环 → Ctrl+C 优雅退出）

---

## [0.14.0] - 2026-06-18

### 共享 Schema 迁移到 eneros-core（Task 1：IPC 共享类型）

> **设计目标**：将跨进程共享的类型从 eneros-gateway、eneros-eventbus、eneros-agent 迁移到 eneros-core，作为 AgentOS 内核 IPC（进程间通信）的共享 Schema。eneros-core 不依赖任何业务 crate，避免循环依赖。

### 变更内容

#### 新增 eneros-core 模块

- **`eneros-core/src/command.rs`**：`CommandType`、`CommandPriority`、`DeviceValue`、`Command`
  - 新增 `DeviceValue` 枚举，镜像 `eneros_device::adapter::DataValue`，使 `Command` 不再依赖 eneros-device
  - `Command::device_value` 字段类型从 `Option<eneros_device::adapter::DataValue>` 改为 `Option<DeviceValue>`，保留 `#[serde(skip)]`
  - `Command::with_device()` 签名改为接受 `DeviceValue`
- **`eneros-core/src/event.rs`**：`EventType`、`EventPayload`、`Event`（从 eneros-eventbus 迁入）
- **`eneros-core/src/agent_message.rs`**：`MessagePriority`、`AgentMessage`（从 eneros-agent 迁入）
- **`eneros-core/src/execution.rs`**：`ExecutionResult`（从 eneros-gateway 迁入），新增 `Serialize/Deserialize` derive
- **`eneros-core/src/pipeline_types.rs`**：`PipelineAuditEntry`（从 eneros-gateway 迁入，新增 `Serialize/Deserialize`）、`DecisionContextCore`、`DecisionResultCore`（可序列化子集，用于 IPC）

#### eneros-core/src/lib.rs

- 声明并 re-export 新模块：`command`、`event`、`agent_message`、`execution`、`pipeline_types`

#### eneros-device

- **`adapter.rs`**：新增 `impl From<eneros_core::DeviceValue> for DataValue`，在网关/设备边界做无损转换

#### eneros-gateway（re-export + 适配）

- **`command.rs`**：`pub use eneros_core::{Command, CommandPriority, CommandType, DeviceValue};`，保留测试
- **`executor.rs`**：`pub use eneros_core::execution::ExecutionResult;`，`execute()` 中将 `DeviceValue` 转换为 `DataValue` 后传给 `DeviceManager`
- **`decision_pipeline.rs`**：`device_value` 构造改用 `DeviceValue`
- **`pipeline_types.rs`**：re-export `PipelineAuditEntry`，保留 `DecisionContext`/`EnhancedPipelineDecision`，新增 `impl From<&DecisionContext> for DecisionContextCore` 和 `impl From<&EnhancedPipelineDecision> for DecisionResultCore`
- **`gateway.rs`**、**`executor.rs`** 测试：`with_device()` 调用改用 `DeviceValue`

#### eneros-eventbus / eneros-agent（re-export）

- **`eneros-eventbus/src/event.rs`**：`pub use eneros_core::event::{Event, EventPayload, EventType};`
- **`eneros-agent/src/message.rs`**：`pub use eneros_core::agent_message::{AgentMessage, MessagePriority};`，保留测试

### AgentOS 内核模块（Task 2-8：eneros-os/agentos/）

> **设计目标**：在 eneros-os crate 内建立 `agentos/` 子模块，实现 AgentOS 内核的 7 个核心组件。所有 Linux 特定系统调用（capabilities、cgroups、SCHED_FIFO）通过 `#[cfg(target_os = "linux")]` 条件编译隔离，非 Linux 平台提供等价语义的 stub 实现，确保整个 workspace 可在 Windows 上编译开发。

#### Task 2：AgentRegistry 进程注册表

- **`crates/eneros-os/src/agentos/registry.rs`**：基于 `RwLock<HashMap<String, AgentInfo>>` 的线程安全 Agent 进程注册表
  - `AgentInfo` 字段：`agent_id`、`pid`、`agent_type`、`authority`、`status`、`started_at`、`last_heartbeat`
  - 接口：`register/lookup/list/unregister/update_status/heartbeat`
  - `RegistryError` 错误枚举（`AlreadyRegistered`/`NotFound`/`Io`）
  - 8 个单元测试覆盖注册/查询/列举/注销/状态更新/心跳/重复注册/未找到场景

#### Task 3：AgentSupervisor 生命周期监督

- **`crates/eneros-os/src/agentos/supervisor.rs`**：Agent 进程生命周期管理器
  - 持有 `AgentRegistry` + `RestartPolicy`（`Never`/`OnFailure`/`Always`）+ 崩溃计数窗口（5 次/分钟降级）
  - 接口：`spawn/stop/restart/health_check/list_agents`
  - `spawn()`：Linux 使用 `std::process::Command::spawn()`，记录 PID 到 registry；非 Linux 使用 stub PID
  - `stop()`：SIGTERM → 10s 超时 → SIGKILL（Linux），非 Linux 直接标记 Stopped
  - `health_check()`：通过 `kill(pid, 0)` 检查进程存活（Linux），非 Linux 查 registry 状态
  - `SupervisorError` 错误枚举，含 `Registry(#[from] RegistryError)` 变体
  - 5 个单元测试覆盖 spawn/stop/restart/health_check/崩溃重启策略

#### Task 4：AgentIPC 进程间通信

- **`crates/eneros-os/src/agentos/ipc.rs`**：基于 TCP/Unix socket 的 Agent 间消息传递
  - `AgentIpcConfig`：`tcp_port_base`（默认 9000）、`unix_socket_dir`（默认 `/var/run/eneros`）、`transport`（`Tcp`/`UnixSocket`）
  - `AgentIpcServer`：异步服务端，监听 TCP 或 Unix socket，接收 `AgentMessage` 并路由
  - `AgentIpcClient`：异步客户端，`send(target_id, msg)`/`recv()`/`publish(topic, event)`
  - 跨平台：Unix socket 类型通过 `#[cfg(unix)]` 条件导入，Windows 仅支持 TCP
  - `IpcError` 错误枚举（`Connect`/`Serialize`/`Io`/`Timeout`）
  - 3 个单元测试覆盖配置/端口分配/Unix socket 路径生成

#### Task 5：EventBusBroker 独立进程

- **`crates/eneros-eventbus/src/broker.rs`**：EventBusBroker 核心实现
  - `BrokerConfig`：`bind_addr`（默认 `127.0.0.1:9876`）、`unix_socket`、`channel_capacity`（默认 4096）、`max_subscribers`（默认 256）
  - `EventFilter`：支持按 `event_type` 和 `source` 过滤，`matches()` 方法
  - `BrokerMessage`：tagged enum（`Publish`/`Subscribe`/`Unsubscribe`/`GetStats`/`Event`/`Stats`/`Ack`/`Error`），serde 序列化
  - `EventBusBroker`：基于 `tokio::sync::broadcast` channel 的 fan-out，`Arc` 共享，原子计数器统计
  - `handle_client()` 支持三种客户端模式：Subscribe（订阅者循环）、Publish（发布者循环）、GetStats（一次性查询）
  - 帧格式：4 字节小端长度前缀 + JSON payload
  - 7 个单元测试（含异步 TCP pub/sub 集成测试）

- **`crates/eneros-eventbus/src/client.rs`**：EventBusClient IPC 客户端
  - `connect_tcp(addr)`/`connect_unix(path)` 连接 Broker
  - `publish(event)` 发布事件，`subscribe(filter)` 返回 `mpsc::Receiver<Event>`（后台 task 读取）
  - `stats()` 查询 Broker 统计，`close()` 关闭连接
  - 跨平台：`GenericConn` 枚举在 Unix 支持 Tcp/Unix，非 Unix 仅 Tcp
  - 3 个单元测试（含异步 subscribe+receive 集成测试）

- **`crates/eneros-eventbus/bins/broker/`**：独立 Broker 二进制
  - `Cargo.toml`：依赖 eneros-eventbus/eneros-core/tokio/tracing/clap
  - `src/main.rs`：clap CLI（`--bind`/`--socket`/`--channel-capacity`/`--max-subscribers`/`-v`），Ctrl+C 优雅关闭

#### Task 6：AuthorityEnforcer 权限强制

- **`crates/eneros-os/src/agentos/authority.rs`**：基于 Linux capabilities 的权限强制
  - `Capability` 枚举：`NetBindService`(10)/`SysAdmin`(21)/`SysRawio`(17)/`SysTime`(25)/`NetAdmin`(12)
  - `CapabilitySet`：`HashSet<Capability>`，支持 grant/revoke/contains
  - `AgentAction` 枚举：`BindPort`/`SystemConfig`/`RawDeviceAccess`/`NetworkConfig`/`Shutdown`
  - `AuthorityEnforcer`：`grant(agent_id, caps)`/`revoke(agent_id, caps)`/`check(agent_id, action)`/`auto_grant(agent_id, level)`
  - `authority_to_capabilities()` 映射：Observer→空，Operator→[NetBindService]，Supervisor→[NetBindService, SysAdmin]，Emergency→[NetBindService, SysAdmin, SysRawio]
  - Linux：通过 `libc::syscall(SYS_capset, ...)` 设置进程 capabilities（`_LINUX_CAPABILITY_VERSION_3`）
  - 非 Linux：仅缓存权限集，不实际调用 syscall
  - 12 个单元测试覆盖 grant/revoke/check/auto_grant/映射/边界条件

#### Task 7：ResourceQuota 资源配额

- **`crates/eneros-os/src/agentos/quota.rs`**：基于 cgroups v2 的资源配额管理
  - `QuotaConfig`：`cpu_percent`（默认 100）/`memory_mb`（默认 512）/`max_pids`（默认 64）
  - `ResourceUsage`：`cpu_usage_percent`/`memory_usage_mb`/`memory_limit_mb`/`pid_count`
  - `ResourceQuota`：`set_quota(agent_id, config)`/`update_quota(agent_id, config)`/`remove_quota(agent_id)`/`usage(agent_id)`
  - Linux：创建 `/sys/fs/cgroup/eneros/agent-<id>/` 目录，写入 `cpu.max`/`memory.max`/`pids.max`，读取 `cpu.stat`/`memory.current`/`pids.current`
  - 非 Linux：返回模拟使用值（基于进程运行时间），不操作文件系统
  - 9 个单元测试覆盖配额设置/更新/删除/查询/边界条件

#### Task 8：AgentScheduler 调度策略

- **`crates/eneros-os/src/agentos/scheduler.rs`**：RT 调度策略管理
  - `SchedulingPolicy` 枚举：`Normal`（SCHED_OTHER）/`Realtime { priority, cpus, lock_memory }`（SCHED_FIFO）
  - `SchedulingPolicy::default_for_agent_type()`：SelfHealing→Realtime(80, [2,3], true)，其他→Normal
  - `AgentScheduler`：`schedule(agent_id, policy)`/`auto_schedule(agent_id, agent_type)`/`preempt(agent_id)`/`demote(agent_id)`
  - Linux：`sched_setscheduler()` 设置 SCHED_FIFO，`sched_setaffinity()` 设置 CPU 亲和性，`mlockall()` 锁定内存
  - 非 Linux：仅缓存调度策略，不实际调用 syscall
  - clippy 修复：`!(1..=99).contains(&priority)` 替代 `priority < 1 || priority > 99`
  - 14 个单元测试覆盖 Normal/Realtime 策略/auto_schedule/preempt/demote/边界条件

### enerosctl 管理 CLI（Task 9）

- **`crates/eneros-os/bins/enerosctl/`**：clap-based 管理 CLI 工具
  - `Cargo.toml`：依赖 eneros-os/eneros-core/eneros-eventbus/tokio/clap/serde_json
  - `src/main.rs`：顶层命令 `agent`/`eventbus`/`system`，`--format`（table/json）全局选项
  - `src/commands.rs`：8 个命令实现
    - `agent list`：查询所有 Agent 状态（TCP 连接控制通道，回退到本地 state 文件）
    - `agent start/stop/restart <id>`：Agent 生命周期控制
    - `agent status <id>`：单个 Agent 详细状态
    - `eventbus status`：查询 EventBusBroker 统计
    - `eventbus subscribe <topic>`：实时订阅事件流
    - `system info`：系统信息（OS/内核/CPU/内存/Agent 数）
  - `src/format.rs`：表格格式化、`SystemInfo` 结构体、辅助格式化函数

### Workspace 配置

- **`Cargo.toml`**（workspace root）：新增 `crates/eneros-eventbus/bins/broker` 和 `crates/eneros-os/bins/enerosctl` 到 `[workspace] members`
- **`crates/eneros-os/src/agentos/mod.rs`**：声明并 re-export 全部 6 个子模块（registry/supervisor/ipc/authority/quota/scheduler）
- **`crates/eneros-eventbus/src/lib.rs`**：新增 `pub mod broker;` 和 `pub mod client;`，re-export `EventBusBroker`/`EventBusClient`/`BrokerConfig`/`BrokerStats`/`EventFilter`/`BrokerMessage`/`BrokerError`

### 跨平台编译策略

所有 Linux 特定系统调用通过条件编译隔离：

| 功能 | Linux 实现 | 非 Linux 实现 |
|------|-----------|--------------|
| capabilities | `libc::syscall(SYS_capset, ...)` | 缓存到 `HashMap` |
| cgroups v2 | 读写 `/sys/fs/cgroup/eneros/agent-<id>/` | 返回模拟使用值 |
| SCHED_FIFO | `sched_setscheduler()` + `sched_setaffinity()` + `mlockall()` | 缓存调度策略 |
| Unix socket | `tokio::net::UnixListener/UnixStream` | 仅 TCP，`#[cfg(unix)]` 守卫导入 |

### 验证

- `cargo build --workspace`：0 错误
- `cargo test --workspace -- --test-threads=1`：1769 通过，0 失败
- `cargo clippy -p eneros-os -p eneros-eventbus -p eneros-eventbus-broker -p enerosctl --all-targets`：0 错误（eneros-os 存在既有 unused 警告，与本次变更无关）

---

## [0.13.1] - 2026-06-18

### 项目结构重构：部署文件归档

> **设计目标**：将根目录散落的部署相关文件（Dockerfile、docker-compose.yml、scripts/）统一归档到 `deploy/` 目录下，降低根目录复杂度，为未来规模化规划让路。容器化保留为可选部署方式（非必须），EnerOS 作为 Rust 原生二进制可直接在 Windows/Linux/macOS 上运行。

### 变更内容

#### 文件迁移

- **`Dockerfile`** → `deploy/docker/Dockerfile`
- **`docker-compose.yml`** → `deploy/docker/docker-compose.yml`
- **`scripts/build.sh`** → `deploy/scripts/build.sh`
- **`scripts/dev.sh`** → `deploy/scripts/dev.sh`
- **`scripts/healthcheck.sh`** → `deploy/scripts/healthcheck.sh`
- 删除空的 `scripts/` 目录

#### 引用同步更新

- **`.github/workflows/ci.yml`**：Docker 构建步骤的 `file` 路径更新为 `./deploy/docker/Dockerfile`（build context 仍为项目根）
- **`deploy/docker/docker-compose.yml`**：
  - `build.context` 改为 `../..`（指向项目根）
  - `build.dockerfile` 改为 `deploy/docker/Dockerfile`（相对 context）
  - `eneros.toml` 挂载路径改为 `../../eneros.toml`
  - `prometheus.yml` 挂载路径改为 `../prometheus.yml`
  - 顶部 Usage 注释更新为 `docker compose -f deploy/docker/docker-compose.yml up -d`
- **`deploy/scripts/build.sh`**：
  - `PROJECT_ROOT` 计算从 `dirname $SCRIPT_DIR` 改为 `dirname $(dirname $SCRIPT_DIR)`（两级向上）
  - `docker build` 命令增加 `-f deploy/docker/Dockerfile` 参数
  - 完成提示中的 `docker compose up -d` 改为 `docker compose -f deploy/docker/docker-compose.yml up -d`
- **`deploy/scripts/dev.sh`**：
  - `PROJECT_ROOT` 计算同上调整
  - Usage 注释中的路径更新为 `./deploy/scripts/dev.sh`
- **`docs/deployment.md`**：
  - 所有 `./scripts/*.sh` 引用更新为 `./deploy/scripts/*.sh`
  - 所有 `docker compose up/logs/--profile` 命令增加 `-f deploy/docker/docker-compose.yml` 参数
  - 新增 Windows 用户注意说明：`.sh` 脚本需 Git Bash/WSL，原生 PowerShell 可直接用 `cargo run` 替代

### 设计说明

- **容器化非必须**：EnerOS 编译产物为单一原生二进制 `eneros-api`，可直接 `cargo run` 或运行 release 二进制，无需 Docker
- **历史记录保留**：`CHANGELOG.md`/`ROADMAP.md` 中过往版本的文件路径引用保持原样，作为历史快照不修改
- **向后兼容**：此次仅为文件位置调整，无 API/功能/配置格式变更

---

## [0.13.0] - 2026-06-18

### OS 启动集成测试

> **设计目标**：v0.13.0 在 v0.12.0 引导与镜像构建链路之上新增 OS 启动集成测试基础设施，新增 `os/tests/` 目录承载两类测试：Rust 单元测试（`boot_test.rs`，可在 Windows/Linux/macOS 任意开发主机运行）验证 eneros-init 启动逻辑（服务图构建、配置加载、启动顺序、信号处理、rootfs 结构与内核启动参数文档化）；Shell 集成测试脚本（`boot_test.sh`，在 Linux 构建环境运行）通过 QEMU 启动 raw 镜像并验证内核启动、eneros-init 作为 PID 1 启动、服务启动顺序、应用层 eneros-api 启动及 HTTP 健康检查通过。测试 crate `eneros-os-tests` 作为独立 workspace 成员注册，依赖 `eneros-os` crate，10 个单元测试全部通过，clippy 0 警告。

### 新功能

#### Rust 单元测试（`os/tests/boot_test.rs`）

- **`os/tests/boot_test.rs`** — 10 个单元测试，验证 eneros-init 启动逻辑：
  - `test_default_service_config_valid` — 验证默认服务配置有效（无环、依赖存在），network 在 timesync 之前，power-app 最后启动
  - `test_service_dependencies` — 验证服务依赖关系正确（network 无依赖、timesync 依赖 network、power-app 依赖 network/timesync/syslog/devmgr）
  - `test_restart_policies` — 验证重启策略（network/timesync/syslog/devmgr 为 Always，power-app 为 OnFailure）
  - `test_service_manager_creation` — 验证 ServiceManager 创建后调用 `prepare()` 注册 5 个服务到 supervisor
  - `test_startup_order` — 验证拓扑排序结果（5 个服务、network 在 timesync 之前、power-app 最后），处理 HashMap 迭代顺序非确定性
  - `test_config_from_toml` — 验证从 TOML 字符串解析配置（服务名、二进制路径、重启策略、环境变量）
  - `test_config_file_path` — 验证默认配置文件路径格式（`/etc/eneros/init.toml`）
  - `test_signal_handler_creation` — 验证 SignalHandler 初始状态（无 shutdown/reload 请求）
  - `test_rootfs_structure_documentation` — 文档化 rootfs 必需文件结构（/bin/eneros-init、/bin/eneros-api、/etc/eneros/init.toml 等）
  - `test_kernel_boot_parameters` — 文档化内核 RT 优化启动参数（isolcpus/nohz_full/rcu_nocbs/irqaffinity/mlock）

#### QEMU 启动测试脚本（`os/tests/boot_test.sh`）

- **`os/tests/boot_test.sh`** — QEMU 集成测试脚本：
  - 环境变量可配置：`ARCH`（默认 x86_64）、`IMAGE`（默认 `../image-builder/output/eneros-$ARCH.img`）、`QEMU_MEMORY`（默认 2G）、`QEMU_CPUS`（默认 4）、`TIMEOUT`（默认 120s）
  - 架构映射：x86_64→qemu-system-x86_64、aarch64→qemu-system-aarch64
  - 检查镜像存在性和 QEMU 可用性
  - QEMU 启动参数：raw 驱动、内存/CPU 配置、headless 模式、串口日志输出、端口转发（8080→health check）、virtio-net-pci 设备、KVM 加速（如可用）
  - 启动检测循环：检查 QEMU 进程存活、扫描日志中的启动标志（"EnerOS init starting"、"initialization complete"、"Service startup order"）、curl HTTP 健康检查
  - 失败时输出最后 50 行启动日志
  - `set -euo pipefail` 严格错误处理，`trap` 清理临时日志文件

#### 测试 crate 与 workspace 集成

- **`os/tests/Cargo.toml`** — 测试 crate 配置：
  - 包名 `eneros-os-tests`，继承 workspace 版本/edition/authors/license
  - `[[test]]` 目标 `boot_test` 指向 `boot_test.rs`
  - 依赖：`eneros-os`（path 依赖）、`toml`（workspace）、`serde`（workspace）
- **`Cargo.toml`（workspace）** — 在 members 列表新增 `"os/tests"`

#### 测试说明文档

- **`os/tests/README.md`** — 测试说明：
  - 两类测试说明（单元测试 + 集成测试）
  - 运行命令（`cargo test -p eneros-os-tests` / `./boot_test.sh`）
  - 前置条件（Rust 工具链 / Linux + QEMU + 镜像）
  - 测试流程（开发时单元测试 → CI/CD 集成测试）
  - GitHub Actions CI/CD 集成示例

### API 适配说明

- 测试代码根据 `crates/eneros-os/src/init/manager.rs` 实际 API 调整：`ServiceManager::new()` 不会自动注册服务到 supervisor，需调用 `prepare()` 方法后才注册
- `test_startup_order` 测试处理 `ServiceGraph::topological_sort()` 基于 HashMap 迭代的非确定性顺序，仅断言确定性约束（network 在 timesync 之前、power-app 最后）

---

## [0.12.0] - 2026-06-18

### UEFI 引导配置 + initramfs 构建 + 镜像构建器

> **设计目标**：v0.12.0 在 v0.11.0 操作系统基础设施（kernel + rootfs）之上补齐引导与镜像构建链路，新增 `os/boot/` 目录承载 initramfs 构建脚本和 UEFI 引导配置（GRUB + systemd-boot 双方案），新增 `os/image-builder/` 目录承载可启动 raw 镜像的端到端构建流程（分区创建 → rootfs 安装 → 内核安装 → initramfs 安装 → 引导加载程序安装 → fstab 生成）。initramfs 包含 eneros-init/eneros-api 二进制和必要内核模块（virtio/net/ext4），提供 init 脚本完成 proc/sys/dev 挂载、根分区发现（sda2/vda2/nvme0n1p2）、switch_root 切换到真实根文件系统。GRUB 配置提供 3 个启动项（正常/恢复/Slot B），携带 RT 优化启动参数（isolcpus/nohz_full/rcu_nocbs/irqaffinity/mlock）和 A/B 双分区启动槽位。镜像构建器输出可通过 QEMU 启动测试的 raw 镜像，为后续 v0.9.0 高可用部署和 v1.0.0 生态构建提供可交付的镜像产物。

### 新功能

#### initramfs 构建脚本

- **新增 `os/boot/` 目录结构**：
  - `build-initramfs.sh` — initramfs 构建脚本
  - `grub.cfg` — GRUB UEFI 引导菜单配置
  - `systemd-boot.conf` — systemd-boot 条目配置（GRUB 备选方案）
  - `README.md` — 说明文档
- **`os/boot/build-initramfs.sh`** 构建脚本：
  - 环境变量可配置：`ARCH`（默认 x86_64）、`OUTPUT_DIR`、`KERNEL_OUTPUT`（默认 `../kernel/output`）、`ROOTFS_OUTPUT`（默认 `../rootfs/output`）、`INITRAMFS`
  - 架构映射：x86_64→x86、aarch64→arm64
  - 从 rootfs 复制 `eneros-init` 和 `eneros-api` 二进制到 `/bin/`
  - 生成 `/init` 脚本（PID 1）：挂载 proc/sys/devtmpfs/tmpfs（/run、/tmp）→ 扫描根分区（/dev/sda2、/dev/vda2、/dev/nvme0n1p2）→ 挂载 ext4 根 → `mount --move` 迁移伪文件系统 → `exec switch_root` 切换到真实根并执行 `/bin/eneros-init`；未找到根设备时降级到紧急 shell
  - 创建最小 `/etc/passwd`（root:0:0）和 `/etc/group`（root:0）
  - 复制必要内核模块：virtio、net、ext4 驱动 + `modules.dep` + `modules.builtin`
  - 创建设备节点：console（c 5 1）、null（c 1 3）、zero（c 1 5）、tty（c 5 0）
  - 打包：`find . | cpio -H newc -o | gzip -9` 生成 `initramfs.img`
  - `set -euo pipefail` 严格错误处理，`trap` 清理临时目录
  - 输出：`output/initramfs.img`

#### GRUB UEFI 引导配置

- **`os/boot/grub.cfg`** 配置文件：
  - 加载模块：part_gpt、ext2、fat、search、search_fs_uuid
  - 通过 `search --file /boot/vmlinuz-eneros` 定位启动分区
  - 3 个启动项：
    - **EnerOS Power-Native OS**（默认）— root=/dev/sda2，RT 优化参数（isolcpus=2,3、nohz_full=2,3、rcu_nocbs=2,3、irqaffinity=0,1、mlock=1），双控制台（ttyS0,115200 + tty0），panic=10，ENEROS_BOOT_SLOT=A
    - **EnerOS Power-Native OS (Recovery Mode)** — root=/dev/sda2，single 单用户模式，ENEROS_BOOT_SLOT=A
    - **EnerOS Power-Native OS (Slot B)** — root=/dev/sda3（A/B 双分区槽位 B），RT 优化参数，ENEROS_BOOT_SLOT=B
  - 超时 3 秒，默认启动项 0
  - 配色方案：menu_color_normal=white/blue、menu_color_highlight=black/light-gray

#### systemd-boot 引导配置（备选）

- **`os/boot/systemd-boot.conf`** 配置文件：
  - 2 个启动条目：正常模式（Slot A）+ 恢复模式（single）
  - 与 GRUB 一致的启动参数（root、RT 优化、双控制台、ENEROS_BOOT_SLOT）
  - 放置于 EFI 系统分区的 `loader/entries/eneros.conf`

#### 镜像构建器

- **新增 `os/image-builder/` 目录结构**：
  - `build.sh` — 镜像构建主脚本
  - `create-partitions.sh` — 分区创建脚本（被 build.sh source）
  - `install-bootloader.sh` — 引导加载程序安装脚本（被 build.sh source）
  - `README.md` — 说明文档
- **`os/image-builder/build.sh`** 主构建脚本：
  - 环境变量可配置：`ARCH`（默认 x86_64）、`OUTPUT_DIR`、`IMAGE_NAME`、`IMAGE_SIZE`（默认 2G）、`EFI_SIZE`（默认 512M）
  - 自动定位依赖目录：`SCRIPT_DIR`、`OS_DIR`、`KERNEL_DIR`、`ROOTFS_DIR`、`BOOT_DIR`
  - 12 步构建流程：
    1. 构建内核（若 `vmlinuz-eneros` 不存在则调用 `os/kernel/build.sh`）
    2. 构建 rootfs（若 `eneros-init` 不存在则调用 `os/rootfs/build.sh`）
    3. 构建 initramfs（若 `initramfs.img` 不存在则调用 `os/boot/build-initramfs.sh`）
    4. 创建 raw 镜像文件（`truncate -s`）
    5. 创建 GPT 分区（source `create-partitions.sh`）
    6. Loop 挂载镜像（`losetup -fP --show`），挂载 EFI 和 root 分区
    7. 安装 rootfs（解压 `eneros-rootfs-$ARCH.tar.gz`）
    8. 安装内核（vmlinuz-eneros、System.map、config、modules）
    9. 安装 initramfs
    10. 安装引导加载程序（source `install-bootloader.sh`）
    11. 生成 `/etc/fstab`（sda2→/、sda1→/boot/efi、proc、sysfs、devtmpfs、tmpfs）
    12. sync 同步文件系统
  - `trap cleanup EXIT` 清理：umount EFI/root 分区、losetup -d、删除临时挂载点
  - 输出 QEMU 测试命令提示
  - `set -euo pipefail` 严格错误处理
- **`os/image-builder/create-partitions.sh`** 分区创建脚本：
  - `create_partitions()` 函数：`sgdisk --zap-all` 清空 → 分区 1（EFI System，typecode EF00，FAT32，从扇区 2048 开始）→ 分区 2（EnerOS Root，typecode 8300，ext4，`--largest-new` 占用剩余空间）→ `sgdisk -p` 打印分区表
  - `format_partitions()` 函数：`mkfs.vfat -F 32 -n EFI` 格式化 EFI 分区、`mkfs.ext4 -F -L eneros-root` 格式化 root 分区
  - 扇区大小转换：`numfmt --from=iec` + awk 计算（512 字节/扇区）
- **`os/image-builder/install-bootloader.sh`** 引导加载程序安装脚本：
  - `install_bootloader()` 函数：架构映射（x86_64→x86_64-efi、aarch64→arm64-efi）
  - 优先复制预构建 GRUB EFI 二进制（grubx64.efi→BOOTX64.EFI / grubaa64.efi→BOOTAA64.EFI）
  - 回退到 `grub-install --target=$grub_target --efi-directory=... --bootloader-id=ENEROS --removable`
  - 复制 `grub.cfg` 到 `$root_mount/boot/grub/grub.cfg` 和 `$efi_mount/EFI/ENEROS/grub.cfg`（fallback）
  - 创建 EFI 目录结构：`EFI/BOOT`、`EFI/ENEROS`

### 镜像布局

```
┌─────────────────────────────────────┐
│  GPT Partition Table                │
├─────────────────────────────────────┤
│  Partition 1: EFI System (FAT32)    │  512MB
│  - /EFI/BOOT/BOOTX64.EFI            │
│  - GRUB UEFI bootloader             │
├─────────────────────────────────────┤
│  Partition 2: EnerOS Root (ext4)    │  ~1.5GB
│  - /bin/eneros-init                 │
│  - /bin/eneros-api                  │
│  - /etc/eneros/                     │
│  - /boot/vmlinuz-eneros             │
│  - /boot/initramfs.img              │
│  - /lib/modules/                    │
└─────────────────────────────────────┘
```

### 引导流程

1. UEFI 固件从 EFI 系统分区加载 GRUB
2. GRUB 加载 Linux 内核和 initramfs
3. 内核启动并执行 initramfs 的 `/init` 脚本
4. init 脚本挂载伪文件系统（proc、sys、dev）
5. init 脚本发现并挂载真实根分区
6. init 脚本通过 `switch_root` 切换到真实根
7. `eneros-init` 作为 PID 1 在真实根文件系统上启动
8. eneros-init 按依赖顺序启动系统服务

---

## [0.11.0] - 2026-06-18

### 操作系统基础设施（Linux kernel + PREEMPT_RT 配置 + 最小 rootfs 构建脚本）

> **设计目标**：v0.11.0 引入 EnerOS 的操作系统构建基础设施，新增 `os/kernel/` 目录承载 Linux kernel + PREEMPT_RT 实时补丁的配置文件和构建脚本，新增 `os/rootfs/` 目录承载基于 musl libc 和静态链接 Rust 二进制的最小根文件系统构建脚本。内核侧提供 x86_64 与 aarch64 双架构内核配置，覆盖 PREEMPT_RT 实时抢占、CPU 隔离、高精度定时器、No-HZ full tickless、硬件看门狗、AF_PACKET（GOOSE/SV 协议）、AppArmor 安全加固、模块签名等电力原生 OS 所需的实时性与安全性能力。rootfs 侧构建 eneros-init（PID 1）和 eneros-api 静态二进制，配置 5 个系统服务（network/timesync/syslog/devmgr/power-app）的依赖图与重启策略，生成可部署的最小 rootfs tarball。构建脚本自动化下载内核源码、应用 RT 补丁、配置、编译和安装，为后续 v1.5.0 安全扩展和实时性调优奠定基础。

### 新功能

#### Linux kernel + PREEMPT_RT 配置与构建

- **新增 `os/kernel/` 目录结构**：
  - `config-x86_64` — x86_64 架构内核配置
  - `config-aarch64` — ARM64 架构内核配置
  - `build.sh` — 内核构建脚本
  - `README.md` — 说明文档
  - `patches/README.md` — 补丁目录说明（预留）
- **`os/kernel/config-x86_64`** 配置文件：
  - PREEMPT_RT 实时抢占：`CONFIG_PREEMPT_RT=y`、`CONFIG_PREEMPT_RT_FULL=y`、`CONFIG_HIGH_RES_TIMERS=y`、`CONFIG_NO_HZ_FULL=y`
  - CPU 隔离：`CONFIG_CPU_ISOLATION=y`、`CONFIG_RCU_NOCB_CPU=y`、`CONFIG_RCU_NOCB_CPU_DEFAULT_ALL=y`、`CONFIG_RCU_BOOST=y`
  - 内存锁定：`CONFIG_MLOCK=y`、`CONFIG_MLOCK_ONFAULT=y`、`CONFIG_HUGETLBFS=y`、`CONFIG_HUGETLB_PAGE=y`、`CONFIG_TRANSPARENT_HUGEPAGE=y`
  - 设备驱动：PCI、USB（EHCI/OHCI/UHCI/XHCI）、USB Serial（FTDI/PL2303）、E1000/E1000E（QEMU 网卡）、VirtIO（PCI/Net/Blk/Console）
  - 串口：`CONFIG_SERIAL_8250=y`、`CONFIG_SERIAL_8250_CONSOLE=y`
  - 文件系统：EXT4、FAT/VFAT、TMPFS、PROC、SYSFS、DEVTMPFS（自动挂载）
  - UEFI 引导：`CONFIG_EFI=y`、`CONFIG_EFI_STUB=y`、`CONFIG_EFI_PARTITION=y`
  - 看门狗：`CONFIG_WATCHDOG=y`、`CONFIG_WATCHDOG_NOWAYOUT=y`、`CONFIG_SOFT_WATCHDOG=y`、`CONFIG_X86_BOOTPARAM_WATCHDOG=y`、`CONFIG_ITCO_WDT=y`
  - 安全功能：AppArmor、Hardened Usercopy、FORTIFY_SOURCE、Stack Protector Strong、Strict Kernel/Module RWX、Lockdown LSM、Integrity
  - 网络原始套接字：`CONFIG_PACKET=y`（GOOSE/SV 协议支持）
  - 模块支持：`CONFIG_MODULES=y`、`CONFIG_MODULE_UNLOAD=y`、`CONFIG_MODULE_SIG=y`、`CONFIG_MODULE_SIG_FORCE=y`、`CONFIG_MODULE_SIG_ALL=y`（SHA256 签名）
  - 加密 API：AES（X86_64 加速）、GCM、CBC、CTR、SHA256/512、DRBG、Jitter RNG
  - 调试与追踪：ftrace、function tracer、sched tracer、hwlat/osnoise/timerlat tracer、preempt tracer、hung task 检测
  - 禁用不需要的功能：KEXEC、HIBERNATION、SOUND、WIRELESS、BLUETOOTH、DRM、FB、VGA Console、XEN、BPF JIT
  - x86_64 特定：SMP（64 CPU）、NUMA、X2APIC、TSC、MCE（Intel/AMD）、Microcode、MTRR、PAT、SMAP、UMIP、MPK、TSX、Seccomp
- **`os/kernel/config-aarch64`** 配置文件：
  - ARM64 特定：`CONFIG_ARM64=y`、`CONFIG_ARCH_ARM64=y`、`CONFIG_ARM64_4K_PAGES=y`、`CONFIG_ARCH_DMA_ADDR_T_64BIT=y`
  - ARM64 CPU 特性：PAN、LSE Atomics、VHE、UAO、PMEM、RAS、PAuth、BTI、MTE、E0PD、SVE、NEON
  - ARM64 errata 修复：826319/827319/824069/819471/832075/843419/1024718/1418040/1165522/1286807/1463225/1542419/1508412/2051678/2077057/2658417
  - ARM64 平台支持：Actions/Sunxi/Alpine/Apple/BCM/Berlin/Bitmain/Exynos/Sparx5/K3/LG1K/HisI/Keembay/MediaTek/Meson/Mvebu/MXC/NPCM/QCom/Realtek/Renesas/Rockchip/Seattle/SocFPGA/Synquacer/Tegra/TeslaFSD/Sprd/Thunder/Thunder2/Uniphier/VExpress/Visconti/XGene/ZynqMP
  - ARM64 虚拟化（QEMU）：`CONFIG_VIRTIO=y`、`CONFIG_VIRTIO_MMIO=y`、`CONFIG_VIRTIO_MMIO_CMDLINE_DEVICES=y`、`CONFIG_VIRTIO_NET=y`、`CONFIG_VIRTIO_BLK=y`
  - ARM64 串口：`CONFIG_SERIAL_AMBA_PL011=y`、`CONFIG_SERIAL_AMBA_PL011_CONSOLE=y`、`CONFIG_SERIAL_OF_PLATFORM=y`
  - ARM64 看门狗：`CONFIG_ARM_SP805_WATCHDOG=y`、`CONFIG_ARM_SBSA_WATCHDOG=y`、`CONFIG_DW_WATCHDOG=y`、`CONFIG_IMX2_WDT=y`
  - 其他配置（PREEMPT_RT、CPU 隔离、文件系统、安全、模块、加密、调试）与 x86_64 一致
- **`os/kernel/build.sh`** 构建脚本：
  - 环境变量可配置：`KERNEL_VERSION`（默认 6.6）、`RT_PATCH_VERSION`（默认 6.6-rt23）、`ARCH`（默认 x86_64）、`JOBS`（默认 nproc）、`BUILD_DIR`、`OUTPUT_DIR`
  - 架构映射：x86_64→x86、aarch64→arm64
  - 8 步构建流程：下载内核源码 → 解压 → 下载 PREEMPT_RT 补丁 → 应用补丁（dry-run 检测已应用）→ 复制配置 → olddefconfig → 编译 bzImage+modules → 安装到 output
  - 输出：`output/boot/vmlinuz-eneros`、`output/boot/config-eneros`、`output/boot/System.map-eneros`、`output/lib/modules/`
  - `set -euo pipefail` 严格错误处理
- **`os/kernel/README.md`** 说明文档：构建前置依赖、x86_64/ARM64/自定义版本构建命令、配置说明、推荐启动参数（isolcpus/nohz_full/rcu_nocbs/irqaffinity/mlock）
- **`os/kernel/patches/README.md`** 补丁目录说明：命名规范（`NNNN-description.patch`）、按数字序应用、当前无自定义补丁（使用 stock PREEMPT_RT）

#### 最小 rootfs 构建脚本

- **新增 `os/rootfs/` 目录结构**：
  - `build.sh` — rootfs 构建脚本
  - `README.md` — 说明文档
  - `files/etc/passwd` — 最小用户数据库
  - `files/etc/group` — 最小用户组数据库
  - `files/etc/hostname` — 主机名配置
  - `files/etc/eneros/init.toml` — eneros-init 服务配置
  - `files/var/lib/eneros/.gitkeep` — 持久化数据目录占位符
- **`os/rootfs/build.sh`** 构建脚本：
  - 环境变量可配置：`ARCH`（默认 x86_64）、`TARGET_TRIPLE`、`OUTPUT_DIR`、`ROOTFS_DIR`、`ROOTFS_TARBALL`
  - 架构映射：x86_64→x86_64-unknown-linux-musl、aarch64→aarch64-unknown-linux-musl
  - 静态链接：`RUSTFLAGS="-C target-feature=+crt-static"`，构建 `eneros-api` 和 `eneros-init` 二进制
  - 9 步构建流程：创建目录结构 → 构建 Rust 二进制（musl 静态链接）→ 安装二进制到 `/bin/` → 安装配置文件 → 创建系统文件（os-release/hosts/resolv.conf/nsswitch.conf）→ 创建设备节点（console/null/zero/ptmx/tty/random/urandom）→ 设置权限 → 计算大小 → 打包 tarball
  - init 符号链接：`/sbin/init` → `/bin/eneros-init`、`/bin/init` → `/bin/eneros-init`
  - `set -euo pipefail` 严格错误处理
- **`os/rootfs/files/etc/passwd`**：最小用户数据库（root:0:0 + eneros:1000:1000，shell 为 /bin/sh）
- **`os/rootfs/files/etc/group`**：最小用户组数据库（root:0、eneros:1000、tty:5、disk:6、wheel:10:eneros）
- **`os/rootfs/files/etc/hostname`**：主机名 `eneros`
- **`os/rootfs/files/etc/eneros/init.toml`** 服务配置：
  - 5 个服务：network（eneros-netcfg）、timesync（eneros-timesync）、syslog（eneros-syslog）、devmgr（eneros-devmgr）、power-app（eneros-api）
  - 依赖关系图：timesync 依赖 network；power-app 依赖 network/timesync/syslog/devmgr
  - 重启策略：always（network/timesync/syslog/devmgr 系统服务）、on_failure（power-app 应用服务）
  - graceful_timeout_secs：10s（系统服务）、30s（power-app）
  - 环境变量：RUST_LOG=info、ENEROS_CONFIG=/etc/eneros/eneros.toml
- **`os/rootfs/files/var/lib/eneros/.gitkeep`**：持久化数据目录占位符（空文件）
- **`os/rootfs/README.md`** 说明文档：构建前置依赖（Linux + Rust musl target）、x86_64/ARM64 构建命令、rootfs 内容清单、目标大小（<50MB）、设计原则（静态链接/无包管理/无 systemd/eneros-init 为 PID 1/eneros-netcfg 管理网络）

#### eneros-init PID 1 系统完整实现

- **新增 `crates/eneros-os/src/init/config.rs`** 配置加载模块：
  - `InitConfig` 结构体（`services: Vec<ServiceConfig>`），派生 `Serialize`/`Deserialize`/`Default`
  - `load_from_file(path)` — 从 TOML 文件加载配置，返回 `Result<Self, ConfigError>`
  - `load_from_file_or_default(path)` — 文件不存在时回退到内置默认配置
  - `load_default()` — 内置 5 个默认服务（network/timesync/syslog/devmgr/power-app），与历史硬编码配置一致
  - `apply_env_overrides()` — 环境变量覆盖：`ENEROS_INIT_<SERVICE>_BINARY`/`_ARGS`/`_RESTART_POLICY`，服务名大写化、非字母数字转 `_`
  - `validate()` — 校验空名称、空 binary、重复名称
  - `ConfigError` 枚举（Io/Parse/Invalid），派生 `thiserror::Error`
  - 10 个单元测试覆盖默认配置、TOML 解析（含 args/deps）、校验逻辑、环境变量覆盖、env_prefix 规范化
- **新增 `crates/eneros-os/src/init/signal.rs`** 信号处理模块：
  - `SignalHandler` 结构体（`shutdown_requested: Arc<AtomicBool>`、`reload_requested: Arc<AtomicBool>`），派生 `Clone`
  - `install()` — Linux 平台通过 `nix::sys::signal::sigaction` 注册 SIGTERM/SIGINT（→shutdown）和 SIGHUP（→reload）处理器，使用 `SaFlags::SA_RESTART`；非 Linux 平台为 no-op
  - `should_shutdown()` / `should_reload()` — 查询原子标志（Linux 同时检查 static flag 和 Arc flag）
  - `clear_reload()` / `clear_shutdown()` — 清除标志
  - `request_shutdown()` / `request_reload()` — 测试辅助方法，模拟信号到达
  - Linux 信号处理器仅执行 `AtomicBool::store`（async-signal-safe），使用 static `AtomicBool` 而非捕获 Rust 状态
  - 8 个单元测试覆盖标志设置/清除、clone 共享状态、install 不报错、Default 实现
- **新增 `crates/eneros-os/src/init/manager.rs`** 服务管理器模块：
  - `ServiceManager` 结构体（graph/supervisor/processes/startup_times/exit_times/degraded/crash_history/startup_order/max_restarts_per_minute/restart_delay）
  - `new(graph)` / `with_max_restarts_per_minute(n)` / `with_restart_delay(delay)` — 构造与配置
  - `prepare()` — 注册服务到 supervisor，计算拓扑排序缓存 startup_order
  - `start_all()` — 按依赖顺序启动所有服务，单个失败不中断
  - `start_service(name)` — 检查依赖就绪 → spawn 进程 → 记录 PID/startup_time → 更新 supervisor 状态
  - `stop_all(timeout_secs)` — 逆序停止所有服务
  - `stop_service(name, timeout_secs)` — Linux 发送 SIGTERM → 轮询等待（100ms 间隔）→ 超时 SIGKILL；非 Linux 直接 `child.kill()`
  - `reap_children()` — 遍历所有子进程 `try_wait()`，记录退出码/崩溃历史/降级状态；Linux 额外调用 `waitpid(-1, WNOHANG)` 回收孤儿僵尸进程
  - `restart_pending()` — 返回满足重启条件的服务列表（策略 + 崩溃频率 + 延迟）
  - `restart_service(name)` — 重置状态后调用 `start_service`
  - `record_crash(name)` — 滚动窗口（60s）崩溃计数，超限（默认 5 次/分钟）进入降级模式
  - `dependencies_ready(name)` / `is_running(name)` / `running_count()` / `degraded_count()` / `has_running()` — 状态查询
  - `graph_mut()` / `supervisor_mut()` / `refresh_startup_order()` — 支持 SIGHUP 热重载
  - `spawn_service(config)` — 使用 `std::process::Command` 跨平台 spawn（Linux 内部 fork+exec），继承 stdout/stderr，null stdin
  - `InitError` 枚举（ServiceNotFound/AlreadyRunning/DependenciesNotReady/Graph/Spawn/Stop/Signal），派生 `thiserror::Error`
  - 常量：`DEFAULT_GRACEFUL_TIMEOUT_SECS=10`、`DEFAULT_RESTART_DELAY=1s`、`DEFAULT_MAX_RESTARTS_PER_MINUTE=5`、`CRASH_WINDOW=60s`
  - 18 个单元测试覆盖空管理器、prepare、依赖检查、启动缺失服务、依赖阻塞、停止未运行服务、reap 空列表、崩溃计数、降级触发、重启延迟、重启就绪、builder 方法、不可启动 binary 容错
- **修改 `crates/eneros-os/src/init/service.rs`**：
  - `RestartPolicy` 添加 `#[serde(rename_all = "snake_case")]`，支持 TOML 配置中的 `always`/`on_failure`/`no` 小写变体
  - `ServiceConfig` 所有可选字段添加 `#[serde(default)]`（args/restart_policy/dependencies/env/working_dir/user），`graceful_timeout_secs` 添加 `#[serde(default = "default_graceful_timeout")]`（默认 10）
  - 新增 `default_graceful_timeout()` 函数
- **修改 `crates/eneros-os/src/init/mod.rs`**：添加 `pub mod manager`/`pub mod config`/`pub mod signal`，导出 `ServiceManager`/`InitConfig`/`SignalHandler`
- **重写 `crates/eneros-os/bins/eneros-init/src/main.rs`**（从 stub 到完整实现）：
  - 8 步启动流程：初始化日志 → 加载配置（`ENEROS_INIT_CONFIG` 环境变量或 `/etc/eneros/init.toml`）→ 构建依赖图 → 验证图 → 安装信号处理器 → 创建 ServiceManager + prepare → start_all → 主循环
  - `run_main_loop()` — 100ms 轮询：检查 shutdown 信号 → 检查 reload 信号 → reap_children → restart_pending → restart_service；PID 1 永不退出（除非 shutdown），非 PID 1 在无服务时退出（测试/开发模式）
  - `handle_reload()` — SIGHUP 热重载：重新加载配置 → 替换 graph → 重新注册非运行服务到 supervisor → refresh_startup_order
  - `is_pid1()` — `std::process::id() == 1`
  - `LoopResult` 枚举（Shutdown/NoServicesAndNotPid1）
  - 常量：`DEFAULT_CONFIG_PATH="/etc/eneros/init.toml"`、`LOOP_INTERVAL=100ms`、`SHUTDOWN_TIMEOUT_SECS=10`
  - 5 个单元测试覆盖 is_pid1、LoopResult 枚举、shutdown 信号退出、无服务非 PID1 退出、reload 不崩溃
- **验证结果**：`cargo build -p eneros-os` 0 errors，`cargo build -p eneros-init` 0 errors，`cargo test -p eneros-os` 51+1 测试通过，`cargo test -p eneros-init` 5 测试通过，`cargo clippy -p eneros-os --all-targets` 0 警告，`cargo clippy -p eneros-init --all-targets` 0 警告

---

## [0.10.0] - 2026-06-18

### 生产深化（性能优化 + 时序增强 + 协议补全 + API/可视化改进）

> **设计目标**：v0.10.0 聚焦生产深化，采用"综合推进（混合）"策略覆盖性能优化、时序数据增强、协议模型补全和 API/可视化改进四大方向。PipelineStatistics 原子化消除锁争用，per-device 锁池实现设备级并发，SOE 事件顺序记录补全保护动作时标，存储级降采样支持长周期查询，CIM→PowerNetwork 转换器补全 IEC 61968/61970 模型导入，OpenAPI 自动文档提升 API 可用性，Dashboard SVG data-* 修复恢复热力图 overlay。
>
> **验证结果**：`cargo build --workspace` 0 errors，`cargo clippy --workspace --all-targets` 0 errors。eneros-timeseries 70 项测试、eneros-gateway 133 项测试、eneros-api 114 项测试、eneros-network 40 项测试、eneros-dashboard 35 项测试、eneros-scada 58 项测试全部通过。

### 新功能

#### Task 4：SOE 事件顺序记录

- **新增 `crates/eneros-timeseries/src/soe.rs`** 模块：
  - `SoeEventType` 枚举（BreakerOpen/BreakerClose/ProtectionTrip/Alarm/Manual），`as_str()` / `from_str()` 双向转换，`#[serde(rename_all = "snake_case")]` 序列化
  - `SoeRecord` 结构体（sequence_number / timestamp / device_id / event_type / priority / value），派生 `Serialize`/`Deserialize`/`ToSchema`
  - `SoeStorage` 枚举支持双后端：`Memory(RwLock<Vec<SoeRecord>>)` 和 `Sqlite(Mutex<Connection>)`（使用 `std::sync::Mutex` 保护 `rusqlite::Connection`）
  - `SoeRecorder` 结构体：`AtomicU64` 全局序号 + `SoeStorage` 后端
    - `new_memory()` / `new_sqlite(db_path)` 构造函数（SQLite 创建 `soe_events` 表 + `idx_soe_time` / `idx_soe_device` 索引）
    - `record()` / `record_now()` 方法：`fetch_add(1, Relaxed)` 分配全局唯一递增序号，存储记录
    - `query()` 方法：按时间范围 + 可选 device_id / event_type 过滤，按 sequence_number 升序返回
    - `latest(limit)` 方法：最近 N 个事件（按 sequence_number 降序）
    - `count()` 方法：总记录数
    - `Default` 实现（返回内存版本）
  - 时间戳存储为 RFC3339 字符串（`to_rfc3339()` / `parse_from_rfc3339()`）
  - 11 个单元测试覆盖序号递增、内存/SQLite 存储查询、device_id 过滤、时间范围过滤、latest 限制、event_type 序列化/反序列化、计数、event_type 过滤、默认构造、record_now 时间戳
- **修改 `crates/eneros-timeseries/src/lib.rs`**：导出 `pub mod soe` 和 `pub use soe::{SoeRecord, SoeEventType, SoeRecorder, SoeStorage}`
- **修改 `crates/eneros-timeseries/Cargo.toml`**：添加 `utoipa = { workspace = true, features = ["chrono"] }` 依赖（为 `DateTime<Utc>` 实现 `ToSchema`）；添加 `serde_json` dev-dependency
- **新增 `crates/eneros-api/src/handlers/soe.rs`** handler 模块：
  - `SoeQueryParams`（start/end/device_id/event_type/limit，派生 `IntoParams`）
  - `SoeLatestParams`（limit，派生 `IntoParams`）
  - `SoeResponse`（success/count/data/error，派生 `ToSchema`）
  - `GET /api/soe` — `query_handler`：按时间范围查询，支持 device_id / event_type / limit 过滤，recorder 未配置返回 503
  - `GET /api/soe/latest` — `latest_handler`：最近 N 个事件（limit 默认 100），recorder 未配置返回 503
  - 两个 handler 均添加 `#[utoipa::path(...)]` 注解
  - 5 个测试：无 recorder 返回 503、正常查询、latest 默认 limit、latest 自定义 limit、无效 event_type 返回 400
- **修改 `crates/eneros-api/src/handlers/mod.rs`**：添加 `pub mod soe;`
- **修改 `crates/eneros-api/src/app.rs`**：
  - `AppState` 新增 `soe_recorder: Option<Arc<eneros_timeseries::SoeRecorder>>` 字段
  - 新增 `with_soe_recorder(recorder)` builder 方法
  - `create_router()` 注册 `/soe` 和 `/soe/latest` 路由
- **修改 `crates/eneros-api/src/main.rs`**：
  - TimeSeriesEngine 初始化后（步骤 4a 之后）创建 `SoeRecorder::new_sqlite("eneros_soe.db")`
  - DataPipeline 构建时调用 `.with_soe_recorder(soe_recorder.clone())` 注入
  - AppState 构建时调用 `.with_soe_recorder(soe_recorder.clone())` 注入
- **修改 `crates/eneros-api/src/openapi.rs`**：OpenApiDoc 添加 `soe::query_handler` / `soe::latest_handler` 路径和 `SoeRecord` / `SoeResponse` schema
- **修改 `crates/eneros-scada/src/pipeline.rs`**：
  - `DataPipeline` 新增 `soe_recorder: Option<Arc<SoeRecorder>>` 和 `last_bool_states: RwLock<HashMap<(ElementId, String), bool>>` 字段
  - 新增 `with_soe_recorder(recorder)` builder 方法
  - 新增 `detect_soe_events()` 私有方法：对 parameter 名包含 "breaker"/"switch"/"position"/"relay" 且 value 为 0.0/1.0 的 reading 检测状态翻转，0→1 触发 `BreakerClose`，1→0 触发 `BreakerOpen`，device_id=`element_{id}`，priority=1
  - `run_once()` 在时序记录前调用 `detect_soe_events()`
- **验证**：`cargo build --workspace` 0 errors；`cargo test -p eneros-timeseries` 70 项通过（含 11 项新增 SOE 测试）；`cargo test -p eneros-api` 114 项通过（含 5 项新增 SOE handler 测试）；`cargo test -p eneros-scada` 58 项通过；`cargo clippy --workspace --all-targets` 0 errors（新增代码无警告）

#### Task 8：OpenAPI 自动文档

- **新增依赖**：`utoipa = "5"` 添加到 `[workspace.dependencies]`（`d:\eneros\Cargo.toml`）和 `eneros-api` crate 依赖
- **新增 `crates/eneros-api/src/openapi.rs`** 模块：
  - `OpenApiDoc` 结构体派生 `utoipa::OpenApi`，聚合 6 个已注解端点路径和 16 个 schema 组件
  - info 元数据：title="EnerOS API"、version="0.10.0"、description="Power-Native Agent Operating System for electrical grid control"
- **修改 `crates/eneros-api/src/lib.rs`**：导出 `pub mod openapi` 和 `pub use openapi::OpenApiDoc`
- **修改 `crates/eneros-api/src/app.rs`**：
  - 新增 `GET /api/openapi.json` 路由，返回 `OpenApiDoc::openapi()` 序列化的 OpenAPI 3.1.0 JSON
  - 新增 `GET /docs` 路由，返回嵌入 CDN Swagger UI 的 HTML 页面（指向 `/api/openapi.json`）
  - 新增 `openapi_json_handler` 和 `swagger_ui_handler` 两个 handler 函数
  - 新增 2 个测试：`test_openapi_json_endpoint`（验证 200 OK、OpenAPI 3.1.0、title、version、6 个路径存在）、`test_swagger_ui_endpoint`（验证 200 OK、HTML 含 swagger-ui 和 openapi.json 链接）
- **为关键类型添加 `#[derive(utoipa::ToSchema)]`**：
  - `types.rs`：`ApiResponse<T>`、`PowerFlowRequest`、`PowerFlowResponse`、`BusVoltageResponse`、`BranchFlowResponse`、`ScadaLatestResponse`、`ScadaReadingResponse`、`OpfRequest`、`OpfResponse`、`GenBidRequest`、`BranchLimitRequest`
  - `handlers/auth.rs`：`LoginRequest`、`LoginResponse`
  - `handlers/timeseries.rs`：`TimeseriesResponse`、`DataPointDto`；`TimeseriesQueryParams` 派生 `IntoParams`
  - `handlers/actions.rs`：新增 `StructuredActionSchema`（镜像 `eneros_core::StructuredAction`）、`StructuredActionRequestSchema`、`StructuredActionResponseSchema` 三个 schema 包装类型（因 `StructuredAction` 定义在 `eneros-core` 无法直接派生 `ToSchema`）
- **为 6 个关键 handler 添加 `#[utoipa::path(...)]` 注解**：
  - `POST /api/power-flow`（powerflow.rs）
  - `POST /api/analysis/opf`（analysis.rs）
  - `POST /api/actions/structured`（actions.rs）
  - `GET /api/scada/latest`（scada.rs）
  - `GET /api/timeseries/query`（timeseries.rs）
  - `POST /api/auth/login`（auth.rs）
- **验证**：`cargo test -p eneros-api -- --test-threads=1` 全部 110 项测试通过（含 2 项新增 OpenAPI 测试）；`cargo clippy -p eneros-api --all-targets` 无错误

### 性能优化

#### Task 5：存储级降采样基础

- **新增 `crates/eneros-timeseries/src/downsample.rs`** 模块：
  - `DownsampleLevel` 枚举（Second/Minute/Hour），`interval_ms()` 返回窗口大小，`for_range()` 根据查询时间范围自动选择粒度（≤1h→Second、≤7d→Minute、>7d→Hour）
  - `AggregatedPoint` 结构体（timestamp/avg/min/max/count/sum）
  - `DownsampledCache` 多粒度降采样缓存，以 `(element_id, parameter, level)` 为键存储聚合数据
  - `rollup()` 方法：将原始 DataPoint 按时间窗口分组（窗口对齐到整秒/整分/整时），计算 avg/min/max/count/sum，结果按时间戳排序后存入缓存
  - `query()` 方法：按时间范围过滤聚合数据点
  - `has_data()` 方法：检查指定键/粒度是否有缓存数据
  - 10 个单元测试覆盖 for_range 粒度选择、interval_ms、rollup 基本聚合（1min/1h）、窗口对齐、空输入、单点、时间范围过滤、has_data
- **修改 `crates/eneros-timeseries/src/engine.rs`**：
  - `TimeSeriesEngine` 新增 `downsample_cache: Arc<RwLock<DownsampledCache>>` 字段（使用 `parking_lot::RwLock` + `Arc` 以便后台任务与查询路径共享）
  - 两个构造函数（`new` / `with_persistent_storage`）同步初始化 `downsample_cache`
  - 新增 `rollup_now(&self, level)` 方法：同步执行一次 rollup，读取所有键的原始数据并聚合到指定粒度（适合测试和手动触发）
  - 新增 `start_rollup_task(self: Arc<Self>, shutdown_rx)` 方法：启动后台 tokio 任务，每 60s 将 1s 数据聚合为 1min，每 60min（第 60 次 tick）聚合为 1h；通过 `tokio::sync::watch` 接收 shutdown 信号优雅退出（与 v0.9.0 graceful shutdown 模式一致）
  - 新增 `query_downsampled(&self, ...)` 方法：根据查询时间范围自动选择粒度（<1h 返回原始数据转换为 AggregatedPoint、1h–7d 优先读 1min 缓存否则即时聚合、>7d 优先读 1h 缓存）
  - 6 个新增单元测试覆盖 Minute/Second/Hour 三级粒度查询、缓存未命中回退即时聚合、多键 rollup、后台任务优雅关停
- **修改 `crates/eneros-timeseries/src/lib.rs`**：导出 `downsample` 模块及 `DownsampleLevel`、`AggregatedPoint`、`DownsampledCache` 类型
- **修改 `crates/eneros-api/src/main.rs`**：
  - TimeSeriesEngine 初始化后创建 `watch::channel(false)` 并调用 `ts_engine.clone().start_rollup_task(rollup_shutdown_rx)` 启动后台 rollup 任务
  - 优雅关停序列中发送 `rollup_shutdown_tx.send(true)` 并 `rollup_handle.await` 等待任务退出
- **约束遵守**：未修改 `aggregation.rs`（查询时聚合保留为独立能力）；未修改 `sqlite_storage.rs`（降采样在内存层，不涉及持久化）
- **验证**：`cargo test -p eneros-timeseries` 全部 59 项测试通过（含 16 项新增降采样测试）；`cargo clippy -p eneros-timeseries --all-targets` 无错误无警告

#### H3：SafetyGateway per-device 锁池（Task 2）

- **`crates/eneros-gateway/src/gateway.rs`** 重构：
  - `SafetyGateway` 移除全局单锁 `execution_lock: tokio::sync::Mutex<()>`（原实现串行化所有设备的命令执行，慢设备阻塞快设备）
  - 新增 `device_locks: parking_lot::RwLock<HashMap<String, Arc<tokio::sync::Mutex<()>>>>` per-device 锁池（读多写少，锁按 device_id 懒创建）
  - 新增 `global_lock: Arc<tokio::sync::Mutex<()>>` 兜底锁（无 device_id 的命令共用，用 `Arc` 包裹以统一 `get_device_lock` 返回类型）
  - 新增 `history_lock: tokio::sync::Mutex<()>` 短持有锁（仅保护 `command_history` push，不保护设备 I/O）
  - 新增 `get_device_lock(&self, device_id: &Option<String>) -> Arc<Mutex<()>>` 方法：读锁快速路径 + 写锁慢路径插入
  - `execute_command()` 重构为：获取 per-device 锁 → validate → execute → 更新 `last_execution_result` → 释放设备锁 → 获取 `history_lock` 写入 `command_history`；不同设备命令可并发执行，同设备命令串行
  - 4 个构造函数（`new` / `with_executor` / `with_queue` / `with_queue_and_executor`）同步初始化新字段，移除 `execution_lock`
  - `validate_command()` 未修改（只读 `safety_checks`，不需要设备锁保护）
  - 保留原 `if !exec_result.success { return Err(...) }` 行为（失败命令仍写入 history 后返回错误）
  - 新增 3 个并发测试：不同设备并发执行（< 200ms）、同设备串行执行（>= 200ms）、无 device_id 兜底执行
- **验证**：`cargo test -p eneros-gateway -- --test-threads=1` 全部 133 项测试通过（含 3 项新增并发测试）；`cargo clippy -p eneros-gateway --all-targets` 无错误

#### M5：PipelineStatistics 原子化（Task 1）

- **`crates/eneros-gateway/src/pipeline_types.rs`** 重构：
  - `PipelineStatistics` 所有 `u64` 计数字段改为 `std::sync::atomic::AtomicU64`，移除 `Clone` derive（`AtomicU64` 不可 `Clone`）
  - 新增 `PipelineStatisticsSnapshot` 结构体（全部 `u64` 字段，字段名与原结构体一致，保证 JSON 序列化向后兼容）
  - `record_decision(&mut self, ...)` → `record_decision(&self, ...)`，使用 `fetch_add` / `fetch_max` + `Ordering::Relaxed` 更新计数器
  - 新增 `reset(&self)` 方法（`store(0, Relaxed)` 重置全部字段）
  - 新增 `snapshot(&self) -> PipelineStatisticsSnapshot` 方法（`load(Relaxed)` 一次性读取所有字段）
  - 实现 `Default`（所有 `AtomicU64` 初始化为 0）
  - 新增 5 个单元测试：默认值、延迟统计、重置、8 线程 × 1000 次并发 `fetch_add` 计数正确性、并发更新下 `snapshot()` 不 panic
- **`crates/eneros-gateway/src/decision_pipeline.rs`** 重构：
  - `statistics: RwLock<PipelineStatistics>` → `statistics: PipelineStatistics`（直接持有，移除 `RwLock` 包裹）
  - 移除 `use parking_lot::RwLock` 导入（该 crate 其他模块仍使用 `parking_lot`，依赖保留）
  - rollback 路径原 3 次连续写锁（`postcondition_failures` / `rollbacks_triggered` / `rollbacks_succeeded|failed`）改为 3 次独立 `fetch_add(1, Relaxed)`，无锁争用
  - `record_stats` 方法改为直接调用原子方法
  - `statistics()` 公共方法返回类型由 `PipelineStatistics` 改为 `PipelineStatisticsSnapshot`
  - `reset_statistics()` 改为调用 `self.statistics.reset()`
- **`crates/eneros-gateway/src/lib.rs`** 导出新增 `PipelineStatisticsSnapshot`
- **验证**：`cargo test -p eneros-gateway -- --test-threads=1` 全部 130 项测试通过（含 5 项新增并发测试）；`cargo clippy -p eneros-gateway --all-targets` 无错误；下游 `eneros-network` e2e 测试编译通过

#### T6：CIM→PowerNetwork 转换器（Task 6）

- **`crates/eneros-network/src/cim.rs`** 新增 `cim_to_power_network()` 转换函数（约 270 行）：
  - 新增 `CimTopology<'a>` 辅助结构体（持有 `bus_id_by_mrid`、`cn_to_bus_id`、`equip_terminals` 反向映射），提供 `resolve_terminal()`、`resolve_equipment_buses()`、`resolve_equipment_bus()`、`nominal_voltage()` 方法
  - 拓扑解析：按 mRID 排序为 `BusbarSection` 分配确定性 1-based `ElementId`；扫描所有 `Terminal` 构建 equipment→terminals 反向映射（CIM 标准中 Terminal 顶层引用 ConductingEquipment，需反向查找）；构建 ConnectivityNode→bus_id 映射
  - 支路构建：`ACLineSegment`（r/x/bch 物理值→标幺值，Z_base = V_base² / S_base，S_base=100MVA）、`PowerTransformer`（扫描 `power_transformer_ends` 按 `transformer_mrid` 过滤，求和各绕组阻抗，因解析器未填充 `power_transformer_end_mrids` 字段）、`Breaker`/`Disconnector`（闭合开关用 1e-6 小阻抗以出现在 Y-Bus，断开开关用 0.0 被 `YBusMatrix::from_branches` 跳过）
  - 注入量构建：`SynchronousMachine` 生成正 P/Q 注入 `p_spec`/`q_spec`；`EnergyConsumer` 生成负 P/Q（负荷以负注入表示）；`LinearShuntCompensator` 导纳叠加到 Y-Bus 对角线
  - 母线类型分配：首台发电机母线=Slack，其余发电机母线=PV，无发电机时首母线=Slack，其余=PQ
  - 标幺转换常量 `CIM_BASE_MVA = 100.0`；缺失电压数据回退 110kV 默认值
  - 错误处理：无 `BusbarSection` 返回 `Err`；支路/发电机/负荷/并联器无法解析母线返回带 mRID 的描述性错误
  - 新增 11 个单元测试（使用 `SAMPLE_CIM_TOPO` 3 节点测试拓扑：1 线路 + 1 变压器 + 1 断路器 + 1 隔离开关 + 1 发电机 + 2 负荷 + 1 并联器，全部经 Terminal/ConnectivityNode 连接）：母线数、支路数、发电机数、负荷反映到 p_spec、支路拓扑、母线类型、发电机规格、支路 ID、潮流收敛、空模型错误、无 Terminal 错误
- **`crates/eneros-network/src/network.rs`** 新增两个 builder 方法：
  - `with_generators(Vec<GeneratorSpec>)`：供 CIM 转换器等外部导入器设置发电机表
  - `with_branch_ids(Vec<ElementId>)`：供导入器设置显式支路 ID（而非默认 1..=n 序列）
- **`crates/eneros-network/src/lib.rs`** 导出 `cim_to_power_network`
- **验证**：`cargo test -p eneros-network` 全部 40 项单元测试通过（含 11 项新增转换器测试）；`cargo clippy -p eneros-network --all-targets` 无 eneros-network 警告
- **约束遵守**：未修改 `main.rs`（CIM 加载路径接线由后续任务完成）；未修改 `eneros.toml`（配置字段添加由后续任务完成）

#### T3：时序配置接线（Task 3）

- **`crates/eneros-api/src/main.rs`** 时序引擎初始化从硬编码改为配置驱动：
  - 新增 `compute_retention_capacity(retention_days, sampling_interval_ms)` 函数：按 `retention_days × 86400 × 1000 / sampling_interval_ms` 计算每点序列最大容量，上限 10,000,000（1000 万点）防止内存溢出
  - `TimeSeriesEngine::with_sqlite()` 的 `max_retention` 参数从硬编码 10000 改为 `compute_retention_capacity(config.timeseries.retention_days, config.timeseries.sampling_interval_ms)` 计算值
  - 新增 6 个单元测试覆盖 retention 计算：默认值（7天/1000ms=604800点）、30天长周期、100ms高频采样、0天边界、上限保护、配置缺失回退
- **`eneros.toml`** `[timeseries]` 段注释更新：说明 retention_days 与 sampling_interval_ms 如何影响内存容量，标注 1000 万点上限
- **验证**：`cargo test -p eneros-api` 全部测试通过（含 6 项新增 retention 计算测试）

#### M8：Dashboard SVG data-* 属性修复（Task 7）

- **`crates/eneros-dashboard/src/topology_svg.rs`** 修复 SVG 元素缺少 `data-*` 属性导致前端热力图 overlay 无法定位的问题：
  - branch `<line>` 元素新增 `data-branch-id="{branch.id}"` 属性
  - bus `<circle>` 元素新增 `data-bus-id="{bus.id}"` 属性
  - bus `<text>` 标签元素新增 `data-bus-id="{bus.id}"` 属性
  - 2 个新增单元测试验证生成的 SVG 包含 `data-bus-id` 和 `data-branch-id` 属性
- **验证**：`cargo test -p eneros-dashboard` 全部 35 项测试通过（含 2 项新增 data-* 属性测试）

#### T6.8：CIM 加载路径接线

- **`crates/eneros-api/src/main.rs`** `build_cim_network()` 函数从约 270 行手动转换简化为 30 行：
  - 复用 `NetworkConfig.path` 字段作为 CIM 文件路径（无需新增 `cim_file` 配置字段）
  - 调用 `eneros_network::parse_cim()` 解析 CIM XML
  - 调用 `eneros_network::cim_to_power_network()` 转换为 PowerNetwork
  - 启动日志输出解析统计（busbar/line/transformer/generator/load 数量）和转换结果（bus/branch 数量）
- **验证**：`cargo build --workspace` 通过，`source = "cim"` 配置路径生效

---

## [0.9.0] - 2026-06-18

### 交付级运维与可观测性补全（配置热重载 + 分布式追踪 + DualScanGroup 修复 + 容器化部署 + CI/CD）

> **设计目标**：v0.9.0 聚焦交付级运维能力补全，解决配置热重载、分布式追踪、SCADA 双扫描组生命周期管理、容器化部署和 CI/CD 流水线，使 EnerOS 达到生产可部署状态。

#### M11：DualScanGroup 生命周期修复

- **`crates/eneros-scada/src/dual_scan.rs`** 重写：
  - `DualScanHandles` 新增 `async fn shutdown(self)` 方法，基于 `tokio::sync::watch` 信号实现优雅关停（发送信号 → 等待当前采集周期完成 → join）
  - 实现 `Drop` trait 防止后台任务泄漏（drop 时自动发送关停信号）
  - 新增 `DualScanOptions` 结构体（`timeout_ms`、`enable_quality_check`、`event_bus`），消除硬编码
  - 新增 `DualScanGroup::auto_classify_with_intervals()` 方法，支持从配置传入 fast/normal 间隔
  - `classify_point` 移除 `current` 从快速组分类（电流为测量量而非保护信号）
  - `start_dual_scan` 现在接受 `DualScanOptions`，dual scan pipeline 现在发布 `DataReceived` 事件
  - 新增 `test_dual_scan_shutdown_graceful` 集成测试验证优雅关停
- **`crates/eneros-scada/src/pipeline.rs`** 增强：
  - 新增 `start_with_shutdown(interval_ms, shutdown_rx)` 方法，支持 `tokio::select!` 监听关停信号
  - `start()` 保持向后兼容（内部创建永不触发的 watch channel）
  - 关停时完成当前采集周期后再退出，避免时序数据写入中断
- **`crates/eneros-api/src/main.rs`** 修复：
  - 共享 `data_source: Arc<dyn DataSource>` 避免重复创建 IEC 104 TCP 连接
  - 移除重复的主 pipeline 后台任务（dual scan 覆盖全部测点，主 pipeline Arc 保留供 `run_once()` 使用）
  - 从 `config.scada.fast_interval_ms` / `normal_interval_ms` 读取间隔（消除 100ms/1000ms 硬编码）
  - 优雅关停使用 `dual_scan_handles.shutdown().await` 替代 `abort()`

#### M9：配置热重载

- **`crates/eneros-api/src/config_reload.rs`** 新增模块：
  - `SharedConfig = Arc<parking_lot::RwLock<EnerOSConfig>>` 共享配置句柄类型
  - `ConfigWatcher` 基于轮询的文件监听（2 秒检查 mtime，避免外部依赖）
  - `reload_from_file()` 安全字段热重载：`log_level`（立即生效）、`enable_metrics`、`scada.*_interval_ms`、`emergency.*` 阈值、`powerflow.tolerance/max_iterations`
  - 不安全字段（`api.host/port`、`api.tls_*`、`network.*`、`devices`、`scada.source`、`security.jwt_secret`、`eventbus.max_queue_size`）标记为 skipped
  - `ReloadResult` 返回 applied_fields 和 skipped_fields 列表
- **`crates/eneros-api/src/handlers/config_reload.rs`** 新增 handler：
  - `POST /api/config/reload` — 手动触发配置重载
  - `GET /api/config` — 查看运行时配置（`jwt_secret` 和 `api_keys` 脱敏）
- **`crates/eneros-api/src/app.rs`** 扩展：
  - `AppState` 新增 `shared_config` 和 `config_watcher` 字段
  - 新增 `with_shared_config()` 和 `with_config_watcher()` builder 方法
- **`crates/eneros-api/src/main.rs`** 集成：
  - 启动时包装 config 为 `SharedConfig`，启动 `ConfigWatcher` 后台任务
  - 优雅关停时停止 config watcher

#### M10：分布式追踪基础

- **`crates/eneros-core/src/config.rs`** 扩展 `ObservabilityConfig`：
  - 新增 `otel_endpoint: Option<String>` 字段（OTLP 导出端点）
  - 新增 `otel_service_name: String` 字段（默认 "eneros"）
  - 新增对应环境变量覆盖：`ENEROS_OBSERVABILITY__OTEL_ENDPOINT`、`ENEROS_OBSERVABILITY__OTEL_SERVICE_NAME`
- **`crates/eneros-api/src/main.rs`** tracing 初始化增强：
  - `enable_tracing=true` 时启用 `FmtSpan::NEW | FmtSpan::CLOSE` span 事件记录到 JSON 日志
  - 启动时日志输出 tracing 配置（otel_endpoint、service_name）
- **`crates/eneros-api/src/handlers/`** 添加 `#[tracing::instrument]` 注解：
  - `auth.rs::login_handler` — 登录链路追踪
  - `powerflow.rs::power_flow_handler` — 潮流计算链路追踪
  - `analysis.rs::opf_handler` / `state_estimation_handler` / `short_circuit_handler` / `ac_opf_handler` / `transient_handler` — 分析链路追踪

#### F9/S1-S4：容器化部署与 CI/CD

- **`Dockerfile`** 新增：多阶段构建（rust:1.95-bookworm 构建 → debian:bookworm-slim 运行），非 root 用户，健康检查
- **`docker-compose.yml`** 新增：EnerOS 核心服务 + 可选 Jaeger（tracing profile）+ Prometheus + Grafana（monitoring profile），持久化卷
- **`.github/workflows/ci.yml`** 新增：build-test / clippy / fmt / docker-build 四个 job，cargo 缓存
- **`deploy/prometheus.yml`** 新增：Prometheus scrape 配置
- **`scripts/dev.sh`** 新增：开发模式启动脚本
- **`scripts/build.sh`** 新增：生产构建脚本（编译 + 测试 + Docker 镜像）
- **`scripts/healthcheck.sh`** 新增：健康检查脚本
- **`docs/deployment.md`** 新增：完整部署运维指南（Docker 部署、配置管理、热重载、可观测性、SCADA 采集、安全、故障排查）

#### 其他修复

- **`crates/eneros-api/src/handlers/dashboard.rs`** 修复 `test_build_svg_data_empty_state` 测试断言（fallback 到 IEEE 14 数据时 buses 非空）
- **`crates/eneros-api/src/main.rs`** 修复 clippy `for_kv_map` 警告（`for (_mrid, x) in &map` → `for x in map.values()`）

---

## [0.8.0] - 2026-06-18

### 分析精度进阶（稀疏线性代数 + AC-OPF + 暂态稳定 + 状态估计增强 + 不对称短路 + 开关物理建模 + 5 个新 API 端点）

> **设计目标**：v0.8.0 聚焦分析精度进阶，从 DC-OPF 升级到 AC-OPF，补全暂态稳定分析、不良数据检测、可观测性分析、不对称短路计算，实现开关动作物理建模，并通过 5 个新 API 端点将所有分析能力暴露给调度决策场景。
>
> **验证结果**：1564 个测试通过（0 失败），0 clippy 错误，`cargo build --workspace` 成功。IEEE-118 潮流 17.15ms < 100ms，IEEE-14 AC-OPF 168.2μs < 500ms。

#### T1：稀疏线性代数层（eneros-linalg crate）

- **新增 `crates/eneros-linalg/`**：基于 `sprs::CsMat` 的稀疏矩阵库
  - `SparseMatrix` 类型支持复数（`Complex64`），封装 CSR 存储
  - 稀疏 LU 分解（列主元 pivoting + `SymbolicFactorization` 符号分解缓存）
  - 稀疏 Cholesky 分解（用于对称正定矩阵，如 SE 增益矩阵）
  - 稀疏矩阵-向量乘法、转置、矩阵-矩阵加法
  - 8 个单元测试覆盖构造、LU、Cholesky、SpMV、符号缓存复用、奇异矩阵检测

#### T2：YBusMatrix 稀疏存储重构

- **修改 `crates/eneros-powerflow/src/matrix.rs`**：Y-Bus 内部存储迁移到稀疏 CSR
  - `to_csr()` 方法返回 `CsMat<Complex64>` 视图，供稀疏求解器使用
  - 公共 API 向后兼容：`new(size)`、`get(i,j)`、`set(i,j,g,b)`、`add_branch()`
  - `eneros-powerflow/src/solver.rs` 牛顿-拉夫逊求解器集成稀疏 LU 求解
  - 性能基准测试 `test_perf_ieee118_scale`：IEEE-118 规模（118 节点、180 支路、352 非零元）CSR 转换 + LU 求解 17.15ms < 100ms

#### T3：AC-OPF 交流最优潮流求解器

- **新增 `crates/eneros-analysis/src/ac_opf.rs`**：完整的 AC-OPF 求解器实现
  - **T3.1 类型定义**：`AcGenerator`（含 P/Q 上下限和二次成本曲线）、`AcBranch`（含 R/X/B/变比/视在功率限额）、`AcBus`（含负荷和电压上下限）、`AcOpfProblem`、`AcOpfResult`、`OpfMethod` 枚举（NewtonRaphson/InteriorPoint）
  - **T3.2 牛顿-拉夫逊法 AC-OPF**：极坐标形式潮流求解，含 Y-Bus 导纳矩阵构建（支持变压器变比）、功率不平衡方程（P/Q 注入）、完整 4 分块雅可比矩阵（H/N/M/L 子矩阵）、迭代求解（最大 50 次，容差 1e-6）、经济调度初值、平衡机出力调整
  - **T3.3 原对偶内点法**：日志障碍函数处理不等式约束（电压/出力边界），障碍参数 μ 自适应衰减（0.5 倍率），线搜索保证可行域，最大 50 次迭代
  - **T3.4 LMP 节点边际电价计算**：能量分量（边际发电机成本）+ 阻塞分量（支路越限影子价格）+ 损耗分量（网损灵敏度），公共接口 `compute_lmp()` 可从求解结果重算
  - **T3.5 SCOPF N-1 安全约束**：基态 OPF + 逐支路故障扫描，发现越限则调整送/受端发电机出力，最大 3 轮迭代
  - **T3.6 简化机组组合**：按时段独立求解 AC-OPF，支持多时段负荷曲线输入
  - **T3.7-T3.9 验证测试**：16 个单元测试覆盖 2 母线系统、类 IEEE 14 节点系统、LMP 计算、内点法收敛、SCOPF N-1、机组组合、Y-Bus 构建（含变比）、经济调度、潮流收敛、雅可比矩阵、支路潮流、约束检查、无效问题、方法分发、求解器构建器、**IEEE-14 AC-OPF 性能 < 500ms（实测 168.2μs）**

#### T4-T5：暂态稳定分析

- **新增 `crates/eneros-analysis/src/transient_stability.rs`**：完整暂态稳定分析模块
  - **T4 发电机模型**：经典二阶模型（摇摆方程 `M·dδ/dt = Pm - Pe - D·dδ/dt`）、四阶模型（含 AVR 励磁调节）
  - **T4 积分器**：RK4（龙格-库塔 4 阶）显式积分器 + 隐式梯形积分器（用于刚性系统）
  - **T4 故障建模**：故障期间/故障清除后 Y-Bus 修改 + 网络方程求解
  - **T5 CCT 计算**：临界故障清除时间二分搜索算法
  - **T5 等面积法则**：单机无穷大系统快速稳定性判定，解析求解临界清除功角 δ_c
  - **T5 连续潮流（CPF）**：预测-校正步长控制、鼻点检测、PV 曲线追踪
  - **T5 电压稳定模态分析**：雅可比矩阵奇异值分解
  - 验证测试覆盖等面积法则、CCT 计算、暂态仿真收敛性、参数校验

#### T6-T7：状态估计增强

- **新增 `crates/eneros-analysis/src/bad_data.rs`**：不良数据检测模块
  - 最大标准残差法（LNR）：残差灵敏度矩阵、归一化残差 r^N 计算
  - χ² 假设检验（显著性水平可配置，默认 0.05）
  - 迭代剔除算法：自动识别并剔除坏数据，最大轮数可配置
  - 拓扑错误辨识（基于残差分析）
  - `build_state_vector()` 公开供 API handler 调用
- **新增 `crates/eneros-analysis/src/observability.rs`**：可观测性分析模块
  - 数值法：雅可比矩阵秩分析，识别不可观测母线
  - 拓扑法：图论 BFS/DFS 可观测性判定
  - 最小 PMU 配置建议（贪心算法，最大化覆盖范围）
- **增强 `crates/eneros-analysis/src/state_estimation.rs`**：
  - PMU 测量支持（`MeasType::PmuVoltage`、`PmuCurrent`），扩展雅可比为实部+虚部双行
  - PMU 线性状态估计（`estimate_pmu_linear()`，直接求解无需迭代）
  - 变压器分接头估计（`estimate_with_tap()`，扩展状态向量）
  - `build_jacobian_network()` 公开供 API handler 调用
  - Tikhonov 正则化保证增益矩阵非奇异

#### T8：不对称短路分析

- **增强 `crates/eneros-analysis/src/short_circuit.rs`**：不对称故障分析
  - `SequenceNetworks` 类型：正序/负序/零序 Z-bus 矩阵构建
  - SLG（单相接地）故障分析：三序网络串联
  - LL（两相短路）故障分析：正负序并联
  - DLG（两相接地）故障分析：三序网络组合
  - 动态短路（发电机暂态电抗 x'd 代替同步电抗 xd）
  - 故障电流、各序电压、各母线电压全面计算
  - 验证测试覆盖 SLG/LL/DLG 三种故障类型

#### T9：开关动作物理建模

- **增强 `crates/eneros-network/src/simulator.rs`**：`NetworkSimulatorAdapter` 开关建模
  - `simulate_with_opened_branches()`：断开指定支路 → 修改邻接矩阵 → 重建 Y-Bus → 重新潮流计算
  - `ExecuteDevice{operation="open"/"close"}` 物理建模（替换 `conservative_switching_reject`）
  - `IsolateFault` 动作物理建模：断开故障支路上游开关 + 重新潮流
  - `CloseTieSwitch` 动作物理建模：合上联络开关 + 重新潮流
  - `conservative_switching_reject()` 标记为 `#[deprecated]`
  - 验证测试覆盖开关开合、故障隔离、联络开关闭合、未知支路拒绝

#### T10：API 端点扩展（5 个新端点）

- **修改 `crates/eneros-api/src/handlers/analysis.rs`**：新增 5 个分析端点
  - `POST /api/analysis/ac-opf`：AC-OPF 求解（NewtonRaphson / InteriorPoint 方法可选），支持从已加载网络模型或请求自定义数据构建问题
  - `POST /api/analysis/transient`：暂态稳定分析（simulate / cct / equal_area 三种模式），支持 RK4 和隐式梯形积分
  - `POST /api/analysis/observability`：可观测性分析（numerical / topological 方法），可选 PMU 最优配置建议
  - `POST /api/analysis/bad-data`：不良数据检测（χ² 检验 + LNR），可选迭代剔除
  - `POST /api/analysis/short-circuit/asymmetric`：不对称短路分析（SLG / LL / DLG）
- **修改 `crates/eneros-api/src/app.rs`**：注册 5 个新路由
- **修改 `crates/eneros-api/src/types.rs`**：新增 ~450 行请求/响应类型定义
- **新增 `crates/eneros-api/tests/e2e_v08_analysis.rs`**：18 个集成测试覆盖全部 5 个端点的成功路径、错误路径和边界情况
- **修复 `build_synthetic_measurements()`**：修复 `idx_to_bus.remove()` 导致支路测量丢失母线映射的 bug，改为只读 `get()` 查询

#### T11：编译 + 测试 + Clippy 验证

- `cargo build --workspace` 成功（0 错误）
- `cargo test --workspace -- --test-threads=1` 全部通过：**1564 个测试通过，0 失败**
- `cargo clippy --workspace --all-targets` 0 错误（需 `CARGO_INCREMENTAL=0` 避免 rustc 1.95.0 Windows 增量编译 ICE）
- 性能基准：IEEE-118 潮流 17.15ms < 100ms ✓，IEEE-14 AC-OPF 168.2μs < 500ms ✓
- 测试总数 1564 ≥ 1550 ✓（v0.7.0 基线 1456 + 新增 108）

---

## [0.7.0] - 2026-06-17

### 协议覆盖完善（新增 4 个协议适配器 + 增强 2 个协议 + 设备发现智能化 + CIM 导入 + v0.6.0 推迟项）

> **设计目标**：v0.7.0 聚焦协议覆盖完善，补全 GOOSE/SV/OPC UA/DNP3 四个主流工业协议适配器，增强 IEC 104/61850 功能完整性，实现设备发现智能化（多协议端口探测+握手识别），新增 CIM 模型导入支持，并完成 v0.6.0 推迟的 TLS 运行时、WatchdogTimer 管线集成、补齐 7 个 API 端点、TraceLayer HTTP 追踪、结构化 JSON 日志等可观测性增强。
>
> **验证结果**：1456 个测试通过（0 失败），0 clippy 错误，`cargo build --workspace` 成功。

#### T1：GOOSE 协议适配器（Layer 2 以太网多播）

- **新增 `device/src/adapters/goose.rs`**：IEC 61850-8-1 GOOSE 协议适配器
  - Layer 2 以太网多播通信（MAC 01-0C-CD-01-00-00 ~ 01-0C-CD-04-00-00）
  - GOOSE PDU 解析：AppID、GoCBRef、DataSetRef、T（时间戳）、StNum/SqNum/NumDatSetEntries
  - GoCB（GOOSE Control Block）管理：enable/disable/subscribe
  - 数据集映射到 `DataValue`（支持 Boolean/Integer/Float/MV/Quality）
  - `MockGooseTransport` 用于测试（基于 tokio mpsc channel）
  - 8 个单元测试覆盖 PDU 解析、GoCB 管理、订阅机制

#### T2：SV 采样值协议适配器（IEC 61850-9-2 LE）

- **新增 `device/src/adapters/sv.rs`**：IEC 61850-9-2 LE 采样值传输协议
  - SV PDU 解析：noASDU、seqNum、refrTm、smpCnt
  - 4 通道/8 通道 ASDU 支持（IEC 61850-9-2 LE 80 点/周波）
  - 通道映射：电压瞬时值（V）、电流瞬时值（A）
  - `SvSubscriber` 订阅机制：多通道同步采样
  - `to_engineering()` 工程值转换（支持变比配置）
  - 6 个单元测试覆盖 PDU 解析、通道映射、工程值转换

#### T3：OPC UA 客户端适配器

- **新增 `device/src/adapters/opcua.rs`**：OPC UA 客户端适配器
  - 节点 ID 解析（Numeric/String/Guid/ByteString 四种格式）
  - `OpcUaConfig`：endpoint_url、security_policy、security_mode、用户名/密码认证
  - 节点浏览（Browse）、属性读取（Read）、订阅（Subscribe）、方法调用（Call）
  - `OpcUaClient`：连接管理、节点缓存、订阅回调
  - `OpcUaNodeId` 实现 `Display` trait（标准 OPC UA 地址格式）
  - 12 个单元测试覆盖节点 ID 解析、配置、浏览、读取

#### T4：DNP3 适配器（Class 0/1/2/3 + CROB）

- **新增 `device/src/adapters/dnp3.rs`**：DNP3 客户端适配器
  - DNP3 链路层/应用层帧解析（IEC 60870-5）
  - Class 0/1/2/3 事件扫描（Integrity Poll + Event Scan）
  - CROB（Control Relay Output Block）控制输出
  - `Dnp3Config`：master_address、source_address、timeout
  - `Dnp3Client`：连接管理、数据轮询、命令执行
  - 10 个单元测试覆盖帧解析、Class 扫描、CROB 命令

#### T5：IEC 104 增强（双点/步位置/BCR/时钟同步/参数下装/冗余/TLS）

- **修改 `device/src/adapters/iec104/client.rs`**：
  - 新增 ASDU 类型：DoublePoint(3)、StepPosition(5)、BCR(8)、DoubleCommand(46)、ClockSync(103)、ParameterFloat(112)、ParameterScaled(111)
  - 新增 `TlsConfig`：IEC 62351-3 TLS 安全传输（client_cert/client_key/ca_bundle/server_name）
  - 新增 `RedundancyMode`：Single/ActiveStandby/DualActive 双机冗余
  - 新增方法：`send_double_command()`、`send_clock_sync()`、`send_parameter_float()`、`send_parameter_scaled()`
  - 新增方法：`active_connection()`、`switch_to_secondary()`、`build_tls_connector()`
  - TLS 连接器使用 `rustls::ClientConfig` + webpki-roots 或自定义 CA
  - 8 个新测试覆盖 TLS 配置、冗余模式、切换逻辑
- **修改 `device/src/adapters/iec104/mod.rs`**：
  - `info_object_to_value_quality()` 新增 DoublePoint/StepPosition/BCR 匹配分支
  - 导出 `TlsConfig` 和 `RedundancyMode`

#### T6：IEC 61850 增强（RCB/SCL/数据集/控制服务）

- **新增 `device/src/adapters/iec61850/rcb.rs`**：报告控制块管理
  - `TrgOp` 位掩码：dchg/qchg/dupd/period/gi
  - `RcbType`：URCB（非缓存）/ BRCB（缓存）
  - `RcbManager`：register/enable/disable/reserve/set_trg_op/set_integrity_period/receive_report
  - 10 个单元测试
- **新增 `device/src/adapters/iec61850/scl.rs`**：SCL 文件解析（IEC 61850-6）
  - 最小 XML 解析器：`extract_element()`、`extract_all_elements()`、`extract_attr()`
  - 解析：SclHeader、Substation、IED、LogicalDevice、LogicalNode、DataSet、RcbDef、GoCbDef
  - `parse_scl()` 返回 `SclDocument`，`all_object_refs()` 生成 MMS 对象引用列表
  - 支持自闭合标签（`<tag .../>`）
  - 13 个单元测试
- **新增 `device/src/adapters/iec61850/control.rs`**：SBO 控制服务
  - `ControlState`：Idle/Selected/SelectedWithValue/Operated/Failed
  - `ControlMode`：Direct/SboNormal/SboEnhanced
  - `ControllableCdc`：SPC/DPC/APC/BSC/ISC
  - `ControlService`：register/select/select_with_value/operate/cancel/reset/state
  - SBO 超时检查（`Instant::elapsed()`）
  - 13 个单元测试
- **新增 `device/src/adapters/iec61850/dataset.rs`**：数据集管理
  - `FunctionalConstraint` 枚举（15 个变体：ST/MX/SP/SV/CF/DC/SG/SE/SR/OR/CO/US/GO/RP/LG）
  - `FcdaRef` 解析 `LD/LN.DO.DA.FC` 格式
  - `DataSetManager`：register_static/create_dynamic/delete_dynamic/get/list/set_values/get_values
  - 14 个单元测试

#### T7：设备发现智能化（多协议端口探测+握手识别）

- **修改 `device/src/discovery.rs`**：
  - `DiscoveredDevice` 新增 `confidence: u8` 和 `detected_protocols: Vec<ProtocolType>`
  - `DiscoveryConfig` 新增 `protocols: Vec<ProtocolType>` 和 `handshake_identify: bool`
  - `ProtocolSignature`：6 个协议签名（Modbus/502、IEC104/2404、IEC61850/102、OPC UA/4840、DNP3/20000、MQTT/1883）
  - `probe_device_smart()`：尝试所有签名，选择最高置信度
  - `probe_protocol()`：发送探测帧，读取响应，匹配预期
  - `create_connection_config()`：支持所有协议类型的正确 `ProtocolConfig` 变体
  - 11 个新测试

#### T8：CIM 模型导入（IEC 61968/61970）

- **新增 `network/src/cim.rs`**：CIM RDF/XML 解析器
  - 14 个 CIM 数据结构：CimBaseVoltage、CimSubstation、CimVoltageLevel、CimBusbarSection、CimAcLineSegment、CimPowerTransformer、CimPowerTransformerEnd、CimSynchronousMachine、CimEnergyConsumer、CimLinearShuntCompensator、CimTerminal、CimConnectivityNode、CimBreaker、CimDisconnector
  - `CimModel`：HashMap 集合管理各类 CIM 对象
  - `parse_cim()`：使用最小 XML 解析器提取元素和属性
  - 辅助函数：`extract_mrid()`、`extract_reference()`、`parse_float()`、`parse_bool()`
  - 13 个单元测试

#### T9：v0.6.0 推迟项（TLS 运行时 + WatchdogTimer 管线集成 + 补齐 API 端点）

- **WatchdogTimer 管线集成**（`gateway/src/decision_pipeline.rs`）：
  - `ConstrainedDecisionPipeline` 新增 `watchdog` 和 `command_timeout` 字段
  - 新增 `with_watchdog()` 构建器方法
  - Stage 5 执行循环中为每个命令注册 `WatchdogGuard`，超时触发回调
  - Guard 在命令完成时自动取消（RAII 语义）
- **TLS 运行时接线**（`api/src/server.rs` + `api/src/main.rs`）：
  - `ApiServer` 新增 `tls: Option<TlsConfig>` 字段和 `with_tls()` 方法
  - TLS 路径使用 `axum_server::bind_rustls()` + `rustls::ServerConfig`
  - CLI 新增 `--tls-cert` / `--tls-key` 参数
  - 证书/密钥 PEM 加载使用 `rustls_pemfile`
- **补齐 7 个 API 端点**（`api/src/handlers/`）：
  - `GET /api/audit` — 审计日志查询（支持 actor/result 过滤 + limit）
  - `POST /api/whatif` — WhatIf 假设计算（FeasibilityProjector）
  - `POST /api/validation/check` — 系统级校验（GB/T 12325/15945/14549/12326/38306/15544）
  - `POST /api/compliance/check` — 设备合规检查（GB/T 6451 变压器/电缆/开关）
  - `POST /api/planning/evaluate` — 配网规划评估（DL/T 5729 A/B/C/D/E 类供电区）
  - `POST /api/agents/{id}/control` — Agent 控制（start/stop/pause/resume）
  - `GET /api/log-level` + `POST /api/log-level` — 动态日志级别调整
- **TraceLayer HTTP 追踪**（`api/src/app.rs`）：
  - 添加 `tower_http::trace::TraceLayer::new_for_http()` 记录所有 HTTP 请求
- **结构化 JSON 日志**（`api/src/main.rs`）：
  - CLI 新增 `--json-log` 参数，启用 `tracing_subscriber::fmt().json()` 输出
- **新增依赖**：
  - `axum-server = { version = "0.7", features = ["tls-rustls"] }`
  - `tokio-rustls = "0.26"` / `rustls = "0.23"` / `rustls-pemfile = "2"`
  - `tower-http` 启用 `trace` feature
  - `tracing-subscriber` 启用 `json` + `env-filter` feature
  - `log = "0.4"` / `serde_urlencoded = "0.7"`

#### 其他修复

- **修复 OPC UA `OpcUaNodeId.to_string()` 遮蔽 `Display` trait**：重命名为 `to_address_string()`，`Display` 实现内联格式化逻辑
- **修复 `discovery.rs` 未使用变量**：移除 `best_banner`
- **修复 `validation.rs` 测试**：`serde_json::from_str` 返回 `Result`，需 `.unwrap()`
- **修复 clippy `approximate_constant` 错误**：`goose.rs` 和 `opcua.rs` 测试中的 `3.14` 替换为 `1.5`（遵循项目约定，避免触发 PI 近似值 lint）

---

## [0.6.0] - 2026-06-17

### 生产加固（修复 6 个严重差距 S1/S2/S3/S4/S6/S7）

> **设计目标**：v0.6.0 聚焦生产部署能力补齐，将 v0.5.0 的"功能完整但不可运维"升级为"可部署、可监控、可认证、可恢复"的生产级系统。所有改动均为新增模块和向后兼容的扩展（serde default 保证旧配置兼容），不破坏 v0.5.0 的 API 和配置。
>
> **架构重构任务（M1/M2/M3）经评估后推迟到 v0.7.0**，以保持当前版本的稳定性。

#### S2 修复：配置系统接线（env 覆盖 + 校验）

- **新增 `ApiConfig` / `SecurityConfig` / `ObservabilityConfig` 三个配置节**（`eneros-core::config`）
  - 全部带 `#[serde(default)]`，向后兼容 v0.5.0 的 `eneros.toml`
  - `ApiConfig`：host / port / request_timeout_ms / max_body_size / enable_cors
  - `SecurityConfig`：enable_auth / jwt_secret / token_ttl / enable_api_key / api_key / enable_rbac / enable_tls / tls_cert_path / tls_key_path
  - `ObservabilityConfig`：enable_metrics / metrics_path / enable_tracing / log_level / enable_audit_log / audit_log_path
- **新增 `ConfigError` 枚举**：`EnvOverrideFailed` / `ValidationFailed`（多错误聚合）
- **新增 `apply_env_overrides()`**：扫描 35+ 个 `ENEROS_*` 环境变量，按 `ENEROS_<SECTION>__<FIELD>` 模式覆盖配置
  - 使用 `parse_toml_value<T>()` 泛型解析器，自动识别字符串/数字/布尔
  - 字符串值自动加引号，布尔和数字直接传递
- **新增 `validate()`**：15+ 条校验规则
  - network.source 必须是 ieee14/cnpower/cim
  - scada.source 必须是 simulated/iec104/modbus
  - 数值范围校验（max_iterations > 0、intervals > 0、ttl > 0）
  - 认证校验：enable_auth=true 时 jwt_secret 必填
  - TLS 校验：enable_tls=true 时证书路径必填
  - 日志级别校验：必须是 trace/debug/info/warn/error
- **新增 `load_with_env_overrides()`**：一站式加载（文件 → env 覆盖 → 校验）
- 22 个单元测试覆盖校验、env 覆盖、TOML 解析、往返

#### S3 修复：可观测性体系（Metrics + Audit）

- **新增 `MetricsRegistry`**（`eneros-api::handlers::metrics`）
  - `Counter`：单调递增计数器，支持 `inc()` / `inc_by()` / `with_labels()` / `to_prometheus()`
  - `Gauge`：瞬时值仪表，`set()` 使用 AtomicU64 存储 f64 的位模式
  - `Histogram`：分桶直方图，`observe()` / `observe_duration()`，桶计数已累积（Prometheus 约定）
  - 全部 EnerOS 指标：commands_success/failed、command_duration、command_queue_depth、constraint_violations（voltage/thermal/frequency）、agent_decisions、device_connections、powerflow_iterations、pipeline_stage_duration、http_requests_total/duration
  - `metrics_handler`：`GET /metrics` 导出 Prometheus 文本格式
  - 10 个单元测试
- **新增 `AuditLog`**（`eneros-api::audit`）
  - `AuditEntry`：id / timestamp / actor / role / method / path / client_ip / result / detail
  - 内存 `RwLock<Vec<AuditEntry>>` + 可选文件持久化
  - `record()` / `query()` / `count()` / `clear()` 方法
  - 最大条目限制，自动裁剪旧条目
  - 7 个单元测试

#### S1 修复：API 安全加固（JWT + RBAC + Auth）

- **新增 `AuthManager`**（`eneros-api::auth`）
  - JWT HS256 手动实现（使用 `hmac` / `sha2` / `base64` crate）
    - `issue_token()`：签发 JWT（header.payload.signature）
    - `verify_token()`：验证签名 + 过期时间
    - `Claims`：sub / role / exp / iat
  - API Key 认证（备用）：`X-API-Key` header
  - `authenticate()`：先尝试 API Key，再尝试 Bearer token
  - `AuthExtractor::from_headers()`：从 axum 请求头提取认证信息
- **新增 `Role` 枚举 + `Permission` 枚举**（RBAC 权限模型）
  - 4 个角色：Observer（只读）/ Operator（读写）/ Supervisor（控制动作）/ Emergency（紧急操作）
  - 4 个权限：Read / Write / Control / Emergency
  - `has_permission()` 矩阵：Emergency 拥有所有权限，Supervisor 拥有 Read/Write/Control
  - `required_permission(method, path)`：HTTP 方法+路径 → 权限映射
- **新增认证端点**（`eneros-api::handlers::auth`）
  - `POST /api/auth/login`：签发 JWT
  - `POST /api/auth/refresh`：验证旧 token + 签发新 token
  - `GET /api/auth/me`：返回当前用户信息
  - 4 个单元测试
- 20 个单元测试覆盖 JWT 签发/验证/过期、RBAC 权限矩阵、API Key 认证

#### S4 修复：API 覆盖完善（6/17 → 16/17 crate 暴露）

- **新增 5 个 handler 模块 + 16 个端点**：
  - `timeseries.rs`：`GET /api/timeseries/query` / `GET /api/timeseries/latest` / `GET /api/timeseries/statistics`
  - `events.rs`：`POST /api/events/publish` / `GET /api/events/stats`
  - `devices.rs`：`GET /api/devices` / `GET /api/devices/{id}/health` / `POST /api/devices/{id}/connect` / `POST /api/devices/{id}/disconnect`
  - `tools.rs`：`GET /api/tools` / `POST /api/tools/{name}/execute`
  - `memory.rs`：`POST /api/memory/{agent_id}/store` / `POST /api/memory/{agent_id}/recall` / `GET /api/memory/{agent_id}/count` / `DELETE /api/memory/{agent_id}/{entry_id}` / `DELETE /api/memory/{agent_id}`
- **AppState 扩展 6 个新字段**：metrics_registry / audit_log / auth_manager / device_manager / tool_engine / agent_memory
  - 全部 `Option<Arc<...>>`，向后兼容（默认 None）
  - Builder 方法：`with_metrics_registry` / `with_audit_log` / `with_auth_manager` / `with_device_manager` / `with_tool_engine` / `with_agent_memory`
- 13 个单元测试覆盖请求/响应序列化

#### S6 修复：自动回滚执行（后条件失败 → 执行 rollback_plan）

- **新增 `RollbackExecution` 结构**（`eneros-gateway::pipeline_types`）
  - succeeded / steps_attempted / steps_succeeded / error / duration_us
  - `success()` / `failure()` 构造方法
- **`EnhancedPipelineDecision` 新增 `rollback_executed` 字段**
- **`ConstrainedDecisionPipeline` 新增 Stage 7：自动回滚执行**
  - 后条件失败时检查 `rollback_plan.can_auto_rollback()`
  - 若允许自动回滚，按逆序执行 `rollback_plan.steps` 的 `undo_action`
  - 每步回滚通过 `gateway.execute_command()` 走完整执行路径
  - `BestEffort` 策略：跳过失败步骤继续；其他策略：首步失败即停止
  - 回滚结果记录到审计日志（`stage: "rollback"`）
  - 统计跟踪：`rollbacks_triggered` / `rollbacks_succeeded` / `rollbacks_failed`
- **`PipelineStatistics` 新增 2 个字段**：`rollbacks_succeeded` / `rollbacks_failed`
- 3 个单元测试覆盖回滚触发、回滚跳过、审计条目

#### S7 修复：WebSocket 实时推送（EventBus → WS 桥接）

- **新增 `start_event_bus_ws_bridge()`**（`eneros-api::app`）
  - 订阅 `EventBus::subscribe()` 的 broadcast channel
  - 每个事件序列化为 JSON（type / event_type / id / timestamp / source / payload）
  - 通过 `broadcast_event()` 推送到所有已连接 WS 客户端
  - 非阻塞：客户端缓冲区满时跳过并告警
  - 返回 `JoinHandle` 供优雅关闭时 abort
- **`main.rs` 集成**：启动时调用 `start_event_bus_ws_bridge(state)`，关闭时 abort
- 3 个单元测试覆盖无 EventBus、单客户端转发、多客户端转发

#### 依赖更新

- 新增安全相关依赖：`jsonwebtoken = "9"` / `sha2 = "0.10"` / `hmac = "0.12"` / `base64 = "0.22"`

#### 验证结果

- 编译错误：0
- 测试通过：1259（eneros-core 87 + 其他 crate 1172），新增 84 个测试
- Clippy 警告：0
- 向后兼容：v0.5.0 的 `eneros.toml` 和 API 完全兼容（serde default）

---

## [0.5.0] - 2026-06-17

### Agent 自主化（修复 3 个致命架构缺陷 F4/F5/F6 + 3 个严重/中等差距 S5/S8/M4）

> **设计目标**：v0.5.0 聚焦 Agent 操作系统核心能力的补齐，将 v0.4.0 的"被动响应器 + 单向数据流"升级为"自主体 + 规划-反思-学习闭环 + 统一工具协议 + 语义记忆"。所有改动均为新增模块和向后兼容的扩展，不破坏 v0.4.0 的配置和 API。

#### F4 修复：Agent spawn 生命周期（被动响应器 → 自主体）

- **新增 `SpawnedAgent`**（`eneros-agent::spawn`）
  - 后台 tokio task 包装 `Arc<Mutex<Box<dyn Agent>>>`（tokio::sync::Mutex，因 `Agent::tick()` 需 `&mut self`）
  - 感知-行动循环：接收消息 → `handle_event` → `tick` → 分发动作 → sleep
  - watch channel 控制 Run/Pause/Stop 信号
  - 共享 `AgentLifecycle` 状态：Created → Initializing → Running ⇄ Paused → Stopping → Stopped
  - 4 个单元测试覆盖生命周期、暂停/恢复、存活检测、消息处理

#### F5 修复：行为规划引擎（无规划 → DAG 计划）

- **新增 `eneros-agent::planning` 模块**
  - `Goal` 结构：goal_type / description / priority / params
  - `PlanStep`：step_id / action / depends_on / preconditions / expected_outcome
  - `Plan`：DAG 验证（Kahn 拓扑排序）+ topological_order()
  - `Planner` trait：`async fn plan(&self, goal: &Goal) -> Result<Plan>`
  - `RuleBasedPlanner`：4 个内置模板
    - `voltage_violation`（3 步：检测 → 调无功 → 验证）
    - `overload`（3 步：检测 → 切负荷 → 验证）
    - `frequency_deviation`（3 步：检测 → 调出力 → 验证）
    - `restore_supply`（4 步：隔离故障 → 恢复馈线 → 并网 → 验证）
  - `PlanExecutor`：按拓扑序执行，首步失败即中止
  - 11 个单元测试覆盖规划、验证、执行、依赖链

#### F6 修复：反思与学习闭环（无学习 → Lesson 提取 + 程序性记忆）

- **新增 `eneros-agent::reflection` 模块**
  - `Lesson` 结构：scenario / failure_reason / improvement / importance；可序列化为 `MemoryEntry`
  - `ReflectionEngine::reflect()`：对比计划预期结果与执行结果，提取 Lesson
  - `LearningPolicy`：控制学习频率（每 N 次执行学习一次）+ 最小重要性阈值 + 每 agent 最大 Lesson 数
  - `store_lessons()` / `recall_lessons()`：与 `AgentMemory` 集成，存储为 Procedural 记忆
  - `generate_improvement_suggestion()`：按 goal_type 生成改进建议
  - `calculate_importance()`：约束拒绝和安全失败提升重要性
  - 7 个单元测试覆盖成功/失败反思、存储/召回、策略跳过、往返、建议、重要性

#### S5 修复：统一工具调用协议（工具断裂 → CallTool + ToolEngine 集成）

- **`AgentAction` 新增 `CallTool { tool_name, params }` 变体**（`eneros-agent::agent`）
- **`ActionDispatcher` 持有 `Option<Arc<tokio::sync::RwLock<ToolEngine>>>`**（`eneros-agent::dispatcher`）
  - 使用 `tokio::sync::RwLock` 而非 `parking_lot::RwLock`，因为读锁需跨 `.await` 点持有（`Send` 约束）
  - 新增构造器：`with_pipeline_and_tools()` / `with_tool_engine()`
  - `CallTool` 分发：调用 `engine.execute()`，返回 `ToolExecuted` 或 `CommandRejected`
- **`DispatchResult` 新增 `ToolExecuted(String)` 变体**
- 3 个单元测试覆盖无引擎、有引擎（EchoTool）、未知工具

#### S8 修复：DelegateTask 路由 + 并发 tick（协作断裂 → 消息路由 + 并发执行）

- **`ActionDispatcher` 持有 `Option<Arc<AgentContext>>`**（`eneros-agent::dispatcher`）
  - 新增 `with_context()` 构造器
  - `DelegateTask` 分发：当 context 可用时，通过 `MessageStore` 投递 `AgentMessage::direct()` 到目标 agent
- **`AgentOrchestrator::tick_all()` 改为并发执行**（`eneros-agent::orchestrator`）
  - 使用 `futures::future::join_all` 并发执行所有 agent 的 tick
  - 工作区依赖新增 `futures = "0.3"`

#### M4 修复：记忆系统语义检索（关键词匹配 → TF-IDF 语义搜索）

- **新增 `SemanticMemory`**（`eneros-memory::vector`）
  - 纯 Rust 实现，零外部 ML 依赖
  - TF-IDF（词频-逆文档频率）+ 余弦相似度
  - `recall_semantic()` 方法：自然语言查询，按语义相关性排序
  - 实现 `AgentMemory` trait，可作为 `InMemoryMemory` 的直接替代
  - `recall()` 当指定 keyword 时自动走语义搜索路径
  - 支持所有原有过滤器（memory_type / min_importance / tags / time_range）
- 12 个单元测试覆盖存储/召回、无精确匹配的语义匹配、相关性排序、空查询、无条目、limit、forget、clear、类型过滤、tokenize、余弦相似度

#### 其他改动

- **`eneros-agent::lib.rs`** 新增 `pub mod spawn / planning / reflection` 及类型重导出
- **`eneros-agent/Cargo.toml`** 新增 `futures` 依赖
- **`eneros-memory::lib.rs`** 新增 `pub mod vector` 及 `SemanticMemory` 重导出

#### 验证结果

- 编译：`cargo build --workspace` 通过，0 error
- 测试：`cargo test --workspace` **1175 passed; 0 failed**（v0.4.0: 1137 + v0.5.0 新增 38）
- 静态检查：`cargo clippy --workspace --all-targets` **0 warning**

---

## [0.4.0] - 2026-06-17

### 生产路径接线（修复 3 个致命架构缺陷 F1/F2/F3）

> **设计目标**：v0.4.0 聚焦系统级架构缺陷修复，将 v0.3.0 的"组件齐全但未接线"状态升级为"生产路径全链路打通"。所有改动均向后兼容，无配置文件可降级到 v0.3.0 行为。

#### F2 修复：SCADA 数据管道断裂（DataSource::refresh + DataPipeline 异步化）

- **`DataSource` trait 新增 `async fn refresh()` 默认方法**（`eneros-scada::collector`）
  - 使用 `#[async_trait]`，默认 no-op，向后兼容 push-based 源（MQTT/Simulated）
  - pull-based 源（IEC 104/Modbus）覆写此方法在 `collect_once()` 前拉取最新数据
- **`Iec104DataSource::refresh()` 实现**（`eneros-scada::iec104::datasource`）
  - 检查 `ConnectionState::Active` 后调用 `refresh_cache()` 拉取客户端缓存
  - 非 Active 状态跳过刷新，保留 last-known-good 缓存（避免瞬断丢数据）
- **`DataPipeline::run_once()` 改为 `async`**（`eneros-scada::pipeline`）
  - 调用 `collector.refresh_data_source().await` 后再 `collect_once()`
  - `start()` 后台循环同样先 refresh 再 collect
  - 所有调用方（`data_driven_loop`、`e2e_integration` 测试）已更新为 `.await`
- **`ScadaCollector::refresh_data_source()` 新增**，委托给 `DataSource::refresh()`

#### F1 修复：生产执行路径接线（DeviceManager → DeviceCommandExecutor → SafetyGateway）

- **`build_device_manager()` 从 `[[devices]]` 配置构建 `DeviceManager`**（`eneros-api::main`）
  - 支持 iec104 / iec61850 / modbus / mqtt 四种协议适配器
  - 设备连接失败为非致命（记录警告，gateway 降级为 LoggingExecutor）
- **`build_command_executor()` 根据设备数选择执行器**（`eneros-api::main`）
  - `devices_configured > 0` → `DeviceCommandExecutor`（生产路径，含 ACK 校验+重试）
  - `devices_configured == 0` → `LoggingExecutor`（仿真降级）
- **`SafetyGateway::with_queue_and_executor()` 接线生产执行器**（`eneros-gateway`）
  - 命令队列 + 真实执行器双输入，命令实际下发到设备而非仅记录日志
- **`ObservationProvider` 闭包接线 SCADA → 后置条件验证**（`eneros-api::main`）
  - 读取 `ScadaCollector::latest_all()` 构建 `PowerObservation`
  - `build_observation_from_readings()` 映射 voltage_pu/angle_deg/gen_p_mw/load_p_mw/frequency_hz
  - `ConstrainedDecisionPipeline::with_observation_provider()` 优先使用实测值而非仿真预测
  - 闭合 execute → measure → verify 循环

#### F3 修复：网络模型配置化（ieee14 / cnpower / cim）

- **`build_network_from_config()` 支持三种网络源**（`eneros-api::main`）
  - `ieee14`：内置 IEEE 14-bus 测试用例（默认）
  - `cnpower`：通过 `eneros-bridge::CnpowerEquipmentLoader` 从设备库加载（桥接不可用时降级 IEEE 14）
  - `cim`：CIM/CGMES 配置文件加载（预留接口，当前降级 IEEE 14）
- **`eneros-bridge` 依赖加入 `eneros-api`**，启用 cnpower 路径

#### S2 修复：配置系统接线（eneros.toml 全字段驱动）

- **`EnerOSConfig` 扩展三个新配置结构**（`eneros-core::config`）
  - `NetworkConfig { source, path, initial_powerflow }` — 网络模型源选择
  - `ScadaSourceConfig { source, iec104_addr, iec104_asdu, fast_interval_ms, normal_interval_ms }` — SCADA 源选择
  - `DeviceConnectionConfig { device_id, protocol, host, port, params }` — 设备连接配置
  - 全部字段 `#[serde(default)]`，v0.3.0 配置文件无需修改即可加载
- **`eneros.toml` 新增 `[network]` / `[scada]` / `[[devices]]` 段**，含注释示例
- **`run_server()` 18 步初始化流程**全部从 `EnerOSConfig` 读取参数
  - 0. 加载 eneros.toml → 1. EventBus → 2. ConstraintEngine → 3. PowerNetwork(配置) → 4. TimeSeriesEngine → 5. DeviceManager(配置) → 6. SCADA 源(配置) → 7. DataPipeline(refresh+collect) → 8. DualScanGroup → 9. SnapshotBuilder → 10. SafetyGateway(生产执行器) → 11. RealtimeExecutor+Watchdog → 12. Reasoning → 13. ConstrainedDecisionPipeline(ObservationProvider) → 14. FeedbackLoop → 15. AgentOrchestrator(6 agents) → 16. DataDrivenAgentLoop → 17. HTTP server → 18. 优雅关停（含设备断开）

#### 端到端集成测试（18 个新测试）

- **新增 `crates/eneros-api/tests/e2e_v04_wiring.rs`**，验证三大缺陷修复的真实代码路径：
  - T6 配置：解析 ieee14/iec104/devices 段、向后兼容默认值
  - T3 网络：`NetworkConfig::default()` 选择 ieee14
  - T2 SCADA：`CountingDataSource` 证明 `refresh()` 在 `collect_once()` 前被调用
  - T2 SCADA：`SimulatedDataSource::refresh()` 为 no-op（push-based 兼容）
  - T2 SCADA：`Iec104DataSource::refresh()` 非 Active 状态跳过、Active 状态拉取 IOA 映射
  - T1 设备：`DeviceManager` 注册+连接失败非致命、`DeviceCommandExecutor`/`LoggingExecutor` 选择逻辑
  - T1 设备：`SafetyGateway::with_queue_and_executor()` 生产路径构造
  - T4 观测：`ObservationProvider` 从 SCADA 读数构建 `PowerObservation`、无数据时返回 None
  - 全链路：config → network → SCADA → pipeline → observation → gateway

#### 其他改动

- **`Iec104Client::set_state_for_testing()` 新增**（`eneros-device::adapters::iec104::client`）
  - 公开方法，允许测试模拟 Active 连接状态而无需启动 mock TCP 服务器
- **`async-trait` 加入 `eneros-api` dev-dependencies**，供集成测试实现 `DataSource` trait

#### 验证结果

- 编译：`cargo build --workspace` 通过，0 error
- 测试：`cargo test --workspace` **1137 passed; 0 failed**（v0.3.0: 1119 + v0.4.0 新增 18）
- 静态检查：`cargo clippy --workspace --all-targets` **0 warning**

---


## [0.3.0] - 2026-06-17

### pandapower/cnpower 融入升级

> **设计原则**：不删除 EnerOS 独有层（agent/SCADA/协议栈/API），而是把 pandapower/cnpower 的算法和数据优点融入 EnerOS 的 Rust 原生实现。

#### 改进 1：BFSW 配电网潮流算法（融入 pandapower 优点）

- 新增 `eneros-powerflow::bfsw_solver::BfswSolver`，实现前推回代（Backward/Forward Sweep）算法
- 参考 pandapower `run_bfswpf.py` 实现 BIBC/BCBV/DLF 矩阵构造
- 支持辐射状配电网（树形拓扑），自动检测孤岛
- 支持变压器分接比调整
- 新增 `PowerFlowAlgorithm` 枚举（NewtonRaphson / BackwardForwardSweep / DC）
- `PowerFlowSolver::with_algorithm()` 支持算法选择
- 3 个单元测试验证 2-bus、3-bus 辐射网和孤岛检测

#### 改进 2：合规规则引擎（融入 cnpower 优点）

- 新增 `eneros-constraint::compliance` 模块
- `ComplianceChecker` 提供 5 条国标合规检查规则：
  - `TR2_LOAD_001`: 变压器负载率（GB/T 6451-2023）
  - `TR2_THERMAL_001`: 变压器热稳定（GB/T 1094.7-2024）
  - `VOLTAGE_DEV_001`: 电压偏差（GB/T 12325-2008，按电压等级分级）
  - `CAB_LOAD_001`: 电缆载流量（GB/T 12706-2020）
  - `SWG_BREAK_001`: 断路器开断能力（GB/T 1984-2024）
- 三态评估：Passed / Failed / Inconclusive（支持数据缺失检测）
- `EquipmentSpec` 和 `OperatingConditions` 结构化输入
- `check_all()` 按设备类型自动选择适用规则
- 7 个单元测试覆盖通过/失败/不确定三种状态

#### 改进 3：Q 限值强制 + Recycle 机制（融入 pandapower 优点）

- 新增 `QLimits` 结构，支持 PV 节点 Q 限值配置
- `PowerFlowSolver::solve_with_options()` 实现 Q 限值强制：
  - 参考 pandapower `_run_ac_pf_with_qlims_enforced`
  - 检测 PV 节点 Q 越限，自动转 PQ
  - 支持单点修复模式（每次迭代修复最严重越限）
  - 最大 10 次外层迭代
- 新增 `RecycleCache` 结构，支持时序计算复用：
  - 参考 pandapower `powerflow.py:73-134` recycle 机制
  - 缓存上次电压幅值和相角作为初值
  - 加速连续潮流计算收敛
  - `invalidate()` 方法支持拓扑变更时清空缓存

#### 改进 4：配网规划参数库 + 典型接线模式（融入 cnpower 优点）

- 新增 `eneros-analysis::planning` 模块，提供配网规划参数库：
  - `SupplyAreaClass` 枚举（A/B/C/D/E 供电区域分类，对应 DL/T 5729）
  - `VoltageLimits` 按 GB/T 12325 分电压等级提供偏差限值
  - `LoadingLimits` 变压器负载率限值（按区域类型，含 N-1 事故和紧急限值）
  - `SupplyRadius` 各电压等级供电半径（A/B 类 3 km，C/D 类 5 km，E 类 15 km）
  - `LoadModel` 负荷模型（恒功率/恒电流/恒阻抗及比例组合）
  - `RenewableHosting` 分布式电源接纳能力评估
  - `StorageApplication` 储能配置参数
  - `PlanningScenario` / `CandidateAction` / `CandidatePlan` 候选方案生成与评估
  - `PlanningEvaluator` 综合规划评估器
  - 7 个单元测试覆盖电压限值、N-1 要求、供电半径、负载率、候选方案生成
- 新增 `eneros-topology::connection_modes` 模块，提供 7 种典型接线模式：
  - `ConnectionMode` 枚举：单辐射 / 单联络 / 双联络 / 三段三联络 / 多分段多联络 / 单环网 / 双环网
  - `TopologyTemplate` 拓扑模板（分段数、联络数、环网结构）
  - 可靠性指标：SAIFI / SAIDI / RS-1 自动计算
  - `satisfies_n1()` N-1 安全校验
  - `applicable_area()` 适用区域判定
  - `match_network()` 根据网络结构自动识别接线模式
  - 7 个单元测试覆盖可靠性指标、拓扑模板、网络匹配

#### 改进 5：系统级校验规则引擎（融入 pandapower/cnpower 优点）

- 新增 `eneros-constraint::validation_rules` 模块，提供系统级校验规则：
  - **电压质量规则**（参考 GB/T 12325/15945/14549/12326）：
    - `check_voltage_deviation()` 电压偏差（按电压等级分级：220 kV ±5%，10–35 kV ±7%，0.4 kV +7%/-10%）
    - `check_frequency_deviation()` 频率偏差（±0.2 Hz）
    - `check_harmonics()` 电压总谐波畸变率 THD（≤5%）
    - `check_flicker()` 长期闪变 Plt（≤1.0）
  - **N-1 安全规则**（参考 GB/T 38306-2025 / DL/T 7233-2017）：
    - `check_n1_security()` 校验每个预想事故后：母线不坍塌、电压偏差 ≤0.1 p.u.、支路负载率 ≤100%
  - **短路规则**（参考 GB/T 15544.1-2023）：
    - `check_short_circuit_capacity()` 三相短路电流 vs 断路器开断能力（含 10% 安全裕度）
    - `check_fault_clearing_time()` 故障切除时间（≤0.25 s）
  - 三态评估：`ValidationStatus` (Passed / Failed / Inconclusive)
  - `SystemStateSnapshot` 聚合母线电压、频率、预想事故、短路观测
  - `validate_all()` 一次性运行所有规则族
  - `ValidationSummary` 汇总统计（passed / failed / inconclusive 计数）
  - 18 个单元测试覆盖每条规则的通过/失败/不确定分支

### 验证结果
- 编译：0 error, 0 warning
- 测试：**1119 passed, 0 failed**（+33 新测试：planning 7 + connection_modes 7 + validation_rules 18 + 其他 1）
- Clippy：0 warning, 0 error
- BFSW 测试：3 passed（2-bus、3-bus、孤岛检测）
- 合规检查测试：7 passed（变压器/电缆/断路器/电压偏差）
- 配网规划测试：7 passed（电压限值/N-1/供电半径/负载率/候选方案）
- 接线模式测试：7 passed（可靠性指标/拓扑模板/网络匹配）
- 校验规则测试：18 passed（电压质量/N-1 安全/短路容量/故障切除时间）

---

## [0.2.2] - 2026-06-17

### cnpower 接入 BUG 修复（C1-C5）

#### C1: bridge_server.py 未知命令错误协议修复
- `bridge_server.py` 的 `main()` 函数中，未知命令原先返回 `{"ok": true, "data": {"error": "..."}}`，导致 Rust 端误认为调用成功
- 修复为正确返回 `{"ok": false, "error": "Unknown command: ..."}`

#### C2: bridge_server.py 补全缺失命令
- `bridge_server.py` 的 COMMAND_MAP 原先缺少 `build_full_network` 和 `run_powerflow` 两个命令
- 从 `bridge_http_server.py` 移植 `_run_powerflow()` 和 `_build_full_network()` 函数及对应 COMMAND_MAP 条目
- 子进程模式现在支持与 HTTP 模式相同的完整命令集

#### C3: CnpowerEquipmentLoader 默认使用 BridgeClient
- `CnpowerEquipmentLoader::new()` 原先默认使用 `PythonBridge`（每次调用 spawn 新 Python 进程，性能差）
- 改为默认使用 `BridgeClient`（HTTP 常驻服务，性能优）
- 新增 `BridgeKind` 枚举（`Subprocess`/`Http`）支持后端选择
- 新增 `start_server()` 方法用于启动 HTTP 服务
- 新增 `with_backend()` 方法支持自定义后端

#### C4: 设备 ID 用递增计数器替代硬编码
- `parse_transformer`/`parse_cable`/`parse_overhead_line` 中 `id: 0`、`hv_bus_id: 0`、`lv_bus_id: 1` 等全部硬编码
- `load_all_*` 方法中用 `enumerate()` 为每个设备分配唯一递增 ID
- `load_transformer_by_model` 用 FNV-1a 哈希生成稳定 ID
- bus_id 统一设为 0（由 network builder 分配）

#### C5: load_all_loads 文档说明
- 确认 `load_all_loads()` 返回空 Vec 是合理设计（cnpower 设备目录不含负荷数据）
- 文档明确说明负荷数据应通过 `build_full_network()` 获取

### 验证结果
- 编译：0 error, 0 warning
- 测试：1076 passed, 0 failed
- Clippy：0 warning, 0 error
- E2E 测试（cnpower 接入）：7 passed, 0 failed
- bridge_server.py C1/C2 修复验证：未知命令正确返回错误，build_full_network/run_powerflow 正常工作

---

## [0.2.1] - 2026-06-17

### API 端点修复（B1-B7）

#### B1: CLI `-h` 参数冲突
- `eneros-api` 的 `--host` 参数移除 `-h` 短选项，避免与 `--help` 冲突

#### B2: 状态估计端点缺少 measurements 字段
- `SeRequest.measurements` 添加 `#[serde(default)]`，使字段可选
- SE handler 改用 `estimate_with_network()` 配合真实 Y-bus 矩阵，从潮流结果合成虚拟测量（VoltageMagnitude、BusInjectionP/Q、BranchFlowP/Q）
- `eneros-analysis` 导出 `NetworkModel` 类型

#### B3: Dashboard JS 端点路径不匹配
- `APP_JS` 的 `refreshData()` 修正为调用真实 API 端点：`/api/dashboard/topology-svg`、`/api/dashboard/flow-heatmap`、`/api/agents`、`/api/scada/latest`
- 新增 `applyFlowOverlay()`、`renderAgents()`、`renderScadaData()` JS 函数正确渲染 API JSON 响应

#### B4: flow-panel 重复使用 topology_svg
- `generate_dashboard_page()` 签名变更：新增 `flow_heatmap_svg: &str` 独立参数
- flow 面板使用独立 SVG，由前端 JS overlay 应用着色

#### B5: data_panel 单位显示错误
- 新增 `infer_unit()` 函数从参数名推断工程单位（p.u./deg/Hz/MW/MVar/%/kA）
- 返回 `&'static str` 避免每次调用的 String 分配

#### B6: health 端点健康检查增强
- `health_handler` 从简单 `{"status":"ok"}` 增强为全组件健康检查
- 检查 network、topology_engine、constraint_engine、scada_collector、agent_orchestrator、ts_engine
- 使用 `agent_count()` 替代 `registered_agents().len()`，零分配获取 agent 数量

#### B7: workspace 版本号同步
- `Cargo.toml` workspace.package 版本从 `0.1.0` 更新为 `0.2.0`

### 性能优化（系统级审查）

#### H4: SQLite 时序存储索引优化
- `time_series` 表改为 `WITHOUT ROWID` 利用聚簇主键
- 新增 `idx_ts_time` 索引加速 `cleanup()` 和 `latest()` 的时间戳查询

#### M1: health_handler 零分配 agent 计数
- 使用 `AgentOrchestrator::agent_count()`（直接返回 `self.agents.len()`）替代 `registered_agents().len()`（克隆全部 agent 到 Vec 仅取长度）

#### M2: ObservationProvider 超时保护
- `decision_pipeline.rs` 中 ObservationProvider 调用包装 `tokio::task::spawn_blocking` + `tokio::time::timeout(500ms)`
- 防止 SCADA/RTU 同步 I/O 阻塞 async runtime，超时或 panic 时回退到 simulator

#### M3: rt_executor stats 锁合并
- `execute_one()` 的 Ok 分支从两次锁获取合并为一次，减少原子操作和内存屏障

#### M4: infer_unit 零分配
- `infer_unit()` 返回类型从 `String` 改为 `&'static str`
- 使用 `to_ascii_lowercase()` 替代 `to_lowercase()`（ASCII 参数名足够）

### 验证结果
- 编译：0 error, 0 warning
- 测试：1076 passed, 0 failed
- Clippy：0 warning, 0 error

---

## [0.2.0] - 2026-06-17

### 核心架构修复（BUG3 全部9项）

#### 接入层：协议适配器真实化
- **IEC 104**：删除 `eneros-device` 中的 HashMap 假实现，替换为真实 TCP 协议栈（APCI 帧、STARTDT 握手、接收循环），`eneros-scada` crate 复用 `eneros-device` 的实现而非维护独立副本
- **IEC 61850**：替换 HashMap 假实现为完整 MMS 协议栈（COTP 连接、MMS 读/写服务），支持报告和 GOOSE 模型
- **TESTFR 应答**：IEC 104 客户端收到 TESTFR_ACT 时回复 TESTFR_CON，防止 RTU 断开连接
- 新增 98 个协议适配器测试（IEC104 TCP 传输 6 个、IEC61850 MMS 8 个等）

#### 执行层：命令执行落地
- 新增 `CommandExecutor` trait（`execute()` + `read_back()` 异步接口）
- 新增 `DeviceCommandExecutor`：桥接 `Command` → `DeviceManager::write()` → `ProtocolAdapter::write()`，写后读回 ACK 验证，失败自动重试
- 新增 `LoggingExecutor`：向后兼容的日志回退执行器
- `Command` 结构体新增 `device_id`、`device_address`、`device_value` 字段用于设备路由
- `SafetyGateway::execute_command` 改为 async，使用 `tokio::sync::Mutex` 串行化 validate→execute→record
- `RealtimeExecutor::execute_one` 移除假 ACK 等待，使用真实执行结果

#### 状态机联动
- `SystemStateMachine::on_state_changed` 真正调用 `ConstraintEngine::set_emergency_thresholds()`，不再只 push 字符串消息
- 状态转换时记录阈值乘数到 `triggered_actions`

#### 冲突解析
- 重构 `ActionConflictResolver` 为 authority→time→proximity→id 四级解析链
- `resolve_by_time` 不再返回 None，使用时间戳比较实现"谁先到谁赢"
- 新增 `ProximityProvider` trait 支持拓扑近邻性解析

#### 负荷预测
- `HoltWinters` 不再退化为二次指数平滑，调用真正的 `holt_winters_fit()` 实现
- 支持加性（Additive）和乘性（Multiplicative）季节分解
- 新增 `HoltWintersTyped` 变体支持显式季节性类型选择

#### 持久化
- `TimeSeriesEngine` 新增 `with_persistent_storage()` 和 `with_sqlite()` 构造函数
- 实现 write-through 缓存模式：`record()` 同时写内存和 SQLite，`query()`/`latest()` 优先读内存，缓存未命中时回退到 SQLite 并回填
- 重启后数据不丢失（`test_real_sqlite_survives_restart` 验证）

#### 分析层：数值算法生产级化
- **状态估计**：新增 `estimate_with_network()` 方法，使用 Y-bus 导纳矩阵推导真实雅可比矩阵；新增 `NetworkModel` 结构体；`Measurement` 新增 `to_element_id` 支持支路测量；Tikhonov 正则化保证增益矩阵非奇异；使用精确非线性 h(x) 替代 H·x 线性近似
- **短路分析**：新增 `SequenceNetworks` 结构体（独立正序/负序/零序 Z-bus 矩阵）；新增 `analyze_with_sequence_networks()` 生产级方法，SLG/LL/DLG 各序网络独立计算
- **OPF**：新增 `compute_lmp_rigorous()` 基于拉格朗日对偶的严格 LMP 计算（能量分量 + 拥塞分量），影子价格通过 KKT 条件计算
- **变压器分接头**：`TwoWindingTransformer` 新增 `tap_step_percent` 字段，步长从设备参数读取而非硬编码 1%

#### P16 端到端闭环
- 新增 `ObservationProvider` 类型：执行后从 SCADA/RTU 读回实际电网观测值
- `WhatIfResult::from_observation()`：从实际 `PowerObservation` 构建 WhatIfResult，直接检查电压/热力约束
- `ConstrainedDecisionPipeline` Stage 6 优先使用实测观测（`field_observation`），无 provider 时回退到模拟器预测（`simulator_prediction`/`simulator_fallback`）
- 审计日志记录 postcondition 数据来源

### 测试
- 全部 930+ 测试通过，0 失败，0 编译警告
- 新增测试：IEC104 TCP 传输 6 个、IEC61850 MMS 8 个、执行器 8 个、状态估计真实雅可比 8 个、短路序网络 8 个、postcondition 实测观测 4 个

---

## [0.1.0] - 2026-06-15

### 初始发布

#### 核心框架（19 个 crate）
- **eneros-core**：基础类型定义（StructuredAction、PowerObservation、AuthorityLevel 等）
- **eneros-topology**：电网拓扑建模
- **eneros-powerflow**：潮流计算（牛顿-拉夫逊、Y-bus 矩阵）
- **eneros-constraint**：约束引擎、可行性投影器、What-If 分析
- **eneros-equipment**：设备模型（变压器、线路、负荷、发电机）
- **eneros-timeseries**：时序数据引擎 + SQLite 存储
- **eneros-eventbus**：事件总线
- **eneros-gateway**：安全网关、命令队列、实时执行器、决策管线
- **eneros-device**：设备管理器、协议适配器（Modbus、MQTT、IEC104、IEC61850）
- **eneros-api**：REST API 服务
- **eneros-bridge**：设备桥接
- **eneros-network**：电力网络集成
- **eneros-memory**：Agent 记忆系统
- **eneros-tool**：工具链
- **eneros-reasoning**：推理引擎
- **eneros-agent**：Agent 运行时、领域 Agent、冲突解析、系统状态机
- **eneros-scada**：SCADA 数据采集
- **eneros-analysis**：分析模块（状态估计、OPF、短路计算）
- **eneros-dashboard**：Web 仪表盘

#### Phase 1-14 功能
- Phase 1：内核基础（类型系统、事件总线、时序存储）
- Phase 2：Agent 运行时（Agent trait、调度器、权威等级）
- Phase 3-5：设备模型、潮流计算、约束引擎
- Phase 6：领域 Agent（预测、规划、自愈、电力协同）
- Phase 7：实时集成（RT 执行器、看门狗、优先级队列）
- Phase 8：深度集成（Bridge、多 Agent 协同）
- Phase 9：Bug 修复轮
- Phase 10：LLM 集成（推理引擎、Agent-LLM 对接）
- Phase 11：RIG 工具统一
- Phase 12：实时执行域
- Phase 13：约束决策管线（6 步验证、预/后条件检查）
- Phase 14：闭环（执行→验证→回滚）

#### Phase 16-17 功能
- Phase 16：端到端管线验证（14 个集成测试）
- Phase 17：IEC 104 适配器（TCP 传输、心跳、半包/粘包处理）

### 测试
- 985 个测试全绿 / 0 编译警告 / clippy 零告警

---

## 版本号规则

| 版本号部分 | 变更触发 |
|-----------|---------|
| **主版本号** (X.0.0) | 不兼容的 API 修改 |
| **次版本号** (0.X.0) | 向下兼容的功能新增 |
| **修订号** (0.0.X) | 向下兼容的问题修复 |

## 链接

[Unreleased]: https://github.com/GAWG-AI/EnerOS/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/GAWG-AI/EnerOS/releases/tag/v0.2.0
[0.1.0]: https://github.com/GAWG-AI/EnerOS/releases/tag/v0.1.0
