# EnerOS v0.3.0 设备树说明

> 版本：v0.3.0
> 适用范围：EnerOS QEMU virt 及真机板级设备树（DTS/DTB）
> 蓝图依据：`蓝图/phase0.md` §v0.3.0 §5.2、`configs/qemu-virt.dts`、`board/qemu-virt/dts`

---

## 概述

设备树（Device Tree）是 ARM64 平台描述硬件拓扑的标准方式。seL4 内核在启动时解析设备树二进制（DTB），据此初始化内存、串口、中断控制器等外设。EnerOS v0.3.0 以 QEMU virt 为首要目标，设备树源文件位于 `configs/qemu-virt.dts`，板级副本位于 `board/qemu-virt/dts`。

---

## 1. DTS 结构概述

### 1.1 源文件与编译产物

| 类型 | 扩展名 | 说明 |
|------|--------|------|
| 设备树源文件 | `.dts` | 人类可读的文本描述 |
| 设备树二进制 | `.dtb` | 编译后的二进制，供 bootloader/seL4 加载 |

### 1.2 编译命令

```bash
# 方式一：Makefile 目标（推荐）
make dtb
# 产出 build/qemu-virt.dtb

# 方式二：手动 dtc 编译
dtc -I dts -O dtb -o qemu-virt.dtb configs/qemu-virt.dts
# 板级副本
dtc -I dts -O dtb -o build/qemu-virt.dtb board/qemu-virt/dts
```

### 1.3 顶层结构

```dts
/dts-v1/;

/ {
    #address-cells = <2>;   // 地址为 2 个 cell（64 位）
    #size-cells = <2>;      // 长度为 2 个 cell（64 位）
    compatible = "linux,dummy-virt";
    interrupt-parent = <&gic>;

    cpus { ... }
    psci { ... }
    memory@40000000 { ... }
    uart@9000000 { ... }
    timer { ... }
    gic: interrupt-controller@8000000 { ... }
}
```

---

## 2. 节点说明

### 2.1 cpus —— CPU 节点

```dts
cpus {
    #address-cells = <1>;
    #size-cells = <0>;

    cpu@0 {
        device_type = "cpu";
        compatible = "arm,cortex-a57";
        reg = <0>;
        enable-method = "psci";
    };
    cpu@1 { /* 同上, reg = <1> */ };
};
```

- 2 × Cortex-A57，对应 QEMU `-smp 2`
- `enable-method = "psci"`：通过 PSCI 接口上电次核（为 v0.15.0 多核启动铺路）

### 2.2 memory@40000000 —— 内存节点

```dts
memory@40000000 {
    device_type = "memory";
    reg = <0x0 0x40000000 0x0 0x08000000>; // 128MB at 0x40000000
};
```

- 基址 `0x40000000`，大小 `0x08000000`（128MB）
- 与 QEMU `-m 128M`、Makefile `QEMU_MEM := 128M` 一致

### 2.3 uart@9000000 —— PL011 串口

```dts
uart@9000000 {
    compatible = "arm,pl011", "arm,primecell";
    reg = <0x0 0x09000000 0x0 0x1000>;
    interrupts = <0 1 4>;
    clock-names = "uartclk", "apb_pclk";
    clocks = <&clk24mhz>, <&clk24mhz>;
    status = "okay";
};
```

- 基址 `0x09000000`，映射大小 `0x1000`
- 中断 `<0 1 4>` = GIC_SPI + 中断号 1 + LEVEL_HIGH
- `board/src/mini_uart.rs` 的 `Pl011Serial` 驱动此节点

### 2.4 timer —— ARMv8 架构定时器

```dts
timer {
    compatible = "arm,armv8-timer", "arm,armv7-timer";
    interrupts = <1 13 11>, <1 14 11>, <1 11 11>, <1 10 11>;
    always-on;
};
```

- 4 个 PPI 中断（secure/non-secure physical + virtual）
- 为 v0.12.0 RTC 与定时子系统预留

### 2.5 gic —— GICv3 中断控制器

```dts
gic: interrupt-controller@8000000 {
    compatible = "arm,gic-v3";
    #interrupt-cells = <3>;
    interrupt-controller;
    reg = <0x0 0x08000000 0x0 0x0100000>,  // GICD
          <0x0 0x080a0000 0x0 0xf60000>;   // GICR
    interrupts = <1 9 7>;
};
```

- GICv3：GICD（Distributor）+ GICR（Redistributor）
- `#interrupt-cells = <3>`：每个中断用 3 个 cell 描述

### 2.6 psci —— 电源管理

```dts
psci {
    compatible = "arm,psci-0.2";
    method = "hvc";   // 通过 hypervisor call 调用
};
```

- PSCI 0.2，`hvc` 方法（非 `smc`）

---

## 3. 中断编码说明

GIC 中断描述符为 3 个 cell：`<type number flags>`。

| 字段 | 含义 | 取值 |
|------|------|------|
| type | 中断类型 | `GIC_SPI = 0`（共享外设中断）/ `GIC_PPI = 1`（私有外设中断） |
| number | 中断号 | SPI 从 0 开始（硬件 IRQ = SPI + 32）；PPI 0~15 |
| flags | 触发类型 + CPU 掩码 | 见下表 |

### 触发类型标志

| 标志 | 值 | 含义 |
|------|-----|------|
| IRQ_TYPE_LEVEL_HIGH | 4 | 高电平触发 |
| IRQ_TYPE_LEVEL_LOW | 8 | 低电平触发 |
| IRQ_TYPE_EDGE_RISING | 2 | 上升沿触发 |
| IRQ_TYPE_EDGE_FALLING | 1 | 下降沿触发 |

### CPU 掩码（PPI 专用）

| 宏 | 值 | 含义 |
|----|-----|------|
| GIC_CPU_MASK_SIMPLE(2) | 0x3 | 2 核掩码（bit0=cpu0, bit1=cpu1） |

> PPI 的 flags = `GIC_CPU_MASK_SIMPLE(2) | IRQ_TYPE_LEVEL_LOW` = `0x3 | 0x8` = `11`。
> UART 中断 `<0 1 4>` = SPI 类型、中断号 1、高电平触发。

---

## 4. 与 board crate 的关系

`board` crate（`eneros-board`）的 `BootInfo` 结构体字段与设备树节点一一对应：

| BootInfo 字段 | 类型 | 对应 DTS 节点 | QEMU virt 值 |
|---------------|------|--------------|--------------|
| `board_name` | `&'static str` | —（板名标识） | `"qemu-virt"` |
| `ram_base` | `u64` | `memory` 节点 `reg` 首地址 | `0x40000000` |
| `ram_size` | `u64` | `memory` 节点 `reg` 长度 | `0x08000000`（128MB） |
| `serial_base` | `u64` | `uart` 节点 `reg` 首地址 | `0x09000000` |
| `cpu_count` | `u32` | `cpus` 节点下 `cpu` 子节点数 | `2` |
| `freq_mhz` | `u32` | CPU 频率（由 `clk24mhz` 推导） | `1500` |

`BoardConfig` trait 为多板适配的抽象层，每种板型实现 `boot_info()` 返回自身参数：

```rust
pub trait BoardConfig {
    fn boot_info() -> BootInfo;
}
```

---

## 5. 板级配置文件位置

| 文件 | 位置 | 用途 |
|------|------|------|
| 主 DTS 文件 | `configs/qemu-virt.dts` | QEMU virt 设备树源文件（构建入口） |
| 板级 DTS 副本 | `board/qemu-virt/dts` | 板级交付物，内容与主 DTS 一致，便于多板管理 |
| U-Boot 启动脚本 | `board/qemu-virt/boot.txt` | 定义 `bootcmd` 加载镜像与 DTB |
| 编译产物 | `build/qemu-virt.dtb` | `make dtb` 产出，传给 seL4 |

> `configs/qemu-virt.dts` 与 `board/qemu-virt/dts` 内容保持一致（蓝图 §v0.3.0 §3 交付物），前者为构建系统引用，后者为板级集中管理。

---

## 6. 适配新板指南

以适配飞腾 D2000 为例，新增板级支持需以下步骤：

### 6.1 创建板级目录

```bash
mkdir -p board/phytium-d2000
```

### 6.2 创建板级 DTS

创建 `board/phytium-d2000/dts`，参照厂商参考 DTS 修改以下节点：

- `cpus`：CPU 核数与型号（飞腾 D2000 为 8 核）
- `memory`：物理内存基址与大小
- `uart`：PL011 基址（飞腾 D2000 通常为 `0x28001000`）
- `gic`：GIC 版本与寄存器布局
- `psci`：上电方法（`smc` 或 `hvc`）

### 6.3 创建 U-Boot 启动脚本

创建 `board/phytium-d2000/boot.txt`，修改加载地址与 `bootargs`：

```bash
setenv kernel_addr_r <飞腾 D2000 RAM 基址>
setenv dtb_addr_r    <DTB 加载地址>
setenv bootargs      console=ttyAMA0,115200
```

### 6.4 实现 BoardConfig trait

在 `board/src/` 中为新板实现 `BoardConfig`：

```rust
struct PhytiumD2000;
impl BoardConfig for PhytiumD2000 {
    fn boot_info() -> BootInfo {
        BootInfo {
            board_name: "phytium-d2000",
            ram_base: 0x8000_0000,
            ram_size: 0x4000_0000,   // 1GB
            serial_base: 0x2800_1000,
            cpu_count: 8,
            freq_mhz: 2000,
        }
    }
}
```

### 6.5 更新 BootInfo 字段

若新板有额外硬件参数需传递，在 `board/src/boot_info.rs` 的 `BootInfo` 结构体中新增字段并补充单元测试。

---

## 7. 参考

- seL4 手册：设备树解析（KernelBoot）— seL4 14.0.0 文档
- ARM Device Tree 规范：Devicetree Specification Release v0.4
- 项目内文件：
  - `configs/qemu-virt.dts`：主 DTS 源文件
  - `board/qemu-virt/dts`：板级 DTS 副本
  - `board/src/boot_info.rs`：`BootInfo` / `BoardConfig` 定义
  - `docs/hardware-boot-guide.md`：真机启动指南
  - `docs/serial-debug-manual.md`：串口调试手册
