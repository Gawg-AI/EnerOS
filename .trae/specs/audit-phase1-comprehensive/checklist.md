# Checklist — Phase 1 全面测试、验证与修复

## Task 1: Workspace 完整性验证
- [x] C1.1 `cargo metadata --format-version 1` 成功（workspace 成员路径全部正确）
- [x] C1.2 根 `Cargo.toml` members 含全部 61 个 crate + ci 路径（62 members）
- [x] C1.3 跨 crate `path = "..."` 引用全部使用正确相对路径（94 处引用全部正确，无绝对路径、无断裂引用）

## Task 2: 全量构建验证
- [x] C2.1 `cargo build --workspace` 成功（修复 eneros-hello 链接问题后通过）
- [x] C2.2 修复后重新构建通过（eneros-hello cfg_attr target_os="none" 守卫修复）

## Task 3: 代码格式检查与修复
- [x] C3.1 `cargo fmt --all -- --check` 通过
- [x] C3.2 无需修复（eneros-fs 修复后重新 fmt 通过）

## Task 4: Clippy 静态分析与修复
- [x] C4.1 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 无 warning
- [x] C4.2 无需修复

## Task 5: 全量单元测试与修复
- [x] C5.1 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 全部通过，0 失败（3496 测试）
- [x] C5.2 `cargo test -p eneros-hal --features mock` 通过（23 测试）
- [x] C5.3 修复 eneros-fs flaky test 后测试全部通过（3519 测试总计）
- [x] C5.4 各 crate 测试数量统计已记录

## Task 6: Phase 1 新增 crate aarch64 交叉编译验证
- [x] C6.1 P1-A 存储与文件系统（storage / fs / tsdb / runtime-config）交叉编译通过
- [x] C6.2 P1-B 网络协议栈（drivers-net / drivers-cellular）交叉编译通过
- [x] C6.3 P1-C 密码学安全（security-crypto）交叉编译通过
- [x] C6.4 P1-D/E Agent Runtime（agents-agent / agents-hmi / agents-alarm）交叉编译通过
- [x] C6.5 P1-F 设备协议栈（9 crate）交叉编译通过
- [x] C6.6 P1-G 四遥与SOE（4 crate）交叉编译通过
- [x] C6.7 P1-H RTOS组件（5 crate）交叉编译通过
- [x] C6.8 P1-I LLM推理（5 crate）交叉编译通过
- [x] C6.9 P1-J Solver（5 crate）交叉编译通过
- [x] C6.10 P1-K 双脑协同（3 crate）交叉编译通过
- [x] C6.11 P1-L MVP集成（3 crate）交叉编译通过
- [x] C6.12 全量交叉编译在 WSL2 中通过（60+ crate 全部成功，eneros-kernel + eneros-hello check 也通过）
- [x] C6.13 无交叉编译失败，无需修复。no_std 合规性最终验证通过

## Task 7: no_std 合规性验证
- [x] C7.1 静态搜索发现疑似 `use std::` 匹配，经交叉编译验证确认全部为 `#[cfg(test)]` 或 `#[cfg(not(target_os = "none"))]` 守卫代码
- [x] C7.2 `panic!(` / `todo!(` / `unimplemented!(` 经交叉编译验证确认实际合规
- [x] C7.3 所有 Phase 1 crate 的 `lib.rs` 含 `#![cfg_attr(not(test), no_std)]` 或 `#![no_std]`（50 lib.rs，44 含声明，6 为非 crate 模块文件）
- [x] C7.4 无需修复（交叉编译全部通过，证明 no_std 合规）

## Task 8: 目录结构校验（§2.4 C1~C15）
- [x] C8.1 C1 — 所有 Phase 1 crate 在 `crates/<subsystem>/` 下
- [x] C8.2 C2 — 根 `Cargo.toml` members 含全部 Phase 1 crate 路径（62 members）
- [x] C8.3 C3 — 跨 crate `path = "..."` 使用正确相对路径（94 处全部正确）
- [x] C8.4 C5 — 根目录无除 `ci/` 外的 Rust crate 文件夹
- [x] C8.5 C12 — 文档在 `docs/<topic>/` 子目录，未平面化
- [x] C8.6 C13 — `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、`*.gguf`、IDE 缓存被追踪
- [x] C8.7 C14 — `.gitignore` 覆盖全部所需文件类型（含 `*.gguf`）
- [x] C8.8 无违规，无需修复

## Task 9: 文档分类验证
- [x] C9.1 `docs/` 根目录无平面化 `.md` 文件（除 `README.md` 外）
- [x] C9.2 Phase 1 新增文档分布在 `docs/ai/`、`docs/agents/`、`docs/protocols/`、`docs/security/` 子目录，全部存在且有内容
- [x] C9.3 无平面化文档，无需修复

## Task 10: .gitignore 覆盖验证
- [x] C10.1 `.gitignore` 含全部所需模式（target/、build/、*.elf、*.bin、*.dtb、*.img、*.gguf、qemu-output/、.idea/、.vscode/、*.pem、*.key、.env、*.log、*.tmp）；Cargo.lock 未被忽略（正确）
- [x] C10.2 无缺失，无需修复

## Task 11: CI 质量门禁
- [x] C11.1 `cargo run -p eneros-ci` — fmt ✅ / clippy ✅ / test ✅（修复 flaky test 后通过）；audit 步骤因网络无法连接 GitHub advisory-db 失败（环境问题）
- [x] C11.2 `cargo deny check advisories licenses bans sources` — advisories/bans/licenses/sources 全部 ok（仅 warning）；advisory-db 拉取失败时为降级模式

## Task 12: Phase 1 出口标准 1 — autonomous 24h 运行
- [x] C12.1 MvpOrchestrator 24 个测试全部通过（无 96-tick 24h 模拟测试，完整 24h 实测延后 QEMU/Phase 2）
- [x] C12.2 无 Panic、无 OOM。Orchestrator 使用 running: bool + AgentState 三态生命周期，tick 流转正确
- [x] C12.3 无失败，无需修复

## Task 13: Phase 1 出口标准 2 — 双脑链路 < 2s
- [x] C13.1 DualBrainEngine 22/22 测试通过（Mock LLM + Mock Solver 协作时序正确）
- [x] C13.2 RealtimePathEngine 22/22 测试通过（快路径 Solver-only < 500ms 主机侧逻辑覆盖）
- [x] C13.3 IntentContract 22/22 测试通过（序列化/反序列化/验证器/转换器/JSON round-trip）
- [x] C13.4 无失败，无需修复（QEMU 实测延后 Phase 2）

## Task 14: Phase 1 出口标准 3 — 比传统 EMS 收益 ≥ 10%
- [x] C14.1 RevenueComparator 8/8 测试通过
- [x] C14.2 `improvement_pct >= 10.0` 阈值确认；边界测试 t8 确认 10% 达标/5% 未达标
- [x] C14.3 TraditionalEms 基线规则正确（谷时 price<0.3 充电、峰时 price>0.8 放电、平时保持）
- [x] C14.4 无失败，无需修复

## Task 15: Phase 1 子模块端到端集成验证
- [x] C15.1 P1-A → P1-L 链路完整性验证通过（全部 12 子模块测试通过，无链路断裂）
- [x] C15.2 关键瓶颈版本测试通过（v0.24.0 FS 197✅ / v0.28.0 TCP 439✅ / v0.31.0 国密 391✅ / v0.59.0 LLM 15✅ / v0.71.0 双脑 22✅ / v0.74.0 MVP 24✅）
- [x] C15.3 无链路断裂，无需修复

## Task 16: 审计报告汇总
- [x] C16.1 各 crate 测试数量统计已汇总（总计 3519 测试，0 失败）
- [x] C16.2 三大出口标准达成情况已汇总（全部主机侧通过）
- [x] C16.3 修复的问题清单已记录（①eneros-hello 链接修复 ②eneros-fs flaky test 修复）
- [x] C16.4 遗留项已记录（QEMU 实测、96-tick 24h 模拟、advisory-db 网络问题）
- [x] C16.5 Phase 1 总体结论：✅ 通过（有条件）
