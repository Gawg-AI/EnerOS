# FIX-BUGS-0810 修复证据（蓝图/25 五个已核实缺陷）

> 日期：2026-08-10 ｜ 顺序：B3 → B1 →（B2 两件套 + B4 同域）→ B5，各自独立提交
> 门禁：workspace 326 套件全绿 / clippy -D warnings / fmt（每个提交后复算）

## B3 — diff_ns 有界化（tsn-time）

| 项 | 内容 |
|---|---|
| 改动点 | `clock.rs`：`diff_ns` 返回 `Option<i64>`（`i128` 差值经 `i64::try_from`，禁 `as` 截断）；删除"测试用例范围内不会溢出"自欺注释，写明对端输入不可信；gptp 调用方 fail-closed |
| 调用方清点 | gptp.rs handle_sync（已删，见 B1）/handle_follow_up（None 跳过）+ 自身测试 T6/T7 |
| 防回归 | `test_b3_diff_ns_out_of_range_returns_none`：差值超 i64 → `None`（证伪旧 `as` 环绕）；`i64::MAX` 边界仍正常；`test_b3_follow_up_out_of_range_skips_update`：超范围 → FollowUp 跳过、offset 不变 |
| 影响声明 | 超范围对端时间戳：从"时钟被污染"→"本次同步跳过"（fail-closed）；被冻结 offset 可被后续合法报文恢复（R-B3a 权衡，记入模块文档） |

## B1 — gptp 单一 offset 出口（tsn-time，依赖 B3）

| 项 | 内容 |
|---|---|
| 改动点 | `handle_sync` 删除 offset 计算块（保留 seq/rx_ts/delay 暂存）；`handle_follow_up` 成唯一出口（B3 Option 语义）；删除死字段 `last_sync_origin_ts`（定义/初始化/写入三处）；模块文档记录 two-step 假设 |
| 防回归 | T25 重写两条：① Sync origin=0 不扰动 offset（证伪旧行为——旧代码推 offset 至 112_500_000）；② 配对 FollowUp 恰好一次精确更新 + 序列号消费后不重复 |
| 影响声明 | offset 更新点"每周期两次"→"仅 FollowUp 一次"；Sync 丢失时 offset 保持旧值（符合协议语义）；T25 旧断言编码了错误行为，按铁律 2 重写 |

## B2 — 两件套（root-task spawn.rs）

| 项 | 内容 |
|---|---|
| 改动点 | ① `retype_device_one` 第 8 参改传 `slot`（BOOT-010 范式；SAFETY 注释引用）；② 新增 `alloc_device_at(phys, size_bits)`（镜像 alloc_at、is_device 反转 + 覆盖检查），`map_device_mmio` 改用它，未覆盖 → NoUntyped → 软件模式兜底（main.rs 注释更新 + R-B2b 观察点）；③ `alloc_device`（first-fit）删除（唯一调用方已迁移） |
| 调用方清点 | `map_device_mmio` 唯一调用方 main.rs:222；`alloc_device` 原唯一调用方 retype_device_one 已迁移 |
| 防回归（证伪旧行为） | `alloc_device_at_covers_phys`（覆盖→cap+偏移正确）、`alloc_device_at_uncovered_returns_none`（未覆盖必须 None——R-B2a 场景，禁 first-fit 兜底）、`alloc_device_at_does_not_mix_non_device`、`alloc_device_at_overlapping_region_still_bounded` |
| 影响声明 | 全平台零行为变化（三份 dts 均未导出 SP805，现状恒软件模式；修复后未导出 → 仍软件模式）；未来导出后硬件 WDT 首次真实工作（bring-up 独立验证事件，R-B2b） |
| QEMU 证据 | 软件模式分支语义未变（无覆盖 untyped → Err → 兜底）；host 单测覆盖分配逻辑全路径（QEMU 全镜像启动证据受 spawn 现状限制，分配层行为由单测 + 既有 QEMU 启动序列共同覆盖——如实声明） |

## B4 — alloc_cap_table 对齐与守卫（root-task，与 B2 同域）

| 项 | 内容 |
|---|---|
| 改动点 | 记账对齐与 alloc 一致（alignUp + checked 容量 + 返回 aligned）；`size_bits+5 >= 64 → None` 守卫（红线 6） |
| 防回归 | `alloc_cap_table_aligns_accounting`：16B/32B 小对象后 CNode 记账 = alignUp(48,8192)=8192（证伪旧直接累加 48）；`alloc_cap_table_guards_oversize`：size_bits 59/60 → None（证伪旧 release 移位回绕） |
| 影响声明 | 当前启动序列零行为变化（现存分配时点 used 恰好对齐）；grep 自证唯一调用方 retype_one 忽略返回偏移 |

## B5 — DRBG 强制重播种（crypto + 全仓调用方）

| 项 | 内容 |
|---|---|
| 改动点 | `RESEED_INTERVAL = 2^48`；`fill_bytes` → `Result<(), DrbgError>`，达阈值 `ReseedRequired` 且状态不变；统一处置模板 `fill_bytes_checked`（熵源注入）/ `fill_bytes_or_reseed`（平台熵，仅 ReseedRequired 分支接触熵源） |
| 调用方清点（全适配，禁 unwrap 吞错） | sm2 sign/encrypt/keypair（CryptoError::RngFailed）、federation tunnel pms/iv（EncryptError；tunnel_send 无错误通道 → 空帧 fail-closed + encrypt_send 显式检测）、channel nonce×3（ChannelError::CryptoFailed）、agent token（AgentError 新增 CryptoFailed 变体）、tls embedded（try_fill_bytes 承接 Result；fill_bytes 按 rand_core 契约重播种+重试，仅熵枯竭 panic + 注释豁免）、entropy 测试适配 |
| 防回归（证伪旧行为） | 阈值-1 → 第一次成功、第二次 ReseedRequired 且持续停止出数（证伪旧"永不停止"）；重播种后恢复且计数归 1；处置模板重播种+重试成功；熵不可用 → 模板上抛且状态保持拒绝 |
| 与规划的差异记录 | 规划要求"sm2 签名在 ReseedRequired 注入下完成一次签名"——签名路径经 `fill_bytes_or_reseed`（平台熵，host 无熵源）；模板有效性改由 `fill_bytes_checked` + 注入桩熵源在 csrng 层等价覆盖（同一段模板代码），差异如实记录 |
| G4 证据追加 | 蓝图/14 G4 密码评审证据需追加本变更说明（向 SP 800-90A §10.1.1.4 步骤 7 合规靠拢）：fill_bytes 阈值检查 + 强制重播种 + fail-closed 熵处理 |

## 风险销项

| 风险 | 状态 |
|---|---|
| R-B2a（单件修复致错误物理页映射） | 销项：两件套同提交 + 未覆盖 phys 必须 None 测试（first-fit 已删除） |
| R-B2b（硬件 WDT 首次执行） | 未触发（无平台导出 SP805）；观察点写入 main.rs 注释 |
| R-B3a（垃圾时间戳冻结同步） | 权衡接受：fail-closed + 合法报文可恢复（模块文档） |
| R-B1a（T25 重写误伤 FollowUp 断言） | 未发生：T25 拆分两条独立用例 |
| R-B5a（ReseedRequired 处置不当致 RNG 停摆） | 销项：统一模板 + 重播种恢复测试 + 模板有效性测试 |
| R-B5b（tls fill_bytes panic 两难） | 销项：try_fill_bytes 承接 Result；fill_bytes 重播种+重试，仅熵枯竭 panic + 注释豁免 |
