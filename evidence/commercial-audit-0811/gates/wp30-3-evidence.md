# WP-30.3 panic/WDT 守约（RT-001/028/029/030）修复与证据

执行日期：2026-08-11。蓝图坐标：30 §5。

## RT-001（P1，panic 路径二次 VM fault）
- panic_reboot 武装 SP805 前检查 WDT_MMIO_MAPPED（旧实现软件模式下写未映射
  0x0905_0000 → 二次 VM fault，审计/计数后续不可达）。
- 修复：MMIO 映射成功才武装复位；软件模式打印判据行
  `WDT unmapped — spin, awaiting external reset` 后 spin（与 ipc_fail_closed
  同语义）。grep 全消费方：main.rs（store）、service.rs（load 正确）、
  panic.rs（本次修复 load）——闭合。

## RT-028②（P2，services panic 路径死锁）
- svc_panic_hook 的 PANIC_LOGGER.lock() → try_lock()——panic 由持锁代码
  （logger 内部/重入）触发时 lock() 死锁连打印都没有；拿不到锁跳过审计，
  保证打印与挂起可达（不变量"panic 路径不引入新阻塞"加强）。

## RT-028①（P2，PANIC_GENESIS 假哈希）
- root-task panic.rs 创世注释诚实化：字节 = ASCII 占位（非 SM3 摘要，可被
  离线伪造）——原注释声称 SM3 哈希实为 ASCII。真实 SM3 创世根由
  boot-manager 装配期提供并经 DurableLog 持久化（WP-30.7 收口）。

## RT-029（P2，supervisor 占位服务名）
- run_ipc_loop 改 Supervisor::new(真实服务名)（handles 提取）；
  new_for_count 标 #[cfg(test)]（生产路径禁止占位名）。SUPERVISOR 故障
  日志服务名 = 真实名（WP-27.4 证据可信度前置）。

## RT-030（P2，allocator 文档漂移）
- 模块文档 512 KiB → 1 MiB（对齐实现，G-R1 决策引用）。

## 回归
- root-task 59 全绿；QEMU 判据（panic 注入无新增 vm fault 行 + spin 判据
  行 + SUPERVISOR 真实服务名）在版本验收统一跑（svc-fault panic 注入为
  33 域 WP-27.4 出证项，本版本以代码走查 + 既有 QEMU 序列（fault 注入 →
  suspending）核验不回归）。
