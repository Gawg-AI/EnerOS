# EnerOS 蜂窝通信模块设计文档 (v0.30.1)

> **范围**：4G/5G 蜂窝调制解调器（modem）驱动——AT 命令封装、PPP 拨号状态机、
> HDLC 帧编解码、`CellularModem<S: HalSerial>` 泛型驱动，为无有线网络场景
> 提供蜂窝数据通道，并为 v0.30.2 双网冗余提供备份链路底层。
>
> **Crate**：`eneros-cellular` (`crates/drivers/cellular/`)
> **版本**：v0.30.1（Phase 1 刚性子版本 R2 — 蜂窝通信模块）
> **状态**：设计中 — 主机测试覆盖 AT 解析与 PPP 状态机；真实拨号与硬件集成延后。

---

## 1. 概述

`eneros-cellular` 是 EnerOS Edge Box 的蜂窝数据通道驱动。储能场景下终端部署位置
多样（偏远电站、移动储能车），有线以太网不可达时需以 4G/5G 蜂窝网络作为主或备
份数据通道，保障市场数据上报、远程运维与告警推送不中断。本版本交付以下能力：

| 能力 | 模块 | 说明 |
|------|------|------|
| AT 命令封装 | `at_command.rs` | `AtCommand` + `AtParser` + `AtResponse`，AT+CSQ 信号解析、AT+CCID SIM 检查 |
| PPP 拨号 | `ppp.rs` | 6 状态有限状态机 + HDLC 帧编解码（Flag/Protocol/Data/FCS） |
| Modem 驱动 | `modem.rs` | `CellularModem<S: HalSerial>` 泛型驱动，`CellularDriver` trait |

### 设计原则

- **HAL 解耦**：通过 `eneros-hal` 的 `HalSerial` trait 抽象串口访问，驱动核心逻辑与具体 UART/USB 后端解耦，便于在 mock 串口上完成主机测试。
- **no_std 合规**（蓝图 §43.1）：使用 `alloc::string::String`、`alloc::vec::Vec`，禁止 `std::io` / `std::net`。
- **smoltcp 适配**：定义 `PppDevice` 适配器接口，将 PPP 数据通道桥接为 smoltcp `phy::Device`，复用 v0.28.0 TCP/IP 协议栈。
- **PPP 最小实现**：仅实现状态机骨架与基础 HDLC 帧；完整 LCP/IPCP/PAP/CHAP 协商需在硬件 modem 上验证后补全。

### v0.30.1 交付物

| 组件 | 文件 | 说明 |
|------|------|------|
| AT 命令 | `at_command.rs` | AtCommand + AtParser + AtResponse + SignalQuality |
| PPP 协议 | `ppp.rs` | PppStateMachine + PppState + PppFrame + HDLC 编解码 |
| Modem 驱动 | `modem.rs` | CellularModem<S> + CellularDriver trait + CellularError |
| smoltcp 适配 | `ppp.rs` | PppDevice 接口定义（实现需硬件验证） |
| 模块入口 | `mod.rs` | 模块声明 + re-exports |

---

## 2. 架构设计

```text
┌──────────────────────────────────────────────────────┐
│  Caller (Agent Runtime / smoltcp interface)          │
└─────────────────┬────────────────────────────────────┘
                  │  CellularDriver trait
┌─────────────────▼────────────────────────────────────┐
│  eneros_cellular::CellularModem<S: HalSerial>        │
│  ┌────────────────────────────────────────────────┐  │
│  │  AtParser      — AT 命令构造与响应解析          │  │
│  │  PppStateMachine — 拨号状态迁移                 │  │
│  │  PppFrame      — HDLC 帧编解码 + FCS 校验       │  │
│  │  PppDevice     — smoltcp::phy::Device 适配接口  │  │
│  └──────────────────┬─────────────────────────────┘  │
└─────────────────────┼────────────────────────────────┘
                      │  HalSerial trait
┌─────────────────────▼────────────────────────────────┐
│  eneros_hal::HalSerial (real UART / USB serial)      │
│  MockSerial (testing)                                │
└──────────────────────────────────────────────────────┘
```

### 2.1 AT 命令层

`AtCommand` 表示一条待发送的 AT 命令（命令字符串 + 期望响应前缀）；
`AtParser` 负责从串口读取的字节流中切分响应行并匹配 `OK` / `ERROR` / `+CSQ:` / `+CCID:` 等前缀；`AtResponse` 枚举封装解析结果。

关键命令：

- **AT+CSQ** — 信号质量查询，解析 `+CSQ: <rssi>,<ber>`，转换为 `SignalQuality { rssi: u8, ber: u8 }`。
- **AT+CCID** — SIM 卡 ICCID 查询，验证 SIM 卡在位且已激活。
- **ATD\*99#** — 触发 PPP 数据模式（modem 进入 PPP 拨号会话）。

### 2.2 PPP 状态机

`PppStateMachine` 维护 6 状态有限自动机，由串口事件与定时器驱动状态迁移。

### 2.3 Modem 驱动

`CellularModem<S: HalSerial>` 是泛型驱动，泛型参数 `S` 解耦串口后端：

- 真实硬件使用 `eneros-hal` 提供的 `HalSerial` 实现（MMIO UART 或 USB CDC-ACM）。
- 单元测试使用 `MockSerial`（内存字节缓冲）验证 AT 解析与 PPP 状态迁移。

`CellularDriver` trait 暴露统一接口供上层（v0.30.2 双网冗余）调用。

---

## 3. 关键类型签名

```rust
// at_command.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtCommand {
    pub cmd: String,
    pub expected_prefix: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtResponse {
    Ok,
    Error,
    Csq(SignalQuality),
    Ccid(String),
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalQuality {
    pub rssi: u8,   // 0..31, 99 = unknown
    pub ber: u8,    // 0..7, 99 = unknown
}

pub struct AtParser {
    buf: String,
}

// ppp.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PppState {
    Closed,
    Establishing,
    Authenticating,
    Networking,
    Connected,
    Terminating,
}

pub struct PppStateMachine {
    state: PppState,
    started_at_ms: u64,
    last_event_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PppFrame {
    pub protocol: u16,
    pub data: Vec<u8>,
}

// modem.rs
pub struct CellularModem<S: HalSerial> {
    serial: S,
    at_parser: AtParser,
    ppp: PppStateMachine,
    signal: Option<SignalQuality>,
    iccid: Option<String>,
}

pub trait CellularDriver {
    fn dial(&mut self) -> Result<(), CellularError>;
    fn hangup(&mut self) -> Result<(), CellularError>;
    fn signal_quality(&self) -> Option<SignalQuality>;
    fn is_connected(&self) -> bool;
    fn current_state(&self) -> PppState;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellularError {
    NoSimCard,
    NoSignal,
    DialFailed,
    SerialError,
    PppTimeout,
    PppAuthFailed,
    NotConnected,
}
```

---

## 4. HalSerial 集成

蜂窝驱动通过 `eneros-hal` 提供的 `HalSerial` trait 抽象串口访问，复用既有 HAL 抽象
而非自研串口层（蓝图 §5.5 "禁止重复造轮子"）：

```rust
/// 来自 eneros-hal 的串口抽象（已存在，本 crate 仅引用）。
pub trait HalSerial {
    type Error;
    fn write(&mut self, data: &[u8]) -> Result<usize, Self::Error>;
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error>;
    fn flush(&mut self) -> Result<(), Self::Error>;
}
```

- **真实硬件**：`eneros-hal` 在飞腾 / 鲲鹏 / QEMU 上提供基于 MMIO 的 `UartSerial` 实现。
- **USB CDC-ACM**：后续版本可在 `eneros-hal` 中扩展 `UsbCdcAcmSerial`，无需改动 `CellularModem` 核心逻辑。
- **测试**：`MockSerial` 实现预置响应队列的内存串口，主机测试无需真实 modem。

---

## 5. PPP 状态机迁移图

```text
                          dial()
              ┌──────────────────────────────────────┐
              ▼                                       │
    ┌─────────────────┐  ATD*99# / LCP CONFREQ       │
    │     Closed      │────────────────────────────►│
    └─────────────────┘                              │
              ▲                                       │
              │ LCP TERMACK / hangup()                │
              │                                       │
              │              ┌────────────────────────┴───┐
              │              │                             │
              │              ▼                             │
    ┌─────────────────┐  LCP CONFACK  ┌─────────────────────┐
    │   Terminating   │◄──────────────│     Establishing    │
    └─────────────────┘               └──────────┬──────────┘
                                                 │
                                                 │ LCP OPEN / PAP/CHAP REQ
                                                 ▼
                                       ┌─────────────────────┐
                                       │   Authenticating    │
                                       └──────────┬──────────┘
                                                  │ AUTH OK
                                                  ▼
                                       ┌─────────────────────┐
                                       │     Networking      │  (IPCP 协商)
                                       └──────────┬──────────┘
                                                  │ IPCP CONFACK
                                                  ▼
                                       ┌─────────────────────┐
                                       │     Connected       │  ── 数据通道 ──► PppDevice
                                       └──────────┬──────────┘
                                                  │ LCP TERMREQ / hangup()
                                                  ▼
                                       (回到 Terminating)

    任意状态 ──force_switch / fatal error──► Terminating ──► Closed
```

### 状态迁移规则

| 当前状态 | 事件 | 下一状态 | 备注 |
|---------|------|---------|------|
| Closed | `dial()` / LCP CONFREQ | Establishing | 发送 ATD\*99# 触发 modem PPP 模式 |
| Establishing | LCP CONFACK | Authenticating | LCP 链路建立成功 |
| Establishing | LCP CONFREJ / 超时 | Terminating | 协商失败 |
| Authenticating | PAP/CHAP AUTH-OK | Networking | 鉴权通过 |
| Authenticating | AUTH-FAIL | Terminating | 鉴权失败，触发 `PppAuthFailed` |
| Networking | IPCP CONFACK | Connected | IP 地址协商完成 |
| Networking | 超时 | Terminating | IPCP 协商超时 |
| Connected | LCP TERMREQ / `hangup()` | Terminating | 主动或被动断开 |
| Terminating | LCP TERMACK | Closed | 清理资源，回到初始态 |
| 任意 | fatal error / `force_switch` | Terminating | 强制降级路径 |

---

## 6. HDLC 帧格式

PPP 数据在串口链路上以 HDLC-like 帧封装传输：

```text
┌────────┬──────────┬─────────────────┬────────┬────────┐
│ Flag   │ Protocol │ Information     │ FCS    │ Flag   │
│ 0x7E   │ 2 bytes  │ 0..N bytes      │ 2 bytes│ 0x7E   │
└────────┴──────────┴─────────────────┴────────┴────────┘
   1 B       2 B          可变长          2 B      1 B
```

### 字段说明

| 字段 | 长度 | 说明 |
|------|------|------|
| Flag | 1 B | 帧边界标志，固定 `0x7E` |
| Protocol | 2 B | PPP 协议号（如 `0xC021` = LCP，`0x80FD` = CBCP，`0x0021` = IPv4） |
| Information | 0..N B | 上层载荷（LCP/IPCP/PAP/CHAP 报文或 IP 数据报） |
| FCS | 2 B | CRC-16 校验（CCITT-16，多项式 `0x8408`，初始值 `0xFFFF`） |
| Flag | 1 B | 帧尾标志，固定 `0x7E` |

### 转义规则

由于 `0x7E` 用作帧边界，载荷中出现的 `0x7E` / `0x7D` 必须转义：

| 原始字节 | 转义后 |
|---------|--------|
| `0x7E`（Flag） | `0x7D 0x5E` |
| `0x7D`（Escape） | `0x7D 0x5D` |
| 其他 | 不变 |

- **发送**：编码时扫描 Information + FCS 字段，遇到 `0x7E` / `0x7D` 替换为转义序列。
- **接收**：解码时遇到 `0x7D`，将下一字节与 `0x20` 异或还原原始字节。

### FCS 校验

- 算法：CRC-16/CCITT（X.25），多项式 `0x8408`（反向），初始值 `0xFFFF`。
- 计算范围：Protocol + Information 字段（不含 Flag 与 FCS 自身）。
- 接收方重新计算并对比 FCS 字段，不匹配则丢弃整帧。

---

## 7. no_std 合规（蓝图 §43.1）

| 标准库用法 | 本模块替代 | 出现位置 |
|-----------|-----------|---------|
| `std::string::String` | `alloc::string::String` | AtCommand / AtResponse / AtParser |
| `std::vec::Vec` | `alloc::vec::Vec` | PppFrame.data |
| `std::io::Read/Write` | `HalSerial` trait | CellularModem 串口访问 |
| `std::net` | smoltcp + `PppDevice` 适配 | 不直接使用网络栈 |
| `std::time::Instant` | `HalClock::now_ms()` | PPP 状态机超时判定 |

模块顶层通过 crate `eneros-cellular` 的 `lib.rs` 继承 `#![no_std]`，所有 `use` 均限定在 `alloc::` / `core::` / `eneros_hal::` 命名空间内。

---

## 8. 内存预算声明（蓝图 §5.6）

| 组件 | 预估内存 | 说明 |
|------|---------|------|
| CellularModem\<S\> 主体 | ~2 KB | AT 缓冲（512 B）+ PPP 状态机 + 信号 / ICCID 缓存 |
| AtParser 缓冲 | ~512 B | 单行 AT 响应缓冲（`String` 容量上限 256） |
| PppFrame 临时帧 | ~1.5 KB | 单帧 Information 上限 1500 B（典型 MTU） |
| PppDevice 适配 | ~256 B | smoltcp RxToken / TxToken 缓冲指针 |
| **运行时总计** | **≤ 4 KB** | 不含 smoltcp 协议栈自身缓冲 |

> 蜂窝链路 TCP 窗口与 smoltcp socket 缓冲复用 v0.28.0 的全局网络预算，本模块不计入。

---

## 9. OOM 策略

当蜂窝通道因 AT 响应异常或 PPP 帧积压导致内存接近上限时，按以下优先级降级：

1. **截断 AT 缓冲**：`AtParser` 缓冲超过 256 字节时丢弃最早字节，避免 `String` 无限增长。
2. **重置 PPP 状态机**：调用 `PppStateMachine::reset()` 回到 `Closed`，丢弃未完成帧。
3. **降级到 L1 路径**：若蜂窝不可用且有线网络同时故障，Agent Runtime 切换到 Solver-only 路径（蓝图 L1 主路径），暂停远程通信与 LLM 增强。
4. **冻结非关键 Agent**：触发 OOM handler（蓝图 §43.6），冻结依赖蜂窝通道的 Agent。

---

## 10. 偏差声明

| 偏差项 | 蓝图原计划 | 实际实现 | 原因 |
|--------|-----------|---------|------|
| PPP 协议完整度 | 完整 LCP + IPCP + PAP + CHAP | 最小实现：状态机 + 基础 HDLC 帧 | 完整协议协商需在真实 modem（移远 EC20 / 华为 ME909s）上验证；当前仅交付状态机骨架与帧编解码，留作硬件验证后补全 |
| `PppDevice` smoltcp 适配 | 完整 `phy::Device` 实现 | 仅定义接口与帧缓冲 | 实际数据通道需硬件拨号成功后才能验证 RxToken / TxToken 行为，集成测试延后 |
| 集成测试 | 真实 modem 拨号测试 | 主机 mock 串口测试 | 真实 modem 拨号需硬件环境（SIM 卡 + 4G 信号），主机测试仅覆盖 AT 解析与 PPP 状态迁移逻辑 |
| PAP/CHAP 鉴权 | 双算法支持 | 接口预留，实现延后 | 鉴权算法需与运营商网络对接验证，主机侧无法独立测试 |
| `CellularDriver` trait | 同步接口 | 同步接口（无 async） | RTOS 控制大区为同步模型；async 留待 v0.30.x 后续子版本评估 |
