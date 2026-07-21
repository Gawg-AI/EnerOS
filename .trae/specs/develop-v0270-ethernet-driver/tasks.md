# Tasks — v0.27.0 以太网网卡驱动

- [x] Task 1: 创建 eneros-net crate 骨架
  - [x] SubTask 1.1: 创建 `crates/drivers/net/Cargo.toml`（name=eneros-net, version=0.27.0, 无外部依赖, edition=2021）
  - [x] SubTask 1.2: 创建 `crates/drivers/net/src/lib.rs`（`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明 + VERSION 常量）
  - [x] SubTask 1.3: 根 `Cargo.toml` workspace.members 添加 `"crates/drivers/net"`，workspace.package.version 改为 `"0.27.0"`
  - [x] 验证: `cargo metadata --format-version 1 > /dev/null` 成功

- [x] Task 2: 实现 error.rs — NetError + NetStats
  - [x] SubTask 2.1: 定义 `NetError` 枚举（9 变体：NotInitialized/LinkDown/NoBuffer/DmaError(u32)/FrameTooLarge{size,max}/FrameTooSmall/CrcError/PhyError/Timeout）
  - [x] SubTask 2.2: 实现 `Debug` + `Clone` + `PartialEq` + `core::fmt::Display`
  - [x] SubTask 2.3: 定义 `NetStats` 结构体（tx_packets/rx_packets/tx_bytes/rx_bytes/tx_errors/rx_errors/rx_dropped，全 u64）+ `Default` + `Clone`
  - [x] 验证: error 类型构造与 Display 测试 (16 tests)

- [x] Task 3: 实现 eth_frame.rs — 以太网帧结构
  - [x] SubTask 3.1: 定义 `EtherType` 枚举（Ipv4=0x0800/Ipv6=0x86DD/Arp=0x0806/Other(u16)）+ `from_u16` / `to_u16` + Display
  - [x] SubTask 3.2: 定义 `EthFrame` 结构体（dst_mac: [u8;6], src_mac: [u8;6], ethertype: EtherType, payload: Vec<u8>）
  - [x] SubTask 3.3: 实现 `EthFrame::encode(&self) -> Vec<u8>`（dst+src+ethertype 大端+payload）
  - [x] SubTask 3.4: 实现 `EthFrame::decode(&[u8]) -> Result<Self, NetError>`（<14 字节返回 FrameTooSmall）
  - [x] SubTask 3.5: 实现 `EthFrame::new(dst, src, ethertype, payload)` 构造器 + `is_broadcast()` 判断
  - [x] 验证: 帧编解码往返测试 + 边界测试（空 payload、最小帧、超长帧）(23 tests)

- [x] Task 4: 实现 dma_ring.rs — DMA 描述符环
  - [x] SubTask 4.1: 定义 `DescFlags` bitflags 常量（DESC_OWN=0x80000000, DESC_IOC=0x40000000, DESC_LS=0x20000000, DESC_FS=0x10000000）
  - [x] SubTask 4.2: 定义 `DmaDescriptor` 结构体（buffer_addr: u64, buffer_length: u32, flags: u32, status: u32）+ `is_owned_by_dma()` / `set_owned_by_dma()` / `is_first()` / `is_last()`
  - [x] SubTask 4.3: 定义 `DmaRing` 结构体（tx_desc: Vec<DmaDescriptor>, rx_desc: Vec, tx_head/tail/rx_head/tail: u32）+ `new(tx_count, rx_count)` 构造器
  - [x] SubTask 4.4: 实现 TX 环操作：`tx_enqueue(&mut self) -> Option<usize>`（返回可用描述符索引，满返回 None）+ `tx_advance(&mut self)` + `tx_is_full()` / `tx_is_empty()`
  - [x] SubTask 4.5: 实现 RX 环操作：`rx_dequeue(&mut self) -> Option<usize>`（返回有新帧的描述符索引）+ `rx_recycle(&mut self)` + `rx_is_full()` / `rx_is_empty()`
  - [x] SubTask 4.6: 实现 `tx_count()` / `rx_count()` / `tx_pending()` / `rx_available()` 统计方法
  - [x] 验证: 环形缓冲入队/出队/满/空/环绕测试 (26 tests)

- [x] Task 5: 实现 phy.rs — PHY 驱动
  - [x] SubTask 5.1: 定义 `PhySpeed` 枚举（Speed10M/Speed100M/Speed1000M）+ Display + Default（#[default] Speed10M）
  - [x] SubTask 5.2: 定义 `PhyDuplex` 枚举（Half/Full）+ Display + Default（#[default] Half）
  - [x] SubTask 5.3: 定义 `PhyState` 结构体（link_up: bool, speed: PhySpeed, duplex: PhyDuplex, autoneg_complete: bool）+ `Default`
  - [x] SubTask 5.4: 定义 `PhyDriver` trait（reset/autoneg/read_reg/write_reg/link_state）+ MII 寄存器常量（BMCR=0x00, BMSR=0x01, PHYID1=0x02, PHYID2=0x03, ANAR=0x04, ANLPAR=0x05）
  - [x] SubTask 5.5: 实现 `GenericPhy<R: MacRegs>` 通用 PHY 驱动（通过 MacRegs 读写 MII 寄存器，phy_addr: u8）
  - [x] SubTask 5.6: 实现 `GenericPhy::reset()`（写 BMCR 0x8000 复位位）+ `autoneg()`（写 BMCR 0x1200 自协商+重启，轮询 BMSR 0x20 完成）+ `link_state()`（读 BMSR + ANLPAR 解析速率/双工）
  - [x] 验证: PHY 寄存器读写 mock 测试 + 自协商状态解析测试 (21 tests)

- [x] Task 6: 实现 mac.rs — MAC 控制器驱动 + NetDevice 实现
  - [x] SubTask 6.1: 定义 `MacRegs` trait（`read(&self, offset: u64) -> u32` + `write(&mut self, offset: u64, value: u32)`）+ MAC 寄存器偏移常量（MAC_CR/MAC_FF/MAC_MII_ADDR/MAC_MII_DATA/DMA_TX_POLL/DMA_RX_POLL/DMA_STATUS）
  - [x] SubTask 6.2: 定义 `MacController<R: MacRegs>` 结构体（regs, mac_addr, mtu, dma_tx, dma_rx, tx_buffers, rx_buffers, stats, promiscuous, phy_state）+ `new(regs, mac_addr, mtu, tx_count, rx_count)`
  - [x] SubTask 6.3: 实现 `MacController::init(&mut self)` — 配置 DMA 环、初始化 RX 描述符为 OWN+IOC、启动 TX/RX
  - [x] SubTask 6.4: 实现 `MacController::handle_irq(&mut self)` — 处理 TX 完成中断（推进 tail）+ RX 到达中断
  - [x] SubTask 6.5: 实现 `NetDevice` trait for `MacController<R>`：send/recv/mac_address/mtu/link_up/set_promiscuous/stats
  - [x] SubTask 6.6: 实现 `MmioMacRegs`（真实硬件实现，base_addr: u64 + volatile 读写，仅 `#[cfg(target_arch = "aarch64")]`）
  - [x] 验证: 用 MockMacRegs 测试 MAC 初始化 + send/recv 全流程 + 错误路径 (35 tests)

- [x] Task 7: 实现 mock.rs — 测试用 Mock
  - [x] SubTask 7.1: 实现 `MockMacRegs`（BTreeMap<u64, u32> 模拟寄存器，实现 MacRegs trait）+ 预设寄存器值 helper
  - [x] SubTask 7.2: mock.rs 仅在 `#[cfg(test)]` 下编译
  - [x] 验证: mock 可被 mac.rs 测试使用 (9 tests，集成在 Task 6 测试中)

- [x] Task 8: lib.rs 导出与文档注释
  - [x] SubTask 8.1: lib.rs 添加模块导出（pub mod error/eth_frame/dma_ring/phy/mac/mock）+ pub use 关键类型（NetDevice/NetError/NetStats/EthFrame/EtherType/DmaRing/DmaDescriptor/PhyDriver/GenericPhy/PhyState/MacController/MacRegs）
  - [x] SubTask 8.2: lib.rs 添加 crate 文档注释（架构图 + 使用示例 + 设计决策 + intra-doc 链接）
  - [x] SubTask 8.3: MmioMacRegs 条件导出 `#[cfg(target_arch = "aarch64")]`
  - [x] 验证: `cargo doc -p eneros-net --no-deps` 生成文档无警告

- [x] Task 9: 文档与配置
  - [x] SubTask 9.1: 创建 `docs/drivers/net-driver-design.md`（488 行设计文档：架构 + 数据结构 + DMA 收发流程 + PHY 配置 + Mock 测试策略 + 性能基准 + 文件布局 + 设计决策 + 后续版本 + no_std 合规 + 参考）
  - [x] SubTask 9.2: 创建 `configs/network.toml`（65 行默认网络配置：[device] mac_addr/mtu/tx_desc_count/rx_desc_count/mac_base_addr，[phy] phy_addr/autoneg/manual_speed/manual_duplex/autoneg_timeout_ms，[promiscuous] enabled，[dma] tx_ioc/rx_ioc）
  - [x] 验证: 文档位于 `docs/drivers/`（§2.3.3 文档分类），非 docs/ 根

- [x] Task 10: 版本标识更新
  - [x] SubTask 10.1: 根 `Cargo.toml` workspace.package.version = "0.27.0"（Task 1 已完成）
  - [x] SubTask 10.2: `Makefile` VERSION := 0.27.0 + Version: v0.27.0 + 添加 net-build/net-test 目标 + help 条目
  - [x] SubTask 10.3: `.github/workflows/ci.yml` Version: v0.27.0 + 添加 "Build net crate" 步骤（aarch64-unknown-none 交叉编译）
  - [x] SubTask 10.4: `ci/src/gate.rs` 注释添加 eneros-net（v0.27.0 以太网网卡驱动）说明（clippy + test 两处）
  - [x] 验证: 版本号一致性 + ci.yml 含 eneros-net 构建步骤

- [x] Task 11: 构建与质量验证
  - [x] SubTask 11.1: `cargo fmt -p eneros-net -- --check` 通过
  - [x] SubTask 11.2: `cargo clippy -p eneros-net --all-targets -- -D warnings` 通过（修复 12 个 clippy 警告：derivable-impls ×3、unnecessary-cast ×4、identity-op ×4、needless-range-loop ×1）
  - [x] SubTask 11.3: `cargo test -p eneros-net` 通过（130 tests: 0 failed, 1 ignored doc-test）
  - [x] SubTask 11.4: `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（回归测试全部 PASS）
  - [x] SubTask 11.5: `cargo run -p eneros-ci` — fmt ✓ / clippy ✓ / test ✓ / audit ⚠️（网络故障：无法连接 github.com 拉取 RustSec advisory DB，非代码问题；eneros-net 无外部依赖）
  - [x] SubTask 11.6: aarch64 交叉编译通过（WSL2 Ubuntu-22.04，1.02s，`cargo build -p eneros-net --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）
  - [x] SubTask 11.7: `cargo deny check advisories licenses bans sources` — ⚠️ 网络故障（无法 fetch advisory DB），非代码问题（eneros-net 无外部依赖，不引入任何 advisory/license 风险）
  - [x] 验证: 所有代码检查项 PASS（audit/deny 失败为网络问题，非 v0.27.0 引入）

# Task Dependencies

- Task 2 (error) 无依赖，可先开始
- Task 3 (eth_frame) 依赖 Task 2 (NetError)
- Task 4 (dma_ring) 依赖 Task 2 (NetError 可选，主要用于错误码)
- Task 5 (phy) 依赖 Task 2 (NetError) + Task 6 (MacRegs trait)
- Task 6 (mac) 依赖 Task 2 (NetError/NetStats) + Task 4 (DmaRing) + Task 5 (PhyState)
- Task 7 (mock) 依赖 Task 5 (PhyDriver) + Task 6 (MacRegs)
- Task 8 (lib.rs) 依赖 Task 2-7
- Task 9 (文档) 依赖 Task 7
- Task 10 (版本) 可与 Task 2-7 并行（仅改配置文件）
- Task 11 (验证) 依赖 Task 8,9,10 全部完成

# 关键技术要点

## MacRegs trait 解耦
- `MacRegs` trait 定义在 mac.rs，phy.rs 通过 `PhyDriver` trait 的泛型参数依赖 MacRegs
- `GenericPhy<R: MacRegs>` 接受 MacRegs 作为泛型参数，方法签名 `&mut R`（MII 读需写 MAC_MII_ADDR）
- mac.rs 的 `MacController<R: MacRegs>` 同样接受 MacRegs
- mock.rs 的 `MockMacRegs` 实现 MacRegs，模拟 MII 管理协议

## DMA 描述符 OWN 位语义
- `DESC_OWN` 位置 1：描述符由 DMA 拥有（CPU 不可修改）
- `DESC_OWN` 位清 0：描述符由 CPU 拥有（DMA 已释放）
- TX：CPU 写数据 → 设置 OWN=1 交给 DMA → DMA 发送 → 清 OWN=0 通知 CPU 完成
- RX：DMA 接收 → 写数据 → 清 OWN=0 交给 CPU → CPU 读取 → 设置 OWN=1 交还 DMA

## no_std 注意事项
- `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
- `MockMacRegs` 用 `alloc::collections::BTreeMap`（no_std 兼容），整个 mock.rs 在 `#[cfg(test)]` 下编译
- `MmioMacRegs` 用 `core::ptr::read_volatile` / `write_volatile`，仅 `#[cfg(target_arch = "aarch64")]` 导出

## Clippy 修复（Task 11）
- `derivable-impls` ×3：DmaDescriptor/PhySpeed/PhyDuplex 的手动 Default impl 改为 `#[derive(Default)]` + `#[default]` 属性
- `unnecessary-cast` ×4：dma_ring.rs 中 `(self.tx_head + 1) as u32` 等 u32→u32 冗余转型移除
- `identity-op` ×4：mock.rs 测试中 `(0u32 << 11)` 零移位无效果，移除
- `needless-range-loop` ×1：mac.rs 测试中 `for i in 14..len` 改为 `frame.iter_mut().enumerate()`
