# Tasks — v0.107.0 IEC 61850 GOOSE 快速事件传输

> Spec：`spec.md`（develop-v10700-iec61850-goose）。T1→T2 顺序（T2 消费 T1 数据集类型）；T3 依赖 T2；T4/T5 顺序收尾。

- [x] **T1：新建 crate 骨架 + dataset.rs + lib.rs — L2Transport 与数据集**
  - [x] 1.1 `crates/protocols/iec61850-goose/Cargo.toml`：`eneros-iec61850-goose`，workspace 继承，依赖仅 `eneros-iec61850-model = { path = "../iec61850-model" }`
  - [x] 1.2 `src/lib.rs`：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc` + 模块声明（dataset/goose_tx/goose_rx）+ 重导出 + `GooseError`（4 变体：TransportError/BerEncodeError/BerDecodeError/InvalidConfig，D10，derive Debug/Clone/PartialEq）+ `L2Transport` trait（send/recv）+ `MockL2`（帧队列 + 发送记录 + 注入错误，D4）+ crate 文档（版本定位 + 核心类型清单 + D1~D12 偏差表 + no_std 合规声明，风格对齐 iec61850-mms）
  - [x] 1.3 `src/dataset.rs`：`GooseDataset { pub entries: Vec<GooseEntry> }` / `GooseEntry { pub path: String, pub value: DaValue }`（derive Debug/Clone/PartialEq）；`new()`、`set(path, value)`（有则覆盖无则追加）、`get(path)` → Option
  - [x] 1.4 测试 DS1~DS6（6 个，见 spec 测试规划表）
  - 验证：`cargo test -p eneros-iec61850-goose dataset::` 6/6 全过 ✅

- [x] **T2：goose_tx.rs + goose_rx.rs — 发布者/订阅者 + BER 编解码**
  - [x] 2.1 `src/goose_tx.rs`：`GooseControlBlock`（全部字段 pub，derive Debug/Clone/PartialEq）+ `GoosePublisher<T: L2Transport>`（D5）：`new(cb, transport)`（app_id==0 → InvalidConfig）/`update_value`（st_num+1、sq_num=0、needs_retransmit=true）/`publish(now)`（dst MAC + 组播 src MAC 01:0C:CD:01:00:00 + EtherType 0x88B8 + GOOSE PDU BER：gocbRef 0x80 / timeAllowedToLive 0x81 / datSet 0x82 / goID 0x83 / t 0x84 8 字节 / stNum 0x85 / sqNum 0x86 / simulation 0x87 / confRef 0x88 / ndsCom 0x89 / numDatSetEntries 0x8A / allData 0xAB **含长度**（D7）；数据 tag 统一 boolean 0x80 / integer 0x85 / floating-point 0x87（D8）；发送后 sq_num+1、last_tx_time=now）/`retransmit_if_needed(now)`（前 3 次 min_time、其后 max_time；return bool）；`cb()`/`dataset()`/`transport()`/`transport_mut()` 访问器
  - [x] 2.2 `src/goose_rx.rs`：`GoosePdu`（st_num/sq_num/timestamp/dataset）+ `RxStatus`（New/Duplicate/StJump，D12，derive Debug/Clone/Copy/PartialEq）+ `GooseSubscriber<T: L2Transport>`：`new(app_id, mac, transport)` / `set_callback<F: Fn(&GoosePdu) + 'static>`（去 Send+Sync bound，D9）/`poll()` → Option<(GoosePdu, RxStatus)>（0x88B8 过滤 / MAC 过滤 / APPID 过滤 → Ok(None)；st_num 跳变 → StJump、重复 → Duplicate、新帧 → New；截断 → BerDecodeError）；`last_st_num()`/`transport_mut()` 访问器
  - [x] 2.3 测试 TX7~TX18 + RX19~RX30 + LB31~LB36（30 个，覆盖点见 spec 测试规划表；LB35 全链路 < 4ms 用 `std::time::Instant` 仅 cfg(test)，D11）
  - 验证：`cargo test -p eneros-iec61850-goose` 36/36 全过 ✅

- [x] **T3：workspace 接线 + 配置 + 设计文档**
  - [x] 3.1 根 `Cargo.toml` members 追加 `"crates/protocols/iec61850-goose"`（protocols 段 iec61850-mms 之后）
  - [x] 3.2 `configs/iec61850-goose.toml`：`[gocb]` go_cb_ref / app_id / dst_mac / min_time_ms = 2 / max_time_ms = 5000 / dataset_ref + 中文注释 ≥7 点（L2 选型 §5.1 / EtherType 0x88B8 组播 / 重传策略 §4.3 / L2Transport 抽象 D4 / 时间注入 D6 / 性能 <4ms D11 / 内存预算 / GPU 不适用 §6.6 / 安全待 v0.108.0 §7.3）
  - [x] 3.3 `docs/protocols/iec61850-goose-design.md`：12 章节 + ≥2 Mermaid（蓝图 §4.3 重传时序图 + GOOSE 帧结构图）+ D1~D12 偏差表（与 spec.md 逐字一致）+ 性能口径声明（D11）
  - 验证：`cargo metadata` 解析成功；`cargo test -p eneros-iec61850-goose` 36 全过

- [x] **T4：版本同步 0.107.0 + 全量构建验证**
  - [x] 4.1 根 `Cargo.toml` version = "0.107.0"；`Makefile` VERSION + L3 头部注释；`ci.yml` 注释；`gate.rs` 注释串尾 2 处追加 v0.107.0 类型清单（10 类型：GooseControlBlock/GooseDataset/GooseEntry/GoosePublisher/GooseSubscriber/GoosePdu/RxStatus/GooseError/L2Transport/MockL2）
  - [x] 4.2 §2.4.2 构建校验：C6 metadata / C7 本 crate 36 + 全 workspace 回归 / C8 aarch64 交叉编译（`cargo build -p eneros-iec61850-goose --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`）/ C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - 验证：C6~C11 全绿

- [x] **T5：checklist 逐项核验收工**
  - [x] 5.1 `checklist.md` 逐项核验勾选 + 验收记录
  - 验证：checklist 全勾，收工

# Task Dependencies

- T1 先行（T2 消费 dataset.rs 类型）
- T2 depends on T1
- T3 depends on T2（文档需最终代码签名）
- T4 depends on T3
- T5 depends on T4
