# CROSS_VERSION_CHANGE_REQUEST（蓝图 38 登记）

> schema: `eneros.cross-version.v1` — 38 实施中发现必须触碰非 38 owner 文件/接口
> 的请求登记；未获 39/42 owner 确认前保持生效，38 不改动目标文件。

## CVR-38-01：Aarch64Cache 固定 64B cache line → CTR_EL0

- 当前版本：38（WP-38.6：cache line/maintenance 必须来自平台/架构，禁止固定值）
- 目标文件：`crates/drivers/storage/src/driver/virtio_blk.rs`（39 owner：`drivers/storage/net/drv-svc`）
- 接口：`Aarch64Cache::clean/invalidate` 内 `addr.wrapping_add(64)` 硬编码 64B
- 为什么无法通过现有接口完成：cache line 是驱动实现内部细节，root 侧无法注入；
  38 无权改 storage 实现（CROSS_VERSION 矩阵）
- 最小修改范围（39）：clean/invalidate 内 `CTR_EL0.DminLine/Detect` 读取
  （`mrs ctr_el0` → (DminLine+1)*4 字节）替代常量 64
- 影响的后续版本：39（virtio 服务化时），42（真机 cache 资格）
- 回归测试：aarch64 单元测试注入 CTR_EL0 返回值；QEMU durable-log 路径不回归
- 回退方式：保留常量 64B（QEMU 无 cache 模型，行为等价）
- 38 现状：QEMU virt 为一致内存平台（适配器声明 Coherent），cache 维护
  为无害 no-op；非一致盒子在 39 修 cache line 后方可启用

## CVR-38-02：seL4 BootInfo `numNodes` CPU 交叉验证通道（登记，不修改）

- 当前版本：38（WP-38.4：CPU 数来自 inventory/seL4 BootInfo 交叉验证）
- 目标接口：elfloader → root-task BootInfo 传参通道（37 登记 C3，
  `resolve_bundle_phys` 同通道）
- 38 现状：bootinfo_cpu_count 传 0（未知跳过），机制
  （`cross_check_cpu_count`）host 测试覆盖不一致拒绝；FDT CPU 节点为事实源
- 影响：42/43 真机/SMP 验证时接通 numNodes；不阻塞 38 L0-L2

## CVR-38-03：root-task main.rs/service.rs 接线（37 文件最小触碰）

- 当前版本：38（root 装配必须进入 rust_entry 组合）
- 目标文件：`root-task/src/main.rs`（37 构建）、`service.rs`（37 IPC 循环）
- 修改内容：`platform::assemble` 调用点 + `time_init` 移后（timer 经
  HalRuntime）+ `run_ipc_loop` 增 `driver_supervisor` 参数（badge 路由）
- 为什么无法通过现有接口完成：38 Exit Gate 1 要求 root 正式调用
  PlatformAdapter/HalRuntime——组合根必须在 rust_entry；spawn/bundle
  语义未动（37 ABI 保持）
- 最小修改范围：main.rs 增 ~80 行装配 + 3 处调用点；service.rs 增参数 +
  2 处打印/路由
- 影响的后续版本：37 复验（bundle/spawn/NX 路径未动，36/37 Gate 已复跑 PASS）
- 回归测试：root-task host 97/97 + aarch64 target check + QEMU round8 序列
- 回退方式：删除装配块 + 恢复 time_init 原位 + 去掉 supervisor 参数

## CVR-38-04：FDT `interrupt-map` 多级路由未实现（38-review 登记）

- 当前版本：38-review（WP-38.2 面：`interrupt-parent` 单级解析——本节点显式
  → 否则根级缺省；`#interrupt-cells` 缺省 3；SPI=32+n / PPI=16+n）
- 目标文件：`crates/hal/board/src/device_tree.rs`（39 owner 后续平台扩展）
- 为什么无法通过现有接口完成：`interrupt-map` 需 per-bus interrupt domain
  路由表（父 bus interrupt-parent 链回溯 + map 条目匹配），38 合成平台 A/B
  （QEMU virt / nested soc）均无该属性；错误面 `BadInterruptMap` 已预留，
  未知路由 fail-closed（不猜）
- 最小修改范围（39+）：`parse_interrupts` 增 interrupt-map 属性解析 +
  沿父链回溯 interrupt-parent；合成平台加带 interrupt-map 的 bus
- 影响的后续版本：39（更多外设/总线盒子）、42/43（真机资格）
- 回归测试：新增合成平台（interrupt-map bus）→ 全局号断言；现有 A/B 不回归
- 回退方式：保持单级解析（QEMU/合成 A/B 语义不变）
- 38-review 现状：Exit Gate 2 由单级 interrupt-parent + 其余面（nested bus/
  cells/ranges/clocks/resets/gpios/dmas/iommus/status）覆盖；interrupt-map
  登记本 CVR 后诚实标注

## 38-review 观察点（登记，38 不改动）

- **OBS-01**：`framework/src/binding.rs:39` `map_mmio(phys: u64) -> u64` 装配
  seam 用裸 u64 传地址（31 时代接口；38 不变量 3 的强类型在 hal 层
  `DmaAllocator/PhysAddr` 收口）。改动会牵动 framework/root-task 全部
  factory 签名 + 测试——39 驱动服务化时随 DeviceResources 迁移。
- **OBS-02**：`root-task/main.rs` timer IRQ 兜底值 30（DTB 缺失时；带打印
  判据行，非静默）。QEMU 实测 DTB 恒存在不触发；真机 DTB 缺失时 fail-loud。
- **OBS-03**：hal `IrqHandlerTable`（frozen-cell）无生产消费者——root-task
  生产 IRQ 表是 37 的 `root-task/irq.rs`（38 不重写 37 产物）。static mut
  面已收口（类型面），接线统一在 39 驱动服务化时完成。
- **OBS-04**：`driver_probe.rs` `VIRTIO_REGION_BASE=0x0a00_0000` 等 virtio
  区域常量保持（RECON-38 §1.3：39 接 net/blk 服务时迁至 DeviceResources）。
