# 蓝图/24 FIX-24.R3 审查修复证据

> 日期：2026-08-10 ｜ 前置：蓝图/24 全量实施（R1/R2）在库
> 审查结论来源：标准轴 + 规格轴报告（三项 P0）。实施前已逐项复核现状，以代码为准。

## 侦察：三项符号全仓使用点清单（修复前）

| 符号 | 使用点 | 修复前状态 |
|---|---|---|
| `FeedbackBundle::fresh` | ingress.rs:423（复验侧捏造）、chain_sink.rs:202（签发侧捏造）、chain/tests.rs×9（测试，合理） | 生产路径 2 处常量捏造 |
| `update_fencing_token` | chain_sink.rs:165（唯一定义） | **全仓零调用**（差异未记入 R1） |
| ingress `let _ =` | 13 处（metrics.increment ×5 / audit.log ×7 / maybe_flush ×1） | 静默丢 Result |

## 修复 1：消除 feedback 捏造（CTL-010 真实化）

### 改动点
- `control-gateway chain/mod.rs`：新增 `FeedbackBundle::untrusted(now)`（全字段
  显式不可信，CTL-010 reconcile 全拒 → fail-safe）。
- `ingress.rs`：
  - `build_feedback()`：复验前经 PointAccess 真实回读设备遥测点
    （grid_up/soc_pct/dc_bus_v/overcurrent/overtemp，与 point_map 同名）构建
    `FeedbackSample`（真实值 + 质量位转换 `quality_flag()` + 真实时间戳）；
    无对应点/回读失败/值不可转 → 该字段保持显式不可信；
  - 新增轻量 HMAC 预检（路由/反馈提前到复验前时保篡改分类 = Hmac 桶）；
  - 路由/点匹配存在性检查提前（分类语义不变：设备 → `device_id=`、点 →
    `point_id=`，与既有断言兼容）。
- `chain_sink.rs`：`ChainCommandSink::new` 增 `feedback_fn: fn(u64) -> FeedbackBundle`
  注入点；`write()` 用注入值（`FeedbackBundle::fresh` 硬编码移除）。
- `runner`：`build_production_sink`/`run_once` 增 `feedback_fn` 参数；`main.rs`
  注入 `untrusted`（显式未知 + R3-1 偏差登记）。

### 防回归测试（证伪旧行为）
| 测试 | 证伪内容 |
|---|---|
| `missing_telemetry_denies_via_feedback_untrusted`（ingress 用例 A） | 旧行为（fresh 常量）放行；新行为：设备无遥测 → 复验 Safety deny + 零落写 |
| `bad_quality_telemetry_denies`（用例 B） | 遥测质量非 Good（离线）→ fail-safe deny |
| `injected_unknown_feedback_denies_chain`（runner） | 签发侧注入 untrusted → 自证链 deny（旧行为硬编码常量无法注入） |
| `tampered_envelope_denied_hmac`（保持） | 篡改信封仍归 Hmac 桶（HMAC 预检保分类） |
| `protocol_failure_denied_and_skipped`（更新） | 遥测可信但写失败 → Protocol 桶（写失败与离线解耦，`set_write_fail`） |

### grep 自证（零常量捏造残留）
```
ingress.rs:607        FeedbackBundle::untrusted(now_secs)   ← 真实化起点（逐字段真实回读覆盖）
main.rs:159           FeedbackBundle::untrusted(now)        ← 显式未知注入（R3-1 偏差登记）
chain_sink.rs:344-345 / runner lib.rs:598-604  ← 仅测试装置（#[cfg(test)]）
生产路径 FeedbackBundle::fresh：零残留
```

## 修复 2：ingress 静默丢 Result 清偿

### 改动点
- `audit: Box<dyn AuditLogger>`（可注入失败实现验证兜底；生产仍为
  `AuditLoggerImpl<MemStore>` 哈希链，完整性不变）+ `audit_dropped: u64`
  infallible 主计数 + `METRIC_AUDIT_DROPPED`（`eneros_audit_dropped_total`，
  第 12 个注册计数器，随既有导出可见）；
- 全部 7 处 `audit.log` 收拢为 `log_audit()`：失败 → `audit_dropped += 1` +
  metrics 增量（best-effort）；`metric_total` 对 audit_dropped 特判返回字段值
  （不依赖 best-effort 通道）；
- 全部 5 处 `metrics.increment` 保留 `let _ =` 但**每处带豁免注释**（metrics 为
  best-effort 通道：注册后 increment 仅在存储满丢弃最旧时静默降级，不影响
  写路径——铁律 3 注释豁免理由）。

### 防回归测试
`audit_failure_increments_audit_dropped`（ingress 用例 C）：注入 FailingAudit
（`SeqOutOfOrder` 恒失败）→ 触发一条拒绝 → `metric_total(METRIC_AUDIT_DROPPED)
== 1`（infallible 字段主计数，随指标导出可见）。

### 残留自证
`grep -n "let _ = " ingress.rs`：metrics.increment/maybe_flush/register 处全部
带豁免注释；audit.log 零残留（全部经 log_audit）。

## 修复 3：握手 token 真正生效（D7 语义归位）

### 改动点
- `build_production_sink`：装配期握手（`dispatcher.handshake()`）——失败 →
  装配失败（fail-fast fail-closed）；握手返回值作为信封 token 传入
  `ChainCommandSink::new`，并调用 `sink.update_fencing_token(token)`（接通，
  此前全仓零调用）。
- dispatcher 内部握手逻辑不变（≤5 次有界重试、零降级——单测
  `handshake_bounded_retries_then_offline`/`write_fails_closed_without_handshake`
  保持绿）。

### 防回归测试
`envelope_fencing_token_equals_handshake_token`（runner）：捕获通道记录握手
响应 token 与投递帧信封 → 断言 **信封 `fencing_token` == 握手返回值
（== epoch_seed+1，复验侧铸造）**；证伪"本地 grant 铸造值进生产信封"的旧
机制（旧代码 update_fencing_token 零调用、token 来源未归位）。
**侦察结论（R3-2）**：D7 同源约束下握手值 == 本地 grant 值，数值不可区分；
以机制断言完成证伪，未改 D7 设计。`handshake_failure_fails_closed` 更新为
装配期 fail-fast 断言；stale token 拒绝由 ingress 用例 5 保持。

## 门禁复算

```bash
cargo test --workspace        # 326 套件全绿
cargo test -p eneros-device-agent   # 44 passed（+2：修复 1 用例 A/B、修复 2 用例 C）
cargo test -p eneros-dual-brain     # 55+1 passed
cargo test -p eneros-ai-runner      # 16 passed（+2：修复 1 注入生效、修复 3 信封 token）
cargo test -p eneros-protocol-abstract / -p eneros-control-gateway  # 全绿（随 workspace）
cargo clippy --workspace -- -D warnings   # 干净
cargo fmt --check                 # 干净
```
（aarch64-unknown-none：受影响 crate 依赖链无新增 std；device-agent 目标侧
受 littlefs2-sys C 工具链缺失阻塞为既有环境债，宿主全绿。）

## 修订记录
蓝图/24 R3（修复摘要 + R3-1 偏差登记 + R3-2 侦察结论）已追加。
