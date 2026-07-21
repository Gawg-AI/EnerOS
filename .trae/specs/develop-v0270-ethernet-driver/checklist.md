# Checklist — v0.27.0 以太网网卡驱动

## 目录结构校验（§2.4 C1-C5）

- [x] C1 新 crate 位置：eneros-net 位于 `crates/drivers/net/`，未直接放根目录
- [x] C2 workspace members：根 `Cargo.toml` 的 members 已添加 `"crates/drivers/net"`
- [x] C3 跨 crate path 引用：v0.27.0 无外部 crate 依赖（纯新增），v0.28.0 将用 `eneros-net = { path = "../net" }`
- [x] C4 文档分类：`docs/drivers/net-driver-design.md` 位于 `docs/drivers/` 子目录，未平面化放 `docs/` 根
- [x] C5 无根目录 crate：仓库根目录无新增 Rust crate 文件夹

## no_std 合规（§4.3）

- [x] `crates/drivers/net/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] 无 `use std::*`，改用 `alloc::*` / `core::*`
- [x] MockMacRegs/MockPhy 仅在 `#[cfg(test)]` 下编译（避免 BTreeMap 污染 no_std）
- [x] `MmioMacRegs` 用 `core::ptr::read_volatile` / `write_volatile`（无 std 依赖）
- [x] 无 `std::collections::HashMap`（改用 `alloc::collections::BTreeMap`）

## 接口实现完整性

- [x] error.rs: NetError 9 变体 + Debug/Clone/PartialEq/Display + NetStats 结构体
- [x] eth_frame.rs: EthFrame + EtherType (Ipv4/Ipv6/Arp/Other) + encode/decode + new/is_broadcast
- [x] dma_ring.rs: DmaRing + DmaDescriptor + DescFlags bitflags + TX/RX 入队出队 + 满空判断 + 统计
- [x] phy.rs: PhyDriver trait + GenericPhy + PhyState/PhySpeed/PhyDuplex + MII 寄存器常量 + reset/autoneg/link_state
- [x] mac.rs: MacRegs trait + MacController + NetDevice impl + init/handle_irq + MmioMacRegs
- [x] mock.rs: MockMacRegs（#[cfg(test)]）
- [x] lib.rs: 模块导出 + pub use 关键类型 + crate 文档注释

## NetDevice Trait 行为验证

- [x] send() 成功：链路 up + 帧 ≤ MTU + TX 环有空闲 → Ok(())
- [x] send() 失败 — 链路断开 → Err(LinkDown)
- [x] send() 失败 — 帧过大 → Err(FrameTooLarge { size, max })
- [x] send() 失败 — TX 环满 → Err(NoBuffer)
- [x] recv() 成功：RX 环有新帧 → Ok(len)
- [x] recv() 失败 — 无新帧 → Err(NoBuffer)
- [x] recv() 失败 — buf 过小 → Err(FrameTooLarge)
- [x] mac_address() 返回初始化时设置的 MAC
- [x] mtu() 返回初始化时设置的 MTU
- [x] link_up() 反映 phy_state.link_up
- [x] set_promiscuous(true/false) 正确修改 MAC_FF 寄存器 bit0
- [x] stats() 返回正确的收发统计

## 以太网帧编解码验证

- [x] encode() 输出格式正确（dst 6B + src 6B + ethertype 2B 大端 + payload）
- [x] decode() 正确解析标准帧
- [x] decode() < 14 字节返回 FrameTooSmall
- [x] EtherType::from_u16 / to_u16 双向转换正确（0x0800/0x86DD/0x0806/其他）
- [x] encode → decode 往返一致
- [x] is_broadcast() 判断全 0xFF 的 dst_mac

## DMA 环形缓冲验证

- [x] TX 环入队：未满时返回 Some(idx)，head 推进
- [x] TX 环入队：满时返回 None
- [x] RX 环出队：有新帧（OWN=0）返回 Some(idx)
- [x] RX 环出队：无新帧（OWN=1）返回 None
- [x] 环形环绕：head 到达 count 后回到 0
- [x] tx_is_full / tx_is_empty / rx_is_full / rx_is_empty 正确
- [x] tx_pending / rx_available 计数正确
- [x] DmaDescriptor::is_owned_by_dma / set_owned_by_dma 正确
- [x] DESC_OWN / DESC_IOC / DESC_FS / DESC_LS 标志位正确

## PHY 驱动验证

- [x] GenericPhy::reset() 写 BMCR 0x8000
- [x] GenericPhy::autoneg() 写 BMCR 0x1200，轮询 BMSR 0x20
- [x] GenericPhy::link_state() 读 BMSR + ANLPAR 解析速率/双工
- [x] read_reg / write_reg 通过 MacRegs 读写 MII
- [x] PhySpeed (10M/100M/1000M) 正确解析
- [x] PhyDuplex (Half/Full) 正确解析
- [x] PhyState::Default() 为 link_down 状态

## MAC 控制器验证

- [x] MacController::new() 创建实例，DMA 环为空，stats 全 0
- [x] MacController::init() 配置 DMA 环，RX 描述符设 OWN+IOC，启动 TX/RX
- [x] handle_irq() 处理 TX 完成（推进 tail）
- [x] handle_irq() 处理 RX 到达
- [x] MmioMacRegs 用 volatile 读写（仅 aarch64 编译）

## 构建校验（§2.4 C6-C11）

- [x] C6 `cargo metadata --format-version 1 > /dev/null` 成功（workspace 成员路径全部正确）
- [x] C7 `cargo test -p eneros-net` 通过（130 单元测试，0 failed, 1 ignored doc-test）
- [x] C8 `cargo build -p eneros-net --target aarch64-unknown-none` 通过（WSL2 交叉编译，1.02s）
- [x] C9 `cargo fmt -p eneros-net -- --check` 通过
- [x] C10 `cargo clippy -p eneros-net --all-targets -- -D warnings` 无 warning（修复 12 个 clippy 警告）
- [x] C11 `cargo deny check advisories licenses bans sources` — ⚠️ 网络故障（无法 fetch advisory DB from github.com），非代码问题（eneros-net 无外部依赖）

## 功能验证

- [x] 以太网帧编解码正确（含 Ipv4/Ipv6/Arp 三种 ethertype）
- [x] DMA 环形缓冲入队/出队/环绕正确
- [x] PHY 寄存器读写通过 MacRegs trait 正确传递
- [x] MAC 初始化配置 DMA 环 + 启动 TX/RX
- [x] NetDevice send/recv 全流程通过 mock 测试
- [x] 错误路径覆盖：LinkDown/FrameTooLarge/FrameTooSmall/NoBuffer
- [x] 混杂模式切换正确修改寄存器
- [x] 收发统计正确累加

## 回归测试

- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（全部 PASS，0 failures）
- [x] `cargo run -p eneros-ci` — fmt ✓ / clippy ✓ / test ✓ / audit ⚠️（网络故障，非代码问题）

## 文档与规范校验（§2.4 C12-C15）

- [x] C12 文档位置：`docs/drivers/net-driver-design.md` 在 `docs/drivers/` 下
- [x] C13 无垃圾文件：`git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] C14 .gitignore 覆盖：新产生的文件类型已在 .gitignore 中
- [x] C15 提交信息：遵循 Conventional Commits（待提交时遵守）

## 版本标识一致性

- [x] 根 `Cargo.toml` workspace.package.version = "0.27.0"
- [x] `crates/drivers/net/Cargo.toml` version = "0.27.0"
- [x] `Makefile` VERSION := 0.27.0
- [x] `.github/workflows/ci.yml` Version: v0.27.0 + 含 eneros-net 交叉编译步骤
- [x] `ci/src/gate.rs` 注释含 eneros-net（v0.27.0 以太网网卡驱动）说明

## CI 配置

- [x] ci.yml 添加 `Build net crate` 步骤（aarch64-unknown-none 交叉编译）
- [x] ci.yml clippy/test 步骤无需修改（workspace 级别已覆盖）
- [x] gate.rs clippy/test 排除列表注释更新（eneros-net 为 no_std crate，host-testable）

## 依赖许可证（SBOM）

- [x] v0.27.0 无外部依赖（纯 Rust 实现），无需新增 SBOM 条目
- [x] `cargo deny check licenses` — ⚠️ 网络故障（与 advisories 同因），非代码问题

## 测试统计

| 模块 | 测试数 | 状态 |
|------|--------|------|
| error.rs | 16 | ✅ 通过 |
| eth_frame.rs | 23 | ✅ 通过 |
| dma_ring.rs | 26 | ✅ 通过 |
| phy.rs | 21 | ✅ 通过 |
| mac.rs | 35 | ✅ 通过 |
| mock.rs | 9 | ✅ 通过 |
| **总计** | **130** | ✅ 全部通过 |

## 已知限制 / 后续增强

1. **无真实硬件验证**：v0.27.0 仅交付软件实现 + mock 测试，性能目标（<10μs 延迟、≥500 Mbps 吞吐）需 QEMU/实机验证
2. **无中断注册**：`handle_irq()` 接口已提供，但中断向量注册由上层（HAL/中断控制器）负责，v0.27.0 不实现
3. **无特定 PHY 芯片驱动**：仅提供 `GenericPhy` 通用实现，RTL8211/YT8521 等特定芯片适配为后续增强
4. **无 VLAN 支持**：`EthFrame` 不解析 VLAN 标签（0x8100），后续可扩展
5. **DMA 缓冲区缓存一致性**：ARM64 上需 cache flush/invalidate，v0.27.0 软件层不处理，依赖硬件一致性或上层处理
6. **cargo-deny 网络依赖**：audit 检查需联网拉取 RustSec advisory DB，离线环境失败（非代码问题）
