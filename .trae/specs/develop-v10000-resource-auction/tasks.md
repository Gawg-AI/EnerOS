# Tasks — v0.100.0 资源争抢竞价机制

> Spec：`spec.md`（develop-v10000-resource-auction）。crate 内聚性强，T1~T3 严格顺序（类型→算法→引擎），T4~T6 顺序收尾。

- [x] **T1：bid_book.rs — 定点类型与订单簿**
  - [ ] 1.1 类型别名：`pub type AgentId = u64; pub type Price = u64; // 毫元` `pub type Qty = u64; // Wh`（D1/D2）
  - [ ] 1.2 `BidOrder { agent, price, qty }` / `AskOrder { agent, price, qty }`（Debug/Clone/Copy/PartialEq/Eq）
  - [ ] 1.3 `OrderBook { bids: Vec<BidOrder>, asks: Vec<AskOrder> }`：`new/submit_bid/submit_ask/clear/is_empty/len`；submit 校验 price==0||qty==0 → `Err(InvalidOrder)`（D7）
  - [ ] 1.4 `Match { buyer, seller, price, qty }` / `MatchResult { matches: Vec<Match>, clearing_price }`
  - [ ] 1.5 `AuctionError { InvalidOrder, PriceCapExceeded }`（Debug/Clone/Copy/PartialEq/Eq）
  - [ ] 1.6 `MatchResult::to_bytes()` 确定性序列化（clearing_price u64be ‖ count u64be ‖ 每 match buyer‖seller‖price‖qty 各 u64be）+ `match_digest(&MatchResult) -> [u8;32]`（SM3，复用 eneros-crypto `sm3_hash`，D10）
  - [ ] 1.7 测试 TB1~TB10：类型派生/Copy 语义；submit 合法入簿；price=0 拒；qty=0 拒；clear 清空；is_empty/len；to_bytes 长度与字节布局；to_bytes 两次一致（确定性）；match_digest 两次一致；digest 对内容敏感（改 qty → digest 变）
  - 验证：`cargo test -p eneros-federation bid_book` 全过

- [x] **T2：matching.rs — 统一价格撮合纯函数**
  - [ ] 2.1 `pub fn match_book(book: &OrderBook, safety_floor: Price) -> MatchResult`（D6 纯函数，不修改入参簿）
  - [ ] 2.2 bids 价格降序、asks 价格升序排序（定点 u64 `Ord`，无浮点比较，D1）
  - [ ] 2.3 双指针撮合：`bid.price >= ask.price` 成交，`price = (bid.price + ask.price) / 2`（u64 向下取整，D13），`qty = min`；price < safety_floor → 停止（蓝图 §4.4）；部分成交后余量继续
  - [ ] 2.4 `clearing_price = matches.last().price`，无成交 → 0 + 空 matches（蓝图 §4.4 无匹配兜底）
  - [ ] 2.5 测试 TM11~TM22：空簿 → 空结果；单对成交（100/90→95）；bid<ask 不成交；价格相等成交；部分成交（bid 余量）；部分成交（ask 余量）；多对撮合顺序与出清价=末笔；安全底线全部拒；安全底线边界（price==floor 成交）；qty 守恒（成交总量 ≤ min( bids 总量, asks 总量)）；大簿冒烟（1 万订单，结果合法）；同价 bids 顺序稳定（agent 序确定性）
  - 验证：`cargo test -p eneros-federation matching` 全过

- [x] **T3：auction.rs — AuctionEngine 与可观测**
  - [ ] 3.1 `AuctionEngine { book, safety_floor, max_price: Option<Price>, bid_count, ask_count, match_count, rejected_count, last_clearing_price }` 字段全 pub（D8/D9）
  - [ ] 3.2 `new(safety_floor, max_price)`；`submit_bid/submit_ask`：簿校验 + 超 max_price → `Err(PriceCapExceeded)` + rejected_count+=1，成功则对应计数器+=1（D7/D8）
  - [ ] 3.3 `match_orders(&self) -> MatchResult`（快照语义，D12）：内部调 `match_book`，更新 match_count（+=本轮成交数）与 last_clearing_price
  - [ ] 3.4 `clear_book(&mut self)`：清簿开新轮（D12）
  - [ ] 3.5 测试 TA23~TA30：new 初始计数全零；submit 双方向计数；超限价拒 + rejected_count；match_orders 更新 match_count/last_clearing_price 且簿不变（快照）；clear_book 后可重新入簿；空簿撮合计数不变；**4 节点共识 e2e**（复用 consensus::testutil `build_cluster`：主节点 submit(result.to_bytes()) → drive → 4 节点 is_committed 且 digest==match_digest）；**性能**：1 万笔撮合 < 100ms（`std::time::Instant`，仅 cfg(test)）
  - 验证：`cargo test -p eneros-federation auction` 全过

- [x] **T4：模块接线 + 配置 + 设计文档**
  - [ ] 4.1 `lib.rs`：`pub mod auction; pub mod bid_book; pub mod matching;` + 全类型重导出 + crate 文档追加 v0.100.0 段与 D1~D13 偏差表（既有模块零改动）
  - [ ] 4.2 `Cargo.toml` description 追加 "v0.100.0 资源争抢竞价"
  - [ ] 4.3 `configs/federation-auction.toml`：`[auction]` safety_floor / max_price + 中文注释 ≥6 点（统一价格拍卖 §5.1 / 定点单位 D1 / 安全底线 §4.4 / 限价 §8.5 / 共识确认 §4.3 / 计数器 D9）
  - [ ] 4.4 `docs/agents/auction-design.md`：12 章节（版本目标/前置依赖/交付物清单/详细设计/技术交底/测试计划/验收标准/风险/多角度要求/接口契约/偏差声明/附录）+ ≥2 Mermaid（撮合流程图按蓝图 §4.3 重绘 + 撮合→共识确认时序图）+ D1~D13 偏差表
  - 验证：`cargo test -p eneros-federation` 全过（既有 160 + 新增 30 = 190）

- [x] **T5：版本同步 0.100.0 + 全量构建验证**
  - [ ] 5.1 根 `Cargo.toml` version = "0.100.0"；`Makefile` ×2；`ci.yml`；`gate.rs` 注释串尾追加 v0.100.0 类型清单（2 处，replace_all）
  - [ ] 5.2 §2.4.2 构建校验：C6 metadata / C7 `cargo test -p eneros-federation` + 全 workspace 回归 + `cargo test -p eneros-crypto` 零回归 / C8 aarch64 交叉编译 / C9 fmt / C10 clippy -D warnings / C11 cargo deny
  - 验证：C6~C11 全绿

- [x] **T6：checklist 逐项核验收工**
  - [ ] 6.1 `checklist.md` C1~C~95 逐项核验勾选 + 验收记录
  - 验证：checklist 全勾，收工

# Task Dependencies

- T2 depends on T1（撮合消费订单簿类型）
- T3 depends on T1 + T2（引擎组合簿与撮合）
- T4 depends on T3（接线/文档基于完成实现）
- T5 depends on T4（版本同步后统一验证）
- T6 depends on T5
