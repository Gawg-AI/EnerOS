# Tasks — 刚性子版本 v0.9.1 / v0.12.1 / v0.12.2 / v0.17.1

> **依赖关系**：Task 1 / Task 2 / Task 4 可并行；Task 3 依赖 Task 2 完成；Task 5 依赖前 4 个任务全部完成

---

## Task 1: v0.9.1 — 横向隔离合规路径

实现基于 36 号文的双分区横向隔离合规验证，采集 v0.9.0 隔离证据并形成 Go/No-Go 结论。

- [x] SubTask 1.1: 创建 `crates/kernel/mm/src/isolation/mod.rs`，定义模块入口与公共类型 re-export
  - 定义 `ComplianceResult`（Go/NoGo 枚举）、`IsolationEvidence`、`BomImpact` 结构体
  - 使用 `heapless::String<256>` + `heapless::Vec<&'static str, 8>`（D1 决策）
  - 验证：`cargo build -p eneros-mm` 通过
- [x] SubTask 1.2: 创建 `crates/kernel/mm/src/isolation/compliance.rs`，实现合规路径验证逻辑
  - 实现 `verify_horizontal_isolation()`：采集证据 → 四项检查 → 返回 Go/NoGo
  - 实现 `collect_isolation_evidence()`：从 v0.9.0 Partition 采集隔离证据（物理隔离/capability/单向流/形式化验证）
  - 实现四项检查的判定逻辑（至少三项满足 → Go）
  - 验证：单元测试覆盖 Go/NoGo 两条路径
- [x] SubTask 1.3: 创建 `crates/kernel/mm/src/isolation/audit.rs`，实现隔离证据采集与报告生成
  - 实现 `generate_compliance_report()`：生成 `ComplianceReport`（含 PartitionInfo、数据流验证、36 号文条款引用）
  - 实现 `writeback_bom()`：NoGo 时回写 BOM 影响
  - 验证：单元测试覆盖报告生成与 BOM 回写
- [x] SubTask 1.4: 修改 `crates/kernel/mm/src/lib.rs`，注册 `pub mod isolation;`
  - 验证：`cargo build -p eneros-mm` 通过
- [x] SubTask 1.5: 修改 `crates/kernel/mm/Cargo.toml`，添加 `heapless` 依赖，版本号升至 0.9.1
  - 验证：`cargo metadata --format-version 1 > /dev/null` 通过
- [x] SubTask 1.6: 创建 `configs/compliance/isolation-policy.toml`，定义双分区策略配置
  - 包含：分区 A/B 内存基址、capability root、36 号文条款引用、Go/NoGo 阈值
- [x] SubTask 1.7: 创建 `docs/kernel/horizontal-isolation-compliance.md`，合规路径书面结论文档
  - 包含：合规依据、证据链、Go/No-Go 判定流程、BOM 影响分析、签字栏
- [x] SubTask 1.8: 添加单元测试
  - `verify_horizontal_isolation` 在 mock 证据下分别返回 Go / NoGo
  - `writeback_bom` 在 NoGo 下正确更新成本
  - 证据采集可复现（相同输入 → 相同输出）
  - 验证：`cargo test -p eneros-mm` 全部通过（58 tests passed, 0 failed）

## Task 2: v0.12.1 — 北斗授时

集成北斗 GNSS 接收模块，通过 1PPS + NMEA 报文配对授时，同步精度 < 100ns。

- [x] SubTask 2.1: 创建 `crates/drivers/time/src/beidou/mod.rs`，北斗驱动入口
  - 定义 `TimeStamp`（BDT 纳秒时戳）、`FixQuality`、`SyncError`、`BeidouState` 结构体
  - 定义 `beidou_sync()` / `on_pps_pulse()` 公共接口
  - 验证：`cargo build -p eneros-time` 通过
- [x] SubTask 2.2: 创建 `crates/drivers/time/src/beidou/nmea.rs`，NMEA 0183 报文解析
  - 实现 `parse_nmea(line: &[u8]) -> Result<NmeaMessage, SyncError>`
  - 支持 `$GNZDA`（时间报文）与 `$GPRMC`（定位报文）
  - 校验和验证（XOR）、异常报文处理（截断、非法字段、校验和错）不 panic
  - 验证：单元测试覆盖正常/异常报文
- [x] SubTask 2.3: 创建 `crates/drivers/time/src/beidou/pps.rs`，1PPS 中断处理与时钟 disciplining
  - 实现 `on_pps_pulse()`：捕获硬件时戳，与 NMEA 配对计算钟差
  - 实现 `discipline_clock()`：PI 控制器微调单调时钟斜率，避免时钟回退
  - 1PPS 抖动 < 100ns（通过绑核 + 高优先级中断保证，host 侧 mock）
  - 验证：单元测试覆盖 PPS 配对与钟差计算
- [x] SubTask 2.4: 修改 `crates/drivers/time/src/lib.rs`，注册 `pub mod beidou;`
  - 验证：`cargo build -p eneros-time` 通过
- [x] SubTask 2.5: 创建 `configs/time/beidou.toml`，北斗配置
  - 包含：UART 波特率（9600/115200）、1PPS GPIO 引脚、闰秒表、BDT 偏移
- [x] SubTask 2.6: 创建 `docs/drivers/beidou-time-sync-design.md`，北斗授时设计文档
  - 包含：1PPS+NMEA 配对原理、PI 控制器设计、闰秒处理、降级策略
- [x] SubTask 2.7: 添加单元测试
  - NMEA 解析（正常 + 校验和错 + 截断 + 非法字段）
  - 闰秒边界处理
  - PPS 配对与钟差计算
  - 验证：`cargo test -p eneros-time` 全部通过（79 tests passed, 0 failed）

## Task 3: v0.12.2 — 守时与时钟冗余（依赖 Task 2）

实现北斗失锁时的 OCXO + RTC 守时，24h 漂移 < 1ms，三源故障切换。

- [x] SubTask 3.1: 创建 `crates/drivers/time/src/holdover/mod.rs`，守时状态机
  - 定义 `HoldoverStatus`、`ClockSource`、`HoldoverQuality`、`ClockPriority` 结构体
  - 实现 `holdover_quality()`：查询当前守时质量
  - 实现守时状态机（北斗主授时 → OCXO 守时 → RTC 降级）
  - 验证：`cargo build -p eneros-time` 通过
- [x] SubTask 3.2: 创建 `crates/drivers/time/src/holdover/ocxo.rs`，OCXO 频率补偿模型
  - 定义 `OcxoModel`（freq_offset_ppb、temp_coeff、last_calibration）
  - 实现 `extrapolate_time()`：基于 OCXO 模型推算时间
  - 线性漂移模型 + 温度补偿
  - 验证：单元测试覆盖 24h 漂移推算（< 1ms）
- [x] SubTask 3.3: 创建 `crates/drivers/time/src/redundancy.rs`，三源故障切换
  - 实现 `evaluate_sources()`：评估各时钟源健康度（北斗/OCXO/RTC）
  - 实现 `switch_clock_source()`：强制切换时钟源（需授权）
  - 自动故障切换逻辑（健康度评分驱动）
  - 切换瞬间平滑过渡，不产生时钟跳变
  - 验证：单元测试覆盖三源切换与平滑过渡
- [x] SubTask 3.4: 修改 `crates/drivers/time/src/lib.rs`，注册 `pub mod holdover;` + `pub mod redundancy;`
  - 验证：`cargo build -p eneros-time` 通过
- [x] SubTask 3.5: 修改 `crates/drivers/time/Cargo.toml`，版本号升至 0.12.2
- [x] SubTask 3.6: 创建 `configs/time/holdover.toml`，守时配置
  - 包含：OCXO 漂移参数、温度系数、切换阈值、24h 精度阈值
- [x] SubTask 3.7: 创建 `docs/drivers/holdover-redundancy-design.md`，守时与冗余设计文档
  - 包含：三源冗余架构、OCXO 漂移模型、健康度评分、平滑切换算法
- [x] SubTask 3.8: 添加单元测试
  - OCXO 漂移模型推算（24h < 1ms）
  - 健康度评分算法
  - 三源自动切换无时钟跳变
  - RTC-only 降级模式时标单调递增
  - 验证：`cargo test -p eneros-time` 全部通过（117 tests passed, 0 failed）

## Task 4: v0.17.1 — Edge Box 电源管理

实现掉电检测、UPS/超级电容 ride-through、紧急 checkpoint、优雅关机序列。

- [x] SubTask 4.1: 创建 `crates/drivers/power/Cargo.toml`，新 crate 配置
  - 包名 `eneros-power`，版本 0.17.1，no_std
  - 依赖：`heapless`（固定容量容器）、`spin`（自旋锁）
  - 验证：`cargo metadata --format-version 1 > /dev/null` 通过
- [x] SubTask 4.2: 创建 `crates/drivers/power/src/lib.rs`，电源管理入口
  - 定义 `PowerDownSequence`、`ShutdownStage`、`PowerEvent`、`PowerState`、`CheckpointError` 结构体
  - 定义 `on_power_loss()` / `advance_sequence()` / `current_state()` 公共接口
  - `#![cfg_attr(not(test), no_std)]`
  - 验证：`cargo build -p eneros-power` 通过
- [x] SubTask 4.3: 创建 `crates/drivers/power/src/detect.rs`，掉电检测与中断处理
  - 实现 `register_power_irq()`：注册掉电中断回调
  - ADC 比较主电源电压 + GPIO 中断（双路冗余，aarch64 代码 cfg-gated）
  - 掉电检测延迟 < 10ms
  - 验证：单元测试覆盖检测逻辑（host 侧 mock）
- [x] SubTask 4.4: 创建 `crates/drivers/power/src/sequence.rs`，关机序列状态机
  - 实现 `advance_sequence()`：状态机推进（Detect → RideThrough → Checkpoint → GracefulShutdown → HardOff）
  - 实现 `emergency_checkpoint()`：紧急刷盘（调用方提供回调，power crate 不依赖 FS）
  - ride-through 预算估算与超时兜底
  - 主电恢复时取消关机序列
  - 关机序列不可被普通任务取消（授权检查）
  - 验证：单元测试覆盖所有状态转换
- [x] SubTask 4.5: 修改根 `Cargo.toml`，workspace members 添加 `"crates/drivers/power"`
  - 验证：`cargo metadata --format-version 1 > /dev/null` 通过
- [x] SubTask 4.6: 创建 `configs/power/sequence.toml`，关机序列配置
  - 包含：ride-through 预算（ms）、各阶段超时、checkpoint 回调配置
- [x] SubTask 4.7: 创建 `docs/drivers/edge-box-power-design.md`，电源管理设计文档
  - 包含：掉电检测原理、ride-through 预算、关机序列状态机、checkpoint 完整性、多核协调
- [x] SubTask 4.8: 添加单元测试
  - 关机序列状态机转换（所有路径）
  - ride-through 预算计算
  - 主电恢复取消关机
  - 普通任务取消关机被拒绝
  - 验证：`cargo test -p eneros-power` 全部通过（27 tests passed, 0 failed）

## Task 5: 集成校验（依赖 Task 1~4 全部完成）

全 workspace 构建校验与 CI 配置更新。

- [x] SubTask 5.1: 修改 `Makefile`，VERSION 注释更新覆盖 4 个子版本
- [x] SubTask 5.2: 修改 `.github/workflows/ci.yml`，交叉编译覆盖新 power crate
  - 添加 `cargo build -p eneros-power --target aarch64-unknown-none` 步骤
- [x] SubTask 5.3: 修改 `ci/src/gate.rs`，注释更新覆盖 4 个子版本
- [x] SubTask 5.4: 运行 `cargo fmt --all -- --check`
  - 验证：无格式错误（exit 0）
- [x] SubTask 5.5: 运行 `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings`
  - 验证：无 warning（exit 0）
- [x] SubTask 5.6: 运行 `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello`
  - 验证：全部测试通过（exit 0）
- [x] SubTask 5.7: 运行 `cargo deny check advisories licenses bans sources`
  - 验证：cargo-deny 本地未安装（degraded 模式，CI 中自动安装）
- [x] SubTask 5.8: 交叉编译验证
  - `cargo build -p eneros-mm --target aarch64-unknown-none` ✅（v0.9.1）
  - `cargo build -p eneros-time --target aarch64-unknown-none` ✅（v0.12.2）
  - `cargo build -p eneros-power --target aarch64-unknown-none` ✅（v0.17.1）
  - 验证：3 个 crate 交叉编译通过

---

# Task Dependencies

- Task 1（v0.9.1）：独立，可与 Task 2 / Task 4 并行
- Task 2（v0.12.1）：独立，可与 Task 1 / Task 4 并行
- Task 3（v0.12.2）：**依赖 Task 2 完成**（守时依赖北斗授时接口）
- Task 4（v0.17.1）：独立，可与 Task 1 / Task 2 并行
- Task 5（集成校验）：**依赖 Task 1~4 全部完成**
