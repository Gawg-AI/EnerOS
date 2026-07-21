# Checklist — v0.108.0 IEC 61850 SV + IEC 62351 安全

> 逐项核验后勾选。分组：A 蓝图合规 / B 目录结构 / C iec61850-sv crate / D sv_buffer.rs / E sv_rx.rs / F iec62351 crate / G key_mgmt.rs / H secure_goose.rs+secure_sv.rs / I 配置与文档 / J 版本同步与构建验证。

## A. 蓝图合规与 spec 对齐（C1~C10）

- [x] C1: 交付物对齐蓝图 §3：sv_rx.rs / sv_buffer.rs / secure_goose.rs / secure_sv.rs / key_mgmt.rs 五模块齐全
- [x] C2: 接口对齐 spec 接口契约：`SvSubscriber` 含 new/receive/take_samples/set_callback/last_smp_cnt/transport_mut；`SecureGoose`/`SecureSv` 含 new/encrypt/decrypt；`KeyMgmt` 含 new/add_key/get_current_key/rotate_keys/get_key
- [x] C3: 数据结构对齐 spec：SvSample/SampleStatus/RingBuffer/SecureFrame/SessionKey/KeyMgmt 字段一致
- [x] C4: `SvError` 4 变体齐全（TransportError/BerDecodeError/InvalidConfig/BufferOverflow，D10）；`SecError` 5 变体齐全（KeyExpired/HmacMismatch/DecryptFailed/EncryptFailed/InvalidKeyId，D10）
- [x] C5: SV PDU BER 各字段 tag 正确：smpCnt 0x80 / timestamp 0x81 / channels 0x82 含长度（D7）
- [x] C6: SecureFrame 结构：key_id / iv[12] / ciphertext / tag[16] / hmac[32]（蓝图 §4.5）
- [x] C7: IV 构造：8 字节计数器 BE + 4 字节 key_id BE，计数器递增（蓝图 §4.5）
- [x] C8: `L2Transport` trait + `MockL2` 存在（D4），SvSubscriber 泛型化（D5）
- [x] C9: 蓝图 §4.5 `extern "C"` SM4/SM3 FFI + unsafe 全删除（D7），复用 eneros-crypto 纯 Rust 实现
- [x] C10: spec.md D1~D12 偏差表与两 crate lib.rs crate 文档偏差表逐字一致

## B. 目录结构（C11~C16，记忆 §2.4.1）

- [x] C11: iec61850-sv 位于 `crates/protocols/iec61850-sv/`，iec62351 位于 `crates/security/iec62351/`，未放根目录（D1）
- [x] C12: 根 `Cargo.toml` members 已追加 `"crates/protocols/iec61850-sv"` 与 `"crates/security/iec62351"`
- [x] C13: 跨 crate path 引用为相对路径：`eneros-iec61850-model = { path = "../iec61850-model" }`、`eneros-crypto = { path = "../crypto" }`
- [x] C14: 文档位于 `docs/protocols/iec61850-sv-design.md` 与 `docs/protocols/iec62351-design.md`，未平面化放 docs/ 根（D2）
- [x] C15: 测试全部 src 内嵌 `#[cfg(test)]`，未新增 tests/ 文件（D3）
- [x] C16: `cargo metadata --format-version 1` 解析成功（exit=0）

## C. iec61850-sv crate 骨架与 no_std（C17~C22）

- [x] C17: lib.rs 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明
- [x] C18: 全 crate 零 `std::*` 引用（仅 `alloc::*`/`core::*`；Instant 仅 cfg(test) 内）
- [x] C19: 零 `panic!`/`todo!`/`unimplemented!`（生产路径）；零 `unwrap()` 于生产路径
- [x] C20: 依赖仅 eneros-iec61850-model（零第三方）；零 unsafe；零 extern "C"
- [x] C21: `cargo build -p eneros-iec61850-sv --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C22: lib.rs crate 文档含版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明（风格对齐 iec61850-goose）

## D. sv_buffer.rs（C23~C28）

- [x] C23: `RingBuffer<T>` 字段私有（buf/head/tail/len），`new(capacity)` 创建
- [x] C24: `push` 未满追加，满则覆盖最旧（head 前移）
- [x] C25: `drain` 返回全部元素并清空缓冲
- [x] C26: `len()` / `is_empty()` 返回正确
- [x] C27: 溢出后保序（最新 N 个元素，N=capacity）
- [x] C28: 测试 RB1~RB6 共 6 个全部通过

## E. sv_rx.rs（C29~C40）

- [x] C29: `SvSample` 全 pub 字段（smp_cnt/timestamp/channels/status），derive Debug/Clone/PartialEq
- [x] C30: `SampleStatus` 3 变体（New/Duplicate/SmpJump），derive Debug/Clone/Copy/PartialEq（D12）
- [x] C31: `SvSubscriber<T: L2Transport>` 泛型（D5）
- [x] C32: 非 EtherType 0x88BA → Ok(false)（RX10）
- [x] C33: dst MAC 不匹配丢弃 → Ok(false)（RX9）
- [x] C34: APPID 不匹配丢弃 → Ok(false)（RX8）
- [x] C35: 有效帧解码 → Ok(true)，样本写入缓冲（RX7）
- [x] C36: smpCnt 跳变 >1 → SmpJump（D12）（RX11）
- [x] C37: smpCnt 相同 → Duplicate（RX12）
- [x] C38: 截断帧 → BerDecodeError（RX13）
- [x] C39: `take_samples` 返回全部并清空（RX18）
- [x] C40: 测试 RX7~RX18 共 12 个全部通过

## F. iec62351 crate 骨架与 no_std（C41~C46）

- [x] C41: lib.rs 顶部 `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`；子模块不重复 no_std 声明
- [x] C42: 全 crate 零 `std::*` 引用（仅 `alloc::*`/`core::*`；Instant 仅 cfg(test) 内）
- [x] C43: 零 `panic!`/`todo!`/`unimplemented!`（生产路径）；零 `unwrap()` 于生产路径
- [x] C44: 依赖仅 eneros-crypto（零第三方）；零 unsafe；零 extern "C"
- [x] C45: `cargo build -p eneros-iec62351 --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C46: lib.rs crate 文档含版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明

## G. key_mgmt.rs（C47~C54）

- [x] C47: `SessionKey` 全 pub 字段（key_id/key_data/mac_key/expiry），derive Clone/PartialEq（不派生 Debug 防泄露，D9）
- [x] C48: `KeyMgmt` 字段私有（local_keys/key_lifetime/next_key_id），`new(key_lifetime)` 创建
- [x] C49: `add_key` 存储密钥
- [x] C50: `get_current_key(now)` 命中未过期返回 Some，全过期 → KeyExpired（KM3/KM4）
- [x] C51: `rotate_keys(now, new_key_data, new_mac_key)` 当前过期时生成新密钥（key_id+1、expiry=now+lifetime），未过期不轮换（D9）（KM5/KM6）
- [x] C52: `get_key(key_id)` 命中返回，miss → InvalidKeyId（KM7/KM8）
- [x] C53: `local_keys` 为 `Vec`（非 HashMap，D6）
- [x] C54: 测试 KM1~KM8 共 8 个全部通过

## H. secure_goose.rs + secure_sv.rs（C55~C66）

- [x] C55: `SecureFrame` 全 pub 字段（key_id/iv/ciphertext/tag/hmac），derive Debug/Clone/PartialEq
- [x] C56: `SecureGoose`/`SecureSv` 同构（D8），内部委托私有 `SecureChannel`
- [x] C57: `new(session)` 初始化 Sm4Gcm + Sm3Hmac + IV 计数器=0
- [x] C58: `encrypt` 生成 IV（计数器+key_id）→ SM4-GCM 加密 → HMAC（iv+ct+tag）→ SecureFrame（SG9/SG11）
- [x] C59: `decrypt` 先 HMAC 常量时间校验（防时序攻击）→ SM4-GCM 解密（SG12）
- [x] C60: 篡改 ciphertext → HmacMismatch（SG13）
- [x] C61: 篡改 tag → HmacMismatch（先校验 HMAC，SG14）
- [x] C62: 篡改 hmac → HmacMismatch（SG15）
- [x] C63: IV 计数器递增（同一实例连续加密 IV 不同，SG11）
- [x] C64: 加密延迟 < 0.5ms（cfg(test) Instant 断言，D11）（SG19）
- [x] C65: 测试 SG9~SG19 共 11 个全部通过
- [x] C66: 测试 SS20~SS22 共 3 个全部通过

## I. 配置与文档（C67~C76）

- [x] C67: `configs/iec61850-sv.toml` 存在，`[sv]` 节含 app_id / dst_mac / buf_size = 16 + 中文注释 ≥7 点
- [x] C68: 配置中文注释覆盖：EtherType 0x88BA / L2Transport 抽象 D4 / 环形缓冲溢出策略 §4.4 / 性能 <4ms 口径 D11 / 内存预算 / GPU 不适用 / 安全加密由 iec62351 提供
- [x] C69: `configs/iec62351.toml` 存在，`[security]` 节含 key_lifetime_ms = 3600000 / initial_key_id = 1 + 中文注释 ≥7 点
- [x] C70: 配置中文注释覆盖：SM4-GCM 选型 §5.1 / SM3-HMAC 认证 / 密钥轮换策略 §4.4 / IV 构造规则 §4.5 / 性能 <0.5ms 口径 D11 / 内存预算 / GPU 不适用
- [x] C71: `docs/protocols/iec61850-sv-design.md` 存在，12 章节齐全
- [x] C72: 文档含 ≥2 个 Mermaid 图：SV 帧结构图 + smpCnt 状态机图
- [x] C73: `docs/protocols/iec62351-design.md` 存在，12 章节齐全
- [x] C74: 文档含 ≥2 个 Mermaid 图：安全校验流程图 + SecureFrame 结构图
- [x] C75: 两文档含 D1~D12 偏差表，与 spec.md 逐字一致
- [x] C76: 两文档含性能口径声明（D11）

## J. 版本同步与构建验证（C77~C90）

- [x] C77: 根 `Cargo.toml` version == "0.108.0"
- [x] C78: `Makefile` VERSION == 0.108.0 且 L3 头部注释同步
- [x] C79: `ci.yml` 版本注释 == v0.108.0
- [x] C80: `gate.rs` 注释串尾 2 处追加 v0.108.0 类型清单（11 类型）
- [x] C81: `cargo test -p eneros-iec61850-sv` 18/18 通过
- [x] C82: `cargo test -p eneros-iec62351` 22/22 通过
- [x] C83: eneros-iec61850-goose 回归 36/36 通过（零改动验证）
- [x] C84: eneros-crypto 回归通过（零改动验证）
- [x] C85: 全 workspace 回归通过（cargo test --workspace --exclude eneros-kernel --exclude eneros-hello，exit=0，零回归）
- [x] C86: `cargo fmt --all -- --check` 通过
- [x] C87: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C88: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）
- [x] C89: `git status` 无 target/elf/bin/dtb/IDE 缓存被追踪
- [x] C90: spec.md / tasks.md / checklist.md 三件齐全且内容一致；tasks.md 全部复选框已勾选；无超范围交付（Karpathy Simplicity First）

## 验收记录

- **核验日期**：2026-07-20
- **核验人**：Trae Agent
- **通过项数**：90/90

**关键命令结果摘要**：

| 命令 | 结果 |
|------|------|
| `cargo metadata --format-version 1` | exit=0（C6/C16） |
| `cargo test -p eneros-iec61850-sv sv_buffer` | 6/6 通过（RB1~RB6，C28） |
| `cargo test -p eneros-iec61850-sv sv_rx` | 12/12 通过（RX7~RX18，C40） |
| `cargo test -p eneros-iec61850-sv` | 25/25 通过（RB6 + RX12 + MockL2 自测 ×7，C81，见备注①） |
| `cargo test -p eneros-iec62351 key_mgmt` | 8/8 通过（KM1~KM8，C54） |
| `cargo test -p eneros-iec62351 secure` | 14/14 通过（SG9~SG19 ×11 + SS20~SS22 ×3，C65/C66） |
| `cargo test -p eneros-iec62351` | 22/22 通过（C82） |
| `cargo test -p eneros-iec61850-goose` | 36/36 通过（零改动回归，C83） |
| `cargo test -p eneros-crypto` | 417/417 通过（358 + pki 11 + sm2 15 + sm3 10 + sm4 10 + 13，零改动回归，C84） |
| `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` | exit=0 全绿（C85） |
| `cargo build -p eneros-iec61850-sv --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` | 通过（C8/C21） |
| `cargo build -p eneros-iec62351 --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` | 通过（C8/C45） |
| `cargo fmt --all -- --check` | exit=0（C9/C86） |
| `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` | 0 warning，exit=0（C10/C87） |
| `cargo deny check advisories licenses bans sources` | advisories/bans/licenses/sources 全 ok（C11/C88，见备注②） |
| `git status --porcelain` | 无 target/elf/bin/dtb/IDE 缓存被追踪（C89） |
| D1~D12 偏差表自动化比对（spec.md vs 两 lib.rs vs 两设计文档） | 4 方 × 12 行 Compare-Object 零差异，逐字一致（C10/C75） |

**偏差与备注**：

- D1~D12 全部落实并在两 crate lib.rs crate 文档 / 两设计文档 §9.1 与 spec.md 逐字一致（自动化比对零差异）。
- 实施增量偏差 6 项（两设计文档 §9.2 集中记录，已逐项对照实际代码核验一致）：SV 侧 3 项——① `RingBuffer` 内部字段简化为 `{ buf: Vec<T>, head, capacity }`（spec 接口的 `tail`/`len` 由 `head` + `buf.len()` 隐式替代，语义等价：满时覆盖最旧、drain 旧→新保序；C3/C23 按此核验）；② `SvSubscriber` 增加内部 `has_last: bool` 基线标志（区分「未收采样」与「last_smp_cnt == 0」，首采样一律 New 不误判 SmpJump）；③ lib.rs 补充 7 个 MockL2 自测，错误注入方法命名 `inject_send_error_once`/`inject_recv_error_once`（一次性语义显式化）。iec62351 侧 3 项——① `SessionKey` 整体不派生 Debug（spec 注释自相矛盾，按 D9 防泄露精神取更严格解，仅 Clone/PartialEq；C47 按此核验）；② `SecError::EncryptFailed` 当前不产生（`Sm4Gcm::encrypt` 为无错接口，变体保留对齐 D10 错误模型）；③ IV 计数器先自增后取值（首帧计数器段为 1，SG11 断言 c2 == c1 + 1）+ `add_key` 存入更大 key_id 时同步推进 `next_key_id`。
- 备注①：C81 文本写「18/18」，实际 `cargo test -p eneros-iec61850-sv` 为 25/25（spec 测试矩阵 RB1~RB6 + RX7~RX18 共 18 个，另含设计文档 §9.2③ 记录的 MockL2 自测 7 个），全集通过视为满足。
- 备注②：本次核验时在线 `cargo deny` 因无法连接 github.com（RustSec advisory-db fetch 超时）失败；改用 `cargo deny --offline`（本地已缓存 v0.108.0 开发期 fetch 的 advisory-db）复核：advisories/bans/licenses/sources 全 ok，exit=0。属环境网络限制，非代码问题；零新增第三方依赖（两 crate 仅 path 引用 eneros-iec61850-model / eneros-crypto）。
- 性能口径：加密延迟 < 0.5ms 为 SG19 cfg(test) Instant 断言（256B 载荷 × 100 次加密+解密 < 50ms，D11）；SV 接收 < 4ms 为 cfg(test) MockL2 回路口径；真实网卡端到端时延均为实验室硬件项。
- 密钥安全：`SessionKey` 不派生 Debug（D9），密钥材料外部注入（硬件 TRNG/密钥管理系统），crate 内零 unsafe、零 C FFI、零密钥硬编码。
- 下一阶段：v0.109.0 故障录波 COMTRADE（消费本版安全采样数据源）。
