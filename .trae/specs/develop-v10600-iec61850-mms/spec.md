# v0.106.0 IEC 61850 MMS 协议 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.106.0（P2-G 第 2 版，9 节齐全）。新建 crate `crates/protocols/iec61850-mms/`（eneros-iec61850-mms），依赖 eneros-iec61850-model（v0.105.0）。蓝图检索确认无 v0.106.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1）。

## Why

边缘设备与变电站认证 IED 的标准互操作需要 MMS（IEC 61850-8-1）协议栈。v0.105.0 已落地 LD/LN/DO/DA 信息模型，本版在其类型基座上实现 BER 编解码 + ACSE 关联 + MMS Read/Write 服务，打通联邦多机 IEC 61850 通信的服务层，为 v0.107.0 GOOSE 奠基。

## What Changes

- **新建** `crates/protocols/iec61850-mms/`（`eneros-iec61850-mms`，no_std + alloc，依赖仅 `eneros-iec61850-model` path 引用）：
  - `src/ber_encode.rs`：`BerEncoder`（tag+长度占位+内容+回填；encode_read_request / encode_write_request，长度恒为字节数，D6）
  - `src/ber_decode.rs`：`decode_read_response` / `decode_write_response` / `read_tag_length`（长/短型长度；浮点按长度右对齐，4→Float32、8→Float64，D7）
  - `src/acse.rs`：`encode_aarq` / `decode_aare` + COTP CR/CC 辅助（定长结构简化，D9）
  - `src/mms_client.rs`：`MmsClient<T: MmsTransport>`（泛型传输，D4）+ `MmsConnection`/`ConnState` + `MmsRequest`/`MmsResponse`/`VarAccessSpec`/`MmsReadResult`/`MmsWriteResult`/`MmsErrorCode` + `MmsTransport` trait + `MockTransport`
  - `src/lib.rs`：`MmsError`（7 变体，D10）+ 模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新增** `configs/iec61850-mms.toml`：`[ied]` 连接配置模板 + 中文注释 ≥7 点
- **新增** `docs/protocols/iec61850-mms-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 38 个单元测试**（src 内嵌 `#[cfg(test)]`：BE1~BE10 + BD11~BD20 + AC21~AC26 + MC27~MC38）
- 根 `Cargo.toml`：members 追加 `"crates/protocols/iec61850-mms"` + version 0.105.0 → 0.106.0；`Makefile`（VERSION + 头部注释）/ `ci.yml` 注释 / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：纯新增 crate，既有 crate 零改动（eneros-iec61850-model 仅被引用不修改）

## Impact

- Affected specs：develop-v10600-iec61850-mms（新建）
- Affected code：`crates/protocols/iec61850-mms/`（新建）、`configs/`、`docs/protocols/`、根 4 文件版本号
- 上游：v0.105.0 eneros-iec61850-model（DaValue/Quality/Validity/Source 类型复用）、v0.29.0 Socket（真实接线在集成层，D4）
- 下游：v0.107.0 GOOSE、v0.108.0 SV+IEC 62351

## ADDED Requirements

### Requirement: BER 编解码（ber_encode.rs / ber_decode.rs）

The system SHALL provide `BerEncoder`：以「tag + 0x00 长度占位 + 内容 + 回填」构造 MMS ConfirmedRequestPDU（0xA0）Read（0xA4）/Write（0xA5）请求；所有 BER 长度为内容字节数（短型 <0x80 单字节，否则 0x82 双字节长型）；解码侧 `read_tag_length` 支持长短两型，`decode_read_response` 识别 boolean(0x80)/integer(0x85)/floating-point(0x87)，浮点按实际长度右对齐解码为 Float32(4)/Float64(8)，未知 tag 跳过得 None。

#### Scenario: Read 请求编码结构正确（D6）
- **WHEN** 编码 2 个 VarAccessSpec（domain="IED1_LD0", item="XCBR1.Pos.stVal" 等）
- **THEN** 字节流可被 `read_tag_length` 逐层解出：0xA0 长度 == 其后全部内容字节数；0xA0(listOfVariable) 长度 == 两个条目字节数和（非元素个数 2）

#### Scenario: 浮点解码（D7）
- **WHEN** 响应含 0x87 长度 4 的浮点（f32 1.5 的 BE 字节）与长度 8 的浮点
- **THEN** 分别解出 `DaValue::Float32(1.5)` 与 `DaValue::Float64(_)`；截断输入返回 `Err(MmsError::BerDecodeError)`

### Requirement: ACSE 关联与 COTP（acse.rs）

The system SHALL provide `encode_aarq(ap_title)`（AARQ 0x60 + AP-title VisibleString）与 `decode_aare`（AARE 0x61 接受 → Ok；拒绝 → `Err(IedError(Refused))`；畸形 → BerDecodeError）；COTP CR 编码（定长简化结构）与 CC 解析（D9）。

#### Scenario: 关联时序（蓝图 §4.3）
- **WHEN** `MmsClient::connect` 被调用
- **THEN** 传输层依次发出 COTP CR → ACSE AARQ；收到 CC + AARE 后 `state == Connected`

### Requirement: MMS 客户端读写服务（mms_client.rs）

The system SHALL provide `MmsClient<T: MmsTransport>`：`connect`（COTP+ACSE，超时重试至多 3 次，D11）/`read`（编码 → 发送 → 接收 → 解码，保序）/`write`（同构）/`disconnect`（state → Idle）；未连接调用 read/write 返回 `NotConnected`；接收错误置 `state = Error`；`MmsTransport { connect/send/recv }` 由 `MockTransport` 提供脚本化响应用于测试与集成占位。

#### Scenario: 连接重试（蓝图 §4.4，D11）
- **WHEN** MockTransport 前 2 次 connect 返回超时、第 3 次成功
- **THEN** connect 返回 Ok，尝试计数 == 3；3 次全超时 → `Err(MmsError::Timeout)`，state == Error

#### Scenario: 断连重连（蓝图 §6.5 故障注入）
- **WHEN** 已连接状态下 recv 返回 TransportError
- **THEN** read 返回 Err 且 state == Error；再次 `connect` 成功后 read 恢复

#### Scenario: 100 点读取性能（蓝图 §6.3/§7.2，D12）
- **WHEN** 构造 100 个 VarAccessSpec，mock 回路完成一次 read（编码+解码口径）
- **THEN** 耗时 < 50ms（cfg(test) `std::time::Instant` 断言）；结果数 == 100 且保序

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/iec61850_mms/` → `crates/protocols/iec61850-mms/`（eneros-iec61850-mms） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；与 modbus/iec104/iec61850-model 同 protocols 子系统 |
| **D2** | 蓝图 `docs/phase2/mms_protocol.md` → `docs/protocols/iec61850-mms-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
| **D3** | 蓝图 `tests/mms_client.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.105.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 新增 `MmsTransport` trait（connect/send/recv）+ `MockTransport`（置于 mms_client.rs，不新增文件）；`MmsClient<T: MmsTransport>` 泛型化；v0.29.0 Socket 真实接线在集成层 | 蓝图 §4.3 时序需要传输层但 §4.1/§4.5 无抽象；mqtt/iec104/agent-bus-dds 同先例（crate 内 trait+Mock，无真实网络 I/O）；no_std 主机可测 |
| **D5** | 蓝图 §4.1 `model: Arc<Iec61850Model>` 字段删除 | 蓝图 §4.5 全部代码从未使用该字段（read/write 仅以字符串 VarAccessSpec 操作），死字段（Karpathy Simplicity First）；DaValue/Quality 等类型经 eneros-iec61850-model crate 依赖保留；GetVariableAccessAttributes 的模型消费在后续版本接入 |
| **D6** | 蓝图 bug 修复①：BER 编码长度回填（`write_tag` 后无占位字节即写内容，`backfill_length` 会覆盖后续 tag；listOfVariable 用 `vars.len()` 元素个数冒充字节长度）→ tag+0x00 占位+内容+回填，长度恒为内容字节数 | 蓝图代码直接运行产出畸形 BER（Karpathy：不带着疑问照抄）；BER 长度语义为字节数（X.690） |
| **D7** | 蓝图 bug 修复②：浮点解码 `bytes[..copy_len]` 左对齐致 4 字节浮点错位 → 按 val_len 右对齐，4→`Float32`、8→`Float64`（蓝图一律 Float64） | IEC 61850 测量值可为 32 位浮点；左对齐解码数值错误 |
| **D8** | std `String`/`Vec`/`Arc` → `alloc::*`；trait/struct 无 Send+Sync bound | 蓝图 §43.1 + 记忆 §4.3 全项目 no_std；与 v0.64.0/v0.105.0 去 bound 惯例一致 |
| **D9** | COTP CR/CC 辅助（定长简化结构）放入 acse.rs（蓝图文件清单无 cotp.rs）；COTP 数据 TPDU 头在 mms_client 内联 | §4.3 时序含 COTP 握手但 §3 交付物无对应文件；acse.rs 同属关联建立层，不新增文件（Simplicity First）；真实 COTP 选项协商在集成层 |
| **D10** | 错误模型统一：`MmsError` = Timeout/ConnRefused/NotConnected/BerDecodeError/TransportError/IedError(MmsErrorCode)；§4.4"BER 解码失败→MmsErrorCode::Unknown"与 §4.5 代码 `MmsError::BerDecodeError` 矛盾 → 采用代码侧 | 蓝图自相矛盾（Karpathy：surface inconsistencies）；BerDecodeError 可区分本地解码失败与对端拒绝 |
| **D11** | 连接重试：§4.4"超时重试 3 次" → connect 至多 3 次尝试，第 3 次失败返回 Timeout；无 sleep（传输层内部决定超时语义），重试计数经 MockTransport 断言 | no_std 无计时器（v0.64.0 D1 时间注入先例）；重试次数上限语义与蓝图一致 |
| **D12** | 性能 100 点 < 50ms 落地为 cfg(test) Instant 断言（mock 回路，编码+解码口径，文档声明）；§6.2"与认证 IED 通信"集成测试为实验室硬件项，以 MockTransport 脚本化响应替代 | 无真实 IED 硬件（与 v0.105.0 D13 同口径）；v0.104.0 D12 测试计时先例 |

## 接口契约

```rust
// lib.rs
pub enum MmsError {
    Timeout, ConnRefused, NotConnected, BerDecodeError,
    TransportError, IedError(MmsErrorCode),
}  // Debug/Clone/PartialEq（D10）

// mms_client.rs
pub trait MmsTransport {                        // D4
    fn connect(&mut self, addr: &str, port: u16) -> Result<(), MmsError>;
    fn send(&mut self, pdu: &[u8]) -> Result<(), MmsError>;
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, MmsError>;
}
pub struct MmsConnection {
    pub peer_addr: String, pub peer_port: u16,
    pub local_ap_title: String, pub state: ConnState,
}  // Debug/Clone/PartialEq
pub enum ConnState { Idle, Connecting, Connected, Error }  // Debug/Clone/Copy/PartialEq
pub struct VarAccessSpec { pub domain: String, pub item: String }  // Debug/Clone/PartialEq
pub enum MmsRequest {                           // 蓝图 §4.1 全量（含前瞻变体，D5）
    Read { variable_access: Vec<VarAccessSpec> },
    Write { variable_access: Vec<(VarAccessSpec, DaValue)> },
    GetVariableAccessAttributes { domain: String, item: String },
    DefineNamedVariableList { name: String, entries: Vec<VarAccessSpec> },
}  // Debug/Clone/PartialEq
pub enum MmsResponse {
    ReadResult { results: Vec<MmsReadResult> },
    WriteResult { results: Vec<MmsWriteResult> },
    Error { code: MmsErrorCode },
}  // Debug/Clone/PartialEq
pub struct MmsReadResult { pub value: Option<DaValue>, pub quality: Quality, pub timestamp: u64 }  // Debug/Clone/PartialEq
pub enum MmsWriteResult { Success, Failed(String) }  // Debug/Clone/PartialEq
pub enum MmsErrorCode { Timeout, Refused, NotFound, TypeMismatch, Unknown(u16) }  // Debug/Clone/Copy/PartialEq
pub struct MmsClient<T: MmsTransport> { conn: MmsConnection, transport: T, timeout_ms: u32, invoke_id: u32 }
impl<T: MmsTransport> MmsClient<T> {
    pub fn new(transport: T, local_ap_title: &str, timeout_ms: u32) -> Self;
    pub fn connect(&mut self, addr: &str, port: u16) -> Result<(), MmsError>;   // 重试 ≤3（D11）
    pub fn read(&mut self, vars: &[VarAccessSpec]) -> Result<MmsResponse, MmsError>;
    pub fn write(&mut self, vars: &[(VarAccessSpec, DaValue)]) -> Result<MmsResponse, MmsError>;
    pub fn disconnect(&mut self);
    pub fn conn_state(&self) -> ConnState;
}
pub struct MockTransport { /* 脚本化响应 + 尝试计数（测试/集成占位，D4） */ }

// ber_encode.rs
pub struct BerEncoder { buffer: Vec<u8> }
impl BerEncoder {
    pub fn new() -> Self;
    pub fn encode_read_request(&mut self, invoke_id: u32, vars: &[VarAccessSpec]) -> &[u8];
    pub fn encode_write_request(&mut self, invoke_id: u32, vars: &[(VarAccessSpec, DaValue)]) -> &[u8];
}

// ber_decode.rs
pub fn decode_read_response(data: &[u8]) -> Result<Vec<MmsReadResult>, MmsError>;
pub fn decode_write_response(data: &[u8]) -> Result<Vec<MmsWriteResult>, MmsError>;
pub fn read_tag_length(data: &[u8], pos: &mut usize) -> Result<(u8, usize), MmsError>;

// acse.rs
pub fn encode_aarq(ap_title: &str) -> Vec<u8>;
pub fn decode_aare(data: &[u8]) -> Result<(), MmsError>;
pub fn encode_cotp_cr() -> Vec<u8>;      // 定长简化（D9）
pub fn decode_cotp_cc(data: &[u8]) -> Result<(), MmsError>;
```

## 测试规划（iec61850-mms 38 个，src 内嵌）

| 文件 | 编号 | 数量 | 覆盖 |
|------|------|------|------|
| ber_encode.rs | BE1~BE10 | 10 | read 请求 0xA0/0xA4 tag / invokeID 编码 / domain+item VisibleString / 单变量字节长度正确 / 多变量 listOfVariable 长度为字节和（非个数，D6）/ ≥0x80 长型长度 / write 请求 0xA5 / Bool 值编码 / Int32 值编码 / Float64 值编码 |
| ber_decode.rs | BD11~BD20 | 10 | boolean 0x80 解码 / integer 0x85 多字节 / float 4B→Float32（D7）/ float 8B→Float64 / 未知 tag → None / 截断 → BerDecodeError / 长型长度解析 / write 响应 Success / write 响应 Failed / 顶层 tag 非法 → Err |
| acse.rs | AC21~AC26 | 6 | AARQ 含 0x60+ap_title / AARE 接受 → Ok / AARE 拒绝 → IedError(Refused) / 畸形 → BerDecodeError / COTP CR 定长结构 / COTP CC 解析 |
| mms_client.rs | MC27~MC38 | 12 | new 初始 Idle / connect 成功状态机 Idle→Connecting→Connected / 时序：先发 COTP CR 再发 AARQ（mock 记录）/ 重试 2 次后第 3 次成功（D11）/ 3 次全超时 → Timeout+Error / read mock 回路结果 / 未连接 read → NotConnected / write Success+Failed / disconnect → Idle / recv 错误 → state Error → 重连恢复 / 100 点 read < 50ms 且保序（D12）/ MmsResponse::Error code 映射 |

## 配置与文档

- `configs/iec61850-mms.toml`：`[ied]` peer_addr / peer_port = 102 / local_ap_title / timeout_ms = 3000 / connect_retry = 3 + 中文注释 ≥7 点（自研 BER 选型 §5.1 / MMS over TCP 102 端口 / 重试 3 次 §4.4 D11 / 传输抽象 D4 / 性能 100 点 <50ms §6.3 / 内存预算声明 / GPU 不适用 §6.6 / 安全待 v0.108.0 §7.3）
- `docs/protocols/iec61850-mms-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 COTP/ACSE/MMS 时序图重绘 + BER 编码结构图）+ D1~D12 偏差表 + 性能口径声明（D12）

## 版本同步

根 `Cargo.toml` version = "0.106.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` 注释；`gate.rs` 注释串尾 2 处追加 v0.106.0 类型清单（MmsClient/MmsConnection/ConnState/MmsRequest/MmsResponse/VarAccessSpec/MmsReadResult/MmsWriteResult/MmsErrorCode/MmsError/MmsTransport/MockTransport/BerEncoder）。
