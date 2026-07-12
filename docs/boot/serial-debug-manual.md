# EnerOS v0.3.0 串口调试手册

> 版本：v0.3.0
> 适用范围：EnerOS 真机启动串口调试（PL011 UART）
> 蓝图依据：`蓝图/phase0.md` §v0.3.0 §4.4 错误处理、`board/src/mini_uart.rs`

---

## 概述

串口是 EnerOS 启动阶段唯一的输出通道。本手册说明串口参数、接线方式、主机端工具使用及常见故障排查，配套 `docs/hardware-boot-guide.md` 使用。

PL011 串口驱动实现位于 `board/src/mini_uart.rs`，通过内存映射寄存器直接访问，不依赖外部 crate。

---

## 1. 串口参数

EnerOS 启动串口固定使用以下参数（与 `board/qemu-virt/boot.txt` 中 `bootargs=console=ttyAMA0,115200` 一致）：

| 参数 | 值 |
|------|-----|
| 波特率 | 115200 |
| 数据位 | 8 |
| 停止位 | 1 |
| 校验位 | 无（None） |
| 流控 | 无（None） |

---

## 2. USB 转串口接线

USB 转串口线与开发板 UART 引脚**交叉对接**：

| 开发板引脚 | USB 转串口 | 说明 |
|-----------|-----------|------|
| TX | RX | 开发板发送 → 主机接收 |
| RX | TX | 主机发送 → 开发板接收 |
| GND | GND | 共地（**必须连接**，否则信号不稳） |

> **常见芯片**：CP2102 / CH340 / PL2303。Linux 下设备名为 `/dev/ttyUSB0`（CP2102/CH340）或 `/dev/ttyACM0`（部分 CDC 设备）。

> **坑点**：部分开发板的 UART 引脚电平为 3.3V，禁止直接连接 RS232 电平（±12V），需经过电平转换。

---

## 3. Linux 工具

### 3.1 minicom

```bash
# 打开串口（115200, 8N1）
minicom -D /dev/ttyUSB0 -b 115200

# 退出：Ctrl+A → Q
# 保存日志：Ctrl+A → Z → L
```

首次使用需关闭硬件流控：`Ctrl+A → Z → O → Serial port setup → F (Hardware Flow Control) → No`。

### 3.2 screen

```bash
# 打开串口
screen /dev/ttyUSB0 115200

# 退出：Ctrl+A → K → y
# 保存日志：Ctrl+A → H（切换日志记录）
```

### 3.3 picocom

```bash
# 打开串口（推荐，轻量且默认无流控）
picocom -b 115200 /dev/ttyUSB0

# 退出：Ctrl+A → Ctrl+X
# 保存日志：Ctrl+A → Ctrl+R（开始/停止记录）
```

> **权限**：访问 `/dev/ttyUSB0` 需 `dialout` 组权限，否则报 `Permission denied`：
> ```bash
> sudo usermod -aG dialout $USER
> # 重新登录后生效
> ```

---

## 4. Windows 工具

### 4.1 PuTTY

1. Connection type 选择 **Serial**
2. Serial line 填入 COM 端口（设备管理器查看，如 `COM3`）
3. Speed 填入 `115200`
4. Connection → Serial：Data bits=8, Stop bits=1, Parity=None, Flow control=None
5. Session → Logging 可开启日志保存

### 4.2 Tera Term

1. New connection 选择 **Serial**
2. Serial port 选择对应 COM 端口
3. Setup → Serial port：Speed=115200, Data=8 bit, Parity=none, Stop bits=1, Flow control=none
4. File → Log 可开启日志保存

---

## 5. 常见故障排查表

| 症状 | 可能原因 | 解决方案 |
|------|---------|---------|
| 无输出 | 波特率错误 | 确认为 115200 |
| 无输出 | TX/RX 接反 | 交换 TX 与 RX 接线 |
| 无输出 | GND 未连接 | 连接开发板与 USB 转串口的 GND |
| 无输出 | 设备树 serial 节点错误 | 检查 DTS `uart` 节点 `reg` 基址与 `compatible` |
| 无输出 | 权限不足（Linux） | 将用户加入 `dialout` 组 |
| 乱码 | 波特率不匹配 | 依次尝试 9600 / 38400 / 115200 |
| 乱码 | 电平不匹配 | 确认 3.3V TTL，非 RS232 |
| 间歇性丢字 | 线材质量差 | 更换带屏蔽的串口线 |
| 间歇性丢字 | 接线松动 | 重新插拔，确保接触良好 |
| 仅回显无内核输出 | 串口回路自环 | 检查 TX/RX 是否短接 |

---

## 6. 串口日志保存

启动调试时建议保存完整串口日志，便于回溯分析。

| 工具 | 保存日志操作 | 说明 |
|------|------------|------|
| minicom | `Ctrl+A → Z → L` | 选择文件路径后开始记录 |
| screen | `Ctrl+A → H` | 切换日志记录（开/关） |
| picocom | `Ctrl+A → Ctrl+R` | 开始/停止记录到 `picocom.log` |
| PuTTY | Session → Logging | 配置日志文件路径与覆盖策略 |
| Tera Term | File → Log | 选择文件路径开始记录 |

> **命令行重定向**（Linux 通用）：
> ```bash
> # 直接将串口输出重定向到文件
> cat /dev/ttyUSB0 > boot.log
> # 或用 script 命令录制会话
> script -c "picocom -b 115200 /dev/ttyUSB0" boot.log
> ```

---

## 7. PL011 寄存器速查

EnerOS PL011 驱动（`board/src/mini_uart.rs`）使用以下寄存器，基址由 `BootInfo.serial_base` 指定（QEMU virt 为 `0x09000000`）：

| 寄存器 | 偏移 | 名称 | 用途 |
|--------|------|------|------|
| UART_DR | `0x00` | 数据寄存器 | 写入发送字节 / 读出接收字节 |
| UART_FR | `0x18` | 标志寄存器 | 反映 FIFO 与线路状态 |

### 标志寄存器（UART_FR）位定义

| 位 | 名称 | 值 | 含义 |
|----|------|-----|------|
| bit 5 | FR_TXFF | `0x20` | 发送 FIFO 满（1=满，需等待） |
| bit 4 | FR_RXFE | `0x10` | 接收 FIFO 空（1=空） |
| bit 3 | FR_BUSY | `0x08` | UART 忙（正在发送） |

### 驱动关键逻辑

```rust
// 发送单字节：轮询 FR_TXFF，等待 FIFO 不满后写 DR
fn putc(&self, c: u8) {
    unsafe {
        while (self.read(UART_FR) & FR_TXFF as u32) != 0 {}  // 等待
        self.write(UART_DR, c as u32);                        // 写入
    }
}
```

> **调试技巧**：若串口输出卡死，多为 `FR_TXFF` 永远为 1——检查 `serial_base` 地址是否正确、UART 时钟是否使能。
