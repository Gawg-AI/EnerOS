# 蓝图 61 Phase 7 WP-7.1 登记与退出 gate 核销（v0.314.47，联邦能力与分层优化）

日期：2026-09-01
提交：见 git HEAD（本卷唯一提交）
版本注意：蓝图 12 号文把 Phase 7 标为 v0.314.45-47；Phase 6 实际消耗 .44-46
（Phase 5/6 版本漂移先例），因此 Phase 7 实际版本为 **0.314.47/48/49**。

## 1. 交付内容（蓝图 §12 第 2 节）

| 蓝图条目 | 落点 | 实现 |
|---|---|---|
| §2.2 协议产物 | `offer.rs` | CapabilityOffer/AllocationProposal/AllocationCommitment/CommitmentAck/OfferReject/OfferRevocation/ProgressEvidence/SettlementEvidence + `FedEnvelope` 签名信封（magic/version/epoch/seq/tenant/deadline/digest/key_id/SM2）+ append-only revision；解码 fail-closed（NaN/Inf/截断/超长/bad magic/digest 不符全拒） |
| §2.3 协议流程 | `negotiation.rs` | SiteNegotiator/CoordinatorNegotiator 六步状态机 + 崩溃恢复 restore（journal 重放 + 注册表恢复 + 纪元 +1） |
| §2.4 网络与身份 | `leadership.rs` + `fedxport.rs` + `svc-federation` | 两节点主备+人工 failover 策略门；真实 mTLS（enerso-tls/rustls + 证书链验证）；epoch 单调 + per-peer 窗口（旧 leader 拒收）；SeedPresenceBus 记录式 bus 移出生产闭包（`#[cfg(any(test, feature="mock"))]`） |
| §2.5 隐私最小化 | `offer.rs` 数据最小化测试 | 证据/报价 wire 大小与点位数量无关（BlobRef 定长引用），无原始点位字段 |
| §2.6 分层求解 | `allocation.rs` | 区域层确定性 merit-order 分配（爬坡前向/反向+能量修复），不可行→`InfeasibleEnvelope` 冲突清单（不 default-fill）；站侧 `check_envelope` 复用于拒恶意目标；单位/网格兼容校验 |
| §2.7 测试 | `tests/phase7_security.rs`（9）+ lib 单测 | 100 站 offer/commit/revision；网络分区；协调器重启；旧 leader；证书撤销；重放；能力突降；承诺不可行；部分站离线；跨租户隔离；恶意云目标拒绝且站内继续 |
| §2.8 退出 gate | `tests/phase7_no_writer.rs` + 实弹 | 依赖图无 ControlGateway/AuthorizedWriter；生产路径无记录式 bus；100 站 C3 控制面压力通过 |

## 2. 退出 gate 核销（.47）

| 判据 | 状态 | 证据 |
|---|---|---|
| 联邦依赖图无 ControlGateway/AuthorizedWriter | PASS | `phase7_no_writer`（剥离注释后源码扫描 + Cargo.toml 依赖闭包） |
| 真实 mTLS transport；Unavailable/记录式 bus 不在生产闭包 | PARTIAL→PASS(E3)/登记(E5) | 真实 rustls TLS 1.3/mTLS 双进程实弹（svc coord+site）与进程内 e2e 测试；`UnavailableTransport` 全仓零引用；SeedPresenceBus 已 cfg(test\|mock)。E5（与真实调度主站/对端互操作）登记 BLOCKED（环境无对端硬件） |
| 100 站 C3 控制面压力通过，原始流量不汇聚单点 | PASS(E3 host) | `hundred_sites_offer_commit_revision`（100 站 × 真实 SM2 签名 + 确定性模拟网，60s 界内）；协议层只传 offer/commitment/evidence，原始点位无通道（数据最小化测试） |
| 联邦不把站内安全权威交给云 | PASS | 恶意超范围目标结构化拒绝 + 站内继续自治测试；svc 无 writer capability |
| 两节点自动共识不足以安全选主 | PASS | `LeadPolicy::validate` 拒绝 voters<3 + auto；svc 启动即校验 |

## 3. 登记项（诚实声明）

1. **E5/HIL**：真实调度主站/联邦对端不在本环境——mTLS 传输层的 E3（进程间真实 TLS）为当前可复现上限；E5 随现场接线（登记 BLOCKED）。
2. **目标侧（seL4）装配**：svc-federation 是区域/云协调层 host 服务（部署原则 §1 三层之一，不属 qemu-prod seL4 站内闭包）；站内联邦客户端由站点计算扩展层（Linux）承载，本站未建 target 侧联邦组件——站内路径以 `negotiation` 库在站侧复用（BLE 版后续按现场需求装配）。
3. **协调器 HA/分片**：本卷只落实"两节点人工 failover"策略 + 崩溃恢复（journal+epoch）；3 voter quorum/fencing/分片在 v0.314.48（WP-7.2）。
4. **价格曲线 ref 语义**：offer 携带 `price_curve_ref`（BlobRef），svc 演示不计价（price_rank 由市场层注入的接口已留；solver 按 rank 确定性排序）。
5. **站点自动接受策略**：svc 站点模式默认对包络内目标自动接受（保守演示策略）；拒绝率/人工审批等现场策略是配置面，不在本卷协议层。

## 4. 顺手修复

- 联邦 crate 全量 clippy 清理（needless_range_loop/redundant closure/collapsible match/type_complexity + 测试文件 `disallowed_macros` 惯例豁免）。
- `KeyRegistry::MAX_KEYS` 64→2048（协调器需容纳 100 站 + 轮换宽限；原值仅够单站点）。
- `tamper_rejected` 测试分层的安全属性修正：body 篡改→digest 拒；头/签名篡改→验签假。
- 协调器 commitment 校验加时钟偏斜容忍（`CLOCK_SKEW_TOLERANCE_MS`），跨节点时钟差不再误拒绝。
- svc 站点恢复语义：restore 后不重复发布 offer（revision 冲突修复）。