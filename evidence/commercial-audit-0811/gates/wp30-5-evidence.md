# WP-30.5 ABI/sel4-sys 卫生批（RT-012~015/017~020/023/026/031，11 项）修复与证据

执行日期：2026-08-11。蓝图坐标：30 §7（低风险批量）。

| ID | 修复 | 判据/验证 |
|---|---|---|
| RT-012 | REG_X30 与 REG_NZCV 同为 33 碰撞：X 寄存器索引整体 -1（X0=2...X30=32，NZCV=33 不变——与 seL4 aarch64 布局注释对齐；旧 REG_X0=3 实为 x1，服务参数错位被 QEMU 掩盖） | reg_indices_unique（36 常量两两不等 + 关键索引断言） |
| RT-013 | host stub message_info_new `(length&0xFF)<<8` → types::message_info_new（bits 0-7 统一） | 既有 message_info_round_trips |
| RT-014 | build_test_bootinfo_blob 写 off+16 vs 读 off+9 → 共享常量 UNTYPED_FLAGS_OFFSET=9（isDevice 标志，假绿修复） | 编译 + 既有测试 |
| RT-015 | sel4_shim.h IRQ 声明移入 header guard（守卫失效修复） | 编译通过 |
| RT-017 | frame_size_bits 对 ≤2MiB 一律 21-bit → 按实际尺寸（4K→12 / 2M→21 / 1G→30；4KiB 设备不再过度映射） | 编译 + QEMU 回归（版本验收） |
| RT-018 | virtio 探测 ACKNOWLEDGE 后失败分支复位 STATUS=0（状态污染修复） | 编译 + QEMU 回归 |
| RT-019 | DMA_POOL_BYTES static mut → dma_pool 私有模块内聚封装（统一访问面 + SAFETY 单线程不变量） | 编译 + 既有测试 |
| RT-020 | ipc-wire MR 952B < el0 1024B → MAX_EL0_PAYLOAD_IN_MR 契约常量 + 双向编译期断言 | el0_payload_at_mr_capacity_roundtrips（恰好成帧、+1 拒绝） |
| RT-023 | boot-manager build.rs GUID 注入重复块删一（保留前段） | 编译 + 产物 diff（版本验收） |
| RT-026 | mini_uart TX 无界忙等 → 有界 100_000 次（超限丢弃字符继续，绝不死循环） | 编译（零消费方） |
| RT-031 | retype_objects → retype_page_tables（诚实命名，3 处） | 编译 + 既有测试 |

## 回归
sel4-sys 18 / hal 25 / ipc-wire 13 / root-task 60 / boot-manager 5 全绿。
