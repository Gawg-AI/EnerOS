# Tasks — v0.109.0 故障录波 COMTRADE

> Spec：`spec.md`（develop-v10900-fault-recorder-comtrade）。T1→T2/T3 可并行（T2/T3 互不依赖）；T4 依赖 T1~T3；T5/T6 顺序收尾。

- [x] **T1：新建 fault-recorder crate 骨架 + ring_buffer.rs + lib.rs 基座 — 环形缓冲与 FileSink**
  - [x] 1.1 `crates/protocols/fault-recorder/Cargo.toml`：`eneros-fault-recorder`，workspace 继承，零依赖（D1）
  - [x] 1.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明（ring_buffer/trigger/comtrade_writer）+ 重导出 + `RecorderError`（4 变体：IoError/InvalidConfig/NotReady/ChannelMismatch，D12，derive Debug/Clone/PartialEq）+ `FileSink` trait（write_file(path, data)，D4）+ `MockSink`（Vec<(String, Vec<u8>)> 写入记录 + 一次性错误注入 + 查询访问器，D4）+ crate 文档（版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明，风格对齐 iec61850-goose/iec61850-sv）
  - [x] 1.3 `src/ring_buffer.rs`：`RingSampleBuffer<T: Copy + Default> { data: Vec<T>, capacity, write_pos, samples_written }`（D5）；`new(capacity)` / `push(value)`（满则覆盖最旧）/ `push_slice(slice)` / `get_recent(n)`（min(n, 已写入)，旧→新保序）/ `len()` / `capacity()`
  - [x] 1.4 测试 RB1~RB6（6 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-fault-recorder ring_buffer::` 6/6 全过 ✅

- [x] **T2：trigger.rs — 触发条件引擎**
  - [x] 2.1 `src/trigger.rs`：`TriggerType`（7 变体，derive Debug/Clone/Copy/PartialEq）+ `TriggerCondition { trigger_type, threshold, duration_ms, channel }`（derive Debug/Clone/PartialEq）+ `pub(crate) TriggerEngine`（conditions + consec: Vec<u32> + required: Vec<u32>，构造时 required = max(1, duration_ms × sample_rate / 1000)，D12）；`evaluate(ch_idx: impl Fn(&str) -> Option<usize>, analog, prev_analog, digital, prev_digital) -> Option<usize>`（语义：OverCurrent/OverVoltage/OverFrequency `v>threshold`、UnderVoltage `v<threshold`、RateOfChange `|v-v_prev|>threshold`、DigitalEvent 上升沿、Manual 不自动触发；连续满足达 required 帧触发并复位计数；同帧多命中返回最小索引即配置序优先级；channel 未匹配按不满足处理）
  - [x] 2.2 测试 TG7~TG13（7 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-fault-recorder trigger::` 7/7 全过 ✅

- [x] **T3：comtrade_writer.rs — COMTRADE C37.111 文件生成**
  - [x] 3.1 `src/comtrade_writer.rs`：`Phase`（5 变体，derive Debug/Clone/Copy/PartialEq）+ `ChannelConfig`（derive Debug/Clone/PartialEq，`phase_str()` / `is_analog()`）+ `ComtradeConfig` / `ComtradeFormat`（3 变体）/ `SampleRecord` + 无状态 `ComtradeWriter`（D4 纯函数）
  - [x] 3.2 `write_cfg(config, channels, total_samples, sample_rate, time_str) -> String`：行序——`station,dev,rev` / `TT,nA,nD`（D6）/ 模拟量 13 字段 `An,ch_id,ph,,uu,a,b,0,-32767,32767,1,1,P`（D8）/ 数字量 `Dn,ch_id,,,0` / `50` / `1` / `sample_rate,total_samples`（D7）/ 两行 `time_str` / `ASCII|BINARY|BINARY32` / `1`
  - [x] 3.3 `write_dat(records, channels, format) -> Vec<u8>`：逆变换 `raw = round((v-b)/a)`（a==0 按 1，D9）；Ascii → 逐行 `num,ts,raw…,0/1…`；Binary → u32 LE num + u32 LE ts + i16 LE×n_analog（钳位 ±32767）+ u16 LE 数字量打包；Binary32 → i32 LE×n_analog（钳位 i32 范围）+ u16 LE 数字量打包（D9）
  - [x] 3.4 测试 CW14~CW22（9 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-fault-recorder comtrade_writer::` 9/9 全过 ✅

- [x] **T4：lib.rs FaultRecorder — 状态机 + 窗口截取 + 导出**
  - [x] 4.1 `RecorderConfig`（channels/triggers/comtrade/pre_fault_samples/post_fault_samples/sample_rate/buffer_frames）+ `RecorderState`（Idle/Recording/Ready，derive Debug/Clone/Copy/PartialEq）+ `FaultRecorder`（D10：analog/digital 交错环形缓冲容量 = buffer_frames×n_ch + timestamps u64 缓冲 + TriggerEngine + prev_analog/prev_digital scratch + 状态机 + latched_trigger: Option<usize> + frames_written）
  - [x] 4.2 `new(config)` 校验（channels 非空、sample_rate>0、buffer_frames ≥ pre+post、模拟/数字通道数与 channels 一致推导，否则 InvalidConfig）；`push_sample(analog, digital, ts)` 长度校验（ChannelMismatch）→ 交错写缓冲 → Idle 态 evaluate（命中则 latched + 转 Recording，remaining=post_fault_samples）→ Recording 态 remaining-1 归零转 Ready；`check_triggers()` 返回 latched 触发引用（D12）；`start_recording()` Manual 入口（Idle 才生效）
  - [x] 4.3 `export_comtrade<S: FileSink>(sink, base_path, time_str)`：非 Ready → NotReady；取触发点前 pre + 后 post 帧窗口（get_recent_frames 语义，D10）重建 Vec<SampleRecord>（sample_num 1 起、timestamp_us 相对首帧 u32）→ ComtradeWriter 生成 cfg/dat → sink 写 `{base}.cfg` + `{base}.dat` → record_count+1 → 状态/latched 复位 Idle；`state()` / `record_count()` 访问器
  - [x] 4.4 测试 FR23~FR30 + PERF31（9 个，见 spec 测试规划表；PERF31 用 `std::time::Instant` 仅 cfg(test)，D12）
  - 验证：`cargo test -p eneros-fault-recorder` 31/31 全过 ✅

- [x] **T5：workspace 接线 + 配置 + 设计文档**
  - [x] 5.1 根 `Cargo.toml` members 追加 `"crates/protocols/fault-recorder"`（protocols 段 iec61850-sv 之后）
  - [x] 5.2 `configs/fault-recorder.toml`：`[recorder]` sample_rate=4000 / pre_fault_samples / post_fault_samples / buffer_frames + `[[triggers]]` 过流/低压示例 + `[comtrade]` station/device/rev/format + 中文注释 ≥7 点（C37.111-2013 选型 §5.1 / FileSink 抽象 D4 / 环形缓冲溢出策略 §4.4 / 触发性能 <1ms 口径 D12 / 内存预算声明：buffer_frames×(n_analog×4+n_digital+8)B / GPU 不适用 §6.6 / 录波文件经 v0.110.0 云边同步上传）
  - [x] 5.3 `docs/protocols/fault-recorder-comtrade-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 录波流程图重绘 + Idle→Recording→Ready 状态机图）+ D1~D12 偏差表（与 spec.md 逐字一致）+ 性能口径声明（D12）
  - 验证：`cargo metadata` 解析成功；crate 测试全过（D1~D12 表自动化比对零差异）✅

- [x] **T6：版本同步 0.109.0 + 全量构建验证 + checklist 核验收工**
  - [x] 6.1 根 `Cargo.toml` version = "0.109.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` L3 注释；`gate.rs` 注释串尾 2 处追加 v0.109.0 类型清单（15 类型：FaultRecorder/RecorderConfig/RecorderState/RecorderError/FileSink/MockSink/RingSampleBuffer/TriggerCondition/TriggerType/ComtradeWriter/ComtradeConfig/ComtradeFormat/ChannelConfig/Phase/SampleRecord 按实际定稿）
  - [x] 6.2 §2.4.2 构建校验：C6 metadata / C7 本 crate 31 测试 + 全 workspace 回归 / C8 aarch64 交叉编译 / C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - [x] 6.3 `checklist.md` 逐项核验勾选 + 验收记录（首轮 77/80，C8/C45 差距转 T7 修复后回补 80/80）
  - 验证：C6~C11 全绿，checklist 全勾 + 验收记录已填，收工

- [x] **T7：修复 quantize() 舍入偏差（checklist 核验发现 C8/C45 未通过）**
  - [x] 7.1 `comtrade_writer.rs` 的 `quantize()` 补舍入（spec D9 声明 `raw=round((v-b)/a)`，实现误为向零截断）；实施注：no_std 下 `f64::round` 不可用（libm 方法），改手写舍入 `adj - (adj % 1.0)`（`adj = x ± 0.5`，half away from zero，语义等价）
  - [x] 7.2 新增 1 个可区分 round/trunc 的测试 `cw23_quantize_rounds_half_up`（v=12.36、a=0.1 → 期望 124；截断实现得 123；ASCII + Binary 双断言）；测试总数 31 → 32
  - [x] 7.3 设计文档 §9.2 补记本修复（第 6 条）；重跑 `cargo test -p eneros-fault-recorder`（32/32）+ fmt + clippy + aarch64 交叉编译全绿
  - 验证：C8/C45 复核通过，checklist 回补勾选

# Task Dependencies

- T1 先行（T2/T3 依赖 lib.rs 的 RecorderError/模块声明）
- T2 depends on T1；T3 depends on T1；T2 与 T3 互不依赖可并行
- T4 depends on T1 + T2 + T3（FaultRecorder 消费全部模块）
- T5 depends on T4（文档需最终代码签名）
- T6 depends on T5
- T7 depends on T6（checklist 核验发现的修复项）
