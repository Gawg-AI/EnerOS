# ADR-0028：唯一驱动 seam = eneros-driver-framework（删除 driver-sdk）

> 状态：Accepted（2026-08-04）
> 决策者：产品负责人 + 架构收敛实施（07 蓝图 ARCH-D04 / ARCH-S03）
> 前置：ADR-0025（插件执行模型）；07 蓝图 §1.4
> 审计证据（evidence/arch-convergence/baseline-before.md）：
> - `eneros-driver-framework` 消费者：`crates/drivers/rs485`、`crates/drivers/can`、
>   `crates/protocols/modbus-rtu`（3 个真实消费者，`can/src/driver.rs:159` 实现
>   `DeviceDriver` 为证）；零外部依赖、`#![cfg_attr(not(test), no_std)]`。
> - `eneros-driver-sdk` 消费者：**无**；且其 `register_driver!` 导出
>   `extern "C" -> *mut dyn DriverPlugin`（ADR-0025 已冻结删除）。

## 1. 背景

07 蓝图 C5 确认驱动抽象三套并存：`framework`（v0.43.0 `DeviceDriver`，生命周期/状态/
注册表抽象）、`driver-sdk`（v1.0 `DriverPlugin`，寄存器级 read/write + C ABI 注册宏）、
各驱动 crate 私有 `driver.rs`/`mock.rs` 抽象。ARCH-D04 要求二选一，由真实驱动的接口
需求反推，落选者删除、不保留"历史兼容区"。

## 2. 真实驱动接口需求清单（反推依据）

| 需求 | framework `DeviceDriver` | driver-sdk `DriverPlugin` |
|---|---|---|
| 生命周期状态机（Uninitialized→Ready→Running→Stopped→Dead） | ✅ 内置 6 态 | ❌ 仅 status() 简单枚举 |
| 注册表/能力/句柄（registry/handle 模块） | ✅ 已有 | ❌ 无 |
| 健康检查 | ✅ `health_check() -> DriverHealth` | ❌ 无 |
| 中断处理 | ✅ `handle_irq(irq_id)` | ✅ `handle_irq()` |
| 寄存器级 read/write | 由具体驱动扩展 | ✅（但无消费者） |
| 跨二进制插件导出 | 无（正确） | ❌ `*mut dyn` C ABI（AGT-003 违规） |
| no_std 零依赖 | ✅ | ✅（alloc + heapless） |
| 真实消费者 | ✅ 3 个（rs485/can/modbus-rtu） | ❌ 0 个 |

结论：framework 是**已被真实驱动实现的 seam**；driver-sdk 是 hypothetical seam 且
违反 ADR-0025/AGT-003。保留 framework，删除 driver-sdk。

## 3. 决策

1. **唯一驱动 seam = `eneros-driver-framework`**（`DeviceDriver` + `DriverRegistry` +
   `DriverHandle`/`DriverCapability`）。所有用户态设备驱动实现此 trait。
2. **删除 `crates/drivers/driver-sdk`**（ARCH-S03）：Cargo.toml / workspace members /
   Cargo.lock 引用清零；其可复用价值（wire codec 概念）已由 ADR-0025 覆盖——若未来
   驱动需要 ELF 进程插件化，直接复用共享 wire codec crate（ARCH-S04），不复活 driver-sdk。
3. **驱动私有抽象收敛**：各驱动 crate 内私有 `driver.rs`/`mock.rs` 抽象逐步迁移到
   唯一 seam（每次迁移随对应驱动进入产品闭包完成，07 §1.4）。`MockDriver`（framework
   自带）为契约测试与快速单测用，qemu-prod 门禁仍须在真实 virtio 后端起验证（MASTER §8.4）。
4. **契约测试套件**：framework 定义 seam 层契约测试套件（生命周期状态机、
   非法状态迁移拒绝、注册/注销、句柄权限），mock 与每个真实驱动跑同一套件。

## 4. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 保留 driver-sdk、删除 framework | driver-sdk 零消费者 + `*mut dyn` 违规；删 framework 需重写 rs485/can/modbus-rtu 三处实现，破坏真实闭环 |
| 两套并存（"历史兼容区"） | 07 §6 明令禁止保留落选框架为兼容区；重复抽象正是 C5 要消灭的 |
| 合并为第三套新抽象 | 已存在被真实使用的 framework；新抽象即第四套（C5 恶化） |

## 5. 影响面

- 删除 `crates/drivers/driver-sdk/`（ARCH-S03）；workspace members 与 Cargo.lock 更新。
- `rs485`/`can`/`modbus-rtu` 依赖不变（仍依赖 framework）。
- 出口：07 §5「驱动抽象唯一；落选框架已删除」；`eneros-driver-sdk` 全仓引用清零。

## 6. 追踪

- 蓝图：07 §1.4、§2.2（ARCH-S03）、§3（ARCH-S04）；MASTER §G3 DRV-003。
- 实施：ARCH-S03（删除）、ARCH-S04（共享 crate）。
