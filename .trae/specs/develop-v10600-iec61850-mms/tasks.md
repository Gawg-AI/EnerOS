# Tasks — v0.106.0 IEC 61850 MMS 协议栈

> Spec：`spec.md`（develop-v10600-iec61850-mms）。T1→T2 顺序（T2 消费 T1 编码器）；T3 依赖 T2；T4/T5 顺序收尾。

- [x] **T1：新建 crate 骨架 + ber_encode.rs / ber_decode.rs — BER 编解码**
  - [x] 1.1 `crates/protocols/iec61850-mms/Cargo.toml`：`eneros-iec61850-mms`，workspace 继承，依赖仅 `eneros-iec61850-model = { path = "../iec61850-model" }`
  - [x] 1.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明（ber_encode/ber_decode/acse/mms_client）+ 重导出 + `MmsError`（Timeout/ConnRefused/NotConnected/BerDecodeError/TransportError/IedError(MmsErrorCode)，D10）+ crate 文档（版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明，风格对齐 iec61850-model）
  - [x] 1.3 `src/ber_encode.rs`：`BerEncoder { buffer: Vec<u8> }` + `new()` + `encode_read_request(invoke_id, vars)` / `encode_write_request(invoke_id, vars)`；「tag + 0x00 长度占位 + 内容 + 回填」构造；长度恒为内容字节数（短型 <0x80 单字节，否则 0x82 双字节长型，D6）
  - [x] 1.4 `src/ber_decode.rs`：`read_tag_length(data, pos) -> (u8, usize)`（长短两型）+ `decode_read_response`（boolean 0x80 / integer 0x85 / floating-point 0x87；浮点按 val_len 右对齐，4→Float32、8→Float64，D7；未知 tag 跳过得 None）+ `decode_write_response`（Success/Failed）+ 截断 → BerDecodeError
  - [x] 1.5 测试 BE1~BE10 + BD11~BD20（20 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-iec61850-mms ber_encode:: ber_decode::` 20/20 全过 ✅

- [x] **T2：acse.rs + mms_client.rs — ACSE 关联 / COTP / MMS 客户端**
  - [x] 2.1 `src/acse.rs`：`encode_aarq(ap_title)`（AARQ 0x60 + AP-title VisibleString）/ `decode_aare`（接受 → Ok；拒绝 → IedError(Refused)；畸形 → BerDecodeError）+ COTP CR 编码 / CC 解析（定长简化结构，D9）
  - [x] 2.2 `src/mms_client.rs`：`MmsTransport` trait（connect/send/recv，D4）+ `MmsConnection`/`ConnState`（Idle/Connecting/Connected/Error）+ `MmsRequest`（4 变体全量，D5）/`MmsResponse`/`VarAccessSpec`/`MmsReadResult`/`MmsWriteResult`/`MmsErrorCode` + `MmsClient<T: MmsTransport>`（new/connect 重试 ≤3 次 D11/read/write 保序/disconnect/conn_state；未连接 read/write → NotConnected；recv 错误 → state = Error）+ `MockTransport`（脚本化响应 + 尝试计数，同文件，D4）
  - [x] 2.3 测试 AC21~AC26 + MC27~MC38（18 个，见 spec 测试规划表；100 点 read < 50ms 用 `std::time::Instant` 仅 cfg(test)，D12）
  - 验证：`cargo test -p eneros-iec61850-mms` 38/38 全过 ✅

- [x] **T3：workspace 接线 + 配置 + 设计文档**
  - [x] 3.1 根 `Cargo.toml` members 追加 `"crates/protocols/iec61850-mms"`（protocols 段 iec61850-model 之后）
  - [x] 3.2 `configs/iec61850-mms.toml`：`[ied]` peer_addr / peer_port = 102 / local_ap_title / timeout_ms = 3000 / connect_retry = 3 + 中文注释 ≥7 点（自研 BER 选型 §5.1 / MMS over TCP 102 端口 / 重试 3 次 D11 / 传输抽象 D4 / 性能 100 点 <50ms / 内存预算 / GPU 不适用 §6.6 / 安全待 v0.108.0 §7.3）
  - [x] 3.3 `docs/protocols/iec61850-mms-design.md`：12 章节 + ≥2 Mermaid（COTP/ACSE/MMS 关联时序图 + BER 编码结构图）+ D1~D12 偏差表（与 spec.md 逐字一致）+ 性能口径声明（D12）
  - 验证：`cargo metadata` 解析成功；`cargo test -p eneros-iec61850-mms` 38 全过

- [x] **T4：版本同步 0.106.0 + 全量构建验证**
  - [x] 4.1 根 `Cargo.toml` version = "0.106.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` 注释；`gate.rs` 注释串尾 2 处追加 v0.106.0 类型清单（13 类型：MmsClient/MmsConnection/ConnState/MmsRequest/MmsResponse/VarAccessSpec/MmsReadResult/MmsWriteResult/MmsErrorCode/MmsError/MmsTransport/MockTransport/BerEncoder）
  - [x] 4.2 §2.4.2 构建校验：C6 metadata / C7 本 crate 38 + 全 workspace 回归 / C8 aarch64 交叉编译（`cargo build -p eneros-iec61850-mms --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）/ C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - 验证：C6~C11 全绿 ✅（C11 因沙箱网络无法连接 github.com 拉取 advisory-db，改用 `cargo deny --offline check` 以本地缓存库校验：advisories/bans/licenses/sources 全 ok）

- [x] **T5：checklist 逐项核验收工**
  - [x] 5.1 `checklist.md` 逐项核验勾选 + 验收记录
  - 验证：checklist 全勾（86/86），收工

# Task Dependencies

- T1 先行（T2 消费 ber_encode/ber_decode）
- T2 depends on T1
- T3 depends on T2（文档需最终代码签名）
- T4 depends on T3
- T5 depends on T4
