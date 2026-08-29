# WP-A.3 生产桩三件收编（SPEC-SERIES-2026-08-27/A）

## 判据结果

| 判据 | 结果 | 留证 |
|------|------|------|
| C1 kunpeng 模拟态如实自报 / phytium release 符号不存在 | ✅ | kunpeng：`name()="Kunpeng 920 KAE (simulated)"`、`is_hardware()=false`、`is_available()=false`（单测 `test_kunpeng_simulated_honest_self_report`）；phytium：rustc 探针编译失败 `E0599: no associated function new_simulated`（default features rlib），硬件 `new()` + `create_selector_with_hw` 正控编译通过；`#[cfg(any(test, feature="simulated"))]` 三处（new_simulated / create_simulated_selector / lib.rs re-export）+ Cargo feature 声明 |
| C2 digest 真值落盘确定性 / 空 schema 拒注册 | ✅ | `wp_a3_register_digest_is_real_and_deterministic` + `wp_a3_replay_heals_zero_placeholder_and_guards_empty_schema` + `capability_digest` 单测；回放自愈（register_with_registration 按四元组重算，旧全零快照自动换真值，行格式不变）；CAPABILITY_DIGEST_PLACEHOLDER 常量删除（零消费者） |
| C3 QEMU 冒烟零 PANIC / clippy 四包零警 | ✅ | gate-run 与 neg-nokey 两次窗口 PANIC=0；crypto/device-agent/control-services/control-dispatch clippy --all-targets 零警告 |

## 装配要点

- kunpeng 收编走"如实自报"（不删类，驱动预留位；选择器注册门据 is_available=false 不误收模拟引擎）。
- phytium 收编走"编译期不可达"（cfg 门控 + feature 声明，防幽灵）；集成测试模拟引擎用例迁入 `src/hw/tests_sim.rs`（门控可见性），软件-only 用例留原文件——计数不降。
- digest 单源：`device-agent/src/capability_digest.rs`（SM3(endpoint‖0‖kind‖schema_le32‖ops_bits)）；`register()`/`register_with_registration()`/`update()` 全按四元组重算；空点映射 → `RegistryError::EmptyPointMap` 拒注册（约 12 处调用点适配）。
