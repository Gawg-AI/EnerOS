# ADR-0030：唯一内核路线执行裁决 = seL4（KRN-001）

> 状态：Accepted（2026-08-05，S2 KRN-001）
> 决策者：产品负责人（延续 ADR-0023 用户决策）+ 主开发 AI 执行
> 前置：ADR-0023（Accepted，2026-08-02）「seL4 微内核作为 EnerOS 内核基座」
> KRN-001（08 蓝图阶段二第 1 项）：以正式 ADR 裁决唯一内核路线，代码/镜像工厂/README/CI 对齐，落选一侧 crate 移出 workspace members。

## 1. 结论

**唯一内核路线 = seL4**。确认 ADR-0023 裁决并转入执行态；自研 microkernel（`crates/kernel/microkernel`）已删除且无 workspace crate 残留——落选一侧无需移出（G2 删除已执行，见 ADR-0023 §1 与 `xtask/src/image_factory.rs` 注释）。

## 2. KRN-001 三要素依据（08 蓝图规定）

| 要素 | 依据 |
|---|---|
| boot 域 BootInfo 契约 | `boot-protocol` 的 `BootInfo`（magic/abi_major/dt/boot_id/init_bundle_phys…）由 boot-manager-uefi 构造；**seL4 elfloader 不解析 BootInfo**——契约消费方 = EnerOS root-task（`crates/runtime/root-task`），S2 KRN-002 落地 root-task 读取 BootInfo 校验摘要 |
| agents/agent 的 ELF loader 复用度 | seL4 elfloader 使用 libcpio+elfloader 装载 rootserver；EnerOS **用户态 EL0 服务装载**复用 `agent/src/elf.rs`（`LoadedElf::parse` + SegmentFlags W^X 检查），由 root-task spawn 路径接入（KRN-002/003）——ELF 解析与安全检查复用量最大化 |
| third_party/sel4 构建链可维护性 | `third_party/sel4-build.sh`（内核+libsel4+elfloader+root task 一键构建，qemu-arm-virt 参数已验证）；产物 `third_party/sel4-build/top/{kernel.elf,elfloader,images/*}` 存在；xtask `sel4:` 前缀引用 `target/sel4/`（KRN-006 对齐）。缓存工具链（cmake/gcc-xpack/sel4-src）缺失属环境依赖，不影响路线裁决 |

## 3. 对齐要求（KRN-001 执行项）

1. 代码：`crates/runtime/{hello,root-task,sel4-sys,services}` 归属 seL4 路线保留；无自研内核 crate。
2. 镜像工厂：`xtask` 的 `sel4:` selector 是唯一内核/引导镜像来源（`image_factory.rs:1660` 注释与 KRN-006 对齐路径）。
3. README/CI：产品内核表述统一为 seL4（08 偏差：README 曾含"自研内核地基"历史表述，S2 修正为 seL4 唯一路线）。
4. BootInfo 链：root-task 消费 BootInfo（KRN-002）——消除 08 §2「启动链断头路」：验签链尾接物不再是真空。

## 4. 反对方与裁决记录

- 反方：自研 Rust microkernel（定制性最强）——G2 QEMU 验证系统性缺陷（SVC ELR 语义、TTBR0 切换缺失、kstack 预映射、GICv3 配置、异常帧交互），修复等于重写；无 QEMU 系统证据（ADR-0023 §1）。维持淘汰。
- 裁决：seL4 是唯一满足形式化验证 + capability 模型 + AArch64 QEMU 官方支持 + 工业部署的候选（ADR-0023 §2 调研表）。沿用。

## 5. 后果

- root-task 成为 BootInfo 消费方与 EL0 服务 spawner（KRN-002/003/004）；
- `target/sel4` 产物路径与 xtask `sel4:` selector 对齐（KRN-006）；
- EL0 服务装载复用 `agent/src/elf.rs`（W^X），IPC 复用 `abi/ipc-wire` 帧格式（KRN-002）。
