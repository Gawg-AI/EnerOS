# EnerOS 以太网网卡驱动设计文档 (v0.27.0)

> **范围**：以太网 MAC 控制器驱动、DMA 描述符环形缓冲、PHY 自协商与原始
> 以太网帧收发，为 v0.28.0 TCP/IP 协议栈提供 `NetDevice` trait 抽象。
>
> **Crate**：`eneros-net` (`crates/drivers/net/`)
> **版本**：v0.27.0（Phase 1 Layer 6 基础服务）
> **状态**：已实现 — 主机测试通过（130 个单元测试），aarch64 交叉编译验证通过。

---

## 1. 概述

`eneros-net` 是 EnerOS Edge Box 的以太网底层驱动。储能场景下 Edge Box 需通过
以太网接收市场数据、与云端通信、接入 IEC 104 / Modbus TCP 设备。本 crate 实现
原始以太网帧的收发能力，向上为 v0.28.0 TCP/IP 协议栈（smoltcp）提供统一的
`NetDevice` trait 接口，向下通过 `MacRegs` trait 抽象寄存器访问，支持真实硬件
（MMIO）与 mock 测试两种后端。

### v0.27.0 交付物

| 组件 | 状态 | 说明 |
|------|------|------|
| `error.rs` | 完成 | NetError（9 变体）+ NetStats 统计结构体 |
| `eth_frame.rs` | 完成 | EthFrame + EtherType + 编解码 + is_broadcast |
| `dma_ring.rs` | 完成 | DmaRing + DmaDescriptor + DESC_OWN/IOC/LS/FS 标志位 |
| `phy.rs` | 完成 | PhyDriver trait + GenericPhy + PhyState/Speed/Duplex + MII 寄存器常量 |
| `mac.rs` | 完成 | MacRegs trait + MacController + NetDevice impl + MmioMacRegs（aarch64） |
| `mock.rs` | 完成 | MockMacRegs（BTreeMap 后端，模拟 MII 协议） |

---

## 2. 架构设计

```text
┌──────────────────────────────────────────────┐
│  Caller (v0.28.0 TCP/IP stack — smoltcp)     │
└─────────────┬────────────────────────────────┘
              │  NetDevice trait (send/recv/mac_address/mtu/link_up)
┌─────────────▼────────────────────────────────┐
│  eneros-net::MacController (this crate)      │
│  ┌────────────────────────────────────────┐  │
│  │  DMA Ring (TX + RX descriptor rings)   │  │
│  │  PHY Driver (GenericPhy via MII)       │  │
│  │  Frame Buffers (Vec<Vec<u8>>)          │  │
│  │  NetStats (tx/rx counters)             │  │
│  └────────────────────────────────────────┘  │
└─────────────┬────────────────────────────────┘
              │  MacRegs trait (read/write register offsets)
┌─────────────▼────────────────────────────────┐
│  MmioMacRegs (real hardware, aarch64 only)   │
│  MockMacRegs (testing, BTreeMap-backed)      │
└──────────────────────────────────────────────┘
```

`MacController<R: MacRegs>` 是核心结构，泛型参数 `R` 解耦寄存器访问：
- 真实硬件使用 `MmioMacRegs`（`core::ptr::read_volatile` / `write_volatile`）
- 单元测试使用 `MockMacRegs`（`BTreeMap<u64, u32>` 模拟寄存器空间）

`GenericPhy` 不持有寄存器，所有方法接受 `&mut R: MacRegs`，通过 MII 管理协议
（写 `MAC_MII_ADDR` 启动事务、读写 `MAC_MII_DATA`）访问 PHY 寄存器。这允许
`MacController` 通过 `self.phy.autoneg(&mut self.regs)` 将自己的寄存器借用给 PHY
驱动（Rust 的 disjoint field borrow 规则允许同时以不同可变性借用不同命名字段）。

---

## 3. 数据结构

### 3.1 NetError

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum NetError {
    NotInitialized,
    LinkDown,
    NoBuffer,
    DmaError(u32),
    FrameTooLarge { size: usize, max: usize },
    FrameTooSmall,
    CrcError,
    PhyError,
    Timeout,
}
```

9 个变体覆盖所有网络驱动错误场景。`FrameTooLarge` 携带实际尺寸与最大尺寸便于
诊断；`DmaError` 携带 DMA 状态寄存器原始值供调用方解析硬件错误位。

### 3.2 EthFrame

```rust
pub struct EthFrame {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub ethertype: EtherType,
    pub payload: Vec<u8>,
}

pub enum EtherType {
    Ipv4,       // 0x0800
    Ipv6,       // 0x86DD
    Arp,        // 0x0806
    Other(u16),
}
```

`encode()` 输出格式：`dst(6) + src(6) + ethertype(2, 大端) + payload`。
`decode()` 接受 ≥14 字节输入，<14 字节返回 `FrameTooSmall`。
`is_broadcast()` 判断 `dst_mac` 是否全 0xFF。

**设计决策**：不含 VLAN 标签（0x8100）与 FCS/CRC 字段。硬件 MAC 自动剥离 FCS，
VLAN 解析推迟到 TCP/IP 协议栈层处理（蓝图未要求 VLAN 支持）。

### 3.3 DmaDescriptor

```rust
#[derive(Debug, Clone, Copy)]
pub struct DmaDescriptor {
    pub buffer_addr: u64,    // 物理地址
    pub buffer_length: u32,  // 有效数据长度
    pub flags: u32,          // 控制标志（OWN/IOC/LS/FS）
    pub status: u32,         // DMA 完成后写入的状态
}
```

标志位常量：
- `DESC_OWN  = 0x8000_0000`（bit 31）：DMA 拥有
- `DESC_IOC  = 0x4000_0000`（bit 30）：完成时触发中断
- `DESC_LS   = 0x2000_0000`（bit 29）：帧最后一段
- `DESC_FS   = 0x1000_0000`（bit 28）：帧第一段

### 3.4 DmaRing

```rust
pub struct DmaRing {
    pub tx_desc: Vec<DmaDescriptor>,
    pub rx_desc: Vec<DmaDescriptor>,
    pub tx_head: u32,  // CPU 写位置（生产者）
    pub tx_tail: u32,  // DMA 完成位置（消费者）
    pub rx_head: u32,  // CPU 回收位置（生产者，归还缓冲给 DMA）
    pub rx_tail: u32,  // CPU 检查位置（消费者，查找新帧）
}
```

环形缓冲满空判定（保留 1 个空槽区分满与空）：
- 满：`(head + 1) % count == tail`
- 空：`head == tail`

### 3.5 MacController

```rust
pub struct MacController<R: MacRegs> {
    pub regs: R,
    pub mac_addr: [u8; 6],
    pub mtu: usize,
    pub dma: DmaRing,
    pub tx_buffers: Vec<Vec<u8>>,
    pub rx_buffers: Vec<Vec<u8>>,
    pub stats: NetStats,
    pub promiscuous: bool,
    pub phy: GenericPhy,
    pub initialized: bool,
}
```

---

## 4. DMA 收发流程

### 4.1 OWN 位所有权协议

| OWN | 所有者 | 含义 |
|-----|--------|------|
| 1 | DMA | DMA 正在处理该描述符，CPU 不可修改 |
| 0 | CPU | DMA 已释放，CPU 可读写 |

### 4.2 TX 发送流程

```text
1. CPU 调用 send(frame):
   a. 检查 link_up / initialized / frame.len() <= mtu
   b. dma.tx_enqueue() → 返回可用描述符索引 idx（满返回 None → NoBuffer）
   c. 将 frame 复制到 tx_buffers[idx]
   d. 设置 desc.buffer_addr / buffer_length
   e. 设置 desc.flags = DESC_OWN | DESC_IOC | DESC_FS | DESC_LS（整帧单描述符）
   f. 推进 tx_head
   g. 写 DMA_TX_POLL 触发 DMA 轮询
   h. stats.record_tx(len)
2. DMA 发送帧，完成后清 DESC_OWN，置中断
3. handle_irq() 处理 TX 完成中断:
   a. 读 DMA_STATUS，检测 TX_INT
   b. 推进 tx_tail（释放已完成的描述符）
   c. 清 DMA_STATUS
```

### 4.3 RX 接收流程

```text
1. init() 时:
   a. 所有 RX 描述符设置 DESC_OWN | DESC_IOC（交给 DMA）
   b. 分配 rx_buffers
2. DMA 接收帧:
   a. 写数据到 rx_buffers[idx]
   b. 设置 desc.status = 帧长度
   c. 清 DESC_OWN（交还 CPU）
   d. 置 RX 中断
3. handle_irq() 处理 RX 中断:
   a. 读 DMA_STATUS，检测 RX_INT
   b. 清 DMA_STATUS
4. CPU 调用 recv(buf):
   a. dma.rx_dequeue() → 返回有新帧的描述符索引 idx（无返回 None → NoBuffer）
   b. 检查 buf.len() >= desc.status（帧长度），不足返回 FrameTooLarge
   c. 复制 rx_buffers[idx][..frame_len] 到 buf
   d. dma.rx_recycle(idx)：重置 desc，设置 DESC_OWN | DESC_IOC 交还 DMA
   e. stats.record_rx(frame_len)
   f. 返回 Ok(frame_len)
```

---

## 5. PHY 配置

### 5.1 MII 管理协议

PHY 寄存器通过 MAC 的 MII 管理接口访问：

```text
读操作:
1. 写 MAC_MII_ADDR = (phy_addr << 11) | (reg << 6) | MII_BUSY
2. 轮询 MAC_MII_ADDR 的 MII_BUSY 位直至清除
3. 读 MAC_MII_DATA 获取寄存器值

写操作:
1. 写 MAC_MII_DATA = value
2. 写 MAC_MII_ADDR = (phy_addr << 11) | (reg << 6) | MII_BUSY | MII_WRITE
3. 轮询 MAC_MII_ADDR 的 MII_BUSY 位直至清除
```

`MAC_MII_ADDR` 位域：`[15:11]` PHY 地址，`[10:6]` 寄存器地址，bit 1 写标志，bit 0 忙标志。

### 5.2 标准 MII 寄存器

| 寄存器 | 地址 | 用途 |
|--------|------|------|
| BMCR | 0x00 | 基本控制（复位/自协商/速率/双工） |
| BMSR | 0x01 | 基本状态（链路/自协商完成） |
| PHYID1 | 0x02 | PHY ID 高位 |
| PHYID2 | 0x03 | PHY ID 低位 |
| ANAR | 0x04 | 自协商能力宣告 |
| ANLPAR | 0x05 | 自协商对端能力 |

### 5.3 BMCR/BMSR 关键位

```rust
pub const BMCR_RESET: u16   = 0x8000;  // 复位位
pub const BMCR_AUTONEG: u16 = 0x1000;  // 自协商使能
pub const BMCR_RESTART: u16 = 0x0200;  // 重启自协商
pub const BMSR_LINK: u16         = 0x0004;  // 链路已建立
pub const BMSR_ANEG_COMPLETE: u16 = 0x0020; // 自协商完成
```

### 5.4 GenericPhy 自协商流程

```text
1. reset(): 写 BMCR = BMCR_RESET，等待复位完成（BMCR_RESET 位自动清除）
2. autoneg():
   a. 写 BMCR = BMCR_AUTONEG | BMCR_RESTART
   b. 轮询 BMSR & BMSR_ANEG_COMPLETE（最多 1000 次，超时返回 Timeout）
   c. 调用 update_link_state() 解析最终速率/双工
3. update_link_state():
   a. 读 BMSR，检查 BMSR_LINK
   b. 读 ANLPAR，解析对端能力：
      - 0x1000 优先 1000M
      - 0x0100 100M Full
      - 0x0080 100M Half
      - 0x0040 10M Full
      - 0x0020 10M Half
   c. 更新 phy_state
```

---

## 6. Mock 测试策略

### 6.1 MockMacRegs

`MockMacRegs` 用两个 `BTreeMap` 模拟寄存器空间：
- `mac_regs: BTreeMap<u64, u32>` — MAC 寄存器
- `phy_regs: BTreeMap<u8, u16>` — PHY 寄存器

关键在于模拟 MII 管理协议：当测试代码写 `MAC_MII_ADDR` 时，`MockMacRegs::write()`
解析该值，执行对应的 PHY 寄存器读写，更新 `MAC_MII_DATA`，并清除 `MII_BUSY`。
这使得 `GenericPhy` 的 MII 访问代码无需修改即可在 mock 上运行。

### 6.2 测试辅助方法

`MacController` 在 `#[cfg(test)]` 下提供：
- `simulate_tx_completion()` — 模拟 DMA 完成发送（清 OWN，推进 tail）
- `simulate_rx_frame(frame)` — 模拟 DMA 接收一帧（写入 rx_buffer，清 OWN，置状态）

这些方法仅在测试编译，不污染生产代码。

### 6.3 测试覆盖

| 模块 | 测试数 | 覆盖范围 |
|------|--------|---------|
| error.rs | 16 | 9 变体 Display + NetStats 累加 + Default |
| eth_frame.rs | 23 | 编解码 + 三种 ethertype + 边界 + 往返 |
| dma_ring.rs | 26 | 入队/出队/环绕/满空/统计/标志位 |
| phy.rs | 21 | 寄存器读写 + 自协商 + 速率/双工解析 + 超时 |
| mac.rs | 35 | 初始化 + send/recv 全流程 + 错误路径 + 混杂模式 |
| mock.rs | 9 | MII 协议模拟 + 寄存器读写 + PHY 地址隔离 |
| **总计** | **130** | — |

---

## 7. MAC 寄存器偏移

```rust
pub const MAC_CR: u64       = 0x00;  // MAC 配置（TX/RX 使能）
pub const MAC_FF: u64       = 0x04;  // 帧过滤器（bit0 = 混杂模式）
pub const MAC_MII_ADDR: u64 = 0x10;  // MII 地址（PHY 寄存器访问控制）
pub const MAC_MII_DATA: u64 = 0x14;  // MII 数据
pub const DMA_TX_POLL: u64  = 0x48;  // DMA TX 轮询需求
pub const DMA_RX_POLL: u64  = 0x4C;  // DMA RX 轮询需求
pub const DMA_STATUS: u64   = 0x60;  // DMA 状态（中断原因）
```

`MmioMacRegs`（仅 `#[cfg(target_arch = "aarch64")]`）通过 `core::ptr::read_volatile`
/ `write_volatile` 访问这些偏移，基址由 `new(base_addr: u64)` 指定。

---

## 8. 性能基准

蓝图 §6.3 目标（待 QEMU/真机验证）：

| 指标 | 目标 |
|------|------|
| 帧发送延迟 | < 10μs |
| 帧接收延迟 | < 10μs |
| 吞吐量 | ≥ 500 Mbps |

v0.27.0 仅交付软件实现 + mock 测试，性能验证延后到 QEMU/实机阶段。

---

## 9. 文件布局

```text
crates/drivers/net/
├── Cargo.toml          # 无外部依赖
└── src/
    ├── lib.rs          # 模块导出 + crate 文档 + VERSION + 条件导出 MmioMacRegs
    ├── error.rs        # NetError(9) + NetStats
    ├── eth_frame.rs    # EthFrame + EtherType + 编解码
    ├── dma_ring.rs     # DmaRing + DmaDescriptor + DESC_* 标志位
    ├── phy.rs          # PhyDriver trait + GenericPhy + PhyState/Speed/Duplex
    ├── mac.rs          # MacRegs trait + MacController + NetDevice impl + MmioMacRegs
    └── mock.rs         # MockMacRegs（#[cfg(test)]，BTreeMap 后端）
```

---

## 10. 设计决策记录

### 10.1 为什么用 MacRegs trait 抽象寄存器访问

- **可测试性**：mock 测试无需真实硬件，CI 可在主机上运行全部 130 个测试
- **解耦**：MacController 逻辑与寄存器访问机制分离，未来支持 PCI 设备仅需新增
  `PciMacRegs` 实现
- **不修改 HAL**：[crates/hal/hal/src/arm64/net_mmio.rs](../../crates/hal/hal/src/arm64/net_mmio.rs)
  是 v0.7.0 交付物，已稳定。MmioMacRegs 复用相同的 volatile 读写模式但不依赖 HAL

### 10.2 为什么 GenericPhy 不持有寄存器

`GenericPhy` 的所有方法接受 `&mut R: MacRegs`，而非自己持有寄存器引用：
- 避免 `MacController` 与 `GenericPhy` 同时借用同一寄存器集的冲突
- 利用 Rust 的 disjoint field borrow：`self.phy.autoneg(&mut self.regs)` 合法
- `GenericPhy` 仅持有 `phy_addr: u8` 与缓存 `PhyState`，无状态冲突

### 10.3 为什么不用 std::collections::HashMap

项目规则 §4.3 要求全项目 no_std。`MockMacRegs` 使用 `alloc::collections::BTreeMap`，
兼容 no_std。`mock.rs` 整个模块在 `#[cfg(test)]` 下编译，测试时启用 std 但仍用
BTreeMap 保持一致性。

### 10.4 为什么 MmioMacRegs 条件导出

`MmioMacRegs` 使用 `core::ptr::read_volatile`，本身 no_std 安全，但仅在 aarch64
目标有意义。`lib.rs` 用 `#[cfg(target_arch = "aarch64")]` 条件导出，避免主机测试
时引用不存在的硬件地址。

### 10.5 为什么归属 drivers 子系统

按 §2.3.2 归属判定：网卡属外设驱动，与 eneros-time、eneros-watchdog、eneros-fs
同属 `crates/drivers/`。crate 名 `eneros-net`，路径 `crates/drivers/net/`。

---

## 11. 依赖关系

| 依赖 | 版本 | 用途 |
|------|------|------|
| 无外部依赖 | — | 纯 Rust 实现，no_std |
| 用户堆 | v0.11.0 | Vec<Vec<u8>> 帧缓冲分配 |

依赖链：
```
v0.11.0(用户堆) → v0.27.0(以太网驱动) → v0.28.0(TCP/IP 协议栈)
```

v0.28.0 将通过 `eneros-net = { path = "../net" }`（同在 drivers/）依赖本 crate，
并基于 `NetDevice` trait 接入 smoltcp 协议栈。

---

## 12. 后续版本

| 版本 | 消费方式 | 说明 |
|------|---------|------|
| v0.28.0 | TCP/IP 协议栈 | smoltcp 通过 NetDevice trait 收发原始帧 |
| v0.30.x | IEC 104 / Modbus TCP | 基于 TCP/IP 的工业协议 |
| v0.98.0/v0.98.1 | 纵向加密 | 链路层数据加密 |

`NetDevice` trait 的 7 个方法（send/recv/mac_address/mtu/link_up/set_promiscuous/stats）
在 v0.28.0 中保持稳定，无需重构。

---

## 13. no_std 合规性

本 crate 严格遵守 §4.3 no_std 要求：

```rust
#![cfg_attr(not(test), no_std)]
extern crate alloc;
```

- 使用 `alloc::vec::Vec`、`alloc::collections::BTreeMap`、`alloc::format!`
- 无 `std::sync::Mutex`、`std::net`、`std::io`、`std::time`
- 寄存器访问用 `core::ptr::read_volatile` / `write_volatile`（no_std 安全）
- `MockMacRegs` 用 `BTreeMap` 而非 `HashMap`（no_std 兼容）
- `mock.rs` 整个模块在 `#[cfg(test)]` 下编译

---

## 14. 已知限制

1. **无真实硬件验证**：v0.27.0 仅交付软件实现 + mock 测试，性能目标（<10μs 延迟、
   ≥500 Mbps 吞吐）需 QEMU/实机验证
2. **无中断注册**：`handle_irq()` 接口已提供，但中断向量注册由上层（HAL/中断
   控制器）负责，v0.27.0 不实现
3. **无特定 PHY 芯片驱动**：仅提供 `GenericPhy` 通用实现，RTL8211/YT8521 等
   特定芯片适配为后续增强
4. **无 VLAN 支持**：`EthFrame` 不解析 VLAN 标签（0x8100），后续可扩展
5. **DMA 缓冲区缓存一致性**：ARM64 上需 cache flush/invalidate，v0.27.0 软件层
   不处理，依赖硬件一致性或上层处理

---

## 15. 构建与测试

```bash
# 主机侧单元测试（130 个）
cargo test -p eneros-net

# aarch64 交叉编译验证
cargo build -p eneros-net --target aarch64-unknown-none \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem

# Lint
cargo clippy -p eneros-net --all-targets -- -D warnings

# 文档生成
cargo doc -p eneros-net --no-deps
```

---

## 16. 参考

- 蓝图 §42.4（v0.27.0 未标注为过度设计，按完整版实现）
- 项目规则 §4.3（no_std 合规）、§5.5（默认集成清单：smoltcp）
- [IEEE 802.3](https://standards.ieee.org/ieee/802.3/10450/) — 以太网帧格式
- [MII Management Interface](https://en.wikipedia.org/wiki/Media_Independent_Interface#Management_Interface)
- v0.24.0 eneros-fs 设计文档（crate 结构参考）
