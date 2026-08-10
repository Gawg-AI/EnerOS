# WP-31.4 协议卫生批（PR-09~13、PR-15）修复与证据

执行日期：2026-08-11。蓝图坐标：31 §6。

| ID | 修复 | 判据 |
|---|---|---|
| PR-09 | Modbus MBAP `try_new`（data_len >252 → None）；build_frame 链 Result 化（PduTooLong） | test_mbap_rejects_oversized_pdu |
| PR-10 | MQTT CONNECT will payload 超 u16::MAX → 编码拒绝（PacketEncode） | 编解码测试保持 |
| PR-11 | PlainTls → PlainTcp（S8 诚实命名） | grep PlainTls 归零 |
| PR-12 | MQTT QoS1 超时 DUP=1 重传（inflight 缓存 + ≤2 次重试） | test_qos1_timeout_retransmits_dup |
| PR-13 | MQTT decode 固定头 flags 校验（SUBSCRIBE/PUBREL/UNSUBSCRIBE=0b0010） | 既有 decode 测试保持 |
| PR-15 | mqtt real.rs/mms_net.rs/iec104-master adapter `now_ms += 1` → eneros_time 单调时钟（9 处） | grep now_ms += 1 归零 |

## 回归
mqtt 48（+1 判据）/ mms 38 / modbus-tcp 58 / iec104-master 62+1 全绿。
