# EnerOS SDK 架构（统一）

> 对应：07 蓝图 ARCH-S04；ADR-0025（插件执行模型）、ADR-0027（点模型）、ADR-0028（驱动 seam）
> 状态：v1 冻结

## 1. 统一插件执行模型

Agent、协议、驱动三类插件共享同一执行模型（MASTER §10.1 / ADR-0025）：

```
签名静态 ELF PIE → 受限 EL0 进程 → 版本化 IPC wire
```

- **禁止** cross-binary Rust ABI：`*mut dyn`、trait object vtable、String/Vec/enum 直传。
- 宿主进程内（同一地址空间）的 trait 实现仅用于编译期组合。
- 跨进程通信一律经 `eneros-ipc-wire` 共享层的帧规则（固定头 + 有界长度 + CRC32）。

## 2. SDK 清单

| SDK | crate | 形态（v1） | 插件接口 | wire |
|---|---|---|---|---|
| Agent SDK | `eneros-agent-sdk` | 静态框架 + IPC wire | `Agent` trait（宿主内） | `wire.rs`（EIPC，16B 头含 target） |
| Protocol SDK | `eneros-protocol-sdk` | IPC wire codec 客户端 | `ProtocolPlugin` trait（宿主内）+ `wire::ProtocolRequest/Response` | `wire.rs`（EPRO，12B 头） |
| Driver SDK | ~~`eneros-driver-sdk`~~（已删，ADR-0028） | 唯一驱动 seam = `eneros-driver-framework` | `DeviceDriver` trait | 未来驱动进程插件化时复用 `eneros-ipc-wire` |

## 3. 共享 crate：`eneros-ipc-wire`

公共部分（三 SDK 一致）：

- `crc32` — 帧完整性 CRC（消除 agent/protocol 重复实现）；
- `WIRE_VERSION` / `wire_version()` / `abi_compatible()` / `SDK_ABI_VERSION` — 版本发现与插件 manifest 握手；
- `CRC_LEN` / `check_payload_len()` — 长度边界规则。

各 SDK 保留帧布局差异（magic、header 结构），但完整性、版本、长度规则单一来源。

## 4. 点模型（ADR-0027）

- canonical 点类型（`DataPoint`/`PointValue`/`PointQuality`/`PointId`/`DeviceId`）唯一来自 `eneros-upa-model`；
- protocol-sdk 已 re-export；其他模型（telemetry-model IEC 四遥）经 `bridge` 转换；
- 全仓禁止新定义等价点类型。

## 5. 版本发现与兼容规则

- 插件 manifest 声明 `abi_version`；宿主用 `abi_compatible()` 校验，major 不匹配 fail-closed；
- wire 帧 version 由 `WIRE_VERSION` 单一来源；
- 破坏性 ABI 升级必须同步系统 A/B 槽处理（MASTER §7.5）。

## 6. 测试要求（07 §4）

- wire 层：round-trip 属性测试 + 畸形输入（截断/坏 magic/坏 CRC/长度越界/未知类型）；
- 转换层：round-trip 属性测试（不接受样例断言）；
- seam 层：mock 与每个真实适配器跑同一契约套件。
