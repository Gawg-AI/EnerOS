# Checklist — v0.107.0 IEC 61850 GOOSE 快速事件传输

> 逐项核验后勾选。分组：A 蓝图合规 / B 目录结构 / C crate 骨架 / D dataset.rs / E goose_tx.rs / F goose_rx.rs / G 配置与文档 / H 版本同步与构建验证。

## A. 蓝图合规与 spec 对齐（C1~C10）

- [x] C1: 交付物对齐蓝图 §3：dataset.rs / goose_tx.rs / goose_rx.rs 三模块齐全（D4 删除 ffi/unsafe 后合并为 lib.rs 的 trait+Mock）
- [x] C2: 接口对齐 spec 接口契约：`GoosePublisher` 含 new/update_value/publish/retransmit_if_needed/cb/dataset/transport/transport_mut
- [x] C3: 数据结构对齐 spec：GooseControlBlock/GooseDataset/GooseEntry/GoosePdu/GooseSubscriber/RxStatus(3 变体) 字段一致
- [x] C4: `GooseError` 4 变体齐全（TransportError/BerEncodeError/BerDecodeError/InvalidConfig，D10）
- [x] C5: allData 0xAB 含长度字段（D7）；数据 tag 统一 boolean 0x80 / integer 0x85 / floating-point 0x87（D8）
- [x] C6: 重传时序：前 3 次 min_time 间隔、其后 max_time 周期心跳
- [x] C7: `L2Transport` trait + `MockL2` 存在（D4），Publisher/Subscriber 泛型化（D5）
- [x] C8: 蓝图 §4.5 `extern "C"` raw socket FFI + unsafe 全删除（D4）
- [x] C9: 蓝图 §6.6 GPU 规则遵守：零 GPU 代码，纯 CPU 编解码
- [x] C10: spec.md D1~D12 偏差表与 lib.rs crate 文档偏差表逐字一致

## B. 目录结构（C11~C16，记忆 §2.4.1）

- [x] C11: crate 位于 `crates/protocols/iec61850-goose/`，未放根目录（D1）
- [x] C12: 根 `Cargo.toml` members 已追加 `"crates/protocols/iec61850-goose"`（protocols 段）
- [x] C13: 跨 crate path 引用为相对路径：`eneros-iec61850-model = { path = "../iec61850-model" }`
- [x] C14: 文档位于 `docs/protocols/iec61850-goose-design.md`，未平面化放 docs/ 根（D2）
- [x] C15: 测试全部 src 内嵌 `#[cfg(test)]`，未新增 tests/ 文件（D3）
- [x] C16: `cargo metadata --format-version 1` 解析成功（exit=0）

## C. crate 骨架与 no_std（C17~C22）

- [x] C17: lib.rs 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明
- [x] C18: 全 crate 零 `std::*` 引用（仅 `alloc::*`/`core::*`；Instant 仅 cfg(test) 内）
- [x] C19: 零 `panic!`/`todo!`/`unimplemented!`（生产路径）；零 `unwrap()` 于生产路径
- [x] C20: 依赖仅 eneros-iec61850-model（零第三方）；零 unsafe；零 extern "C"
- [x] C21: `cargo build -p eneros-iec61850-goose --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C22: lib.rs crate 文档含版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明（风格对齐 iec61850-mms）

## D. dataset.rs（C23~C28）

- [x] C23: `GooseDataset` 全 pub 字段（entries: Vec），derive Debug/Clone/PartialEq
- [x] C24: `GooseEntry` 全 pub 字段（path/value），derive Debug/Clone/PartialEq
- [x] C25: `new()` 创建空数据集
- [x] C26: `set` 新 path → 追加；已有 path → 覆盖（不新增条目）
- [x] C27: `get` 命中返回 Some，miss 返回 None
- [x] C28: 测试 DS1~DS6 共 6 个全部通过

## E. goose_tx.rs（C29~C42）

- [x] C29: `GooseControlBlock` 全部 pub 字段，derive Debug/Clone/PartialEq
- [x] C30: `GoosePublisher<T: L2Transport>` 泛型（D5）
- [x] C31: `new` app_id==0 → InvalidConfig；否则 state 初始化
- [x] C32: 以太网帧头：dst MAC + src 组播 MAC 01:0C:CD:01:00:00 + EtherType 0x88B8（TX7）
- [x] C33: GOOSE PDU BER 各字段 tag 正确：0x80 gocbRef / 0x81 timeAllowedToLive / 0x82 datSet / 0x83 goID / 0x84 t / 0x85 stNum / 0x86 sqNum / 0x87 simulation / 0x88 confRef / 0x89 ndsCom / 0x8A numDatSetEntries
- [x] C34: allData 0xAB 携带内容字节长度（D7）（TX13）
- [x] C35: 数据 tag 统一：Bool 0x80 / Int32 0x85 / Float 0x87（4B→Float32、8B→Float64）（D8）（TX14）
- [x] C36: `update_value` 后 st_num+1、sq_num=0、needs_retransmit=true（TX15）
- [x] C37: `publish(now)` 后 sq_num+1、last_tx_time=now（TX16）
- [x] C38: `retransmit_if_needed` 前 3 次 min_time、其后 max_time；返回 bool（TX17）
- [x] C39: 整帧 TLV 可被 v0.106.0 `read_tag_length` 逐层解析（TX18）
- [x] C40: 测试 TX7~TX18 共 12 个全部通过

## F. goose_rx.rs（C41~C56）

- [x] C41: `GoosePdu` 全 pub 字段，derive Debug/Clone/PartialEq
- [x] C42: `RxStatus` 3 变体（New/Duplicate/StJump），derive Debug/Clone/Copy/PartialEq（D12）
- [x] C43: `GooseSubscriber<T: L2Transport>` 泛型（D5）
- [x] C44: 非 EtherType 0x88B8 → Ok(None)（RX23）
- [x] C45: dst MAC 不匹配丢弃 → Ok(None)（RX21）
- [x] C46: APPID 不匹配丢弃 → Ok(None)（RX20）
- [x] C47: 有效帧解码 → Some((pdu, New))（RX19）
- [x] C48: st_num 跳变 >1 → StJump（D12）（RX19）
- [x] C49: st_num 相同 sq_num 递增 → Duplicate（RX22）
- [x] C50: 截断帧 → BerDecodeError（RX24）
- [x] C51: Bool 0x80 / Int32 0x85 / Float32 4B / Float64 8B 值解码正确（RX25~RX27）
- [x] C52: 时间戳提取正确（RX28）
- [x] C53: 未知 tag 跳过不报错（RX29）
- [x] C54: `set_callback` 在收到有效帧时被调用（LB33）
- [x] C55: 端到端 loopback < 4ms（cfg(test) Instant，D11）且值保序（LB35）
- [x] C56: 测试 RX19~RX30 + LB31~LB36 共 18 个全部通过

## G. 配置与文档（C57~C66）

- [x] C57: `configs/iec61850-goose.toml` 存在，`[gocb]` 节含 go_cb_ref / app_id / dst_mac / min_time_ms=2 / max_time_ms=5000 / dataset_ref + 中文注释 ≥7 点
- [x] C58: 配置中文注释覆盖：L2 选型 §5.1 / EtherType 0x88B8 组播 / 重传策略 §4.3 / L2Transport 抽象 D4 / 时间注入 D6 / 性能 <4ms D11 / 内存预算 / GPU 不适用 / 安全待 v0.108.0
- [x] C59: `docs/protocols/iec61850-goose-design.md` 存在，12 章节齐全
- [x] C60: 文档含 ≥2 个 Mermaid 图：重传时序图 + GOOSE 帧结构图
- [x] C61: 文档含 D1~D12 偏差表，与 spec.md 逐字一致
- [x] C62: 文档含性能口径声明（<4ms 为 cfg(test) mock 回路编码+传输+解码口径，D11）
- [x] C63: 文档风格对齐 docs/protocols/ 既有设计文档（头部版本块 + 目录 + 12 章节）
- [x] C64: 配置文件风格对齐 configs/ 既有文件（头部版本块 + 编号注释点）
- [x] C65: 文档测试计划章节列出 DS1~DS6/TX7~TX18/RX19~RX30/LB31~LB36
- [x] C66: 文档接口契约与实际源码签名一致

## H. 版本同步与构建验证（C67~C80）

- [x] C67: 根 `Cargo.toml` version == "0.107.0"
- [x] C68: `Makefile` VERSION == 0.107.0 且 L3 头部注释同步
- [x] C69: `ci.yml` 版本注释 == v0.107.0
- [x] C70: `gate.rs` 注释串尾 2 处追加 v0.107.0 类型清单（10 类型）
- [x] C71: `cargo test -p eneros-iec61850-goose` 36/36 通过
- [x] C72: eneros-iec61850-model 回归 33/33 通过（零改动验证）
- [x] C73: 全 workspace 回归通过（cargo test --workspace --exclude eneros-kernel --exclude eneros-hello，exit=0，零回归）
- [x] C74: `cargo fmt --all -- --check` 通过
- [x] C75: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C76: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）
- [x] C77: `git status` 无 target/elf/bin/dtb/IDE 缓存被追踪
- [x] C78: spec.md / tasks.md / checklist.md 三件齐全且内容一致
- [x] C79: tasks.md 全部复选框已勾选
- [x] C80: 无超范围交付（无 blueprint 未要求的额外模块/抽象，Karpathy Simplicity First）

## 验收记录

- **核验日期**：2026-07-20
- **核验人**：Trae Agent
- **通过项数**：80/80

**关键命令结果摘要**：

| 命令 | 结果 |
|------|------|
| `cargo metadata --format-version 1` | exit=0（C6/C16） |
| `cargo test -p eneros-iec61850-goose` | 36/36 通过（DS1~DS6 + TX7~TX18 + RX19~RX30 + LB31~LB36，C71） |
| `cargo test -p eneros-iec61850-model` | 33/33 通过（零改动回归，C72） |
| `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` | exit=0 全绿（C73） |
| `cargo build -p eneros-iec61850-goose --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` | 通过（C8/C21） |
| `cargo fmt --all -- --check` | exit=0（C9/C74） |
| `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` | 0 warning（C10/C75） |
| `cargo deny check advisories licenses bans sources` | advisories/bans/licenses/sources 全 ok（C11/C76） |
| `git status --porcelain` | 无 target/elf/bin/dtb/IDE 缓存被追踪（C77） |

**偏差与备注**：

- D1~D12 全部落实并在 lib.rs crate 文档 / 设计文档 §9.1 与 spec.md 逐字一致。
- 实施增量偏差 3 项（设计文档 §9.2 集中记录）：① APPID 2 字节字段补入（蓝图 §4.5 组帧省略但 §4.4 要求过滤，自相矛盾）；② rx 侧 `GooseEntry.path` 置空字符串（allData 仅值保序无路径语义）；③ `MockL2` 增加 loopback 开关（全链路回路测试刚需）。
- 性能 < 4ms 为 cfg(test) MockL2 回路编码+传输+解码全链路口径（D11），真实网卡端到端为实验室硬件项。
- 下一阶段：v0.108.0 SV + IEC 62351 安全加固。
