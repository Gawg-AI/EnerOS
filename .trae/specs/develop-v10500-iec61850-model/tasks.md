# Tasks — v0.105.0 IEC 61850 信息模型

> Spec：`spec.md`（develop-v10500-iec61850-model）。T1→T2 顺序（T2 消费 T1 类型）；T3 依赖 T2；T4/T5 顺序收尾。

- [x] **T1：新建 crate 骨架 + ld.rs / ln.rs / do_da.rs — 四层数据结构**
  - [x] 1.1 `crates/protocols/iec61850-model/Cargo.toml`：`eneros-iec61850-model`，workspace 继承，**零依赖**（D4）
  - [x] 1.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明（ld/ln/do_da/scl_parser）+ 重导出 + `Iec61850Model` trait（无 Send+Sync，D6）+ `ModelError`（SclParseError(String)/NotFound(String)/TypeMismatch(String)，D12）+ crate 文档（版本定位 + 核心类型 + D1~D13 偏差表 + no_std 合规声明，风格对齐 upa-model/solver-pareto）
  - [x] 1.3 `src/ld.rs`：`LogicalDevice`（全 pub，derive Debug/Clone/PartialEq）
  - [x] 1.4 `src/ln.rs`：`LogicalNode` + `LnClass`（XCBR/MMXU/PTRC/CSWI/GGIO/Other(String)）
  - [x] 1.5 `src/do_da.rs`：`DataObject`/`CommonDataClass`/`DataAttribute`/`FunctionalConstraint`(8 变体)/`DaValue`(7 变体)/`Quality`/`Validity`/`Source`（derive 按接口契约；DaValue 含 f32/f64 仅 PartialEq 不派生 Eq）
  - [x] 1.6 测试 LD1~LD4 + LN5~LN10 + DD11~DD20（20 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-iec61850-model ld:: ln:: do_da::` 20/20 全过 ✅

- [x] **T2：scl_parser.rs — 迷你 XML DOM + SCL 解析 + 路径索引 + trait 实现**
  - [x] 2.1 私有迷你 XML DOM 解析器（339 行 `xml.rs`，D4）：元素/属性/文本/嵌套/自闭合/XML 声明/注释/CDATA 跳过或取文本、实体转义解码（&amp;/&lt;/&gt;/&quot;/&apos;）、错误携带行:列位置；零 unsafe
  - [x] 2.2 `SclParser { lds: Vec<LogicalDevice>, path_index: BTreeMap<String, (usize,usize,usize,usize)> }` + `new()`；遍历 IED→LDevice→LN0+LN→DOI→DAI→Val 建模（LD 名 `{ied}_{inst}`；LN0 支持 D10；ln_ref = prefix+class+inst，LLN0 不带实例后缀；parse_ln_class/parse_fc（8 分支，未知 FC 报错 D8）/infer_cdc/parse_da_value（D9 规则））；重复 LD 名报错（D11）
  - [x] 2.3 路径索引构建：`LD/LN.DO.DA` → 四元组（D7）；`impl Iec61850Model`：load_scl（可重复调用追加）/get_ld/get_ln/get_da（Option）/set_da（NotFound 携路径、variant 判别式不一致 TypeMismatch，D12）/list_lds
  - [x] 2.4 测试 SP21~SP32（12 个，见 spec 测试规划表；性能 1000 查找 < 1ms 用 `std::time::Instant`，仅 cfg(test)，D13）
  - 验证：`cargo test -p eneros-iec61850-model` 33/33 全过 ✅（32 spec 项 + 1 独立性能测试 test_perf_1000_da_lookups）

- [x] **T3：workspace 接线 + 配置 + 设计文档**
  - [x] 3.1 根 `Cargo.toml` members 追加 `"crates/protocols/iec61850-model"`（protocols 段 tsn-time 之后）
  - [x] 3.2 `configs/iec61850-model.toml`：`[iec61850_model]` 配置模板 + 中文注释 7 点（SCL 自动解析选型 §5.1 / 路径语法 D7 / 未知 FC 策略 D8 / LN0 支持 D10 / 性能 <1ms §6.3 / 内存预算 / GPU 不适用 §6.6）
  - [x] 3.3 `docs/protocols/iec61850-model-design.md`：12 章节 + 2 Mermaid（SCL 解析流程图 + get_da 路径解析时序图）+ D1~D13 偏差表 + UPA 语义映射表（D13）+ 性能口径声明
  - 验证：`cargo metadata` 解析成功 ✅；`cargo test -p eneros-iec61850-model` 33 全过 ✅

- [x] **T4：版本同步 0.105.0 + 全量构建验证**
  - [x] 4.1 根 `Cargo.toml` version = "0.105.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` 注释；`gate.rs` 注释串尾 2 处追加 v0.105.0 类型清单（14 类型：LogicalDevice/LogicalNode/LnClass/DataObject/CommonDataClass/DataAttribute/FunctionalConstraint/DaValue/Quality/Validity/Source/Iec61850Model/SclParser/ModelError）
  - [x] 4.2 §2.4.2 构建校验：C6 metadata ✅ / C7 本 crate 33 + 全 workspace 回归 ✅ / C8 aarch64 交叉编译 ✅ / C9 fmt ✅ / C10 clippy -D warnings ✅（cargo clean 后 ICE 消除，0 warning）/ C11 cargo deny ✅
  - 验证：C6~C11 全绿 ✅

- [x] **T5：checklist 逐项核验收工**
  - [x] 5.1 `checklist.md` 逐项核验勾选 + 验收记录
  - 验证：checklist 全勾，收工 ✅

# Task Dependencies

- T1 先行（T2 消费 ld/ln/do_da 类型）
- T2 depends on T1
- T3 depends on T2（文档需最终代码签名）
- T4 depends on T3
- T5 depends on T4

# 实施备注（收工记录）

- **增量修复 1（T3 阶段）**：`do_da.rs` 的 `Quality`/`Validity`/`Source` 补齐 `Copy` derive（对齐 spec 接口契约与 checklist C42~C44）。
- **增量修复 2（T3 阶段）**：lib.rs crate 文档 D1~D13 偏差表重写为与 spec.md 逐字一致（C10）。
- **增量修复 3（T4 阶段）**：四个测试模块补 `#![allow(clippy::disallowed_macros)]`（项目惯例）；`ld.rs` LD4 删除冗余 `first().is_none()` 断言（clippy `unnecessary_first_then_check`）。
- **环境事件**：workspace clippy 一度因 nightly-2026-04-04 clippy-driver ICE 失败（eneros-net/sched/storage/heap/power 随机 crate，`the compiler unexpectedly panicked`），`cargo clean` 清除 20.6GiB 增量缓存后全绿；与本次代码变更无关。
- **测试计数说明**：spec 规划 32 项（LD1~4 + LN5~10 + DD11~20 + SP21~32）；实际 33 个 `#[test]`——SP 编号 12 项之外性能测试以独立函数 `test_perf_1000_da_lookups` 存在（spec 将其列为 SP 系列最后一项的覆盖点），总覆盖率 ≥ spec。
