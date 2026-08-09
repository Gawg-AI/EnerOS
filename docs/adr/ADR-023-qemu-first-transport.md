# ADR-023 — QEMU-first transport：真机对接前软件侧闭环（21-B B1）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 21-B §5 预设裁决 3 落地）
- **关联**：ADR-021（DDS→MQTT）、ADR-022（GmSSL binding）、`蓝图/21-B` §2.B1

## 背景

B1 要求 Modbus-TCP / IEC104 / Modbus-RTU 三个 transport trait 具备生产实现。
真设备（飞腾主板 / 真主站 / Modbus/IEC104 设备）未采购——21-B §5 决策 3
预设："硬件未定、软件先行……B1 不等硬件"。

## 决策

1. **transport 生产实现的对端验证一律先走 QEMU 闭环**：
   - TCP 类（Modbus-TCP / IEC104）：QEMU 内部 loopback 或主机侧 test 服务器
     作为对端，真设备到位仅换对端 IP。
   - RTU 类（Modbus-RTU）：`Pl011UartHw`（真 PL011 MMIO 路径）+ `RamMmio`
     对通，真 RS485 收发器到位仅换物理层载体。
2. **证据形态**：每个 transport 一个集成测试（QEMU loopback / RamMmio 对通，
   完成一次完整请求-响应），D2 互操作标记从 `PENDING-REAL-PEERS` 升级为
   `PENDING-REAL-DEVICE`（对端 = QEMU loopback，非"无"）。
3. **不等待硬件**：硬件采购到位后，B1 的剩余工作仅为"换对端"（真实 IP /
   物理 UART），无软件架构变更。

## 替代方案（否决）

- **等硬件到位再实现**：阻塞 v1.0.0 软件侧清偿，且 21-B §0 明确"代码已
  就绪、只差真机对接的条目，软件侧先写到位"。
- **host 单元测试冒充对通**：不满足"证明旧行为无法完成、新行为可以"的
  测试纪律（21-B §6.4），且无 QEMU 证据。

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| QEMU loopback 与真网卡行为差异（延迟/时序） | transport 语义按协议规范实现；真机阶段以 72h 浸泡（14 号文）覆盖 |
| PL011 MMIO 与真 UART 芯片差异 | 驱动层按寄存器级实现（Pl011UartHw 走真实 MMIO 路径），真机仅验证电气层 |

## 验收判据映射（21-B §2.B1）

- [x] 三个 transport 集成测试：QEMU loopback（TCP）/ RamMmio+Pl011UartHw（RTU）对通，
      完成一次完整请求-响应（B1 提交：4fdafff / f85f2ef / 2e4902d）
- [x] release 默认 transport 为生产实现，mock 仅 `#[cfg(test)]`
- [x] D2 标记升级为 `PENDING-REAL-DEVICE`（对端 = QEMU loopback）
      （提交 bf9c045）
