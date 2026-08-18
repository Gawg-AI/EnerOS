# 42 — RECON 与历史证据隔离

> 执行版本：42-证据链CI工具与零硬件验收修复蓝图
> 本文件是 WP-42.1 侦察记录；旧 evidence 不作为当前候选 PASS。

## 1. Evidence producer / verifier / gate / schema / signature 现状

| 组件 | 路径 | 职责 | 42 处置 |
|---|---|---|---|
| 旧 G0/G1 evidence | `xtask/src/evidence.rs` | 存在性 + requirement registry gate | 保留为历史门禁；42 新增 EvidenceEnvelope 不混用 |
| QA 门禁 | `xtask/src/qa.rs` | soak/perf/interop/kat/compliance/g7 | 保留；42 增加 role-specific semantic verifier 由 evidence42 gate 消费 |
| 签名工具 | `xtask/src/qa.rs::sign-evidence/verify-evidence-sig` | 侧车 ed25519 签名 | 复用；EvidenceEnvelope 自动 dev 签名 |
| Candidate lineage | `xtask/src/evidence_chain.rs` | 42 新增候选血缘、EvidenceEnvelope artifact/input 完整性、gate-dir | 新 owner |
| 媒体 harness | `xtask/src/qemu_harness.rs` | raw/ISO/recovery 启动 | 保留；42 不重写 |
| CI | `.github/workflows/ci.yml` | Linux 全量 + 新增 Windows/Linux evidence42 矩阵 | 42 新增 portable job |

## 2. CI/xtask/Makefile/setup/toolchain 调用图

- `cargo xtask` 是唯一正式入口；`make` 已被 CI 判为 legacy fail。
- 工具链 pinned: `rust-toolchain.toml` (nightly-2026-04-04), `product/toolchains/pinned-tools.json`。
- 全局 `.cargo/config.toml` 已移除写死的 Linux LLVM/GCC include 路径（42 portability）。
- 第三方 native backend（llama/HiGHS/OpenSSL）不进入通用 evidence binary；xtask 保持纯 host 工具。

## 3. QEMU harness 启动/超时/关机判据

- `qemu_harness.rs` 支持 `--expect PASS|Reject|PowerCut`，要求受控退出；timeout 必须由哨兵/exit code 证明，不能仅靠日志。
- CI gkernel job 已要求 `evidence-complete` 哨兵；缺哨兵的 124 = FAIL。
- 42 的 EvidenceEnvelope gate 进一步要求 `oracle` 含 `qmp/exit/readback` 等语义，防止 marker-only PASS。

## 4. Interop/soak/perf/fuzz/miri/repro 工具

- interop: `tests/interop/*.sh` 当前无真实 peer 时产出 PENDING 并退出 1；42 需要独立 peer 合约，当前保留。
- soak: `tests/soak/soak72.sh` 已删除空 `run_executors(){ :; }`，改为 QEMU software adapter 或 BLOCKED。
- perf: `qa perf-baseline/gate` 已拒绝自基线和 SLO=0；42 新增 `perf-measure` 候选流程、`--slo-threshold`/`--tolerance-pct` 冻结参数，以及 envelope/report-gate 的 PERF 语义检查。
- fuzz: `cargo xtask fuzz` + `cargo-fuzz` 已存在；42 保留。
- repro: `tests/rebuild-ci.sh`、`vrf002::command_build_compare` 已存在。

## 5. 当前 `target/eneros/evidence` 中 source/artifact/profile digest

- 以 `cargo xtask evidence42 lineage --profile qemu-prod --artifact Cargo.lock` 生成当前候选血缘：
  - source_commit 与 dirty_tree_digest 见 `target/eneros/evidence/42/candidate-lineage.json`。
- 旧 `target/eneros/evidence/g0-g1/*.json` 等历史报告不跨 candidate 复用。

## 6. 旧 PASS / task card / 当前 baseline 不匹配项

- 35-00/35-01 已判定 36–41 完成前不得引用旧 PASS。
- 42 的 mutation suite 列出 12 类旧假绿反例，全部必须使 gate 转红。

## 7. Windows/Linux native dependency 与 PATH/WSL 假设

- Windows/Linux 均运行 `cargo xtask evidence42 lineage|envelope|verify|gate|mutation` 纯真值工具。
- 不再依赖 `/usr/lib/llvm-21`、`/usr/lib/gcc/...` 全局路径。
- WSL 仅在 pinned-tools audit 中使用；若命令要求 WSL 则 capability probe + 明确参数。

## 7.1 Round 34 实跑记录

- `target/evidence42/round34-boot/qemu-result.json`：raw normal boot PASS（Round 34 当前候选）。
- `target/round34-soak.json`：QEMU software soak smoke 2 cycles PASS（Round 34 当前候选）。
- `target/evidence42/round34-install/qemu-install-result.json`：installer ISO 当前候选 PASS（Round 35）。
- `target/evidence42/round34-recovery/qemu-result.json`：recovery 介质当前候选 PASS（Round 35）。
- `target/eneros/artifacts/qemu-prod/eneros-qemu-prod-round34-full.enupd.report.json`：当前候选 full update 包生成/验签 PASS（Round 35）。
- Round 35 fuzz smoke：6 targets × 2000 iters，0 crashes。
- Round 37：5 个正式介质/soak Envelope 已按新 dirty digest 重生成，gate-dir 5/5 PASS。
- Round 39：新增 `xtask/src/harness.rs`（InteropPeer/SoakHarnessAdapter traits），并刷新 5 个 Envelope 到新 dirty digest。
- Round 41：在 harness.rs 中新增 `QemuSoakAdapter`，实现 QEMU software soak 的 SoakHarnessAdapter。
- Round 42：在 harness.rs 中新增 `UnavailableInteropPeer`，作为未配置 peer 的 fail-closed 实现。
- Round 44：在 harness.rs 中新增 `ScriptInteropPeer`，用于接入外部独立 peer 脚本并要求 PASS 语义。
- Round 48：report-gate 新增 `service`、`control`、`repro` 三类语义 verifier。
- Round 49：report-gate 新增 `security`、`fault` 两类语义 verifier。
- Round 50：report-gate 新增 `isolation`、`readiness` 两类语义 verifier。
- Round 51：report-gate 新增 `build` 语义 verifier。
- Round 53：report-gate 新增 `bom` 语义 verifier。
- Round 54：report-gate 新增 `graph` 语义 verifier。
- Round 56：report-gate 新增 `media` 语义 verifier。
- Round 57：report-gate 新增 `compliance` 语义 verifier。
- Round 58：report-gate 新增 `kat` 语义 verifier。
- Round 59：report-gate 新增 `proof`、`validation` 语义 verifier。
- Round 60：report-gate 新增 `trace` 语义 verifier。
- Round 61：report-gate 新增 `index` 语义 verifier。
- 对应 Envelope：`evidence/42/round34-formal-boot.json`、`evidence/42/round34-soak-smoke.json`。

## 7.2 BLOCKED 登记

- 见 `evidence/42/BLOCKED-42.md`：closure gate、orphan-check、旧 evidence gate、独立 peer、72h soak、Windows、clean CI。

## 8. 旧 evidence 逻辑 stale index

```text
status=SUPERSEDED
reason=BASELINE_OR_SCHEMA_CHANGED
replacement_run_id=<none until rerun>
```

任何旧 evidence 文件不得被复制进新 `EvidenceEnvelope`。

## 7.3 Round 93 新增工具
- `cargo xtask evidence42 fuzz-report`：生成 `eneros.fuzz-smoke.v1`，`report-gate --kind fuzz` 语义门。
- `cargo xtask evidence42 unsafe-report`：生成 `eneros.unsafe-audit.v1`，`report-gate --kind unsafe` 语义门；当前 unsafe audit FAIL 已登记 RETURN_TO_OWNER。
- `cargo xtask test ci-policy` 已修复 PASS（补齐 evidence42/config/closure/policy/manifest/qa 白名单）。
