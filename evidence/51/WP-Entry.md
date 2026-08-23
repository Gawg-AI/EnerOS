# 蓝图 51 卷 Entry 登记（锚点重验 + 前置检查）

> 日期：2026-08-23 · 基线版本：0.313.40 · 基线 commit：9d4144c（50 卷
> Exit Gate 收口提交，HEAD）

## 锚点重验（44-00 §4.5.1，逐条实跑，2026-08-23）

| 锚点 | 结果 |
|---|---|
| SKU 状态 | ✅ `product/compatibility-catalog.json`：3 SKU 全 `not-qualified`；`kunpeng-920` board_revision=`unknown`、`phytium-d2000`=`0.1.0-draft`，两者 `bsp_adapter="FdtDescriptionProvider + <pending-board-adapter>"`；status_reason=无 L4 证据 |
| 板级事实 | ✅ `product/skus/phytium-d2000.json`/`kunpeng-920.json`=占位骨架（schema `eneros.board-fact-freeze.v1`，多数 fact `verified:false`+source=`BLOCKED：缺硬件/BSP 文档`，missing 清单各 8–9 项）；`qemu-virt-aarch64.json` 有真实板级事实 |
| 真机熵源 | ✅ `crates/security/crypto/src/rng/entropy.rs:44-58`：`PhytiumTrng`/`KunpengTrng::fill_entropy` 恒 `Err(EntropyError::Unavailable)` fail-closed（不伪造读数）；`platform_entropy_source()` 按编译配置选择平台源 |
| 现有驱动面 | ✅ virtio（QEMU 专用：`drivers/virtio-core`、`net/src/virtio_net.rs`、`storage/driver/virtio_blk.rs`、`crypto/src/rng/virtio_rng.rs`）；PL011=`crates/hal/hal/src/arm64/uart_pl011.rs`（真寄存器未上硬件）、SJA1000=`crates/drivers/can`；emmc=`crates/drivers/storage/src/driver/emmc.rs`（837 行，含「RAM simulation backend (QEMU sd model equivalent)」:171-183）；nvme=`.../driver/nvme.rs` 恰 **299 行**未验证；watchdog SP805=`crates/drivers/watchdog/src/wdt.rs`；power FDT 绑定在 hal |
| CAN | ✅ `crates/protocols/gbt27930/src/plugin.rs:3-6,104,120`：「CAN 传输（SJA1000+TP）接线 =51 卷真机路径，**QEMU virt 无 CAN**」descriptor `transport_kinds=&[]`；L1 USB-CAN=owner hardware |

## 上卷回归（§4.5.2，≥2 项照抄实跑，2026-08-23 实测）

1. **closure gate**：`cargo xtask closure gate --profile qemu-prod` →
   `binaries=32 reached_packages=122 denied=0 crypto_violations=0 digest=bdd2332e… PASS`
   （与 50 卷 Exit 登记逐字一致）✅
2. **CATALOG-VALIDATE**：`cargo xtask catalog validate` → `CATALOG-VALIDATE PASS` ✅
3. **CF01 config-refs**：`cargo xtask qa gate config-refs` →
   `CONFIG-REFS-CF01 OK（active 18，templates 74，deleted 3）`（与 50 卷 Exit
   active=18 一致）✅

## 锚点修订记录

- 无偏差（五锚点与卷内登记一致，未触发 STOP）。

## 执行模式确认（卷首覆盖条款）

本卷全部交付物（判据表/评估表/模板/BLOCKED 登记表）均可由执行 AI 独立完
成，无停顿点。最终选板决策=owner 事项——评估表产出后以「待 owner 裁决」
登记并继续后续 WP 直至 Exit，不停等答复。真机执行项全部 BLOCKED（owner 动
作），不得用 QEMU/合成证据泛化；SKU 冻结=owner 签字。按 SOP §4.5.3 连续执
行 WP-51.1→51.2→51.3 → Exit Gate。

## 文件清单（§4.5.3，开工前登记）

- **51.1**：`product/skus/board-selection-criteria.md`(新)、
  `evidence/51/WP-Entry.md`(新)+`evidence/51/WP-51.1.md`(新)、bump。
- **51.2**：`product/skus/bring-up-template.md`(新)、
  `evidence/51/WP-51.2.md`(新)、bump。
- **51.3**：`evidence/51/blocked-register.json`(新)+`WP-51.3.md`(新)、bump。
- **Exit**：`evidence/51/exit-gate.{json,md}`(新)、44-00 §6 状态板 51 行、bump。
