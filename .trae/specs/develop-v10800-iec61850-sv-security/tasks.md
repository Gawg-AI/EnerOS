# Tasks — v0.108.0 IEC 61850 SV + IEC 62351 安全

> Spec：`spec.md`（develop-v10800-iec61850-sv-security）。T1→T2 顺序（T2 消费 T1 环形缓冲类型）；T3 独立（密钥管理）；T4 依赖 T3；T5/T6 顺序收尾。

- [x] **T1：新建 iec61850-sv crate 骨架 + sv_buffer.rs + lib.rs — 环形缓冲与 L2Transport**
  - [x] 1.1 `crates/protocols/iec61850-sv/Cargo.toml`：`eneros-iec61850-sv`，workspace 继承，依赖仅 `eneros-iec61850-model = { path = "../iec61850-model" }`
  - [x] 1.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明（sv_rx/sv_buffer）+ 重导出 + `SvError`（4 变体：TransportError/BerDecodeError/InvalidConfig/BufferOverflow，D10，derive Debug/Clone/PartialEq）+ `L2Transport` trait（send/recv，D4，复用 GOOSE 版本）+ `MockL2`（帧队列 + 发送记录 + 注入错误 + loopback，D4，复用 GOOSE 版本）+ crate 文档（版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明，风格对齐 iec61850-goose）
  - [x] 1.3 `src/sv_buffer.rs`：`RingBuffer<T> { buf: Vec<T>, head, tail, len }`（D6）；`new(capacity)` / `push(item)`（满则覆盖最旧）/ `drain()` → Vec<T>（返回全部并清空）/ `len()` / `is_empty()`
  - [x] 1.4 测试 RB1~RB6（6 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-iec61850-sv sv_buffer::` 6/6 全过 ✅（连同 MockL2 7 个共 13/13）

- [x] **T2：sv_rx.rs — SV 订阅者 + BER 解码**
  - [x] 2.1 `src/sv_rx.rs`：`SvSample { smp_cnt, timestamp, channels: Vec<f32>, status }`（derive Debug/Clone/PartialEq）+ `SampleStatus`（New/Duplicate/SmpJump，D12，derive Debug/Clone/Copy/PartialEq）+ `SvSubscriber<T: L2Transport>`（D5）：`new(app_id, mac, buf_size, transport)`（app_id==0 → InvalidConfig）/ `receive(frame)`（EtherType 0x88BA 过滤 → dst MAC 过滤 → APPID 过滤 → SV PDU BER 解码：smpCnt 0x80 / timestamp 0x81 8 字节 / channels 0x82 含长度，f32 大端 4 字节/个；smpCnt 跳变 >1 → SmpJump、重复 → Duplicate、新 → New；有效样本写入缓冲返回 Ok(true)，过滤返回 Ok(false)）/ `take_samples()` / `set_callback<F: Fn(&SvSample) + 'static>`（去 Send+Sync bound，D9）/ `last_smp_cnt()` / `transport_mut()` 访问器
  - [x] 2.2 测试 RX7~RX18（12 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-iec61850-sv` 25/25 全过 ✅（RB6+MockL2 7+RX12）

- [x] **T3：新建 iec62351 crate 骨架 + key_mgmt.rs + lib.rs — 密钥管理**
  - [x] 3.1 `crates/security/iec62351/Cargo.toml`：`eneros-iec62351`，workspace 继承，依赖仅 `eneros-crypto = { path = "../crypto" }`
  - [x] 3.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明（secure_goose/secure_sv/key_mgmt）+ 重导出 + `SecError`（5 变体：KeyExpired/HmacMismatch/DecryptFailed/EncryptFailed/InvalidKeyId，D10，derive Debug/Clone/PartialEq）+ crate 文档（版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明）
  - [x] 3.3 `src/key_mgmt.rs`：`SessionKey { key_id, key_data: [u8;16], mac_key: [u8;32], expiry }`（derive Clone/PartialEq，不派生 Debug 防泄露，D9）+ `KeyMgmt { local_keys: Vec<SessionKey>, key_lifetime, next_key_id }`（D6）：`new(key_lifetime)` / `add_key(session)` / `get_current_key(now)` → 最近添加且 expiry > now，无则 KeyExpired / `rotate_keys(now, new_key_data, new_mac_key)`（D9，当前过期则生成新密钥 key_id+1、expiry=now+lifetime）/ `get_key(key_id)` → 命中返回，miss → InvalidKeyId
  - [x] 3.4 测试 KM1~KM8（8 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-iec62351 key_mgmt::` 8/8 全过 ✅

- [x] **T4：secure_goose.rs + secure_sv.rs — 加密封装/解封**
  - [x] 4.1 `src/secure_goose.rs`：`SecureFrame { key_id, iv: [u8;12], ciphertext: Vec<u8>, tag: [u8;16], hmac: [u8;32] }`（derive Debug/Clone/PartialEq）+ `SecureGoose`（内部委托私有 `SecureChannel`，D8）：`new(session)`（初始化 Sm4Gcm + Sm3Hmac + IV 计数器=0）/ `encrypt(plaintext)`（IV = 计数器8字节BE + key_id 4字节BE，计数器+1；Sm4Gcm.encrypt(iv, plaintext, aad=&[]) → (ct, tag)；HMAC(iv + ct + tag) → mac；返回 SecureFrame）/ `decrypt(frame)`（先 HMAC 常量时间校验 → Sm4Gcm.decrypt(iv, ct, &[], tag) → 明文；HMAC 失败 → HmacMismatch；tag 失败 → DecryptFailed）
  - [x] 4.2 `src/secure_sv.rs`：`SecureSv` 同构于 SecureGoose（D8，内部委托同一 `SecureChannel`）
  - [x] 4.3 测试 SG9~SG19 + SS20~SS22（14 个，见 spec 测试规划表；SG19 加密延迟 < 0.5ms 用 `std::time::Instant` 仅 cfg(test)，D11）
  - 验证：`cargo test -p eneros-iec62351` 22/22 全过 ✅

- [x] **T5：workspace 接线 + 配置 + 设计文档**
  - [x] 5.1 根 `Cargo.toml` members 追加 `"crates/protocols/iec61850-sv"`（protocols 段 iec61850-goose 之后）与 `"crates/security/iec62351"`（security 段 crypto 之后）
  - [x] 5.2 `configs/iec61850-sv.toml`：`[sv]` app_id / dst_mac / buf_size = 16 + 中文注释 ≥7 点（EtherType 0x88BA / L2Transport 抽象 D4 / 环形缓冲溢出策略 §4.4 / 性能 <4ms 口径 D11 / 内存预算声明 / GPU 不适用 §6.6 / 安全加密由 iec62351 提供）
  - [x] 5.3 `configs/iec62351.toml`：`[security]` key_lifetime_ms = 3600000 / initial_key_id = 1 + 中文注释 ≥7 点（SM4-GCM 选型 §5.1 / SM3-HMAC 认证 / 密钥轮换策略 §4.4 / IV 构造规则 §4.5 / 性能 <0.5ms 口径 D11 / 内存预算声明 / GPU 不适用 §6.6）
  - [x] 5.4 `docs/protocols/iec61850-sv-design.md`：12 章节 + ≥2 Mermaid（SV 帧结构图 + smpCnt 状态机图）+ D1~D12 偏差表（与 spec.md 逐字一致）+ 性能口径声明（D11）
  - [x] 5.5 `docs/protocols/iec62351-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 安全校验流程图重绘 + SecureFrame 结构图）+ D1~D12 偏差表（与 spec.md 逐字一致）+ 性能口径声明（D11）
  - 验证：`cargo metadata` 解析成功；两 crate 测试全过 ✅（25 + 22，D1~D12 表自动化比对零差异）

- [x] **T6：版本同步 0.108.0 + 全量构建验证 + checklist 核验收工**
  - [x] 6.1 根 `Cargo.toml` version = "0.108.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` 注释；`gate.rs` 注释串尾 2 处追加 v0.108.0 类型清单（11 类型：SvSubscriber/SvSample/SampleStatus/RingBuffer/SvError/SecureGoose/SecureSv/SecureFrame/SessionKey/KeyMgmt/SecError）
  - [x] 6.2 §2.4.2 构建校验：C6 metadata ✅ / C7 两 crate 47 测试（25+22）+ goose 36 回归 + crypto 417 回归 + 全 workspace 回归 ✅ / C8 aarch64 交叉编译（两 crate 均通过）✅ / C9 fmt ✅ / C10 clippy -D warnings ✅ / C11 cargo deny（advisories/bans/licenses/sources 全 ok）✅
  - [x] 6.3 `checklist.md` 逐项核验勾选 + 验收记录
  - 验证：C6~C11 全绿 ✅，checklist 90/90 已勾 + 验收记录已填，收工

# Task Dependencies

- T1 先行（T2 消费 sv_buffer.rs 类型）
- T2 depends on T1
- T3 独立于 T1/T2（密钥管理）
- T4 depends on T3（消费 SessionKey/KeyMgmt）
- T5 depends on T2 + T4（文档需最终代码签名）
- T6 depends on T5
