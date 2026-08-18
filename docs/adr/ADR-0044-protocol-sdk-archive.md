# ADR-0044：protocol-sdk 归档为 host 互操作资产（唯一协议 seam = protocol-abstract）

> 状态：Accepted（2026-08-19）
> 决策者：产品负责人 D2（44-00 总纲 §1 裁决，2026-08-18）+ 主开发 AI 执行
> 前置：ADR-0026（protocol-sdk 边缘化）、ADR-0043（seL4 唯一内核）、
>        蓝图 44-00（D2 协议范围裁决）
> 取代：无（将 ADR-0026 的"注释性边界"升级为正式决策）

## 1. 结论

**`eneros-protocol-sdk`（wire codec）正式归档为 host 互操作资产**：
- 不作为产品侧协议 seam——唯一协议 seam = `protocol-abstract`（trait /
  plugin / transport / channel 层）；
- 该 crate 保留编译与互操作测试（KAT / 对端测试复用其 wire 编解码），
  但**不得被任何产品 crate 依赖**（closure gate 执行）；
- 归档标记"archived, host-only"写入 crate 头部 doc 注释。

## 2. 背景

两套协议 SDK 并存多年：
- `protocol-sdk`：IPC wire codec 客户端开发框架（ADR-0025/0026），历史产物；
- `protocol-abstract`：插件 trait seam（蓝图 36 起唯一协议接入点）。

长期边界只靠注释说明（ADR-0026 边缘化），无机器强制。44 卷 D2 裁决：
先闭环现有 7 协议一 seam 接线，新增协议走定型 seam——两套并存升级为
正式归档决策，避免 47+ 卷接入点分叉。

## 3. 决策细节

| 项 | 处置 |
|---|---|
| 协议 seam | 唯一 = `protocol-abstract`（plugin/transport/channel/write/access） |
| protocol-sdk | host 互操作资产：保留编译/测试，closure gate 拒绝产品依赖 |
| 冻结登记 | 见 `product/policy/frozen-crates.json`（44.4 冻结，理由 = 归档） |
| 解冻条件 | 无（不接受产品侧重新依赖；若未来需 wire 编解码，改走 protocol-abstract 实现） |

## 4. 影响

- closure/policy gate：`protocol-sdk` 进入 frozen-crates 清单 → 产品闭包零引入；
- 44.3 release profile 出清：SQL 服务清单不含 protocol-sdk；
- 后续卷（47+）协议新增一律挂 protocol-abstract。

## 5. 证据

- ADR 本文件 + registry.toml 登记；
- `crates/protocols/protocol-sdk/src/lib.rs` 头 doc 归档标记；
- closure gate PASS（protocol-sdk 不在任何 release profile 闭包）。
