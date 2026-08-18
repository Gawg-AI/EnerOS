# 05 — 可观测性 / 工程治理 / 配置一致性域 全量代码级审查

审查日期：2026-08-11　子代理域：observability / governance / config-consistency
审查基准：蓝图/27（§4 S 条目、§13 不可回退清单、§15 WP-27.9/10/11/12、§17 物理证据判据）、蓝图/09 销项台账、docs/v1.0.0-go-no-go.md、docs/22-GR-total-exit-review.md、docs/dead-surface-audit.md

范围：crates/observability/（12 crate）、xtask/（gate.rs 570 行 + qa.rs 2714 行逐行）、ci/、.github/workflows/、fuzz/、根 Cargo.toml/clippy.toml/deny.toml/rust-toolchain.toml、configs/ 81 个 toml、tests/（14 个 sh 与 delivery/interop/soak）

---

## 0. 对蓝图/09 销项台账的事实纠偏（先声明）

| 蓝图/09 旧结论 | 现状核查 | 结论 |
|---|---|---|
| fuzz 语料全为空目录 | `fuzz/corpus/` 有 19 个语料文件 | **已推翻**（已销项属实） |
| fuzz harness 仅 decode 不 panic | 6 个 harness（v0.250.0 C-250）已含往返/结构/差分断言 | **已推翻**（已销项属实） |
| cargo-deny 缺失 fail-open | ci/src/gate.rs:199-201 已改 fail-closed | **已推翻**（已销项属实） |
| metrics increment 丢计数 | metrics.rs:252-262 **原样未修** | 未销项，见 OB-01 |
| mgmt-api 无认证 | auth.rs 已实现 SM2 签名 token（digest+过期+公钥验签） | 已修 |
| dashboard 无渲染 | real_render.rs 真实 SVG 渲染 | 已修 |
| support-bundle 中文乱码 | format.rs lossy UTF-8（v0.246.0） | 已修 |
| rate_limit FIFO 冒充 LRU | rate_limit.rs:70-85 真 LRU（v0.239.0） | 已修 |
| slo_baseline 缺 ContextSwitch | slo_baseline.rs:204-206 已补（v0.241.0 B5-04） | 已修 |
| telemetry-queue 无持久化 | persist.rs 快照+断点+OTLP exporter（v0.238.0） | 已修 |
| logging flush_to 重复写 | 已修，但引入反向新 BUG（丢数据） | 见 OB-02 |
| phy/kunpeng skip 即 PASS | 脚本 exit 0 跳过，但**不被任何 CI/qa/go-no-go 消费**（grep 零命中） | 降级为无害，见 GV-19 |

---

## 1. GV — 治理发现

### GV-01（P0）config-refs 门禁恒不阻断 — 与蓝图/27 §10/§13 直接冲突
- 类别：门禁失效
- 定位：xtask/src/gate.rs:272-337 `command_config_refs`，函数末尾 "阶段 0：只告警不阻断。Ok(())"
- 根因：规则只打印告警永远返回 Ok。蓝图/27 §13 不可回退清单要求 "config-refs 门禁转阻断后全绿"，现状从未转阻断。
- 修复：删除阶段 0 短路；改为收集 violations 后 `Err(format!(...))`；同时补子目录扫描（见 GV-15）。签名：`fn command_config_refs(root: &Path) -> DynResult<()>`，末尾 `if !violations.is_empty() { return Err(...) }`。
- 二次验证：转阻断后预期被拦截的文件——目前 grep 实测只有注释级引用，预期违规清单为空（13 个被引用配置均在 crates 注释/文档串中出现且对应文件存在）。若 CI 转阻断后出现新违规，即为真实孤儿引用，属应拦项。次生风险：低。
- host 验证：`cargo xtask gate config-refs; echo $?` 判据：存在虚构引用（临时在 tests 里加 `configs/nonexistent.toml` 字样）时 exit≠0。
- 冲突声明：**与蓝图/27 §10、§13 冲突**（不可回退项未达成）。

### GV-02（P0）authorized-construction 门禁实质恒 PASS — 只查顶层非缩进行
- 类别：门禁绕过
- 定位：xtask/src/gate.rs:420-441 `command_mock_leak` authorized-construction 段，`if line.starts_with(char::is_whitespace) { continue; }`
- 根因：跳过所有缩进行；而 `AuthorizedCommand::from_parts` 的合法/非法调用必然出现在函数体内（缩进）。规则只可能命中顶层 use/常量行 → 实质恒 PASS。
- 修复：改为全行扫描，检测 `from_parts(` 调用点是否位于 `#[cfg(any(test, feature = "mock"))]` 模块/块之外（复用 qa.rs mock_leak_lines 的窗口法，但窗口需覆盖模块级 cfg 而非仅前一行）；函数签名不变。
- 二次验证：新规则会拦截的现有文件预期清单：grep `from_parts` 生产调用点（crates/agents、crates/security），凡在 cfg 保护外的均会被拦——这正是规则意图，不是误伤；若拦截数=0 需人工抽样确认扫描真的执行（防再次恒 PASS）。次生风险：若既有生产代码确实裸调 from_parts，门禁首次转阻断会红——属应修项。
- host 验证：`cargo xtask gate authorized-construction`（或 qa 等效入口）；构造一个函数体内裸调 from_parts 的临时文件判 exit≠0。
- 冲突声明：与蓝图/27 §13 "门禁非恒 PASS" 冲突。

### GV-03（P0）QA-019 perf-gate 回归方向反了 — 真实退化静默放行
- 类别：判据 BUG
- 定位：xtask/src/qa.rs:1910 附近 `command_perf_gate`：`if metric.measured > metric.baseline_value { delta_pct = ... }`
- 根因：全部 5 项基线指标均为吞吐类（frames/s，越高越好），但代码把 measured>baseline（性能变好）判为回归计算 delta，而 measured<baseline（真实退化）delta=0 恒不触发回归。方向完全反了。
- 修复：按指标方向字段判定：`fn regressed(measured: f64, baseline: f64, higher_is_better: bool, max_regression_pct: f64) -> bool`；吞吐类 `higher_is_better=true` 时退化条件为 `measured < baseline*(1-max_pct)`。为基线 JSON 增加 `direction` 字段（缺省按指标名白名单）。
- 二次验证：修正后现有 CI（同跑 baseline+gate，measured==baseline）仍 PASS（delta=0 双向都不触发），不会误伤既有通过项；只有真实退化开始被拦。次生风险：历史"靠退化混过"的版本将首次现形——属收益。
- host 验证：构造 measured=baseline*0.5 的基线 JSON，跑 `cargo xtask qa perf-gate ...` 判 exit≠0。
- 冲突声明：与蓝图/27 §17 物理证据判据（证据必须真实反映系统行为）冲突。

### GV-04（P0）CI 名义 libFuzzer 实跑 PRNG driver — 真 harness 在 CI 从不执行
- 类别：测试基建失实
- 定位：.github/workflows/ci.yml:222-226（"libFuzzer 6 parse surfaces" 步骤实跑 `cargo xtask fuzz $t 10000`）；xtask/src/fuzz_driver.rs:150-191（固定种子 xorshift64* + 32 步变异 + catch_unwind，无覆盖率反馈）；nightly-fuzz.yml:35 同样跑 PRNG driver（500k 迭代）。
- 根因：fuzz/ 目录 6 个 cargo-fuzz harness（含往返/结构断言）与 19 个语料文件无任何 CI 路径消费；确定性 PRNG 每次运行输入序列完全相同，10k 与 500k 迭代探索的是同一批固定输入，"深 fuzz"名义不成立。
- 修复：CI 增加真实 cargo-fuzz job：`cargo install cargo-fuzz; cargo fuzz run <target> -- -max_total_time=300`（需 nightly+clang，与现有 nightly-2026-04-04 工具链兼容）；PRNG driver 降级为 smoke 并改名；nightly-fuzz 改用 `cargo fuzz run <target> -timeout=60 -max_total_time=5400` 且消费 fuzz/corpus。
- 二次验证：新增 job 不影响既有 job 通过状态；预期 cargo-fuzz 首跑可能在 6 面之外暴露新问题（MQTT/MMS/GOOSE/SV/COMTRADE/TOML 配置解析面仍无 harness，列入缺口）。次生风险：CI 时长 +10~20min。
- host 验证：`cargo +nightly-2026-04-04 fuzz run boot_protocol -- -max_total_time=30`，判据：输出含 "Done ... runs in"。
- 冲突声明：与蓝图/27 §17（证据充分性）冲突；与 ci.yml 自身注释"libFuzzer"表述不符（宣称一致性，G7 精神）。

### GV-05（P1）soak-run 自产证据永远过不了自家 soak 门禁（自证链断裂）
- 类别：证据链 BUG
- 定位：xtask/src/qa.rs:1396-1401 `command_soak_run` 硬编码 `artifact_sha256: String::new(), memory_leak_bytes_per_day: 0, data_corruption: false`；`duration_days: ((total as f64 / 86400.0).max(0.0)) as u64`。
- 根因：① 短时 CI 运行 duration_days 必为 0 < min_duration_days=3，soak-run 产出的证据必然被判不合格；② artifact_sha256 空串且下游 QA-018 从不校验绑定（见 GV-07）；③ memory_leak=0/corruption=false 为断言性硬编码而非测量值。
- 修复：`fn command_soak_run(..., artifact_path: &Path)` 增加产物 sha256 实算（`ring::digest` 或 crypto crate SM3）；memory_leak 从 RSS 采样序列线性回归得出（host soak 可用 /proc/self/status VmRSS）；duration 以实际运行秒数为准并把门禁阈值改为可配置 `--min-duration-days`（CI 短周期用 0 + 显式标注，不得静默）。
- 二次验证：修正后 soak-run + qa soak 形成闭环；现有 evidence/soak*.json 若含空 sha256 会被新校验拦下——预期清单：evidence/ 下既有 soak 证据文件，属应重新生成项。
- host 验证：`cargo xtask qa soak-run --duration 10 ...` 后 `jq .artifact_sha256` 判非空且等于产物 sha256sum。
- 冲突声明：与蓝图/27 §17 判据 1/3（证据含产物指纹、可复核）冲突。

### GV-06（P1）QA-021 `--degrade` 把扫出 sorry 的证明强制改 PASS
- 类别：证据伪造开关
- 定位：xtask/src/qa.rs:2177-2179 `command_proof`：`if report.result != "PASS" { report.pass(); }`（degrade 分支）
- 根因：降级开关语义应是"降级为 WARN 但如实报告"，现状是直接改写结果为 PASS，违反 S9（skip/PENDING 不计入通过）。
- 修复：degrade 分支改为 `report.result = "DEGRADED"`（新增状态），qa 汇总时 DEGRADED 计为非 PASS；函数签名不变。
- 二次验证：若 go-no-go.md 中 QA-021 行曾依赖 degrade-PASS，则 gng-version 会转 -rc 约束——这是正确行为。预期受影响文件：无既有 PASS 证据被撤销，仅未来不再产生假 PASS。
- host 验证：在 formal/ 放一个含 sorry 的 thy，`cargo xtask qa proof --degrade` 判输出 DEGRADED 而非 PASS。
- 冲突声明：与蓝图/27 §4 S9 直接冲突。

### GV-07（P1）QA-017~022 证据链仅文件存在性检查，无密码学/时间绑定
- 类别：证据政策缺口
- 定位：xtask/src/qa.rs:879-883（`present: evidence_dir.join(f).exists()`）；command_compliance_report L767-930：签名可选（无 --sign-key 仅 WARN）、`generated_at: String::new()` 占位；QA-022 reviewer signed/independent_signoff 为自报 JSON 字段。
- 根因：G9 商业资格六项证据均为"文件存在+自报字段"，无 ed25519/SM2 签名验证、无时间戳绑定、artifact_sha256 从不与产物比对。
- 修复：① compliance-report 签名改强制：`fn command_compliance_report(..., sign_key: &Path) -> DynResult<()>` 无 key 即 Err；generated_at 取系统时间 RFC3339。② QA-017~022 校验器增加 `verify_evidence_sig(path, sig_path, pubkey)` 步骤。③ 现有证据补签迁移窗口：一次性对既有 JSON 补签入库。
- 二次验证：强制签名后 CI 需提供 key（仓库密钥走 secret，测试用 dev key 入仓——product/security/dev-signing.key 已有先例）。预期被拦：无签名的既有证据 JSON（evidence/ 下 QA-017~022 产物），需补签后通过。
- host 验证：`cargo xtask qa compliance-report`（无 key）判 exit≠0。
- 冲突声明：与蓝图/27 §4 S9、§17 判据 2（签名证据链）冲突。

### GV-08（P1）mock 门禁覆盖域不全 + mock_leak_lines 三处弱化
- 类别：门禁泛化不足
- 定位：① ci/src/gate.rs:434-482 `mock_gate_at` 只扫 crates/agents + crates/ai；protocols/drivers/observability/security 的 mock 泄漏无人执法。② xtask/src/qa.rs:1026-1065 `mock_leak_lines`：cfg 检查只看前一行且接受任意 `#[cfg(`（非仅 test/mock feature）；`pub(crate) mod mock` / `pub(super) mod mock` 不命中（只查 `t == "pub mod mock"`）；非 lib.rs 文件顶层 `use ...::mock` 不检测（仅 is_lib 分支）。
- 根因：扫描白名单硬编码两个目录；可见性模式匹配过窄；cfg 窗口 1 行且不过滤 cfg 内容。
- 修复：mock_gate_at 扩至全部 crates/* 一级域目录；mock_leak_lines 的 mod 匹配改为 `t.split_whitespace().last() == Some("mock")` 系判定（含 pub(crate)/pub(super)/pub(in ...)）；cfg 窗口扩为向上找最近属性行且必须含 `test` 或 `feature = "mock"`；use-mock 检查对所有 crate 内 .rs 生效。
- 二次验证：预期新增拦截文件清单（需跑一遍确认）：crates/protocols、crates/drivers、crates/security 中若有非 cfg 保护的 mock 引用将被新规则拦——若存在，属真实泄漏应修；若为 0 则证明现状只是覆盖盲区无存量。次生风险：豁免表 tools/gate-exemptions.toml 需同步扩充合法 mock 集成点。
- host 验证：`cargo xtask gate mock-leak`（或 ci gate 等效）+ 临时在 protocols 某 crate 放未保护 mock 引用，判被拦。
- 冲突声明：与蓝图/27 §4 S2（mock 门控范式全域）冲突。

### GV-09（P1）orphan-check 豁免表后缀匹配过松 + 假 mod 声明绕过
- 类别：门禁绕过
- 定位：xtask/src/gate.rs:240 `rel.to_string_lossy().ends_with(key)`（豁免）；gate.rs:12-56 `parse_mod_decls` 逐行正则匹配 `mod x;`。
- 根因：① endswith 使 `foo.rs` 豁免键意外豁免 `barfoo.rs` / 任意路径尾同名文件；② 孤儿文件只要在任一"可达"文件顶层加一行假 `mod orphan_name;`（编译不必引用该 mod）即被 BFS 视为可达——而 parse_mod_decls 不验证 mod 是否真实被编译路径使用。
- 修复：豁免匹配改 `rel == key || rel.to_string_lossy().ends_with(&format!("/{key}"))`；parse_mod_decls 结果与"文件实际存在"绑定之外，增加第二轮校验：被声明的 mod 对应文件必须至少被一个编译入口（lib.rs/main.rs 链）可达且非 `#[cfg(test)]` 独占——最小改动版：对 mod 声明行要求其后（同文件）存在对该 mod 路径符号的 use/引用，否则不计。
- 二次验证：收紧后预期新拦文件清单：现豁免表 tools/gate-exemptions.toml 中若含短名键且仓内有同尾缀文件将现形；假 mod 声明的存量孤儿需跑一次 orphan-check diff 得出清单。次生风险：豁免表需人工复核一轮。
- host 验证：构造 tests/tmp 孤儿 + 顶层假 mod 声明，`cargo xtask gate orphan-check` 判 exit≠0。
- 冲突声明：与蓝图/27 §13（孤儿源扫描不可回退）的精神冲突（可绕过即非不可回退）。

### GV-10（P1）CI 同跑 baseline+gate 使 perf 门禁恒过 + SLO 阈值全 0 永空
- 类别：判据设计缺陷
- 定位：xtask/src/qa.rs perf-baseline/perf-gate 在 CI 同一 run 内先后执行（measured==baseline 恒过）；slo_threshold=0 语义"未冻结即跳过"，CI 基线 5 项 threshold 全 0。
- 根因：基线必须来自独立冻结的历史产物（带签名，见 qa.rs --sign-key 机制），而非本次测量。
- 修复：perf-gate 拒绝接受与本次 run 同时间戳的 baseline（校验 baseline.generated_at 与 measured_at 差 ≥ 门禁配置）；SLO 阈值冻结流程：`cargo xtask qa perf-freeze-slo` 把非零阈值写入基线并签名，qa perf-gate 对 threshold==0 项报 WARN 且计入"未冻结 SLO 数"上报 go-no-go。
- 二次验证：现有 CI 步骤 perf-baseline→perf-gate 链在冻结前会开始报"未冻结"WARN（非 FAIL），不破坏既有绿；冻结后才有真实拦截。
- host 验证：手工置 threshold>0 且 measured 劣化，判 exit≠0。
- 冲突声明：与蓝图/27 §17 判据 5（判据可区分好坏版本）冲突。

### GV-11（P2）ENEROS_PERF_ALLOW_UNVERIFIED 环境变量可跳过签名验证
- 类别：逃生门
- 定位：xtask/src/qa.rs:1875-1883
- 根因：环境变量逃生门使签名验证可被 CI 配置一行绕过，且无审计日志。
- 修复：保留变量但强制写审计行（`PERF_GATE_UNVERIFIED_BYPASS at <sha>`）且 go-no-go 汇总时把该 bypass 计为非 PASS 证据；或直接移除（推荐）。
- 二次验证：移除后若 CI 曾依赖该变量，perf-gate 在缺 key 时转 FAIL——预期需为 CI 配 dev key。
- host 验证：`grep -rn ENERGOS_PERF_ALLOW_UNVERIFIED xtask/ .github/` 判 0 命中（移除方案）。
- 冲突声明：与 S9 冲突（逃生门产生的 PASS 不得计入）。

### GV-12（P2）CI 缺 miri / sanitizers；fuzz 覆盖只有 6 个解析面
- 类别：缺失关卡
- 定位：.github/workflows/ci.yml 全文无 miri/asan（grep 零命中）；fuzz 6 面 = boot_protocol/ipc_wire/modbus_rtu/iec104/elf_loader/gguf_loader。
- 根因：unsafe 面（hal/runtime/elf-loader/ffI 依赖）无动态检测关卡；MQTT/MMS/GOOSE/SV/COMTRADE/TOML 配置解析面（81 个 toml 无语法校验！）无 fuzz。
- 修复：nightly workflow 增 `cargo miri test -p <no_std 纯逻辑 crate 清单>`；增一个 toml-schema 校验步骤（host 侧用 toml crate 解析 configs/** 全部 81 个文件，语法错即 FAIL——xtask 侧实现 `xtask gate config-syntax`）；为 MMS/GOOSE/SV/COMTRADE 补 fuzz harness。
- 二次验证：config-syntax 门禁首跑预期全绿（81 个 toml 应均合法，若有个别语法错即为真实发现）；miri 首跑可能暴露 no_std crate UB——属应修项，先以 allow-list 限定 crate 再逐步扩。
- host 验证：`cargo xtask gate config-syntax`（新增）exit 0；`cargo +nightly miri test -p eneros-metrics` 通过。
- 冲突声明：无直接冲突；属 蓝图/27 §15 WP-27.11（工具链与质量基建）缺口。

### GV-13（P2）工具链 nightly-2026-04-04 与 WP-27.11 stable 目标冲突
- 类别：合规三项（27.11）现状
- 定位：rust-toolchain.toml:6 `channel = "nightly-2026-04-04"`；ci.yml/nightly-fuzz.yml 同钉 nightly。
- 根因：build-std（no_std 编译 core/alloc）依赖 unstable。stable 迁移前提是把内核/hal 拆到独立 profile 用 `-Z build-std`（仍需 nightly）或将 boot 链改用预编译 core。
- 修复：现状作为既成事实记录；迁移路线列入 30 号蓝图（运行时域）与合规项：workspace 拆双 toolchain（用户态 crate stable + 内核链 nightly-pin），clippy.toml msrv=1.75 与 nightly 并存属过渡态。
- 二次验证：无即时改动，无波及。
- host 验证：N/A（规划项）。
- 冲突声明：**与蓝图/27 §15 WP-27.11 "工具链 stable" 冲突**（现状 nightly，迁移有 build-std 技术阻塞，需明示豁免或拆双链）。

### GV-14（P2）compliance-report 判定仅凭单测退出码
- 类别：合规三项（27.10/27.12）现状代码基础
- 定位：xtask/src/qa.rs command_compliance_report：xinchuang/dengbao 判定 = `cargo test -p eneros-compliance --lib` 退出码；crates/security/compliance/src/{dengbao,xinchuang,verifier}.rs 存在。
- 根因：单测通过 ≠ 送检/测评通过；报告把代码自测表述为合规结论存在宣称风险。
- 修复：report 中把字段改名为 `self_test_status`，另设 `external_certification: "PENDING" | "SUBMITTED" | "CERTIFIED"`（人工/流程写入并签名），两者不得混淆。
- 二次验证：仅报告结构变化；消费方（go-no-go 汇总）需同步字段名。预期受影响：compliance-report.json 下游 jq 解析处（xtask qa gng 汇总）。
- host 验证：`cargo xtask qa compliance-report --sign-key <devkey>` 后 jq 检查字段拆分。
- 冲突声明：与蓝图/27 §15 WP-27.10（国密送检）/27.12（等保）的"外部证据"要求存在表述层冲突。

### GV-15（P2）config-refs 扫描范围双重遗漏（子目录 + 反向检查缺失）
- 类别：门禁泛化不足
- 定位：xtask/src/gate.rs:339-353 `config_names()` 只扫 configs/ 顶层（漏 compliance/isolation-policy.toml、power/sequence.toml、time/beidou.toml、time/holdover.toml）；被扫描目标只有 crates/xtask/ci/tests 的 .rs/.sh（漏 tools/、Makefile、.github/）；无"被引用但不存在的配置"反向检查。
- 修复：config_names 递归扫 configs/**；引用扫描加 tools/ 与 .github/workflows/*.yml；增加反向：对每个引用串 `configs/<name>.toml` 校验文件存在。
- 二次验证：反向检查首跑预期违规清单：grep 实测现有引用全部对应真实文件（13 个被引用者均存在），预期 0 新拦；递归扫描使 4 个子目录 toml 纳入保护。
- host 验证：删除某子目录 toml 的临时副本实验（用 tmp 目录模拟）判被拦。
- 冲突声明：无直接条目冲突；GV-01 的配套修复。

### GV-16（P2）G7 宣称审计检测器过弱
- 类别：审计泛化不足
- 定位：xtask/src/qa.rs:1067-1283 `command_g7_audit`：CapabilityDetector::TestPresence 只要求文件含 `#[cfg(test)]` + 能力子串；cap-attestation 只要求 docs/security/attestation-design.md 文件存在。
- 根因："存在性"检测器无法区分实现与占位。
- 修复：TestPresence 升级为"测试存在 + 该测试被 CI 测试 job 覆盖（出现在 cargo test 执行集）"；attestation 能力断言要求 attestation crate 有非 mock 的验证路径测试（grep `verify` 断言行）。此项可延后到 32 号蓝图（Agent/AI 域）联合修。
- 二次验证：升级后 G7 报告可能出现能力降级（如实），go-no-go 相关行可能转非 PASS——属诚实化，非误伤。
- host 验证：`cargo xtask qa g7-audit` 前后 diff 报告。
- 冲突声明：无直接冲突；与 G7 宣称一致性精神一致（修后更严格）。

### GV-17（P2）fuzz corpus 无人消费 + 崩溃归档条件缺陷
- 类别：测试基建
- 定位：fuzz/corpus/ 19 文件无 CI 消费（GV-04 同源）；nightly-fuzz.yml:50 `if: failure()` 才归档崩溃语料——job 被 cancel/超时则语料丢失。
- 修复：corpus 接入 cargo-fuzz（`cargo fuzz run <t> fuzz/corpus/<t>/`）；归档条件改 `if: always()` + `if-no-files-found: ignore`。
- 二次验证：无既有通过项受影响。
- host 验证：`cargo fuzz run boot_protocol fuzz/corpus/boot_protocol/ -- -runs=100`。
- 冲突声明：无。

### GV-18（P2）clippy.toml msrv 与实际工具链脱节
- 类别：配置一致性
- 定位：clippy.toml:10 `msrv = "1.75.0"` vs rust-toolchain.toml nightly-2026-04-04。
- 根因：no_std build-std 链不可能在 1.75 stable 上构建；msrv 声明成空文。
- 修复：与 GV-13 合并处理（双工具链拆分后用户态链定真实 msrv）。
- 二次验证：无即时改动。
- host 验证：N/A。
- 冲突声明：无。

### GV-19（P2）kunpeng/phytium 脚本 skip=exit 0（已核实不被任何门禁消费，降级记录）
- 类别：证据政策（S9）边缘
- 定位：tests/kunpeng_boot.sh、tests/phytium_boot.sh（无硬件标记 exit 0）。
- 根因：skip 即 exit 0 违反 S9 精神，但 grep 核实 .github/、xtask/、ci/、go-no-go.md 均不消费这两个脚本 → 无害。phytium_boot.sh 硬件就绪分支为 TODO 且显式 FAIL，诚实。
- 修复：保持现状 + 在脚本头注释明示"不得计入任何 PASS 证据"；若未来纳入 CI，必须改 exit 77（skip 专用码）且门禁不计入。
- 二次验证：无改动无波及。
- host 验证：`grep -rn kunpeng_boot .github/ xtask/ ci/ docs/v1.0.0-go-no-go.md` 判 0 命中（防回归）。
- 冲突声明：与蓝图/09 旧发现"skip 即 PASS"部分相关——现状无消费方，风险未实化。

### GV-20（P2）probe 脚本硬编码个人主机 QEMU 路径
- 类别：可移植性
- 定位：tests/panic-probe.sh、tests/oom-probe.sh fallback 路径 `/home/ahx/.cache/xpack-qemu-arm-9.2.4-1/bin/qemu-system-aarch64`。
- 根因：开发机路径入仓；换机即断（与 rebuild-ci.sh 的换机可复现目标矛盾）。
- 修复：fallback 改为 `command -v qemu-system-aarch64 || { echo "set ENEROS_QEMU_BIN"; exit 2; }`。
- 二次验证：本机（有该路径）行为不变；无 QEMU 环境从"莫名运行"变为"显式 exit 2"，属正确 fail-closed。
- host 验证：`ENEROS_QEMU_BIN= bash tests/oom-probe.sh` 在无 qemu PATH 环境判 exit 2。
- 冲突声明：无。

---

## 2. OB — 可观测性发现

### OB-01（P1）metrics increment 丢计数 — 蓝图/09 旧发现原样未修
- 类别：BUG（丢数据）
- 定位：crates/observability/metrics/src/metrics.rs:252-262
- 根因：`increment` 读 `store.get_latest(name)` 作基数，但 buffer 中未 flush 的增量不在 store 里——两次 increment 间隔 < flush_interval 时，第二次基数漏掉第一次增量（col10 测试靠每次 increment 后手工 flush 掩盖了该缺陷）。
- 修复：维护每 counter 的内存累计：`pending: BTreeMap`/线性表（no_std：heapless::Vec<(HString<64>, u64), 32>）记未 flush 增量；increment 基数 = store.get_latest + pending[name]；maybe_flush 时清零对应 pending。签名不变。
- 二次验证：col10 等既有测试不受影响（flush 后 pending=0 语义一致）；新增测试：连续 increment 不 flush，断言 flush 后总值 = 累计和。次生风险：低。
- host 验证：`cargo test -p eneros-metrics`（新增用例 inc_no_flush_accumulates）。
- 冲突声明：无；蓝图/09 台账未销项确认。

### OB-02（P1）logging flush_to：append 失败后 buffer.clear() 丢已缓冲日志
- 类别：BUG（丢数据）
- 定位：crates/observability/logging/src/logging.rs:237-248
- 根因：`store.append(...)?` 中途失败时提前 return Err，但失败前已 append 的条目仍在 buffer，下次 flush 会重复写；若走完整循环后 clear 又丢数据——现状是"失败即整批留在 buffer"，重复写风险仍在（蓝图/09 旧发现的变体未根治）：失败 return 后 buffer 未 clear，下次 flush_to 把已成功条目再写一遍。
- 修复：按成功进度收缩 buffer：`for i in 0..count { ...append?; self.buffer.remove(0); }`（逐条成功后移除），失败时已成功的不再重复；或引入 `flushed: usize` 游标，失败时 `self.buffer.drain(..flushed)` 后 return。签名不变。
- 二次验证：log15/log16 既有测试不受影响（全成功路径行为一致）。
- host 验证：新增测试：store 容量 3，flush 10 条，断言 store 恰 3 条且 buffer 剩 7 条（无重复写）。`cargo test -p eneros-logging`。
- 冲突声明：无；修正蓝图/09 "flush_to 重复写"销项状态为"变体仍存"。

### OB-03（P2）telemetry-queue restore 丢失原始容量与溢出策略
- 类别：BUG（语义漂移）
- 定位：crates/observability/telemetry-queue/src/queue.rs:134-147 `TelemetryQueue::restore`
- 根因：恢复时 `capacity = snap.events.len().max(16)*2`、policy 硬编码 DropOldest——原队列若是 RejectNew/DropLowPriority 或容量更大，重启后静默改变溢出语义；snapshot 未序列化 policy/capacity。
- 修复：QueueSnapshot 增字段 `capacity: usize, policy: u8`（persist.rs 序列化行 `#capacity=...`/`#policy=...`，旧快照兼容默认 DropOldest + len*2）；restore 按快照字段构造。
- 二次验证：旧格式快照按兼容默认解析，既有 persist 测试不破；新格式多两行头。预期受影响文件：telemetry-queue/src/persist.rs 及其测试。
- host 验证：`cargo test -p eneros-telemetry-queue`（新增 restore_preserves_policy 用例）。
- 冲突声明：无。

### OB-04（P2）fault-recorder 录波时间戳 u32 截断（>71 分钟溢出）
- 类别：BUG（溢出）
- 定位：crates/protocols/fault-recorder/src/lib.rs:336 `timestamp_us: ts[i].wrapping_sub(base_ts) as u32`
- 根因：相对首帧微秒数存 u32，上限 ~4295s≈71.6 分钟；长录波窗口（pre+post 可配置）超限时 COMTRADE 时间戳回卷。
- 修复：COMTRADE cfg 时间列改 u64 输出（格式本就是文本十进制，无兼容问题）：字段类型改 `timestamp_us: u64`，comtrade_writer.rs 输出处同步。
- 二次验证：comtrade_writer 解析方若做 u32 断言需同步；预期受影响：fault-recorder/tests。
- host 验证：`cargo test -p eneros-fault-recorder`（新增窗口 >4295s 用例）。
- 冲突声明：无。

### OB-05（P2）slo-alert AlertStatus 零引用死代码
- 类别：死表面
- 定位：crates/observability/slo-alert/src/alert_catalog.rs:49-60 定义；`grep AlertStatus:: crates/` 除定义文件外 0 命中。
- 根因：告警状态机（OPEN/ACKED/RESOLVED 语义）只有类型没有流转实现——商用交付的告警生命周期管理缺失。
- 修复：短期：dead-surface-audit.md 登记 + 要么实现状态流转（trigger.rs 接 AlertStatus 迁移函数 `fn transition(&mut Alert, to: AlertStatus) -> Result<(), AlertError>`）要么删除类型。建议 33 号蓝图（可观测域）实现。
- 二次验证：删除或实现都不影响既有通过项（无引用）。
- host 验证：`grep -rn "AlertStatus::" crates/ | grep -v alert_catalog.rs | wc -l` 判 >0（实现路线）。
- 冲突声明：与 docs/dead-surface-audit.md 精神相关（需登记）。

### OB-06（P2）aiops self_heal 无执行器时回退"Mock 成功"
- 类别：语义风险（fail-open）
- 定位：crates/observability/aiops/src/self_heal.rs:66、162-183（`None = 回退 Mock 成功`；None 分支把 actions 描述直接记为成功副作用）
- 根因：生产装配若忘注入 executor，self_heal 会"报告修复成功"而实际什么都没做——观测面误导运维。
- 修复：None 分支改为 `return Err(AiOpsError::HealFailed)` 或返回带 `executed: false` 标记的报告；Mock 回退只在 cfg(test/mock) 下存在。
- 二次验证：依赖 None 回退的既有测试需显式注入 MockExecutor——预期受影响：self_heal.rs 内 #[cfg(test)] 用例（194 行起）。
- host 验证：`cargo test -p eneros-aiops`。
- 冲突声明：与蓝图/27 S2（mock 不得在 release 语义中冒充成功）精神冲突。

### OB-07（P2）logging/metrics/tracing 无导出后端接线（可观测能力缺口）
- 类别：缺失功能
- 定位：logging StructuredLoggerImpl 只到 LogStorage trait（内存后端 MemoryLogStorage）；metrics 仅 MemoryMetricsStore；telemetry-queue OtlpExporter 是闭包注入（persist.rs 注释"注入 HTTP 闭包"）——三者均无仓内可运行的持久/网络导出实现。
- 根因：no_std 约束下导出由集成层注入，属设计决策（偏差声明有先例），但商用验收需要至少一条 host 可跑的端到端导出证据链（文件后端即可）。
- 修复：为 logging/metrics 各增一个 `FileStorage`/`FileMetricsStore` 后端（std feature 门控），并在 tests/ 增端到端用例（record→flush→文件 JSONL 可读回）；列入 33 号蓝图。
- 二次验证：std feature 门控不影响 no_std 既有构建。
- host 验证：`cargo test -p eneros-logging --features std`。
- 冲突声明：无；对照 configs/logging.toml、metrics.toml、tracing.toml 的承诺（见 CF-01：这些配置本身也无消费者）。

---

## 3. CF — 配置一致性发现

### CF-01（P0）81 个 toml 中仅 1 个被真实加载，其余 80 个为纯文档模板且无任何语法校验门禁
- 类别：配置一致性 / 宣称一致性
- 定位：全量核查（逐文件 grep 文件名于 crates/xtask/ci/tests/tools/fuzz）：
  - 68 个 toml 文件名 0 引用（含 logging.toml、metrics.toml、tracing.toml、fault-recorder.toml、latency_probe.toml 等可观测配置——即 OB-07 承诺的配置侧同样悬空）；
  - 12 个仅注释/文档串提及（gptp、latency_probe、ota、power/sequence、recovery、secure-boot、signing_keys、socket、tcpip、topics、dds、market_source），无解析代码——no_std 无 toml crate，D 偏差注释自认"集成层读取"；
  - **唯一真实消费者**：configs/control-write.toml（crates/agents/device-agent/src/assembly_demo.rs:31-54，且仅测试路径）。
  - 无任何门禁校验 toml 语法（xtask 只有 config-refs 引用检查，不 parse）。
- 根因：configs/ 目录在版本演进中按"交付物外观"堆积，配置与代码的绑定停留在注释宣称层；集成层读取的承诺无验证机制。
- 修复：① 新增 `xtask gate config-syntax`：host 侧用 toml crate 解析 configs/** 全部文件，语法错 FAIL（顺带堵住 81 个 toml 无校验缺口）；② 每个 toml 头部强制注释 `consumer: <crate::path | 集成层占位>`，config-refs 门禁扩展校验"声明的消费者存在"（与 GV-01/GV-15 合并实施）；③ 把"配置仅模板、产线注入"的事实写入 docs/dead-surface-audit.md，不得在 go-no-go 中宣称为活配置。
- 二次验证：config-syntax 首跑预期全绿（存量 toml 语法应合法；若有红即真实发现）。consumer 声明门禁首跑预期拦截清单 = 80 个模板文件——需一次性批量补注释（机械工作，无代码风险）。次生风险：无。
- host 验证：`cargo xtask gate config-syntax; echo $?` 判 0；故意破坏一个 tmp toml 判非 0。
- 冲突声明：与蓝图/27 G7 宣称一致性冲突（文档宣称"配置位于 configs/xxx.toml"但无消费路径）；与 §17 判据冲突（配置证据不可复核）。

### CF-02（P1）secure_boot.toml vs secure-boot.toml 重复且内容冲突
- 类别：重复配置
- 定位：configs/secure_boot.toml（v0.160.0，[secure_boot]/[root_of_trust] 段，含 debug_mode 字段）vs configs/secure-boot.toml（v0.113.0，[trust_root] 段，root_pubkey_hex 模板）。
- 根因：两个版本迭代各自新建文件；前者无消费者（0 引用），后者仅 lib.rs 注释提及。secure_boot.toml 的 `debug_mode = false`（跳过验签开关）即使为 false 也不应存在于交付配置——该字段本身是安全面隐患。
- 修复：删除 configs/secure_boot.toml（0 引用，v0.160.0 内容并入 secure-boot.toml 的注释性策略说明；debug_mode 字段整体移除）；secure-boot 系列保留单一 secure-boot.toml。
- 二次验证：删除后 config-refs 无新违规（0 引用已核实）；crates/security/secure-boot 注释引用的是 secure-boot.toml，不受影响。
- host 验证：`grep -rn "secure_boot.toml" crates/ xtask/ ci/ tests/` 判 0 命中后删除；`cargo xtask gate config-refs` 通过。
- 冲突声明：无。

### CF-03（P1）federation 系列 5 配置 / twin 系列 3 配置全部孤儿
- 类别：孤儿配置
- 定位：configs/federation.toml、federation-auction.toml、federation-channel.toml、federation-consensus.toml、federation-island.toml、twin_forecast.toml、twin_mirror.toml、twin_whatif.toml——8 个文件全部 0 引用。
- 根因：同 CF-01；federation/twin 代码（crates/agents/federation、twin-agent、twin-global）以构造参数吃配置，toml 无解析入口。
- 修复：并入 CF-01 的 consumer 声明门禁统一处理；短期在每文件头标注 `consumer: 集成层占位（构造参数注入）`。不建议直接删除（蓝图/14 交付清单可能引用）。
- 二次验证：注释性修改零风险。
- host 验证：N/A（注释）。
- 冲突声明：同 CF-01。

### CF-04（P2）config-refs 门禁与 81 个 toml 现状的组合矛盾
- 类别：门禁-配置交互
- 定位：xtask/src/gate.rs:272-337。门禁"阶段 0 只告警"（GV-01）+ 仅扫顶层（GV-15）+ 81 个 toml 几乎全孤儿（CF-01）三者组合：门禁即使转阻断也拦不住配置腐化，因为引用方本来就几乎没有。
- 修复：依赖 GV-01+GV-15+CF-01 三修联动：先补 consumer 注释 → 门禁校验 consumer 存在性 → 转阻断。顺序不可反（否则门禁空转或误拦）。
- 二次验证：见各条。
- host 验证：见各条。
- 冲突声明：同 GV-01。

---

## 4. 依赖治理核查（deny.toml / Cargo.lock / unsafe）

- deny.toml：advisories 默认 deny、unmaintained=workspace；licenses allow 表含 OpenSSL（rustls 链，有注释说明）；bans multiple-versions=warn、wildcards=warn（git 依赖 rust-sel4 豁免，合理）；sources 白名单 crates.io + rust-sel4 git。**已核查无发现**（策略面健全；cargo-deny 缺失 fail-closed 已在 ci/src/gate.rs 落实——蓝图/09 旧发现确认已修）。
- 残留风险：unmaintained=workspace 仅查直接依赖，传递依赖的 unmaintained 不报——P2 级备注，建议商用前改 `unmaintained = "all"` 并处理豁免清单。
- unsafe 依赖面：无专门门禁统计第三方 unsafe（依赖 audit 由 cargo-deny advisories 承担）；仓内 unsafe 有 SAFETY 注释门禁（rebuild-ci.sh 第 5 步），已核查存在。

## 5. tests/ 基建总评

- boot_test.sh：fail-closed 委托 qemu-prod harness，拒绝 -kernel/关键词成功——健全，QEMU 可跑（零真机依赖，镜像经环境变量注入）。
- soak72.sh：检查器/执行器分离 + --self-test 注入假证据断言 FAIL——符合蓝图/20 R1-5，健全；无硬件 verdict=SKIP exit 0 但自带自检模式补偿，可接受。
- interop/*.sh：对端不可用产出 skip 证据 exit 0——**注意**：若互操作矩阵进入 go-no-go，skip 证据不得计 PASS（S9）。现状 go-no-go.md 未 grep 到 interop.sh 消费，风险未实化；纳入时须改判。
- ota-rehearsal.sh：QEMU 可用则演练、否则产出预案——诚实降级，健全。
- rebuild-ci.sh：与 CI g0 对应 8 步，含 SOURCE_DATE_EPOCH 纪律——健全。
- g1-artifacts.sh：fail-closed + toolchain 容错落盘（report 如实 FAIL 但存在性保证）——健全，与 S9 兼容（FAIL 如实记录）。
- panic-probe.sh / oom-probe.sh：QEMU 侧探针，判据为 marker 计数——健全；GV-20 硬编码路径除外。

---

## 6. 统计与摘要

总计 27 项发现：
- GV：20 项（P0×4、P1×6、P2×10）
- OB：7 项（P1×2、P2×5）
- CF：4 项（P0×1、P1×2、P2×1）
- 依赖治理：1 项备注（unmaintained=all 建议，并入 P2 统计外附注）

P0 一行清单：GV-01 config-refs 恒不阻断（冲突 §13）、GV-02 authorized-construction 恒 PASS、GV-03 perf-gate 回归方向反、GV-04 CI 假 libFuzzer、CF-01 81 toml 仅 1 个被真实加载且无语法门禁。

与蓝图/27 冲突项：GV-01（§10/§13）、GV-02（§13）、GV-03（§17）、GV-04（§17）、GV-05（§17）、GV-06（§4 S9）、GV-07（§4 S9/§17）、GV-08（§4 S2）、GV-09（§13 精神）、GV-10（§17 判据 5）、GV-11（S9）、GV-13（§15 WP-27.11）、OB-06（S2 精神）、CF-01/CF-04（G7/§17）。
