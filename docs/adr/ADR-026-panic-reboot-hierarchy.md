# ADR-026 — panic 分级重启语义与复位机制（22 W1.2）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 22 §2.W1.2 强约束 + §1 锁定 6 分级重启）
- **关联**：ADR-025（alloc_error_handler）、`蓝图/22` §2.W1.2/W1.6、§8 G-R1

## 背景

22 §2.W1.2：panic handler 行为 = ①写审计（SM3 链）→ ②重启计数持久化 →
③喂看门狗触发硬件重启。现状 `root-task/main.rs` 与 `services/lib.rs` 的
panic handler 纯 `spin_loop`。

## 现状侦察（2026-08-09 代码为准）

1. 审计链 `eneros-audit` no_std 兼容（heapless 定容 + SM3），
   `AuditLoggerImpl<MemStore>::log` 全无堆路径——panic 可安全调用。
2. `BootReason::Watchdog` 已存在（boot-manager 持久化重启原因）。
3. SP805 驱动真实（`eneros-watchdog::wdt::HwWatchdog::init` 序列：
   unlock→load→intclr→ctrl(EN|RESEN)→lock；超时→SoC 复位）。
4. **xpack QEMU 9.2.4 构建缺陷**：`-device help` 无任何 ARM watchdog
   （仅 x86 i6300esb），virt 设备树无 watchdog 节点，`psci` 机器属性缺失
   ——裸机 `-kernel` 场景无可用硬件复位机制（UEFI/AAVMF 场景除外，其
   ResetSystem 走固件路径）。

## 决策

1. **分级 panic 语义**（锁定 6）：
   - **root-task panic** → 审计 + 重启计数 + 武装 SP805 10 ms 短超时 →
     SoC 复位（root-task 无 supervisor 兜底，panic 即整机复位）。
   - **EL0 服务 panic** → 审计 + 打印 + **挂起**（不复位整机——隔离
     语义）；重启由 supervisor（W1.3）执行；整机看门狗（W1.6）为
     supervisor 失效时的最后防线。
2. **重启计数持久化暂缓**：W1.2 用内存原子（`PANIC_REBOOT_COUNT`）+
   串口日志（可观测）；跨掉电持久化随 W3.5 存储层（WAL/fs）落地后接入。
   重启**原因**（`BootReason::Watchdog`）由 boot-manager 侧已有机制记录。
3. **复位机制分层**：生产 = SP805 武装（真机 BSP 提供基址，QEMU 完整版
   亦有效）；`WDT_BASE=0` 或设备缺失时 fail-closed 挂起（软件模式）。
   不依赖 PSCI（xpack QEMU 裁剪；真机后续可加）。
4. **可执行证据分层**：
   - host：`sp805_arm_sequence` 寄存器序列断言（unlock/load/intclr/
     ctrl/lock 五步）+ 审计链递增 + 计数递增；
   - QEMU：`panic-probe` 验证 panic → 打印 + 武装序列执行 + 挂起不
     abort；
   - **硬件复位动作（超时→SoC 复位→重启）**：真机/QEMU-完整版验证项，
     列入 G-R1 闸门真机清单与 ADR-025 的 G-R1 补充项。

## 影响

- panic 从"变砖挂起"变"可观测 + 可复位"：审计链存证 + 计数指标 +
  硬件复位武装（真机）。
- 服务 panic 不误伤整机（supervisor 隔离语义），符合 W1.3 前提。
- QEMU 端到端"重启序列"验证受限（QEMU 构建缺 SP805/PSCI）——已在
  验收证据中显式标注真机项，不做虚假声称。

## 备选方案

- **PSCI SYSTEM_RESET（smc #0）**：ARM 标准 API，真机固件必支持；xpack
  QEMU 9.2.4 无 psci 属性（实测报错），弃用；ADR 记录待 QEMU 升级后
  可切换。
- **服务 panic 也复位整机**：违背 21-A 隔离验证语义（svc-fault 挂起
  不扩散是既有验收），否决。
- **panic 内写文件持久化计数**：panic 时存储栈不可信（可能再次 panic），
  否决；计数持久化随 W3.5 正式存储层。
