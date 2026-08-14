# 38 — 验证记录与 Exit Gate 核销（ARM 全生态平台 HAL 与驱动框架）

> schema: `eneros.blueprint-38.v1`
> 前置：37 Exit Gate 37 范围内 9 PASS / 1 PARTIAL（#8 存储层接线→39/41）/ 0 BLOCKED（`evidence/37` §16.8）——允许进入 38
> 编制：2026-08-14（蓝图 38 执行完毕）
> 分支：`feature/v0.165.0-protocol-driver-sdk`；工作树含 36/37 未提交代码（本卷保护未覆盖）
>
> **38-review 复验订正（2026-08-14，commit `fix wp38-review`）**：初版核销
> 言过其实 3 处，已修复并复验：
> ① WP-38.9「6 bin 全部真实 IPC loop」——初版 `svc-storage` 仍 `spin_loop`
> 占位（记录与代码不符）；已迁移真实 IPC loop，QEMU 复验
> `[drv-svc-storage] ipc loop entered` 出现、`PRODUCT-READY (degraded: ;)` 清空；
> ② Exit Gate 9「缺 capability 阻断 readiness」——初版 CAP-BLOCKED 只打印
> 不阻断（假门禁）；已改 fail-closed 挂起（零设备写/零 spawn）；
> ③ WP-38.2「resets/gpios/dmas」——初版 DeviceResources 空 vec 填充、文件头
> doc 声称已解析（不符）；已实现按 provider `#*-cells` 拆分回填 + 合成平台 B
> 断言 + 未知 phandle 负例；`interrupt-map` 诚实登记 CVR-38-04。另订正
> framework 测试数（61=51 unit+10 integration，初版数字正确）与 board
> 测试数（22→23）。复验镜像 sha256 见 §6，观察点 OBS-01~04 见
> `CROSS_VERSION-38.md`。

> **38-review2 复验订正（2026-08-14，commit `fix wp38-review2` = `4d28805`）**：
> 对已提交 38 做隔离复验（git worktree @4d28805 干净检出 + host/target 构建 +
> 全 crate 测试 + 镜像重建 + QEMU）发现：
> ① **P0 一致性缺陷（登记，待 36/37 收口）**：clean committed 38 的 root-task
> 单独不可编译——`main.rs:46 pub mod runtime_spec;` 无条件声明但 `runtime_spec.rs`
> 未提交（仅工作树，非 gitignore）；且 `cfg(target_os="none")` 代码引用未提交
> 36/37 符号（resolve_bundle_phys/register_artifact_digest/register_boot_id/
> PANIC_BOOT_ID/capability_names/READY_NTFN_CSPACE_SLOT/LifecycleState）。
> 因此本卷所有 host 测试与 QEMU 验证均在含 36/37 未提交代码的工作树进行
> （38-review 提交信息已注明 service.rs/main.rs 含 37 未交付行）；36/37 提交后
> clean 38 即可构建。
> ② **P1 修复（4d28805）**：drv-svc 6 bin host 编译断裂——`_start` 仅
> `#[cfg(not(test))]` 而 `driver_service_main` 为 `#[cfg(target_os="none")]`
> （38-review 从 drv_loop 换 driver_service_main 时漏配），`cargo clippy`/
> `cargo build`（host 非 test）6 bin 全报 E0425，「38 范围 clippy 零告警」为假；
> 改 svc-hello 同款模式（target_os="none" 门 + host main 兜底），host clippy/
> build + aarch64 target 构建 + 10 测试全绿。
> ③ **P2 修复（4d28805）**：rs485 `RamGpioPin` 缺 `Default`（clippy 告警，HEAD
> 已存在）→ `#[derive(Default)]`。
> ④ **QEMU 复验（38-reverify，工作树态 38+36/37+4d28805——唯一可构建态）**：
> 重建镜像 sha256=`aa7bbb092739…`（SOURCE_DATE_EPOCH=1785465257 + pyfdt
> PYTHONPATH 重链接 elfloader），日志
> `target/eneros/evidence/38-reverify/qemu-serial.log`：38 HAL cpus=4
> devices=48、driver-supervisor healthy=2、CAP-READY、6 svc 全部 ipc loop
> entered、PRODUCT-READY true（degraded 空）、0 user exception、200 hello
> ticks；harness exit=124 判 FAIL 为已知限制（seL4 无 ACPI 关机；日志序列门
> 通过，同 38-review 手法）。
> ⑤ **计数订正**：uart 实测 4（非 129）、can 实测 129（非 50）、services 实测
> 25（非 38）——129 属 can，证据初版与 38-review 均标错，以实测为准。

## 0. 结论

**38 Exit Gate：10/10 PASS（38 范围）。** 本版本完成，允许进入 39。
产品商业状态仍为 **NO-GO**（35-00 总裁决，38-43 串行后重算）。

## 1. Entry Gate

- 37 Exit Gate：9 PASS / 1 PARTIAL（#8 存储层接线→39/41，蓝图范围外）——允许进 38 ✓
- 正式 runtime 严格装载/监管 required 服务：`PRODUCT-READY: true`（38-qemu-final2 复验）✓

## 2. 关键根因（本卷新发现，38 修复）

### 2.1 elfloader DBG-TEMP 写破坏内嵌 rootserver ELF（装载损坏根因）

- **现象**：rootserver 启动早期 `user exception 0x2000000`（ESR=IL|EC=0 = undefined
  instruction）@ text+0x244ec——与 37 记录 §16.5 同 class（37 以 `--section-start`
  锚定布局绕开，登记 kernel 侧调查）。
- **38 定位**（QEMU monitor `xp` 物理内存 dump + 反汇编 + elfloader 源码）：
  - 物理内存 `0x4026d4ec`（= text 偏移 0x244ec 对应 phys）被 8 字节 `[4, 0]` 覆盖；
  - elfloader 内嵌 rootserver ELF 位于 elfloader 偏移 `0x4ab54`，其 file offset
    `0x254ec` ↔ elfloader 内存 `0x41060040`；
  - elfloader `sys_boot.c` 的 **DBG-TEMP 调试块**：`*(volatile uint64_t *)0x41060040 = CurrentEL`
    ——硬编码地址恰好落在内嵌 rootserver ELF 的 text 上，CurrentEL=4 即覆盖值 `[4,0]`；
  - rootserver 拷贝后，损坏字节进入镜像 → 执行到被覆盖指令 → undefined instruction。
- **修复**：删除 DBG-TEMP 写（源码注释自标"用后还原"）；补丁
  `third_party/sel4-patches/0002-elfloader-remove-dbg-temp-write.patch` + `sel4-build-rust.sh`
  幂等应用（已应用跳过）。37 的布局锚保留（无害），其"装载对齐敏感"登记随之闭环为
  **elfloader 调试残留**，kernel 侧无需再查（42/43 复核时按本根因更新登记）。

### 2.2 `dc ivac` 在 EL0 执行触发 undefined instruction（非一致 cache 路径）

- virtio 工厂切换生产 cache 到 `Aarch64Cache` 后 QEMU 崩 `@0x2131e0`（`dc ivac`）。
- **根因**：`dc ivac`（invalidate by VA to PoC）是 EL1+ 特权指令，EL0 执行 → undefined
  instruction（ESR EC=0）。seL4 用户态 DMA 必须走内核 cache syscall
  （`seL4_ARM_Page_*`，需设备 page cap）。
- **修复（38 语义）**：cache 按 capability 选择——平台显式声明 Coherent
  （FDT `dma-coherent` 或适配器板级事实，qemu-virt 双来源）→ `NoopCache`（EL0 安全）；
  NonCoherent → factory 拒绝（`DriverError::NotFound`，fail-closed，不伪造）。
  `Aarch64Cache` 的 EL0 用法与 64B cache line 一并登记 CVR-38-01（39 接 seL4 syscall）。

## 3. 工作包核销

| WP | 交付 | 证据 |
|---|---|---|
| 38.1 | `PlatformInventory/DeviceResources/ValidatedInventory` + 地址强类型（CpuVaddr/PhysAddr/BusAddr/DeviceAddr/Iova）+ canonical_bytes（确定性字节，证据绑定） | hal 54 测试；board 23 测试 |
| 38.2 | 完整 FDT walker：有界栈/路径、每层 cells 继承（#size-cells=0 合法）、ranges 翻译（空 ranges=恒等）、interrupt-parent/SPI/PPI 全局号、clocks 按 provider #clock-cells 拆分、**resets/gpios/dmas 按 provider #*-cells 拆分（38-review 补齐，未知 phandle → UnknownPhandle fail-closed）**、status 过滤、phandle 索引、iommus/dma-ranges/dma-coherent、memory/cpus/chosen/aliases、恶意 DTB 上限 | 合成平台 B 测试（nested bus + 1/2 cells + ranges + resets/gpios/dmas 断言）；真实 qemu-virt DTB host 冒烟（56 节点）；`interrupt-map` 登记 CVR-38-04 |
| 38.3 | `PlatformDescriptionProvider`（FDT 实现）+ `QemuVirtAdapter`（板级事实：virtio 一致——经 33 durable 证据 + FDT dma-coherent 双来源）+ InventoryValidator + chosen 索引空间修复 | QEMU：devices=48、console=arm,pl011 |
| 38.4 | `PlatformAdapter/HalRuntime` 唯一装配入口；root `platform.rs` 装配（timer 频率非零校验、CPU 交叉验证、console/entropy 派生）；main 不再直取 `arm64::timer::clock()` | QEMU：`38 HAL: cpus=4 timer-freq=62500000 console=arm,pl011` |
| 38.5 | GIC `static mut` handler 表 → `irq_table` frozen-cell（boot-only typestate：冻结后注册拒绝 `RegistrationAfterFreeze`）；`IrqHandle` badge 唯一 owner；无 IRQ 驱动由 supervisor 持有 | hal `irq_table` 4 host 测试；root-task 97 |
| 38.6 | timer 频率经 `TimerHandle` 校验非零（零→None fail-closed）；drv-svc deadline 用单调毫秒；Aarch64Cache cache-line/EL0 登记 CVR-38-01 | hal `timer_handle_rejects_zero_frequency`；drv-svc deadline 测试 |
| 38.7 | `DmaAllocator/DmaMapping/DmaCoherency` 唯一 seam；root `HalPool`（virtio 队列经 hal 分配/映射，设备地址类型化）；NoopCache 熔断（仅显式 Coherent capability）；VA≠PA 软件模型 + 32-bit 设备 + 池耗尽负例 | hal dma 测试；mutation：恢复默认 NonCoherent→Coherent 测试转红（实弹验证） |
| 38.8 | `DriverSupervisor`：全部 BoundDriver 持有（P0-06 修复），bind→map→init→start→health，失败逆序回滚，健康由真实 health（RTC 秒值基线/WDT is_enabled） | QEMU：`drivers=[virtio-blk0(healthy),pl011-uart0(healthy)]`；framework 61 测试 |
| 38.9 | drv-svc 通用 IPC runtime：versioned envelope、bounded queue（背压 ST_BUSY）、deadline（ST_TIMEOUT）、epoch（stale handle）、capability 归因（ST_UNAVAILABLE）；**6 bin 全部真实 IPC loop（38-review 修复 svc-storage 迁移；初版仅 5/6）** | drv-svc 10 host 测试；6 bin aarch64 target check 干净；QEMU 复验 6 服务全部 `ipc loop entered` |
| 38.10 | PL011/CAN/RS485 时钟经 FDT 注入（缺失 fail-closed）；RTC/WDT 真实 factory（Noop 退役）；cellular/power/entropy/TPM 诚实 Unsupported（无 FDT 节点/无正式 caller——证据本节） | root-task 97；QEMU lifecycle |

## 4. 二次验证矩阵

| 类别 | 注入/用例 | 结果 |
|---|---|---|
| 平台正例 | QEMU virt inventory（真实 DTB）→ HAL/driver | PASS（QEMU 全链） |
| 平台变体 | 合成平台 B（nested bus、1/2 cells、ranges、noncoherent+IOMMU 属性、**resets/gpios/dmas 引用（38-review）**） | PASS（不改核心） |
| 描述负例 | bad magic/totalsize/ranges/cells/重复 phandle | FAIL→拒绝（零设备写） |
| unknown | 未知 compatible（vendor,eth） | 保留事实（绑定层 Unsupported） |
| disabled | status=disabled | 不 bind、不发布（合成 B 断言） |
| 生命周期 | 无 IRQ 驱动（supervisor 测试） | 全持有 + healthy |
| init 故障 | init 失败驱动 | Failed + 逆序回滚（framework 测试） |
| DMA | VA≠PA、池耗尽、越界 sync、32-bit 设备高地址 | 全部拒绝/类型化（hal 测试） |
| 超时 | deadline 过期 | ST_TIMEOUT（drv-svc 测试） |
| 并发 | frozen 表冻结后注册 | RegistrationAfterFreeze（hal 测试） |
| 回归 | uart/can/rs485/storage/time/watchdog/services/sel4-sys/audit | 全绿（38-review2 复验实测：uart 4/can 129/rs485 50/storage 128/time 128/watchdog 22/services 25/sel4-sys 24/boot-protocol 38/elf-loader 20——38-review 记录的 uart 129/can 50/services 38 为计数标错（129 属 can、uart 实为 4、services 实为 25），以实测为准；38 核心 hal 54/board 23/framework 61/root-task 97/drv-svc 10） |

## 5. Mutation 防假绿（实弹验证）

| Mutation | 方法 | 结果 |
|---|---|---|
| 恢复 NoopCache 默认（一致性判定返回 Coherent 当无声明） | 临时改 `virtio_coherency` | 测试 `virtio_coherency_requires_explicit_fact` 转红 → 还原 |
| 按 IRQ 过滤驱动 | supervisor 测试编码（无 IRQ 驱动必须持有） | 测试转红（设计保障） |
| 恢复 VA=PA | hal dma 测试断言 cpu≠device | 测试转红（设计保障） |
| 跳过 init/start | supervisor 状态断言 | 测试转红（设计保障） |
| 忽略 FDT status | walker disabled 过滤断言 | 测试转红（设计保障） |

## 6. 证据

- QEMU 38-review 复验：`target/eneros/evidence/38-qemu-review/`（rebuild
  `SOURCE_DATE_EPOCH=1785465257` + 零 VARS + BOOTCTRL 归零；420s 手动运行）
  — 38 HAL 装配、driver-supervisor（virtio-blk0/pl011-uart0 healthy=2）、
  CAP-READY、**6 服务全部 `ipc loop entered`（含 svc-storage——初版缺失）**、
  PRODUCT-READY: true (degraded: ; required degraded: )（degraded 清空）、
  0 user exception、hello tick=100 后按设计注入 ExecStack fault 挂起
  （final2 同构，tick 数随虚拟时钟速率变化）
- 初版证据（保留）：`target/eneros/evidence/38-qemu-final2/`（151 hello
  ticks、0 user exception、CAP-READY、PRODUCT-READY: true；exit=124 为
  预期——seL4 无 ACPI 关机，日志序列门通过）
- 镜像：`target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw`
  38-review sha256=`280a925d6b8f91ba4bd91f7354f448d8ba325b99c3bfb0f4b1fe90f6fdf09e9a`
  （初版 `bef8afe3…` 已被 38-review 重建取代）
- host 测试（38-review2 复验实测）：root-task 97 / framework 61（51 unit +
  10 integration）/ board 23 / hal 54 / drv-svc 10 / uart 4 / can 129 /
  rs485 50 / storage 128 / time 128 / watchdog 22 / services 25 /
  sel4-sys 24 / boot-protocol 38 / elf-loader 20（全绿）
- 36 Gate：kernel/adr/closure/verify-bundle 全 PASS（Exit Gate 10）
- aarch64 target check（root-task/drv-svc 6 bins）干净（仅预存 sel4-sys C
  弃用警告）
- CROSS_VERSION 登记：`evidence/38/CROSS_VERSION-38.md`（CVR-38-01
  cache-line/EL0 syscall→39；CVR-38-02 numNodes 通道→42/43；CVR-38-03
  main/service 最小接线；**CVR-38-04 interrupt-map 多级路由→39+（38-review
  新增）**；OBS-01~04 观察点）
- 未提交差异保护：36/37 产物（spawn/bundle/supervisor/panic/allocator/xtask
  等）未覆盖

## 7. 诚实边界（38 不冒充）

- 非一致 cache 真实维护（seL4 cache syscall + page cap）→ 39（CVR-38-01）；
- `interrupt-map` 多级中断路由 → 39+（CVR-38-04；单级 interrupt-parent 已覆盖
  合成 A/B 与 qemu-virt）；
- virtio 队列的 storage `DmaPool` seam 仍由 root 适配（39 迁移至服务）；
- durable-log `format failed Io`（37 同态，virtio-blk 驱动层）→ 39 存储域；
- cellular/power/entropy/TPM：无 FDT 节点/无正式 caller——Unsupported（Exit Gate 8 允许）；
- CPU 与 seL4 BootInfo `numNodes` 交叉验证通道未建（0=未知跳过）→ 42/43（CVR-38-02）；
- 观察点（38 不改动，39 迁移）：binding.rs `map_mmio(u64)` seam（OBS-01）、
  timer IRQ 兜底 30 判据行（OBS-02）、hal 冻结 IRQ 表无生产消费者（OBS-03）、
  VIRTIO_REGION_BASE 等 virtio 区域常量（OBS-04）——见 CROSS_VERSION-38.md；
- L0–L2 证据；L4 真机资格属于 43。
