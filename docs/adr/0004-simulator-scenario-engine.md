# ADR-0004：采用 TOML 场景脚本引擎

- **状态**：Accepted
- **日期**：2026-06-21
- **决策者**：EnerOS 核心团队
- **相关 ADR**：[ADR-0001](./0001-record-architecture-decisions.md)

## 上下文

EnerOS 需要一个电网仿真器（eneros-simulator），用于验证 Agent 决策、测试保护逻辑、回归测试分析模块。仿真器的核心需求是**可复现的电网场景**：能够在确定的时间线注入故障、改变负荷、触发跳闸，并记录观察点。

场景描述需要满足以下要求：

1. **可复现**：相同场景脚本多次运行产生相同结果，便于回归测试
2. **可读性**：场景内容人类可读，便于编写与审查
3. **版本控制友好**：场景脚本可纳入 Git 管理，diff 清晰
4. **参数化**：支持初始状态参数与动作参数，便于场景复用与变体
5. **时序描述**：电力系统是强时序耦合系统，场景需描述事件的时间线

可选的方案包括：

- **Python 脚本**：灵活但引入 Python 依赖，且脚本行为不可静态分析
- **JSON**：可读性一般，不支持注释，难以描述复杂结构
- **YAML**：可读性好，但缩进敏感易出错，且与 EnerOS 的 TOML 配置体系不一致
- **TOML**：与 EnerOS 配置文件（eneros.toml/init.toml/plugin.toml）格式一致，可读性好，支持注释

## 决策

**采用 TOML 描述时序场景脚本，由 ScenarioRunner 按时间顺序执行事件。**

### 场景脚本结构

场景脚本（`crates/eneros-simulator/src/scenario.rs`）包含以下部分：

```toml
name = "ieee14-line-trip"
description = "IEEE 14 节点系统线路跳闸场景"
duration = 60.0
time_step = 0.1

# 事件按时间顺序排列
[[timeline]]
time = 0.0
action = { type = "observe" }
params = { label = "steady_state" }

[[timeline]]
time = 10.0
action = { type = "line_trip" }
params = { line = "L1-2" }

[[timeline]]
time = 30.0
action = { type = "observe" }
params = { label = "post_fault" }

[initial_state]
load_level = 0.8
gen_output = [1.0, 0.5]
```

### 支持的动作类型

| 动作 | 说明 |
|------|------|
| `inject_fault` | 注入故障（短路、接地等） |
| `clear_fault` | 清除故障 |
| `load_change` | 负荷变化 |
| `generator_trip` | 发电机跳闸 |
| `line_trip` | 线路跳闸 |
| `load_shed` | 负荷切除 |
| `observe` | 观察记录点（记录当前状态快照） |

### 执行模型

`ScenarioRunner` 按 `time` 字段升序执行事件，每个事件携带 `params` 参数。`Observe` 动作在指定时间点记录系统状态快照，用于结果验证。

## 后果

### 正面

- **可读性好**：TOML 语法简洁，支持注释，场景内容一目了然
- **版本控制友好**：纯文本格式，Git diff 清晰，便于审查场景变更
- **与配置体系一致**：与 EnerOS 其他配置文件（eneros.toml 等）格式统一，降低学习成本
- **可复现**：场景脚本完全描述事件时间线，相同输入产生相同结果
- **参数化**：`initial_state` 与 `params` 支持参数化，便于场景复用
- **静态可分析**：TOML 可被工具解析验证，无需执行即可检查场景合法性

### 负面

- **表达力有限**：TOML 不支持循环、条件等逻辑，复杂场景需拆分为多个脚本或通过参数变体实现
- **无法描述动态行为**：场景是预定义的时间线，无法根据运行时状态动态调整后续事件

### 中性

- 对于需要动态逻辑的场景，可通过 Agent 决策驱动仿真，而非在场景脚本中编写逻辑

## 参考

- [eneros-simulator crate](../../crates/eneros-simulator/src/scenario.rs) — 场景脚本引擎实现
- [用户手册 — simulator 命令](../user-manual.md) — 场景脚本运行与验证
- [TOML 规范](https://toml.io/cn/)
