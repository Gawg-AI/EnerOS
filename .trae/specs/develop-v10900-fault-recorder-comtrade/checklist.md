# Checklist — v0.109.0 故障录波 COMTRADE

> 逐项核验后勾选。分组：A 蓝图合规 / B 目录结构 / C crate 骨架 no_std / D ring_buffer.rs / E trigger.rs / F comtrade_writer.rs / G FaultRecorder / H 配置与文档 / I 版本同步与构建验证。

## A. 蓝图合规与 spec 对齐（C1~C10）

- [x] C1: 交付物对齐蓝图 §3：ring_buffer.rs / trigger.rs / comtrade_writer.rs 三模块 + lib.rs FaultRecorder 齐全
- [x] C2: 接口对齐 spec 接口契约：`FaultRecorder` 含 new/push_sample/check_triggers/start_recording/export_comtrade/state/record_count；`ComtradeWriter` 含 write_cfg/write_dat；`RingSampleBuffer` 含 new/push/push_slice/get_recent/len/capacity
- [x] C3: 数据结构对齐 spec：RecorderConfig/ChannelConfig/TriggerCondition/ComtradeConfig/SampleRecord 字段一致
- [x] C4: `RecorderError` 4 变体齐全（IoError/InvalidConfig/NotReady/ChannelMismatch，D12）
- [x] C5: `TriggerType` 7 变体齐全（OverCurrent/OverVoltage/UnderVoltage/OverFrequency/RateOfChange/DigitalEvent/Manual）
- [x] C6: cfg 第 2 行 `TT,nA,nD` 带 A/D 后缀（D6）；模拟量行 13 字段含 a/b（D8）
- [x] C7: cfg 行序合规：频率行 50 / 档数行 1 / 档行 `sample_rate,total_samples` / 时间戳行含日期（D7）
- [x] C8: BINARY=i16 LE、BINARY32=i32 LE、数字量 16 位字打包、逆变换 `round((v-b)/a)`（D9）
- [x] C9: `FileSink` trait + `MockSink` 存在（D4），零 `std::fs` 调用
- [x] C10: spec.md D1~D12 偏差表与 lib.rs crate 文档偏差表、设计文档偏差表逐字一致

## B. 目录结构（C11~C16，记忆 §2.4.1）

- [x] C11: crate 位于 `crates/protocols/fault-recorder/`，未放根目录（D1）
- [x] C12: 根 `Cargo.toml` members 已追加 `"crates/protocols/fault-recorder"`
- [x] C13: `Cargo.toml` 零第三方依赖（无 path 引用需求）；package 名 `eneros-fault-recorder`
- [x] C14: 文档位于 `docs/protocols/fault-recorder-comtrade-design.md`，未平面化放 docs/ 根（D2）
- [x] C15: 测试全部 src 内嵌 `#[cfg(test)]`，未新增 tests/ 文件（D3）
- [x] C16: `cargo metadata --format-version 1` 解析成功（exit=0）

## C. crate 骨架与 no_std（C17~C22）

- [x] C17: lib.rs 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明
- [x] C18: 全 crate 零 `std::*` 引用（仅 `alloc::*`/`core::*`；Instant 仅 cfg(test) 内）
- [x] C19: 零 `panic!`/`todo!`/`unimplemented!`（生产路径）；零 `unwrap()` 于生产路径
- [x] C20: 零第三方依赖；零 unsafe；零 extern "C"
- [x] C21: `cargo build -p eneros-fault-recorder --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C22: lib.rs crate 文档含版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明（风格对齐 iec61850-sv）

## D. ring_buffer.rs（C23~C28）

- [x] C23: `RingSampleBuffer<T: Copy + Default>` 字段私有（data/capacity/write_pos/samples_written），`new(capacity)` 创建（D5 Vec 承载）
- [x] C24: `push` 未满追加，满则覆盖最旧（write_pos 回绕）
- [x] C25: `get_recent(n)` 返回 min(n, 已写入) 个元素，旧→新保序
- [x] C26: `len()` 返回 min(samples_written, capacity)；`capacity()` 返回容量
- [x] C27: 溢出后保序（最新 capacity 个元素）
- [x] C28: 测试 RB1~RB6 共 6 个全部通过

## E. trigger.rs（C29~C36）

- [x] C29: `TriggerCondition` 全 pub 字段（trigger_type/threshold/duration_ms/channel），derive Debug/Clone/PartialEq
- [x] C30: 构造时 required = max(1, duration_ms × sample_rate / 1000) 帧折算（D12）
- [x] C31: OverCurrent/OverVoltage/OverFrequency 为 `v > threshold`；UnderVoltage 为 `v < threshold`
- [x] C32: RateOfChange 为 `|v - v_prev| > threshold`（相邻差分）
- [x] C33: DigitalEvent 为上升沿（false→true），持续 true 不重复命中
- [x] C34: Manual 不自动触发；channel 未匹配按不满足处理
- [x] C35: 同帧多条件命中返回最小索引（配置序优先级，蓝图 §4.4）；触发后该条件计数复位
- [x] C36: 测试 TG7~TG13 共 7 个全部通过

## F. comtrade_writer.rs（C37~C48）

- [x] C37: `ComtradeWriter` 无状态纯函数（D4），write_cfg 返回 String、write_dat 返回 Vec<u8>
- [x] C38: cfg 第 1 行 `station_name,device_id,rev_year`
- [x] C39: cfg 第 2 行 `TT,nA,nD`（D6）
- [x] C40: 模拟量行 13 字段 `An,ch_id,ph,,uu,a,b,0,-32767,32767,1,1,P`，a=scale_factor、b=offset（D8）
- [x] C41: 数字量行 `Dn,ch_id,,,0`
- [x] C42: 频率行 50 / 档数行 1 / 档行 `sample_rate,total_samples` / 两行 time_str / 类型行 / 时标乘数行 1（D7）
- [x] C43: ASCII dat 行格式 `num,ts,raw…,0/1…`
- [x] C44: BINARY 布局 u32 LE num + u32 LE ts + i16 LE×n_analog + u16 LE 数字量（D9）
- [x] C45: 逆变换 `round((v-b)/a)` 且 i16 钳位 ±32767；a==0 按 1 处理（D9）
- [x] C46: BINARY32 模拟量 i32 LE（非 f32，D9）
- [x] C47: 数字量按 16 位字打包（≤16 通道 = 2 字节，D9）
- [x] C48: 测试 CW14~CW22 共 9 个全部通过

## G. FaultRecorder（C49~C60）

- [x] C49: 模拟/数字缓冲按帧交错存储（帧×n_ch+c），时间戳独立 u64 缓冲（D10）
- [x] C50: `new` 校验：channels 空 / sample_rate==0 / buffer_frames < pre+post → InvalidConfig
- [x] C51: `push_sample` 长度不匹配 → ChannelMismatch
- [x] C52: 状态机 Idle→Recording→Ready 转换正确；Recording 每帧 remaining-1 归零转 Ready
- [x] C53: `check_triggers()` 返回锁存触发引用（D12）；`start_recording()` Manual 入口仅 Idle 生效
- [x] C54: 非 Ready 态 export → NotReady，sink 零写入
- [x] C55: export 生成 `{base}.cfg` + `{base}.dat` 两文件经 sink 写出
- [x] C56: 导出窗口 = 触发点前 pre + 后 post 帧；SampleRecord sample_num 从 1 起、timestamp_us 相对首帧 u32
- [x] C57: 导出后 record_count+1、状态/latched 复位 Idle，可再次录波
- [x] C58: 窗口数据正确性（触发点前后采样值回放一致）
- [x] C59: 测试 FR23~FR30 共 8 个全部通过
- [x] C60: PERF31 触发检测性能断言通过（4000 push < 1000ms，cfg(test) Instant，D12）

## H. 配置与文档（C61~C66）

- [x] C61: `configs/fault-recorder.toml` 存在，`[recorder]` + `[[triggers]]` + `[comtrade]` 节齐全 + 中文注释 ≥7 点
- [x] C62: 配置中文注释覆盖：C37.111-2013 选型 / FileSink 抽象 D4 / 环形缓冲溢出策略 / 触发性能口径 D12 / 内存预算 / GPU 不适用 / v0.110.0 下游
- [x] C63: `docs/protocols/fault-recorder-comtrade-design.md` 存在，12 章节齐全
- [x] C64: 文档含 ≥2 个 Mermaid 图：录波流程图 + 状态机图
- [x] C65: 文档含 D1~D12 偏差表，与 spec.md 逐字一致
- [x] C66: 文档含性能口径声明（D12）

## I. 版本同步与构建验证（C67~C80）

- [x] C67: 根 `Cargo.toml` version == "0.109.0"
- [x] C68: `Makefile` VERSION == 0.109.0 且 L3 头部注释同步
- [x] C69: `ci.yml` L3 版本注释 == v0.109.0
- [x] C70: `gate.rs` 注释串尾 2 处追加 v0.109.0 类型清单
- [x] C71: `cargo test -p eneros-fault-recorder` 32/32 通过（含 T7 新增 cw23）
- [x] C72: eneros-iec61850-sv 回归通过（零改动验证）
- [x] C73: 全 workspace 回归通过（cargo test --workspace --exclude eneros-kernel --exclude eneros-hello，exit=0，零回归）
- [x] C74: `cargo build -p eneros-fault-recorder --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C75: `cargo fmt --all -- --check` 通过
- [x] C76: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C77: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）
- [x] C78: `git status` 无 target/elf/bin/dtb/IDE 缓存被追踪
- [x] C79: spec.md / tasks.md / checklist.md 三件齐全且内容一致；tasks.md 全部复选框已勾选；无超范围交付（Karpathy Simplicity First）
- [x] C80: 内存预算声明已落地文档（buffer_frames×(n_analog×4+n_digital+8)B，蓝图 §43.6）

## 验收记录

- **核验日期**：2026-07-20（首轮 77/80；T7 修复后复核）
- **核验人**：Trae Agent
- **通过项数**：80/80

### 首轮未通过项与修复闭环（3 项，均已闭环）

| 编号 | 首轮差距 | 修复与复核 |
|------|---------|-----------|
| **C8** | `quantize()` 未实现 `round((v-b)/a)`，向零截断代替 | T7：no_std 下 `f64::round` 不可用（libm 方法），改手写舍入 `adj - (adj % 1.0)`（`adj = x ± 0.5`，half away from zero，语义等价）；新增 `cw23_quantize_rounds_half_up`（v=12.36、a=0.1 → 124，截断实现得 123，可区分）防回归；32/32 复核通过 |
| **C45** | 同 C8（钳位 ±32767 与 a==0 按 1 首轮已实现） | 同 T7 修复，复核通过 |
| **C79** | tasks.md T6 复选框首轮核验时点未勾选 | 主流程已勾选 T6/T7 全部复选框，复核通过 |

### 关键命令结果摘要（2026-07-20 T7 后复核）

| 命令 | 结果 |
|------|------|
| `cargo metadata --format-version 1` | exit=0 |
| `cargo test -p eneros-fault-recorder` | 32/32 通过（含 cw23） |
| `cargo test -p eneros-iec61850-sv` | 25/25 回归通过（零改动） |
| `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` | exit=0，零 FAILED |
| `cargo build -p eneros-fault-recorder --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` | exit=0（T7 手写舍入 no_std 复核） |
| `cargo fmt --all -- --check` | exit=0 |
| `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` | exit=0，0 warning |
| `cargo deny check advisories licenses bans sources` | 在线拉取 advisory-db 网络失败（连接重置）；`--offline` 重跑 advisories/bans/licenses/sources 全 ok，exit=0 |
| `git status --porcelain` | 无 target/elf/bin/dtb/IDE 缓存被追踪 |

### 偏差与备注

- D1~D12 偏差表经程序化比对：spec.md vs lib.rs crate 文档 vs 设计文档 §9.1 三处 12 行**逐字一致**。
- 实施增量偏差 6 条记录于设计文档 §9.2（TriggerEngine 全量更新 / ch_idx 闭包签名 / capacity=0 退化 + is_empty / FR 测试拆分 / PERF31 口径 / quantize() 舍入缺失修复），逐条对照代码确认属实。
- PERF31 为主机 `std::time::Instant` 口径（cfg(test) 内，D12），真实硬件触发时延为实验室项。
- C8/C45 的 round 截断差距为首轮核验发现的实施偏差，经 T7 修复闭环并补记设计文档 §9.2 第 6 条。
