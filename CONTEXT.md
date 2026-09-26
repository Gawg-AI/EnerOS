# EnerOS 领域上下文（CONTEXT.md）

> 单上下文布局：本文档 + `docs/adr/`（MASTER §0）。
> 本文是领域词汇的唯一事实源；与蓝图/代码冲突时，以本文 + 最新 ADR 为准。
> 词汇归属裁定见 ADR-0029；点模型见 ADR-0027；协议 seam 见 ADR-0026；驱动 seam 见 ADR-0028。

## 1. 核心实体

| 术语 | 定义 | 唯一来源 | 别名（禁止混用） |
|---|---|---|---|
| `DataPoint` | 统一数据点：point_id(u32) + device_id(u16) + value + quality + timestamp_ms + source + unit | `upa-model`（ADR-0027） | protocol-sdk 旧 `DataPoint`（已删除） |
| `PointValue` | 点值 6 变体：Float/Int/Bool/Enum/String/Null | `upa-model` | protocol-sdk 8 变体（已删除） |
| `PointQuality` | 数据品质 7 位标志：valid/invalid/questionable/substituted/overflow/blocked/outdated | `upa-model` | `Quality`（protocol-sdk 3 态）、`QualityFlag`（telemetry-model，保留为转换层） |
| `PointId` | 点唯一标识，u32 | `upa-model` | `HString<64>` 点 ID（已删除） |
| `DeviceId` | 设备标识，u16 | `upa-model` | — |
| `DataSource` | 采集来源：ModbusRtu/ModbusTcp/Iec104/Can/Internal/Manual | `upa-model` | — |
| `ProtocolAdapter` | 协议适配器 trait（生命周期 + 点访问），唯一协议 seam | `protocol-abstract`（ADR-0026） | driver-sdk/protocol-sdk 插件 trait（宿主内接口） |
| `PointAccess` | 统一点读写 trait（唯一协议 seam 底座） | `protocol-abstract` | device-agent 旧 `DeviceAdapter`（已删除） |
| `AuthorizedWriter` | 协议侧写接缝：只接受 `AuthorizedCommand` 的写能力，与 `PointAccess`（读）信任分级（蓝图/24 D1 已落地：写从 PointAccess 分离，读写信任分级） | `protocol-abstract`（蓝图/24，ADR-0026 §7） | 裸 `write_point` 无授权写（已移除，删除闸门见蓝图/24 R2） |
| `AuthorizedCommand` | 控制链裁决产物：device_id(u16)/point_id(u32)/command/actor_kind/issuer/lease/fencing 等；唯一构造路径 `ControlGateway::authorize` | `telemetry-model::command`（蓝图/24 D4 下移，control-gateway re-export） | 信封 wire 上的裸 u32 字段（仅 wire 编解码内部） |
| `ProtocolPointMapping` | 点 ID ↔ 协议地址映射 + 工程量变换（scale/offset） | `protocol-abstract` | — |
| `DeviceDriver` | 用户态设备驱动 trait（生命周期/状态/健康），唯一驱动 seam | `drivers/framework`（ADR-0028） | `DriverPlugin`（driver-sdk，已删除） |
| `MockAdapter`/`MockDriver` | 契约测试用 mock（seam 层共享套件成员） | `protocol-abstract`/`framework` | — |
| `ObservationEvent` | 不可变观测事实（V1 wire 128B）：tenant/site/source/epoch/sequence/时间/资产/单位/schema/品质/完整性/映射世代 + event_id（SHA-256 前 16B，去重键） | `telemetry-model::event`（蓝图 61，ADR-0046） | `TelemetrySample`（旧 58B 兼容轨，.30 切换）、`DataPoint`（配置/状态模型，不替换） |
| `RawObservation` | adapter 内部原始观测（无 epoch 字段——身份由注册表分配；跨出 adapter 必须成为 ObservationEvent） | `ingestion-core`（蓝图 61，ADR-0046） | 裸 TelemetrySample 跨 binary（禁止） |
| `SourceRegistry` | 来源注册深模块：SourceDescriptor/register/verify/open_source；未注册事件只进 quarantine（控制 evaluator 零消费） | `ingestion-core`（蓝图 61，ADR-0046） | 评估器白名单数字（allowed_sources 硬编码，已废） |
| `UnitCatalog` | 封闭数字单位码表（0=未知；wire 层 unit_code 唯一事实） | `telemetry-model::unit`（蓝图 61） | 裸 f64 跨域传递（禁止） |
| `CandidatePlan` | 候选计划（全时域分配/状态/摘要；非成功 solve status 不可构造） | `decision-contract`（蓝图 61，ADR-0048） | `ProposedControlPlan`（dual-brain 内部决策表示；旧 70B wire 读臂已于 v0.329.0 删除；legacy facade `submit` 入口已于 2026-09-24 C2 收口退役——类型经 unbound adapter 转 typed 候选，仍在 energy-market-agent → dispatch.submit_typed 链上） |
| `CertifiedPlan` | 独立认证证书（seal 收口构造；外部只有 decode+verify） | `decision-contract`（蓝图 61，ADR-0048） | SafetyValidator bool（不得当证书） |
| `CommandIntent`/`CommandBatch` | 编译后命令意图/有序批（批构造必须持证书+资源视图） | `decision-contract`（蓝图 61，ADR-0048） | 直接设备写（禁止） |
| `SettleTrigger` | 结算触发源枚举（SafetyReject/BudgetRate/BudgetEnergy/…/RecoveredApplied 恢复对账终局）：`settle(trigger)` 纯函数按表裁决唯一结算出口（恢复路径终局写序亦收表——蓝图 75 WP-75.1.1；states 死参数已删 WP-75.1.5） | `control-dispatch::settle`（表）+ `control-dispatch::engine`（编排面 SettleDispatch；C1 拆分后表自 engine.rs 迁出） | 结算逻辑散在调用点（已收表；恢复路径手写写序已收表） |
| `SERVICE_WIRING` | 服务装配接线表：name/heartbeat/net_slot/stack_pages/driven（周期+相位）/driven_none_reason/miss_limit（心跳缺失容忍轮次覆盖，None=缺省 150；svc-telemetry=3000 实弹校准——蓝图 75-03 WP-75.3.1 入表）——装配属性单源，表序即驱动编排序；root 直连名单=service.rs DirectIndexes 索引束（10 槽位，75-03 WP-75.3.0） | `root-task::wiring`（蓝图 62，D-1） | spawn 期按名手抄表（已删）；heartbeat_miss_limit 按名手抄 match（已收表投影）；完备性门：每个被 spawn 服务必须 ∈ 本表 ∪ EXPLICIT_UNWIRED（63.2） |
| `IngestionStage` | 采集交付半环单源模块：run_acq_round（tick→checkpoint→drain→ACQ 行→分批上行）+ TelemetryUplink（分批/chunked/epoch 内化）+ ACQ 词形单源 | `protocol-services::ingestion_stage`（蓝图 62 S2a） | bins 本地 collect_and_report/report_obs 双抄（已删） |
| `ChunkedEndpoint`/`ChunkedSession` | chunked Call 端点（epoch 私有，递增仅 call 内）；ChunkedSession=共享 epoch 会话（每次 call 后无条件写回共享计数器，成败无关——root 三步手工协议的结构化收拢） | `agent-sdk::endpoint`（蓝图 62 S4 / 63.2 WP-63.2.1） | 调用方 `*epoch = endpoint.epoch()` 手写回（已删，结构性不可漏） |
| `SignerKeyring` | SM2 密钥环 + 类型化验签原语（verify_signature→VerifiedSignature，Ok(false) 陷阱不外泄）+ 修订水位（check_revision 非变异判定 / commit_revision 推进——判据单源，消费方零手写比较） | `signer-keyring`（蓝图 62.3 / 63.3.1） | 消费方手写 `revision <= seen`（已删）；裸 sm2_verify 直调（逐处收口/豁免登记） |
| `WatermarkTracker` | **序列水位**（ingestion-core）：同源 (epoch, sequence) 两维，check 非变异 + advance 分离——sink 准入守卫判据唯一实现 | `ingestion-core`（蓝图 62.3 W3） | snapshot-service 旧同名 tracker（63.3.1 消歧改名 `SnapshotWatermark`）；手写水位比较（已删） |
| `SnapshotWatermark` | **快照构建水位**（snapshot-service）：epoch/seq + event_time/ingest_time 四维，observe 合体——快照域分层判据（⊃ 序列水位 + 两时间维），与 ingestion-core WatermarkTracker 不合并 | `snapshot-service::watermark`（蓝图 63.3.1 消歧定名） | 与 WatermarkTracker 同名混用（禁止——分层关系见 watermark.rs 模块 doc） |
| `EventStore` | **同名异物消歧**（蓝图 66 WP-66.6.3）：`cloud-sync::event_store::EventStore` = **跨域同步事件缓存**（生产在用，svc-cloud-sync 链）≠ `runtime/event-store` 的 `EventStore` = **分区事件日志**（上游断链 BLOCKED——机制深、生产零消费是裁决内事实）。两词不混用；裸写 `EventStore` 须以 crate 路径限定 | `cloud-sync::event_store` / `runtime/event-store`（蓝图 66 消歧登记） | 同名裸引用（禁止——须路径限定） |
| `eneros-protocol-keys` | **协议配置键表单源**：12 协议已知键 const 数组 + `by_config_id` 映射（零依赖叶 crate，const-only）——config-bundle 编译面 `known_keys` 与插件面 `KNOWN_KEYS` 双面引用消费 | `protocols/protocol-keys`（蓝图 64.1） | 双面镜像手抄（已删，13 处字面 → 1 处）；双面引用锁 22 道 + 叶内容字面锁；cj188 编译面显式 None（行为新增边界） |
| 计划绑定代次（catalog-generation plan binding）vs 准入同代（admission generation） | **两判据有意分层，不合并单源化**（R13-R2 裁决）：计划绑定代次 = 消费侧计划保留判据（在途/已接纳绑定的 `catalog_version` 与当前可信输入一致即保留——快照推进不丢计划，消费时重锚定当前代）；准入同代 = 入口侧投递判据（候选 snapshot_id + catalog_version + horizon_start + objective_digest 与当前可信输入逐字段一致，fail-closed 第二层） | `eneros-control-services::remote_brain::catalog_generation_matches` / `eneros-control-dispatch::engine::admit_trusted_input` | 「同集合判据」「答案必须一致」旧宣称（已废止 2026-09-20，R13-R2 订正）；把两判据收敛为共享函数（禁止） |
| `ServiceSpawnSession`（服务装配会话） | Deepening-v1 §D：`configure_service` 的**不变参束**（11 命名字段：module/entry/ctx/types/rights/fault_ep/ntfn_badge/eipc_mask/heartbeat_slot/wall_epoch + 两个 `&mut Option<u64>` net 页框出借）——装配一次成型，`configure_service(session, svc, elf)` 返回 `ConfiguredService`（旧 8 元组的命名字段形态，逐位对应）；函数体逐字迁移零删改 | `root-task::spawn`（仅 `cfg(target_os = "none")`，host 不可直测） | 14 位置参手递（已收，五个相邻 u64 错位在命名字段下不可表示）；把移交面收为 `Handover` 和类型（禁止——§11.4 证伪：X7 正交写会静默丢失） |
| `HoldRegistry`（快照钉住账目） | 钉住生命周期账目单源深 module：持有代次（`admit` 唯一递增点）/已释放事实（按当代代次判幂等）/待重试释放队列（收尾 pass `begin_retry_cycle`→`end_retry_cycle`，本 pass 新失败在前、重试仍失败在后）/终态行一次性标记/钉面失败计数（`pin_failures` 只读观测，写入点唯一 `note_pin_failure`）。有界淘汰统一为私有 `evict_bounded`＋入队闸门（原六处四种惯用法 → 单点），F70-04 代次安全从注释维系升级为结构维系 | `control-dispatch::engine::hold_registry`（Deepening-v1 §A） | engine 裸字段 `held_pin_failures`（已去 pub，零读者缺口经 `pin_failures()` 兑现）；把存活引用计数搬进 registry（禁止——事实源是持久计划，计数归 engine，R-A2） |
| `DeliveryContext`（计划投递上下文） | 计划级投递标签（`STATION_PLAN_TARGET_LABEL` 单源）+ 逐投递清算超时（`HELD_DELIVERY_SETTLE_MS` 单源，≠saga 批 deadline）；随计划同生命周期流动，与 `PlanBinding`（计划派生绑定束：plan_id/input_digest/wire_sha/ttl/legacy_single——[u8;32] 同型互换防御）配对作 `EngineSagaExecutor` 构造面（R13-B，11 参→5 参）；持久化=登记 R13-G 未实施。**R14-A（2026-09-24）**：两单源常量构造点收敛 `DeliveryContext::default()`（saga_exec 唯一落点）；typed/trusted 入口的 `target_device`/`deadline_ms` 幻影形参随 trait 收窄退役（引擎内部自派生，与 submit_intent 惯例对齐）；值域 host 锁 `station_delivery_identity_value_frozen`（"pcs" 观察者期望值从未接断言——变异实锤后补） | `control-dispatch::engine`（蓝图 72 修复批B / R14-A） | 三处手抄 "pcs"/5_000（已删）；调用点绕过 Default 手拼 DeliveryContext 字面（禁止）；把 settle 超时混称「批 deadline」 |
| `svc_wire`（控制服务信封） | bin 族控制服务信封单源：magic(4)‖cmd(1)‖payload 判别 `parse_envelope` + 三错误词形常量 `ERR_PROTO`/`ERR_LEN`/`ERR_CMD`（b"ERR:proto"/b"ERR:len"/b"ERR:cmd"——请求方可 grep 的 wire 形态）。R14-E 收编 8 处 bin 手抄（market/ledger/agent-bus/cloud-sync/config/observability/ota 七 bin + ota_service 库内）；market/ledger/cloud-sync 载荷 codec 已入 lib mkt_wire/ldg_wire/cld_wire（WP-75.2.6；其余服务 module 就地持有；obs_wire/ota_wire 等 agent-sdk 跨 binary 单源不在此层）；MGT1 不属本信封族（帧文法单源在 `mgmt-api::mgt1_frame`，蓝图 76 卷二），但错误词形引用本模块常量 | `business-services::svc_wire`（R14-E，2026-09-24） | bin 内手写信封谓词（已删）；绕过常量手写三错误词形字面（已删，词形冻结测锁死） |
| `svc_panic!`（EL0 panic 声化单源） | 全族 EL0 bin panic handler 单源宏，宿主 `eneros-runtime`（五族公共依赖，蓝图 76 卷一自 business-services 下沉）：bin crate 根一行 `eneros_runtime::svc_panic!("svc-x");` 收编（cfg 门 = target 展开、host 零展开 ⇒ 展开点仍在 bin 根，链接期语义不变）；语义 = ADR-026 §1 的**打印下限**（println 死因 + spin_loop 挂起，不复位整机），词形 `[<svc>] PANIC: {info}` 冻结（REG-70.7「无声 spin 掩盖死因」排障史 + vol5/6/7 PANIC-SCAN `none_of` 覆盖）。30 bin 在宏内（business 8 + control 5 + svc-storage 1 + protocol 11 + drv 5）；静默 handler 回流由 `runtime/src/panic_guard.rs` 五判据锁死（30 bin 三断言 + 宏本体两半边 + 四族目录完全划分 + 豁免形态分立 + 自咬锁）。豁免：svc-hang 挂起靶保静默、A 族五 bin（hello/fault/hog/pi/pie）保 `svc_panic_hook` 审计链（全文语义 = 审计+打印+挂起）、panic/oom-probe 靶、root-task 自分层 | `eneros-runtime::svc_panic!`（蓝图 75 卷二 WP-75.2.3 立宏 → 蓝图 76 卷一 WP-76.1.1 下沉 + 扩族，2026-09-26） | 任一表内 bin 静默 spin panic handler（已删）；bin 内自有 `#[panic_handler]` 定义或手抄 `fn panic(`（禁止——单源宏内）；把豁免族好心收编进宏（禁止——判据④反向锁死） |
| `blk_wire`（块命令 body 文法单源） | 块命令帧「哪一段在哪几个字节」的唯一书写点（蓝图 76 卷二 WP-76.2.2~4）：数据面 `pack_read/parse_read/parse_write/pack_write_prefix`（**字节偏移**）、`pack_health/parse_health`（13B）、boot 面 `pack_boot_read/parse_boot_read/parse_boot_write/pack_boot_trial/pack_boot_confirm/parse_*/pack_seq_resp/parse_seq_resp`（**扇区 LBA**）+ `body_of`（信封→body 唯一桥：子码占 `payload[0]`）+ `LenErr::Short` 窄错误面。**两单位并存是 wire 史实不是设计**（模块 doc 首行），禁第三单位。错误→status 映射与域规则（count==0/超上限/空数据段）**留调用方**：服务端 ST_INVALID 与 host 替身 Err(()) 各是其测试族契约（统一属登记不修）。消费面：服务端 block_cmd 两 handler + agent-sdk BlockIo 构帧/解帧 + svc-ota boot seq 解包 + 4 host 替身（business/agent-sdk SimStorage、control-services 两 FakeStorage）——收编前 1 服务端+1 客户端+4 替身各手抄一遍、替身无长度门（弱于生产端）。零 wire 字节移动，字节裁判 = `block_cmd` golden 族（WP-76.2.1，全子码请求+响应逐字节） | `drv-svc::blk_wire`（蓝图 76 卷二，2026-09-27） | 服务端/客户端/替身内手抄 `payload[1..9]`/`payload[9..13]` 或局部 `OFF_BYTES`（已删）；把 status 语义焊进单源模块（禁止——两端惯例不同）；新子码引入第三单位（禁止） |
| `mgt1_frame`（MGT1 帧文法单源） | 管理面 MGT1 帧「哪一段在哪几个字节」的唯一书写点（蓝图 76 卷二 WP-76.2.5~6），no_std 叶 `eneros-mgmt-api` 内 alloc-free 表达：常量面板（`MGT1_HEADER_LEN`/`SESSION_V1_LEN`/`SESSION_V2_LEN`/`TOKEN_LEN`/`SIG_FIELD_LEN`/`V1_MIN_REQ_LEN`/`V2_MIN_REQ_LEN`）+ 段偏移 const（session 五段 / token 七段 / 签名字段）+ `classify_session`（`SessionShape` 三臂互斥完备，70.5 复查 P0 短尾帧回归锁随迁）+ 借用访问器（`v2_tail`/`request_id`/`action_len`/`action_and_arg`/`v1_session`/`token_view`）+ 构帧器（`build_token`/`build_sig_field`/`build_v1_frame`/`build_v2_frame`/`build_truncated_v2_frame`）。**分工线**：验签（`verify_v2_tail`，ECDSA+时钟+alloc）与 `AuthenticatedSession` 留 business-services，`mgmt_principal` 对文法面 `pub use` 保既有公共名。**撞名纪律**：`mgmt-api::TOKEN_LEN`（auth.rs SM2 = 113B）与本模块 `TOKEN_LEN`（64B）同名不同物，故本模块**不做 crate 根 glob 再导出**（消费方走 `mgt1_frame::` 路径）；签名 token 字段序与 `tools/run70-vol6-gate.sh` 内 Python 签名器逐段同构（门在门外签 SHA-256(token)，字段挪位即门红，不属代码可测面） | `eneros-mgmt-api::mgt1_frame`（蓝图 76 卷二，2026-09-27） | 消费 bin（svc-mgmt 两臂）与 root 探针（mgt_probe）各拼一遍偏移算术（收编前实况，已删）；`5 + SESSION_LEN + V2_TAIL_LEN` 型裸算与 `[16..20]`/`[24..40]` 型手切（禁止）；把验签拉进 no_std 叶（禁止——crypto+时钟+alloc 是 business 的事） |
| `mgmt_relay`（svc-mgmt 判据下沉库） | svc-mgmt TargetExecutor 纯判据面：ConfigSet S/C/A 中继响应归类（`classify_stage_resp`——**先判 ERR** 判据：svc-config 拒绝以 `ERR:` 文本承载，非空即成功会把拒绝记成完成副作用写进幂等账本（2026-09-24 已修 bug 的 host 锁）；`classify_commit_resp`/`classify_activate_resp` 同族）+ 重启在途对账终态三态（`parse_restart_detail`/`classify_restart_observation`：gen>base=Completed/超时=Failed/其余在途=None）+ 快照分节格式化（`format_snapshot_section` sm3 分节行 golden + 五分节 body 构造，缺项词形锁）。**EIPC 拨号/root_call/persist CAS 留 bin**（跨进程面不下沉） | `business-services::mgmt_relay`（蓝图 75 卷二 WP-75.2.8，2026-09-26） | bin 内手抄响应归类/词形（已删）；把拨号面搬进 lib（禁止——root 交互留 bin） |
| `InstallRanges`（安装区间） | boot A/B 安全面区间具名承载：`inactive_start_lba`/`inactive_lbas`（非运行 BOOT 槽 = 可写安装目标；身份缺失 0/0 = 写面整体关闭）+ `bootctrl_start_lba`/`bootctrl_lbas`（BOOTCTRL 事务分区）。**单位契约**：全部字段 = 512B 扇区 LBA / 扇区数；wire 40B（4×u64 LE + 尾部 8B 保留恒零 = `BLK_BOOT_GUARD` 响应）打包/解包单源 `pack_le`/`unpack_le`；boot 面读写一律走 `InstallBackend::read_sectors`/`write_sectors`，数据面 `BlockBackend` 的 `offset_bytes` = 字节偏移——同名异单位分裂终结（蓝图 73 批A C9） | `drv-svc::boot_guard`（蓝图 73 批A） | 裸 `(u64,u64,u64,u64)` 四元组（已删——换位曾编译干净；窄域签名禁令锁死）；boot 面对 `dyn InstallBackend` 的裸名 `.read(`/`.write(`（禁止——一律具名单位方法） |
| `BootSignatureProvider` | **验签唯一入口**：域校验（算法/签名长度/密钥域/版本下限/dev-key）先行 + provider 密码学验证（算法域自证 + Ed25519 verify_strict + sha256 消息/摘要一致性——74 复查轮 FR-6：pub API 直调不再依赖调用方先跑域门）；唯一实现 `Ed25519SignatureProvider` 在 boot-protocol（WP-74.3.1 迁入，boot-manager pub use 保路径）——boot-manager 与 installer 两 UEFI bin 经同一 trait 共享一份密码学路径 | `boot-protocol::signature`（蓝图 74 卷三，2026-09-25） | 消费方裸 `VerifyingKey::from_bytes` 直验（已删——installer 修复前即此形态，dev-key 面缺失；grep 判据=boot 族唯一 from_bytes 在 provider 本体）；provider 外手写域谓词（禁止） |
| `jsonl-wire` | **JSONL 线码单源叶**：`escape_json_into`（`"`/`\`+控制字符 `\u00xx`，JSONL 线路写侧唯一实现）+ `Scan` 骨架（skip_ws/peek/bump/expect/read_string/read_number_text/skip_value 读侧唯一实现）+ `hstr` 截断转换 + `JsonlError::Syntax`（各 crate 经 `From` 映射自有错误词形，变体名冻结）——logging/metrics file_store 双胞胎 Scan 收编（领域数值与形状留各 crate） | `eneros-jsonl-wire`（蓝图 74 卷三 WP-74.3.3，2026-09-25） | 消费 crate 内手写转义/扫描骨架（已删）；security/recovery（三处，含 `truncate_heapless` 换名）/vpp-scenario/pentest/dashboard `esc` 的 hstr/HString 别位（异族，登记不动——74 复查轮 FR-10 清点订正） |
| `LogLevel::from_config_level` | config min_level 码表（0=ERROR..4=TRACE）→ logging 枚举**唯一换算点**；非双射（config TRACE(4) 无对位 → clamp Debug），非法值 → None fail-closed；Phase 2 接线/休眠/退役被裁决门 REG-74-M-APPLY 阻断 | `eneros-logging::logging`（蓝图 74 卷三 WP-74.3.4，2026-09-25） | 消费点手写两表比较或换算（禁止——schema.rs 注释互指）；在死岛之间盲统一接线（未裁决前禁止） |

## 2. 市场域词汇

| 术语 | 定义 | 唯一来源 | 现状 |
|---|---|---|---|
| `TradeDir` | 交易方向：Buy/Sell | `market-types`（ADR-0029） | spot-market/realtime-market 旧定义收敛为 re-export |
| `TimeSlot` | 时间段（UTC 秒，start/end，end>start） | `market-types` | spot-market 旧定义收敛 |
| `Bid` | 报价单：bid_id + market + submitter + segments + submit_at | `market-types` | spot-market、energy-market-agent 旧定义收敛 |
| `YuanPerKwh` | 价格 newtype（元/kWh） | `market-types` | spot-market 裸 f64（已替换） |
| `YuanPerMwh` | 价格 newtype（元/MWh） | `market-types` | realtime-market 裸 f64（已替换） |
| `BidSegment` | 报价段：period + volume_kwh + price + direction | `market-types` | spot-market 旧定义收敛 |
| `BidAck` | 报价回执 | `market-types` | spot-market/ancillary 旧定义收敛 |
| `Period` | 时段（峰/平/谷），进入 `PricePoint.period`/`ResourceBid.period` 跨 crate 契约 | `market-types`（ADR-0029，W5.1 蓝图23） | market_feed 旧定义已收敛为 re-export |
| `aggregator` | 仅用于"聚合计算"（见 §3 归属） | 各领域保留 | 见 ADR-0029 |

## 3. aggregator 归属边界（ADR-0029）

| 位置 | 语义 | 命名 |
|---|---|---|
| `coordinator/src/vpp_aggregator.rs` | VPP 资源聚合与调度（VppResource/AggregatedDispatch） | `VppAggregator` 语义 |
| `cloud-coordinator/src/aggregator.rs` | 跨域数据汇聚（DataSource→DataAggregator→DataSink） | `DataAggregator` |
| `dr/src/aggregator.rs` | 需求响应负荷聚合（aggregate_capacity） | `DrAggregator` |
| `twin-global` | 多域联合仿真/模型融合（MultiDomainSimulator/ModelFusion） | 不用 aggregator 字样（用"联合"） |

同义词纪律：`aggregator`=聚合计算；`coordinator`=编排/协调；`federation`=跨信任域协作；`fusion`=模型/数据融合。
水位消歧：**watermark 序列水位（ingestion-core WatermarkTracker，(epoch,seq) 准入判据）≠ event-store `WatermarkReport` 容量水位（分区容量观测报告）**——两词不混用（蓝图 63.3 WP-63.3.4）。

## 4. 插件执行模型（ADR-0025）

| 术语 | 定义 |
|---|---|
| 插件 | 签名静态 ELF PIE → 受限 EL0 进程 → 版本化 IPC wire ABI（Agent/驱动/协议统一） |
| IPC wire | 二进制消息格式：固定头 + 有界长度 + CRC32（对齐 agent-sdk `wire.rs` EIPC） |
| cross-binary ABI | 禁止：`*mut dyn`、Rust trait object vtable、String/Vec/enum 直传 |
| `InteropLevel::SpecBlocked` | 互操作诚实分级最低档：规约文本缺失——codec 字节层未实现（蓝图 48；owner 提供规约并落地权威 KAT 后方可升 CodecReady） | `protocol-abstract::plugin::InteropLevel` |

## 5. 已删除/已废弃词汇（不再使用）

- `DeviceAdapter`（device-agent 字符串点名旁路，ARCH-P02 删除）
- `DriverPlugin`/`ProtocolPlugin` C ABI 注册宏（ADR-0025 删除）
- `register_driver!`/`register_protocol!`（同上）
- protocol-sdk 旧点类型（`HString` 点 ID、8 变体 `PointValue`、3 态 `Quality`，ADR-0027）
- `eneros-driver-sdk` crate（ADR-0028）
