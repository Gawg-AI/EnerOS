# WP-29.1 修复与负例证据（29 版本 DoD 2：四条门禁各有坏样本被拦）

执行日期：2026-08-11。蓝图坐标：29 §3（GV-01/02/03/04/17）。

## GV-01 config-refs 转阻断

- **修复**：`xtask/src/gate.rs` `command_config_refs` 删除"阶段 0 只告警"短路；
  新增正向检测 `toml_refs_in_line`（代码引用 `configs/<name>.toml` 但文件
  不存在 → FAIL）。形态甄别：显式 `configs/` 前缀或裸词计入；`tools/`、
  `/etc/` 等非 configs 路径排除（避免误报）。
- **负例**：`crates/_gv01_neg.rs` 含 `configs/nonexistent-xyz.toml` 字样 →
  `qa gate config-refs` exit=2 被拦；删除后 exit=0。
- **转阻断后的真实发现**：`configs/dengbao.toml` 被 compliance 测试引用但
  文件不存在（孤儿引用，宣称一致性违规）→ 已补模板文件（见 WP-29.2 证据，
  同提交）。其余 2 处为路径误报（gate-exemptions.toml→tools/、
  config.toml→/etc/），检测逻辑已排除。
- 现状：missing=0，exit 0。

## GV-02 authorized-construction 全行扫描

- **修复**：`xtask/src/gate.rs` `command_mock_leak` authorized-construction 段
  删除 `starts_with(char::is_whitespace) → continue` 短路，改为全行扫描 +
  **模块级 cfg 保护窗口**：`cfg_attr_text` + 括号深度栈 `protected[depth]`，
  向上回溯到最近块声明（覆盖 `#[cfg(test)] mod tests { ... }`、
  `#[cfg(any(test, feature = "mock"))]` 与属性块同行两种形态）。
- **应修项**（转全行扫描后暴露的真实裸调点）：`protocol-abstract/src/contract.rs`
  的契约套件（`run_adapter_contract`/`authorized_command`/`simple_adapter_config`）
  无 cfg 保护位于生产文件 → 整体 `#[cfg(any(test, feature = "mock"))]` 门控
  （消费方全在 cfg(test)，grep 核实 modbus-tcp/rtu/iec104-master）；
  消费 crate dev-dependencies 补 `eneros-protocol-abstract features=["mock"]`
  （cfg(test) 不跨 crate 传播，feature 传播是唯一正解）。
- **负例**：构造 `crates/_gv02_neg`（生产文件裸调 from_parts）→ exit=2 被拦；
  删除后 0 violations。抽样确认扫描真实执行（防恒 PASS 复发）。
- 现状：0 violations，exit 0。4 个既有调用点（iec104-slave auth_for /
  dispatcher test_cmd / runner lib 等）均在 cfg(test) 内未误伤。

## GV-03 perf-gate 方向修正

- **修复**：`xtask/src/qa.rs` 新增 `regressed(measured, baseline,
  higher_is_better, max_regression_pct)`；PerfMetric 增
  `direction: Option<MetricDirection>`（serde default 兼容旧基线），缺省按
  指标名白名单 `default_higher_is_better` 推断；perf-baseline 生成显式
  `direction: Some(HigherIsBetter)`（5 项吞吐）。吞吐退化条件
  `measured < baseline*(1-pct)`，延迟恶化 `measured > baseline*(1+pct)`。
- **负例**：t6b 吞吐实测=基线×0.5 → Err（旧逻辑此处静默放行）；t6d 延迟
  恶化 15%（缺省推断方向）→ Err。
- **正例**：t6c 吞吐×2 → 通过（不误伤）；t5b 签名链闭环仍绿。
- 附：perf 测试环境变量竞争（3 用例并行 set_var ENEROS_PERF_VERIFY_PUBKEY）
  以静态 Mutex 串行化临界区修复。

## GV-04 + GV-17 fuzz 真 harness 入 CI

- **修复**：ci.yml PRNG 步骤改名 `fuzz-smoke-prng` 保留；新增
  `libFuzzer real harness` job（cargo-fuzz + 6 target × 300s，消费
  fuzz/corpus/）；nightly-fuzz.yml 改 `cargo fuzz run fuzz_$t corpus/$t/`
  （-timeout=60 -max_total_time=5400）；崩溃归档 `if: failure()` →
  `if: always()`（GV-17）+ `if-no-files-found: ignore` 保持。
- **host 验证**（GV-04 判据）：本机安装 clang-21 + cargo-fuzz 0.13.2，
  `cargo +nightly-2026-04-04 fuzz run fuzz_boot_protocol corpus/boot_protocol/
  -- -timeout=60 -max_total_time=30` → 输出 `Done 33401 runs in 90 second(s)`
  ✓（libFuzzer 真实执行，非 PRNG）。
- 注意：bin target 名为 `fuzz_<surface>`（fuzz/Cargo.toml [[bin]]），CI 已按
  此映射。

## 回归

- `cargo test -p eneros-xtask`：59 passed，0 failed。
- `cargo test -p eneros-protocol-abstract/-p eneros-modbus-tcp/-p eneros-modbus-rtu/-p eneros-iec104-master`：全绿（契约套件门控后）。
- `cargo xtask gate`（gate 三子命令）全绿：orphan-check / config-refs /
  mock-leak。
