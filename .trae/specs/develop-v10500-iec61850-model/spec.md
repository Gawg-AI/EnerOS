# v0.105.0 IEC 61850 信息模型 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.105.0（P2-G 第 1 版，IEC 61850 协议栈起点，9 节齐全）。新建 crate `crates/protocols/iec61850-model/`（eneros-iec61850-model）。蓝图检索确认无 v0.105.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1）。

## Why

联邦多机 IEC 61850 通信（MMS/GOOSE/SV）必须以统一的 LD/LN/DO/DA 信息模型为地基，无信息模型则 v0.106.0~v0.108.0 无从构建。蓝图要求实现 IEC 61850-7 系列四层层次模型 + SCL（XML）自动解析 + 路径索引快速查找（1000 次 DA 查找 < 1ms），为 IED 互操作奠定语义基础。

## What Changes

- **新建** `crates/protocols/iec61850-model/`（`eneros-iec61850-model`，no_std + alloc，**零依赖**）：
  - `src/ld.rs`：`LogicalDevice`（ld_name/ref_name/lns: Vec\<LogicalNode\>）
  - `src/ln.rs`：`LogicalNode`（ln_class/ln_inst/ln_prefix/do_list）+ `LnClass`（XCBR/MMXU/PTRC/CSWI/GGIO/Other）
  - `src/do_da.rs`：`DataObject`（do_name/da_list/cdc）+ `CommonDataClass`（SPS/DPS/MV/ENS/ACT/ASG/Other）+ `DataAttribute`（da_name/fc/value/quality/timestamp）+ `FunctionalConstraint`（ST/MX/CO/SP/SG/SE/BR/OR）+ `DaValue`（Bool/Int32/Float32/Float64/Enum/StringVal/Timestamp）+ `Quality`/`Validity`/`Source`
  - `src/scl_parser.rs`：`SclParser`（lds: Vec + path_index: BTreeMap，内置私有迷你 XML DOM 解析器，D4）+ `impl Iec61850Model`
  - `src/lib.rs`：`Iec61850Model` trait（无 Send+Sync，D6）+ `ModelError` + 模块声明 + 重导出 + crate 文档（含 D1~D13 偏差表）
- **新增** `configs/iec61850-model.toml`：`[iec61850_model]` SCL 解析配置模板 + 中文注释 ≥7 点
- **新增** `docs/protocols/iec61850-model-design.md`：12 章节 + ≥2 Mermaid + D1~D13 偏差表
- **新增 32 个单元测试**（src 内嵌 `#[cfg(test)]`：LD1~LD4 + LN5~LN10 + DD11~DD20 + SP21~SP32）
- 根 `Cargo.toml`：members 追加 `"crates/protocols/iec61850-model"` + version 0.104.0 → 0.105.0；`Makefile`（VERSION + 头部注释）/ `ci.yml` 注释 / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：纯新增 crate，既有 crate 零改动

## Impact

- Affected specs：develop-v10500-iec61850-model（新建）
- Affected code：`crates/protocols/iec61850-model/`（新建）、`configs/`、`docs/protocols/`、根 4 文件版本号
- 上游：v0.51.0 协议抽象层（protocol-abstract）、v0.50.0 统一点表（upa-model，语义对齐，零代码依赖，D13）
- 下游：v0.106.0 MMS 协议（消费本模型）、v0.107.0 GOOSE、v0.108.0 SV+IEC 62351

## ADDED Requirements

### Requirement: LD/LN/DO/DA 四层数据结构（ld.rs / ln.rs / do_da.rs）

The system SHALL provide 蓝图 §4.1 全部类型：`LogicalDevice` / `LogicalNode` / `LnClass` / `DataObject` / `CommonDataClass` / `DataAttribute` / `FunctionalConstraint` / `DaValue` / `Quality` / `Validity` / `Source`，字段全 pub，存储容器为 `alloc::vec::Vec`（替代蓝图 SlotMap，D4），字符串为 `alloc::string::String`（D5）。

#### Scenario: 四层层次构造
- **WHEN** 构造 LD → 压入 LN（XCBR, inst=1, prefix=""）→ 压入 DO（"Pos", DPS）→ 压入 DA（"stVal", ST, Bool(true)）
- **THEN** 四层嵌套深度正确，各层字段与蓝图 §4.1 一致

#### Scenario: 开放枚举扩展（蓝图 §9 可扩展）
- **WHEN** 遇到非标准 LN 类（如 "PDIS"）或 CDC（如 "WYE"）
- **THEN** 分别以 `LnClass::Other("PDIS")` / `CommonDataClass::Other("WYE")` 保留原始名，不丢弃不 panic

### Requirement: SCL 解析与模型构建（scl_parser.rs）

The system SHALL provide `SclParser`：内置迷你 XML DOM 解析器（元素/属性/文本/嵌套/自闭合/声明/注释/CDATA，实体转义解码，D4），按 IED → LDevice → LN0/LN → DOI → DAI → Val 遍历构建模型；LD 名 = `{ied_name}_{ld_inst}`；解析同时构建路径索引（四元组 ld/ln/do/da，D7）；支持 LN0（LLN0，D10）；重复 LD 名报错（D11）。

#### Scenario: 最小 SCL 端到端解析
- **WHEN** 输入含 1 个 IED("IED1") + LDevice(inst="LD0") + LN(lnClass="XCBR", inst="1") + DOI("Pos") + DAI("stVal", fc="ST", Val=1) 的 SCL XML
- **THEN** `list_lds()` == ["IED1_LD0"]；`get_ln("IED1_LD0", "XCBR1")` 为 Some；`get_da("IED1_LD0/XCBR1.Pos.stVal")` 为 Some 且 value == Int32(1)

#### Scenario: 畸形 SCL 故障注入（蓝图 §6.5）
- **WHEN** 输入未闭合标签 / 非法属性的 SCL
- **THEN** 返回 `Err(ModelError::SclParseError(_))`，错误信息含行列位置（蓝图 §4.4"返回详细错误位置"）

#### Scenario: 未知 FC 与重复 LD（D8/D11）
- **WHEN** DAI fc="XX9"（未知）或两个 LDevice 解析出同名 LD
- **THEN** 均返回 `Err(ModelError::SclParseError(_))`，不静默容错

### Requirement: 路径查询与写值（Iec61850Model trait）

The system SHALL provide `get_da(path)` / `set_da(path, value)`：路径语法统一 `LD/LN.DO.DA`（D7）；`get_da` 不存在返回 None；`set_da` 路径不存在返回 `NotFound`，DaValue variant 判别式不一致返回 `TypeMismatch`（D12），一致则更新 value 字段（quality/timestamp 不动）。

#### Scenario: 路径查找性能（蓝图 §6.3/§7.2）
- **WHEN** 模型含 ≥1000 个 DA，循环执行 1000 次 `get_da`
- **THEN** 总耗时 < 1ms（cfg(test) `std::time::Instant` 断言，D13）

#### Scenario: set_da 类型检查
- **WHEN** 对 value 为 Int32 的 DA 执行 `set_da(path, DaValue::Float64(1.0))`
- **THEN** 返回 `Err(ModelError::TypeMismatch(_))`；对不存在路径 set_da 返回 `Err(ModelError::NotFound(_))` 携带路径

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D13，相对蓝图 §3/§4/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/iec61850_model/` → `crates/protocols/iec61850-model/`（eneros-iec61850-model） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；IEC 61850 属设备协议栈，与 modbus/iec104 同 protocols 子系统 |
| **D2** | 蓝图 `docs/phase2/iec61850_model.md` → `docs/protocols/iec61850-model-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
| **D3** | 蓝图 `tests/iec61850_model.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.104.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 蓝图 roxmltree + SlotMap + HashMap → 零第三方依赖：内置迷你 XML DOM 解析器（~300 行，仅 SCL 子集：元素/属性/文本/嵌套/自闭合/声明/注释/CDATA/实体转义）+ `Vec` 存储 + `alloc::collections::BTreeMap` 路径索引 | 全项目 no_std + aarch64-unknown-none 交叉编译（记忆 §4.3）；cargo deny 离线 SBOM 零新增依赖先例；v0.104.0 D4 内置 PRNG 替代 rand 先例；SlotMap/HashMap 非 no_std/需 RandomState |
| **D5** | 蓝图 `String`/`Vec`/`format!`/`HashMap`（std）→ `alloc::string::String`/`alloc::vec::Vec`/`alloc::format!`/BTreeMap | 蓝图 §43.1 + 记忆 §4.3：全项目 Rust 代码必须 no_std |
| **D6** | 蓝图 `Iec61850Model: Send + Sync` → 去除 bound | 与 v0.64.0 `Solver` / v0.104.0 `ParetoSolver` 惯例一致；单线程数据模型无跨线程需求 |
| **D7** | 蓝图 bug 修复①：`path_index` 值三元组 `(LnId, do_idx, da_idx)` 缺 LN 索引（DA 无法回溯）→ 四元组 `(ld, ln, do_, da)`；路径语法统一为 `LD/LN.DO.DA`（蓝图 §4.2 注释 "LD/LN.DO.DA" 与 §4.5 `format!("{}/{}/{}.{}")` = "LD/LN/DO.DA" 自相矛盾） | IEC 61850 引用惯例即 `LD/LN.DO.DA`；不修复则 get_da/set_da 无法实现（Karpathy：surface inconsistencies） |
| **D8** | 蓝图 bug 修复②：`parse_fc` 缺 `"OR"` 分支（落入 `_ => ST`）→ 补齐 8 分支；未知 FC 静默映射 ST → 返回 `SclParseError` | 静默映射丢失语义且与蓝图 §9"解析容错"的容错语义（可诊断）冲突；故障注入测试（§6.5）要求错误可报告 |
| **D9** | 蓝图 bug 修复③：`parse_da_value` 的 stVal 分支 `"true"` 被 `unwrap_or(Bool(val=="1"))` 误判为 `Bool(false)` → 明确规则：true/false→Bool、可解析整数→Int32、其余→StringVal | 蓝图代码逻辑错误，直接运行将产生错误模型值（Karpathy：不带着疑问照抄） |
| **D10** | 蓝图仅过滤 `"LN"` 元素 → 同时解析 `"LN0"`（真实 SCL 每个 LDevice 必含 LLN0；ln_class=`Other("LLN0")`，inst=0，ln_ref 不带实例号后缀即 "LLN0"） | IEC 61850-6：LLN0 为每 LD 强制节点；蓝图遗漏导致真实 SCL 模型不完整 |
| **D11** | 重复 LD 名 → `SclParseError`（蓝图未定义重复行为） | IEC 61850 LD 名系统内唯一；静默接受将致路径索引覆盖、查询结果不确定（确定性优先） |
| **D12** | `ModelError::NotFound` / `TypeMismatch` 裸变体 → `NotFound(String)` 携带路径 / `TypeMismatch(String)` 携带期望与实际 variant；`set_da` 类型检查定义为 DaValue variant 判别式一致（quality/timestamp 不参与） | 蓝图 §4.4 仅给变体名未定义载荷；诊断信息为运维必需（蓝图 §9 可观测） |
| **D13** | 蓝图 §6.4"点表映射一致性"回归 → 零依赖交付：设计文档给出 `Quality.validity`↔UPA `PointQuality`、`DaValue`↔`PointValue` 语义映射表 + 语义对齐测试；性能 1000 查找 < 1ms 落地为 cfg(test) `Instant` 断言 | UPA 协议适配属 v0.51.0 适配层职责（upa-model D6 零耦合先例）；本 crate 不引入 eneros 依赖（v0.104.0 D12 测试计时先例） |

## 接口契约

```rust
// lib.rs
pub trait Iec61850Model {        // 无 Send+Sync（D6）
    fn load_scl(&mut self, scl_xml: &str) -> Result<(), ModelError>;
    fn get_ld(&self, ld_name: &str) -> Option<&LogicalDevice>;
    fn get_ln(&self, ld_name: &str, ln_ref: &str) -> Option<&LogicalNode>;
    fn get_da(&self, path: &str) -> Option<&DataAttribute>;   // "LD/LN.DO.DA"（D7）
    fn set_da(&mut self, path: &str, value: DaValue) -> Result<(), ModelError>;
    fn list_lds(&self) -> Vec<&str>;
}
pub enum ModelError {
    SclParseError(String),   // 含行:列位置（§4.4）
    NotFound(String),        // 携带路径（D12）
    TypeMismatch(String),    // 携带期望/实际 variant（D12）
}  // Debug/Clone/PartialEq

// ld.rs
pub struct LogicalDevice {
    pub ld_name: String, pub ref_name: String,
    pub lns: Vec<LogicalNode>,              // Vec 替代 SlotMap（D4）
}  // Debug/Clone/PartialEq

// ln.rs
pub struct LogicalNode {
    pub ln_class: LnClass, pub ln_inst: u16,
    pub ln_prefix: String, pub do_list: Vec<DataObject>,
}  // Debug/Clone/PartialEq
pub enum LnClass { XCBR, MMXU, PTRC, CSWI, GGIO, Other(String) }  // Debug/Clone/PartialEq

// do_da.rs
pub struct DataObject {
    pub do_name: String, pub da_list: Vec<DataAttribute>, pub cdc: CommonDataClass,
}  // Debug/Clone/PartialEq
pub enum CommonDataClass { SPS, DPS, MV, ENS, ACT, ASG, Other(String) }  // Debug/Clone/PartialEq
pub struct DataAttribute {
    pub da_name: String, pub fc: FunctionalConstraint,
    pub value: DaValue, pub quality: Quality, pub timestamp: u64,
}  // Debug/Clone/PartialEq
pub enum FunctionalConstraint { ST, MX, CO, SP, SG, SE, BR, OR }  // Debug/Clone/Copy/PartialEq
pub enum DaValue {
    Bool(bool), Int32(i32), Float32(f32), Float64(f64),
    Enum(u16), StringVal(String), Timestamp(u64),
}  // Debug/Clone/PartialEq（f32/f64 → 不派生 Eq）
pub struct Quality {
    pub validity: Validity, pub source: Source,
    pub test: bool, pub operator_blocked: bool,
}  // Debug/Clone/Copy/PartialEq
pub enum Validity { Good, Invalid, Reserved, Questionable }  // Debug/Clone/Copy/PartialEq
pub enum Source { Process, Substituted }                     // Debug/Clone/Copy/PartialEq

// scl_parser.rs
pub struct SclParser {
    lds: Vec<LogicalDevice>,                       // 私有
    path_index: BTreeMap<String, (usize, usize, usize, usize)>,  // 私有，四元组（D7）
}
impl SclParser { pub fn new() -> Self; }
impl Iec61850Model for SclParser { /* load_scl = 迷你 DOM 解析 → 遍历 IED/LDevice/LN0+LN/DOI/DAI/Val → 建模 + 索引 */ }
```

## 测试规划（iec61850-model 32 个，src 内嵌）

| 文件 | 编号 | 数量 | 覆盖 |
|------|------|------|------|
| ld.rs | LD1~LD4 | 4 | LD 构造字段 / lns Vec 压入与 len / Debug+Clone derive / 空 lns 行为 |
| ln.rs | LN5~LN10 | 6 | LnClass 五标准变体 + Other / LogicalNode 构造 / ln_inst+prefix / do_list 压入 / Debug+Clone+PartialEq / Other(String) 相等比较 |
| do_da.rs | DD11~DD20 | 10 | DaValue 7 变体构造 / DaValue PartialEq（同变体等、异变体不等）/ Quality 4 字段 / Validity 4 变体 / Source 2 变体 / FC 8 变体（含 OR，D8）/ CDC 7 变体 / DataObject 构造 / DataAttribute 构造含 timestamp / Debug+Clone derive |
| scl_parser.rs | SP21~SP32 | 12 | 迷你 XML（元素/属性/文本/嵌套/自闭合/声明注释跳过）/ 实体转义解码 / e2e 最小 SCL 建模（LD/LN/DO/DA 树 + Int32(1)）/ LN0 解析（LLN0，D10）/ get_da "LD/LN.DO.DA" 命中 / get_ld+get_ln+list_lds / set_da 成功更新 / set_da NotFound / set_da TypeMismatch / 畸形 XML 报错含位置 / 未知 FC + 重复 LD 名报错 / 1000 次 get_da < 1ms（Instant，D13） |

## 配置与文档

- `configs/iec61850-model.toml`：`[iec61850_model]` max_lds / max_lns_per_ld / max_dos_per_ln / max_das_per_do / path 语法说明 / unknown_fc 策略（error）/ allow_duplicate_ld = false + 中文注释 ≥7 点（SCL 自动解析选型 §5.1 / 路径语法 D7 / 未知 FC 策略 D8 / LN0 支持 D10 / 性能 <1ms §6.3 / 内存预算声明 / GPU 不适用 §6.6）
- `docs/protocols/iec61850-model-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 SCL 解析流程图重绘 + get_da 路径解析时序图）+ D1~D13 偏差表 + UPA 语义映射表（D13）+ 性能口径声明

## 版本同步

根 `Cargo.toml` version = "0.105.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` 注释；`gate.rs` 注释串尾 2 处追加 v0.105.0 类型清单（LogicalDevice/LogicalNode/LnClass/DataObject/CommonDataClass/DataAttribute/FunctionalConstraint/DaValue/Quality/Validity/Source/Iec61850Model/SclParser/ModelError）。
