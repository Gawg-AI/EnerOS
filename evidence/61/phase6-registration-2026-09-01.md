# 蓝图 61 Phase 6 登记与退出 gate 核销（v0.314.44-46）

日期：2026-09-01
提交：WP-6.1（model-harness v0.314.44）、WP-6.2（skill/loop v0.314.45 + 补遗）、WP-6.3（adapters v0.314.46）
测试：model-harness 78、business-services 89 单元 + 集成 4（objective shadow）+ 全仓 pre-commit 六门 PASS（fmt/clippy/audit/test/version-gate/mock-gate）

## 1. 退出 gate 核销

### WP-6.1（[11] §3.10）
| 判据 | 状态 | 证据 |
|---|---|---|
| 全仓生产模型调用只有 ModelHarness 一个入口 | PARTIAL→登记 | `crates/ai/runner`（eneros-ai-runner-demo）保持 DEMO_ONLY（JSONL FileCommandSink 非执行）；svc-ai-worker 启发式后端不经 harness——生产收编随 svc-model-harness 装配（Phase 7 前置） |
| model-deploy 不再仅凭 prompt pass 进入 Approved | PARTIAL→登记 | model-deploy 无 Approved 语义（DeployVerifier 仅加载/推理报告）；harness 侧 OfflineEval 门（golden/对抗/安全/拒绝率）已实现为离开 Registered 的前置——编排关系登记，DeployVerifier 改造不必要（其能力被 harness 取代） |
| 模型节点 capability closure 无 writer/Gateway key | PASS | `CapabilityClosure`（无字段）+ `authorized_command_not_reachable` 源码扫描测试 |
| Phase 0-5 全链 ModelOff 可运行 | PASS | `model_off_short_circuits` + `shutdown_returns_to_d1` + 无全局 static 源码测试；objective-engine 不依赖本 crate |

### WP-6.2（[11] §4.7）
| 判据 | 状态 | 证据 |
|---|---|---|
| 工具 allowlist 由 capability/manifest 双重限制 | PASS | loop 双门（allowlist ∩ capabilities）；`loop_enforces_allowlist_and_capability` |
| max_steps 等硬代码计数 | PASS | `loop_hard_counts_budgets`（64 步计划在 max_tool_calls=6 处拒） |
| 任一 skill 输出到设备的静态路径不存在 | PASS | 封闭 `ToolId`（无写设备变体）+ `builtin_skills_have_no_device_write_tools` |
| kill ModelHarness 系统回 D1 | PASS | `shutdown_returns_to_d1`（卸载后 run → ModelNotLoaded） |

### WP-6.3（[11] §5.9）
| 判据 | 状态 | 证据 |
|---|---|---|
| qemu-prod/正式 profile 不含 UnavailableTransport | PASS | 全仓代码零 `UnavailableTransport`（残留仅注释提及历史）；svc-market/svc-cloud-sync 装配 `DisabledMarketTransport`/`DisabledCloudTransport` + svc-market 0x04 STATUS（0=disabled） |
| 三类事实可追到外部 revision 和签名 | PASS | FactEventSink 条目保留完整 canonical（含签名）+ 外部 id/revision；合同套件断言可追溯 |
| 全仓 adapter 无 Gateway/AuthorizedWriter 依赖 | PASS | business-services 源码零 `AuthorizedWriter`/`ControlGateway` 引用（grep 实证） |

## 2. 登记项（诚实声明，不宣称未做）

1. **svc-market EIPC 行为变更**：0x02 ACK 现显式失败（`ERR:peer-disabled`）——peer 未部署时不存在合法签名回执路径；QEMU 侧若有用例需按 STATUS(0x04) 语义更新。target 侧编
2. **外部事实 target 装配**：FactEventSink 的 StoreSink/event-store 接线、svc-process/svc-enterprise binary 未建（host adapter + 合同套件已就绪）；svc-market 的 SignedPeerReceipt 走 EIPC 的 wire 扩展未做（0x02 已 fail-closed，扩展随真实 peer 接入）。
3. **IntentCandidate → ObjectiveCompiler 生产启用**：`CompileInputs.intent` 仍 `IntentDisabled`（Phase 4 生产禁用保持）；harness 输出的候选经人工审批构造 ObjectiveSpec 的接缝已就位（ObjectiveDraft 层 7 语义），编译面启用随 Phase 7。
4. **qemu-prod 实弹验证**：本卷未跑 QEMU（model/adapters 均为 host 面；svc-market 行为变更需 QEMU 窗口重验——纳入 Phase 7 回归），host 全量测试 + pre-commit 六门 PASS 为当前证据。
5. **target 侧编译**：aarch64-none-elf-gcc 交叉工具链不在本环境，`cfg(target_os="none")` 代码（svc-market/cloud-sync 修改）未在 target 侧编译验证；改动为纯类型/名称替换 + 控制流简化，下一 QEMU 窗口首查。

## 3. 附：本卷顺手修复

- 测试抓到 `MemOutbox::mark_sent` 不更新 state（与 AppendOutbox 行为不一致）——修复后「Sent 重放如实报 Sent」成立；该差异会掩盖 outbox 分层语义问题。

## 4. 独立复查（2026-09-01，第二轮）：8 缺陷全修

复查方法：全量新代码通读 + 每缺陷先写复现测试（红）→ 修复 → 回归绿；交叉面 grep（UnavailableTransport 残留/MKT1 0x02 库内消费方/FactReject 穷举匹配/BoundedLoop 外部调用方/workspace-lock 版本同步）均零次生影响。修复后 model-harness 84（78+6 复现测试）、business-services 90（89+1）、集成 4 全绿；fmt 收敛（Sink 变体触发 FactReject 枚举 rustfmt 重排）。

| # | 位置 | 缺陷 | 修复 |
|---|---|---|---|
| 1 | gate.rs `parse_hex16` | 32 **字节**但含多字节 UTF-8 的引用串按字节下标切片 → **panic**（不可信模型输出直达崩溃面） | 字节级 hex 解码；非 ASCII hex 字节一律拒绝（结构化 Reference 拒） |
| 2 | registry.rs `restore` | **eval 报告 40B 解析后游标未推进（`p+=40` 缺失）**——快照中带评测条目后还有任一条目 → 恢复必然错位失败（单条目 roundtrip 测试掩盖；现网恢复路径实际不可用） | 补 `p += 40`；新增多包快照恢复测试 |
| 3 | registry.rs `restore` | 过期条目**整表拒绝毒化**——老包（含 Retired）过期后重启 → 全部模型不可用；且 now_ns=0（时钟未知）极性与 `prepare_inference`（放行）相反 | 签名先验后过期条目跳过（过期是预期状态非篡改；篡改面仍整表拒）；now_ns=0 不做过期裁决，口径统一 |
| 4 | harness.rs `run` | 非 active 包延迟挂起发生在请求中途时 `health_signal` 返回 None → 继续走门并以 **stale Approved 生命周期返回 Candidate** | 挂起后查状态，Suspended → 结构化拒绝 |
| 5 | skill.rs `SkillRegistry::register` | 缺**结构门 + package_sha256 摘要门**（签名只覆盖 canonical；instructions 被替换、max_tool_calls>max_steps 均可通过注册；与注释声称的「摘要门」不符） | `validate_structure()` + `package_digest()` 比对（与 ModelRegistry 装配对等） |
| 6 | loop.rs `BoundedLoop::new` | 构造器 `note_tool_call` **预扣一次预算** → max_tool_calls=1 的 skill 0 次可用（off-by-one） | 显式 `max_tool_calls==0` 校验（不消耗），首次真实调用才计数 |
| 7 | loop.rs `run` | 工具 remaining = **相对** max_runtime_ns − **绝对** now_ns → 生产 epoch 量级恒饱和为 0，一切工具收 0 预算必炸 | `min(deadline − now, max_runtime)`；新增 epoch 量级预算捕获测试 |
| 8 | external_facts.rs `FactAdapter::accept` | sink 存储故障 `map_err(\|_\| BadWindow)`——**存储面故障谎报成窗口语义**（审计失真；该路径拒绝计数不一致） | 新增 `FactReject::Sink(&'static str)` 显式上报；认证已过 → 计数/水位不动（可重试） |

另：lifecycle.rs `LifecyclePolicy` 注释声称「构造时校验」实无校验——注释如实化（NaN 阈值=挂起永不触发，属装配方信任配置责任，已在注释言明）。

**观察项（登记不修）**：eval 空集 vacuous pass（无对抗/安全用例的套件按 1.0 通过——评测套件经 manifest 签名锁定，签发方受信，防御性收紧随 Phase 7）；`PackageLifecycle::rollback()`/`LifecycleBook` 本卷无生产调用方（回滚经 active() 兜底语义实现）；scratchpad 1024B < ToolResult PAYLOAD_MAX 8192B（超 1KB 工具结果会终止 loop，装配期需对齐）；`ProjectedField.time_ns/device_id` 未进 canonical/prompt（模型不可见，死数据）。