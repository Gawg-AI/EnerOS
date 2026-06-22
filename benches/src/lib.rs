//! EnerOS 统一性能基准体系 (T030-06)
//!
//! 本 crate 汇聚 EnerOS 各子系统的 criterion 基准测试，提供统一的性能基线
//! 管理与回归检测入口。基准测试文件位于 `benches/` 目录下，每个文件对应
//! 一个独立的 criterion 可执行目标（`harness = false`）。
//!
//! ## 基准测试目标
//!
//! | 文件 | 子系统 | 关键路径 |
//! |------|--------|----------|
//! | `scada_bench` | SCADA 数据采集 | refresh / collect / store |
//! | `agent_bench` | Agent 决策 | 感知 / 决策 / 执行 |
//! | `ha_bench` | 高可用同步 | 心跳 / 状态同步 / 故障检测 |
//! | `api_bench` | API 响应 | GET /agents, /topology, POST /actions |
//! | `powerflow_bench` | 潮流计算 | IEEE-14 / IEEE-118 求解 |
