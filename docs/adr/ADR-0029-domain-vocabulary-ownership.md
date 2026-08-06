# ADR-0029：领域词汇归属裁定（TradeDir/Bid/aggregator/DataPoint）+ CONTEXT.md 词汇表

> 状态：Accepted（2026-08-04）
> 决策者：产品负责人 + 架构收敛实施（07 蓝图 ARCH-D05）
> 前置：ADR-0027（upa-model canonical 点模型）
> 审计证据：`enum TradeDir` 2 处（spot-market/realtime-market）、`struct Bid` 2 处
> （spot-market/energy-market-agent）、`pub struct DataPoint` 2 处（upa-model/
> protocol-sdk）、aggregator 概念 4 处（coordinator/cloud-coordinator/dr/twin-global）。

## 1. 背景

市场域与数据域存在"同名不同义"与"同义不同名"两类词汇分裂：`TradeDir`/`Bid` 在多
crate 重复定义（且价格单位不一致）；`aggregator` 一词在 coordinator（VPP 资源聚合
调度）、cloud-coordinator（数据汇聚）、dr（负荷聚合）、twin-global（多域联合仿真
聚合）四处含义各不相同；`DataPoint` 在 upa-model 与 protocol-sdk 各定义一份。
07 蓝图 ARCH-D05 要求建立 `CONTEXT.md` 领域词汇表并完成归属裁定。

## 2. 决策

1. **建立根目录 `CONTEXT.md`** 为领域词汇唯一事实源（单上下文布局，MASTER §0）。
2. **词汇归属裁定**（详见 CONTEXT.md 词条）：

| 术语 | 唯一来源 | 现状（改造前） | 处置 |
|---|---|---|---|
| `TradeDir` | `market-types`（ARCH-M01 新建） | spot-market、realtime-market 各 1 份 | 收敛为 1 份，其余 re-export |
| `TimeSlot` | `market-types` | spot-market 1 份 | 收敛为 1 份 |
| `Bid` | `market-types` | spot-market、energy-market-agent 各 1 份 | 收敛为 1 份（M03 切共享 Bid） |
| 价格 | `YuanPerKwh`/`YuanPerMwh` newtype（market-types） | spot 元/kWh、realtime 元/MWh 裸 f64 注释 | newtype 化（M02），跨 crate 无裸 f64 |
| `DataPoint`/`PointValue`/`Quality` | `upa-model` | upa-model、protocol-sdk 各 1 份 | protocol-sdk 切 re-export（S02） |
| `aggregator` | 无单一来源（4 处语义不同） | coordinator/cloud-coordinator/dr/twin-global | 保留各领域语义，CONTEXT.md 记录归属边界；改名消除同名误导（G02） |

3. **aggregator 归属边界**（G02 落地，本 ADR 冻结语义）：

| 位置 | 语义 | 建议命名（G02 实施） |
|---|---|---|
| `coordinator/src/vpp_aggregator.rs` | VPP 资源聚合与调度（VppResource/AggregatedDispatch） | 保留 `VppAggregator` 语义，文件名即义 |
| `cloud-coordinator/src/aggregator.rs` | 跨域数据汇聚（DataSource→DataAggregator→DataSink） | 保留 `DataAggregator` |
| `dr/src/aggregator.rs` | 需求响应负荷聚合（DrAggregator::aggregate_capacity） | 保留 `DrAggregator` |
| `twin-global` | 多域联合仿真/模型融合（MultiDomainSimulator/ModelFusion） | 不引入 aggregator 字样；如出现则明确为"联合" |

4. **同义词不再混用**：`aggregator` 仅用于"聚合计算"，`coordinator` 仅用于"编排/
   协调"，`federation` 仅用于"跨信任域协作"，`fusion` 仅用于"模型/数据融合"。

## 3. 备选方案与拒绝理由

| 备选 | 拒绝理由 |
|---|---|
| 允许各 crate 独立定义 TradeDir/Bid | 跨 crate 对账（M05）无法基于共享类型；价格单位 1000 倍隐患（M02）无法编译期消除 |
| 以 spot-market 为市场类型唯一源 | spot-market 自身零消费者；market-types 是中立层，可被 spot/realtime/settlement 共同依赖，无循环依赖 |
| 不建 CONTEXT.md，仅靠 ADR | 07 蓝图 ARCH-D05 明确要求 CONTEXT.md 作为词汇表并被后续任务引用 |

## 4. 影响面

- 新建 `CONTEXT.md`（术语条目数 ≥ 15，覆盖本表全部 + 核心协议/驱动/点模型术语）。
- `market-types` crate（M01）作为 TradeDir/TimeSlot/Bid/价格 newtype 唯一来源。
- aggregator 改名（G02）仅在语义确有误导时执行；本 ADR 优先冻结语义边界。
- 出口：07 §5「全仓 `TradeDir`/`Bid` 定义唯一；价格传递无裸 `f64`；`CONTEXT.md`
  存在并被后续任务引用」。

## 5. 追踪

- 蓝图：07 §2.1（ARCH-D05）、§2.4（ARCH-M01~M05）、§2.5（ARCH-G02）。
- 实施：ARCH-M01/M02/M03（market-types + 收敛）、ARCH-G02（aggregator 归一）。
