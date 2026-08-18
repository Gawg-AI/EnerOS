# 01 运行时/启动/HAL/ABI 域 — 商业交付级代码级审查报告

- 审查日期：2026-08-11
- 审查域：`crates/runtime/`（root-task、sel4-sys 含 shim/sel4_shim.c、services、runtime、hello、oom-probe）、`crates/boot/`（boot-manager-uefi、boot-protocol 等 4 个）、`crates/hal/`（hal、board）、`crates/abi/`（elf-loader、ipc-wire）；第三方接缝仅审我方调用与配置（sel4 FFI/shim、AAVMF 交互）
- 审查方式：逐文件代码级深读（全部源文件完整读毕）+ 关键符号 grep 二次验证 + 行号锁定
- 前置依据：蓝图/27-窗户纸计划（§4 S 条目、WP-27.2~27.9）、evidence/driver-binding/、evidence/boot-unblock/、FIX-BUGS-0810 两件套语义

---

## 一、总览表

**统计：共 31 条发现 — P0：0；P1：11；P2：20**

| ID | 严重度 | 类别 | 定位 | 一句话 |
|---|---|---|---|---|
| RT-001 | P1 | BUG | root-task/src/panic.rs:62-79 | `panic_reboot` 不检查 `WDT_MMIO_MAPPED`，软件模式下 panic 路径写未映射 MMIO → 二次 fault |
| RT-002 | P1 | 硬件耦合 | root-task/src/main.rs:54,141 | `INIT_BUNDLE_PHYS=0x4200_0000` 硬编码，从不消费 BootInfo.init_bundle_phys（boot-manager 已写入） |
| RT-003 | P1 | BUG | root-task/src/main.rs:303,307-325 | `MAX_BUNDLE_LEN=64MiB` 远超内核 2MiB 固定映射窗口，越窗 `copy_nonoverlapping` → VM fault |
| RT-004 | P1 | 缺失功能 | root-task/src/bundle.rs:parse | init bundle 模块 sha256 字段存在但解析器从不校验（供应链完整性缺失，测试注释自认） |
| RT-005 | P1 | BUG | abi/elf-loader/src/lib.rs:278 | 入口校验 `e_entry < s.vaddr + s.memsz` 未 checked_add，u64 回绕绕过校验 |
| RT-006 | P1 | BUG | root-task/src/spawn.rs:136-188 | `plan_one` 无 PT_LOAD 段 vaddr 区间重叠检查，非单调段 → 页重叠/栈碰撞 |
| RT-007 | P1 | BUG | root-task/src/spawn.rs:1175 | `debug_assert!(len <= PAGE_SIZE)` 在 release 无效 → 越页写 WINDOW_BASE 窗口 |
| RT-008 | P1 | 硬件耦合 | root-task/src/main.rs:258 | 虚拟 timer IRQ 27 硬编码 `irq_table.bind(&mut ctx, 27)`，未走 DTB/绑定表注入 |
| RT-009 | P1 | BUG | root-task/src/driver_probe.rs:499-500 | IRQ 接线 `bound.zip(bindings)` 按位置耦合可错位截断；`*badge - 1` badge=0 时 u32 回绕 |
| RT-010 | P1 | BUG | boot/boot-manager-uefi/src/lib.rs:16-17,200 | INIT_LOAD 区间浮动（0x4200_0000~0x4280_0000）与内核固定 2MiB 窗口/root-task 硬编码读冲突 |
| RT-011 | P1 | 硬件耦合 | hal/hal/src/arm64/gicv3.rs:256 | `ARM64_GIC` 单例硬编码 QEMU GICD/GICR 基址，绕过 PlatformConfig/init_hal 注入接缝 |
| RT-012 | P2 | BUG | runtime/sel4-sys/src/types.rs:119-120 | `REG_X30=33` 与 `REG_NZCV=33` 碰撞（当前无消费方，潜在） |
| RT-013 | P2 | BUG | runtime/sel4-sys/src/lib.rs:109-116 | host stub `message_info_new` 编码 `(len&0xFF)<<8` 与 types.rs bits 0-7 不一致 |
| RT-014 | P2 | BUG | runtime/sel4-sys/src/safe.rs:1090-1095 | `build_test_bootinfo_blob` 写 `off+16` 而 reader 读 `off+9`（测试辅助错位） |
| RT-015 | P2 | BUG | runtime/sel4-sys/shim/sel4_shim.h:242-249 | IRQ 函数声明位于 header guard `#endif` 之外 |
| RT-016 | P2 | BUG | hal/hal/src/arm64/gicv3.rs:54 | `static mut IRQ_HANDLERS` 无访问守卫（依赖单核前提，缺文档不变量） |
| RT-017 | P2 | 硬件耦合 | root-task/src/driver_probe.rs:335-343 | `frame_size_bits` 对 ≤2MiB 设备一律 21-bit 帧，4KiB 设备基址恰 2M 对齐时过度映射 |
| RT-018 | P2 | BUG | root-task/src/driver_probe.rs:181,205 | virtio 探测写 `STATUS=ACKNOWLEDGE` 后非 blk 分支返回 NotFound，设备状态已污染 |
| RT-019 | P2 | BUG | root-task/src/driver_probe.rs:86 | `static mut DMA_POOL_BYTES`（已有单线程论证，建议内聚封装） |
| RT-020 | P2 | BUG | abi/ipc-wire mr.rs:27 vs el0.rs:91 | MR 帧上限 952B < el0 `DEFAULT_MAX_PAYLOAD_LEN=1024`（满载荷必然 FrameTooLong） |
| RT-021 | P2 | 缺失功能 | boot/boot-manager-uefi/src/main.rs:510 | `init_bundle_phys` 已写入 BootInfo 但 root-task 从不消费（与 RT-002 同根因） |
| RT-022 | P2 | BUG | boot/boot-protocol/src/boot_info.rs:20 | `MAX_INIT_BUNDLE_LEN=256MiB` vs root-task 64MiB vs 内核窗口 2MiB 三重不一致 |
| RT-023 | P2 | 缺失功能 | boot/boot-manager-uefi/build.rs:48-71,100-122 | GUID 注入块重复（死代码） |
| RT-024 | P2 | BUG | hal/hal/src/arm64/gicv3.rs:137 | GICR 就绪 `.expect()` 在 boot 路径 panic（应有界重试 + fail-closed 返回） |
| RT-025 | P2 | 硬件耦合 | hal/hal/src/arm64/uart_pl011.rs:147；net_mmio.rs:126 | UART/NET 单例硬编码基址；net_mmio 用 0x09010000 与自身注释（HAL-004：该地址是 PL031 RTC）自相矛盾 |
| RT-026 | P2 | BUG | hal/board/src/mini_uart.rs:63 | TX 忙等 `while ... {}` 无界（HAL-005 有界忙等未覆盖 board crate；无消费方） |
| RT-027 | P2 | BUG | hal/hal/src/lib.rs:169,192 | `static mut HAL` + `expect`（write-once 语义可接受，建议 OnceLock 化） |
| RT-028 | P2 | 缺失功能 | root-task/src/panic.rs:35-38；services/src/panic.rs:25-28 | `PANIC_GENESIS` 注释称 SM3 哈希实为 ASCII 占位（两处同源模式）；services panic 路径持 spin Mutex 潜在死锁 |
| RT-029 | P2 | 缺失功能 | root-task/src/supervisor.rs:160-167 | `new_for_count` 用占位名 `svc-<i>`（run_ipc_loop 实际调用它，故障日志无真实服务名） |
| RT-030 | P2 | 缺失功能 | root-task/src/allocator.rs:7,16 | 模块文档默认堆 512 KiB 与实现 1 MiB 不一致（文档漂移） |
| RT-031 | P2 | BUG | root-task/src/spawn.rs:1144-1157 | `retype_objects` 签名带 `types` 参数但固定 `types.arm_page_table`（潜在误导） |

---

## 二、详情（root-task 域）

### RT-001 【P1 / BUG】panic_reboot 不检查 WDT_MMIO_MAPPED → 软件模式 panic 二次 fault

- **定位**：`crates/runtime/root-task/src/panic.rs:62-79`
- **代码片段**：`panic_reboot` 中直接 `HwWatchdog::new(WDT_BASE)` 并写 SP805 寄存器序列，不先读 `WDT_MMIO_MAPPED`。
- **根因**：main.rs L225-245 中 `map_device_mmio(WDT_BASE,12)` 失败时走软件模式（不 store true），service.rs L94-98 正确地以 `WDT_MMIO_MAPPED` 选择 `wdt_base`（失败则 0，不写 MMIO）；但 panic_reboot 遗漏同一守卫。QEMU xpack 无 SP805 节点时 WDT 永远软件模式 → 任何 root-task panic 都在 panic handler 内访问未映射 0x0905_0000 → 二次 VM fault，审计链/reboot 计数后续步骤全部不可达。
- **修复方案**：
  ```rust
  fn panic_reboot(info: &PanicInfo) -> ! {
      // ... 审计 + 计数 ...
      if WDT_MMIO_MAPPED.load(Ordering::Relaxed) {
          let mut wdt = HwWatchdog::new(WDT_BASE);
          wdt.arm_short(); // 既有序列不变
      }
      // 软件模式兜底：打印后 spin（与 ipc_fail_closed 同语义）
      loop { core::hint::spin_loop(); }
  }
  ```
- **二次验证**：grep `WDT_MMIO_MAPPED` 全部消费方 = main.rs（store）、service.rs（load 正确）、panic.rs（本次修复新增 load）。`HwWatchdog` 无其他构造点。不变量「panic 路径零新增 fault 面」保持。次生风险：软件模式下 panic 不再自动复位 → 依赖外部看门狗/人工复位；规避：打印明确 `WDT unmapped — spin, awaiting external reset` 判据行。
- **QEMU 验证**：构建注入 `svc-fault` Panic 故障 → 观察 root-task 串口输出 `PANIC #` 审计行后**无新增 vm fault 行**；软件模式下出现 spin 判据行。命令：`cargo run -p eneros-xtask -- image qemu`（xtask 实际子命令以仓库 README 为准，下同）→ QEMU virt 串口判据：无 `vm fault on data at 0x905xxxx`。
- **与 27 冲突**：同域 WP-27.3 D6（SP805 迁入绑定表/软件模式语义）——D6 明确「dts 未导出节点 → 软件模式不变」，本发现要求软件模式下 panic 路径也守约；实施 D6 时应一并修复，不冲突但需同步。

### RT-002 【P1 / 硬件耦合】INIT_BUNDLE_PHYS 硬编码，BootInfo.init_bundle_phys 写入后无消费

- **定位**：`crates/runtime/root-task/src/main.rs:54`（`const INIT_BUNDLE_PHYS: u64 = 0x4200_0000;`）、L141 使用点
- **根因**：boot-manager-uefi/src/main.rs:510 已写 `info.init_bundle_phys = init.load_address`，root-task 的 `boot::consume_boot_info` 有完整 decode+validate+测试（boot.rs），但 target 路径从不把 BootInfo 传入 rust_entry、也不消费 init_bundle_phys。契约链「写了但没人读」= 启动域假闭合。
- **修复方案**：
  ```rust
  // main.rs：rust_entry 增加 bootinfo 物理参数（x1，boot-manager/elfloader 传参或固定约定地址）
  fn rust_entry(bi: u64, eneros_bi_phys: u64) -> ! { /* ... */ }
  // 读取 256B BootInfo 区（需与 BOOT-007 窗口方案对齐），consume_boot_info 校验后：
  let init_phys = parsed.init_bundle_phys; // 替代硬编码常量
  ```
  前置依赖：BootInfo 自身物理地址传递机制与内核映射窗口（见 RT-003/RT-010）统一设计；在此之前至少在 main.rs 打印 `boot-protocol ENEROS_INIT_BUNDLE_PHYS` 常量一致性断言，消除三处魔法数。
- **二次验证**：grep `INIT_BUNDLE_PHYS`：main.rs 定义+4 处使用；grep `init_bundle_phys`：boot.rs 测试、host main() demo（L339 打印后丢弃）。修复波及面仅 rust_entry 签名与其调用方（global_asm `b rust_entry`——需同步改汇编传参）。次生风险：elfloader 传参约定与 seL4 capRegister 冲突 → 规避：沿用 seL4 既有约定（BootInfo 区 1:1 映射方案，BOOT-007 模式）。
- **QEMU 验证**：`cargo build -p eneros-root-task --target aarch64-unknown-none -Z build-std=core,alloc`；QEMU 判据：串口出现 `init_bundle @ paddr=0x...` 且该值来自 BootInfo 解码行（新增打印）而非编译期常量。
- **与 27 冲突**：WP-27.1（QEMU 引导链解锁）同域——该 WP 的解锁方案固化了 0x4200_0000 窗口（BOOT-007），本发现要求窗口与 BootInfo 联动，实施顺序上须在 27.1 验收后做，不得回退 27.1 的窗口证据。

### RT-003 【P1 / BUG】MAX_BUNDLE_LEN 64MiB 超出内核 2MiB 映射窗口 → 越窗读 VM fault

- **定位**：`crates/runtime/root-task/src/main.rs:303`（`const MAX_BUNDLE_LEN: usize = 64 * 1024 * 1024;`）、`read_bundle_from_phys` L307-325
- **根因**：内核 boot.c 只映射 [0x4200_0000, 0x4220_0000) 共 2MiB（BOOT-007）。header 校验 `total <= 64MiB` 放行后，L322 `copy_nonoverlapping(header_ptr, buf, total)` 在 total > 2MiB 时读越窗物理地址 → data abort。当前 bundle ~133KiB 未触发，属潜伏契约违背。
- **修复方案**：
  ```rust
  const BUNDLE_WINDOW_LEN: usize = 0x20_0000; // = 内核窗口，与 boot-protocol 共享常量
  // read_bundle_from_phys：
  if total < 64 || total > BUNDLE_WINDOW_LEN { return None; } // 同时保留 OOM 防御上限
  ```
  并在 `eneros-boot-protocol` 导出 `ENEROS_INIT_BUNDLE_WINDOW_LEN` 单一事实源，main.rs/dtb_window_for/boot-manager 三方引用。
- **二次验证**：grep `MAX_BUNDLE_LEN` 仅 main.rs 内部；`dtb_window_for`（spawn.rs）同样基于 0x4200_0000+bundle 末端推导 DTB 窗口——bundle 超窗时 DTB 窗口同样失效，两处修复共享同一常量即可一致。不变量「所有 bundle 访问 ⊆ 内核映射窗口」由类型层常量保证。次生风险：未来 bundle > 2MiB 被硬拒 → 需与 WP-27.2/27.3 的驱动固件打包需求对齐（若确需更大 bundle，须先扩内核窗口并同步 BOOT-007 证据）。
- **QEMU 验证**：host 侧 `cargo test -p eneros-root-task` 新增单测：构造 total=3MiB header → `read_bundle_from_phys` 返回 None（可抽出纯函数部分 host 测）；QEMU 判据：正常 bundle 启动行无变化（回归）。
- **与 27 冲突**：WP-27.1 同域（窗口尺寸是 27.1 既定事实），修复为收紧而非扩窗，不冲突。

### RT-004 【P1 / 缺失功能】init bundle sha256 占位从不校验（供应链完整性缺口）

- **定位**：`crates/runtime/root-task/src/bundle.rs`（`InitBundle::parse`）；测试 L202 注释自认「sha256 占位（全零；解析器不校验摘要）」
- **根因**：v2 模块条目 96B 含 sha256 字段，打包侧与解析侧均未启用；商业交付的镜像完整性依赖此字段。
- **修复方案**：
  ```rust
  impl InitBundle {
      pub fn parse(raw: &[u8]) -> Result<Self, BundleError> { /* 现有校验 */ }
      pub fn parse_verified(raw: &[u8], expect_sha256: fn(&[u8]) -> [u8; 32])
          -> Result<Self, BundleError> {
          let b = Self::parse(raw)?;
          for m in &b.modules {
              if expect_sha256(m.blob) != m.sha256 { return Err(BundleError::DigestMismatch); }
          }
          Ok(b)
      }
  }
  ```
  哈希实现经 trait 注入（target 用现有 SM3/SHA 资产，host 用同一实现单测），避免 bundle.rs 依赖具体 crypto crate。打包侧（xtask）同步写入真实摘要。
- **二次验证**：grep `sha256|payload_sha256` bundle.rs：字段定义 + 测试占位；grep `InitBundle::parse` 消费方仅 main.rs L143 → 替换为 `parse_verified` 波及面 1 处。不变量「fail-closed：摘要不符即挂起」与 main.rs 现有 parse 失败分支语义一致。次生风险：存量 bundle 全零摘要被拒 → 打包流水线必须同 PR 更新（xtask image 写真实摘要），否则 QEMU 启动回归；规避：先落 host 单测再切换 target 路径。
- **QEMU 验证**：host `cargo test -p eneros-root-task bundle`（篡改 1 字节 → DigestMismatch）；QEMU 判据：新 bundle 启动正常；手工篡改 bundle 字节后串口出现 `parse FAILED: DigestMismatch` 挂起。
- **与 27 冲突**：WP-27.9（M2 验收）/蓝图 27 §17 物理证据判据均隐含镜像完整性；S7（elf-loader 验签对象=文件字节）同精神。不冲突，但修复须与 WP-27.7（elf-loader 重定位/验签）共用哈希资产，禁止各写一套。

### RT-005 【P1 / BUG】elf-loader 入口校验加法回绕

- **定位**：`crates/abi/elf-loader/src/lib.rs:278`
- **代码片段**：`if e_entry < s.vaddr + s.memsz { ... }` —— `s.vaddr + s.memsz` 未 checked_add；恶意 ELF 段头 `vaddr=0xFFFF_FFFF_FFFF_F000, memsz=0x2000` 使和回绕为小数 → 入口校验被绕过。
- **根因**：信任 ELF 段头算术的普通加法。
- **修复方案**：
  ```rust
  let end = s.vaddr.checked_add(s.memsz).ok_or(ElfError::BadSegment)?;
  if e_entry < end { /* 原逻辑 */ }
  ```
- **二次验证**：grep 该函数调用方：elf-loader 内部加载路径 + root-task spawn 经 elf-loader 解析（间接）；grep 同文件其他 `+ s.memsz`/`+ p_filesz` 类算术需一并 checked（本次核查：其余边界检查已用 saturating/比较式写法，唯此处遗漏）。次生风险：无（拒绝面只增不减）。
- **QEMU 验证**：host `cargo test -p eneros-elf-loader`（构造回绕 fixture → Err）；QEMU 无新行为（回归现有服务 spawn 全绿）。
- **与 27 冲突**：S7（重定位表边界/对齐校验）与 WP-27.7 同文件同域——27.7 实施重定位时**必须**先吸收本修复，否则在已带洞的校验上叠加重定位。

### RT-006 【P1 / BUG】plan_one 无 PT_LOAD 段 vaddr 重叠检查

- **定位**：`crates/runtime/root-task/src/spawn.rs:136-188`（`plan_one`）
- **根因**：段按 ELF 序逐条转页条目，未做区间重叠检查（grep `overlap` 确认 spawn.rs 与 elf-loader 均无）。非单调/重叠段 vaddr → 同一虚拟页两次映射（后写覆盖前页内容）或与栈区（stack_top 向下）碰撞 → 服务运行期不可预测损坏。
- **修复方案**：
  ```rust
  // plan_one 尾部、返回前：
  let mut spans: Vec<(u64, u64)> = segs.iter().map(|s| (s.vaddr & !PAGE_MASK,
      align_up(s.vaddr + s.memsz, PAGE_SIZE))).collect();
  spans.sort_unstable();
  for w in spans.windows(2) {
      if w[0].1 > w[1].0 { return Err(SpawnError::SegmentOverlap); }
  }
  // 栈碰撞：stack_top - STACK_SIZE 不得落入任何 span
  ```
  新增 `SpawnError::SegmentOverlap` 变体（该 enum 有 host 消费测试，增 variant 波及面仅 Display/match——grep `SpawnError::` 全部 match 已带 `_` 兜底或逐一补齐）。
- **二次验证**：`plan_one` 调用方 = `plan_spawn`（唯一）；错误经 main.rs fail-closed 分支挂起（已有分支语义）。不变量「每虚拟页至多一个来源」保持。次生风险：无（拒绝面收紧）。
- **QEMU 验证**：host 单测：构造两段重叠的合成 plan 输入 → SegmentOverlap；QEMU 回归：现有 bundle 启动行不变。
- **与 27 冲突**：与 RT-005 同域（WP-27.7 elf-loader 重定位需同样段校验）——建议修复落点统一：elf-loader 提供 `validate_segments()` 纯函数，spawn.rs 复用，禁止两处各写。

### RT-007 【P1 / BUG】copy_segment_via_window 的 debug_assert 在 release 无效 → 越页写

- **定位**：`crates/runtime/root-task/src/spawn.rs:1175`
- **代码片段**：`debug_assert!(len <= PAGE_SIZE);` 后按 len 拷贝至 WINDOW_BASE 单页窗口。
- **根因**：release 构建剔除 debug_assert；若上游段计算给出 len > PAGE_SIZE（RT-006 场景即可构造），越页写破坏窗口后内存。
- **修复方案**：
  ```rust
  fn copy_segment_via_window(/*...*/, len: usize) -> Result<(), SpawnError> {
      if len > PAGE_SIZE { return Err(SpawnError::WindowOverflow); }
      // 原拷贝
  }
  ```
  调用方（同文件逐页拷贝循环）传播 Result。
- **二次验证**：grep `copy_segment_via_window` 仅 spawn.rs 内部调用（逐页循环），波及面闭合；`WINDOW_BASE` 其他写点 grep 确认无。次生风险：无。
- **QEMU 验证**：host 单测 len=PAGE_SIZE+1 → Err；QEMU 回归不变。
- **与 27 冲突**：同域无直接 WP；属 RT-006 的纵深防御，应同批修复。

### RT-008 【P1 / 硬件耦合】虚拟 timer IRQ 27 硬编码

- **定位**：`crates/runtime/root-task/src/main.rs:258`（`irq_table.bind(&mut ctx, 27)`）
- **根因**：QEMU virt arch timer 虚拟中断 27（PPI 16+11）直接硬编码；零硬件依赖要求 IRQ 号经 DTB/绑定表注入。换板（如 Phytium D2000）PPI 号不同即静默失效（bind 成功但中断永不触发，计数恒 0——当前错误分支只打印 "bind failed"，无法区分"绑定失败"与"绑错号"）。
- **修复方案**：
  ```rust
  // driver_probe.rs 或新 platform.rs：
  pub fn timer_irq_from_dtb(dtb: &[u8]) -> Option<u32> {
      // FDT /soc/timer 或 arch-timer 节点 interrupts 属性第 3 cell（PPI 号 + 16）
  }
  // main.rs：
  let irq = timer_irq_from_dtb(&dtb).unwrap_or(DEFAULT_QEMU_VIRT_TIMER_IRQ); // 默认值仅 QEMU 兜底 + 显式日志
  ```
  兜底路径必须打印 `timer IRQ from fallback=27` 判据行，禁止静默。
- **二次验证**：grep `bind(&mut ctx` / `irq_table.bind`：main.rs 唯一；`IrqTable::bind` 自身 irq 参数化已就绪（irq.rs 无改动需求）。device_tree.rs FDT 解析器已具备 interrupts 解析能力（board crate，已核查健壮）。次生风险：DTB 缺 arch-timer 节点 → 兜底值 + 日志；规避：判据行入启动日志门禁。
- **QEMU 验证**：QEMU virt 判据：`IRQ: irq=27 badge=... count=` 计数递增（现状基线）不回归；注入修改过的 dtb（timer IRQ=其它）→ 打印 fallback/DTB 来源行正确。
- **与 27 冲突**：**直接命中 WP-27.3 D8**（armv8-timer 节点经绑定表登记）与 S4（IRQ 接线）——D8 完成即本发现关闭；在 D8 落地前本发现保持 OPEN，禁止以"QEMU 能跑"销账。

### RT-009 【P1 / BUG】driver_probe IRQ 接线位置耦合 + badge 回绕

- **定位**：`crates/runtime/root-task/src/driver_probe.rs:499-500`
- **代码片段**：`report.into_bound().iter_mut().zip(ctx.bindings.iter())`；`(*badge - 1)` 用于绑定索引反查。
- **根因**：① zip 依赖 `bound` 顺序与 `ctx.bindings` 追加顺序严格一致——当前仅 `uart_factory` 调 `bind_irq`，顺序巧合成立；任何新驱动调整 probe 顺序或某驱动 bind_irq 失败跳过，zip 立即错位/截断（IRQ 接给错误驱动）。② `*badge - 1`：badge 语义为「绑定索引+1，0 保留」，若 badge=0（route 失败的保留值）到达此处 → u32 回绕成 u32::MAX 索引。
- **修复方案**：
  ```rust
  // ① 显式配对：bind_irq 返回 binding 索引并随 IrqDriver 携带，取消 zip：
  //   factory 内 let idx = irq_table.bind(ctx, irq)?; Ok((driver, idx))
  // ② badge 解码防御：
  let idx = (*badge).checked_sub(1).ok_or(ProbeError::BadBadge)?;
  ```
- **二次验证**：grep `into_bound`/`bind_irq`：driver_probe.rs 内部 + uart factory；`IrqDriver` 结构（service.rs L139-149 消费 `drv.binding/drv.irq`）需同步增字段——消费方 1 处。不变量「每驱动 IRQ 绑定唯一且来源显式」保持（符合 S4 分槽语义）。次生风险：API 变化波及 BindingTable factory 签名（WP-27.2 框架）→ 与 27.2 合并实施。
- **QEMU 验证**：host `cargo test -p eneros-root-task driver_probe`（新增：两个驱动乱序 bind → 索引正确）；QEMU 判据：UART 驱动 IRQ 收包行 `IRQ: irq=33`（PL011）计数递增不回归（WP-26.8/27.3 既有证据基线）。
- **与 27 冲突**：**直接命中 S4 + WP-27.2**（IrqTable 装配资源聚合进 PlatformCtx）——27.2 重构 IrqDriver 装配时若不消除 zip，会把本 BUG 固化进框架；27.2 实施必须携带本修复。

### RT-010 【P1 / BUG】INIT_LOAD 浮动区间 vs 内核固定窗口三重契约冲突

- **定位**：`crates/boot/boot-manager-uefi/src/lib.rs:16-17`（`INIT_LOAD_START=0x4200_0000` / `INIT_LOAD_END=0x4280_0000`）、L200-201 validate 允许 load_address 在区间内浮动；`main.rs:510` 写 `init_bundle_phys = init.load_address`
- **根因**：boot-manager 允许 bundle 落在 8MiB 区间任意处，但①内核只固定映射 0x4200_0000 起 2MiB（BOOT-007）；②root-task 硬编码读 0x4200_0000（RT-002）；③`dtb_window_for` 以 ENEROS_INIT_BUNDLE_PHYS 常量推导。只要 load_address ≠ 0x4200_0000（或长度 > 2MiB，RT-003）整链静默断裂。
- **修复方案**：
  ```rust
  // boot-manager-uefi/lib.rs：
  const INIT_LOAD_START: u64 = eneros_boot_protocol::ENEROS_INIT_BUNDLE_PHYS; // 0x4200_0000
  const INIT_LOAD_END: u64 = INIT_LOAD_START + BUNDLE_WINDOW_LEN as u64;      // 收紧到 2MiB
  // validate：load_address 必须 == INIT_LOAD_START（不再允许浮动）
  ```
  单一事实源常量进 boot-protocol；root-task RT-002/RT-003 修复引用同一常量。
- **二次验证**：grep `INIT_LOAD_START|INIT_LOAD_END` 仅 boot-manager lib.rs；`load_address` 消费方 = main.rs:510 写 BootInfo + validate。修复波及面闭合在 boot crate 内（root-task 侧配套 RT-002）。次生风险：历史证据（evidence/boot-unblock/）基于 0x4200_0000 固定加载，收紧后与证据一致，无回归风险。
- **QEMU 验证**：host `cargo test -p eneros-boot-manager-uefi`（浮动 load_address → validate Err）；QEMU 判据：`BOOT-007 kernel-mapped bundle probe=` 行继续出现。
- **与 27 冲突**：WP-27.1（引导链解锁）同域——27.1 已按固定窗口解锁，本修复是把 27.1 的事实向 boot-manager 回灌；顺序上须在 27.1 验收证据归档后实施。

### RT-011 【P1 / 硬件耦合】gicv3 单例硬编码 QEMU 基址，绕过注入接缝

- **定位**：`crates/hal/hal/src/arm64/gicv3.rs:256`
- **代码片段**：`static ARM64_GIC: Arm64Gic = Arm64Gic::new(0x08000000, 0x080A0000);`（QEMU virt GICD/GICR）
- **根因**：hal crate 已有 `PlatformConfig`（platform.rs，含 QEMU_VIRT / PHYTIUM_D2000 两组基址）与 `init_hal` provider 注入接缝，但 GIC 单例自成一体绕过两者——换板必须改源码。
- **修复方案**：
  ```rust
  impl Arm64Gic { pub fn init_with(cfg: &PlatformConfig) -> Result<(), HalError> { /* 写单例指针 */ } }
  // 或并入 HalProvider：fn irq(&self) 返回基于 PlatformConfig 构造的 GIC
  ```
  装配期（root-task 或 bring-up 入口）从 DTB 读取 GIC 基址填入 PlatformConfig 后调用。
- **二次验证**：grep `ARM64_GIC|Arm64Gic::new` 消费方：gicv3.rs 内部 accessor；grep `PlatformConfig`：platform.rs 定义 + provider 已有引用模式（uart/timer 参照）。波及面限于 hal crate 内。次生风险：init 顺序——GIC 使用点若早于 init_with → 返回 `HalError::NotInitialized` 而非 panic（fail-closed）。
- **QEMU 验证**：host `cargo test -p eneros-hal`；QEMU 判据：IRQ 计数基线（WP-26.8 证据）不回归。
- **与 27 冲突**：零硬件依赖总要求（审查域红线）；WP-27.2 PlatformCtx「配置加载」聚合点即为注入入口——27.2 实施时 GIC 应首个迁入。

---

## 三、详情（sel4-sys 域）

### RT-012 【P2 / BUG】REG_X30 与 REG_NZCV 索引碰撞

- **定位**：`crates/runtime/sel4-sys/src/types.rs:119-120`
- **代码片段**：`pub const REG_X30: usize = 33; pub const REG_NZCV: usize = 33;`
- **根因**：seL4_UserContext aarch64 = 36 words，索引 0-30 = x0-x30/sp/pc 等已核对（third_party sel4_client.h:52 288 字节断言为据）；X30 与 NZCV 被赋同一索引 33，二者只能其一正确。grep 证实当前**无任何消费方**（spawn.rs initial_registers 走 REG_PC/REG_SP/REG_X0），故 QEMU 实测可工作——潜在雷。
- **修复方案**：对照 seL4 `libsel4/include/sel4/arch/ucontext.h`（aarch64：`seL4_UserContext` 字段序）逐一重排 36 个索引常量，并加编译期断言 `const _: () = assert!(core::mem::size_of::<seL4_UserContext>() == 36 * 8);`（FFI 结构体侧）+ 索引唯一性测试（host：36 常量互不相等）。
- **二次验证**：grep `REG_X30|REG_NZCV` 零消费方 → 修复零波及；initial_registers（spawn.rs）只用已核对正确的 3 个索引，不受影响。
- **QEMU 验证**：host `cargo test -p eneros-sel4-sys`（索引唯一性新测）；QEMU 回归（spawn 链不涉这两个寄存器）。
- **与 27 冲突**：无直接 WP；属 ABI 卫生，建议随 29 号 P0 蓝图批量清理。

### RT-013 【P2 / BUG】host stub message_info_new 编码与 types.rs 不一致

- **定位**：`crates/runtime/sel4-sys/src/lib.rs:109-116`（host stub）vs `types.rs` 的 message_info 位段定义（length 在 bits 0-7）
- **根因**：host stub 用 `(length & 0xFF) << 8` 打包，与 types.rs 的 `message_info_length`（取 bits 0-7）解码方向不一致——host 构建自回环测试可能读回错误 length。
- **修复方案**：host stub 与 target 侧统一调用 types.rs 的打包/解包纯函数（`message_info_new`/`message_info_length` 本应是同一实现的 cfg 分支，禁两套编码）。
- **二次验证**：grep `message_info_new|message_info_length`：消费方 = service.rs（run_ipc_loop 编码 hello 请求）、services/lib.rs svc_ipc_loop（reply 编码）、safe.rs 测试。全部经统一函数 → 一致。次生风险：无。
- **QEMU 验证**：host `cargo test -p eneros-sel4-sys`（new→length 往返断言）。
- **与 27 冲突**：无。

### RT-014 【P2 / BUG】build_test_bootinfo_blob 写/读偏移错位

- **定位**：`crates/runtime/sel4-sys/src/safe.rs:1090-1095`
- **根因**：测试辅助写 `blob[off+16] = is_device` 而读取方取 `off+9`——该测试辅助构造的 bootinfo 中 device 标志恒读为 0，掩盖 untyped device 判定的真实测试意图。
- **修复方案**：对齐到 BootInfo untyped 描述符布局的真实偏移（与 safe.rs 读取函数共享常量 `UNTYPED_FLAGS_OFFSET`）。
- **二次验证**：grep `build_test_bootinfo_blob` 消费方：safe.rs 测试 + root-task host 测试（若有）；修正后需重跑全部依赖该 fixture 的测试并复核断言语义（此前"恒 0"的通过可能是假绿）。
- **QEMU 验证**：host `cargo test -p eneros-sel4-sys --lib`。
- **与 27 冲突**：无。

### RT-015 【P2 / BUG】sel4_shim.h IRQ 声明位于 header guard 之外

- **定位**：`crates/runtime/sel4-sys/shim/sel4_shim.h:240`（`#endif`）与 L242-249（IRQ 函数声明）
- **根因**：声明块落在 `#endif` 之后，重复包含时重复声明（C 允许但守卫失效；若后续在声明区加宏定义将重复定义报错）。
- **修复方案**：将 L242-249 移入 guard 内（`#endif` 之前），一次移动零语义变化。
- **二次验证**：grep `sel4_shim.h` 包含方：sel4_shim.c + build.rs 生成的 bindgen 输入；移动后编译即验证。
- **QEMU 验证**：`cargo build -p eneros-sel4-sys --target aarch64-unknown-none -Z build-std=core,alloc`（bindgen 路径重新生成）+ QEMU 回归。
- **与 27 冲突**：无。

### RT-016 【P2 / BUG】gicv3 `static mut IRQ_HANDLERS` 无访问守卫

- **定位**：`crates/hal/hal/src/arm64/gicv3.rs:54`
- **根因**：依赖"当前单核"前提；无编译期/运行时守卫，SMP bring-up 时即数据竞争。
- **修复方案**：`static IRQ_HANDLERS: SpinLock<[Option<IrqHandler>; N]>` 或明确 `// SAFETY: 单核阶段独占` + 门禁规则禁 SMP 前使用；长期随 RT-011 一起 provider 化。
- **二次验证**：grep `IRQ_HANDLERS` 仅 gicv3.rs 内部读写。波及面闭合。
- **QEMU 验证**：host `cargo test -p eneros-hal`；行为无变化。
- **与 27 冲突**：无；SMP 不在 27 范围，登记为技术债。

---

## 四、详情（abi 域）

### RT-020 【P2 / BUG】ipc-wire MR 帧上限 952B < el0 最大载荷 1024B

- **定位**：`crates/abi/ipc-wire/src/mr.rs:27`（`MAX_FRAME_BYTES=952`，120 words 减去长度槽）vs `src/el0.rs:91`（`DEFAULT_MAX_PAYLOAD_LEN=1024`）
- **根因**：el0 帧 = 10B 头 + payload + 4B crc，payload=1024 时帧长 1038B > 952B → `frame_into_mrs` 必然 `FrameTooLong`。两侧常量各自为政。
- **修复方案**：在 ipc-wire 内导出单一契约常量：`pub const MAX_EL0_PAYLOAD_IN_MR: usize = MAX_FRAME_BYTES - EL0_FRAME_OVERHEAD;`，el0.rs 的 DEFAULT_MAX_PAYLOAD_LEN 文档注明"超过 MR 单帧容量，需分帧"或下调对齐；并在 mr.rs/el0.rs 加编译期断言互相引用。
- **二次验证**：grep `MAX_FRAME_BYTES`：service.rs re-export、services/lib.rs svc_ipc_loop（frame_from_mrs/frame_into_mrs 失败走 fail-closed）——当前真实负载（hello/pi 响应远小于 952）不触发；契约修复零行为变化。
- **QEMU 验证**：host `cargo test -p eneros-ipc-wire`（新增：payload=MAX_EL0_PAYLOAD_IN_MR 恰好成帧、+1 拒绝）。
- **与 27 冲突**：无；WP-27.8 IEC104 传输走其他通道，但若未来 IPC 承载大帧必须先解决本条。

### RT-005/RT-006 已在第二章（跨域引用 elf-loader）

---

## 五、详情（boot 域）

### RT-021 【P2 / 缺失功能】init_bundle_phys 写入无消费

- **定位**：`crates/boot/boot-manager-uefi/src/main.rs:510`
- 与 RT-002 同根因的两端：写入端（boot-manager）已就绪、消费端（root-task）缺失。修复方案与二次验证见 RT-002；本条作为契约链台账独立登记，两端同 PR 关闭。
- **与 27 冲突**：WP-27.1 同域（见 RT-002）。

### RT-022 【P2 / BUG】MAX_INIT_BUNDLE_LEN 三重不一致

- **定位**：`crates/boot/boot-protocol/src/boot_info.rs:20`（256MiB）vs root-task main.rs（64MiB，RT-003）vs 内核窗口（2MiB）
- **修复方案**：boot-protocol 导出 `ENEROS_INIT_BUNDLE_MAX_LEN = 2MiB`（与内核窗口对齐），boot-manager validate 与 root-task read 全引用；256MiB 常量删除或标注 dormant。
- **二次验证**：grep `MAX_INIT_BUNDLE_LEN` 消费方：boot_info.rs validate + boot-manager；改小后 validate 逻辑收紧（拒绝 >2MiB bundle），与 RT-010 修复同批。
- **QEMU 验证**：host `cargo test -p eneros-boot-protocol`；QEMU 回归（现有 bundle 远小于 2MiB）。
- **与 27 冲突**：WP-27.1 同域。

### RT-023 【P2 / 缺失功能】build.rs GUID 注入块重复

- **定位**：`crates/boot/boot-manager-uefi/build.rs:48-71` 与 L100-122（两段等价的 GUID 注入逻辑）
- **修复方案**：保留其一（后段），删除死块；`cargo clean -p eneros-boot-manager-uefi && cargo build` 验证产物一致（比对 .efi 内 GUID 字节）。
- **二次验证**：build.rs 无其他消费方；注入目标 GUID 字符串 grep 确认两段写同一目标。次生风险：删错块 → 用构建产物 diff 兜底。
- **与 27 冲突**：无。

---

## 六、详情（hal / board 域）

### RT-024 【P2 / BUG】gicv3 GICR 就绪 `.expect()` 在 boot 路径

- **定位**：`crates/hal/hal/src/arm64/gicv3.rs:137`（`.expect("GICR redistributor failed to become ready")`）
- **根因**：boot 关键路径 panic = 整机不可恢复（无 supervisor 兜底——此时 root-task 尚未起）。HAL-005 有界忙等原则要求轮询有界 + 错误返回。
- **修复方案**：`fn wait_gicr_ready(base: u64) -> Result<(), HalError> { for _ in 0..100_000 { ... } Err(HalError::Timeout) }`，调用方传播 Result。
- **二次验证**：grep 该函数调用方：gicv3 init 链（单点）；init 失败 → 返回错误由装配层 fail-closed。QEMU 上 GICR 恒就绪，无回归。
- **与 27 冲突**：无；HAL-005 既有约束的补漏。

### RT-025 【P2 / 硬件耦合】UART/NET 单例硬编码；net_mmio 基址与自身注释矛盾

- **定位**：`crates/hal/hal/src/arm64/uart_pl011.rs:147`（`static ARM64_UART: Pl011Uart = Pl011Uart::new(0x09000000)`）；`net_mmio.rs:126`（`static ARM64_NET: NetMmio = NetMmio::new(0x09010000)`）
- **根因**：同 RT-011 模式（绕过 PlatformConfig）。net_mmio 更严重：模块注释 L7-10（HAL-004 记录）明确指出 0x09010000 是 **PL031 RTC** 且"不预设默认基址"，单例却预置了该地址——若未来有消费方接入即把 RTC 寄存器当网卡读写。grep 证实 `ARM64_NET`/`net()` 当前**无消费方**，故仅潜在。
- **修复方案**：①两单例随 RT-011 一并 provider 化（PlatformConfig 注入）；②过渡期：`NetMmio` 单例改为 `Option` 未初始化态，`net()` 返回 `Result<&NetMmio, HalError>`，禁默认基址。
- **二次验证**：grep `Pl011Serial|net()|ARM64_UART|ARM64_NET`：UART 单例有 console 消费（eneros-runtime println 链）——provider 化时须保证 console 初始化顺序（init_hal 先于首个 println）；NET 零消费方，改造零波及。
- **QEMU 验证**：QEMU 判据：boot banner 串口输出不回归（console 链）。
- **与 27 冲突**：WP-27.3 D2（virtio-net 新写）将引入真实 NetDevice——D2 实施前必须关闭本条，否则 NetMmio 错误基址会被 D2 接线消费。

### RT-026 【P2 / BUG】mini_uart 无界 TX 忙等

- **定位**：`crates/hal/board/src/mini_uart.rs:63`（`while (self.read(UART_FR) & FR_TXFF as u32) != 0 {}`）
- **根因**：HAL-005（有界忙等）只覆盖了 hal crate，board crate 遗漏；设备无响应时死循环。grep 证实 mini_uart 当前无消费方（Raspberry Pi 系预留），故潜在。
- **修复方案**：有界轮询（如 100_000 次）后返回 `Err(HalError::Timeout)`；签名 `fn write_byte(&self, b: u8) -> Result<(), HalError>`。
- **二次验证**：零消费方 → 签名变化无波及。
- **与 27 冲突**：无。

### RT-027 【P2 / BUG】hal 单例 `static mut HAL` + expect

- **定位**：`crates/hal/hal/src/lib.rs:169,192`
- **根因**：write-once 语义由调用约定保证（init_hal 早于一切）；`hal()` 未初始化时 expect panic。当前单核 boot 序列成立，属可接受但脆弱。
- **修复方案**：`static HAL: OnceLock<&'static dyn HalProvider>`（no_std 用 spin::Once），`hal()` 未初始化返回明确 panic 消息或 `Option`。低优先。
- **二次验证**：grep `init_hal|hal()`：mock.rs 测试 + arm64 provider 装配 + root-task time_init 链。OnceLock 化不改变任何成功路径。
- **与 27 冲突**：无。

### RT-028 【P2 / 缺失功能】PANIC_GENESIS 假哈希（两处）+ services panic 路径 Mutex

- **定位**：`crates/runtime/root-task/src/panic.rs:35-38`；`crates/runtime/services/src/panic.rs:25-28`（`PANIC_GENESIS` 字节 = ASCII "svc-panic-genesis\0..."）、services panic.rs L42 panic 路径 `PANIC_LOGGER.lock()`
- **根因**：①注释/文档称创世值为 SM3 哈希，实为 ASCII 占位——审计链"创世"可被离线伪造（两处同模式）。②services panic hook 持 spin Mutex：若 panic 由持锁代码触发（logger 内部 panic 或重入）→ panic 路径死锁挂起，连打印都没有。
- **修复方案**：①创世值改由 boot-manager 装配期提供真实 SM3 根（蓝图 27.5 DurableLog 持久化收口的输入项；落地前把注释改诚实："占位，W3 持久化后由 boot-manager 提供"——services 侧注释已如实，root-task 侧注释需改）。②panic hook 改 `try_lock`：拿不到锁跳过审计、保证打印与挂起步骤可达。
- **二次验证**：grep `PANIC_GENESIS`：两处定义两处消费（各自 init）；`try_lock` 改造波及仅 svc_panic_hook。不变量"panic 路径不引入新阻塞"加强。
- **QEMU 验证**：QEMU 判据：svc-fault Panic 注入 → `[svc-svc-fault] PANIC #1` 行出现（现状基线）；host 单测：重入 panic 不挂测试进程。
- **与 27 冲突**：WP-27.5（DurableLog + 持久化收口，S5）——创世根持久化正是 27.5 的收口物；本条①在 27.5 落地时关闭，②独立先行。

### RT-029 【P2 / 缺失功能】supervisor 占位服务名

- **定位**：`crates/runtime/root-task/src/supervisor.rs:160-167`（`new_for_count` 生成 `svc-<i>`）
- **根因**：run_ipc_loop（service.rs L86）调用 `new_for_count(handles.len())` 而非带名构造 → SUPERVISOR 故障日志里的服务名是占位序号，故障定位价值打折。
- **修复方案**：`pub fn new_with_names(names: &[&str], now_ms: u64) -> Self`，run_ipc_loop 以 `handles.iter().map(|h| h.name.as_str())` 传入；`new_for_count` 保留为测试便捷构造并标注 `#[cfg(test)]` 或文档。
- **二次验证**：grep `new_for_count`：service.rs L86 + supervisor 自身测试；改后测试仍可用。次生风险：无。
- **QEMU 验证**：QEMU 判据：fault 注入后 `SUPERVISOR: svc-fault fault → restart` 行服务名 = 真实名（而非 svc-3）。
- **与 27 冲突**：WP-27.4（PENDING→真证据：故障重启证据链）——日志服务名是证据可信度的一部分，27.4 出证前应关闭。

### RT-030 【P2 / 缺失功能】allocator 文档与实现不一致

- **定位**：`crates/runtime/root-task/src/allocator.rs:7`（"默认 512 KiB"）vs L16-27（实现默认 1 MiB）
- **根因**：G-R1 把默认从 512KiB 提到 1MiB 时模块头部注释未同步。
- **修复方案**：改文档行即可（或注释引用 G-R1 决策）。
- **二次验证**：纯注释，零波及。
- **与 27 冲突**：无。

### RT-031 【P2 / BUG】retype_objects 参数 types 被忽略

- **定位**：`crates/runtime/root-task/src/spawn.rs:1144-1157`（签名带 `types` 但 L1153 固定 `types.arm_page_table`）
- **根因**：函数名暗示泛型 retype，实现硬编码页表类型；调用方传任何对象类型意图都被静默改写。
- **修复方案**：①重命名 `retype_page_tables`（诚实命名）；或②增加 `obj_type: u64` 参数并删除误导签名。推荐①（波及面：grep `retype_objects` 全部调用点逐一改名，均为 spawn.rs 内部）。
- **二次验证**：grep `retype_objects`：spawn.rs 内部数处调用，全部确属页表 retype 语义 → 改名零语义变化。
- **与 27 冲突**：无。

---

## 七、已核查无发现（诚实清单）

以下方面逐文件/逐调用点核查完毕，**无发现**：

1. **sel4 FFI 参数序**：`retype`、`tcb_set_ipcbuffer`、`cnode_mint/copy/move`、`irq_control_get`、`arm_page_map/arm_page_table_map`（FailedLookup 逐级补表）等 extern 声明与 sel4_shim.c 实现、third_party sel4_client.h 逐个核对一致；seL4_UserContext 288 字节/36 words 与 REG_PC=0/REG_SP=1/REG_X0=3 经 third_party 断言与 QEMU 实测双重佐证。
2. **boot-protocol 编解码健壮性**：manifest/boot_control/measurement/entropy/gpt/boot_info 的 decode 全部带 magic/版本/长度/范围校验，无裸索引、无回绕算术（唯一例外已列 RT-022 常量层）。
3. **elf-loader W^X 与边界**：段权限拒绝 W^X 双映射、文件/内存尺寸边界检查完备（例外 RT-005 入口校验一处）。
4. **ipc-wire 帧完整性**：el0 crc32 校验、MR 帧长度槽编解码有完整 host 测试（例外 RT-020 常量契约）。
5. **board/device_tree.rs FDT 解析**：结构边界检查健壮，真实 dtb fixture 测试，无发现。
6. **hal timer_math.rs / bit_logic.rs / types.rs**：纯函数+测试完备，无发现。
7. **root-task/irq.rs**：route/binding 纯函数测试完备；`bind` 的容量守卫先于 syscall（不留半绑定）；cnode_mint depth=0 语义经 WP-26.8 QEMU 实测验证，无发现。
8. **supervisor.rs 状态机**：指数退避 1s-60s、5 次/小时熔断、滑动窗口均有 host 测试，无发现（例外 RT-029 命名）。
9. **UntypedAllocator 记账**：freeIndex/alignUp、alloc_at/alloc_device_at（物理校验 + node_offset=slot）与 FIX-BUGS-0810 两件套语义一致，已核查无回归。
10. **services/lib.rs 请求路由**：parse_u64 溢出防护、unknown fail-closed、fault 注入语义均有测试；hog/pi/hello 为真实计算非假数据，无发现。
11. **root-task/services allocator**：LockedHeap 生产路径 + UnsafeCell/CAS bump 参考实现并发论证完整，OOM→null 契约正确，host 回收稳定性测试在案，无发现（例外 RT-030 注释）。
12. **AAVMF 接缝**（third_party/AAVMF 仅我方配置）：boot-manager-uefi 的 UEFI 调用走标准 Boot Services 接口，无自定义 hack，无发现（build.rs 重复块已列 RT-023）。
13. **root-task panic 之外的错误吞没扫描**：`let _ =` 全 grep 复核——services/panic.rs 的 `let _ = logger.log(ev)` / `s.push_str` 为定容容器饱和语义（可接受）；ipc 主循环所有 Result 均 fail-closed，无吞没。

---

## 八、与蓝图/27 冲突/重叠汇总

| 发现 | 涉及 WP/条目 | 关系 |
|---|---|---|
| RT-008 | WP-27.3 D8、S4 | D8（timer 经绑定表登记）完成即关闭；D8 前保持 OPEN，禁以 QEMU 现状销账 |
| RT-009 | WP-27.2、S4 | 27.2 重构 IrqDriver/PlatformCtx 时**必须**携带本修复，否则 zip 位置耦合固化进框架 |
| RT-010/RT-002/RT-003/RT-021/RT-022 | WP-27.1 | 27.1 已按固定窗口解锁（BOOT-007 证据在案）；本组是向 boot-manager/protocol 回灌同一事实，须在 27.1 验收归档后实施，不得回退其窗口证据 |
| RT-011/RT-016/RT-024/RT-025/RT-027 | WP-27.2 PlatformCtx、零硬件依赖红线 | PlatformConfig 注入接缝已存在但被 5 处单例绕过；27.2 聚合配置加载时 GIC 首个迁入 |
| RT-025(net_mmio) | WP-27.3 D2 | D2（virtio-net）接线前必须关闭，否则错误基址被消费 |
| RT-004 | S7、WP-27.7、WP-27.9 | 与 elf-loader 验签共用哈希资产，禁各写一套 |
| RT-005/RT-006/RT-007 | S7、WP-27.7 | 27.7 重定位实施前必须先关闭校验洞（禁止在带洞校验上叠加重定位） |
| RT-028① | WP-27.5（S5） | 创世根真实化随 DurableLog 持久化收口落地 |
| RT-029 | WP-27.4 | 故障重启真证据出证前关闭（日志服务名可信度） |
| RT-001 | WP-27.3 D6 | D6 软件模式语义下 panic 路径守约，同批实施 |
| 其余（RT-012~015、017~020、023、026、030、031） | 无 | 与 27 无重叠，属域内卫生/潜伏项 |

## 九、修复优先级建议（供 29 号 P0 蓝图编排）

1. **第一批（阻断性潜伏，随时可能被真实负载/新驱动触发）**：RT-003、RT-006、RT-007、RT-009、RT-001、RT-005
2. **第二批（契约链对齐，一次 PR 全关）**：RT-002、RT-010、RT-021、RT-022（共享 boot-protocol 单一事实源常量）
3. **第三批（硬件耦合收口，并入 WP-27.2/27.3 实施）**：RT-008、RT-011、RT-025
4. **第四批（卫生清理）**：其余 P2

