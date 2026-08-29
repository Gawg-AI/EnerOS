# ADR-0046：掉电持久化按介质分治——协议不收敛、CRC 单源、禁止第四套自筹

- 状态：**Draft（SPEC-SERIES-2026-08-28/C 卷 CQ-2 默认裁决；随卷 Exit 呈 owner
  追认，未签不阻塞）**
- 日期：2026-08-28
- 关联：SPEC-SERIES-2026-08-28/C 卷（WP-C.2）；HYG-001（CRC32 唯一实现归
  abi/ipc-wire）；durable-state 双槽代次（2026-08-28 硬杀撕裂事故当日修复，
  durable-state/src/lib.rs:361 实证注释）；[spec-2026-08-28/INDEX.md](../delivery/spec-2026-08-28/INDEX.md)

## 问题

"跨重启副作用持久化"这一个协议问题在仓内写了多套，各自维护撕裂恢复：
1. `durable-state`（数据状态，双槽代次切换 + AppendOutbox）——收敛点（零
   eneros 依赖纯基元）；
2. `durable-log`（引导自检，唯一消费者 = root-task boot 掉电 probe）；
3. `audit/durable_log`（审计链，自带 RegionIo + 链协议，刻意轻依赖：
   sha2+heapless 零 eneros）；
4. `tsdb` 半写 chunk（CRC 拒读）；
5. `ota-ab` 七态（固件级 + SM2 签名，独立性较有理）。

2026-08-28 当天的硬杀撕裂事故修在 durable-state 双槽——另三套协议一个
不会自动受益，下次实弹可能炸在别处。若不裁决，"第四套协议自筹"会继续。

## 裁决

掉电持久化**按介质分治**（协议不收敛为一套，但边界成文封死自筹通道）：

| 介质 | 承载 | 协议 | 理由 |
|------|------|------|------|
| 数据状态 | durable-state 双槽 + AppendOutbox | 已收敛 | 零依赖纯基元，撕裂修复已落此处 |
| 引导自检 | durable-log on durable-state 基元（重建中，C 卷待窗） | 建在 durable-state | 唯一消费者 boot probe，重写风险面最小 |
| 审计链 | audit durable_log 自有链协议 | **豁免**（轻依赖纪律优先） | audit 零 eneros 依赖是有意纪律；收编代价=打破依赖纯净性 |
| 固件 AB | ota-ab 七态 | 豁免 | 固件级 + SM2 签名，独立演进有理 |

**硬约束**：任何新持久化模块**禁止第四套协议自筹**——需要新格式时必须
（a）建在 durable-state 的 `PersistIo + encode_record` 基元上，或
（b）先经本 ADR 修订明确豁免。closure gate 若能挂 pattern 检查则挂拦截；
不能则冻结册口径登记（ACQ-4）。

**CRC32 单源**：全仓 CRC32 实现唯一 = `abi/ipc-wire::crc32`（HYG-001 收口
10/10）。`fn crc32` 签名处的委托壳是允许的（HYG-001 既有模式），但不允许
任何第二份逐位实现。

## 后果

- 正面：撕裂修复传播到全部数据面消费方；第四套协议有门禁挡；CRC 实现
  单点。
- 代价：audit 与 ota-ab 各自协议继续维护（可辩护的接缝税）；durable-log
  重建期间 boot probe 行为以现测试为基准迁移（C 卷红线 2）。
- 修订路径：本 ADR 属可修订裁决——新介质协议需求经 owner 追认后修订。
