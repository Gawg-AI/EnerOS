# ADR-053：chunked 分页响应 wire v2（判别帧 + 事务标识，蓝图 70.3 WP-70.3.3）

- 状态：已接受（2026-09-09）
- 关联：蓝图 70 [05-方向2] §3.2（Pull 判别方案 A）、§4 WP-70.3.3；
  ADR-0025（版本化跨 binary wire）、ADR-0026（协议唯一 seam）；
  REG-CHUNKED-PULL-NO-DISCRIMINATOR（本 ADR 关闭该登记）
- 裁决对象：`crates/agents/agent-sdk/src/chunked.rs`（wire 单源）、
  `crates/agents/control-services/src/service_slots.rs`（生产服务端）、
  `crates/protocols/protocol-services/src/ingestion_stage.rs`（ingest 服务端
  假件同律）
- 验收映射：CORE-70-R04（异常输入早拒、下一合法事务可完成）中的响应判别
  与事务归属判据

## 背景与事实

1. **v1 不可判别通道（N5，2026-08-28 体检 + 69.1 复核）**：Pull 回复与拒绝
   共用同一字节通道且无判别位——`len < expected` 在 expected ≤ 16B 时无法
   区分"ERR 文本"与"末页载荷"（MR word 粒度零填充把 13B ERR 回执填成 16B，
   service.rs 组帧与 ipc_sel4 组装两端同款）。
2. **发散源 #1（REG-CHUNKED-PULL-NO-DISCRIMINATOR）**：respawn/badge 复用后
   旧客户端按旧分页头续拉——服务端无法鉴别 Pull 是否属于当前安装的响应，
   页可跨事务拼接。
3. **69.1 实弹教训（t12 / VOL69.1-GATE）**：拒绝覆盖式改法造成重试活锁
   （ServerRejected×4050），已撤回——覆盖性 Final 的自愈重同步语义必须保持。
4. **wire 冻结解除**：69 卷 Z1"本卷不能改 wire"是施工期禁区，蓝图 70 规划
   明确解除（方向 2 §3.2："不继承 69.1 本卷不能改 wire 为永久限制"）。

## 裁决

**采纳方案 A（修订 wire），硬退役 v1，同一启动 bundle 两端共同升级：**

1. **帧面**（全部 ≤ `MR_REPLY_BUDGET_BYTES`=112B，编译期锁）：
   - Final 响应头 v2：`ENV2 ‖ total u32 ‖ count u8 ‖ last_len u8 ‖ txn u16`
     （14B）；
   - Pull 请求 v2：`ENVN ‖ index u8 ‖ txn u16`（7B；v1 5B 形 = 协议违例，
     服务端 `CHUNKED-PULL-STALE` 记账拒绝，非静默兼容）；
   - Pull 响应判别帧：`ENVR ‖ page_len u8 ‖ txn u16 ‖ payload`（7+104=111B）。
   - 页大小不变（104B）；响应方向页数域界不变（255 页 / 26520B）；
     请求方向容量合同不变（16 块 / 1664B，WP-70.3.1 另行域检）。
2. **txn（事务标识）**：16-bit = 服务端接受的 Header 中发送方 epoch 低 16
   位。服务端 Header 接受时记录、Final 安装响应时绑定、Pull 校验一致才供
   页；客户端逐页校验回带一致才收载。
   - 同 badge respawn（表清空）→ 旧 Pull 无当前事务 → 结构化拒绝；
   - 旧客户端对覆盖安装的新响应续拉 → txn 失配 → 拒绝（发散源 #1 关闭）；
   - u16 回绕：服务端只做等值比较（非序关系），撞窗需同 badge 65536 次
     传输间隔且旧客户端仍在拉页（Call 超时 5s 下结构性不可达）——残面
     有界并在此登记。
3. **自愈语义保持**：覆盖性 Final 照常安装新响应（PAGE-EVICTED 记账），
   t12 反活锁锁测试逐字保留——本 ADR 只关闭"不可判别"，不触碰"覆盖/拒绝"
   裁决。
4. **v1 硬退役**：`encode_paged_header` / `decode_paged_header` / 5B
   `pull_request` 删除；ENV2 之外的分页头在客户端 decode 面即 `None`
   （ServerRejected）。同一镜像两端同步升级，无混部窗口——与 69.4 旧
   proposal wire 退役同纪律。若未来出现跨版本混部需求（独立升级 OTA），
   须先形成协商能力裁决，禁止静默退回 v1 不可判别模式。

## 迁移与影响

- 生产 writer/reader 全部在仓（服务端 = service_slots / ingestion 服务端
  语义；客户端 = ChunkedClient 经 ChunkedEndpoint）——grep 证实无仓外
  消费者；host 测试假件 5 份手抄 v1 帧律收编为 `PagedServerV2` 单源。
- 帧预算复核：页帧 111B ≤ 112B（`PAGE_FRAME_HEADER_LEN + RESP_PAGE_MAX
  <= MR_REPLY_BUDGET_BYTES` 静态锁）；ERR 文本路径 ≤ 112B 由既有 t13
  穷举锁覆盖（新增 `BadTotalLen`/`BadChunkSize` 变体已入穷举）。
- 回滚：协议共同升级镜像可整体回滚；新 wire 不落任何持久化介质，无旧盘
  兼容面。
