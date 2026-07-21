# v0.27.0 以太网网卡驱动 Spec

## Why

Edge Box 需通过网络接收市场数据、与云端通信、接入 IEC 104 / Modbus TCP 设备。v0.27.0 实现以太网 MAC 驱动、DMA 收发环形缓冲、PHY 配置与原始以太网帧捕获，为 v0.28.0 TCP/IP 协议栈提供底层网络收发能力（`NetDevice` trait）。

蓝图 §42.4 **未**将 v0.27.0 标注为过度设计（仅 v0.24.0 LFS 和 v0.25.0 TSDB 被标注），按完整版实现合理。

## What Changes

- **新增 crate**：`crates/drivers/net/`（crate 名 `eneros-net`，归属 `drivers` 子系统，因网卡属外设驱动）
- **新增模块**（6 个源文件）：
  - `error.rs` — `NetError` 错误类型（9 变体）
  - `eth_frame.rs` — 以太网帧结构 + 编解码（`EthFrame`、`EtherType`）
  - `dma_ring.rs` — DMA 描述符环（`DmaRing`、`DmaDescriptor`、`DescFlags`）
  - `phy.rs` — PHY 驱动 trait + 通用实现（`PhyDriver` trait、`GenericPhy`、`PhyState`、`PhySpeed`、`PhyDuplex`）
  - `mac.rs` — MAC 控制器驱动（`MacController`、`MacRegs` trait、`NetDevice` trait 实现）
  - `mock.rs` — 测试用 Mock 寄存器 + Mock PHY（仅 `#[cfg(test)]` 或 feature gating）
- **新增文档**：`docs/drivers/net-driver-design.md`
- **新增配置**：`configs/network.toml`
- **版本标识更新**：根 `Cargo.toml`（workspace.version + members）、`Makefile`、`.github/workflows/ci.yml`、`ci/src/gate.rs`
- **BREAKING**：无（纯新增 crate，不修改现有 crate）

## Impact

- **Affected specs**: 解锁 v0.28.0 TCP/IP 协议栈（依赖 `NetDevice` trait）
- **Affected code**:
  - 新增：`crates/drivers/net/` 整个 crate
  - 修改：根 `Cargo.toml`（members + version）、`Makefile`、`ci.yml`、`ci/src/gate.rs`
  - **不修改**：[crates/hal/hal/src/arm64/net_mmio.rs](file:///e:/eneros/crates/hal/hal/src/arm64/net_mmio.rs)（v0.7.0 已有基础 MMIO，v0.27.0 通过 `MacRegs` trait 解耦，可选复用）
  - **不修改**：其他所有 crate

## ADDED Requirements

### Requirement: NetDevice Trait 抽象

系统 SHALL 提供 `NetDevice` trait 作为所有网络设备的统一抽象，供 v0.28.0 TCP/IP 协议栈使用。

```rust
pub trait NetDevice {
    fn send(&mut self, frame: &[u8]) -> Result<(), NetError>;
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, NetError>;
    fn mac_address(&self) -> [u8; 6];
    fn mtu(&self) -> usize;
    fn link_up(&self) -> bool;
    fn set_promiscuous(&mut self, on: bool);
    fn stats(&self) -> NetStats;
}
```

#### Scenario: 发送以太网帧
- **WHEN** 调用 `send(frame)` 且链路已up、frame ≤ MTU、TX 环有空闲描述符
- **THEN** 帧被复制到 TX 缓冲，描述符 ownership 交给 DMA，返回 `Ok(())`

#### Scenario: 发送失败 — 链路断开
- **WHEN** 调用 `send(frame)` 且 `link_up() == false`
- **THEN** 返回 `Err(NetError::LinkDown)`

#### Scenario: 发送失败 — 帧过大
- **WHEN** 调用 `send(frame)` 且 `frame.len() > mtu()`
- **THEN** 返回 `Err(NetError::FrameTooLarge { size, max })`

#### Scenario: 发送失败 — 无缓冲
- **WHEN** 调用 `send(frame)` 且 TX 环已满（head+1 == tail）
- **THEN** 返回 `Err(NetError::NoBuffer)`

#### Scenario: 接收以太网帧
- **WHEN** 调用 `recv(buf)` 且 RX 环有新帧（描述符 ownership 已回交给 CPU）
- **THEN** 帧被复制到 buf，描述符 ownership 交还 DMA，返回 `Ok(len)`

#### Scenario: 接收失败 — 无新帧
- **WHEN** 调用 `recv(buf)` 且 RX 环无新帧（所有描述符仍为 DMA 持有）
- **THEN** 返回 `Err(NetError::NoBuffer)`

### Requirement: 以太网帧结构

系统 SHALL 提供 `EthFrame` 结构体表示以太网帧，支持编解码。

```rust
pub struct EthFrame {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub ethertype: EtherType,
    pub payload: Vec<u8>,
}

pub enum EtherType {
    Ipv4,
    Ipv6,
    Arp,
    Other(u16),
}
```

#### Scenario: 帧编码
- **WHEN** 调用 `EthFrame::encode()` 
- **THEN** 返回 `Vec<u8>`，前 6 字节 dst_mac，接下来 6 字节 src_mac，2 字节 ethertype（大端），剩余 payload

#### Scenario: 帧解码
- **WHEN** 调用 `EthFrame::decode(&[u8])` 且输入 ≥ 14 字节
- **THEN** 返回 `Ok(EthFrame)`，正确解析 dst/src/ethertype/payload

#### Scenario: 帧解码失败 — 帧过短
- **WHEN** 调用 `EthFrame::decode(&[u8])` 且输入 < 14 字节
- **THEN** 返回 `Err(NetError::FrameTooSmall)`

### Requirement: DMA 描述符环

系统 SHALL 提供 `DmaRing` 管理 TX/RX 描述符环，支持环形缓冲入队/出队。

```rust
pub struct DmaRing {
    tx_desc: Vec<DmaDescriptor>,
    rx_desc: Vec<DmaDescriptor>,
    tx_head: u32,
    tx_tail: u32,
    rx_head: u32,
    rx_tail: u32,
}

pub struct DmaDescriptor {
    pub buffer_addr: u64,
    pub buffer_length: u32,
    pub flags: u32,
    pub status: u32,
}
```

#### Scenario: TX 环入队
- **WHEN** 调用 `tx_enqueue()` 且环未满
- **THEN** head 推进，返回新描述符索引

#### Scenario: TX 环已满
- **WHEN** 调用 `tx_enqueue()` 且 `(head+1) % count == tail`
- **THEN** 返回 `None`（环满）

#### Scenario: RX 环出队
- **WHEN** 调用 `rx_dequeue()` 且有新帧（描述符 OWN 位已清除）
- **THEN** 返回描述符索引，head 推进

### Requirement: PHY 驱动

系统 SHALL 提供 `PhyDriver` trait 和通用 `GenericPhy` 实现，支持 PHY 自协商与状态查询。

```rust
pub trait PhyDriver {
    fn reset(&mut self) -> Result<(), NetError>;
    fn autoneg(&mut self) -> Result<PhyState, NetError>;
    fn read_reg(&self, reg: u8) -> Result<u16, NetError>;
    fn write_reg(&mut self, reg: u8, val: u16) -> Result<(), NetError>;
    fn link_state(&self) -> PhyState;
}

pub struct PhyState {
    pub link_up: bool,
    pub speed: PhySpeed,
    pub duplex: PhyDuplex,
    pub autoneg_complete: bool,
}
```

#### Scenario: PHY 自协商
- **WHEN** 调用 `autoneg()` 且 PHY 支持自协商
- **THEN** 等待自协商完成，返回 `PhyState { link_up: true, autoneg_complete: true, ... }`

#### Scenario: PHY 寄存器读写
- **WHEN** 调用 `write_reg(0x00, 0x8000)` 然后 `read_reg(0x00)`
- **THEN** 读取值反映写入（或硬件响应）

### Requirement: MAC 控制器驱动

系统 SHALL 提供 `MacController` 实现 `NetDevice` trait，通过 `MacRegs` trait 抽象寄存器访问（便于 mock 测试）。

```rust
pub trait MacRegs {
    fn read(&self, offset: u64) -> u32;
    fn write(&mut self, offset: u64, value: u32);
}

pub struct MacController<R: MacRegs> {
    regs: R,
    mac_addr: [u8; 6],
    mtu: usize,
    dma_tx: DmaRing,
    dma_rx: DmaRing,
    tx_buffers: Vec<Vec<u8>>,
    rx_buffers: Vec<Vec<u8>>,
    stats: NetStats,
    promiscuous: bool,
    phy_state: PhyState,
}
```

#### Scenario: MAC 初始化
- **WHEN** 调用 `MacController::init(mac_addr)`
- **THEN** 配置 DMA 环、设置 MAC 地址、启动 TX/RX，返回 `Ok(())`

#### Scenario: 混杂模式切换
- **WHEN** 调用 `set_promiscuous(true)`
- **THEN** MAC_FF 寄存器 bit0 置位

### Requirement: 错误类型

系统 SHALL 提供 `NetError` 枚举，覆盖所有网络驱动错误场景。

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

## 设计决策（Karpathy 原则应用）

### 1. Simplicity First — 不实现 VLAN/CRC 软件处理

**决策**：`EthFrame` 不含 VLAN 标签解析，不含 FCS/CRC 字段。
**理由**：
- 蓝图未要求 VLAN
- 硬件 MAC 自动剥离 FCS，软件层无需处理
- 最小可用：标准以太网帧（dst+src+ethertype+payload）

### 2. Surgical Changes — 不修改 HAL net_mmio.rs

**决策**：v0.27.0 通过 `MacRegs` trait 抽象寄存器访问，不修改 [crates/hal/hal/src/arm64/net_mmio.rs](file:///e:/eneros/crates/hal/hal/src/arm64/net_mmio.rs)。
**理由**：
- HAL net_mmio.rs 是 v0.7.0 交付物，已稳定
- `MacRegs` trait 解耦：真实硬件实现 `MmioMacRegs`（可复用 HAL 的 volatile 读写模式），测试实现 `MockMacRegs`（内存数组）
- 避免跨 crate 修改，降低回归风险

### 3. Simplicity First — 不绑定特定 PHY 芯片

**决策**：提供 `GenericPhy` 通用实现，通过标准 MII 寄存器协议操作，不实现 RTL8211/YT8521 等特定芯片驱动。
**理由**：
- 蓝图提到 PHY 芯片差异是兼容性风险（§8.4）
- 标准 MII 寄存器（0x00 控制、0x01 状态、0x04/0x05 自协商能力）覆盖 90% PHY
- 特定芯片适配可作为后续增强（返回 `Other` 变体或子 trait）

### 4. Goal-Driven Execution — 测试策略

**验证标准**：
1. `cargo test -p eneros-net` 通过（单元测试 ≥80% 覆盖）
2. `cargo build -p eneros-net --target aarch64-unknown-none` 交叉编译通过
3. `cargo deny check advisories licenses bans sources` 通过
4. `cargo fmt` / `cargo clippy` 无 warning
5. `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 回归测试通过
6. `cargo run -p eneros-ci` Overall: PASS

**Mock 策略**：
- `MockMacRegs`：内存数组模拟寄存器，实现 `MacRegs` trait
- `MockPhy`：模拟 PHY 寄存器响应，实现 `PhyDriver` trait
- DMA 环操作用真实 `DmaRing`（纯逻辑，无硬件依赖）
- 帧编解码用真实 `EthFrame`（纯数据结构）

### 5. no_std 合规

- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- 使用 `alloc::vec::Vec`、`alloc::collections::VecDeque`
- 无 `std::` 依赖
- 寄存器访问用 `core::ptr::read_volatile` / `write_volatile`（真实硬件实现）

### 6. crate 归属

- 路径：`crates/drivers/net/`（网卡属外设驱动，符合 §2.3.2 drivers 子系统定义）
- crate 名：`eneros-net`
- 跨 crate 引用：v0.28.0 TCP/IP 将依赖 `eneros-net = { path = "../net" }`（同在 drivers/）

## 性能目标（蓝图 §6.3）

| 指标 | 目标 |
|------|------|
| 帧发送延迟 | < 10μs |
| 帧接收延迟 | < 10μs |
| 吞吐量 | ≥ 500 Mbps |

**注**：性能目标需真实硬件验证，v0.27.0 仅交付软件实现 + mock 测试，性能验证延后到 QEMU/实机阶段。

## 依赖

```toml
[dependencies]
# 无外部依赖（纯 Rust 实现，no_std）
# eneros-hal 可选（若 MmioMacRegs 需复用 HAL 工具），v0.27.0 暂不依赖

[dev-dependencies]
# 测试用 std
```

## 文件布局

```
crates/drivers/net/
├── Cargo.toml
└── src/
    ├── lib.rs          # 模块导出 + crate 文档 + VERSION
    ├── error.rs        # NetError + NetStats
    ├── eth_frame.rs    # EthFrame + EtherType + 编解码
    ├── dma_ring.rs     # DmaRing + DmaDescriptor + DescFlags
    ├── phy.rs          # PhyDriver trait + GenericPhy + PhyState/Speed/Duplex
    ├── mac.rs          # MacRegs trait + MacController + NetDevice impl
    └── mock.rs         # MockMacRegs + MockPhy（测试用）
```
