# WP-30.7 DurableLog 新写 + 持久化收口（WP-27.5 吸收，S5 第三层）修复与证据

执行日期：2026-08-11。蓝图坐标：30 §9（机制层；四处落盘收口中 30 域部分）。

## 新 crate：crates/runtime/durable-log（eneros-durable-log）

- `DurableLog` trait（WP-27.5 规格）：`append(rec) -> Seq` / `read_from(seq, f)`
  / `recover() -> Seq`。
- `BlockDurableLog<D: BlockDevice>`：基于已活体 virtio-blk 后端（BlockDevice
  seam，D4）——头块（magic/version/tail_seq + CRC 原子提交）+ 记录区线性
  追加（每条记录独占一块：seq8+len4+crc4+payload，跨块 padding）；
  **掉电语义**：先写记录再原子更新头块 tail_seq，两步之间掉电 → recover
  以 tail_seq 为界截断（半写丢弃，不返回损坏数据）。
- `PersistentAuditStore<D>`：`AuditStore` 的 DurableLog 后端（ChainEntry
  定长 112B 编码），供 `AuditLoggerImpl::recover` 重建哈希链。

## 判据测试（host，MockBlockDevice RAM 后端）

- append_recover_roundtrip：序号 1/2 连续；recover 返回 2；recover 后可
  继续追加（序号连续）。
- read_from_replays_in_order：3 条按序回放。
- corrupt_record_rejected：篡改记录 payload → Corrupt，损坏记录之后不返回
  数据（fail-closed）。
- persistent_audit_store_roundtrip：AuditStore trait 落盘/读回/count 一致。

## RT-028① 收口（PANIC_GENESIS 真实 SM3）

- root-task panic.rs + services panic.rs：ASCII 占位常量 → 运行时真实
  `sm3_hash` 创世根（genesis() 函数）——"创世"不再可被离线伪造。
- **执行裁决（提交明示）**：root-task 持久化**接线**（DurableLog 挂
  BlkAdapter + AuditLoggerImpl::recover）随 33 域 WP-27.4 掉电证据出证
  （块设备生命周期与掉电演练耦合）——蓝图 30 §9"本版本提供机制，33
  出证"；BootInfo ABI 增创世根字段留 34 域。
- 附带修复：29 RT-028② 的 services try_lock 改动存在编译错误（spin 0.9
  try_lock 返回 Option，`.as_mut()` 误解析）——本次修正（services 无测试
  覆盖漏网，现 23 测试全绿）。

## 回归

durable-log 4 / root-task 60 / services 23 全绿。
