# 07 蓝图 §5 Exit Criteria 验证报告（架构收敛出口）

> 分支：feature/v0.165.0-protocol-driver-sdk @ 767eba9 + 本分支未提交改动
> 日期：2026-08-04
> 实施：ARCH-D01~D05 / ARCH-S01~S04 / ARCH-P01~P04 / ARCH-M01~M05 / ARCH-G01~G03
> 对比基准：evidence/arch-convergence/baseline-before.md（改造前）

## 逐项验证

| # | Exit criteria | 结果 | 证据 |
|---|---|---|---|
| EC-1 | release profile 闭包内不存在 `*mut dyn` 跨 C ABI 导出 | ✅ | `rg 'extern "C" fn.*\*mut dyn' crates/` → 0 处（改造前 4 处：driver-sdk lib.rs:321/337、protocol-sdk lib.rs:303/319） |
| EC-2 | `DataPoint`/`PointValue`/`Quality` canonical 定义唯一（upa-model），其余为转换层 | ✅ | `pub struct DataPoint` 仅 upa-model:109；`pub enum PointValue` 仅 upa-model:35；protocol-sdk 已 re-export（S02）；device-agent 旁路删除（P02） |
| EC-3 | `TradeDir`/`Bid` 定义唯一；价格传递无裸 `f64` | ✅ | `pub enum TradeDir`/`pub struct Bid` 仅 market-types（M01/M03）；`pub enum BidSide` 零定义；spot（YuanPerKwh）/realtime（YuanPerMwh）/settlement（YuanPerKwh）/energy（YuanPerKwh）全部 newtype 化（M02），裸 f64 报价价格清零 |
| EC-4 | 驱动抽象唯一；落选框架删除 | ✅ | `eneros-driver-sdk` 目录/workspace member/Cargo.lock/文档全部删除（S03/ADR-0028），唯一 seam = eneros-driver-framework（rs485/can/modbus-rtu 消费） |
| EC-5 | 协议 seam 唯一且被 ≥2 真实适配器实现 | ✅ | `ProtocolAdapter` 实现：MockAdapter + ModbusRtuAdapter（P01）+ Iec104Adapter（P04）；三者跑同一 `run_adapter_contract` 共享套件（contract.rs）；`PointAccessExt` extension trait 承载批量语义（P03） |
| EC-6 | 依赖闭包无异常；`CONTEXT.md` 存在并被引用 | ✅ | CONTEXT.md 建立（D05，约 33 术语条目）；ADR-0029 引用；依赖闭包：driver-sdk/protocol-sdk 原零消费者（S01/S03 后 protocol-sdk 保留为 wire codec 客户端，driver-sdk 删除）；8 个零消费者市场 crate 标记 `[SPECULATIVE]`（M04） |
| EC-7 | 拆分目标文件 < 15KB | ✅ | manager.rs 9922B（store.rs 14477B）、domain_optimizer mod 10281B、vpp_aggregator mod 9633B、chain mod 8822B、slo mod 14275B、whatif mod 6573B（G01/G03，characterization 测试保持） |
| EC-8 | aarch64 cross 仍可编译 | ✅ | `cargo build -p eneros-services -p eneros-drv-svc --target aarch64-unknown-none` Finished（RUSTUP_HOME=/tmp/rustup） |

## 回归

| 项 | 结果 |
|---|---|
| `cargo build --workspace --lib`（host stable） | ✅ 0 error |
| `cargo test --workspace`（host stable） | ✅ 0 FAILED（多次全量运行确认；16 个受影响 crate 汇总 1124 ok / 0 fail） |
| `cargo clippy`（受影响 crate，含 --tests） | ✅ 0 warning |
| aarch64 cross 抽查 | ✅ Finished |

## 新增/变更产物

- 新增 crate：`eneros-market-types`（市场词汇唯一源）、`eneros-ipc-wire`（IPC wire 共享层）、`eneros-protocol-sdk/src/wire.rs`（EPRO wire codec）
- 删除 crate：`eneros-driver-sdk`（+ docs/sdk/driver_sdk_guide.md）
- 新 ADR：ADR-0025（插件执行模型）/0026（协议 seam）/0027（点模型）/0028（驱动 seam）/0029（词汇归属）
- 新文档：CONTEXT.md、docs/sdk/architecture.md
- 共享测试套件：`protocol-abstract::contract::run_adapter_contract`（mock/modbus/iec104 三适配器）
- 拆分：config manager.rs（ConfigStore+facade）、5 个超大文件（interface/impl 分离）

## 已知边界（范围外，07 §0 不触碰）

- `product/closure/*.json` 审计快照保留为改造前证据（xtask 在 CI 重新生成）
- 未列入拆分清单的既有 >15KB 文件（tsn-time/federation/pentest/net/agent-bus-dds/multi_objective/x509 等）不在本次收敛范围
- `[SPECULATIVE]` 市场 crate 不进入 release 闭包（M04）
