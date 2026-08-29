# WP-S1.2 · ConfigGet 调用侧管道收口 · 留证（SPEC-SERIES-2026-08-29/S1）

```
执行日     : 2026-08-29
本 WP 提交 : d701ee5（config_gateway 内核）+ 7101d90（cfg_bindings×2 /
             dispatch fetch_control_write / ai_handler response_body）
落点       : eneros-agent-sdk::config_gateway（DQ-6：新模块，职责分离）
```

## 管道架构（收口后）

```
wire 层（不动一字节）      agent-sdk::config_client
                            encode_get_entry / get_entry / decode_response_body
                                      ↑ 唯一调用
调用侧管道内核（新增）      agent-sdk::config_gateway
                            config_get(io, id, decode: HRTB FnOnce(&[u8]))
                            ConfigStep{Unavailable,TooShort,BadPrefix,DecodeFailed}
                            response_body（先检视原始载荷再剥离——ERR: 透传面）
                            ConfigEntrySource trait（Sel4IpcClient impl 同门控）
        ↑ &mut dyn（ServiceIo 经 ServiceIoAsSource 适配；Sel4IpcClient 直实现）
消费方（呈现各归其位，R2 逐字节）
  protocol_ladder::run        ← ServiceIo seam（六网络 bin）
  service_cfg::fetch_*        ← 阶梯 + RTU（PROTOCOL-CONFIG/POINTS-LOADED 词形）
  cfg_bindings::fetch_*       ← control-dispatch 装配（malformed: {e:?} 词形）
  dispatch_assembly::fetch_control_write ← 全吞 .ok()? 语义
  ai_handler::fetch_ai_peer_config       ← ERR: 检视后 response_body
```

## 判据逐条留证

### C1 · 解码调用单点 — ✅ PASS

```
$ grep -rn "decode_response_body" crates/ --include=*.rs \
  | grep -v "agent-sdk/src/config_client.rs" | grep -v "config_gateway.rs"
（无输出）
```

收口前 6 处（service_cfg ×2、cfg_bindings ×2、dispatch_assembly ×1、
**ai_handler ×1**——规格 F-15 计 5 处漏计 ai_handler，执行中补齐）；
收口后仅 config_client.rs（定义+自测）与 config_gateway.rs（管道入口
response_body/config_get 内部）两类命中。

### C2 · wire 层不动 — ✅ PASS

```
$ cargo test -p eneros-agent-sdk
138 passed / 0 failed（基线 F-6 = 131，Δ+7 = config_gateway 6 + C6 冻结 1；
config_client 定义/测试零改动——git diff 687d1af..HEAD config_client.rs 为空）
```

### C3 · 三臂 × 多节覆盖 — ✅ PASS

```
$ cargo test -p eneros-agent-sdk config_gateway
running 6 tests
test config_gateway::tests::bad_prefix_when_length_exceeds_body ... ok
test config_gateway::tests::decode_failed_propagates ... ok
test config_gateway::tests::real_protocol_section_roundtrip ... ok
test config_gateway::tests::response_body_strip_matches_pipeline ... ok
test config_gateway::tests::too_short_when_response_under_4b ... ok
test config_gateway::tests::unavailable_when_entry_missing ... ok
test result: ok. 6 passed; 0 failed; ... finished in 0.00s
```

N=6 ≥ 4：Unavailable/TooShort/BadPrefix/DecodeFailed 四臂 + 真实节 id
（protocol-modbus-tcp canonical KV 往返，decode 收到剥离 4B 前缀后的
body——F-29 借期内断言）+ response_body 剥离对照。

### C4 · 红测证明（R5）— ✅ PASS（变异对象 = 分类映射）

```
变异：From<ConfigError> 映射 TooShort→BadPrefix 改一词
$ cargo test -p eneros-agent-sdk config_gateway
too_short_when_response_under_4b ... FAILED
response_body_strip_matches_pipeline ... FAILED
test result: FAILED. 4 passed; 2 failed
还原后：6 passed
```

口径注记：config_gateway 按 R2 **不持有任何串口词形**（呈现归调用方），
故「TooShort 措辞」的变异对象 = TooShort 的**分类归属**（措辞与实现
同源风险在分类面，等价于词形变异——红测有效性实证）。

### C5 · 消费方无回归 — ✅ PASS

```
$ cargo test -p eneros-control-services dispatch_assembly
test result: ok. 12 passed; 0 failed（基线 F-7 = 12，零回归）
```

### C6 · 静态检查 — ✅ PASS

```
$ cargo clippy -p eneros-agent-sdk -p eneros-control-services --all-targets -- -D warnings
0 error / 0 warning
$ cargo fmt --check
clean
```

### C7 · 全仓无回归 — ✅ PASS

```
$ cargo test --workspace --no-fail-fast
7834 passed / 0 failed（≥7817 基线；同 WP-S1.1 C12）
```

### C8 · QEMU 门槛（与 WP-S1.1 C13 同窗）— ✅ PASS

```
$ grep -c "CHAIN-KEY-MISSING\|POINTS-LOADED\|PROTOCOL-CONFIG loaded" <transcript>
基线（687d1af 镜像）: 4
S1（e15aadb 镜像）  : 4   ← 相等
```

（同窗双跑与判定行 diff 见 WP-S1.1.md C13。4 = POINTS-LOADED×2
[goose-config / control-bindings] + PROTOCOL-CONFIG loaded×2；本场景无
CHAIN-KEY-MISSING——信封密钥正常取回。）
