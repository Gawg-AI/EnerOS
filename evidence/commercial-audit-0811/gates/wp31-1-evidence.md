# WP-31.1 IEC104 master 三连（PR-01/02/14，吸收 WP-27.8）修复与证据

执行日期：2026-08-11。蓝图坐标：31 §3。

## PR-01（P1，无 S 帧确认/序号校验/k 窗口）
- 语义修正：`pending_acks` 死字段拆分——`tx_unacked`（发送侧未确认，k 窗口）
  + `rx_unacked`（接收侧待确认，t2 时限）+ `last_rx_i_ms`。
- 收 I 帧 → 立即发 S 帧（R(S) = 下一期望接收序号）→ rx_unacked 归零；
  `poll_acks`（t2=10s 兜底）接入 poll。
- k 窗口：send_i_format 前 `tx_unacked >= 12 → WindowFull`（暂停发送）。
- S 帧 R(S) 校验：`recv_seq > send_seq → ProtocolViolation`（fail-closed）。
- 判据：test_i_frame_triggers_s_frame_ack（I 帧 → S 帧，ack_count ≥ 1）、
  test_k_window_limits_unacked_i_frames（12 帧后 WindowFull）、
  test_t2_timeout_triggers_late_ack。

## PR-14（P1，硬编码 127.0.0.1:2404）
- MasterConfig 增 peer_ip/peer_port（装配层注入；Default 仅测试/开发兜底）；
  adapter 两处 `RemoteDevice::new([127,0,0,1], 2404, ...)` → `master.peer()`。
- grep 判据：生产路径 `127, 0, 0, 1` 归零（仅测试段/Default 保留）。

## PR-02（P1，无生产传输）
- 新增 src/real.rs `RealMasterTransport<D: NetDevice>`（eneros-net/smoltcp
  客户端模式，对齐 slave SmoltcpSlaveTransport；connect fail-closed；
  local port 20000-20100 扫描）。Cargo.toml 增 net feature。
- **loopback 集成判据**（蓝图 §3 验证）：tests/loopback.rs——127.0.0.1
  起 slave 测试桩（STARTDT_CON + 总召应答 + 遥测 3.14@ioa42），master 经
  StdMasterTransport（互操作测试专用，蓝图/27 §12 许可）总召 → 遥测回包
  PASS。修掉三个坑：桩 bind 竞态（先 bind 后 accept）、recv 阻塞死锁
  （非阻塞 + WouldBlock→None）、粘包丢帧（pending 缓冲切帧）。

## 回归
iec104-master 62 单元 + 1 集成全绿。
