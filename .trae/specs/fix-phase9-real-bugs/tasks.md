# Tasks

## P0: 修复真实 Bug

- [x] Task 1: 修复 await_holding_lock
  - [x] 1.1: AgentEventHandler.agent 从 parking_lot::Mutex 改为 tokio::sync::Mutex
  - [x] 1.2: handle_with_context/handle 使用 lock().await
  - [x] 1.3: DataDrivenAgentLoop::start() 从 std::thread::spawn 改为 tokio::spawn
  - [x] 1.4: DataPipeline::start() 已使用 tokio::spawn，无需修改
  - [x] 1.5: clippy 无 await_holding_lock 警告
  - [x] 1.6: 282 测试通过

- [x] Task 2: 修复 SelfHealingAgent 空壳
  - [x] 2.1: locate_fault_section 接收 &NetworkGraph 替代 Option<&()>
  - [x] 2.2: 实现基于 NetworkGraph 的真实故障区段定位
  - [x] 2.3: heal_fault() 调用 InterlockingRuleEngine.check() 校验每个操作
  - [x] 2.4: 联锁硬约束阻止时返回 EnerOSError::Safety
  - [x] 2.5: 6 个新测试（拓扑定位+联锁通过/旁路/硬约束）

- [x] Task 3: 修复 Y-bus 计算 bug
  - [x] 3.1: base_kv <= 0 时提前返回零导纳
  - [x] 3.2: 3 个 ZIP 负荷模型测试
  - [x] 3.3: 潮流计算结果不受影响

## P1: 修复设计缺陷

- [x] Task 4: 修复消息系统广播
  - [x] 4.1: 实现 MessageStore + 游标机制
  - [x] 4.2: broadcast_message() 支持广播
  - [x] 4.3: receive_messages() 基于游标，不消费消息
  - [x] 4.4: 18 个新测试（广播/直达/游标/清理）

- [x] Task 5: 消除重复代码
  - [x] 5.1: SimulatedDataSource 提取到 eneros-scada/src/simulated.rs
  - [x] 5.2: IEEE 14 配置提取到 eneros-scada/src/ieee14.rs
  - [x] 5.3: 矩阵求逆提取到 eneros-core/src/linalg.rs
  - [x] 5.4: 替换 5 处重复矩阵求逆代码
  - [x] 5.5: main.rs 和 e2e_integration.rs 使用公共模块
  - [x] 5.6: ~320 行代码减少

## P2: 清理

- [x] Task 6: 修复 clippy 警告和死代码
  - [x] 6.1: 修复 needless_range_loop（linalg.rs 允许+矩阵操作允许）
  - [x] 6.2: 清理未使用字段（protocols, next_id, path）
  - [x] 6.3: 清理未使用函数（parse_iec61850_path）
  - [x] 6.4: 清理未使用导入（ProtocolConfig）
  - [x] 6.5: 修复 redundant_closure, derivable_impls, type_complexity 等
  - [x] 6.6: cargo clippy --workspace 零警告

## 全局验证

- [x] Task 7: 全局验证
  - [x] 7.1: cargo test --workspace 全部通过（800+ 测试）
  - [x] 7.2: cargo clippy --workspace 零错误零警告
  - [x] 7.3: 更新 README.md 路线图 Phase 9

# Task Dependencies
- [Task 1, 2, 3] 可并行
- [Task 4, 5] 可与 Task 1-3 并行
- [Task 6] depends on [Task 2]
- [Task 7] depends on [Task 1-6]
