# CROSS_VERSION_CHANGE_REQUEST（蓝图 39 登记）

> schema: `eneros.cross-version.v1` — 39 实施中发现必须触碰非 39 owner
> 文件/接口的请求登记。

## CVR-39-01：hal/board DMA 池容量 9 页 → 36 页（virtio 多设备按需分池）

- 当前版本：39（WP-39.5/39.7：virtio 设备按节点绑定、每设备一个 9 页队列池）
- 目标文件：`crates/hal/board/src/adapter.rs`（38 owner：PlatformAdapter/HalRuntime）
- 接口：`DmaPoolAllocator::<{ 9 * 4096 }>`（板级池容量常量）
- 为什么无法通过现有接口完成：virtio 工厂从"单节点扫描整区域、单池共享"改为
  "每 virtio_mmio 节点一个设备、每设备按需分配独立池"（QEMU DTB 每个槽位一个
  节点；旧实现只接受区域起点节点）。系统盘 blk + 数据盘 blk + virtio-net 共
  需 3 个 9 页池；9 页固定池在第二个设备分配时耗尽 → NotFound。
- 最小修改范围：`DmaPoolAllocator::<{ 9 * 4096 }>` → `DmaPoolAllocator::<{ 36 * 4096 }>`
  （一行常量；DmaAllocator 语义/接口未变，资源模型未改）
- 影响的后续版本：42（真机 DMA 预算按实际设备数核算）、43（板级资源清单）
- 回归测试：hal 54 / board 23 全绿；root-task 97 全绿；QEMU 39 复验
- 回退方式：恢复 9 页（仅单 virtio 设备盒子可用；多设备盒子 fail-closed）

## CVR-39-02：svc-storage/svc-drv-* 的 EL0 设备句柄移交（登记，39 不实施）

- 当前版本：39（WP-39.12：block 命令面 + StorageService 已定义并 host 验证）
- 目标接口：drv-svc `StorageService<B: BlockBackend>` 的 `attach()` 调用点
- 为什么无法通过现有接口完成：设备（virtio-blk/virtio-net）当前由 root-task
  DriverSupervisor 持有（38 语义）；EL0 服务的 MMIO/DMA/IRQ capability 移交
  属蓝图 40 的 EIPC 域（CVR-38-01 同源：`dc ivac` EL0 需 seL4 cache syscall +
  page cap）。
- 39 现状：命令面定义 + StorageService host 测试（read/write/flush/health/
  UNAVAILABLE/INVALID）；target 无设备 → capability None → EXECUTE 显式
  ST_UNAVAILABLE（不伪造 I/O）。
- 影响：40（EIPC 设备移交）、41（服务消费）

## 39 观察点（登记，39 不改动）

- **OBS-39-01**：QEMU DTB 的 virtio_mmio 节点为**每槽位一个节点**（reg
  0x200）；39 工厂按节点绑定、装配层 `durable_log_probe` 取最后一块 blk
  为数据盘——与 33 WP-33.5 "取最后一块"语义一致；系统盘（槽 0）驱动仍
  绑定（不写，仅存在）。
- **OBS-39-02**：`net_echo_poll` 经全局指针访问 net 驱动（root 单线程事件
  循环；与 register_wdt_base 同模式）；EL0 网络服务（svc-drv-net 设备侧）
  随 40 移交。
- **OBS-39-03**：svc-* 4 个协议服务（modbus-rtu/tcp/iec104/mqtt）入 bundle
  并以 optional 声明；target 无传输 capability → PRODUCT-READY degraded
  列表如实呈现（不伪装就绪）。

## CVR-39-03：QEMU netdev 从 virtio-mmio-bus.1 移到 bus.0 槽 3（G3 修正，最终方案）

- 当前版本：39（WP-39.7：virtio-net FDT 装配）
- 目标文件：`xtask/src/qemu_harness.rs`（36/42 owner 的 G3 netdev 段）
- 为什么无法通过现有接口完成：QEMU virt 只为 bus.0 生成 virtio_mmio FDT
  节点（32 槽）；`bus=virtio-mmio-bus.1` 的网卡在 DTB 不可见 → FDT 绑定
  永远探测不到（39 QEMU 实测：32 个节点全部 a000000..a001e00，无 a200000）。
  注释自述"QEMU 槽 3"与 bus.1 矛盾——bus.1 为书写错误。
- 最小修改范围：系统盘保留显式 `bus=virtio-mmio-bus.0`（bus.0 显式容量=1，
  QEMU 实测）；rng/数据盘/网卡**自动分配**（-device 参数序 → 槽 1/2/3，
  QEMU 实测 4 设备无冲突）；网卡不再指定 bus.1
- 影响的后续版本：42（QEMU harness 重建时保留 bus.0 语义）
- 回归测试：QEMU 39 复验（virtio-net bound + 外部 peer 回显）
- 回退方式：恢复 bus.1（网卡不可见，回显证据缺失——不推荐）

## CVR-39-04：DmaPoolAllocator VA→PA 偏移（qemu-virt rootserver 板级事实）

- 当前版本：39（WP-39.5：virtio-blk DMA 设备地址）
- 目标文件：`crates/hal/hal/src/dma.rs` + `crates/hal/board/src/adapter.rs`（38 owner）
- 为什么无法通过现有接口完成：`new_identity()` 假设池内存 VA==PA；qemu-virt
  rootserver 链接于低 VA（0x200000 起）而内核/rootserver 加载于 guest RAM
  （0x4000_0000 起）——VA→PA 偏移 = RAM 基址。无偏移时设备 DMA 地址
  （如 0x275000）落在客户机 RAM 之外，QEMU 静默丢弃 → virtio-blk 读超时
  （37 起 "durable-log format failed Io" 根因，38 §7 诚实登记转 39）。
- 最小修改范围：hal 新增 `new_identity_offset(offset)`（9 行）；board adapter
  用 `new_identity_offset(0x4000_0000)`（qemu-virt 板级常量）
- 影响的后续版本：42（真机 DMA 窗口按平台描述注入）、43（板级资源清单）
- 回归测试：hal 54 / board 23 全绿；QEMU 39 复验（durable-log open_existing
  成功路径）
- 回退方式：恢复 new_identity（QEMU DMA 失败——不推荐）

---

# 收尾复验轮新增登记（2026-08-15）

## CVR-39-07：BOOT-007 内核映射窗口 2MiB→4MiB（内核补丁 + boot-protocol）

- 当前版本：39 收尾复验
- 目标文件：`third_party/sel4-patches/0001-init-bundle-mapping.patch`（sel4
  内核 boot.c + arm64 vspace.c）、`crates/boot/boot-protocol/src/boot_info.rs`
- 为什么无法通过现有接口完成：EL0 服务增至 19 个后 init bundle ≈1.3MiB
  （opt-2 构建），+ 运行时 DTB ≈1.05MiB 超出 2MiB 窗口的 DTB 容纳上限
  （boot-manager `DT_LAYOUT` fail-closed 拒绝）。
- 最小修改范围：ENEROS_INIT_BUNDLE_SIZE 0x200000→0x400000；第二个 2MiB
  段增一个 PT cap（`create_it_pt_cap(..., bv + BIT(21), ...)`）；
  arch_get_n_paging 预留 n+=2→n+=3；boot-protocol
  `ENEROS_MAPPED_WINDOW_LEN` 同步；root-task 窗口判据测试同步 4MiB。
- 影响的后续版本：42/43（bundle 继续增长时窗口再评估；真机 DTB 尺寸）
- 回归测试：QEMU 39-qemu-final5 启动验证 + root-task/boot-protocol 测试全绿
- 回退方式：恢复 2MiB（bundle 需 < ~1.0MiB——服务数/优化级受限）

## CVR-39-08：root-task 栈 64KiB→256KiB + 堆 2MiB→3MiB + EL0 包级 profile 降级

- 当前版本：39 收尾复验
- 目标文件：`crates/runtime/root-task/src/main.rs`（global_asm 栈）、
  `crates/runtime/root-task/src/allocator.rs`（HEAP_SIZE）、根 `Cargo.toml`
  （包级 profile）
- 为什么：复验实测启动期非确定挂起根因 = 64KiB 栈被 run_ipc_loop 大帧
  溢出，静默腐蚀下方 .bss（GDB 实证 WATCHDOG 锁字节野写/堆 hole 乱序
  panic）；堆 2MiB 在 bundle 1.3MiB+DTB 下 OOM；Cargo.toml 注释中的
  EL0 LTO+opt3 降级意图从未实现。
- 最小修改范围：栈 `.skip 65536→262144`；HEAP_SIZE 默认 3MiB；
  root-task/services/drv-svc/protocol-services opt-2+codegen-units=16；
  全局 lto=false。
- 影响的后续版本：40-43（EL0 构建参数基线；栈 guard 页后续安排）
- 回归测试：8/8 连续启动无挂起 + 干净构建证据轮（peer matched/19 READY）
- 回退方式：逐项回退（栈回 64KiB 会复发挂起——不建议）
