# RECON-39 — 存储、网络与全协议适配修复蓝图侦察记录

> schema: `eneros.blueprint-39.recon.v1`
> 侦察基线：`HEAD=b96774e` + 当前工作树全部未提交/未跟踪内容（195 个 git 状态变化）
> 分支：`feature/v0.165.0-protocol-driver-sdk`
> 侦察日期：2026-08-14
> Entry Gate 复核：38 Exit Gate 10/10 PASS（`evidence/38`，38-review2 复验含 QEMU）——允许进入 39

## 1. 块存储现状（WP-39.1~39.6 输入）

### 1.1 探测期写盘（P0-07 核心事实）

- `crates/runtime/root-task/src/driver_probe.rs:358-397`：每次启动对"最后一块 virtio-blk"执行
  `BlockDurableLog::format`（magic 不符即写头块到 LBA0）→ `recover()` → `append(b"boot")`，
  并把 append 序号注册为全局 boot_id（`panic::register_boot_id`）。**唯一生产 format 调用点**。
- `crates/runtime/durable-log/src/lib.rs:112-132`：`format` 幂等——magic 不匹配时直接写初始头
  （隐式格式化）。不存在 `open_existing`/`provision_new` 区分。
- `crates/runtime/durable-log/src/lib.rs:270+`：`PersistentAuditStore::new` 同样走 `format`
  （自动格式化）；全仓无生产 caller（仅本 crate 自用），可安全改为 open_existing 语义。

### 1.2 块设备与 GPT

- `crates/boot/image-format/src/gpt.rs`：完整 GPT 读/写/校验（双份头 + CRC、Protective MBR、
  `read_gpt<D: image-format::media::BlockDevice>` + `parse_gpt(image, sector_size)` 纯函数）。
  image-format 仅依赖 ipc-wire，no_std。storage → image-format 依赖无环。
- `product/image-layouts/qemu-v1.json`：EnerOS 分区 type GUID 命名空间 `454e4552-000N-4001-8000-00000000000N`：
  PLATFORM_BOOT=ESP、BOOTCTRL=...02、BOOT_A=...03、BOOT_B=...04、SYS_A=...05、SYS_B=...06、
  RECOVERY=...07、CONFIG=...08、DATA=...09、LOGS=...0a。
- `crates/drivers/tsdb/src/block_store.rs`：`TSDB_PARTITION_GUID` + 分区内偏移寻址（无生产 caller）。
- storage crate 无 `BlockDeviceHandle`/`PartitionHandle`/`BlockRole`/GPT resolver——全部新建。

### 1.3 virtio-blk 驱动

- `crates/drivers/storage/src/driver/virtio_blk.rs:368-372`：capacity=0 时虚构
  `max_transfer_blocks*16=4096` 块（P1-14）。
- `virtio_blk.rs:277-309`：`Aarch64Cache::clean/invalidate` 硬编码 64B cache line（CVR-38-01；
  `dc ivac` EL0 非一致路径已被 38 fail-closed 拒绝，cache line 修复仍须落地供真机）。
- VMMIO 寄存器偏移/ring 类型（VirtqDesc/VirtqAvail/VirtqUsed/VirtioMmio/DmaPool）均在 storage——
  net 需要共享（方案：抽取 `crates/drivers/virtio-core`，storage re-export 保持 API）。
- storage features：`mock`/`emmc-ram-sim` release 均 compile_error 熔断（铁律已立）。

### 1.4 installer

- `crates/boot/installer-uefi/src/main.rs:291-311` `ensure_blank`：只读 LBA0 与 last_block，
  任一非零拒绝（P1-07）；全 crate 无 `--force`。写入后有读回 digest 校验（保留）。

## 2. 网络现状（WP-39.7~39.9 输入）

- **QEMU harness 已内置 virtio-net**：`xtask/src/qemu_harness.rs:816-824`（G3）——
  `-netdev socket,id=enerosnet,listen=127.0.0.1:5577` + `virtio-net-device,bus=virtio-mmio-bus.1`，
  由 `options.net_present`（默认 true）门控。槽 3，位于两块 virtio-blk 之后（槽 0/2 不变）。
- `tools/sel4-net-peer.py` / `tools/net_peer.py`：外部 peer 已存在——连接 5577，发送
  EtherType=0x88b5 + payload `ENEROS-NET-RX`，期望收到 `ENEROS-NET-TX`。
  **全仓 Rust 无任何代码处理 0x88b5 帧**（peer 脚本为孤件，root 无 vnet_rx）。
- `crates/drivers/net`：no_std + smoltcp v0.13（tcpip/）+ v0.29 socket 抽象 + `NetDevice` trait
  （mac.rs:86）+ MMIO MAC 驱动（非 virtio）。**无 virtio-net 驱动**（要新建）。
- `drv-svc/src/bin/svc-drv-net.rs`：EL0 服务壳（UnassignedService，capability None →
  EXECUTE→ST_UNAVAILABLE——38.9 语义：不伪造设备 I/O）。
- `mqtt/src/real.rs:199-220`：`TlsLayer` trait 由 `PlainTcp` 实现（明文直通）——"名字含 Tls 但无
  TLS 语义"的诚实命名仍不合格（WP-39.9：明文只能实现 `InsecureStream`，release policy 默认禁止）；
  `TlsTransport` 组合只存在于注释；`tls` feature 为空。
- `crates/security/tls`：rustls host 侧（std），无 `SecureStream`/`SecureChannelFactory`。
- VIRTIO_REGION_BASE 常量位于 driver_probe.rs:94（OBS-04：39 迁至 DeviceResources）。

## 3. 协议现状（WP-39.10~39.12 输入）

- `protocol-abstract`（唯一 seam，ADR-0026）：`PointAccess`/`AuthorizedWriter`/`ProtocolAdapter`
  （init→start→poll→stop + state）/`ProtocolManager`。**无 ProtocolPlugin/TransportFactory/
  PointWriteIntent/SecureChannel**——全部新建（35-00 §10.2 固定 seam 清单要求）。
- 各 crate transport 已是"泛型 trait + 构造注入"且 feature 门控、mock 熔断：
  modbus-rtu `Rs485RtuTransport<H: UartHw>`（transport.rs:20）、modbus-tcp
  `SmoltcpTcpTransport<D: NetDevice>`（transport.rs:64）、iec104-master
  `RealMasterTransport<D: NetDevice>`（real.rs:29，STARTDT/TESTFR/t3 已实现）、mqtt
  `NetTcpTransport<D>` + `set_transport(Box<dyn MqttTransport>)`、mms/goose/sv 各有
  `NetMmsTransport`/`RawL2Transport`。
- tsn-time：trait-only（ADR-0040），`dataplane_available()` 恒 false（显式降级，保留）。
- `iec62351`：零 Rust consumer（39 只做 transport 接线面；身份/密钥服务归 41）。
- interop 脚本已是 PENDING 非零退出（36 止血完成），本卷不触碰（42 重建）。
- configs：modbus/iec104/mqtt 无 toml；61850 系模板标注"无 Rust parser"（41 归配置消费）。
- `protocol-sdk`：零消费者的活叶 crate（ADR-0025 收敛为 wire codec 客户端）；`ProtocolPlugin`
  trait 仍在但被 ADR-0026 排除出 seam 候选——39 的 ProtocolPlugin 合同按 35-00 §10.2 新建于
  protocol-abstract（语义不同：服务级生命周期 + 配置校验 + transport 工厂）。
- DDS 残留（P1-25/P1-29）：`ci/src/gate.rs` 签名清单含 `CycloneDdsNode/MockDdsNode`；
  `agent-bus-dds` 其余源文件为纯 Rust 消息签名层（无 cyclone 依赖，保留为 41 AgentBus 候选）；
  `grid_agent/src/real.rs` 引用 DdsNode（41 域）。

## 4. 装配机制（WP-39.12 输入）

- 36 已交付 profile v2 + ProductManifest + `product_deriver`（bundle 从 manifest 派生，
  `verify_media_bundle` 反解析比对）。qemu-prod.json/board-prod.json 为 `eneros.product-profile.v2`，
  services 数组已含 root + demo + drv-svc 6 bin + svc-pie×2（13 服务）。
- `drv-svc/src/runtime.rs` 全 pub：`DriverRequest/DriverResponse` 信封、`BoundedQueue`、
  `DriverService` trait、`UnassignedService`、`driver_service_main`——协议服务可复用
  （OP_EXECUTE=0x10 注释明示"具体命令由 39 定义"）。
- 38 已交付：PlatformInventory/DeviceResources（含 mmio/irq/clocks/resets/gpios/dmas）、
  DriverSupervisor（bind→map→init→start→health）、HalPool（DMA 唯一 seam）、
  FDT walker（单级 interrupt-parent；interrupt-map 登记 CVR-38-04 待 39+）。

## 5. 保护清单（未提交用户代码，禁止破坏）

- 38 全部未提交产物（root-task main/service/spawn/bundle/supervisor、hal、board、framework、
  drv-svc、xtask evidence/qemu_harness 等）——39 只经 39-owned 路径触碰 driver_probe.rs 的
  存储/net 分支与主循环挂接点；不动 spawn/页表/服务 spawn 语义。
- `tools/sel4-net-peer.py`、`qemu_harness.rs` netdev 段（36/33 G3 遗留）——保留并复用。
- iec104/net/tsn 未提交改动（config.rs、socket/api.rs 等）——只读，不重写。

## 6. 实现方案（锁定）

1. 新建 `crates/drivers/virtio-core`（no_std，零依赖）：VMMIO 常量 + ring 类型 + VirtioMmio/DmaPool
   trait；storage re-export（对外 API 不变）。
2. storage 新建 `src/partition.rs`：BlockRole + sealed Role typestates、LbaRange、BlockIdentity、
   PartitionIdentity、PartitionHandle<R: Role>（checked LBA 翻译）、RolePolicy（默认 GUID 映射
   对齐 qemu-v1.json）、GptResolver（复用 image-format read_gpt + 重复 GUID/越界/重叠拒绝）、
   ProvisioningGate/ProvisionRequest/ProvisionAuthorization（factory/recovery 才可授权；
   normal release 拒绝）、BlockTrace（记录读/写 LBA，测试断言 discovery 零写）。
3. durable-log：`open_existing`（零写；Uninitialized/ForeignFormat/Corrupt 区分）+ `provision_new`
   （auth 门控）+ `committed_seq()`；`PersistentAuditStore::new` 改 open_existing。
4. root probe：virtio 区域 base 改经 DeviceResources；blk 分支 open_existing（零写探测，
   未初始化不写盘、boot_id 归零）；net 分支（device_id=1）绑定 VirtioNetDriver；主循环挂
   0x88b5 回显（ENEROS-NET-RX→TX），QEMU socket netdev 外部 peer 观察真实副作用。
5. net 新建 `src/virtio_net.rs`（VirtioNetDriver<Regs,Pool> 实现 NetDevice：feature 协商、
   RX 预置描述符、TX 提交/等待、MAC/STATUS 从 config 空间读、link up 判据）+ host mock 设备测试；
   `src/stack/service.rs` NetworkStackService（smoltcp 包装 + 有界 socket + connection epoch）。
6. protocol-abstract 新建：`write.rs` PointWriteIntent、`transport.rs`
   TransportKind/Spec/Handle/Factory、`plugin.rs` ProtocolPlugin/ProtocolDescriptor/ProtocolConfig/
   ProtocolHealth/WorkBudget/Progress/PointRegistry、`channel.rs`
   SecureChannel/SecureChannelFactory/ChannelPolicy/Endpoint/IdentityRef/ChannelError。
7. mqtt：删除 `TlsLayer`/`PlainTcp` → `InsecureStream`（实现 SecureChannel，security=Plain，
   release policy 默认拒绝）+ `SecureMqttTransport` 组合（SecureChannel 包裹 MqttTransport）+ 工厂。
8. modbus-rtu/modbus-tcp/iec104-master/mqtt：各自 `plugin.rs` 实现 ProtocolPlugin（包装现有
   adapter/transport）+ TransportHandle impl + 具体 TransportFactory；host 测试（validate_config
   未知字段拒绝、start/poll/health/stop）。
9. 新建 `crates/protocols/protocol-services`：bins svc-modbus-rtu/svc-modbus-tcp/svc-iec104/
   svc-mqtt（复用 drv-svc runtime；无设备 capability → 诚实 UNAVAILABLE）；qemu-prod.json/
   board-prod.json 声明（39 允许新增 profile 数据）。
10. drv-svc runtime 增 block 命令面（READ_BLOCK/WRITE_BLOCK/FLUSH opcodes + 有界 payload），
    svc-storage 带 backend 时真实执行（host mock backend 测试），无 device → ST_UNAVAILABLE。
11. installer：`ensure_blank` → 全表面扫描（image-format 新增可 host 测试的 `probe_surface`，
    分层扫描 + GPT/MBR 签名识别，任何未知非零内容拒绝）。
12. virtio_blk：capacity=0 fail-closed（删除虚构）；Aarch64Cache 改 CTR_EL0 cache line。
13. DDS 残留：`ci/src/gate.rs` 签名清单移除 CycloneDdsNode/MockDdsNode 引用。
14. 二次验证 + mutation + 回归 + QEMU（零写探测/网络回显/协议服务 spawn）+ Exit Gate 核销。
