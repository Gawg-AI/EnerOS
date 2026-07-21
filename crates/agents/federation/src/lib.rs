#![cfg_attr(not(test), no_std)]

//! # EnerOS 联邦组件（v0.97.0 联邦发现 + v0.98.0 跨域通信通道 + v0.98.1 纵向加密认证 + v0.99.0 联邦共识 PBFT 变体 + v0.100.0 资源争抢竞价 + v0.101.0 断网处理与孤岛模式）
//!
//! Edge Box 自动加入联邦：证书验证 + 注册 + 广播，心跳保活，超时剔除；
//! Edge Coordinator 间跨域加密通信通道（mTLS 双向认证 + 国密 SM2/SM3/SM4）防窃听篡改，
//! 为 v0.99.0 联邦共识提供安全通道；纵向加密认证（36 号文合规）补齐调度主站合规接入：
//! SM2 IKE 密钥协商 + SM4 密文隧道 + 重放保护；联邦共识协议（PBFT 变体）提供跨域决策
//! 一致性，容忍 ≤ f 个拜占庭节点（3f+1 总节点），为 v0.100.0 资源争抢竞价提供协议一致性
//! 基础。全项目 no_std 合规，仅 core/alloc + eneros-crypto（既有 workspace crate，
//! 零新增第三方依赖）。
//!
//! ## 功能概览
//!
//! - [`membership`]（v0.97.0）：节点角色（[`NodeRole`]）、证书引用（[`CertRef`]，FNV-1a 64
//!   确定性指纹）、成员信息（[`MemberInfo`]）、加入请求（[`JoinRequest`]）、
//!   成员注册表（[`MemberRegistry`]，BTreeMap 按 node_id 升序）。
//! - [`discovery`]（v0.97.0）：证书验证 trait（[`CertVerifier`]）、在线广播 trait
//!   （[`PresenceBus`]）、发现协调器（[`FederationDiscovery`]）、
//!   错误类型（[`FedError`]）与 Mock 实现（[`MockCertVerifier`] /
//!   [`MockPresenceBus`]）。
//! - [`channel`]（v0.98.0）：TLS 配置（[`TlsConfig`] 纯数据 + 非空校验）、端点
//!   （[`Endpoint`]）、安全传输抽象（[`SecureTransport`]）与 Mock
//!   （[`MockSecureTransport`]）、跨域通道（[`FederationChannel`]：确定性握手 +
//!   SM4-GCM 加密通话 + 4 计数器）、错误类型（[`ChannelError`] 6 变体）。
//! - `tunnel`（v0.98.1）：SM2 IKE 密钥协商（PMS 加密 + 签名 + SPI 提议）、
//!   `TunnelKeys`（SM3 域分离派生）、`VerticalEncryptTunnel`（SM4-CBC 密文隧道 +
//!   SM3-HMAC 认证 + 64 位重放窗口 + 原位换钥）、`VerticalEncryptDevice` /
//!   `MockVerticalEncryptDevice`、`DispatchToken` / `AuthResult` 调度令牌验签、
//!   `TunnelManager` 多隧道管理与 4 计数器、`EncryptError` 7 变体。
//!
//! ## v0.97.0 D1~D12 偏差表（相对教科书式联邦发现实现的裁剪决策）
//!
//! | 编号 | 偏差 | 说明 |
//! |------|------|------|
//! | D1 | 标识符用 `u64` 而非 `String` | 项目惯例：no_std 嵌入式环境避免堆分配字符串 |
//! | D2 | 证书仅用 FNV-1a 64 指纹标识，无密码学语义 | 真实证书链验证后置（v0.98.1 纵向加密），本版只做确定性标识 |
//! | D3 | 时间通过 `now_ms: u64` 参数注入 | 不读系统时钟，便于测试与跨平台移植 |
//! | D4 | 集合用 `BTreeMap`/`Vec` 而非 `HashMap` | 项目惯例：BTreeMap 遍历天然按 node_id 升序，剔除结果确定有序 |
//! | D5 | trait 为同步阻塞式，无 `async` | 项目硬规则：禁止 async |
//! | D6 | trait 对象不带 `Send + Sync` 约束 | 单分区单线程模型，调度由上层 Agent Runtime 保证 |
//! | D7 | 广播失败时成员仍保留在注册表 | 注册与广播解耦：失败计入 `reject_count`，可经 `broadcast_presence` 重试 |
//! | D8 | 超时阈值固定为 `heartbeat_interval_ms * 3` | 三次心跳未达即剔除，与业界 gossip/lease 惯例一致 |
//! | D9 | 超时判定为严格大于（`>`），边界存活 | `now - last_seen == timeout` 时成员保留，避免边界抖动误剔 |
//! | D10 | 时差计算用 `saturating_sub` 防下溢 | `now_ms < last_seen`（时钟回拨）时不误剔 |
//! | D11 | 零依赖，仅 core/alloc | 交叉编译 aarch64-unknown-none 友好，SBOM 无新增条目 |
//! | D12 | 依赖注入用 `Box<dyn Trait>` 多态 | 生产可替换真实证书验证/网络广播实现，测试用 Mock |
//!
//! ## v0.98.0 D1~D12 偏差表（精简版，详见 channel 模块文档）
//!
//! | 编号 | 偏差 |
//! |------|------|
//! | D1 | 既有 crate 单模块 `channel.rs`（tls/grpc_service 语义并入，不过度拆分） |
//! | D2 | `node_id: u64` / `connect(u64, SocketAddr)`，无堆字符串标识 |
//! | D3 | sync 方法 + sync `SecureTransport`（no_std 禁 async；tonic 无法交叉编译） |
//! | D4 | `TlsConfig` 纯数据 + `validate()` 非空校验（真实 TLS 后置集成） |
//! | D5 | 复用 v0.97.0 `CertVerifier` 验证对端证书（防重复造轮子） |
//! | D6 | 确定性握手 + SM3 域分离会话密钥派生（双方可独立复算） |
//! | D7 | SM4-GCM 认证加密替代 TLS record 层，nonce 逐 seq 唯一 |
//! | D8 | `use_sm` 纯配置占位，仅国密路径，无分支行为差异 |
//! | D9 | 零新增第三方依赖，仅 path 依赖 eneros-crypto |
//! | D10 | `ChannelError` 6 变体最小完备 |
//! | D11 | crate 内嵌 `#[cfg(test)]` 测试替代 tests/mtls.rs |
//! | D12 | 4 个 pub 计数器替代外部连接状态 metric |
//!
//! ## v0.98.1 E1~E12 偏差表（精简版，详见 tunnel 模块文档）
//!
//! | 编号 | 偏差 |
//! |------|------|
//! | E1 | 同 crate `tunnel.rs` 单模块（与 v0.98.0 同属联邦安全通道族） |
//! | E2 | `VerticalEncryptDevice` sync trait + Mock 回环（真实卡驱动现场适配注入） |
//! | E3 | 最小两方 SM2 IKE：PMS + SM2 加密/签名 + SPI 提议，完整 IKE 状态机后置 |
//! | E4 | 证书 opaque bytes + 复用 `CertVerifier`，不新造证书类型 |
//! | E5 | `Vec<u8>` 缓冲（Agent Runtime 有用户堆，alloc 可用） |
//! | E6 | 随机 IV 由注入 `CsRng` 生成（CBC 可预测 IV 不安全） |
//! | E7 | `EncryptError` 7 变体最小完备（补 TagMismatch/InvalidFrame/UnknownTunnel） |
//! | E8 | `DispatchToken`/`AuthResult` 结构定义，过期判定先于验签 |
//! | E9 | u64 seq + 64-bit 滑动位图重放窗口（IPsec 惯例） |
//! | E10 | Mock 双端回环互通测试替代真实装置互通（现场验收项） |
//! | E11 | eneros-crypto 纯增量 `sm3/hmac.rs`（通用密码原语归属 crypto crate） |
//! | E12 | `rotate` 原位换钥 + 重放窗口重置；`TunnelManager` 多隧道 + 4 计数器 |
//!
//! ## v0.99.0 联邦共识协议（PBFT 变体）
//!
//! ★ 蓝图 PBFT 降级说明（评审 P2）：PBFT 为可选高安全共识模式；默认部署用主从仲裁
//! （v0.92.0 DomainArbiter 既有），仅跨信任域 Byzantine 容错场景经配置启用 PBFT。
//!
//! - [`consensus`]：共识数据结构与总线抽象——[`NodeId`] / [`ConsensusState`] /
//!   [`MsgType`] / [`PbftMessage`] / [`LogEntry`] / [`ConsensusResult`] /
//!   [`ConsensusError`]（7 变体）/ [`ConsensusBus`] trait / [`MockConsensusBus`]
//!   （隔离与故障注入）/ [`ConsensusEngine`]（核心状态机：new/submit/poll/handle_message
//!   + 4 计数器 + 延迟观测；**禁 Debug**，含 Sm2KeyPair 私钥保护）。
//! - [`pbft`]：PBFT 三阶段——[`f`] / [`quorum`] / [`primary_of`] 数学，
//!   [`sign_message`] / [`verify_message`]（SM2 域分离签名），
//!   PrePrepare/Prepare/Commit 处理（2f+1 法定人数，主节点 PrePrepare 计入 prepare 票）。
//! - [`view_change`]：视图切换——超时检测（指数退避 `timeout_ms << min(连续 vc, 3)`）、
//!   ViewChange 投票达 quorum 自主 enter_view（无独立 NewView 消息）、
//!   新主对尾部未提交日志重发 PrePrepare 恢复共识。
//!
//! ## v0.99.0 D1~D12 偏差表（相对蓝图 v0.99.0 原文）
//!
//! | 编号 | 偏差 | 说明 |
//! |------|------|------|
//! | D1 | crate 路径 `crates/federation/src/{consensus,pbft,view_change}.rs` | 既有 `crates/agents/federation/src/` 追加同名 3 模块（项目 §2.3.1 硬规则：crate 必须按子系统分组；v0.97.0/v0.98.0 同 crate 先例） |
//! | D2 | `NodeId` 未定义类型 | `pub type NodeId = u64`（无堆字符串标识，v0.97.0 D1 惯例） |
//! | D3 | `pub async fn submit / handle_message` + `broadcast().await` / `wait_for().await` | sync 方法 + `poll()` 驱动（no_std 硬规则禁 async）：`submit` 广播 PrePrepare 后返回 seq；`poll(bus, now_ms)` 排空邮箱推进状态机，返回新提交结果集；`wait_for` 语义由增量式投票计数替代 |
//! | D4 | `PbftMessage { msg_type, view, sequence, digest, signature }` | 增 `sender: NodeId`（投票去重与签名验证必需）+ `payload: Vec<u8>`（PrePrepare 携带请求本体，蓝图仅有 digest 无法让备份节点获得请求）；`MsgType` 增 `ViewChange` 变体（VC 消息复用同一消息帧/总线，`view` 字段承载目标视图）；`Reply` 保留占位（执行结果经 `ConsensusResult` 同步返回，网络 Reply 后置） |
//! | D5 | `LogEntry { prepare_count: u32, commit_count: u32 }` | `prepare_voters / commit_voters: BTreeSet<NodeId>`（u32 计数无法识别拜占庭节点重复投票；BTreeSet 确定性去重，D4 集合惯例）+ `prepare_count() / commit_count()` 访问器保持蓝图语义 |
//! | D6 | `ConsensusState { PrePrepare, Prepare, Commit, Done }` | 增 `Idle` 初始/视图切换后状态（引擎启动与 enter_view 后的合法静止态，否则初始状态无定义） |
//! | D7 | §4.3 收集 2f+1 Prepare / 2f+1 Commit | 法定人数 `quorum(n) = 2f+1`：`f(n) = (n-1)/3`；PrePrepare 计入主节点 prepare 票（PBFT 经典变体优化，与蓝图 2f+1 数值一致；n=4 时主节点 PrePrepare + 2 备份 Prepare 即 prepared，容忍 1 备份静默/作恶） |
//! | D8 | §4.4 主节点超时 → ViewChange（未定义消息细节） | 无独立 NewView 消息：ViewChange 广播达 2f+1 法定人数后各节点自主 `enter_view(new_view)`（VC 消息全网广播，诚实节点自然收敛同一 new_view，消除 NewView 伪造面）；新主节点对最近未提交日志重发 PrePrepare 恢复共识；连续 VC 超时指数退避（`timeout_ms << min(连续 vc 次数, 3)`，蓝图 §8.5 坑点"ViewChange 风暴"对策） |
//! | D9 | 签名验证失败 → 丢弃（算法未指定） | SM2 签名（eneros-crypto 既有 `sm2_sign`/`sm2_verify` 复用，§5.5 防重复造轮子）：签名消息 = `msg_type:u8‖view:u64be‖sequence:u64be‖digest‖sender:u64be（‖payload）`；验签失败/未知 sender → rejected_count+=1 丢弃 |
//! | D10 | 错误处理仅 2 条（超时/验签失败） | `ConsensusError { NotPrimary, UnknownNode, InvalidSignature, ViewMismatch, StaleMessage, NotEnoughNodes, BusError }`（7 变体最小完备） |
//! | D11 | 测试 `tests/consensus.rs` | crate 内嵌 `#[cfg(test)]` 40 测试（v0.87.0~v0.98.1 项目惯例；Mock 总线故障注入覆盖主节点离线/拜占庭伪造/重复投票） |
//! | D12 | §9 可观测"共识延迟 metric" | 4 个 pub 计数器（`submit_count` / `committed_count` / `rejected_count` / `view_change_count`）+ `last_latency_ms`（注入时钟：提交时刻 − 提交请求时刻） |
//!
//! ### v0.99.0 实现期补充偏差（pinned 修正）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | EX1 | `ConsensusEngine` 增 `pub rng: CsRng` 字段 + `new()` 增 `rng` 参数 | SM2 签名需要随机数（k 值），无法确定性派生；channel.rs/tunnel.rs 注入 CsRng 先例 |
//! | EX2 | `ConsensusEngine` 增 `pub vc_votes: BTreeMap<u64, BTreeSet<NodeId>>` 字段 | spec 正文要求"LogEntry 外的独立 VC 票集"，字段表漏列；按目标视图分桶收集 VC 票 |
//! | EX3 | `LogEntry` 增 `pub submitted_ms: u64` 字段 | D12 延迟口径"提交时刻 − 请求受理时刻"需要记录受理时刻，否则无法计算 |
//! | EX4 | `MockConsensusBus` 增 `register(id)` 方法 | 投递集合 = 已注册且非 isolated 节点邮箱；引擎集合在 register 时建邮箱，测试为每节点 register |
//! | EX5 | 未来视图 PrePrepare 乐观视图同步：先 `enter_view(msg.view)` 再按正常流程处理 | 签名有效的新视图 PrePrepare 证明新主已获 VC 法定人数；诚实节点随新主收敛，避免分区恢复后卡死在旧视图 |
//! | EX6 | 未 committed 条目收到新视图 PrePrepare → 投票集重置为 {主} 并重广播 Prepare | D8 恢复路径的备份侧配套：新主重发 PrePrepare 后，备份须丢弃旧视图投票重新计票才能达成新视图 quorum |
//! | EX7 | `MockConsensusBus::receive` 用 `remove(0)` FIFO 弹队首 | spec "弹出队首"语义；FIFO 保证测试消息序确定性（LIFO 会乱序 Prepare/Commit 处理） |
//! | EX8 | `on_pre_prepare` 状态迁移（Prepare）先于投票广播 | 活性修正：广播失败（BusError）时节点仍保持 Prepare 态，`check_timeout` 可触发 VC 恢复；否则故障注入场景永久停滞 Idle 无法自愈（TV38 依赖） |
//! | EX9 | `check_timeout` 发起 VC 时 `last_progress_ms = now_ms` 重启退避计时 | 防风暴修正：不重启则退避到 8x 封顶后每次调用都触发 VC（违背 §8.5 目的）；重启后 VC 间隔 1x/2x/4x/8x/8x… 频率有界（TV33 依赖） |
//!
//! ## v0.100.0 资源争抢竞价（统一价格拍卖撮合 + 安全底线 + 共识确认）
//!
//! 联邦内多 Agent 对可调资源（储能充放、可中断负荷等）竞价争抢，统一价格拍卖
//! 撮合定价；安全底线拦截异常低价，限价防操纵，撮合结果经 v0.99.0 共识确认后
//! 执行资源分配。
//!
//! - [`auction`]：竞价引擎 [`AuctionEngine`]——簿管理、提交校验、限价过滤、
//!   快照撮合、计数器可观测。
//! - [`bid_book`]：定点订单簿类型——[`AgentId`] / [`Price`] / [`Qty`] /
//!   [`BidOrder`] / [`AskOrder`] / [`Match`] / [`MatchResult`] /
//!   [`OrderBook`] / [`AuctionError`] / [`match_digest`]。
//! - [`matching`]：纯函数 [`match_book`]——统一价格撮合算法（bids 降序 × asks
//!   升序双指针，价=(bid+ask)/2 向下取整），不修改簿状态。
//!
//! ## v0.100.0 D1~D13 偏差表（相对蓝图 v0.100.0 原文）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 f32 price/qty → **定点 u64**（Price=毫元 1e-3 元，Qty=Wh 1e-3 kWh） | 撮合结果须经 v0.99.0 共识确认（蓝图 §4.3 末步），跨节点需逐字节一致——IEEE 浮点存在平台/编译非确定风险；定点无 NaN、字节稳定，且消除 `partial_cmp().unwrap()` panic 路径（no_std 禁 panic 惯例） |
//! | **D2** | `AgentId = u64`（蓝图 `agent.clone()` 暗示 String） | 项目无堆值类型惯例（v0.97.0 NodeId=u64，电力调度确定性可复现审计） |
//! | **D3** | 蓝图 `crates/federation/src/` → `crates/agents/federation/src/` | 记忆 §2.3.1 强制：所有 crate 归 `crates/<subsystem>/`；eneros-federation 既有 crate 增量扩展（v0.98.0~v0.99.0 同例） |
//! | **D4** | 蓝图 `docs/phase2/auction.md` → `docs/agents/auction-design.md` | 记忆 §2.3.3 强制：文档按方向分类，agents 子系统文档归 `docs/agents/` |
//! | **D5** | 蓝图 `tests/auction.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.99.0 项目惯例，不新增 tests/ 文件 |
//! | **D6** | 撮合算法独立 `matching.rs` 纯函数 `match_book` | 蓝图文件名保留；纯函数（无引擎状态）独立可测，AuctionEngine 仅做簿管理/校验/计数 |
//! | **D7** | `submit_bid/submit_ask` 返回 `Result<(), AuctionError>`（蓝图为空返回） | price=0/qty=0/超限价需入簿前拒绝；蓝图 §4.4 错误处理"安全底线违反→拒绝"扩展至提交侧 |
//! | **D8** | `AuctionEngine` 增 `max_price: Option<Price>` 限价 | 蓝图 §8.5 坑点"价格波动大需限价"的直接对策 |
//! | **D9** | 增 4 计数器（bid_count/ask_count/match_count/rejected_count）+ `last_clearing_price` | 蓝图 §9 可观测"成交记录 metric"；no_std 无 log crate，metric 字段化（v0.99.0 D12 同例） |
//! | **D10** | 增 `MatchResult::to_bytes()` + `match_digest()`（SM3） | 蓝图 §4.3 末步"共识确认"的落地 seam；auction 模块不持有 ConsensusEngine，序列化字节交由上层 submit，保持模块独立可测 |
//! | **D11** | 不复用 v0.86.0 `Bid` 类型，新建 `BidOrder/AskOrder` | eneros-federation 保持仅依赖 eneros-crypto（SBOM 不变）；v0.86.0 报价意图由上层适配转换，避免 agents 子系统内横向耦合 |
//! | **D12** | `match_orders(&self)` 保持蓝图快照语义（不消耗簿），增 `clear_book()` 轮次重置 | 蓝图 §4.2 签名为 `&self`；轮次制拍卖需在共识确认后清簿开新轮 |
//! | **D13** | 成交价 `(bid+ask)/2` 定点 u64 向下取整 | 定点化配套确定性规则；取整方向全网点一致方可逐字节一致 |
//!
//! ## v0.101.0 断网处理与孤岛模式
//!
//! - **v0.101.0 断网处理与孤岛模式**：PartitionDetector 四态状态机（quorum 判据）+ 交易冻结查询 + IslandMode 本地自治缓存（EventCache\<T\> 溢出丢弃最旧）+ RecoverySync 增量同步（Conflict 仲裁/UploadFailed 保留重试）
//!
//! ## v0.101.0 D1~D10 偏差表（相对蓝图 v0.101.0 原文）
//!
//! | 编号 | 偏差 | 理由 |
//! |------|------|------|
//! | **D1** | 蓝图 `crates/federation/src/` → `crates/agents/federation/src/` | 记忆 §2.3.1 强制：所有 crate 归 `crates/<subsystem>/`；既有 crate 增量扩展（v0.98.0~v0.100.0 同例） |
//! | **D2** | 蓝图 `docs/phase2/island_mode.md` → `docs/agents/island-mode-design.md` | 记忆 §2.3.3 强制：文档按方向分类 |
//! | **D3** | 蓝图 `tests/partition.rs` → src 内嵌 `#[cfg(test)]` | v0.87.0~v0.100.0 项目惯例，不新增 tests/ 文件 |
//! | **D4** | `RecoverySync::sync` 蓝图 `async fn` → **同步 fn** | no_std 硬规则禁 async（v0.99.0 D3 先例）；上传经 `SyncSink<T>` trait seam，生产由 channel/tunnel 适配注入 |
//! | **D5** | `EventCache<T>` **泛型化**，不直接引用 v0.96.0 `EventRecord` | eneros-federation 保持仅依赖 eneros-crypto（SBOM 不变）；避免 agents 子系统内横向耦合（v0.100.0 D11 先例）；上层以 cloud-coordinator `EventRecord`/`DomainData` 实例化 |
//! | **D6** | 蓝图 `Duration`/`HashMap`/`now_ms()` → `u64` ms / `BTreeMap` / **注入时钟参数** | no_std alloc 无 HashMap；注入时钟保证确定性可复现 + 可测（v0.99.0 D12 先例） |
//! | **D7** | `info!`/`warn!` 日志 → **计数器字段**（overflow_count/activated_count）+ 状态 pub | no_std 无 log crate，metric 字段化（v0.99.0 D12/v0.100.0 D9 同例） |
//! | **D8** | "确认断网"判据落地为 **alive < quorum(n)**（复用 `pbft::quorum`） | 与 v0.99.0 共识语义闭环：quorum 不可达即无法提交任何决议，业务上等同断网；Suspected（部分失联但 ≥ quorum）不冻结，容忍抖动 |
//! | **D9** | 增 `trading_frozen()` 查询 + `complete_recovery()` 显式完成 | 蓝图 §7.3"断网冻结交易"的落地接口（AuctionEngine 使用侧查询）；Recovering→Connected 需同步完成事件驱动（蓝图 §4.3 状态图"同步完成"迁移） |
//! | **D10** | `cache_event` 返回 `bool`（蓝图为空返回） | 未激活静默丢弃需可观测（蓝图 §4.5 return 语义的可测化） |

extern crate alloc;

pub mod auction;
pub mod bid_book;
pub mod cache;
pub mod channel;
pub mod consensus;
pub mod detector;
pub mod discovery;
pub mod matching;
pub mod membership;
pub mod partition;
pub mod pbft;
pub mod recovery;
pub mod tunnel;
pub mod view_change;

// v0.100.0 资源争抢竞价
pub use auction::AuctionEngine;
pub use bid_book::{
    match_digest, AgentId, AskOrder, AuctionError, BidOrder, Match, MatchResult, OrderBook, Price,
    Qty,
};
// v0.101.0 事件缓存（孤岛自治本地缓冲）
pub use cache::EventCache;
// v0.98.0 跨域通信通道
pub use channel::{
    ChannelError, Endpoint, FederationChannel, MockSecureTransport, SecureTransport, TlsConfig,
};
// v0.99.0 联邦共识协议（PBFT 变体）
pub use consensus::{
    ConsensusBus, ConsensusEngine, ConsensusError, ConsensusResult, ConsensusState, LogEntry,
    MockConsensusBus, MsgType, NodeId, PbftMessage,
};
// v0.101.0 联邦网络分区检测
pub use detector::{PartitionDetector, PartitionState};
// v0.97.0 联邦发现
pub use discovery::{
    CertVerifier, FedError, FederationDiscovery, MockCertVerifier, MockPresenceBus, PresenceBus,
};
// v0.100.0 统一价格撮合纯函数
pub use matching::match_book;
pub use membership::{CertRef, JoinRequest, MemberInfo, MemberRegistry, NodeRole};
// v0.101.0 孤岛自治模式
pub use partition::IslandMode;
// v0.99.0 PBFT 三阶段数学与签名辅助
pub use pbft::{f, primary_of, quorum, sign_message, verify_message};
// v0.101.0 恢复增量同步
pub use recovery::{MockSyncSink, RecoverySync, SyncError, SyncReport, SyncSink};
// v0.98.1 纵向加密认证
pub use tunnel::{
    derive_tunnel_keys, initiator_finish, initiator_hello, responder_accept, verify_dispatch_auth,
    AuthResult, DispatchToken, EncryptError, MockVerticalEncryptDevice, TunnelKeys, TunnelManager,
    VerticalEncryptDevice, VerticalEncryptTunnel,
};
