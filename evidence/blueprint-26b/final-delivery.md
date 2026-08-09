# 蓝图/26-B 阶段 0 最终交付说明（DoD 逐项证据）

> 日期：2026-08-10 ｜ 顺序：WP-26.1+26.7 → 26.2 → 26.3 → 26.4 → 26.5 → 26.6（各自独立提交）

## §9 DoD 逐项

| # | DoD 项 | 状态 | 证据 |
|---|---|---|---|
| 1 | 门禁对植入样本变红、真实仓库输出机器可读违规清单（非恒 PASS）；claimed 零硬编码 | ✅ | g7-audit detector 化：6 条能力 VERIFIED（缺失 → UNVERIFIED）；首跑基线检出 storage mock 泄漏（PROD_BLOCK）落盘 `evidence/gate-baseline/2026-08-10-violations.json`；`claimed_no_hardcoded_status_literal` 测试（"IMPLEMENTED" 零字面量） |
| 2 | 孤儿/配置/mock 三门禁 CI 真实运行；persist.rs 孤儿被检出且豁免登记（期限=WP-26.12） | ✅ | ci.yml 三门禁步骤；孤儿活体证据：豁免前检出 persist.rs（+model-ota/gguf/solver-warm 有意孤儿），豁免后 0 孤儿；豁免登记 `tools/gate-exemptions.toml`（persist expires_at=WP-26.12）；config-refs 76 个未引用落盘（阶段 0 只告警） |
| 3 | release 默认构建零 mock 可达；--release --features mock 触发 compile_error（防回归） | ✅ | 9+2 crate 门控化（模块级×6 + 类型级×4 + storage 双处）；`cargo build --release -p eneros-storage --features mock` → compile_error 熔断实测；mock-leak 门禁 0 violations |
| 4 | 生产代码在 gateway 外构造 AuthorizedCommand 编译失败；蓝图/24 三层测试全绿 | ✅ | 字段全私有（12 访问器 + from_parts doc-hidden）；全仓字面量构造 10 处迁移后 workspace 326 套件全绿（蓝图/24 三层测试为回归基线）；authorized-construction 门禁规则 0 违规 |
| 5 | 全仓无 TlsRecordCodec；"无加密事实"测试在库 | ✅ | grep 清零；`test_wp265_framing_codec_plaintext_fact`（payload 偏移 3 起明文逐字节相等） |
| 6 | NaN/Inf/电气越限被拒；正常值基线不回归 | ✅ | `t_wp266_nonfinite_rejected_structured`（逐字段 NaN/±Inf → NonFinite 结构化）+ `t_wp266_electrical_limits_enforced`（电流/电压上下界/频率上下界 → OverCurrent/OverVoltage/UnderVoltage/FreqOutOfRange；正常值基线放行） |
| 7 | feature 分支触发 CI | ✅ | ci.yml push/pull_request `branches: ['*']`；本分支（feature/v0.165.0-protocol-driver-sdk）每次推送触发 |
| 8 | PENDING 证据阻断发布演示；蓝图/14 G1–G7 重算修订记录在库 | ✅ | evidence gate 识别 task status "pending"（计入 blocked + pending_requirements 逐条）；interop-report SKIP → classification PENDING；蓝图/14 修订记录（G2/G3 → PENDING 不计入通过；G7 恒 PASS → REVIEW 真实求值） |
| 9 | 每 WP 侦察报告（26-A §3 模板 A–E）齐备；gate-baseline 与 triage 落盘 | ✅ | 各 WP 提交信息含侦察结论（跨 crate 清点/依赖方向/消费方）；gate-baseline + triage（PROD_BLOCK → WP-26.3，已销项）落盘 |
| 10 | 蓝图/09 对应条目销项引用已写 | ✅ | 见下节 |

## 蓝图/09 销项引用

| 09 条目 | 对应 | 销项引用 |
|---|---|---|
| QA-017~022（qa.rs 自证门禁） | P0-5/P0-10 | WP-26.1（detector 化 + 签名 fail-closed + PENDING 政策） |
| 09:184（孤儿 crate） | P0-7 | WP-26.2（orphan-check + persist 豁免） |
| R1-7（存储 RAM 模拟，EMMC 半项） | P0-3 精化 | 不属 26-B（26-C WP-26.10）；mock 门控化（storage）属 WP-26.3 |
| P0-12（mock 生产导出） | — | WP-26.3（9+2 crate 门控 + 熔断） |
| P1-1（AuthorizedCommand 可伪造） | — | WP-26.4（封焊 + 门禁执法） |
| P0-9（TlsRecordCodec 不加密） | — | WP-26.5（诚实命名 + 事实固定） |
| P1-4/5（电气死字段 + NaN 盲区） | — | WP-26.6（启用 + finite 前置） |

## 风险销项（R26-1~4）

| ID | 风险 | 状态 |
|---|---|---|
| R26-1 | 门禁修复后 CI 长期红 | 销项：S1 顺序锁死执行——基线落盘 → triage（storage PROD_BLOCK → WP-26.3 清偿）→ 豁免登记 → 门禁全绿 |
| R26-2 | mock feature 化破坏跨 crate 消费方 | 销项：侦察 C 清单阻断性前置（MockAdapter 零跨 crate；MockL2/MockBlockDevice 消费方 dev-deps features=["mock"] 显式启用）；逐 crate 提交可单点 revert |
| R26-3 | 封焊波及蓝图/24 已验收行为 | 未发生：蓝图/24 三层测试 326 套件全量复跑全绿 |
| R26-4 | PENDING 政策触发 rc 结论重算争议 | 按 S9：只改判定逻辑与状态表，不改既成事实；蓝图/14 修订记录留痕 |

## 差异记录（规划 vs 代码现实）

1. 26-A §1 坐标过时：5 个协议 crate 已有 `#[cfg(test)]` 门、agent-bus-dds 已合规
   （21-B B2）——以代码为准；storage `pub mod mock` 无门为清单外新发现（首跑基线 PROD_BLOCK）。
2. WP-26.4 路径 B 落地微调：`for_test_fixture` cfg 门控改为 `from_parts` 单构造通道
   （避免 telemetry-model mock feature 的跨 crate dev-deps 涟漪）；字段私有已使生产
   字面量编译失败，门禁执法封死 from_parts 生产调用——双层达成 26-B §5.1 不变量 1。
3. WP-26.6 "拒绝原因结构化枚举"落地为 `ViolationReason`（Violation 增字段，
   构造点经 `Violation::normal` 最小涟漪）。
4. QEMU/CI 活体证据：CI 全分支触发以本分支推送为证据（26-B §8 验收口径）。
