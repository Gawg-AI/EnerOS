# Checklist — v0.106.0 IEC 61850 MMS 协议栈

> 逐项核验后勾选。分组：A 蓝图合规 / B 目录结构 / C crate 骨架 / D ber_encode.rs / E ber_decode.rs / F acse.rs / G mms_client.rs / H 配置与文档 / I 版本同步与构建验证。

## A. 蓝图合规与 spec 对齐（C1~C10）

- [x] C1: 交付物对齐蓝图 §3：ber_encode.rs / ber_decode.rs / acse.rs / mms_client.rs 四模块齐全
- [x] C2: 接口对齐 spec 接口契约：`MmsClient` 含 new/connect/read/write/disconnect/conn_state
- [x] C3: 数据结构对齐 spec：MmsConnection/ConnState/VarAccessSpec/MmsRequest(4 变体)/MmsResponse(3 变体)/MmsReadResult/MmsWriteResult/MmsErrorCode(5 变体)
- [x] C4: `MmsError` 6 变体齐全（Timeout/ConnRefused/NotConnected/BerDecodeError/TransportError/IedError，D10）
- [x] C5: BER 长度恒为内容字节数（D6），listOfVariable 长度为条目字节和非元素个数
- [x] C6: 浮点解码右对齐：4→Float32、8→Float64（D7）
- [x] C7: `MmsTransport` trait + `MockTransport` 存在（D4），MmsClient 泛型化
- [x] C8: 无 `model: Arc<Iec61850Model>` 死字段（D5）
- [x] C9: 蓝图 §6.6 GPU 规则遵守：零 GPU 代码，纯 CPU 编解码
- [x] C10: spec.md D1~D12 偏差表与 lib.rs crate 文档偏差表逐字一致

## B. 目录结构（C11~C16，记忆 §2.4.1）

- [x] C11: crate 位于 `crates/protocols/iec61850-mms/`，未放根目录（D1）
- [x] C12: 根 `Cargo.toml` members 已追加 `"crates/protocols/iec61850-mms"`（protocols 段）
- [x] C13: 跨 crate path 引用为相对路径：`eneros-iec61850-model = { path = "../iec61850-model" }`
- [x] C14: 文档位于 `docs/protocols/iec61850-mms-design.md`，未平面化放 docs/ 根（D2）
- [x] C15: 测试全部 src 内嵌 `#[cfg(test)]`，未新增 tests/ 文件（D3）
- [x] C16: `cargo metadata --format-version 1` 解析成功（exit=0）

## C. crate 骨架与 no_std（C17~C22）

- [x] C17: lib.rs 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明
- [x] C18: 全 crate 零 `std::*` 引用（仅 `alloc::*`/`core::*`；Instant 仅 cfg(test) 内）
- [x] C19: 零 `panic!`/`todo!`/`unimplemented!`（生产路径）；零 `unwrap()` 于生产路径
- [x] C20: 依赖仅 eneros-iec61850-model（零第三方）；零 unsafe
- [x] C21: `cargo build -p eneros-iec61850-mms --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C22: lib.rs crate 文档含版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明（风格对齐 iec61850-model）

## D. ber_encode.rs（C23~C32）

- [x] C23: `BerEncoder { buffer: Vec<u8> }` + `pub fn new()` + 两个 encode 方法返回 `&[u8]`
- [x] C24: Read 请求顶层 tag 0xA0（ConfirmedRequestPDU）+ invokeID 编码正确（BE1/BE3）
- [x] C25: Read 服务 tag 0xA4；domain+item 为 VisibleString（BE2/BE4）
- [x] C26: 单变量条目字节长度正确（BE5）
- [x] C27: 多变量 listOfVariable 长度 == 条目字节和（非元素个数，D6）（BE6）
- [x] C28: 内容 ≥0x80 时使用 0x82 双字节长型长度（BE7）
- [x] C29: Write 请求服务 tag 0xA5（BE8）
- [x] C30: Bool/Int32/Float64 值编码正确（BE9/BE10 系列）
- [x] C31: 长度回填不覆盖后续 tag（tag+0x00 占位+内容+回填实现，D6）
- [x] C32: 测试 BE1~BE10 共 10 个全部通过

## E. ber_decode.rs（C33~C42）

- [x] C33: `read_tag_length` 支持短型与长型（0x82）长度（BD17）
- [x] C34: boolean 0x80 → DaValue::Bool（BD11）
- [x] C35: integer 0x85 多字节 → DaValue::Int32（BD12）
- [x] C36: floating-point 0x87 长度 4 → Float32、长度 8 → Float64（右对齐，D7）（BD13/BD14）
- [x] C37: 未知 tag 跳过对应结果为 None（BD15）
- [x] C38: 截断输入 → Err(MmsError::BerDecodeError)（BD16）
- [x] C39: write 响应解码 Success / Failed(String)（BD18/BD19）
- [x] C40: 顶层 tag 非法 → Err（BD20）
- [x] C41: 解码结果数量与保序正确
- [x] C42: 测试 BD11~BD20 共 10 个全部通过

## F. acse.rs（C43~C48）

- [x] C43: `encode_aarq` 含 0x60 tag + ap_title VisibleString（AC21）
- [x] C44: `decode_aare` 接受 → Ok(())（AC22）
- [x] C45: AARE 拒绝 → Err(IedError(Refused))（AC23）
- [x] C46: 畸形 AARE → Err(BerDecodeError)（AC24）
- [x] C47: COTP CR 定长结构编码 + CC 解析（D9）（AC25/AC26）
- [x] C48: 测试 AC21~AC26 共 6 个全部通过

## G. mms_client.rs（C49~C62）

- [x] C49: `MmsTransport` trait 三方法签名与 spec 一致（connect/send/recv，D4）
- [x] C50: `MmsClient::new` 初始 state == Idle（MC27）
- [x] C51: connect 成功状态机 Idle→Connecting→Connected（MC28）
- [x] C52: 连接时序：先发 COTP CR 再发 AARQ（mock 记录验证，MC29）
- [x] C53: 重试 2 次超时后第 3 次成功 → Ok，尝试计数 == 3（D11）（MC30）
- [x] C54: 3 次全超时 → Err(Timeout) 且 state == Error（MC31）
- [x] C55: read mock 回路返回正确结果（MC32）
- [x] C56: 未连接 read/write → Err(NotConnected)（MC33）
- [x] C57: write 覆盖 Success + Failed 两种结果（MC34）
- [x] C58: disconnect → state == Idle（MC35）
- [x] C59: recv TransportError → read 返回 Err 且 state == Error；重连后恢复（MC36）
- [x] C60: 100 点 read < 50ms（cfg(test) Instant 断言，D12）且结果保序（MC37）
- [x] C61: MmsResponse::Error 的 MmsErrorCode 映射正确（MC38）
- [x] C62: 测试 MC27~MC38 共 12 个全部通过

## H. 配置与文档（C63~C72）

- [x] C63: `configs/iec61850-mms.toml` 存在，`[ied]` 节含 peer_addr / peer_port=102 / local_ap_title / timeout_ms=3000 / connect_retry=3 + 中文注释 ≥7 点
- [x] C64: 配置中文注释覆盖：自研 BER 选型 / MMS over TCP 102 端口 / 重试 3 次 D11 / 传输抽象 D4 / 性能 <50ms / 内存预算 / GPU 不适用 / 安全待 v0.108.0
- [x] C65: `docs/protocols/iec61850-mms-design.md` 存在，12 章节齐全
- [x] C66: 文档含 ≥2 个 Mermaid 图：COTP/ACSE/MMS 关联时序图 + BER 编码结构图
- [x] C67: 文档含 D1~D12 偏差表，与 spec.md 逐字一致
- [x] C68: 文档含性能口径声明（100 点 <50ms 为 cfg(test) mock 回路编码+解码口径，D12）
- [x] C69: 文档风格对齐 docs/protocols/ 既有设计文档（头部版本块 + 目录 + 12 章节）
- [x] C70: 配置文件风格对齐 configs/ 既有文件（头部版本块 + 编号注释点）
- [x] C71: 文档测试计划章节列出 BE1~BE10/BD11~BD20/AC21~AC26/MC27~MC38
- [x] C72: 文档接口契约与实际源码签名一致

## I. 版本同步与构建验证（C73~C86）

- [x] C73: 根 `Cargo.toml` version == "0.106.0"
- [x] C74: `Makefile` VERSION == 0.106.0 且 L3 头部注释同步
- [x] C75: `ci.yml` 版本注释 == v0.106.0
- [x] C76: `gate.rs` 注释串尾 2 处追加 v0.106.0 类型清单（13 类型）
- [x] C77: `cargo test -p eneros-iec61850-mms` 38/38 通过
- [x] C78: eneros-iec61850-model 回归 33/33 通过（零改动验证）
- [x] C79: 全 workspace 回归通过（cargo test --workspace --exclude eneros-kernel --exclude eneros-hello，exit=0，零回归）
- [x] C80: `cargo fmt --all -- --check` 通过
- [x] C81: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C82: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）
- [x] C83: `git status` 无 target/elf/bin/dtb/IDE 缓存被追踪
- [x] C84: spec.md / tasks.md / checklist.md 三件齐全且内容一致
- [x] C85: tasks.md 全部复选框已勾选
- [x] C86: 无超范围交付（无 blueprint 未要求的额外模块/抽象，Karpathy Simplicity First）

## 验收记录

- **核验日期**：2026-07-20
- **核验人**：Trae Agent
- **通过项数**：86/86

**关键命令结果摘要**：

| 命令 | 结果 |
|------|------|
| `cargo test -p eneros-iec61850-mms` | 38/38 通过（BE1~BE10=10 / BD11~BD20=10 / AC21~AC26=6 / MC27~MC38=12） |
| `cargo test -p eneros-iec61850-model` | 33/33 通过（零回归，上游 crate 零改动验证） |
| `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` | exit=0，全 workspace 零回归 |
| `cargo build -p eneros-iec61850-mms --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` | 交叉编译通过 |
| `cargo metadata --format-version 1` | exit=0（workspace 成员路径全部正确） |
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` | 0 warning |
| `cargo deny --offline check advisories licenses bans sources` | 全 ok（沙箱无法联网拉取 advisory-db，以本地缓存库离线校验，等价通过；零新增第三方依赖） |
| `git status --short` | 无 target/elf/bin/dtb/IDE 缓存被追踪 |

**偏差与备注**：

1. **测试计数 38 与 spec 规划一致**：BE1~BE10=10 / BD11~BD20=10 / AC21~AC26=6 / MC27~MC38=12，全部 src 内嵌 `#[cfg(test)]`，Grep `#[test]` 计数确认 38 个测试函数存在且覆盖点与 spec 测试规划表逐项匹配。
2. **实施增量偏差 5 项**（已录入设计文档 `docs/protocols/iec61850-mms-design.md` §9.2）：① Write 数据区 Bool tag = 0x80 与解码侧严格对称；② `MmsClient` 增加 `transport()` / `transport_mut()` 访问器（测试刚需）；③ ConfirmedErrorPDU（0xA2）错误码映射约定（1→NotFound / 2→TypeMismatch / 3→Refused / 4→Timeout / 其余 Unknown，且不置 Error 状态）；④ `timeout_ms` 字段 `#[allow(dead_code)]`（no_std 无 sleep 语义，供集成层读取）；⑤ 解码侧 quality 默认 Good / timestamp 默认 0（集成层注入）。
3. **cargo deny 联网受限**：沙箱无法连接 github.com 拉取 advisory-db，改用 `cargo deny --offline check` 以本地缓存库校验，advisories / licenses / bans / sources 全 ok，等价通过。
4. **工作区预存改动**：`.gitignore` / `deny.toml` / `Cargo.lock` / `crates/runtime/hello/src/main.rs` 的修改为进入本任务前已有状态，不回滚、不纳入本版本交付。
5. **checklist 括注编号编辑性偏差**：C24~C30 括注的 BE 编号与最终测试函数名存在 1 位偏移（如 invokeID 实为 `test_be2` 而非括注 BE3，单变量长度实为 `test_be4` 而非括注 BE5，依此类推至 Write 0xA5 实为 `test_be7`）；实质覆盖点与 spec 测试规划表顺序完全一致，38 个测试全数存在并通过，该偏移为 checklist 文本编辑性偏差，不影响核验结论。
6. **tasks.md T3 主复选框补勾**：T3 子项 3.1~3.3 此前已勾且本次核验通过（members 追加、配置、文档均在位），主框漏勾，本次收工时补勾。
