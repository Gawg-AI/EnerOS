# 31 版本验收报告（蓝图/31 §9 DoD）

执行日期：2026-08-11。版本：31（协议与驱动零硬件依赖）。
9 个提交：e033d98（31.1）/ 45b0fb5（31.2）/ 9fc8d31（31.3）/ acc2d73（31.4）/
b7b970f（31.6）/ 061e84a（31.5）。

## DoD 逐项

| # | 判据 | 状态 | 证据 |
|---|---|---|---|
| 1 | PR/DR 清单全销（DR-01 已在 29） | ✅ | PR-01~16 全处理；DR-01~12 全处理（D2/D10 按 R31-2 拆分登记 33 §真机填空，不宣称全通） |
| 2 | 零硬件依赖 grep 判据全过 | ✅ | 127,0,0,1（master 生产 0）、0x0906/0x0907（0）、now_ms += 1（0）；绑定表 = pl011/virtio-mmio/sja1000/rs485 + sp805/pl031（armv8-timer 经 DTB 提取） |
| 3 | IEC104 loopback 集成 + GOOSE 参考报文 | ✅ | wp31-1 loopback（总召→遥测 3.14）+ wp31-3 goose_ref.bin fixture（valid 解码 + malformed 拒绝） |
| 4 | D 矩阵 host/QEMU 证据 | ✅ | D5/D6 绑定表 + D9 判据 2 项 + D10 FCS 向量/APN；真机项入 33 填空清单 |
| 5 | 回归 | ✅ | workspace 6953 / QEMU 13/13 + probe-report 基线一致（bound=2） |

## QEMU 活体

- 540s 运行（QEMU_EXIT=124 预期）：序列门 **13/13 PASS**；probe-report
  bound=2/unbound=23/failed=31 与 29/30 基线一致（新绑定条目 arm,sp805/
  arm,pl031 在 QEMU virt DTB 无节点 → 如实 unbound；真机 DTB 注入后生效）。
- 证据：gates/gkernel-qemu-boot-31.log（25 MB）。

## 治理自查

- gate 四子命令全绿（29 修真后未回退——版本验收重跑）。
- mock 门控：无新 Mock 生产面（31 无新增）。
- 豁免/拆分登记：D2（virtio-net 完整驱动）、D10（PPP TX 协商/PAP）、
  DR-09（used ring 精确 head 校验）、D9 GPIO 真接线、DR-07——33 §真机
  填空封闭清单。
