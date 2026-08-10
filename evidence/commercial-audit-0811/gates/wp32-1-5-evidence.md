# 32 版本修复与证据（WP-32.1~32.5，11 项）

执行日期：2026-08-11。蓝图坐标：32 §3~§7。

## WP-32.1 AI-03（P0-17 闭环）
- fast-path execute 增 Fatal 闸（validation 不过 → SafetyFatal Err）；coordinator
  快路径传播 DualBrainError::SafetyFatal（与慢路径同语义）。FastPathError 增
  SafetyFatal(usize)。

## WP-32.2（AI-02/AG-02/AG-04）
- AI-02：慢路径成功产出 → last_successful_schedule 赋值（旧实现零赋值，
  降级模式永不生效）。
- AG-02：学习归一化（相对偏差 rel_bias）+ correct() 接线消费（LP 输入
  corrected_price——旧实现"只学不用"）。
- AG-04：coordinator 增 audit 字段 + with_audit 注入；慢路径决策记录
  DecisionAudit（path/validation/gate 字段；W5.1 接线）。

## WP-32.3（AG-01/AI-04/AI-06/AI-08）
- AG-01：错误分级——SafetyBlocked 立即 Error；Transient（限流/通信）N=3
  次连续才 Error，成功归零（t24 更新为新语义判据）。
- AI-04：InferScheduler::submit 背压（MAX_QUEUE_DEPTH=64 → QueueFull）；
  coordinator 按降级处理。
- AI-06：PathSelector load_change_threshold 接线（负荷跳变 → SlowPath）。
- AI-08：coordinator 重复 cfg 属性删除。

## WP-32.4 AI-05（LP 守恒）
- SOC 变量 n+1（soc[n] 末态）；守恒约束从 t=0 起（时段 0 充放电进约束——
  免费能量窗口关闭）；soc_final 指向 soc[n]。变量/约束计数断言更新
  （96→97/95→96 等——修复后解可能更贵属预期）。

## WP-32.5（AG-03/AI-07）
- AG-03 决策 B（默认）：5 个市场/优化 toml 标注 `# status: deferred (v1.x)`
  + dead-surface 登记（34 验收不得声称该能力）。
- AI-07：run_once 增 chain_key 参数（≥16B 校验）；CLI --chain-key-file
  fail-closed（缺省拒绝启动）；测试用固定测试密钥。

## 封焊面复核（DoD 1）
- agents 域生产代码零 ControlCommand 直接构造（grep 复核）；sink.write
  唯一调用点未新增旁路。

## 回归
dual-brain 56 / fast-path 22 / infer-scheduler 15 / energy-lp-model 26 /
energy-market-agent 207 / ai-runner 构建+测试 全绿（343 合计）。
