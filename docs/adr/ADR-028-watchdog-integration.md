# ADR-028 — 看门狗分级接入与"服务卡死"检测语义（22 W1.6）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 22 §2.W1.6 强约束 + §1 锁定 6 分级重启）
- **关联**：ADR-026（panic 分级重启）、ADR-025（alloc_error_handler）、`蓝图/22` §2.W1.3/W1.6

## 背景

22 §2.W1.6：root-task 主循环 + 每个 EL0 服务循环定期喂看门狗（SP805 驱动
已真）；禁止任何长循环不喂狗。验收：正常不超时；服务卡死 → 看门狗触发
重启（supervisor 先兜底，看门狗最后防线）。

## 现状侦察（2026-08-09 代码为准）

1. `eneros-watchdog` 分层 API：`wdt_init(timeout, base)` / `wdt_kick` /
   `wdt_check -> WatchdogStatus{AllFed, LayerTimeout, HardReset}`；`base=0`
   软件模式（MMIO no-op）。
2. **跨进程限制**：layer 注册在进程内静态（各 EL0 服务是独立地址空间），
   root-task 无法检查服务进程的 layer；服务进程也无法让 root-task 感知其
   卡死（卡死 = 无法 self-report）。
3. 21-A 既有机制：root-task 每轮 `ipc_call` hello 心跳（同步阻塞）。
4. seL4 无超时原语：root-task 对卡死服务的 `ipc_call` **永久阻塞**。
5. xpack QEMU 9.2.4 无 SP805 设备（ADR-026 已记录）。

## 决策

1. **root-task 每轮 `wdt_kick()`**（喂硬件狗；超时 10s）。root-task 卡死
   （含对卡死服务的阻塞 `ipc_call`——服务卡死 → hello 心跳阻塞 → root-task
   停止喂狗）→ SP805 超时整机复位。**服务卡死的检测链**：supervisor 兜底
   fault（W1.3，异常路径）→ 卡死路径（无 fault）经 root-task 阻塞传导至
   硬件狗复位。
2. **服务侧不注册 layer**：跨进程不可查询（侦察 2），注册即假实现；服务的
   "活性信号" = hello 心跳响应（21-A 既有），经 root-task 喂狗传导。
   `wdt_check` 每 100 轮诊断（预留 layer 扩展）。
3. **QEMU 无 SP805**：喂狗/kick 为 MMIO no-op（无害）；"硬件复位"动作为
   真机/QEMU-完整版验证项（ADR-026 同）。
4. **验收证据分层**：
   - host/编译：W1.1-W1.5 全量测试 + target 构建；
   - QEMU：oom-probe / panic-probe（裸机通道）验证 OOM/panic 路径；
   - QEMU 全系统：G-R1 闸门跑 qemu-prod——supervisor 序列
     （fault 注入 → 重启 → 熔断）日志可见（无 SP805，硬件复位为真机项）。

## 影响

- 长循环不喂狗 = 系统不可信（10s 内硬件复位，真机）；QEMU 阶段行为
  fail-closed（MMIO no-op 不误复位）。
- 服务卡死检测完整链：fault→supervisor 重启；卡死→硬件狗复位（真机）。

## 备选方案

- **服务进程内 layer + 跨进程查询**：seL4 IPC 探活需新协议 + 超时原语
  （seL4 无），工程量与收益不成比例；否决，留待"心跳 IPC"统一协议
  （W4.2 远程管理通道可复用）。
- **root-task 非阻塞轮询探活**：seL4 无超时 recv（Poll 只有立即返回），
  需每服务 ping 协议 + 状态机，v1.0 范围外。
