# 30 版本后审查修复报告（代码级 bug 审查，全局视角/最小修改风险）

执行日期：2026-08-11（v30 验收 a6a2770 之后）。审查对象：v30 全部 8 提交
（aee7f05~a6a2770）的代码实现 vs 蓝图/30 规格 + 权威 seL4 14.0.0 源码
（/home/ahx/.cache/sel4-src，与本仓库 third_party/sel4-build 构建同源）。

方法：逐 WP diff 深读 + 蓝图规格逐条比对 + seL4 内核/libsel4 权威源码核对
+ QEMU 验收日志（gkernel-qemu-boot-30.log）行为交叉验证。

## 一、P1 发现与修复（2 项，均已修复）

### 1. RT-012 REG 索引"修复"方向错误——spawn 服务参数链回归

- **权威布局**（seL4 14.0.0 libsel4 types.h + 内核 registerset.c）：
  `pc=0, sp=1, spsr=2, x0=3, x1=4, ..., x8=11, x16=12, x17=13, x18=14,
  x29=15, x30=16, x9=17..x15=23, x19=24..x28=33, tpidr_el0=34,
  tpidrro_el0=35`——**X 寄存器不连续，无 NZCV 字段**（PSTATE 在 SPSR）。
- v30 把 REG_X0 从 3 改为 2、REG_X1 从 4 改为 3、REG_X30 改为 32、新增
  REG_NZCV=33——与权威布局矛盾（index 2 实为 SPSR、index 3 实为 x0、
  index 16 实为 x30、index 33 实为 x28）。
- **影响**：`initial_registers`（spawn.rs）`set_x(0, 0)`（ep cap 槽）写进
  SPSR、`set_x(1, buffer_vaddr)` 写进 x0 → 服务 `_start(ep, ipc_buffer)`
  收到 ep=buffer_vaddr、ipc_buffer=0 → NULL IPC buffer + 无效 cptr →
  服务 fail-closed 挂起。若按 v30 代码重建镜像，QEMU 系统级验收必然失败。
- **证据链**：验收日志 L683 `[svc-hello] _start entered ep=0 ipcbuf=0x221000`
  是**旧布局**（REG_X0=3）的行为——说明验收镜像未包含 RT-012 改动
  （旧镜像/未重链 root-task），不能作为"修复正确"的证据；反而证明服务
  参数链依赖 REG_X0=3。审计报告声称"REG_X0=3 经 third_party 断言+QEMU
  双重佐证"——实际 third_party 仅有 288B **尺寸**断言（非字段序）。
- **修复**（types.rs）：恢复权威索引（REG_SPSR=2/X0=3/X1=4/X30=16/
  X9=17/X28=33/TPIDR_EL0=34/TPIDRRO_EL0=35；删除不存在的 REG_NZCV）；
  `set_x` 改 `reg_x_index` 表驱动映射（X 不连续，禁线性 +n）；测试改为
  权威映射断言 + `spawn_register_contract_x0_ep_x1_buffer`（固定索引锚定
  spawn 契约，防再错位）。

### 2. DurableLog::append 缓冲区越界 panic + 小块设备静默截断

- `buf[..record.len()]`：rec.len() > 4080（16B 头 + 4096 缓冲）→ 切片
  panic（库公开契约违反 fail-closed——33 域掉电证据的基础设施）。
- 512B 块设备写 >496B 记录：写入被块边界静默截断，**append 返回 Ok 但
  数据损坏**（读侧 CRC 才暴露 Corrupt）。
- **修复**（durable-log/src/lib.rs）：`record_capacity()` 单一事实源
  （= block_size.saturating_sub(16).min(4080)），append 超容量 →
  `Err(Full)`；read_from/recover 长度检查对齐同一容量；新增 496/497 与
  4080/4081 边界判据测试。
- 顺带：`PersistentAuditStore::read_range` 解码失败静默丢条目（长度 ≠
  112B 的记录被跳过不报错）→ 置 malformed 标志整体 `Err`（fail-closed）。

## 二、P2 发现与修复（4 项，均已修复）

### 3. plan_one 栈碰撞检查遗漏（蓝图 RT-006 规格明确要求）

- 蓝图规格："栈碰撞：stack_top - STACK_SIZE 不得落入任何 span"——v30
  实现只做了段间重叠检查（validate_segments），未做栈 vs 段检查。乱序
  非重叠段（ELF 序最后一段不是最高 vaddr）时栈起点（= 最后一段 end）
  低于前面段的末端 → 栈与段共享虚拟页（后写覆盖/执行损坏）。
- **修复**（spawn.rs）：plan_one 追踪 `max_seg_end`，任何段末端超出栈
  起点 → `Err(SegmentOverlap)`（纯拒绝面新增，正常 ELF 零行为变化）。
- 新判据测试：乱序段 ELF → SegmentOverlap；升序段照常通过。

### 4. RT-007 测试为 include_str 源码字符串检查（假测）

- `test_copy_segment_via_window_rejects_oversized_len` 用
  `include_str!("spawn.rs")` 检查源码文本，不调用生产函数。
- **修复**（spawn.rs）：长度检查提取为纯函数 `window_copy_len_ok`，
  copy_segment_via_window 调用之；测试直接调用纯函数断言行为
  （PAGE_SIZE 边界 ±1）。

### 5. elf-loader 重定位：写入越界 panic 面 + 段边界算术未 checked

- `img[slot..slot+8]`（slot = r_offset）无越界检查：装配层传入未铺到段
  末端的镜像（如文件字节）→ 切片 panic。
- `in_writable` 的 `base + s.vaddr(+memsz)` 普通加法：恶意 base/vaddr
  回绕可绕过检查。
- rela_size 非 rela_ent 整数倍：余数条目静默忽略。
- **修复**（elf-loader/src/lib.rs）：写入前 `slot+8 ≤ img.len()` →
  `Err(RelocOutOfBounds)`；段边界算术 checked（回绕 → 拒绝）；
  size % ent ≠ 0 → `Err(BadRelaTable)`；文档明确 img 契约（链接地址平坦
  化镜像、当前支持文件偏移 == 链接地址形态；真实 ELF 重定位须扩展，
  登记 31 域装配层）。新增 3 个判据测试（越界写、非整除、slot 越 img）。

### 6. allocator 注释失真（RT-030 过度替换）

- "1 MiB bump 时代刚好够"语义失真（实际是 512KiB 时代不够才提到 1MiB）。
- **修复**（allocator.rs）：注释恢复历史事实 + G-R1 决策引用。

## 三、登记不修（P3/低风险，理由在案）

| 项 | 说明 |
|---|---|
| RT-009 位置耦合未完全消除 | zip 改显式索引配对 + MISMATCH 判据行（不再静默截断）；蓝图建议"绑定索引随 IrqDriver 携带"的彻底改造留给 31 域 PlatformCtx 聚合 |
| RT-026 mini_uart | 实现为有界等待 + 静默丢字符（非蓝图建议的 Err 签名）；零消费方；死循环 bug 已消除 |
| RT-023 build.rs | GUID 保留"前段"而非蓝图说的"后段"——功能等价，产物一致 |
| DurableLog 小项 | block_size < 16 设备 → record_capacity=0 全拒绝（fail-closed）；写满块设备返回 Io 而非 Full（语义等价） |
| 重定位 rela 表文件偏移语义 | 当前形态要求文件偏移==链接地址（黄金向量一致）；真实 ELF 扩展登记 31 域 |

## 四、验证

- `cargo test -p eneros-sel4-sys`：19 全绿（+2 新判据）。
- `cargo test -p eneros-durable-log`：7 全绿（+2 新判据）。
- `cargo test -p eneros-root-task`：62 全绿（+2 新判据，假测移除）。
- `cargo test -p eneros-elf-loader`：19 全绿（+3 新判据）。
- `cargo test --workspace`：3486 passed / 0 failed。
- `cargo check --target aarch64-unknown-none -Z build-std=core,alloc`
  （elf-loader、durable-log）：通过。
- **局限**：sel4-sys/root-task 的 aarch64 全链构建需 C 交叉工具链
  （aarch64-none-elf-gcc，本机无，QEMU 构建在 WSL 环境）——types.rs/
  spawn.rs 改动均为无条件编译代码，host 编译已覆盖；QEMU 复跑（含
  RT-012 修复后的服务参数验证）须在 WSL 环境重链镜像执行（见
  evidence/gkernel-qemu-boot-30.log 同目录既有流程）。

## 五、验收日志可信度登记

gkernel-qemu-boot-30.log 的服务参数行（ep=0/ipcbuf=0x221000）与 v30
RT-012 代码（REG_X0=2）行为矛盾 → 该日志（或至少服务运行段）来自
RT-012 之前的构建。wp30-acceptance.md 声称"commit=ab2d221 重建"与实际
行为不符。已按代码为准修复；后续 QEMU 证据须核对服务参数行后再引用。
