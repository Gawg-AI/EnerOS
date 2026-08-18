# ADR-0001: Single product kernel mainline

> **Status: Superseded（2026-08-12，蓝图 36 WP-36.1）**：本文"自研
> `eneros-product-kernel` 唯一产品内核"的主线段已被 ADR-0043 取代；
> 唯一产品内核裁决见 `product/kernel-policy.toml` 与 ADR-0043。本文保留
> 原文作为历史设计来源（35-01 `HISTORICAL_ONLY`），不再拥有执行权。

- Status: Superseded (was Accepted for G0/G1)
- Decision: `eneros-product-kernel` is the only kernel binary in release, recovery, board and factory product profiles. `eneros-boot-manager-uefi` is the only product boot manager.

The historical `eneros-microkernel`, `eneros-bootloader`, seL4 runtime and root Makefile flows remain research inputs only. They are Not Integrated, cannot be selected by a release-class profile, and cannot produce an accepted artifact. `cargo xtask audit profile` enforces that boundary from the resolved Cargo closure.

The formal media path is UEFI firmware → GPT `PLATFORM_BOOT` → signed boot manager → GPT `BOOTCTRL` → signed slot manifest/components → `eneros-product-kernel`. QEMU `-kernel`, `-loader`, fixed-entry jumps and the legacy runtime are prohibited.

Rollback: if a second product kernel or boot path is needed, it requires a new ADR, distinct profile/platform identity, closure policy and complete Gate evidence. It cannot silently reuse this profile.
