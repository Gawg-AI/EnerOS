# WP-29.5 交付阻断与封焊链（DR-01、AI-01）修复与证据

执行日期：2026-08-11。蓝图坐标：29 §7（WP-29.5）。

## DR-01（交付级阻断，最优先单点）
- 定位：drivers/fs/src/lfs/filesystem.rs write_file remove 后 write_file_at
  （open 无 create）→ 新路径首写必失败（littlefs2 NO_SUCH_ENTRY）。
- 根因（测试盲区）：① fs 泛型测试全走 MockFileSystem（内存版正常）；② 真实
  Lfs 测试（littlefs2 + RAM）**编译从未通过**——MockBlockDevice 门控
  cfg(any(test, feature="mock")) 不跨 crate 传播，eneros-fs 测试构建无法
  引用（既有问题，本次一并解锁）。
- 修复（方案 B）：write_file 改 tmp+rename 原子写（对齐 tsdb writer.rs
  模式）：清残留 tmp → create(tmp) → write_file_at(tmp) → remove(path) →
  rename(tmp, path)；掉电不丢旧数据。
- 消费方清点（grep）：mqtt/src/qos_store.rs:229（QoS 持久化）、
  device-agent/persist.rs:90、cloud-coordinator/real.rs:74——三处语义均为
  "全量覆盖写"，tmp+rename 语义兼容（幂等，目标不存在时同样成立）。
- 判据：test_write_file_new_path_first_write_ok（空文件系统首写 Ok 且可读
  回——修复前必 FAIL）+ test_write_file_overwrite_no_tmp_left（覆盖写
  语义保持 + tmp 不残留）。解锁后 Lfs 全量 199 测试跑通（修复前 0 个可跑）。
- 修复前复现确认：git stash 验证改动前 eneros-fs 测试即编译失败（E0432）。

## AI-01（封焊链时效削弱）
- 定位：chain_sink.rs:197 now.saturating_add(cmd.ttl_ms as u64)——毫秒 TTL
  加到秒基准时钟（wire 格式 epoch 秒，envelope.rs:24-25/gateway.rs:189/
  ingress.rs:437 三方确证）→ 信封有效期放大 1000 倍（名义 5 分钟 → ~3.5 天）。
- 执行裁决（29 提交时）：蓝图推荐字段改名 ttl_secs，但 29 排斥矩阵禁止
  触碰 coordinator.rs（32 域）——先采用 04 证据方案一：chain_sink 内部
  统一秒基准 `ttl_secs = (cmd.ttl_ms / 1000).max(1)`（字段名暂保持，32 域
  coordinator 装配点适配时一并改名；当时提交明示该决策）。
- **复查补齐（2026-08-11 二次会话）**：32 域已验收，排斥矩阵解除——按蓝图
  字面完成字段改名：`DispatchCommand.ttl_ms` → `ttl_secs`（sink.rs），
  chain_sink 直接用 `cmd.ttl_secs.max(1)`（无绕路换算），coordinator
  构造点 `300_000 → 300`（秒），runner sink JSON 键 `ttl_ms → ttl_secs`；
  全部 10 个构造点经编译器强制适配。dual-brain 59 / runner 20 测试全绿。
- 判据：test_chain_sink_envelope_expiry_in_seconds——ttl_secs=300 时
  expiry − timestamp == 300s ± 1（修复前为 300_000s）。RecordingExecutor
  增信封记录 + RcExecutor 共享适配器。
- 装配点：runner lib.rs:217/300 与 coordinator ~430 均按秒契约传值
  （lib.rs 测试 60s、coordinator 300s）；device-agent ingress 复验秒比较
  不受影响。

## 回归
- eneros-fs 199 / eneros-dual-brain 56 / eneros-control-gateway 全绿，0 失败。
