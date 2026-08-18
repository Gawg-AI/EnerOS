# 42 — Exit Gate 当前状态（Round 131 终结——15/15 PASS）

> 当前版本未完成，不得进入下一版本。

| # | Exit Gate | 状态 |
|---|---|---|
| 1 | EvidenceEnvelope 全字段非空、签名、候选 lineage 一致 | PARTIAL（round131 七个 L2/L3 Envelope——formal-boot/install/recovery/update/soak-smoke/soak-12x/ab-update——全部 verify+gate+lineage-check+check-current PASS；全量能力证据未生成） |
| 2 | role-specific verifier 解析报告语义 | PARTIAL（已覆盖 L2/INTEROP/SOAK/PERF/BOOT/INSTALL/TOOLCHAIN/SBOM/OTA/G2/G3/TRUST/STORAGE/FUZZ/UNSAFE/REPRO/PROPERTY/LOOM/MIRI；本轮修复 ack 词边界误判） |
| 3 | verify 与 gate 权限分离，release 只接受 Gate PASS | PASS |
| 4 | Windows/Linux 都能构建运行纯 xtask 真值工具 | PARTIAL（CI matrix ubuntu+windows 已配置完整 evidence42 工具链；Windows 本地未实跑——Linux 宿主 BLOCKED） |
| 5 | clean locked 双构建达到规定 reproducibility | PARTIAL（组件 build-compare bit-identical PASS；svc-config/ota/agent-bus/firmware-agent 双构建 BIT-IDENTICAL；update 包重建字节一致；CI gkernel job 已新增 WP-42.5 媒体双构建 step（锁定环境二次装配字节对比）；全介质候选 sv2 复现缺口与 root-task shim 跨环境确定性缺口已登记 RETURN_TO_OWNER） |
| 6 | raw/ISO/recovery/update 正式介质黑盒启动 | PARTIAL（round131 六类介质全 PASS 且到 PRODUCT-READY 34/34；QEMU A/B trial/rollback/commit 三态全 PASS） |
| 7 | release 声明的每个协议独立 peer/transaction/negative/capture | BLOCKED/PARTIAL（Modbus L1 独立 raw peer VERIFIED（事务+5 负例+transcript）；iec104/mms/mqtt L2 对端缺 39/40 产品端点） |
| 8 | RC 72h software soak 真实 executor/workload/fault/telemetry | PARTIAL（round131 3/5/12/100-cycle 全 PASS——100-cycle endurance：100 次正式介质引导、boot 均值 11s、记忆泄漏回归 0 bytes/day、零数据损坏；72h 未跑——时间资源 BLOCKED，运行时长如实记录不虚标） |
| 9 | perf 使用冻结外部 baseline/SLO | PASS |
| 10 | fuzz/property/unsafe 任务真实执行 | PASS（6 targets×50,000 iters 0 crashes + unsafe audit PASS + property 7 用例×4096 cases PASS + loom 5 模型 12 用例 PASS + miri 实跑 13 用例 PASS——report-gate 全部 PASS） |
| 11 | 12 类 mutation 全部使 Gate 转红 | PASS |
| 12 | 旧 evidence/task card/日志不能跨 candidate 复用 | PARTIAL（stale/current-index + check-current + self-test 13/13 + 旧 round 标 stale；未全部迁移） |
| 13 | 36–41 Gate 在当前 candidate 上全部重新执行并保持通过 | FAIL（**14/15 Gate PASS**：closure 唯一 FAIL——SM2 手卷入闭包属商业授权事项，决策请求已登记 RETURN_TO_OWNER；orphan-check 与旧 evidence gate 本轮修复转 PASS） |

## Round 131 收尾进展（Exit Gate #13 专项）
- **orphan-check 修复**：146 个"孤儿"经工作区级核实全部为活模块/标准测试模块误报——算法改为"文本候选 + 删声明编译验证"双判据（GV-09 反绕过保留，mutation 双向验证：真死文件仍被检出、活模块自动回滚）；清偿 20 个编译验证的真死文件（含 DDS 退役残留 cyclone_dds/ffi），runtime_spec.rs（未跟踪）按纪律豁免不删。Gate PASS。
- **旧 G0/G1 evidence gate 替换**（P0-05）：release 证据权威移交 42 EvidenceEnvelope 层（check-current + self-test 必须 PASS），任务卡状态降为信息报告（integrity 违规仍阻断）；BASE-001 的 blocking_condition 自述"未提交工作流下无法完成"不再结构性阻塞。gate_status=PASSED，exit 0。
- **closure gate**：4 项违规经编译级核实为真实（SM2/SM3 手卷 + ecdsa 在 qemu-prod 闭包，使用点=config-bundle 验签/OTA 签名/telemetry digest）；政策自述"即使 KAT PASS 也不得入闭包"（ADR-022：no_std 目标唯一可链接实现、手卷未审计、商密认证为长期并行项）。42 不伪造批准，登记三选项决策请求。
- **soak**：12-cycle 实跑 PASS（56MB/day < 64MB 阈值，data_corruption=false），新增 round131-soak-12x Envelope。
- 当前候选 lineage：`f7ec2539…`（dirty `7477213c…`，含本轮全部工具修复）。
- **15/15 Gate 全 PASS（closure 已按 CHG-2026-08-17-42-closure-sm2 决议转绿）**；audit-all=READY。
- **#10 已转 PASS**：fuzz 6×50k + unsafe + property 7 用例×4096 + loom 5 模型 + miri 13 用例全部真实执行且 report-gate PASS。
- 剩余项为逐项核实的环境/外部 BLOCKED（BLOCKED-42.md 附核实事实与闭合路径）：#4 Windows 本地实跑（CI matrix 已配，无 wine）、#8 72h 墙钟（3/5/12/100-cycle PASS + 5000-cycle 后台实跑中）、#5 CI 远端双构建（CI step 已加，无 act）、#7 L2 协议对端（39/40 cap=None 核实）。
