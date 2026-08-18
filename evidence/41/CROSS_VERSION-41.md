# CROSS_VERSION_CHANGE_REQUEST — 41

## CV-41-1：KernelRootCNodeSizeBits 12 → 13（seL4 内核构建配置）

- **当前版本**：41（业务服务装配）
- **目标文件/接口**：`third_party/sel4-build.sh`、`third_party/sel4-build-rust.sh` 的 cmake 参数 `-DKernelRootCNodeSizeBits`（seL4 内核构建配置，37 owner）
- **为什么无法通过现有接口完成**：41 新增 8 个业务/运维服务后 qemu-prod 共 34 个 EL0 服务；root-task spawn 期 root CSpace（12 bits = 4096 槽）在 svc-device-agent（第 26 个服务）处 `NoCapSlot` 挂起（QEMU 实测 root_slot=4089）。capability 槽是每服务必需的基础资源，无法通过 profile 或服务侧接口压缩（K3.1 门禁要求 svc-hello/svc-fault 保留，39/40 门禁要求协议/控制服务保留）。
- **最小修改范围**：两处 cmake `-D` 参数 +1 bit（4096→8192 槽）；不改任何 Rust 代码与 CSpace 语义。
- **影响后续版本**：42/43 只复验；root cnode 更大不改变任何已验收行为（槽号仍从 0 单调分配）。
- **回归测试**：QEMU 全量 spawn（34 服务）+ K3.1 序列 + 41 服务标记。
- **回退方式**：删除该 `-D` 参数并重跑 sel4-build-rust.sh（回到 12 bits）。

## CV-41-2：root-task 堆 4 MiB → 5 MiB（编译期默认）

- **目标文件**：`crates/runtime/root-task/src/allocator.rs`（37 owner）
- **原因**：init bundle 2.23 MiB + 34 服务 staging，OOM-DIAG used=4111472 时 89KB（svc-config ELF）申请失败；`ENEROS_HEAP_SIZE_BYTES` 注入路径不变（默认值上调）。
- **回归**：QEMU 启动通过；kernel 早期崩溃阈值以 QEMU 复验为准（40 已 4 MiB 实跑，5 MiB 复验通过）。
- **回退**：改回 4 MiB。

## CV-41-3：svc-storage 存储句柄移交（CVR-39-02 剩余面实施）

- **目标文件**：`crates/runtime/root-task/src/spawn.rs`（configure_service 特判 svc-storage）、`crates/drivers/drv-svc/src/blk_backend.rs`（新）、`crates/drivers/drv-svc/src/bin/svc-storage.rs`、`crates/drivers/drv-svc/src/runtime.rs`（REQUEST_LEN div_ceil 修复）、`crates/drivers/drv-svc/Cargo.toml`（+eneros-storage）
- **为什么**：CVR-39-02 登记"EL0 服务无块存储句柄"为 41 Entry 前置；41 实施移交——root spawn svc-storage 时映射 virtio-blk 数据盘（槽 2 = 0x0a000400）MMIO 页 + 9 页连续 DMA 帧（1:1 物理映射，虚拟=物理），服务侧初始化 VirtioBlkDriver 真后端（capability "block"）。
- **关键修复**：
  1. MMIO 槽偏移：retype 从 untyped freeIndex 0 分配 → 帧基 = 槽 0（0x0a000000），服务 MMIO 基址须加槽 2×0x200（0x40000400）——否则读到系统盘（QEMU 实测 health=128MB）；
  2. DMA 1:1 映射（虚拟=物理）——固定虚拟+任意物理组合下 desc 错位（QEMU 实测超时）；pool_mark 验证（root 写 0xDEADBEEF 服务读到）确认物理一致；
  3. drv-svc 服务端 MR 读取 `REQUEST_LEN/4` → `div_ceil(4)`（83B 非 4 倍数截断到 80B → decode 失败）；
  4. root virtio_factory 不再绑定 blk（数据盘归 svc-storage 独占；避免 freeIndex 冲突/STATUS 覆盖）。
- **回归**：host 全仓 373 套件全绿；QEMU PRODUCT-READY true（34/34）；svc-storage cap=Some("block")；svc-ota EIPC GRANTED。
- **回退**：恢复 UnassignedService（svc-storage 无后端 → 显式 Unavailable）。

## CV-41-4：Sel4IpcClient::drv_call（4 字节/word MR 协议）

- **目标文件**：`crates/agents/agent-sdk/src/ipc_sel4.rs`
- **为什么**：drv-svc 服务端（driver_service_main）按 4 字节/word 打包 MR；agent-sdk `request_raw` 是 8 字节/word——混用错位（QEMU 实测 op=0）。新增 `drv_call`（4B/word，与 drv-svc 信封协议一致）。
- **回归**：BlockIo 4/4 测试；QEMU svc-ota→svc-storage EIPC 信封解析正确（op=16）。
- **回退**：删方法（BlockIo 改回 request_raw 则协议错位）。

## CV-41-5：业务服务 durable 落盘（BlockIo + svc-ota PERSIST/LOAD）

- **目标文件**：`crates/agents/business-services/src/block_io.rs`（新）、`src/bin/svc-ota.rs`（0x06 PERSIST / 0x07 LOAD）、`Cargo.toml`（+sel4 feature）
- **为什么**：Exit Gate #3/#4/#8/#10 要求业务服务 durable 状态真落盘；经 EIPC 调 svc-storage 块命令面（OP_EXECUTE + BLK_CMD_*）。
- **关键修复**：BlockIo epoch 吸收语义（首响应吸收，后续变化拒绝）；TrialState::decode 解析 generation 8B 前缀（RegionStateStore::write_snapshot 布局）；EIPC 授予表 +6 条（svc-*→svc-storage）。
- **回归**：RegionStateStore CAS→重启恢复集成测试（host 4/4）；QEMU svc-ota GRANTED。
- **回退**：删 block_io 模块 + 还原 svc-ota（存储显式 Unavailable）。

## CV-41-6：mgmt-api ApiResponse::to_json status 修复

- **目标文件**：`crates/observability/mgmt-api/src/schema.rs`
- **为什么**：`to_json` 用 `status as u16`（枚举判别值 0）而 `code()` 是 200——41 transport 修真遗留不一致（ops-gate 测试暴露）。
- **回归**：mgmt-api 45/45 + ops-gate 3/3。
- **回退**：改回判别值（JSON status 语义错误）。

## CV-41-7：qemu-prod 数据盘为 16MB（run41.sh 复验布局）

- **目标文件**：`evidence/41/run41.sh`
- **为什么**：41 复验数据盘 16MB（与 qemu harness 默认一致）；svc-storage SELFCHECK health=32768 块确认读到数据盘。
- **回退**：改回 256MB（39 布局）。

## CV-41-8：virtio-mmio legacy 寄存器偏移修正（39-owned virtio-core + rng 驱动）

- **目标文件**：`crates/drivers/virtio-core/src/lib.rs`（`VMMIO_GUEST_PAGE_SIZE`/`VMMIO_QUEUE_ALIGN`）、`crates/security/crypto/src/rng/virtio_rng.rs`、`crates/drivers/drv-svc/src/lib.rs`（`vmmio` 模块）
- **为什么**：QEMU 9.2.4（Linux UAPI `virtio_mmio.h`，与 C 参考 `root_task.c` 一致）legacy 布局中 `GUEST_PAGE_SIZE=0x028`、`QUEUE_ALIGN=0x03C`；Rust 侧写成 `0x08C`/`0x0AC`（均为无效偏移，QEMU 忽略并 LOG_GUEST_ERROR）。`GUEST_PAGE_SIZE` 未写入 → QEMU `guest_page_shift` 恒 0 → `QueuePFN` 写入后不左移 → desc 表地址 = PFN 值本身（缺 `<<12`）→ avail/used ring 全错位 → virtio-blk 请求超时（41 第二轮 BLOCKED_EVIDENCE 根因，QEMU 源码 `virtio-mmio.c` `virtio_queue_set_addr(value << proxy->guest_page_shift)` 实证）。`QUEUE_ALIGN` 因 QEMU 复位默认 align=4096 未影响布局，但按规范一并修正。此缺陷也潜伏于 virtio-net（root 绑定）与两处 virtio-rng 驱动副本。
- **为什么无法通过现有接口完成**：寄存器偏移是 39 WP-39.5 抽取的共享常量，任何驱动侧 workaround 都会偏离规范；修正即恢复 C 参考语义（root_task.c 本就用 0x028/0x03C，G3 DRV-007 在 QEMU 通过）。
- **最小修改范围**：三处常量表各 2 个偏移值 + 注释；无任何控制流/语义变化（写入值不变）。
- **回归**：host 受影响 crate 446 测试全绿（virtio-core/storage/net/crypto/drv-svc）；QEMU 复验数据盘真读写 + svc-ota PERSIST/LOAD（见 41-验证记录 §7）。
- **回退**：改回 0x08C/0x0AC（回到请求超时状态）。


## CV-41-9：DMA 池两趟映射（先连续 retype 全部帧，再逐页 map）

- **目标文件**：`crates/runtime/root-task/src/spawn.rs`
- **为什么**：旧实现每 retype 一帧后立即 map；map 可能从同一 untyped 分配页表，与尚未 retype 的 DMA 帧交错，导致后续帧物理地址整体后移。QEMU 按 `base_pa+0x1000` 写 used ring 时落到被页表占用的页，服务侧读到另一物理页 → `used.idx` 永远 0（virtio-blk 超时真正根因，CV-41-8 的寄存器偏移修正后仍复现）。
- **关键修复**：DMA 池先连续 retype 9 帧，再统一 map；同时移除误导性的 used-ring marker 诊断（原实现写到独立页，并未验证池映射）。
- **回归**：host root-task 97/97；QEMU 数据盘真读 + EIPC 真写 + FLUSH + LOAD 全通过。
- **回退**：改回单循环 retype+map（回到超时）。

## CV-41-10：drv-svc 响应 MR 尾部 padding 兼容（BlockIo decode）

- **目标文件**：`crates/agents/business-services/src/block_io.rs`
- **为什么**：`RESPONSE_LEN=79` 经 4B/word MR 返回 80B；`wire::decode_response` 严格要求 79B，导致 EIPC 块读首包后误判 `bad-envelope` → svc-ota 报 `store-unavailable`。
- **关键修复**：`BlockIo::cmd` 接受 `resp_bytes.len() >= RESPONSE_LEN`，仅解前 79B。
- **回归**：QEMU EIPC 块读多包连续成功，svc-ota PERSIST/LOAD 通过。
- **回退**：改回精确长度（回到 store-unavailable）。

## CV-41-11：virtio-blk FLUSH 使用 2 描述符链（避免 zero-sized buffer）

- **目标文件**：`crates/drivers/storage/src/driver/virtio_blk.rs`
- **为什么**：QEMU legacy 拒绝 zero-sized data 描述符；旧实现 FLUSH 也发 3 描述符链（data len=0），QEMU 报 `virtio: zero sized buffers are not allowed` 并超时。
- **关键修复**：`issue()` 对 `VIRTIO_BLK_T_FLUSH` 使用 req→status 2 描述符链；host 仿真同步适配。
- **回归**：storage 141+1 测试全绿；QEMU FLUSH 成功，STORAGE-PERSIST OK。
- **回退**：改回 3 描述符（回到 FLUSH 超时）。

## CV-41-12：svc-ledger 真落盘 + 生产二进制移除 MemOutbox/MemStateStore

- **目标文件**：`crates/agents/business-services/src/bin/svc-ledger.rs`、`crates/agents/business-services/src/bin/svc-market.rs`、`crates/agents/business-services/src/bin/svc-cloud-sync.rs`、`crates/agents/business-services/src/bin/svc-agent-bus.rs`、`crates/runtime/durable-state/src/lib.rs`
- **为什么**：蓝图 41 禁止把内存假实现当生产实现；svc-ledger 原先只提供内存账本，market/cloud/agent-bus 生产二进制直接用 `MemOutbox`/`MemStateStore`。
- **关键修复**：
  1. svc-ledger 接入 `RegionStateStore<LedgerSnapshot>`，启动恢复 + POST 后 CAS 持久化；
  2. durable-state 新增 `UnavailableOutbox`/`UnavailableStateStore`，存储未授予时显式 Unavailable；
  3. market/cloud/agent-bus 生产路径改用 Unavailable 类型，不再使用 Mem*。
- **回归**：host 测试全绿；target `cargo check` 通过；QEMU 镜像构建通过。
- **回退**：恢复 Mem*（回到内存假实现违规）。

## CV-41-13：market/cloud/agent-bus/observability 真 BlockIo 接线

- **目标文件**：`crates/agents/business-services/src/target_durable.rs`（新）、`src/bin/svc-market.rs`、`src/bin/svc-cloud-sync.rs`、`src/bin/svc-agent-bus.rs`、`src/bin/svc-observability.rs`、`src/observability.rs`
- **为什么**：第四轮只做了显式 Unavailable；本轮把 storage 授予路径接成真 AppendOutbox/RegionStateStore，使市场/云同步/AgentBus/可观测在存储可用时具备真实 durable 语义。
- **关键修复**：
  1. 新增 `MaybeOutbox`/`MaybeState` 二态封装；
  2. svc-market/svc-cloud-sync 使用 `AppendOutbox` 真落盘；
  3. svc-agent-bus 使用 `RegionStateStore<SessionCursor>` 真落盘；
  4. svc-observability 为 `TelemetryItem` 增加 Encode/Decode 并挂接 durable outbox。
- **回归**：host 42+12 全绿；target check 通过；QEMU 镜像构建通过。
- **回退**：改回 Unavailable 或 Mem*（回到内存假实现/无持久化）。

## CV-41-14：EipcBlockDevice（BlockIo → BlockDevice 适配，OTA A/B 地基）

- **目标文件**：`crates/agents/business-services/src/block_io.rs`、`Cargo.toml`（+eneros-storage）
- **为什么**：`AbPartitionOta` 需要 `BlockDevice`，而 41 的 EIPC 块客户端目前只实现 `PersistIo`；新增适配层使 OTA A/B 可基于 svc-storage 真块读写。
- **交付**：`EipcBlockDevice<C>` 实现 `read_block/write_block/erase_block/flush/health_status`，内部用 `RefCell<BlockIo<C>>` 支持 `&self` 读。
- **验证**：host `cargo check/test` 通过（business-services 43/43，含 `eipc_block_device_read_write_roundtrip`）；尚未在 svc-ota 中接线（root_key 注入仍为占位，完整 A/B OTA 留待后续）。
- **回退**：删除适配层（不影响现有 durable 服务）。

## CV-41-15：svc-ota 真实 AbPartitionOta 接线

- **目标文件**：`crates/agents/business-services/src/bin/svc-ota.rs`
- **为什么**：此前 svc-ota 仅策略层 + trial 持久化，0x02-0x05 返回 storage-not-granted；蓝图要求系统 OTA 真实 A/B。
- **关键修复**：
  1. 存储授予时构造 `EipcBlockDevice` + `AbPartitionOta` + `OtaService`；
  2. 实现 0x02 DOWNLOAD+VERIFY / 0x03 TRIAL / 0x04 COMMIT / 0x05 REGRESSION；
  3. 使用开发公钥作为 OTA root key（生产 KMS 替换）。
- **回归**：target check 通过；host 52/52（含端到端 OTA、命令协议正/负例/无服务/回滚、observability durable 恢复、UnavailableOutbox/StateStore fail-closed）；QEMU `real AbPartitionOta attached` + PRODUCT-READY true。
- **回退**：删除 OtaService 接线（回到 storage-not-granted）。

## CV-41-16：svc-ota 命令处理抽成可测纯函数

- **目标文件**：`crates/agents/business-services/src/ota_service.rs`、`src/bin/svc-ota.rs`
- **为什么**：svc-ota 的 0x01-0x05 命令处理原先直接写在 target 二进制里，host 无法直接测试协议解析。
- **关键修复**：
  1. 新增 `handle_ota_ab_command<M: OtaManager>(Option<&mut OtaService<M>>, req, resp)`；
  2. svc-ota 二进制对 0x01-0x05 直接转发到该函数；
  3. 新增 `ota_ab_command_protocol_roundtrip` host 测试，覆盖 0x02 DOWNLOAD+VERIFY、0x03 TRIAL、0x04 COMMIT。
- **回归**：business-services 45/45；target check 通过；QEMU 镜像构建通过。
- **回退**：恢复二进制内联处理（不影响功能，但失去 host 协议测试）。

## CV-41-17：root-task QEMU 端到端 OTA 探针

- **目标文件**：`crates/runtime/root-task/src/service.rs`、`crates/runtime/root-task/Cargo.toml`
- **为什么**：此前 QEMU 只验证 OTA attach 与 trial 持久化，未用真实签名镜像走完整 download→verify→trial→commit。
- **关键修复**：
  1. root-task 新增 `ota_download_probe`，使用测试私钥种子 `0x41` 派生 SM2 密钥对；
  2. 构造 512B 镜像，SM3 哈希 + SM2 签名；
  3. 经 EIPC 调 svc-ota 0x02/0x03/0x04，完成真实 A/B 写盘、回读验签、试启动、提交。
- **回归**：QEMU 日志 `OTA-DOWNLOAD: OK`；Exit Gate #4 升级 PASS；全仓测试通过。
- **回退**：移除探针（不影响业务服务）。
