# ADR-025 — `#[alloc_error_handler]` 语义与 OOM fail-safe 设计（22 W1.1）

- **状态**：Accepted（2026-08-09）
- **决策者**：主开发代理（按 22 §2.W1.1 强约束 + 第一性原理自主裁决）
- **关联**：ADR-0034（sel4 spawn FFI）、`蓝图/22` §2.W1.1 / §1 锁定 6（故障恢复）、`蓝图/22` §8 G-R1 闸门

## 背景

22 §2.W1.1：全仓无 `#[alloc_error_handler]`，64 KiB bump 堆满直接 abort
（`services/allocator.rs`、`root-task/allocator.rs` 512 KiB bump 同）。要求
OOM 不 abort，改为：记录指标（OOM 计数 + 布局）+ 触发 fail-safe。

## 现状侦察（2026-08-09 代码为准）

1. 5 个目标二进制需要 handler：`eneros-root-task`（`main.rs:290` 已有
   panic handler）+ 4 个 EL0 服务 bin（`svc-hello/pi/hog/fault`，每个
   `#[global_allocator]` + `#[panic_handler]` 独立定义）。
2. 全仓 grep `alloc_error_handler` 零命中；`#[alloc_error_handler]` 在本
   toolchain（nightly-2026-04-04）**仍是 unstable**（需
   `#![feature(alloc_error_handler)]`），且 **host/std 构建下与 std 自带
   handler 冲突**（`cargo test` 不可定义自定义 handler）。
3. 串口 `println!`（`runtime/console.rs`）走 `core::fmt` + MMIO，**无分配**
   ——OOM handler 内可安全打印。
4. 分配热点（网络/I/O 路径）：`mqtt/packet.rs:199` decode_string 的
   `to_vec()`；`tpm/driver.rs:447` 响应循环 `push`（无上限，W1.5 治理）；
   `ipc-wire/mr.rs` frame 编解码（IPC 热路径，Result 返回）。

## 决策

1. **handler 语义 = 记录 → 打印 → 挂起（fail-closed），不直接喂狗重启**。
   `#[alloc_error_handler]` 契约要求 `-> !` 不返回；挂起后由看门狗兜底
   （W1.2/W1.6 接入，蓝图 §1 锁定 6 分级重启链最后一环）。挂起优于 abort：
   进程环境保留、串口可观测、不破坏 seL4 隔离。
2. **指标通道 = runtime 静态原子**（`OOM_COUNT` + `OOM_LAST_LAYOUT`
   打包 size<<32|align），零依赖零分配；W4.1 metrics 导出时桥接
   （不做进程内 MemoryMetricsStore 依赖——OOM 时分配器已不可信）。
3. **共享逻辑放 `eneros_runtime::oom`**（root-task 与 services 的共同依赖，
   console 同层）；5 个二进制的 lang item 各自一行调用
   `oom_fail_safe(layout)`。
4. **try_* 改造选"返回 Result 的 decode 路径"，encode 路径标注豁免**：
   - 真改（5 处）：`mqtt decode_string` / `decode_subscribe` /
     `decode_suback` / `decode_unsubscribe`（新增 `MqttError::OutOfMemory`）、
     `ipc-wire mr::frame_from_mrs`（新增 `FrameMrError::OutOfMemory`）。
   - 豁免标注（3 处）：`mqtt encode()`（签名 `-> Vec<u8>` 无错误通道，
     改 API 破坏 3 个调用方）、`tpm send_command` 响应循环（容量上限属
     W1.5 治理范围）、`svc_ipc_loop` MR 缓冲（seL4 MR 数有界 <128）。
5. **行为验证 = aarch64 裸机探针 `eneros-oom-probe`**（零依赖新 crate，
   QEMU `-machine virt -kernel` 直跑）：4 KiB bump 堆耗尽 → handler 打印
   `[oom-probe] alloc_error_handler triggered #1 layout=size:1000,align:1` →
   挂起不 abort。host 无法直接验证（std handler 冲突），QEMU 全系统注入
   （svc-hog OOM）属 G-R1 闸门验收项（root-task 主循环需加 hog 请求序列，
   避免 W1.1 触碰 G-KERNEL evidence）。

## 影响

- 堆耗尽从"abort 变砖"变"可观测挂起"：OOM 计数/布局入指标，串口可见，
  watchdog 接入后自动重启（W1.2/W1.6）。
- `MqttError`/`FrameMrError` 各增 1 变体（`OutOfMemory`），调用方用
  `?`/match 传播，无 exhaustive match 破坏（已核查）。
- 新 workspace 成员 `eneros-oom-probe`：host 构建为空壳
  （`cfg(target_os="none")` 门控），不进 release 镜像面；构建/运行脚本
  `tests/oom-probe.sh`。
- G-R1 闸门验收补充项：QEMU 全系统注入 svc-hog OOM（root-task 发
  `hog=<n>` 请求）→ 日志见 `[oom]` 记录 + 服务挂起不 abort。

## 备选方案

- **host 集成测试自定义 handler**：不可行——std 自带 alloc_error_handler
  （`conflicts with allocation error handler in: std`，实测编译错误）。
- **QEMU 全系统验证**（W1.1 内）：需改 root-task 主循环加 hog 序列 →
  破坏 G-KERNEL evidence 哈希锚定，属 G-R1 闸门工作，不在 W1.1 范围。
- **handler 内接 MemoryMetricsStore**：OOM 时堆不可信，任何分配都可能
  再触发 handler（递归）；否决。
