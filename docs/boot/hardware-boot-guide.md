# EnerOS v0.3.0 真机启动指南

> 版本：v0.3.0
> 适用范围：将 EnerOS 镜像烧录到真实 ARM64 开发板并启动 seL4
> 蓝图依据：`蓝图/phase0.md` §v0.3.0、`记忆.md` §九 开发环境检查

---

## 概述

本指南描述如何将 v0.1.0 产出的 EnerOS 镜像（含 seL4 14.0.0 内核）烧录到真实 ARM64 开发板，通过 U-Boot 引导启动，并从串口观察 seL4 boot log。

启动链路（蓝图 §4.3）：

```
上电 → ROM 加载 U-Boot → 读取 SD 卡 eneros.img → 加载 seL4 到 RAM
     → 跳转 seL4 入口 → 打印 banner → 串口可见 boot log
```

> **兜底策略**：若无可用真机硬件，可使用 QEMU virt 验证镜像本身可启动（见 §7 QEMU 验证）。

---

## 1. 前置条件

### 1.1 硬件

| 项目 | 规格 | 说明 |
|------|------|------|
| ARM64 开发板 | 飞腾 D2000 / 树莓派 4B / QEMU virt（备选） | 需支持 SD 卡或网络启动裸机镜像 |
| USB 转串口线 | CP2102 / CH340 / PL2303 | 用于观察串口输出 |
| SD 卡 | ≥ 512MB | 存放 seL4 镜像与设备树 |
| SD 卡读卡器 | — | 将镜像写入 SD 卡 |

### 1.2 软件

| 项目 | 版本 | 用途 |
|------|------|------|
| seL4 镜像 | `build/eneros-0.3.0.img` | 启动镜像（`make build` 产出） |
| U-Boot | ≥ 2021.04（aarch64） | 板载或 SD 卡引导加载器 |
| dtc | ≥ 1.6 | 设备树编译器（`.dts` → `.dtb`） |
| WSL2 Ubuntu | — | 交叉编译与烧录环境（账号 `ahx`） |

---

## 2. 硬件连接

### 2.1 串口线接线

USB 转串口线与开发板 UART 引脚交叉对接：

| 开发板 | USB 转串口 | 说明 |
|--------|-----------|------|
| TX | RX | 发送 ↔ 接收 |
| RX | TX | 接收 ↔ 发送 |
| GND | GND | 共地（必须连接） |

> **注意**：TX/RX 接反是无输出的最常见原因，详见 `docs/serial-debug-manual.md` §6。

### 2.2 SD 卡

将 SD 卡插入读卡器连接至主机（WSL2 中识别为 `/dev/sdX`，详见 §4 烧录）。

---

## 3. 构建镜像

在仓库根目录执行：

```bash
make build
```

该目标依次完成：设备树编译（`dtb`）→ seL4 内核构建（`sel4-build`）→ Rust 运行时构建（`runtime-build`）→ 镜像合并（`image`）。

产物路径：

| 产物 | 路径 | 说明 |
|------|------|------|
| 启动镜像 | `build/eneros-0.3.0.img` | 烧录到 SD 卡的目标文件 |
| 设备树 | `build/qemu-virt.dtb` | 传给 seL4 的设备树二进制 |
| seL4 内核 | `build/sel4/kernel/kernel.elf` | QEMU `-kernel` 直接加载 |

---

## 4. SD 卡烧录

使用 `tools/flash.sh` 烧录（WSL2 中执行）：

```bash
# 1. 确认 SD 卡设备名（切勿选错为系统盘）
lsblk

# 2. 烧录（X 替换为实际盘号，如 sdb）
tools/flash.sh /dev/sdX
```

### 安全注意事项

- 脚本拒绝烧录到 `/dev/sda`、`/dev/nvme0n0`、`/dev/mmcblk0`（系统盘保护）
- 烧录前需二次输入设备名确认
- 使用 `dd ... conv=fsync` + `sync` 确保数据落盘
- **所有数据将被擦除**，请提前备份 SD 卡内容

烧录完成后弹出 SD 卡，插入目标开发板。

---

## 5. U-Boot 配置

板级 U-Boot 启动脚本位于 `board/qemu-virt/boot.txt`，定义了从 SD 卡加载镜像与设备树并跳转入口的完整流程。

### 5.1 编译 boot.scr

```bash
mkimage -C none -A arm64 -T script -d board/qemu-virt/boot.txt \
        -n "EnerOS boot" board/qemu-virt/boot.scr
```

将生成的 `boot.scr` 放入 SD 卡第一个分区。

### 5.2 关键环境变量

| 变量 | 值 | 说明 |
|------|-----|------|
| `kernel_addr_r` | `0x40000000` | 内核镜像加载地址（RAM 基址） |
| `dtb_addr_r` | `0x44000000` | 设备树加载地址 |
| `bootargs` | `console=ttyAMA0,115200` | 内核命令行（PL011 串口） |
| `kernel_name` | `eneros.img` | SD 卡上镜像文件名 |
| `dtb_name` | `qemu-virt.dtb` | SD 卡上设备树文件名 |
| `bootdelay` | `2` | 启动延迟（秒），自动化设为 0 |
| `baudrate` | `115200` | 串口波特率 |

### 5.3 bootcmd 说明

`bootcmd` 依次执行：`mmc dev 0`（选 SD 卡）→ `mmc rescan`（重扫）→ `fatload`（加载镜像与 DTB）→ `bootm`（跳转内核入口）。

> 首次配置后可执行 `saveenv` 保存环境变量到板载 Flash。

---

## 6. 预期串口输出

### 6.1 串口参数

- 波特率：**115200**
- 数据位：8 / 停止位：1 / 校验：无 / 流控：无

### 6.2 boot log 示例

上电后串口应输出（含 seL4 banner 与 EnerOS 启动信息）：

```
U-Boot 2021.04 (...)
=== EnerOS v0.3.0 boot ===
Loading kernel: eneros.img -> 0x40000000
Loading dtb:    qemu-virt.dtb -> 0x44000000
bootargs: console=ttyAMA0,115200

seL4 14.0.0

EnerOS boot: v0.1.0 (seL4 integrated)
```

> 验收标准（蓝图 §7）：真机串口可见 seL4 boot log，冷启动 < 3s。

---

## 7. QEMU 验证（兜底）

无真机时，用 QEMU virt 验证镜像可启动性。

### 7.1 方式一：make run

```bash
make run
```

构建并启动 QEMU（machine=virt, cpu=cortex-a57, 128M, smp 2），串口重定向到 stdio。

### 7.2 方式二：flash.sh --qemu

```bash
tools/flash.sh --qemu
```

与 `make run` 参数一致，直接启动已构建的镜像。

### 7.3 预期输出

QEMU 控制台应输出 seL4 banner 与 EnerOS 启动信息：

```
seL4 14.0.0

EnerOS boot: v0.1.0 (seL4 integrated)
```

---

## 8. 故障排查

| 症状 | 可能原因 | 解决方案 |
|------|---------|---------|
| 串口无输出 | 波特率错误 | 确认 115200（详见 `docs/serial-debug-manual.md`） |
| 串口无输出 | TX/RX 接反 | 交换 TX 与 RX 接线 |
| 串口无输出 | 设备树 serial 节点错误 | 检查 DTS `uart` 节点 `reg` 与 `compatible` |
| 串口乱码 | 波特率不匹配 | 依次尝试 9600 / 38400 / 115200 |
| 内核 hang | 加载地址错误 | 确认 `kernel_addr_r` 与 link script 一致（0x40000000） |
| 内核 hang | DTB 未加载或地址错误 | 检查 `dtb_addr_r` 与 `bootm` 第三参数 |
| U-Boot 不启动 | SD 卡未正确烧录 | 重新烧录，确认 `boot.scr` 在第一分区 |
| 间歇性丢字 | 串口线材质量差 | 更换屏蔽线 |

> 更多串口调试细节见 `docs/serial-debug-manual.md`，设备树细节见 `docs/device-tree-spec.md`。
