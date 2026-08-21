# 蓝图 48 卷 Entry 登记（锚点重验 + 前置检查）

> 日期：2026-08-22 · 基线版本：0.313.24 · 基线 commit：f55bdd6（47 卷收口）

## 锚点重验（44-00 §4.5.1，全部实跑）

| 锚点 | 结果 |
|---|---|
| WP-48.0：iec104-master 复用 slave ASDU | ✅ `Cargo.toml:16` 依赖 `eneros-iec104-slave`；`lib.rs` `pub use eneros_iec104_slave::{Asdu,…}`；asdu.rs 1918 行在 slave |
| 46 卷 Exit（D2 硬前置） | ✅ `evidence/46/exit-gate.json` 在库，状态板 Accepted（0.313.16） |
| 47 卷 Exit（上卷） | ✅ Accepted（0.313.24，f55bdd6） |
| ProtocolPlugin 六方法 seam | ✅ `protocol-abstract/src/plugin.rs`（含 47 卷 `drain_write_intents`/`write_point`） |
| `export_write_intent` 写链 | ✅ `plugin.rs:339` |
| 传输工厂（46 卷） | ✅ `protocol-services/src/net_transport.rs`：NetTransportFactory（Tcp/Udp/RawEthernet）+ SerialTransportFactory；46.1 结论=QEMU 无第二 PL011，Serial 验证走 host 独立 peer |
| service-kind-registry / sku-matrix / configs registry | ✅ `product/service-kind-registry.json`、`product/sku-matrix.json`、`configs/registry.toml`（ACTIVE 13） |
| 蓝图假设「FT1.2/18657 在库证据」 | ❌ **锚点修订**：仓内无任何 FT1.2/GB-T 18657 实现（104=TCP APCI 非串口 FT1.2；grep `18657|FT1.2|FT12` 零命中）→ 101/103 的 FT1.2 链路层字节格式**无在库权威**，与 4 协议 codec 一并 BLOCKED（见下） |

## Entry 前置检查（goal 条目 3）

1. **规约原文 ×4：环境内不存在**。全盘搜索（repo tracked/untracked、`docs/standards/`、
   家目录 文档/下载/桌面/Documents/Downloads、`/tmp` `/media` `/mnt`、全盘 `*.pdf|*.doc*`）
   均无 DL/T 645-2007 / IEC 60870-5-101 / -103 / GB/T 27930-2015 文本。
   → 依 goal「缺任一 → 该协议登记 BLOCKED，不得凭记忆开发」+ 蓝图执行模式 §2：
   **4 协议 codec 字段实现与权威向量 KAT = BLOCKED（owner 输入：规约文本）**；
   其余（WP-48.0、crate 骨架/adapter/plugin seam/传输接线/配置 ACTIVE 登记、
   27930 蓝图文本内状态机结构）按蓝图连续执行，不整体停等。
2. **27930 角色 = 桩端**：owner 默认裁决（蓝图卷首），不再停顿。
3. **上卷回归（44-00 §4.5.2，≥2 项实跑）**：
   - `grep -rn "w40-prod-key" crates/ xtask/ product/` → **0 命中** ✅
   - `cargo xtask qa gate keys` → tracked-keys=0 / default-refs=0 / test-keys=present /
     production-injection=fail-closed 全 **PASS** ✅

## 锚点修订记录（日期 + 差异 + 依据）

- 2026-08-22：蓝图 WP-48.2「已从仓内 104 实现可确证的部分（FT1.2 帧格式…）
  可直接引用仓内既有测试与 18657 系列在库证据」与代码不符——104 无 FT1.2
  （TCP APCI），仓内无 18657 证据。修订：101/103 FT1.2 字节层入 BLOCKED；
  101 可确证部分=WP-48.0 抽取后的共用 ASDU 层（真共享代码 + 既有单测）。
