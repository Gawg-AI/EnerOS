# WP-33.2 可观测语义与导出（OB-05/06/07）修复与证据

执行日期：2026-08-11。蓝图坐标：33 §4。

## 0. 侦察报告（开工前落盘）

- **波及面自证**：OB-06 仅 `crates/observability/aiops/src/self_heal.rs`
  （None 执行器回退 Mock 成功）；OB-05 仅
  `crates/observability/slo-alert/src/alert_catalog.rs`（AlertStatus 零引用）；
  OB-07 为 `crates/observability/logging|metrics` 新增 std 门控导出后端 +
  tests/ 端到端用例。均在 33 §1 排斥矩阵 `crates/observability/**` 内。
- **依赖方向**：logging dev-depends metrics（不动方向）；metrics 不新增任何
  依赖；aiops/slo-alert 无新增依赖。OB-07 不引入跨 crate 依赖（扫描器
  各自内联，保持 crate 独立）。
- **消费方清点**：`RuleBasedHealer`/`HealActionExecutor` 全仓零消费者
  （grep 实证），影响面仅 aiops 自身测试；`AlertStatus` 经 slo-alert
  lib.rs re-export（re-export 保持，增补 AlertError 导出）；
  `LogError`/`MetricsError` 新变体 `IoFailed` 无穷举 match 破坏点（grep
  实证），纯增量。
- **二次影响对照**：`to_catalog_json` 增 `"status"` 字段——既有测试仅
  contains 断言，不破；Alert 结构体增 `status` 字段——8 处标准告警
  构造器同步补 `AlertStatus::Firing`；slo-alert 无外部构造者（grep
  实证）。std feature 门控保证默认构建（no_std 面）零编译差异。
- **测试基线**：开工前 aiops 21 / slo-alert 37 / logging 20+4 /
  metrics 16+3 全绿（WP-33.1 提交后状态）。

## OB-06（S2 精神）：aiops 自愈无执行器 fail-closed

- **定位**：`self_heal.rs:66` `executor: Option<...>`，execute() None 分支
  `plan.actions.iter().map(description).collect()` —— 生产路径无执行器时
  谎报自愈成功。
- **修复**：None 分支 → `Err(AiOpsError::HealFailed)`（fail-closed）；
  Mock 回退限 `cfg(any(test, feature = "mock"))` 的显式
  `MockHealExecutor`（新增 `mock` feature + 公开类型，供消费方测试
  装配）；`test_execute_succeeds_without_approval` 改显式注入 Mock；
  新增判据 `test_execute_without_executor_fails_closed` 与
  `test_mock_executor_records_targets`。
- **证据**：`cargo test -p eneros-aiops` → 23 passed（旧 21 + 新 2）。

## OB-05：slo-alert AlertStatus 状态机（决策：实现而非删除）

- **定位**：`alert_catalog.rs:49` AlertStatus（Firing/Acknowledged/
  Resolved/Suppressed）定义后全 crate 零引用死类型（蓝图/09 §3.7 登记）。
- **决策明示**：选"实现 `transition` 状态机"而非删除——目录语义完整化，
  告警状态流转成为可验证能力；矩阵：
  Firing→{Ack,Resolved,Suppressed}；Ack→{Resolved,Suppressed,Firing}；
  Resolved→Firing；Suppressed→{Firing,Resolved}；矩阵外返回
  `AlertError::InvalidTransition`（新错误类型，Display 实现）。
- **配套**：Alert 增 `pub status: AlertStatus`（8 处标准告警初始 Firing）；
  `to_catalog_json` 增 `"status"` 字段；lib.rs 导出 `AlertError`。
- **证据**：`cargo test -p eneros-slo-alert` → 42 passed（旧 37 + 新 5）。

## OB-07：logging/metrics std 门控文件导出（端到端证据链）

- **定位**：日志/指标唯一存储是内存环形缓冲，无任何导出后端
  （蓝图/09 §3.7 登记，TSDB 适配器"本版本不落地"）——日志重启即失。
- **修复**：
  - logging 新增 `FileStorage`（`std` feature 门控）：追加式 JSONL 落盘
    + `read_all`/`query` 读回（自研固定结构 JSON 扫描器，UTF-8 安全，
    支持 `\"` `\\` `\u00XX` 转义）；`LogError::IoFailed`。
  - metrics 新增 `FileMetricsStore`（`std` feature 门控）：追加式 JSONL
    + `get_latest`/`query_range`/`read_all`；`MetricsError::IoFailed`。
  - 端到端用例：`tests/file_export.rs`（两 crate），整文件
    `#![cfg(feature = "std")]`——默认 `cargo test --workspace` 不编译，
    no_std 面零影响；CI 现有 `cargo test --workspace` 步骤保持。
- **证据（host 可跑命令）**：
  - `cargo test -p eneros-logging --features std` → 23 lib + 1 e2e +
    4 integration 全绿（e2e：record→flush→JSONL 恰两行含中文→新实例
    读回逐字段一致→LogFilter 查询命中）。
  - `cargo test -p eneros-metrics --features std` → 18 lib + 2 e2e +
    3 integration 全绿（e2e：采集器直挂文件 store，maybe_flush 落盘
    →新实例 query_range/get_latest 命中；标签往返）。
- **调试记录（次生缺陷修复）**：JSONL 扫描器 `read_fields` 未消费子对象
  收尾 `}`，导致 fields 之后的 trace_id/span_id/agent_id 全丢——已修复
  并补入往返断言（该缺陷为本次新增代码引入，按原缺陷等级 P1 处理并
  留证）。

## 回归与门禁

- `cargo test -p eneros-aiops -p eneros-slo-alert`（默认）全绿；
  observability 全 crate（dashboard/ops-gate/tracing/mgmt-api/
  telemetry-queue/fault-recorder 等）全绿。
- `cargo clippy -p eneros-aiops -p eneros-slo-alert -p eneros-logging
  -p eneros-metrics --all-targets --locked -- -D warnings`（默认）clean；
  同命令 `--features std`（logging/metrics）clean。
- 无新增硬编码物理地址/IRQ/板级常量；无真机依赖。
