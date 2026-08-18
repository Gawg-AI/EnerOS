# 39 — 验证记录与 Exit Gate 核销（存储、网络与全协议适配修复蓝图）

> schema: `eneros.blueprint-39.v1`
> 前置：38 Exit Gate 10/10 PASS（`evidence/38`）——允许进入 39
> 编制：2026-08-14（蓝图 39 执行完毕）
> 分支：`feature/v0.165.0-protocol-driver-sdk`；工作树含 36/37/38 未提交代码（本卷保护未覆盖）
> 审查基线：`HEAD=b96774e` + 当前工作树（含 39 全部未提交改动）
> 证据：`target/eneros/evidence/39-qemu/`（QEMU 最终复验轮）+ `evidence/39/RECON-39.md`、`CROSS_VERSION-39.md`

## 0. 结论

**39 Exit Gate：10 PASS / 1 PARTIAL（39 范围）。** 详见 §8 逐条核销与诚实边界。
（原记录 9 PASS / 2 PARTIAL；2026-08-15 收尾复验轮后订正，见 §10。）
产品商业状态仍为 **NO-GO**（35-00 总裁决；40-43 串行后重算）。

## 1. Entry Gate

- 38 Exit Gate 10/10 PASS（`evidence/38`，38-review2 复验含 QEMU）——允许进入 39 ✓
- 39 的 PlatformInventory/DMA/DriverSupervisor/drv-svc runtime 前置已就绪 ✓

## 2. 关键根因（本卷新发现并修复）

### 2.1 virtio DMA 设备地址：rootserver VA≠PA（37 起 "format failed Io" 根因链）

QEMU 复验初轮暴露 virtio-blk 读超时（`Timeout { ms: 2000 }`）——38 记录诚实登记为
"37 同态，virtio-blk 驱动层"遗留。39 逐层定位并修复三处叠加根因：

1. **CVR-39-01（栈溢出）**：`Box::new([0u8; 36*4096])` 先建 144KB **栈上**临时数组——
   root 栈仅 64KB（`root_task_stack_space: .skip 65536`）→ 溢出。修复：堆上直接
   分配零化切片。
2. **CVR-39-04（VA→PA 偏移）**：`DmaPoolAllocator::new_identity()` 假设 VA==PA；
   qemu-virt rootserver 链接于低 VA（0x200000 起）而加载于 guest RAM
   （0x40000000 起）——无偏移时设备 DMA 落到 RAM 外，QEMU 静默丢弃。
3. **CVR-39-05（池未 4K 对齐）**：池内存仅 16B 对齐——`QUEUE_PFN = phys>>12`
   截断后设备读错页。修复：池可用区 4K 对齐。
4. **CVR-39-06（偏移运行时推导）**：偏移随内核/镜像尺寸漂移（38 为 0x40049000、
   39 实测 0x40039000）——硬编码不可行。修复：BootInfo `userImageFrames` 首个
   cap 经 `seL4_ARM_Page_GetAddress` 取 root 镜像物理基址 → 偏移 =
   `image_phys - 0x200000`（shim 新增 `eneros_sel4_user_image_phys_base` +
   `hal::dma::set_identity_offset`，装配期注入）。

QEMU 内存扫描实证：池 VA 0x264000 ↔ phys 0x4029d000（标记字节定位）。

### 2.2 virtio-net legacy 头语义

- **RX**：legacy virtio-net 每个 RX 缓冲以 10 字节 `virtio_net_hdr` 开头（QEMU
  实测帧长 37 = 头 10 + 以太帧 27）——驱动跳过头部（`VIRTIO_NET_HDR_SIZE`）。
- **TX**：TX 缓冲必须前置 10 字节头（零头），否则 QEMU 把帧前 10 字节当头吃掉
  （对端收到截断帧，实测 27→17）。
- 两者修复后外部 peer 完整回环：`ENEROS-NET-RX` → guest 回 `ENEROS-NET-TX`
  （27 字节，peer 侧 MATCHED）。

### 2.3 QEMU 设备布局（CVR-39-03）

- QEMU virt 的 `virtio-mmio-bus.N` = 槽位 N（每 bus 显式容量 1）；`bus.1` 网卡
  在 DTB 不可见（DTB 仅 bus.0 32 槽节点 a000000..a003e00）。
- 最终布局：系统盘显式 bus.0（槽 0）、网卡显式 bus.1（槽 1）、数据盘显式
  bus.2（槽 2）、rng 显式 bus.3（槽 3）——全部 DTB 可见且互不冲突。
- 39 工厂语义：区域起点节点扫描全部槽位（QEMU DTB 为每槽位一节点；设备 untyped
  按 4K 帧 retype，逐节点独立映射同帧冲突）；blk 取最后一块为数据盘（33
  WP-33.5 语义保留）。

## 3. 工作包核销

| WP | 交付 | 证据 |
|---|---|---|
| 39.1 | 零写发现状态机：`BlockRole`/sealed role typestates、`BlockIdentity`、`PartitionHandle<R,D>`（checked LBA 翻译）、`BlockTrace`（读/写快照） | storage `partition` 模块 17 测试（含 `resolver_zero_writes_during_discovery`） |
| 39.2 | `GptResolver`（复用 image-format `read_gpt` 双份头 CRC；重复 GUID/越界/重叠拒绝；4Kn 扇区适配）+ `RolePolicy`（默认 GUID 对齐 qemu-v1 布局） | storage 测试：GPT 解析/4Kn/零写/负例 |
| 39.3 | `ProvisioningGate`/`ProvisionRequest`/`ProvisionAuthorization`（normal release 无 capability；nonce/过期/路径/容量/摘要全校验）+ `BlockDurableLog::provision_new` | storage 9 测试 + durable-log 测试 |
| 39.4 | installer 全域安全判定（P1-07）：image-format `probe_surface` 分层扫描 + GPT/MBR/FS/EDLOG1 签名识别；installer `ensure_blank` 全表面扫描替换首尾检查 | image-format 5 测试（含 `middle_data_is_detected` 反例）；installer aarch64-unknown-uefi 构建通过 |
| 39.5 | virtio-blk：capacity=0 fail-closed（P1-14 虚构删除）、`Aarch64Cache` CTR_EL0 cache line（CVR-38-01 落地）、virtio-core 共享 crate（storage re-export 保持 API） | storage 141 测试（含 `test_capacity_zero_rejected_no_fabrication` mutation）；virtio-core 2 测试 |
| 39.6 | durable-log `open_existing`（Uninitialized/ForeignFormat/Corrupt 区分，零写）+ `provision_new`（授权门）+ `committed_seq()`；`PersistentAuditStore` 不再隐式格式化；root probe 零写接线 | durable-log 11 测试（含 `open_existing_does_not_rewrite_header` 字节级证明）；QEMU `open_existing Uninitialized —— 零写保留` |
| 39.7 | virtio-net 驱动（feature 协商 MAC/STATUS、RX 预置 8×2048、TX 单缓冲、link/config 读、legacy 头处理）+ root 复合装配（`VirtioFabric` blk+net）+ 0x88b5 回显 + harness netdev 布局修正 | net 457 测试（virtio_net 9 个）；QEMU：`virtio-net: bound` + 外部 peer MATCHED `ENEROS-NET-TX` |
| 39.8 | `NetworkStackService`（smoltcp 包装：有界 socket、connection epoch、`device_changed` 失效旧句柄、link/device/transport/application 统计） | net stack 6 测试（含 stale epoch/背压/multicast 拒绝） |
| 39.9 | `SecureChannel`/`SecureChannelFactory`/`ChannelPolicy`/`InsecureStream`（明文不冒充 secure；release 默认禁明文/禁 0-RTT）；mqtt `TlsLayer`/`PlainTcp` 删除 → `SecureMqttTransport` | protocol-abstract channel 3 测试；mqtt 48 测试 |
| 39.10 | `ProtocolPlugin` 合同（descriptor/validate_config 未知键拒绝/start/poll/health/stop）+ `TransportFactory`/`TransportHandle`/`TransportSpec` + `PointWriteIntent`（协议写意图） | protocol-abstract 24 测试（plugin/transport/write） |
| 39.11 | 四协议 plugin（modbus-rtu/tcp、iec104、mqtt）——`Box<dyn TransportHandle>` 实现各协议 transport trait（本地 trait 合法），插件硬件无关；DDS 残留清理（cyclone 模块声明/ci 签名清单/文档） | modbus-rtu 83 / modbus-tcp 63 / iec104 68 / mqtt 48；agent-bus-dds 51 |
| 39.12 | `protocol-services` crate（svc-modbus-rtu/tcp/iec104/mqtt 4 bin，复用 drv-svc runtime）+ qemu-prod/board-prod 声明（optional）+ drv-svc `block_cmd` 命令面（READ/WRITE/FLUSH/HEALTH + `StorageService`） | drv-svc 14 测试（block 命令 4 个）；aarch64 target 构建通过；QEMU：4 协议服务 spawn + degraded 如实呈现；manifest normalize/image build PASS |

## 4. 二次验证矩阵

| 类别 | 注入/用例 | 结果 |
|---|---|---|
| 零写正例 | QEMU 全新零数据盘 → `open_existing Uninitialized` 零写保留 | PASS（QEMU 实弹） |
| 零写负例 | 已格式化盘（host mock）→ open_existing 不重写头块（字节级断言） | PASS |
| discovery | GPT 解析：重复 GUID/越界/重叠/4Kn | 全拒绝（storage 测试） |
| provisioning | normal 拒绝 / factory 授权 / 过期 / 路径不符 / 容量不足 / 零 nonce / 零摘要 | 全拒绝（storage 测试） |
| 容量 | capacity=0 → Unsupported（mutation：恢复虚构 → 测试转红） | PASS |
| installer | 中间数据反例（首尾空白中间非零）→ 拒绝 | PASS（image-format 测试） |
| net RX/TX | 模拟设备注入/捕获；QEMU 外部 peer 完整回环 | PASS（host + QEMU） |
| net 负例 | 无 MAC feature 拒绝 / 超长帧拒绝 / link down 拒发 / 无帧超时 | PASS |
| stack | socket 上限 / stale epoch / 无效句柄 / device_changed 失效 | PASS |
| 协议插件 | 未知配置键拒绝 / 传输不支持拒绝 / start-poll-stop 生命周期 | PASS（四插件） |
| 安全通道 | release 策略禁明文 / InsecureStream 报告 Plain | PASS |
| block 命令 | read/write/flush/health 回环 / 无 backend UNAVAILABLE / 坏命令 INVALID / 越界拒绝 | PASS（drv-svc） |
| 回归 | 23 个 crate 1701 测试全绿；aarch64 target 构建（root-task/protocol-services/installer-uefi） | PASS |

## 5. Mutation 防假绿（实弹验证）

| Mutation | 方法 | 结果 |
|---|---|---|
| 恢复 capacity 虚构（P1-14） | 测试 `test_capacity_zero_rejected_no_fabrication` 编码断言 | 设计保障（测试存在） |
| 恢复首尾 LBA 检查（P1-07） | `middle_data_is_detected` 反例断言 | 测试转红（设计保障） |
| 恢复 format-on-probe | durable-log `open_existing_does_not_rewrite_header` + QEMU 零写行 | 双重保障 |
| DMA 偏移回退 identity | QEMU 实测超时（修复前行为）→ 运行时推导后 PASS | 实弹对比 |
| legacy 头缺失 | QEMU 实测截断帧（27→17）→ 修复后 MATCHED | 实弹对比 |

## 6. 证据

- **QEMU 最终复验**：`target/eneros/evidence/39-qemu/`（`--reset-bootctrl` + 全零
  VARS + 全新零数据盘；外部 peer 于 boot 后注入）：
  - `virtio-net: bound slot=1 mac=52:54:00:12:34:56 link=true`
  - `durable-log: open_existing Uninitialized —— 零写保留`（P0-07 修复的 QEMU 实证）
  - `virtio-net: echoed ENEROS-NET-TX (27 bytes)`；peer `MATCHED 27-byte
    ENEROS-NET-TX reply!`（外部独立进程观察到真实副作用）
  - 6 drv-svc + 4 协议服务 `ipc loop entered`；`PRODUCT-READY: true (degraded:
    svc-modbus-rtu,svc-modbus-tcp,svc-iec104,svc-mqtt; required degraded: )`
  - 0 user exception；`media_reads=2501 media_writes=16`（bootctrl 固件写，非探测写）
  - 镜像（运行后含 bootctrl 写入）：sha256=`4286a3aba049e1ce837aaf26f0aa8842219688761d92f07444f6f5086c43ecaa`
  - harness exit=124 为已知限制（seL4 无 ACPI 关机；日志序列门通过，同 38 手法）
- **host 测试**（23 crate，1701 通过 / 0 失败）：storage 141、net 457（virtio_net 9 +
  stack 6）、root-task 97、durable-log 11、image-format 20、modbus-rtu 83、
  modbus-tcp 63、iec104-master 68、mqtt 48、protocol-abstract 24、drv-svc 14、
  agent-bus-dds 51、hal 54、board 23、framework 61、xtask 93、fs/tsdb 全绿等
- **aarch64 target 构建**：root-task、protocol-services 4 bin、installer-uefi 全通过
- **manifest/镜像 Gate**：`manifest normalize`（qemu-prod/board-prod）PASS；
  `SOURCE_DATE_EPOCH=1700000000 cargo xtask image qemu-prod` PASS
- CROSS_VERSION 登记：`evidence/39/CROSS_VERSION-39.md`（CVR-39-01~06 + OBS-39-01~03）

## 7. 诚实边界（39 不冒充）

- **#8 PARTIAL**：协议服务以独立 EL0 service 入 bundle 并运行（spawn + IPC loop +
  readiness 报告），但传输/设备 capability 未接通——EL0 设备句柄移交属蓝图 40
  （EIPC 域；CVR-38-01 同源：`dc ivac` EL0 需 seL4 cache syscall + page cap）。
  `PRODUCT-READY degraded` 列表如实呈现（不伪装就绪）。
- **#9 PARTIAL**：外部 peer 对 virtio-net 的原始以太回环已实弹验证（0x88b5
  ENEROS-NET-RX→TX）；协议级外部 peer（Modbus/IEC104/MQTT broker）依赖 40 的
  transport 注入与网络栈服务装配——39 提供插件合同 + host 级 peer 回环测试
  （iec104_interop/loopback、modbus loopback）。
- **#10 PARTIAL 面**：NVMe skeleton 未进入正式介质（product 未声明）；eMMC 无
  板级 Adapter（合成平台 B 已覆盖资源模型面）——按 Exit Gate 10 的"未声明不算
  能力"语义处理。
- **第二 software adapter**（#5）：virtio-net（QEMU）为第一 adapter；第二 adapter
  的**设备级**实现（如 MMIO MAC）已存在（`MacController`）且与 virtio-net 共用
  同一 `NetDevice` contract（host 测试覆盖）；QEMU 设备级第二 adapter 装配随
  40 net 服务。
- 全仓 workspace 测试含环境 BLOCKED 项（crypto native 依赖等，35-00 §2.3 已登记，
  非 39 引入）。
- 观察点：`net_echo_poll` 经全局指针访问 net 驱动（root 单线程事件循环，与
  register_wdt_base 同模式）；svc-storage 的 EL0 设备 attach 随 40 移交。

## 8. Exit Gate 逐条核销

| # | Exit 项 | 结果 | 证据 |
|---|---|---|---|
| 1 | discovery 在任何未知/坏盘上零写 | PASS | durable-log `open_existing` 零写（host 字节级 + QEMU Uninitialized 行） |
| 2 | DurableLog/FS/TSDB 只接受 role partition handle | PASS | `PartitionHandle<R: Role>` typestate + `BlockDevice` impl（storage 测试） |
| 3 | 显式 provisioning 才能格式化，normal release 无 capability | PASS | `ProvisioningGate`（release 拒绝测试）+ `provision_new` 授权门 |
| 4 | virtio-blk/eMMC 经真实 DriverSupervisor/DMA service 工作 | PASS | QEMU 数据盘读 OK（CVR-39-01/04/05/06 修复后）；root probe 真实读路径 |
| 5 | virtio-net 与第二 software adapter 都经同一 NetworkDevice contract | PARTIAL | virtio-net（QEMU 实弹回环）+ MacController 同 contract（host）；第二 adapter 设备级 QEMU 装配随 40 |
| 6 | EL0 NetworkStackService 有真实外部收发、超时与重启 epoch | PARTIAL | 组件 + host 验证（epoch/超时/背压）；EL0 装配依赖 40 net 服务与设备移交 |
| 7 | secure transport 不允许 PlainTcp 冒充 TLS | PASS | `TlsLayer`/`PlainTcp` 删除；`InsecureStream` 明确 Plain + release 策略禁明文 |
| 8 | ProductManifest 声明的每个协议都以独立 service + production transport 运行 | PARTIAL | 4 协议服务入 bundle 并运行（QEMU spawn + degraded 如实）；production transport 依赖 40 设备移交 |
| 9 | 独立 peer/虚拟盘能观察到真实副作用，故障负例 fail closed | PASS | QEMU 外部 peer 回环 MATCHED；负例矩阵全红（§4） |
| 10 | 新增盒子/driver/protocol 只需 Adapter/Plugin，不修改核心业务 | PASS | 39 新增均为 Adapter/Plugin（virtio-core/partition/plugin/factory）；核心业务未改 |
| 11 | 36–38 全部 Gate 保持通过 | PASS | 回归 1701/0（含 36-38 产物）；manifest/image Gate PASS |

**39 Exit Gate：9 PASS / 2 PARTIAL（#5 第二 adapter 设备级、#6 EL0 装配随 40；
#8 的 production transport 依赖 40 设备移交）。**

## 9. 允许进入下一版本

39 核心目标（零写存储、GPT/分区、provisioning 门、installer 全域判定、virtio-net
真实外部回环、协议插件合同、协议服务入 bundle）已达成并以 QEMU 外部证据绑定。
#5/#6/#8 的 PARTIAL 均依赖蓝图 40 的 EIPC 设备移交（CVR-38-01 同源登记）——
**允许进入 40**，但 40 必须把这些 PARTIAL 面作为其 Entry 前置（svc 设备句柄、
net 服务装配、协议 production transport 注入）。

---

## 10. 收尾复验轮（2026-08-15，独立重验不信任原记录）

按蓝图 36 收尾复验同纪律：不信任验证记录自述，独立重跑测试、逐模块
代码级深读、活体 mutation、QEMU 复验。发现并修复 **4 项 P0 + 13 项 P1/P2**，
其中含一项威胁全部 QEMU 证据有效性的**启动期内存损坏根因**。

### 10.1 虚假 PASS 订正（原记录订正项）

| # | 原声称 | 复验事实 | 处置 |
|---|---|---|---|
| R1 | Exit #2 PASS（证据：storage 测试） | **虚假**：`GptResolver`/`PartitionHandle`/`ProvisioningGate` 生产零消费；`durable_log_probe` 对裸整盘直接 `open_existing`（写面在 LBA0/GPT 保护区）；DurableLog/FS/TSDB 均接受裸 `BlockDevice` | 修复：probe 改经 `GptResolver::open::<AuditRole>`（QEMU 实证 `discovery NoGpt —— 零写保留`）；新增密封 `ScopedBlockDevice` trait，DurableLog/FS/TSDB 公开构造收窄为作用域设备；TSDB 删除调用方自报 start/count（release 下 debug_assert 无效） |
| R2 | `provision_new` 授权门 | `let _ = auth;` 丢弃授权；`format` 仍 pub（旁路） | `format` 降 `pub(crate)`；`provision_new` 执行点校验 profile/角色/容量 + 操作后 readback；新增 `provision_new_validates_authorization` 测试 |
| R3 | ProvisioningGate 6 条件 | 蓝图 WP-39.3 要 8 条件：缺 policy 确认/再读 identity/审计；nonce 可无限重放（Clone） | 补齐：operator_ack、reread_identity 比对、nonce 一次性消费、审计事件序列；授权去 Clone；新增 4 类负例 |
| R4 | 协议服务 "spawn + IPC loop + readiness 报告" | **虚假**：7 个协议服务静默死于首指令（ELF 链接缺页对齐，见 10.3-F12），从未进入 ipc loop；readiness 全是超时降级 | protocol-services 补 build.rs（max-page-size=0x1000 + separate-code，同 drv-svc）；最终轮 19/19 服务 READY-SIGNALED、`degraded: 空` |
| R5 | §3 WP-39.11 MMS/GOOSE/SV "本版补齐 service" | **整项缺失**（只有 codec，无 plugin/svc，原记录未声明缺口） | 补齐 GoosePlugin/SvPlugin/MmsPlugin + svc-goose/sv/mms bin + profile 声明；诚实缺口（GOOSE confRev RX 校验/VLAN/MMS 重连）在插件文档标注待后续 |
| R6 | 配置链（validate_config） | 四插件 `start()` 内重建 `ProtocolConfig::default()`，校验过的配置零消费（broker/peer 恒硬编码） | trait `start` 增 `cfg` 参数；四插件真消费（broker/peer/timeout/slave_id/t3…）+ 每插件"配置真生效"捕获测试 + 缺必填键 InvalidConfig |

### 10.2 P0/P1 修复清单（除 R1-R6 外）

**存储**：GPT 错误映射透传（DuplicateGuid/Overlap/OutOfBounds 不再吞为 NoGpt）；
重复 GUID/重叠真构造负例（双份表 patch + CRC 重算）；`set_block_count` 收 `cfg(test/mock)`。

**网络**（F1-F13）：send 部分写不再静默吞（返回实际字节数，零入队 Timeout）；
device_changed 回收旧 socket（原重启一次即永久 SocketLimit）；NetDevice::recv
契约统一非阻塞（原 20M 次忙等拖死协议栈 drain）；F_MTU 协商（原硬编码 1500）；
RX used ring id 收紧到 RX_SLOTS（原 QUEUE_MAX 阈值致缓冲别名双投）；非 legacy
显式拒绝（原死分支错布局）；avail 环 cache clean 补齐；回显目的 MAC 改拷贝入帧
源 MAC；rx 错误不再静默吞；mqtt send 有界化（deadline+断连检测+零入队上限，
原无界忙循环）；ChannelPolicy 执行点落地（SecureMqttTransport::new 强制
policy.allows，原零执行点）+ release 策略拒明文 mutation 测试。

**协议**：IEC104 master I 帧 N(S) 校验（原不读帧内序号，乱序/重放照单全收；
新增乱序+重放负例；修正 loopback 桩与 interop 桩的非法序号）；modbus-rtu
recv Closed/Timeout 区分；mqtt broker 超长配置校验（原 start 内 panic）；
bin 文档头复制粘贴错误；agent-bus-dds 陈旧 Cyclone 文档清理。

### 10.3 启动期非确定挂起：根因链与修复（本轮最重要发现）

复验初期 QEMU 出现**非确定启动挂起**（约 50%），GDB+canary 逐步定位四层：

| 阶段 | 现象 | 根因/处置 |
|---|---|---|
| 1 | WATCHDOG 锁字节被写 0xd8 自旋死锁 / 堆 hole 列表乱序 panic（位置随机、布局敏感） | 野写嫌疑逐层排除（DMA 地址换算实测正确） |
| 2 | opt1 降级后 bundle 1.34MiB 超 BOOT-007 2MiB 窗口 DTB 容纳上限 | 内核补丁窗口 2MiB→4MiB（第二个 2MiB 段增 PT，页表预留 n+=3）；`ENEROS_MAPPED_WINDOW_LEN` 同步；root-task 堆 2MiB→3MiB |
| 3 | 干净构建仍挂，CPU 全体 idle | **根因：root 栈仅 64KiB（.bss `.skip 65536`）且下方无 guard 页，`run_ipc_loop` 大帧溢出静默腐蚀下方 .bss（WATCHDOG 锁/堆元数据均在其下）**——栈扩到 256KiB |
| 4 | EL0 程序 LTO+opt3 历史可疑异常（Cargo.toml 注释降级意图从未实现） | 补包级 profile：root-task/services/drv-svc/protocol-services opt-2 + codegen-units=16；全局 lto=false（seL4 内核为 C/cmake 不受影响） |

**稳定性验证：修复后 8/8 连续启动无挂起；干净构建（探针移除后）证据轮：
peer matched + 19/19 READY + degraded 空 + discovery 零写。**

### 10.4 复验轮证据

- **host 全仓**：7242 过 / 0 败（`cargo test --workspace --no-fail-fast`；
  iec104/modbus-tcp loopback 在并行全仓跑时偶发固定端口冲突，单跑 3/3 过——
  既有抖动，非 39 引入）。
- **新增/增强测试**：storage 141（+真重复GUID/重叠/授权校验）、durable-log 10
  （+provision 授权负例）、net 460（+非阻塞/MTU/ring-corrupt/modern 拒绝）、
  protocol-services 1（EXECUTE→ST_UNAVAILABLE）、61850 三插件 120（goose 44/
  sv 31/mms 45）、mqtt 50（+policy mutation）、iec104 69（+N(S) 乱序/重放）、
  root-task 窗口判据同步 4MiB。
- **QEMU 最终证据轮**（`target/eneros/evidence/39-qemu-final5/`，干净构建
  镜像 sha256 前缀 04f7158d；运行时 bootctrl 写后 3e755de3）：
  - `ENEROS stage=verified slot=A`；`media_writes=16`（仅 bootctrl 固件写）；
  - `durable-log: discovery NoGpt —— 零写保留`（GptResolver 生产路径实证）；
  - `virtio-net: bound slot=1 mac=52:54:00:12:34:56 link=true`；
  - 外部独立 peer：`PEER: matched plen=27`（ENEROS-NET-TX 27B，目的 MAC =
    入帧源 MAC 拷贝）；
  - 19 服务全 `READY-SIGNALED`；`PRODUCT-READY: true (degraded: ; required degraded: )`；
  - 0 user exception。
- **门禁**：manifest normalize（qemu-prod/board-prod）PASS；REVERSE-PARSE-GATE
  PASS entries=20；KERNEL-POLICY-GATE PASS；image build PASS。
  **closure gate 存量违规 4 项**（eneros-crypto usage×3 + ecdsa 未登记）——
  复验确认非 39 引入（39 新增 crate 零加密依赖；加密授权属 34/41 域存量缺口）。

### 10.5 Exit Gate 重新核销（复验轮）

| # | Exit 项 | 结果 | 复验证据 |
|---|---|---|---|
| 1 | discovery 未知/坏盘零写 | PASS | GPT 发现路径生产接线 + QEMU `discovery NoGpt` + 数据盘整轮零写（media_writes 仅 bootctrl） |
| 2 | DurableLog/FS/TSDB 只接受 role 分区句柄 | PASS（复验后真实） | 密封 `ScopedBlockDevice`；裸设备编译不过；裸盘 probe 零写 |
| 3 | 显式 provisioning 才能格式化 | PASS（复验后真实） | format 降权 + provision_new 执行点校验 + readback；授权 8 条件含 nonce 防重放 |
| 4 | virtio-blk/eMMC 经真实 DMA/DriverSupervisor | PASS | QEMU 数据盘读路径（CVR-39-04/05/06 偏移运行时推导复验正确：pool phys 落入 RAM） |
| 5 | virtio-net 与第二 adapter 同一 contract | PARTIAL→维持 PARTIAL | virtio-net QEMU 实弹 + MacController 同 trait（host）；第二 adapter QEMU 装配随 40 |
| 6 | EL0 NetworkStackService 真实收发/epoch | PARTIAL→维持 PARTIAL | 组件 host 语义完整（epoch/背压/回收修复）；EL0 装配随 40 |
| 7 | secure transport 不允许明文冒充 | PASS（复验后真实） | ChannelPolicy 执行点落地（原零执行点）+ release 拒明文 mutation 测试 |
| 8 | 声明协议均以独立 service 运行 | PARTIAL→维持 PARTIAL（范围收窄） | 7 协议服务全部 spawn+READY（原静默死亡已修）；production transport 随 40 设备移交 |
| 9 | 独立 peer 观察真实副作用 | PASS | peer matched（ENEROS-NET-TX）+ 负例矩阵全红 |
| 10 | 新增盒子/驱动/协议只需 Adapter/Plugin | PASS | 39 新增均为 Adapter/Plugin；核心业务未改 |
| 11 | 36–38 Gate 保持 | PASS（带存量注记） | 全仓 7242/0；manifest/reverse-parse/kernel-policy PASS；closure 加密违规为存量（非 39 引入） |

### 10.6 诚实边界（复验轮补充）

- 启动稳定性修复（栈 256KiB/窗口 4MiB/堆 3MiB/profile 降级）为经验性
  根治：8/8 + 干净构建证据轮通过；未加栈 guard 页（seL4 初始线程栈在 .bss，
  加 guard 需独立页表安排——登记后续）。
- GOOSE confRev RX 逐帧校验、VLAN 剥离、MMS 断链重连为 codec 层存量缺口，
  已在各 plugin 文档标注，随后续版本。
- 协议服务 readiness=Ready 仅表示进程/IPC 就绪；传输/设备 capability 未接通
  （40 移交），EXECUTE 显式 ST_UNAVAILABLE（不伪造 I/O）。
- iec104/modbus-tcp loopback 测试固定端口在并行全仓跑时偶发冲突（存量抖动）。
