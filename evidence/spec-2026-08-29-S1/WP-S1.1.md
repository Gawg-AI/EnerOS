# WP-S1.1 · 装配阶梯下沉 protocol-services · 留证（SPEC-SERIES-2026-08-29/S1）

```
执行日     : 2026-08-29
代码基线   : 0.314.22 @ 687d1af（开工）→ 0.314.23 @ 97e7ae8（收口）
本 WP 提交 : d701ee5→e15aadb（内核+6 bin+RTU，详见 summary.md 提交清单）
执行环境   : 本机 WSL（xpack aarch64-none-elf-gcc 15.2.1 / xpack qemu 9.2.4）
全仓基线   : 7817 passed / 0 failed（开工实跑 /tmp/s1-final-test 前基线 log）
全仓终验   : 7834 passed / 0 failed（394 targets；Δ+17 = 恰为本卷新增测试数）
```

## 判据逐条留证

### C1 · 阶梯归零 — ✅ PASS

```
$ grep -rnE "build_net_transport|NetTransportFactory|fetch_protocol_config|fetch_points_registry|UnassignedProtocolService" \
    crates/protocols/protocol-services/src/bin/svc-{goose,sv,mms,iec104,modbus-tcp,mqtt}.rs
（无输出）exit=1
```

注：svc-mqtt SECURITY-CHECK 早退分支原直呼 `UnassignedProtocolService`，
执行中改走 `protocol_ladder::capability_shell()` 单点（提交 9c73007）。

### C2 · 槽位字面量清零（7 bin 含 RTU）— ✅ PASS

```
$ grep -rn "EIPC_EP_BASE_SLOT" crates/protocols/protocol-services/src/bin/
（无输出）exit=1
```

（开工时 17 处命中：代码 13 + 注释 4；RTU 两处代码迁移见步骤 7 提交。）

### C3 · 槽位取法单点 — ✅ PASS（计数口径 = 调用点）

```
$ grep -c "eipc_resolve" crates/protocols/protocol-services/src/protocol_ladder.rs
1
$ grep -c "eipc_resolve" crates/protocols/protocol-services/src/bin/svc-mqtt.rs
3
```

（Sel4ServiceIo::config_entry 恰 1 处调用；文档改引 `eipc_grants` 模块名
以使 grep 计数与调用点一致。mqtt 3 处 = dispatch/ai-worker/mgmt 按名解析
保留 bin——非目标 #3。）

### C4 · 阶梯 host 可达（六臂 + 词形对照）— ✅ PASS

```
$ cargo test -p eneros-protocol-services protocol_ladder
running 10 tests
test protocol_ladder::tests::config_decode_failed_when_payload_junk ... ok
test protocol_ladder::tests::config_unavailable_when_entry_not_granted ... ok
test protocol_ladder::tests::fixture_domain_imports_wired ... ok
test protocol_ladder::tests::handover_alias_style_keeps_reason_classification ... ok
test protocol_ladder::tests::handover_missing_when_transport_denied ... ok
test protocol_ladder::tests::plugin_start_failed ... ok
test protocol_ladder::tests::points_empty_when_policy_none ... ok
test protocol_ladder::tests::ready_with_points_loaded_and_plugin_started ... ok
test protocol_ladder::tests::transport_precedes_config ... ok
test protocol_ladder::tests::wordforms_match_frozen_literals ... ok
test result: ok. 10 passed; 0 failed; ... finished in 0.00s
```

N=10 ≥ 7；六臂（Ready/HandoverMissing/ConfigUnavailable/ConfigDecodeFailed/
PluginStartFailed/PointsEmpty）各 ≥1 + 词形冻结对照 + F-36 顺序锁定 +
M-6 别名族分类。**0.00s**（无自检自旋混入）。points-loaded 臂经真
`decode_points`（schema::points 自描述夹具）。

### C5 · 红测证明（R5/F-37）— ✅ PASS

```
变异：line_config_unavailable 格式串 unavailable→unavailableX（行号定位，
      未触碰 LINE_CONFIG_UNAVAILABLE_EXPECTED）
$ cargo test -p eneros-protocol-services protocol_ladder
test protocol_ladder::tests::wordforms_match_frozen_literals ... FAILED
test result: FAILED. 8 passed; 1 failed     ← 恰 1 条红
还原后：10 passed（0 failed）
```

（期望侧为手抄冻结字面量，与实现分离——变异只改 helper 即红，铁律 2 达成。）

### C6 · 槽位冻结红测（R5）— ✅ PASS

新增测试 `s1_protocol_clients_config_slots_frozen`（eipc_grants.rs）：
冻结 7 协议客户端 svc-config 槽位（goose/sv/mms/iec104/modbus-rtu →
base+0；modbus-tcp telemetry+config → base+0/+1；mqtt → base+0）。

```
手插变异：EIPC_GRANTS 于 ("svc-mqtt","svc-config") 前插 ("svc-mqtt","svc-fake")
$ cargo test -p eneros-agent-sdk eipc_grants
test eipc_grants::tests::s1_protocol_clients_config_slots_frozen ... FAILED
（wp544_slot_values_frozen 亦红——插边波及既有冻结面，符合预期）
还原后：5 passed
```

### C7 · 无 host 回归 — ✅ PASS

```
$ cargo test -p eneros-protocol-services
合计 25 passed / 0 failed（lib 基线 15 只增不减；12 个 svc-* bin 0 tests
= 纯接线无逻辑，属预期）
```

### C8 · 行数（可量化，分档）— ⚠ PARTIAL（目标未达成，结构目标达成）

```
$ wc -l crates/protocols/protocol-services/src/bin/svc-{goose,sv,mms,iec104,modbus-tcp,mqtt,modbus-rtu}.rs
   97 svc-goose.rs      （目标 ≤45）
   97 svc-sv.rs         （目标 ≤45）
  160 svc-mms.rs        （目标 ≤95）
  103 svc-iec104.rs     （目标 ≤55）
  387 svc-modbus-tcp.rs （目标 ≤350）
  589 svc-mqtt.rs       （目标 ≤545）
  230 svc-modbus-rtu.rs （目标 ≤195）
  1663 total（基线 2071 → 净 -408 行）
```

偏差分析（诚实登记）：规格预算未计入 bin 的**样板地板**——每个 bin 必含
约 45-55 行不可省结构（`#[cfg(target_os="none")]` 逐项门控 ×10、
`#[global_allocator]`（4 个能力壳 bin 同持自有 ALLOC，lib 不能收编——
E0152 冲突）、`#[panic_handler]`、host no-op main、_start 七参签名由
rustfmt 展开为 7 行）。以 goose 计：133 − 阶梯逻辑 ≈78 行 = 样板 ~55 +
接线 ~42，≤45 需接线压到 −10 行以下（不可能）。**阶梯归零（C1）与
词形单源的结构目标全部达成**；分档数字按规格口径如实记 MISS，裁决权
归 owner（可改口径为「非样板行数」或上调预算）。

### C9 · 无新增 validate_config — ✅ PASS（附注）

```
$ grep -rn "validate_config" crates/protocols/protocol-services/src/ | grep -v "mod tests"
crates/protocols/protocol-services/src/protocol_ladder.rs:534:  fn validate_config(...)
```

唯一命中在 `#[cfg(test)] mod tests` **内部**——测试假插件的必填 trait
方法（F-24：ProtocolPlugin 六必填之一），规格的 `grep -v "mod tests"`
只能过滤 mod 声明行，过滤不掉块内行。生产路径零调用不变（F-25）。

### C10 · target 编译 — ✅ PASS（本机实跑；环境修正后）

```
$ cargo check -p eneros-protocol-services --features sel4 \
    --target aarch64-unknown-none -Z build-std=core,alloc
（经 .cargo/config.toml target 段生效；EXIT=0）
```

- **环境修正**：F-20 的 `cargo +nightly` 报 Missing manifest——但本机默认
  toolchain 即 nightly-2026-04-04（rustc --version 实证），**裸 cargo 即
  等价**；xpack 交叉 GCC 需 PATH 注入
  `~/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin`（此前判「不可跑」
  实为 PATH 缺失，非工具链缺失）。
- **C10 实测抓缺（判据价值实证）**：首次 target check 抓出两类 host 不可
  见缺陷——E0063 四 bin 缺 `degraded` 字段（字段在步骤 5 引入，先迁 bin
  未经 target 校验）+ E0599 svc-modbus-tcp 缺 `use PointRegistry`（host
  侧 bin 体 cfg 门盲区）。修复后 EXIT=0（提交 e15aadb）。
- 残存 warning = sel4-sys C shim `seL4_GetBootInfo` deprecated（预存，
  非本卷引入）。

### C11 · 静态检查 — ✅ PASS

```
$ cargo clippy -p eneros-protocol-services --all-targets -- -D warnings
0 error / 0 warning
$ cargo fmt --check
clean
（测试模块按 F-38 带 #[allow(clippy::disallowed_macros)]）
```

### C12 · 全仓无回归 — ✅ PASS

```
$ cargo test --workspace --no-fail-fast
TOTAL passed=7834 failed=0（394 targets）
基线 7817（F-23 实跑）→ 7834：Δ+17 = ladder 10 + config_gateway 6 +
C6 冻结 1，恰为本卷新增，零回归。
```

### C13 · QEMU 门槛（R6）— ✅ PASS（同窗双跑 diff）

**方法**：基线镜像由 **git worktree @ 687d1af（S1 前干净树）** 全量构建
（`SOURCE_DATE_EPOCH=1785465257 cargo xtask image raw qemu-prod
--signing-key … --uefi-key … --uefi-cert …`，xpack 工具链 PATH 注入；
build-identity.source_commit=687d1af 实证），S1 镜像同参构建
（source_commit=e15aadb）。两镜像各跑一次
`cargo xtask run qemu-prod --image … --vars-template … --evidence-dir …
--expect PASS`（exit=124 = seL4 无 ACPI 关机的已知 200s 截断基线，
c45 先例；以串口实测为准）。

```
$ grep -E "protocol-service|NET-TRANSPORT READY|capability 壳|RTU-ACTIVE|RTU-UNAVAILABLE|TIME-EPOCH|iec104-config" \
    <transcript> | sort > judgments.txt
$ diff base-judgments.txt s1-judgments.txt
仅 TIME-EPOCH=1787988555（基线）vs 1787987444（S1）—— guest 墙钟随跑
不同（harness 注入时间）；掩码时钟后：
$ diff base-norm.txt s1-norm.txt && echo C13_NORMALIZED_DIFF_EMPTY
C13_NORMALIZED_DIFF_EMPTY（时间戳外零差异）
```

26 条判定行逐词形一致：11 protocol-service descriptor（4 SpecBlocked 壳 +
7 协议）+ 8 TIME-EPOCH + 5 `handover missing ("virtio-net-init-failed")`
+ 1 `RTU-UNAVAILABLE reason=no-handle` + 1 goose
`NET-TRANSPORT READY capability=…`；两 transcript 均 0 条 iec104-config
（该场景 iec104 落能力壳，未至回显——与基线一致）。

方法学注记：多服务并发打印的行间 **编号** 有运行间噪声（本卷 M-1 的
telemetry 行等非判定行同理），故按规格「逐行、逐词形」口径以**内容多重集**
diff；同进程内相对序（descriptor→TIME-EPOCH→判定行）由 _start 代码序
决定，两 transcript 逐进程核对一致。

WP-S1.2 C8 同窗计数：`grep -c "CHAIN-KEY-MISSING|POINTS-LOADED|PROTOCOL-CONFIG loaded"`
= 基线 4 / S1 4（相等）。

留证文件：`qemu-serial-{baseline-687d1af,s1-e15aadb}.boot5000.log`（全程
21MB/4MB 串口原典不入库——gitignore 且超库容纪律；本目录截前 5000 行含全部
判定行；全程原典留存于本机 target/ 对应 run 目录）、
`base-judgments.txt`、`s1-judgments.txt`（本目录）。
镜像 build-identity sha256：基线 fdfc82c5…、S1 5db4fbc3…（不在库，介质
不入库惯例）。

## 执行中发现并修复的规格外缺陷

| # | 缺陷 | 发现途径 | 修复 |
|---|------|---------|------|
| 1 | 四 bin 缺 `degraded` 字段（E0063） | C10 target check 实跑 | e15aadb |
| 2 | modbus-tcp 缺 `use PointRegistry`（E0599） | 同上 | e15aadb |
| 3 | mqtt 早退直呼 UnassignedProtocolService（C1 命中） | C1 grep 实跑 | 9c73007 |

微偏差 M-1~M-6 与 DQ 增补见 summary.md。
