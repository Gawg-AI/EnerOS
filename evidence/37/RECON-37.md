# RECON-37 — 蓝图 37 侦察报告（正式装配运行时与信任基座）

> schema: `eneros.recon-37.v1`
> 前置：36 Exit Gate 10/10 PASS（`evidence/36/36-验证记录与Exit-Gate核销.md`，含 Reasonix 独立复核轮 §8 与 F6 修复）
> 审查基线：`HEAD=473b1e26681be33acc654f32e88251d8c19ad3cf` + 全部未提交/未跟踪开发代码
> 编制：2026-08-13（蓝图 37 执行，Entry 侦察）
> 37 唯一目标：让 manifest 中的 required 服务在真实 seL4 runtime 中以正确内存权限、加载语义、能力和可信审计启动

## 1. 生产路径侦察（root-task）

| 文件 | 行数 | 关键生产路径 | 37 相关缺口 |
|---|---|---|---|
| `crates/runtime/root-task/src/main.rs` | 428 | `rust_entry`：boot_init → allocator init → panic audit init → time init → bundle parse_verified（SHA-256）→ `plan_spawn` → WDT map → `sel4_spawn_services` → IRQ bind → `run_ipc_loop`；任一步失败 `loop { spin_loop() }` fail-closed | 主路径调用 **无 enforcement 的 `plan_spawn`**（RT-06）；bundle 固定读 `ENEROS_INIT_BUNDLE_PHYS` 常量而非消费 `BootInfo.init_bundle_phys`（RT/P1-06，WP-37.6） |
| `crates/runtime/root-task/src/bundle.rs` | 303 | `InitBundle::parse` / `parse_verified`（模块 sha256 校验，30 RT-004）；ENRINIT2 v2 wire：magic/version/header_len/total_len/source_date_epoch/module_count/payload_sha256 + 96B×n 模块表（kind/flags/offset/len/name/sha256） | flags 无版本化定义（compose 侧把 criticality 塞进 flags 位，runtime 不消费）；未知 flag 不失败；无 manifest required/optional 语义（WP-37.1） |
| `crates/runtime/root-task/src/spawn.rs` | 1885 | `plan_spawn`（root+服务，≥2 校验）；`sel4_spawn_services` 两阶段 spawn（对象创建→统一 resume）；`configure_service`（TCB/VSpace/CNode/ASID/页表×3/ELF 段页/栈/IPC buffer/ep/fault ep） | **坏 ELF `continue`**（P0/RT-05，WP-37.1 #6）；`page_rights` 丢弃 exec 位，`arm_page_map` 恒 Default_VMAttributes → 无 NX（P0-03，WP-37.3）；ET_DYN relocation 从不调用（RT-04，WP-37.4）；`plan_spawn_with_enforcement` 存在但主路径不用（RT-06，WP-37.2） |
| `crates/runtime/root-task/src/service.rs` | 427 | `run_ipc_loop`（fault poll → supervisor 决策 → restart_service） | readiness 来自"spawn 返回成功"而非 IPC handshake/health probe（WP-37.1 #8） |
| `crates/runtime/root-task/src/supervisor.rs` | 294 | `ServiceSupervisor`：6 级状态机（Running/Suspended/Faulted）+ 指数退避 1s…60s + 5 次/小时熔断 + 窗口滑动 + recover | 可复用为 37 supervisor 底座；缺 requirement/criticality/restart policy 数据驱动（WP-37.1） |
| `crates/runtime/root-task/src/panic.rs` | 232 | `panic_reboot`：审计（heapless 定容）→ 重启计数 → SP805 武装 10ms 或软件模式 spin；`init_panic_audit`（MemStore + genesis=SHA-256 真实摘要） | `Mutex<Option<AuditLoggerImpl<MemStore>>>` 阻塞锁 + MemStore 内存链——复位后证据丢失、OOM/重入可能到不了 watchdog（P1-04，WP-37.9） |
| `crates/runtime/root-task/src/boot.rs` / `allocator.rs` / `irq*.rs` / `driver_probe.rs` | — | BootInfo 消费、堆、IRQ 绑定、驱动探测 | driver_probe 属 38（禁止越界） |

## 2. seL4 safe/ffi（sel4-sys）

| 位置 | 事实 | 37 处置 |
|---|---|---|
| `safe.rs:570 arm_page_map` | **恒用 `eneros_sel4_arm_default_attributes()`（seL4_ARM_Default_VMAttributes）**，不接收执行位；aarch64 默认属性不含 ExecuteNever | WP-37.3：新增类型化 `VmPermissions`，`arm_page_map` 增加 attr 参数并换算 XN（ExecuteNever bit）；禁止 caller 再走 default attr 路径 |
| `safe.rs:604 arm_page_table_map` | 同 default attr 收口 | 页表映射属性保持 default（数据语义），但函数签名按 VmPermissions 收口 |
| `safe.rs:629 page_rights`（root-task/spawn.rs） | 注释自认"执行映射由默认 VMAttributes 保证"——exec 位在底层丢失 | 由 VmPermissions 取代权限计算路径 |
| `ffi.rs` / C shim | `eneros_sel4_arm_default_attributes` → `seL4_ARM_Default_VMAttributes`（third_party/sel4-root/root_task.c + safe wrapper） | 新增 shim：`eneros_sel4_arm_mk_execute_never_attributes()` 或 attr 位组合（ExecuteNever=bit，AArch64 seL4 VMAttributes） |
| host cfg | `arm_page_map` host 构建返回 `Sel4Error::HostUnsupported`（红线 2 已守） | 保持：host 只测类型/换算逻辑，不伪造成功 |
| `types.rs` / `lib.rs` | CPtr/Word/RightsTable 已存在 | WP-37.5 类型化 CPtr 大改造属后续 WP，本卷不越界 |

## 3. ELF loader（crates/abi/elf-loader）

- 支持 ET_EXEC（`e_type==2`）与 ET_DYN（`e_type==3`）；`validate_segments`（重叠/回绕）、`parse_dynamic`（DynamicInfo）、`apply_relocations`（越界/未知类型拒绝，golden vector 测试）均已实现（RT-02/RT-04 组件层）。
- **产品缺口**：root `plan_spawn`/`configure_service` 只走 `LoadedElf::parse` + 固定 vaddr 映射，从不调用 `parse_dynamic`/`apply_relocations`；当前 release 服务均为链接到固定地址的 ET_EXEC（SVC_VSPACE_BASE=0x200000 语义），PIE 静态支持未闭合（WP-37.4，需 linker contract 冻结 + target 运行证据）。
- 本卷范围：WP-37.4 完整实现依赖 target 验证（两个 load bias 运行证据），登记 PENDING；不动 loader 现有组件。

## 4. 审计与 DurableLog

- `crates/security/audit`：`AuditEvent`/`AuditEventType`/`ChainEntry`(112B)/`AuditLoggerImpl`/`MemStore`；链只持久化哈希摘要不持久化事件（P1-03，WP-37.8）；36 已把 SM3→SHA-256（product_allowed=false 移除）。
- `crates/runtime/durable-log`：基本块日志 + 恢复 + MockBlockDevice；探测阶段 format 风险属 39（禁止越界）。
- 本卷范围：WP-37.8（DurableAuditRecord 完整事件持久化 + 外部可信锚）登记 PENDING（需 39 存储层接线），37 内只保留既有链。

## 5. Boot manager → BootInfo/init bundle ABI

- `crates/boot/boot-protocol/src/boot_info.rs`：`BootInfo`（slot/reason/init_bundle_phys 等固定 64B 布局，offset 56 = init_bundle_phys）；`ENEROS_INIT_BUNDLE_PHYS=0x4200_0000`；`ENEROS_INIT_BUNDLE_MAX_LEN`（内核映射窗口 2MiB）。
- root `read_bundle_from_phys` 固定用常量地址（main.rs:57 `INIT_BUNDLE_PHYS`），BootInfo 的 `init_bundle_phys` 字段在 target 路径未消费（WP-37.6；注释登记"需 elfloader 传参通道"）。
- 本卷范围：WP-37.6 登记 PENDING（依赖 elfloader/第三方 ABI 通道，QEMU 全链路验证环境 BLOCKED）。

## 6. 唯一信任链（WP-37.7）

- 36 已产出 `product/crypto-policy.toml`（7 用途细分 + product_allowed + allowed_targets/features）+ closure gate（UNREGISTERED_CRYPTO_IN_CLOSURE 闭世界）+ ADR-0043（product_kernel=sel4）。
- 37 剩余：boot manager 只走单一 `BootSignatureProvider`（Ed25519 经批准 provider），UEFI 路径与 eneros-secure-boot 去双轨，test/dev key 编译进 release 失败——依赖 UEFI 真机/固件侧验证，登记 PENDING（secure-boot 链验证环境 BLOCKED）。
- 已确认 36 的 closure gate 拒绝未授权 crypto 进入正式闭包（crypto_violations=0），信任链的构建侧已收口。

## 7. 环境 BLOCKED_EVIDENCE

- QEMU seL4 全链路（NX 页表 trace、PIE 运行、fault oracle）：`SEL4_SRC` 未设置、本机无完整 seL4 重建链证据 → BLOCKED（与 RECON-36 §9 一致）。
- Windows 跨平台：本机 Linux → BLOCKED（42 portability）。
- 真机/HIL：无 → PENDING。

## 8. 本卷 WP 范围裁决

| WP | 内容 | 本卷状态 |
|---|---|---|
| WP-37.1 | 严格 init bundle contract（required 集合、坏 required ELF 必阻断、flags 版本化、依赖环） | **实施**（host 可验证核心：plan_spawn required 语义 + bundle flags 校验） |
| WP-37.2 | 租户/能力强制接入主路径 | 部分（enforcement 已有组件；主路径接线依赖 37.1 的 plan 结构改造，随 37.1 一并接线） |
| WP-37.3 | VmPermissions 类型化映射 + NX | **实施**（类型 + 换算 + arm_page_map 签名 + host 单测；target NX trace 证据 BLOCKED） |
| WP-37.4 | ET_DYN/PIE 产品接线 | PENDING（需 target 运行证据 + linker contract 冻结） |
| WP-37.5 | safe FFI 类型化 CPtr | PENDING（大改造，独立 WP） |
| WP-37.6 | BootInfo ABI 完整消费 | PENDING（依赖 elfloader 通道） |
| WP-37.7 | 唯一信任链落地 | PENDING（依赖 UEFI 侧验证环境） |
| WP-37.8 | 持久化完整 AuditEvent | PENDING（依赖 39 存储层） |
| WP-37.9 | panic 无分配路径 | PENDING（独立 WP；现路径已 heapless 定容 + WDT 检查） |

## 9. 保护策略

- 全部改动保持未提交；不执行 reset/clean/checkout/自动 commit。
- 37 owner 文件：`root-task/src/{main,bundle,spawn,service,panic,supervisor}.rs`、`sel4-sys/**`、`elf-loader/**`（仅回归）、audit 模型部分。
- 禁止越界：driver_probe（38）、storage/net/protocol（39）、Agent/AI（40）、业务（41）、证据工具（42）。
- 修改前记录 `git status --short` 与目标文件 diff（§10）。

---

## 11. 环境判定修正（2026-08-13 第六轮实跑后）

§7 的"`SEL4_SRC` 未设置、本机无完整 seL4 重建链证据 → BLOCKED"**判定错误**：

- 完整重建链全部在 `$HOME/.cache`：`xpack-aarch64-none-elf-gcc-15.2.1-1.1`、
  `cmake-3.31.6-linux-x86_64`、`ninja-bin`、`dtc-deb`、`xmllint-bin`、
  `sel4-src`（含补丁）、`seL4_tools-master`、`util_libs-master`、`pyfdt-0.3`、
  `libxml2-deb`/`libicu74-deb`；QEMU 在 `~/.cache/xpack-qemu-arm-9.2.4-1/bin/`
  （系统未装，需 PATH 注入或 `ENEROS_QEMU_BIN`）。
- 重建流程（既有，见 memory sel4-rebuild-procedure）：root-task target ELF
  → `ninja -C third_party/sel4-build/top-rust`（4 步重链）→ 复制 elfloader
  回 `target/sel4/` → `SOURCE_DATE_EPOCH=… cargo xtask image qemu-prod`
  → 全零 VARS + `--reset-bootctrl` 跑 QEMU（exit=124 为预期，日志即序列门）。
- 第六轮实测 6 次 QEMU 全链运行（run-1/2/3/4/6/final），修复 target-only
  缺陷 T1–T5（见 37 验证记录 §14.1），最终证据绑定 `a92d7277…`。
- **后续版本（38–43）的 target 验证不得再以"无重建链"为由登记 BLOCKED**。
