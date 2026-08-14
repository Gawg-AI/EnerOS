# RECON-38 — 蓝图 38 侦察报告（ARM 全生态平台 HAL 与驱动框架）

> schema: `eneros.recon-38.v1`
> 状态：**执行完成**（2026-08-14；Exit Gate 核销见 `38-验证记录与Exit-Gate核销.md`）
> 新增根因发现（侦察后）：① elfloader DBG-TEMP 硬编码写 `0x41060040` 破坏内嵌
> rootserver ELF（38 定位并修复，补丁 `third_party/sel4-patches/0002-*`）；
> ② `dc ivac` EL0 特权指令——非一致 cache 需 seL4 syscall（39，CVR-38-01）。
> 前置：37 Exit Gate 37 范围内 9 PASS / 1 PARTIAL（#8 存储层接线 → 39/41）/ 0 BLOCKED（`evidence/37/37-验证记录与Exit-Gate核销.md` §16.8）——允许进入 38
> 审查基线：`HEAD=473b1e26681be33acc654f32e88251d8c19ad3cf` + 全部未提交/未跟踪开发代码
> 编制：2026-08-14（蓝图 38 执行，Entry 侦察）
> 38 唯一目标：把 AArch64 平台差异收进可验证的 Adapter 与资源图，建立驱动完整生命周期和所有权

## 1. 生产路径侦察（hal / board / framework / root-task / drv-svc）

### 1.1 hal crate（`crates/hal/hal`，eneros-hal）

| 位置 | 事实 | 38 处置 |
|---|---|---|
| `lib.rs:148-161` `HalProvider` | 4 方法（cpu/irq/clock/serial）；`init_hal`(:174)/`hal()`(:182) **零生产 caller**（35-00 P1-10 死 seam） | WP-38.4：`PlatformAdapter`/`HalRuntime` 取代死 seam，root 接线 |
| `types.rs` | MemFlags/IrqTrigger/HalError(7 变体)/GpioDir/IrqHandler | 保留；IrqTrigger 复用于 InterruptSpec |
| `arm64/platform.rs:13-47` | `PlatformConfig` + `QEMU_VIRT`（core=4/uart=0x0900_0000/gicd=0x0800_0000/gicr=0x080A_0000/timer_irq=27/net_base=0x0A00_0000）+ `PHYTIUM_D2000`（8 核） | 板级常量；WP-38.1 起 inventory 为唯一事实，核心不再直取 |
| `arm64/cpu.rs:12` | `CORE_COUNT = QEMU_VIRT.core_count`（**4 硬编码**） | WP-38.4：core_count 来自 inventory/seL4 BootInfo 交叉验证 |
| `arm64/gicv3.rs:63` | `static mut IRQ_HANDLERS: [Option<IrqHandler>; 256]`（allow static_mut_refs；单核不变量注释） | WP-38.5：boot-only typestate + 唯一 owner，消除运行期 static mut 面 |
| `arm64/timer.rs:11-71` | `Arm64Timer` 实现 HalClock（CNTPCT_EL0/CNTFRQ_EL0/CNTP_TVAL） | WP-38.6：timer frequency 校验非零；经 HalRuntime 注入 |
| `arm64/uart_pl011.rs` | HAL 串口（与 uart 驱动 crate 两套独立 PL011） | 保留；drv-svc console 属 38.9 模板 |
| `arm64/gpio.rs` / `net_mmio.rs` | HalGpio 实现 / net MMIO 无引用 | 死代码面，不重写 |

### 1.2 board crate（`crates/hal/board`，eneros-board）

| 位置 | 事实 | 38 处置 |
|---|---|---|
| `device_tree.rs:48-59` `DeviceNode` | 只提取 name/compatible(首值)/regs/interrupts；**status/phandle/clocks/ranges/#cells 全部丢弃**（:159-174 `_ => {}`） | WP-38.2：完整 walker |
| `device_tree.rs:187-195` `parse_reg` | 固定 `#address-cells=2/#size-cells=2`（16B/对） | WP-38.2：继承每层 cells，支持 1/2 组合 |
| `device_tree.rs:119-140` | 扁平遍历无父子层级，无路径前缀 | WP-38.2：有界栈 + 路径 + 嵌套 bus ranges 翻译 |
| fixture `tests/fixtures/qemu-virt.dtb` | 真实 dtb；pl011 `regs==[(0x0900_0000,0x1000)]`、`interrupts==[0,1,4]`、timer 4 组 PPI | 回归基准：解析升级后必须保持 |
| **新发现** | pl011 `clocks=<&0x02 0x02>`（uartclk phandle 2 + cell 2）；phandle 2 = fixed-clock `clock-frequency=<0x16e3600>`（24 MHz）——**时钟可从 DTB 注入** | WP-38.10：PL011/SJA1000 时钟经 clocks/phandle/clock-frequency 解析，禁止常量猜时钟 |

### 1.3 root-task driver_probe.rs（38 owner 拆分目标）

| 硬编码常量 | 值 | 38 处置 |
|---|---|---|
| `VIRTIO_REGION_BASE/SIZE/SLOT_STRIDE/SLOT_COUNT` | 0x0a00_0000 / 0x20_0000 / 0x200 / 32 | virtio 区域探测保持（39 接 net/blk 服务时迁至 DeviceResources）；38 不引入新常量 |
| `UART_BAUD` | 115_200 | 协议层配置（39），保留默认但经节点可覆盖 |
| `PL011_CLOCK_HZ` | 24_000_000 | **WP-38.10 从 dts clocks 读取**（qemu-virt 可解析）；无 clock → Err(NotFound) fail-closed |
| `SJA1000_CLOCK_HZ` | 16_000_000 | 同上：`clock-frequency` 属性优先 |
| `FRAME_BITS_4K/2M/1G` | 12/21/30 | 页映射帧位宽（37 spawn 语义），保留 |
| DriverId | 0xB10C_0001/0x0A11_0001/0xCA11_0001/0x4850_0001 | 保留 |
| SPI→GIC 换算 | `32 + cell[1]` | 移入 FDT walker interrupt 解析（全局号），工厂不再重复换算 |
| `dma_pool` static mut 1:1 VA==PA | 9×4096 B | WP-38.7：DmaAllocator seam；root 侧 DMA 走映射类型化（VA!=PA 模型 host 验证） |

**probe 流程事实**（:599-703）：`runtime_dtb_slice` → `parse_fdt` → `binding_table().probe_all` → 打印报告 → **按 IRQ badge 过滤取出驱动**（:660-702），无 IRQ 的 bound 驱动随 `BoundEntry` drop（P0-06/HD-04）；WDT/RTC factory 返回 `NoopDriver`（P1：Noop 冒充）；`Main.rs:136` 直取 `arm64::timer::clock()`、`:352-362` timer IRQ 回退 30（QEMU 常量）。

### 1.4 framework（`crates/drivers/framework`）

- `DeviceDriver` trait（lib.rs:93-114，ADR-0028 seam）：id/name/driver_type/state + init/start/stop/deinit/handle_irq/health_check——**保留不重写**（35-01 §3.2；can/rs485/uart/BlkAdapter 已实现）。
- `BindingTable::probe_all`（binding.rs:137-169）：compatible 匹配 + factory；`BoundEntry` 含 mmio_base/irq_badge。
- `DriverState` 6 态（Uninitialized/Ready/Running/Stopped/Error/Dead）——被所有 impl 使用；supervisor 生命周期状态**新增** 11 态枚举（蓝图 WP-38.8 模型），不破坏旧枚举。
- `DriverRegistry/DriverHandle/DriverPermission`（registry.rs/handle.rs）：**零生产 caller**（死代码面）——38 不接 registry，接入由 supervisor 取代（能力经 CapabilityRegistry，见 §7 蓝图）。

### 1.5 concrete drivers

| 驱动 | 现状 | 38 处置 |
|---|---|---|
| UART/PL011（`drivers/uart`） | 已实现 DeviceDriver；`new(id,name,io,clock_hz,baud)`；IRQ 服务 | clock 注入改造在 driver_probe factory（不重写驱动） |
| RS485（`drivers/rs485`） | 已实现；Pl011UartHw 持 clock_hz | 同上 |
| CAN/SJA1000（`drivers/can`） | 已实现；`Sja1000Controller::new(mmio, clock_hz, now)` | clock 从 dts 读 |
| RTC/PL031（`drivers/time`） | `Pl031Rtc{base}` 秒读写；root factory 返回 **NoopDriver** | WP-38.10：新 `RtcAdapter`（真实 health：秒值>epoch 基线） |
| WDT/SP805（`drivers/watchdog`） | `HwWatchdog`；root factory 返回 **NoopDriver** | WP-38.10：新 `WdtAdapter`（health 查 is_enabled；panic ownership 保留 register_wdt_base） |
| cellular/power/entropy/TPM | 无正式装配资源路径（cellular 需 HalSerial+UART；power 无 MMIO；TPM 无 FDT 节点/正式 caller） | **诚实标 Unsupported**（Exit Gate 8 允许） |
| virtio-blk/net | BlkAdapter（inner VirtioBlkDriver）+ NoopCache 生产默认 | WP-38.7：NoopCache 仅允许显式 coherent 平台；virtio 服务接线属 39 |

### 1.6 drv-svc（`crates/drivers/drv-svc`）

- `lib.rs:29-33` `drv_loop()` = `loop { spin_loop() }`；6 个 bin 全部 `_start() -> ! { drv_loop() }`（P0-08）。
- EL0 服务 IPC 模式已有先例：`svc_entry(ep, name, ready_ntfn)` → Signal readiness → `svc_ipc_loop`（ipc_recv → decode → handle_request → reply）（`crates/runtime/services/src/lib.rs:282-360`，37 建立）。
- **drv-svc 无 alloc**；38.9 通用 runtime 用定容队列（bounded queue 无需堆）。

### 1.7 并发/static mut 边界

- `gicv3.rs:63` `static mut IRQ_HANDLERS`（单核启动阶段；WP-38.5 收口）
- `driver_probe.rs:105` `dma_pool` static mut（单线程独占不变量注释；WP-38.7 类型化收口）
- `cpu.rs:12` `CORE_COUNT` 常量（WP-38.4 改注入）
- 无 SMP 实际装配（QEMU 单核运行）；GIC handler 表 typestate 化后 SMP 注册边界由类型保证

## 2. 未提交差异保护（本卷禁止覆盖）

- `root-task/Cargo.toml`、`build.rs`（37 text 64K 边界锚）、`spawn.rs`、`bundle.rs`、`service.rs`、`supervisor.rs`、`panic.rs`、`allocator.rs`、`boot.rs`、`irq.rs`、`irq_timer.rs`、`runtime_spec.rs`——37 产物，38 只经 driver_probe/main 最小接线，不重写。
- `drivers/net/**`、`storage/**`（39 owner）：38 只消费现有接口，不改 net/storage 实现。
- `.cargo/config.toml`、`xtask/**`（36/42 owner）：不改。
- `product/profiles/*.json`：38 只允许"平台/driver service 声明数据"新增；schema 语义不动。

## 3. 38 固定 seam（35-01 §6.2，禁止同义接口）

- `PlatformInventory`（WP-38.1）
- `DeviceResources`（WP-38.1）
- `PlatformDescriptionProvider`（FDT 为一实现；WP-38.2/38.3）
- `PlatformAdapter`/`HalRuntime`（WP-38.4）
- `DmaAllocator`/`DmaMapping`（WP-38.7）
- `DriverSupervisor`/`DriverServiceHandle`（WP-38.8/38.9）
- `CapabilityRegistry`（蓝图 38 §7）

## 4. 环境事实

- rustup target：`aarch64-unknown-none`/`aarch64-unknown-uefi` 已装；无系统 qemu/gcc——工具在 `~/.cache`（xpack-qemu-arm-9.2.4-1、xpack-aarch64-none-elf-gcc-15.2.1-1.1、cmake、dtc、sel4-src、sel4-build/top-rust 已构建）。
- `SEL4_SRC=~/.cache/sel4-src`、`SEL4_BUILD_DIR=third_party/sel4-build/top-rust`（root-task 与内核 label 一致性要求，sel4-build-rust.sh 语义）。
- 内存 22G/13G 可用：QEMU 与整仓测试前先静态审计（memory-budget-first 铁律）。
