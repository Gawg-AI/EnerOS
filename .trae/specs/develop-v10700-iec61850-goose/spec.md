# v0.107.0 IEC 61850 GOOSE 快速事件传输 Spec

> 蓝图：`e:\eneros\蓝图\phase2.md` §v0.107.0（P2-G 第 3 版，9 节齐全）。新建 crate `crates/protocols/iec61850-goose/`（eneros-iec61850-goose），依赖 eneros-iec61850-model（DaValue 复用）。蓝图检索确认无 v0.107.x 刚性子版本（Phase 2 刚性子版本仅 v0.98.1）。

## Why

电力保护级快速事件传输要求 GOOSE（IEC 61850-8-1，EtherType 0x88B8）二层组播直发，端到端 < 4ms。v0.106.0 已落地 MMS 服务层，本版实现 GOOSE 发布/订阅 + BER 编解码 + st_num/sq_num 重传状态机，打通联邦保护协同的事件通道，为 v0.108.0 SV+IEC 62351 安全加固奠基。

## What Changes

- **新建** `crates/protocols/iec61850-goose/`（`eneros-iec61850-goose`，no_std + alloc，依赖仅 `eneros-iec61850-model` path 引用）：
  - `src/dataset.rs`：`GooseDataset` / `GooseEntry`（path + DaValue）
  - `src/goose_tx.rs`：`GooseControlBlock` + `GoosePublisher<T: L2Transport>`（泛型传输注入，D4/D5；以太网头 + GOOSE PDU BER 编码，allData 0xAB 补长度 D7；数据 tag 统一 D8；时间注入 D6）
  - `src/goose_rx.rs`：`GoosePdu` / `RxStatus`（New/Duplicate/StJump，D12）+ `GooseSubscriber<T: L2Transport>`（APPID/MAC 过滤、st_num 跳变检测、回调）
  - `src/lib.rs`：`GooseError`（4 变体，D10）+ `L2Transport` trait + `MockL2` + 模块声明 + 重导出 + crate 文档（含 D1~D12 偏差表）
- **新增** `configs/iec61850-goose.toml`：`[gocb]` 控制块配置模板 + 中文注释 ≥7 点
- **新增** `docs/protocols/iec61850-goose-design.md`：12 章节 + ≥2 Mermaid + D1~D12 偏差表
- **新增 36 个单元测试**（src 内嵌 `#[cfg(test)]`：DS1~DS6 + TX7~TX18 + RX19~RX30 + LB31~LB36）
- 根 `Cargo.toml`：members 追加 `"crates/protocols/iec61850-goose"` + version 0.106.0 → 0.107.0；`Makefile`（VERSION + 头部注释）/ `ci.yml` 注释 / `gate.rs` 注释串尾 2 处同步
- **无 BREAKING**：纯新增 crate，既有 crate 零改动

## Impact

- Affected specs：develop-v10700-iec61850-goose（新建）
- Affected code：`crates/protocols/iec61850-goose/`（新建）、`configs/`、`docs/protocols/`、根 4 文件版本号
- 上游：v0.106.0 eneros-iec61850-mms（BER TLV 规则复用）、v0.105.0 eneros-iec61850-model（DaValue）、v0.27.0 网卡驱动（真实 L2 接线在集成层，D4）
- 下游：v0.108.0 SV + IEC 62351 安全

## ADDED Requirements

### Requirement: GOOSE 数据集（dataset.rs）

The system SHALL provide `GooseDataset { entries: Vec<GooseEntry> }` 与 `GooseEntry { path: String, value: DaValue }`：支持条目追加、按路径更新覆盖、按路径查找；DaValue 复用 eneros-iec61850-model。

#### Scenario: 更新语义
- **WHEN** 对已有 path 调用更新
- **THEN** 原条目 value 被覆盖而非新增；新 path 则追加

### Requirement: GOOSE 发布者（goose_tx.rs）

The system SHALL provide `GoosePublisher<T: L2Transport>`：`update_value`（st_num+1、sq_num=0、needs_retransmit=true）/ `publish(now)`（组帧：dst MAC + 组播 src MAC 01:0C:CD:01:00:00 + EtherType 0x88B8 + GOOSE PDU：gocbRef 0x80 / timeAllowedToLive 0x81 / datSet 0x82 / goID 0x83 / t 0x84 8 字节 / stNum 0x85 / sqNum 0x86 / simulation 0x87 / confRef 0x88 / ndsCom 0x89 / numDatSetEntries 0x8A / allData 0xAB **含长度**（D7）；发送后 sq_num+1、last_tx_time=now）/ `retransmit_if_needed(now)`（前 3 次 min_time 间隔、其后 max_time 周期心跳，蓝图 §4.3）。

#### Scenario: 重传时序（蓝图 §4.3）
- **WHEN** 事件触发后依次以 now = T0+min_time、+2×min_time、+3×min_time、+max_time 调用 retransmit_if_needed
- **THEN** 前 3 次按 min_time 间隔重发，其后按 max_time 周期重发；每次发送 sq_num 递增、st_num 不变

#### Scenario: BER 结构合规（D7/D8）
- **WHEN** publish 组帧完成
- **THEN** 帧内每层 TLV 可被 v0.106.0 `read_tag_length` 逐层解出；allData 0xAB 携带内容字节长度；数据条目 tag 为 boolean 0x80 / integer 0x85 / floating-point 0x87（与 MMS 栈一致，D8）

### Requirement: GOOSE 订阅者（goose_rx.rs）

The system SHALL provide `GooseSubscriber<T: L2Transport>`：`poll()` 接收帧 → 非 0x88B8 返回 Ok(None) → dst MAC 不匹配丢弃 Ok(None) → APPID 不匹配丢弃 Ok(None) → 解码 GOOSE PDU → st_num > last_st_num 且跳变 >1 标记 `RxStatus::StJump`（事件丢失，蓝图 §4.4，D12）、st_num==last 且 sq_num 递增为 `New`（重传帧）、完全重复为 `Duplicate`；`set_callback` 注册后每收到有效 PDU 调用一次。

#### Scenario: 丢帧检测（蓝图 §6.5 故障注入）
- **WHEN** 先收 st_num=5，再收 st_num=7（6 丢失）
- **THEN** 第二次 poll 返回 `(pdu, RxStatus::StJump)`，pdu.st_num == 7

#### Scenario: 端到端 < 4ms（蓝图 §6.3/§7.2，D11）
- **WHEN** MockL2 回路 publish → poll 全链路（编码+传输+解码口径）
- **THEN** 耗时 < 4ms（cfg(test) `std::time::Instant` 断言）；数据集值与保序一致

## MODIFIED Requirements

无（纯新增 crate，既有 crate 零改动）。

## REMOVED Requirements

无。

## 偏差声明（D1~D12，相对蓝图 §3/§4/§6）

| 编号 | 偏差 | 理由 |
|------|------|------|
| **D1** | 蓝图 `crates/iec61850_goose/` → `crates/protocols/iec61850-goose/`（eneros-iec61850-goose） | 记忆 §2.3.1 强制：crate 归 `crates/<subsystem>/`；与 mms/iec61850-model 同 protocols 子系统 |
| **D2** | 蓝图 `docs/phase2/goose.md` → `docs/protocols/iec61850-goose-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
| **D3** | 蓝图 `tests/goose_latency.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.106.0 项目惯例，不新增 tests/ 文件 |
| **D4** | 删除蓝图 §4.5 `extern "C"` raw socket FFI + unsafe；新增 `L2Transport` trait（send/recv）+ `MockL2`（置于 lib.rs）；真实 raw socket 接线在集成层 | aarch64-unknown-none 无 libc 可链接 extern "C"；主机不可测；项目零 unsafe/零 C FFI 惯例；与 v0.106.0 D4 MmsTransport 同先例 |
| **D5** | `GoosePublisher<T: L2Transport>` / `GooseSubscriber<T: L2Transport>` 泛型化，transport 由 `new` 注入（蓝图内部建 socket 写死 "eth0"） | 可测试性 + 网卡选择属集成层决策（Karpathy Simplicity First） |
| **D6** | 时间注入：`publish(now: u64)` / `retransmit_if_needed(now)` 使用外部时间参数；蓝图 `current_time_ms()` 未定义 | no_std 无系统时间（v0.64.0 D1 时间注入先例）；重传间隔判定需要时钟源 |
| **D7** | 蓝图 bug 修复①：allData 0xAB 只有 tag 无长度字段（条目直接尾随）→ 补「tag + 长度 + 内容」完整 TLV | BER TLV 合规（X.690）；无长度则接收端无法确定条目边界 |
| **D8** | 蓝图 bug 修复②：allData 数据 tag（Bool 0x01 / Int32 0x03 / Float64 0x85）与 v0.106.0 MMS 解码约定冲突 → 统一 boolean 0x80 / integer 0x85 / floating-point 0x87（4B→Float32、8B→Float64） | IEC 61850-8-1 数据 tag 与 MMS 一致；栈内编解码对称，rx 可复用 v0.106.0 解码规则 |
| **D9** | `rx_callback: Box<dyn Fn + Send + Sync>` → 去 Send+Sync bound | 蓝图 §43.1 no_std 全项目去 bound 惯例（v0.64.0/v0.105.0/v0.106.0 一致） |
| **D10** | 错误模型统一：`GooseError` = TransportError / BerEncodeError / BerDecodeError / InvalidConfig（4 变体）；蓝图 SocketCreateFailed/SendFailed 随 FFI 删除合并为 TransportError | FFI 删除后原错误无来源；4 变体覆盖组帧/解码/传输/配置全部失败面（对齐 v0.106.0 D10 精简风格） |
| **D11** | 性能 < 4ms 落地为 cfg(test) Instant 断言（MockL2 回路，编码+传输+解码全链路口径，文档声明）；§6.2 真实网卡端到端为实验室硬件项，以 MockL2 脚本化帧替代 | 无真实网卡硬件（与 v0.106.0 D12 同口径） |
| **D12** | 接收侧 st_num 跳变检测以 `RxStatus`（New/Duplicate/StJump）随 PDU 返回；蓝图 §4.4 要求检测跳变但 §4.2 `poll -> Option<GoosePdu>` 无承载 → `poll -> Option<(GoosePdu, RxStatus)>` | 蓝图自相矛盾（要求检测但接口无处上报）；接收方必须能区分新事件/重传/丢帧 |

## 接口契约

```rust
// lib.rs
pub enum GooseError {
    TransportError, BerEncodeError, BerDecodeError, InvalidConfig,
}  // Debug/Clone/PartialEq（D10）
pub trait L2Transport {                              // D4
    fn send(&mut self, frame: &[u8]) -> Result<(), GooseError>;
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, GooseError>;
}
pub struct MockL2 { /* 帧队列 + 发送记录 + 注入错误（测试/集成占位，D4） */ }

// dataset.rs
pub struct GooseDataset { pub entries: Vec<GooseEntry> }       // Debug/Clone/PartialEq
pub struct GooseEntry { pub path: String, pub value: DaValue } // Debug/Clone/PartialEq
impl GooseDataset {
    pub fn new() -> Self;
    pub fn set(&mut self, path: &str, value: DaValue);         // 有则覆盖无则追加
    pub fn get(&self, path: &str) -> Option<&GooseEntry>;
}

// goose_tx.rs
pub struct GooseControlBlock {
    pub go_cb_ref: String, pub app_id: u16, pub dst_addr: [u8; 6],
    pub min_time: u16, pub max_time: u16,
    pub st_num: u32, pub sq_num: u32,
    pub dataset_ref: String, pub needs_retransmit: bool,
}  // Debug/Clone/PartialEq
pub struct GoosePublisher<T: L2Transport> { cb, dataset, last_tx_time, retransmit_count, transport }
impl<T: L2Transport> GoosePublisher<T> {
    pub fn new(cb: GooseControlBlock, transport: T) -> Result<Self, GooseError>;  // app_id==0 → InvalidConfig
    pub fn update_value(&mut self, path: &str, value: DaValue);
    pub fn publish(&mut self, now: u64) -> Result<(), GooseError>;               // D6
    pub fn retransmit_if_needed(&mut self, now: u64) -> bool;
    pub fn cb(&self) -> &GooseControlBlock;
    pub fn dataset(&self) -> &GooseDataset;
    pub fn transport(&self) -> &T;                                               // 测试断言用
    pub fn transport_mut(&mut self) -> &mut T;
}

// goose_rx.rs
pub struct GoosePdu {
    pub st_num: u32, pub sq_num: u32,
    pub timestamp: u64, pub dataset: GooseDataset,
}  // Debug/Clone/PartialEq
pub enum RxStatus { New, Duplicate, StJump }                   // Debug/Clone/Copy/PartialEq（D12）
pub struct GooseSubscriber<T: L2Transport> {
    app_id, filter_mac, last_st_num, last_sq_num,
    callback: Option<alloc::boxed::Box<dyn Fn(&GoosePdu)>>,    // D9
    transport,
}
impl<T: L2Transport> GooseSubscriber<T> {
    pub fn new(app_id: u16, mac: [u8; 6], transport: T) -> Result<Self, GooseError>;
    pub fn set_callback<F: Fn(&GoosePdu) + 'static>(&mut self, f: F);
    pub fn poll(&mut self) -> Result<Option<(GoosePdu, RxStatus)>, GooseError>;  // D12
    pub fn last_st_num(&self) -> u32;
    pub fn transport_mut(&mut self) -> &mut T;
}
```

## 测试规划（iec61850-goose 36 个，src 内嵌）

| 文件 | 编号 | 数量 | 覆盖 |
|------|------|------|------|
| dataset.rs | DS1~DS6 | 6 | new 空数据集 / set 追加新路径 / set 覆盖已有路径（不新增）/ get 命中 / get miss → None / 多 DaValue 变体存储 + Clone/PartialEq |
| goose_tx.rs | TX7~TX18 | 12 | 以太网头 dst/src 组播 MAC + EtherType 0x88B8 / gocbRef 0x80 编码 / timeAllowedToLive 0x81 / t 0x84 8 字节 / stNum 0x85 / sqNum 0x86 / allData 0xAB 含长度（D7）/ 数据 tag 统一 0x80/0x85/0x87（D8）/ update_value 后 st+1 sq=0 needs_retransmit / publish 后 sq_num+1 / retransmit 前 3 次 min_time 其后 max_time / 整帧 TLV 可被 read_tag_length 逐层解析 |
| goose_rx.rs | RX19~RX30 | 12 | 有效帧解码 / APPID 不匹配丢弃 / dst MAC 不匹配丢弃 / st_num 跳变 → StJump（D12）/ 重复帧 → Duplicate / 非 0x88B8 → Ok(None) / 截断帧 → BerDecodeError / Bool 0x80 值解码 / Int32 0x85 解码 / Float32 4B + Float64 8B / 时间戳提取 / 未知 tag 跳过不报错 |
| goose_rx.rs | LB31~LB36 | 6 | publish→poll loopback 值一致 / 多条目保序 / set_callback 被调用 / mock 全链路 < 4ms（Instant，D11）/ 丢帧注入 → 下一帧 StJump / 事件后重传帧 sq_num 递增接收 |

## 配置与文档

- `configs/iec61850-goose.toml`：`[gocb]` go_cb_ref / app_id / dst_mac / min_time_ms = 2 / max_time_ms = 5000 / dataset_ref + 中文注释 ≥7 点（Raw socket L2 选型 §5.1 / EtherType 0x88B8 组播 / 重传 min×3→max 策略 §4.3 / L2Transport 抽象 D4 / 时间注入 D6 / 性能 <4ms 口径 D11 / 内存预算声明 / GPU 不适用 §6.6 / 安全待 v0.108.0 §7.3）
- `docs/protocols/iec61850-goose-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 重传时序图重绘 + GOOSE 帧结构图）+ D1~D12 偏差表 + 性能口径声明（D11）

## 版本同步

根 `Cargo.toml` version = "0.107.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` 注释；`gate.rs` 注释串尾 2 处追加 v0.107.0 类型清单（GooseControlBlock/GooseDataset/GooseEntry/GoosePublisher/GooseSubscriber/GoosePdu/RxStatus/GooseError/L2Transport/MockL2）。
