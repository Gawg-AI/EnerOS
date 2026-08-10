# WP-29.1 侦察报告（随提交落盘）

对应蓝图：29 §3（GV-01/02/03/04/17）。执行日期：2026-08-11。

## A. 波及面自证（逐坐标 grep 复核）

| 坐标 | 文件 | 现状确认 |
|---|---|---|
| GV-01 gate.rs:272-337 | xtask/src/gate.rs `command_config_refs` | 末尾 :335-336 "阶段 0：只告警不阻断。Ok(())" 确认 |
| GV-02 gate.rs:420-441 | xtask/src/gate.rs `command_mock_leak` authorized-construction 段 | :422 `line.starts_with(char::is_whitespace) → continue` 确认 |
| GV-03 qa.rs:1910 | xtask/src/qa.rs `command_perf_gate` | :1909-1914 `measured > baseline` 才算 delta，方向反 确认 |
| GV-04 ci.yml:222-226 | .github/workflows/ci.yml | "libFuzzer 6 parse surfaces" 实跑 `cargo xtask fuzz $t 10000`（PRNG）确认 |
| GV-04 nightly-fuzz.yml:35 | .github/workflows/nightly-fuzz.yml | 同 PRNG 500k 迭代确认 |
| GV-17 nightly-fuzz.yml:50 | 归档条件 `if: failure()` 确认 |
| GV-17 fuzz/corpus/ | 19 语料文件无 CI 消费确认 |

## B. 依赖方向检查

xtask 为根工作区独立工具 crate；gate.rs/qa.rs 均不导出到生产 crate。修改不引入反向依赖（grep：无 `use xtask` 于 crates/）。

## C. 消费方清点

| 符号 | 实现者 | 调用方 | 处置 |
|---|---|---|---|
| command_config_refs | gate.rs:272 | gate.rs:387 分发；ci.yml/QA 调 `cargo xtask gate config-refs` | 保留签名，仅去短路 |
| command_mock_leak | gate.rs:395 | gate.rs:388 分发 | 保留签名，改扫描逻辑 |
| command_perf_gate | qa.rs:1826 | qa.rs:93 分发；CI perf-baseline→perf-gate | 保留签名，方向修正；PerfMetric 增 direction 字段（serde default 兼容旧基线） |
| PerfMetric | qa.rs:1503 | perf-baseline 生成 + perf-gate + 测试 fixture | 增字段；生成侧显式写入 |
| fuzz_driver.rs | xtask/src/fuzz_driver.rs | `cargo xtask fuzz` | 改名 fuzz-smoke-prng，不删除（降级保留） |

## D. 二次影响对照（S 条目）

- S1：GV-03 为 P0，本 WP 全部关闭。
- S9：门禁修真使"坏版本 PASS"不可复现；负例证据入 evidence/commercial-audit-0811/gates/。
- GV-01 转阻断：预期现有违规清单为空（13 个被引用配置均在注释/文档串且文件存在）——转阻断后跑 gate 实测验证。
- GV-02 全行扫描：首跑拦截数若=0 须人工抽样确认执行（防恒 PASS 复发）；若现有生产代码有裸调 from_parts（cfg 保护外），属应修项登记（29 排斥矩阵约束下评估处理方式）。
- GV-03 修正：CI 同跑 baseline+gate（measured==baseline）delta=0 双向不触发，不误伤既有通过项。

## E. 测试基线快照

- `cargo test -p xtask`：见本目录 gate-baseline-xtask.txt（改动前）。
- `cargo xtask gate`（改动前）：记录退出码与输出摘要。
