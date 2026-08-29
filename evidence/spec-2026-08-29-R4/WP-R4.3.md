# WP-R4.3 留证 · C2 handover-abi 未竟收编（值冻结重定位）

日期：2026-08-29 · 基线 0.314.20 @ b5d939b（WIP 树）

## 改动面

| 文件 | 改动 |
|---|---|
| abi/handover-abi/src/lib.rs | 收编 5 常量：`NET_MMIO_VADDR=0x4002_0000`、`NET_DMA_PAGES=9`、`UART_MMIO_VADDR=0x4003_0000`、`VIRTIO_MMIO_REGION_BASE=0x0a00_0000`、`VIRTIO_MMIO_SLOT_STRIDE=0x200`；冻结测试扩为收编面全量值断言；寄存器序浮文档转普通注释（clippy 清洁） |
| root-task/src/spawn.rs | `NET_MMIO_VADDR/NET_DMA_PAGES/UART_MMIO_VADDR` 本地定义删除并入 re-export；`STORAGE_VIRTIO_REGION_BASE/STORAGE_MMIO_SLOT_STRIDE` 改 const→const 引 handover-abi（调用点不动）；SVC_VSPACE_BASE 注释互认陈旧行订正 |
| root-task/src/main.rs | 本地 `VSPACE_BASE` 删除，改引 `crate::spawn::SVC_VSPACE_BASE`（同 crate 单源） |
| root-task/src/driver_probe.rs | `VIRTIO_REGION_BASE/VIRTIO_MMIO_SLOT_STRIDE` 改引 handover-abi（44.2 冻结登记注释保留） |
| protocol-services/src/net_transport.rs | 本地 `NET_DMA_PAGES` 定义删除，`pub use eneros_handover_abi::NET_DMA_PAGES` |
| protocol-services/Cargo.toml | + `eneros-handover-abi` |
| security/crypto/src/rng/entropy.rs | 默认基址改引 handover-abi 常量 |
| security/crypto/Cargo.toml | + `eneros-handover-abi`（no_std 零传递依赖） |

## 判据核对（spec R4 §1 WP-R4.3）

- **C1 单源核验**：`grep -rn 'pub const NET_DMA_PAGES'` → 恰 1 处
  （handover-abi）+ 1 处 re-export；`0x0a00_0000` 字面量仅存 handover-abi
  （定义 + 冻结测试）与 entropy 文档注释/driver_probe 测试夹具（模拟 DTB
  节点名，非竞争定义）；`0x4002_0000/0x4003_0000` 除 handover-abi 外零命中；
  `VSPACE_BASE` 恰 1 处定义（spawn.rs:995，main.rs 引用之）。
- **C2**：`cargo test -p eneros-handover-abi` → **1 passed**（冻结测试含
  收编面 5 值断言）；区间不交由 root-task 既有
  `uart_mmio_vaddr_region_disjoint_from_other_device_regions` /
  `net_mmio_vaddr_unique_across_all_slots` 冻结（消费 re-export 不改名）。
- **C3**：`cargo test -p eneros-root-task` → **112 passed / 0 failed**；
  `cargo test -p eneros-protocol-services --lib net_transport` →
  **14 passed / 0 failed**。
- **C4**：`cargo test -p eneros-crypto --lib rng` → **27 passed / 0 failed**
  （依赖新增无环、host 面编译干净）。
- **C5**：clippy（6 包 + handover-abi 复核）除 WIP 预存 ai_worker.rs:605
  外干净；fmt 全过。

## 体检声称比对（报告 C2 卡）

| 体检证据 | 修复后状态 |
|---|---|
| NET_DMA_PAGES 双源（net_transport.rs:52 注释自称单源） | ✓ 单源 + re-export，虚假"单一事实源"注释删除 |
| 板级 0x0a00_0000/0x200 三抄 | ✓ handover-abi 单源，三面改引 |
| SVC_VSPACE_BASE 注释互认（spawn.rs:994/main.rs:291） | ✓ main.rs 引用 spawn 常量，互认注释订正 |
| UART_MMIO_VADDR 无单源 | ✓ 收编 handover-abi（svc-modbus-rtu 消费侧本无副本，经 X7 接收） |
| storage 版已治 net 版没治 | ✓ 同病同治完成 |

## 行为变更声明

所有常量值逐字节不变（const→const 重定位，编译期等价）；无 QEMU 门槛
（镜像行为不变）。root-task 若需出 target 镜像按既有 SOP 重链接即可。
