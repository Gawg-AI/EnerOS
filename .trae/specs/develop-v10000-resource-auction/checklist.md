# Checklist — v0.100.0 资源争抢竞价机制

> Spec：`spec.md`（develop-v10000-resource-auction）。逐项核验，未通过禁止收工。

## A. 目录结构校验（§2.4.1，C1~C5）

- [x] C1: 3 新模块位于既有 crate `crates/agents/federation/src/{auction,bid_book,matching}.rs`，未新增根目录 crate
- [x] C2: 根 `Cargo.toml` workspace 成员无新增，workspace 仍可解析
- [x] C3: eneros-federation `Cargo.toml` 依赖不变（仅 eneros-crypto），零新增第三方依赖
- [x] C4: 新文档 `auction-design.md` 位于 `docs/agents/`，未平面化放 `docs/` 根
- [x] C5: 仓库根目录无除 `ci/` 外的新 crate 文件夹

## B. 构建校验（§2.4.2，C6~C11）

- [x] C6: `cargo metadata --format-version 1` 成功
- [x] C7: `cargo test -p eneros-federation`（既有 160 + 新增 30 = 190）全部通过；全 workspace 回归全绿；`cargo test -p eneros-crypto` 零回归
- [x] C8: `cargo build -p eneros-federation --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 通过
- [x] C9: `cargo fmt --all -- --check` 通过
- [x] C10: `cargo clippy --workspace --exclude eneros-kernel --exclude eneros-hello --all-targets -- -D warnings` 0 warning
- [x] C11: `cargo deny check advisories licenses bans sources` 通过（零新增第三方依赖）

## C. 文档与规范校验（§2.4.3，C12~C15）

- [x] C12: 新文档在 `docs/agents/` 下，不在 `docs/` 根
- [x] C13: `git status` 无 `target/`、`*.elf`、`*.bin`、`*.dtb`、IDE 缓存被追踪
- [x] C14: 无新文件类型需 `.gitignore` 覆盖
- [x] C15: 新代码无 `use std::*` / `panic!` / `todo!` / `unimplemented!` / `unsafe` / `async`（no_std 合规；测试模块内 std 位于 `#[cfg(test)]` 下允许，如 Instant 性能测量）

## D. bid_book.rs 类型与订单簿（C16~C30）

- [x] C16: `pub type AgentId = u64` / `pub type Price = u64`（毫元）/ `pub type Qty = u64`（Wh）（D1/D2）
- [x] C17: `BidOrder { agent, price, qty }` 字段全 pub，派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C18: `AskOrder` 同构派生
- [x] C19: `OrderBook { bids, asks }` 字段全 pub
- [x] C20: `OrderBook::new()` 空簿；`is_empty()/len()` 正确
- [x] C21: `submit_bid/submit_ask` 合法订单入簿返回 Ok
- [x] C22: price==0 或 qty==0 → `Err(AuctionError::InvalidOrder)` 且不入簿（D7）
- [x] C23: `clear()` 清空双向订单
- [x] C24: `Match { buyer, seller, price, qty }` 字段全 pub 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C25: `MatchResult { matches, clearing_price }` 字段全 pub
- [x] C26: `AuctionError { InvalidOrder, PriceCapExceeded }` 派生 Debug/Clone/Copy/PartialEq/Eq
- [x] C27: `MatchResult::to_bytes()` 字节布局 = clearing_price u64be ‖ count u64be ‖ 每 match (buyer‖seller‖price‖qty 各 u64be)，长度 = 16 + 32×count
- [x] C28: `to_bytes()` 两次调用结果一致（确定性）
- [x] C29: `match_digest()` 两次一致；内容微变（qty±1）→ digest 变
- [x] C30: `match_digest` 复用 eneros-crypto `sm3_hash`（§5.5 防重复造轮子）

## E. matching.rs 撮合算法（C31~C44）

- [x] C31: `match_book(book: &OrderBook, safety_floor: Price) -> MatchResult` 纯函数（D6），不修改入参
- [x] C32: bids 按价格降序、asks 按价格升序（u64 `Ord`，无浮点/unwrap，D1）
- [x] C33: 价格交叉 `bid.price >= ask.price` 成交，价 = `(bid+ask)/2` u64 向下取整（D13）
- [x] C34: 成交量 = `min(bid.qty, ask.qty)`，部分成交后余量继续撮合
- [x] C35: 成交价 < safety_floor → 停止撮合（后续对不再成交，蓝图 §4.4）
- [x] C36: 成交价 == safety_floor → 正常成交（边界含等）
- [x] C37: 无成交 → `matches` 空 + `clearing_price == 0`（蓝图 §4.4 无匹配兜底）
- [x] C38: `clearing_price` = 末笔成交价（统一价格语义，蓝图 §4.5）
- [x] C39: 单对撮合：bid(100,10)/ask(90,8) → 1 笔 price=95 qty=8
- [x] C40: bid.price < ask.price → 不成交
- [x] C41: 多对撮合顺序确定（降序 bids × 升序 asks 双指针），同价订单 agent 序稳定（结果跨节点可复现）
- [x] C42: qty 守恒：成交总量 ≤ min(bids 总量, asks 总量)
- [x] C43: 快照语义：撮合后入参簿 bids/asks 长度与内容不变（D12）
- [x] C44: 1 万订单大簿撮合结果合法（冒烟，不 panic）

## F. auction.rs 引擎（C45~C56）

- [x] C45: `AuctionEngine` 字段全 pub：book/safety_floor/max_price/bid_count/ask_count/match_count/rejected_count/last_clearing_price（D8/D9）
- [x] C46: `new(safety_floor, max_price)` 初始：簿空、4 计数器全零、last_clearing_price==0
- [x] C47: submit_bid 成功 → bid_count+=1；submit_ask 成功 → ask_count+=1
- [x] C48: 超 max_price → `Err(PriceCapExceeded)` + rejected_count+=1 + 不入簿（D8）
- [x] C49: max_price=None → 不限价
- [x] C50: `match_orders(&self)` 快照语义：返回 MatchResult 且 self.book 不变（D12）
- [x] C51: match_orders 后 match_count += 本轮成交数、last_clearing_price 更新
- [x] C52: 空簿 match_orders → match_count 不变、last_clearing_price 不变
- [x] C53: `clear_book()` 清簿后可重新 submit 开新轮（D12）
- [x] C54: 簿校验失败（price=0）→ `Err(InvalidOrder)`，不触及 max_price 分支
- [x] C55: 计数器累计跨多轮正确（submit→match→clear→submit→match）
- [x] C56: AuctionEngine 不持有 ConsensusEngine（D10 seam 分离，撮合/共识模块独立）

## G. 共识确认集成（C57~C60）

- [x] C57: 4 节点集群（consensus::testutil build_cluster）主节点 `submit(match_result.to_bytes())` 成功
- [x] C58: drive 后 4 节点 `is_committed(1)==true`
- [x] C59: 各节点 committed digest == `match_digest(&match_result)`
- [x] C60: 文档声明生产路径：MatchResult.to_bytes() → ConsensusEngine.submit → committed 后执行资源分配（蓝图 §4.3 末步落地）

## H. 配置文件（C61~C66）

- [x] C61: `configs/federation-auction.toml` 存在，`[auction]` 段含 safety_floor / max_price
- [x] C62: 中文注释 ≥6 点（统一价格拍卖 §5.1 / 定点单位 D1 / 安全底线 §4.4 / 限价 §8.5 / 共识确认 §4.3 / 计数器可观测 D9）
- [x] C63: safety_floor 默认 0（不启用底线，向后兼容蓝图默认行为）
- [x] C64: max_price 注释说明 0 或省略 = 不限价（与代码 Option 语义映射声明）
- [x] C65: 定点单位注释明确（price 毫元 1e-3 元、qty Wh 1e-3 kWh，D1）
- [x] C66: 配置项命名与设计文档接口契约一致

## I. 设计文档（C67~C74）

- [x] C67: `docs/agents/auction-design.md` 存在且 12 章节齐全
- [x] C68: Mermaid 图 ≥2（撮合流程图按蓝图 §4.3 重绘含安全底线分支 + 撮合→共识确认时序图）
- [x] C69: D1~D13 偏差表与 spec.md 一致
- [x] C70: 接口契约章节与实现签名一致（含定点单位与取整规则 D13）
- [x] C71: 技术交底含选型对比表（蓝图 §5.1 统一价格拍卖 vs 双向拍卖 vs 轮询）
- [x] C72: 安全章节覆盖安全底线（§7.3）+ 市场操纵防御（§8.1，限价/共识确认/签名来源）
- [x] C73: 性能章节声明撮合 <100ms（蓝图 §7.2）与测量口径（host Instant，1 万订单）
- [x] C74: 上下游交互声明（上游 v0.99.0 共识/v0.86.0 报价适配 D11；下游 v0.101.0 孤岛）

## J. 版本同步（C75~C79）

- [x] C75: 根 `Cargo.toml` `[workspace.package] version = "0.100.0"`
- [x] C76: `Makefile` 版本注释 + VERSION 变量同步 0.100.0
- [x] C77: `.github/workflows/ci.yml` 版本注释同步 0.100.0
- [x] C78: `ci/src/gate.rs` 注释串尾追加 v0.100.0 类型清单（AgentId/Price/Qty/BidOrder/AskOrder/OrderBook/Match/MatchResult/AuctionError/AuctionEngine/match_book/match_digest），2 处
- [x] C79: eneros-federation `Cargo.toml` description 追加 v0.100.0

## K. 测试覆盖（C80~C90）

- [x] C80: bid_book.rs 内嵌 10 测试（TB1~TB10）通过
- [x] C81: matching.rs 内嵌 12 测试（TM11~TM22）通过
- [x] C82: auction.rs 内嵌 8 测试（TA23~TA30）通过
- [x] C83: 新增测试总计 30 个，`cargo test -p eneros-federation` 190 全过
- [x] C84: 提交校验测试覆盖（price=0/qty=0/超限价，TB3/TB4/TA25）
- [x] C85: 撮合正确性测试覆盖（基本/部分成交/无匹配/出清价，TM12~TM19）
- [x] C86: 安全底线测试覆盖（全部拒/边界含等，TM18/TM19 或对应编号）
- [x] C87: 共识 e2e 测试覆盖（TA29 或对应编号：4 节点 committed + digest 一致）
- [x] C88: 性能测试覆盖（TA30 或对应编号：1 万订单 <100ms）
- [x] C89: 既有 160 测试零回归（membership/discovery/channel/tunnel/consensus/pbft/view_change）
- [x] C90: eneros-crypto 测试零回归（本版本未改动 crypto，SM3 复用）

## L. 蓝图对齐与验收（C91~C97）

- [x] C91: v0.100.0 交付物全覆盖：auction/matching/bid_book 3 模块 / AuctionEngine / OrderBook / MatchResult（蓝图 §3）
- [x] C92: 竞价撮合可用（蓝图 §7.1 功能）
- [x] C93: 撮合 <100ms（蓝图 §7.2 性能量化）
- [x] C94: 安全底线保证（蓝图 §7.3 安全：safety_floor 拒绝路径有测试）
- [x] C95: 无匹配兜底（蓝图 §9 可靠：空结果不 panic）
- [x] C96: 规则配置化（蓝图 §9 可维护：safety_floor/max_price toml 配置）
- [x] C97: 成交记录 metric（蓝图 §9 可观测：4 计数器 + last_clearing_price）

---

## 验收记录（2026-07-19 收工核验）

- **B 构建校验实测**：C6 `cargo metadata` 通过；C7 `cargo test -p eneros-federation` **190/190**（既有 160 + 新增 30）通过，全 workspace 回归 149 个测试二进制全部 `test result: ok`（含 eneros-crypto 358 零回归）；C8 aarch64-unknown-none 交叉编译通过；C9 fmt 通过；C10 clippy `-D warnings` 0 warning；C11 `cargo deny check` 全项 ok（零新增第三方依赖，SBOM 不变）。
- **C13 实测**：`git status --porcelain` 过滤 `target/|*.elf|*.bin|*.dtb|.idea|.vscode|*.log` 零匹配，无垃圾文件被追踪。
- **子代理只读核验 61 项全 PASS**（C1~C5/C12/C15/C16~C74/C79~C82/C88/C96/C97）：三模块结构/派生/字段、撮合算法语义、引擎校验与计数器、共识 e2e、配置文件 6 注释点、设计文档 12 章节 + 2 Mermaid + D1~D13 偏差表，逐项证据在案。
- **实现签名说明**：`AuctionEngine::match_orders(&mut self)`（更新 match_count/last_clearing_price 需可变借用）；蓝图 §4.2 签名为 `&self`——D12 快照语义针对**订单簿**（撮合不消耗簿，match_book 纯函数 &OrderBook），计数器更新为 D9 新增可观测能力，接口契约章节已按 `&mut self` 声明，语义一致。
- **版本同步**：根 `Cargo.toml` / `Makefile` VERSION / `ci.yml` 注释 / `gate.rs` 注释串尾（2 处 replace_all）→ 0.100.0；eneros-federation description 已含 v0.100.0。
