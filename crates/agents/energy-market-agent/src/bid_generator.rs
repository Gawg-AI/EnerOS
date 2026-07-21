//! v0.86.0 报价生成 —— 报价生成器.
//!
//! 本模块实现 v0.86.0 报价生成版本的核心：消费 v0.85.0 [`MarketFeed`]（电价点列表），
//! 结合储能 SOC / 容量与 [`BidStrategy`]（报价策略），经「意图 → 优化 → 生成 → 发布」
//! 四级流水线产出 `Vec<Bid>`。意图 / 优化两级失败分别回退规则策略（[`rule_intent`]）
//! 与保守报价（[`conservative_optimize`]），满足蓝图 §4.4 降级要求。
//!
//! # 偏差声明（D1/D5/D6/D7/D8/D11）
//!
//! - **D1**：蓝图 `async fn generate_bids` → sync
//!   `generate(&mut self, feed, soc, capacity, now_ms)`。no_std 无 async runtime /
//!   无 `Instant` / 无 `Duration`；沿用 v0.85.0 D1 sync 模式，由外部调度器驱动调用。
//! - **D5**：蓝图 `Arc<dyn LlmEngine>` 意图来源 → 本地 [`BidIntentSource`] trait +
//!   [`MockBidIntentSource`]。no_std 无 `Arc`；trait 对象以 `Box<dyn>` 注入，
//!   LLM 适配器后续接入。
//! - **D6**：蓝图 `Arc<dyn Solver>` 优化器 → 本地 [`BidOptimizer`] trait +
//!   [`MockBidOptimizer`]。同上；Solver（HiGHS）适配器后续接入。
//! - **D7**：中间结构 [`BidIntent`] / [`BidOptimization`] MVP 最小字段
//!   （side+target_quantity / price_adjust+quantity）。
//! - **D8**：[`BidError`] 4 变体（InvalidInput/IntentFailed/OptimizeFailed/PublishFailed）。
//! - **D11**：蓝图 `DdsNode` 发布 → 本地 [`BidPublisher`] trait + [`MockBidPublisher`]
//!   （沿用 v0.85.0 D9 `MarketFeedPublisher` 模式），DDS 适配器后续注入。

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::market_feed::{MarketFeed, MarketType, Period};

/// 报价方向.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BidSide {
    /// 买入（充电/购电）.
    #[default]
    Buy,
    /// 卖出（放电/售电）.
    Sell,
}

/// 报价单（MVP 最小字段，D7）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Bid {
    /// 报价 ID（生成器内单调递增，从 1 开始）.
    pub bid_id: u64,
    /// 市场类型.
    pub market_type: MarketType,
    /// 资源 ID（储能/可调资源标识）.
    pub resource_id: u64,
    /// 报价价格（元/MWh）.
    pub price: f32,
    /// 报价电量（MW）.
    pub quantity: f32,
    /// 报价方向.
    pub side: BidSide,
    /// 时段.
    pub period: Period,
    /// 时间戳（ms）.
    pub timestamp: u64,
}

/// 报价策略.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BidStrategy {
    /// 价差边际（元/MWh，Sell 加价 / Buy 减价）.
    pub margin: f32,
    /// 单笔最大报价电量（MW）.
    pub max_quantity: f32,
    /// 卖出 SOC 门限（SOC 低于该值禁止卖出）.
    pub soc_threshold: f32,
}

/// 报价意图（D7：MVP 最小字段）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BidIntent {
    /// 报价方向.
    pub side: BidSide,
    /// 目标报价电量（MW）.
    pub target_quantity: f32,
}

/// 报价优化结果（D7：MVP 最小字段）.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BidOptimization {
    /// 价格调整量（元/MWh）.
    pub price_adjust: f32,
    /// 优化后报价电量（MW）.
    pub quantity: f32,
}

/// 报价错误（D8：4 变体）.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BidError {
    /// 输入非法（如容量非正）.
    InvalidInput,
    /// 意图生成失败.
    IntentFailed,
    /// 优化失败.
    OptimizeFailed,
    /// 发布失败.
    PublishFailed,
}

/// 报价意图来源抽象（D5：trait 替代 Arc<dyn LlmEngine>；不要求 Send + Sync，no_std 单线程）.
pub trait BidIntentSource {
    /// 根据市场馈送与储能 SOC 生成报价意图.
    fn generate_intent(&mut self, feed: &MarketFeed, soc: f32) -> Result<BidIntent, BidError>;
}

/// Mock 意图来源：返回预置意图或恒失败.
#[derive(Debug, Clone, Default)]
pub struct MockBidIntentSource {
    /// 下一次 generate_intent 返回的意图（fail=false 且为 None 时返回 IntentFailed）.
    next_intent: Option<BidIntent>,
    /// 恒失败开关.
    fail: bool,
}

impl MockBidIntentSource {
    /// 构造返回指定意图的 Mock 源.
    pub fn new(intent: BidIntent) -> Self {
        Self {
            next_intent: Some(intent),
            fail: false,
        }
    }

    /// 构造恒失败的 Mock 源.
    pub fn new_failing() -> Self {
        Self {
            next_intent: None,
            fail: true,
        }
    }

    /// Builder：设置下一次 generate_intent 返回的意图.
    pub fn with_intent(mut self, intent: BidIntent) -> Self {
        self.next_intent = Some(intent);
        self
    }
}

impl BidIntentSource for MockBidIntentSource {
    fn generate_intent(&mut self, _feed: &MarketFeed, _soc: f32) -> Result<BidIntent, BidError> {
        if self.fail {
            return Err(BidError::IntentFailed);
        }
        match self.next_intent {
            Some(i) => Ok(i),
            None => Err(BidError::IntentFailed),
        }
    }
}

/// 报价优化器抽象（D6：trait 替代 Arc<dyn Solver>；不要求 Send + Sync，no_std 单线程）.
pub trait BidOptimizer {
    /// 根据意图、市场馈送、SOC 与容量优化报价.
    fn optimize(
        &mut self,
        intent: &BidIntent,
        feed: &MarketFeed,
        soc: f32,
        capacity: f32,
    ) -> Result<BidOptimization, BidError>;
}

/// Mock 优化器：返回预置优化结果或恒失败.
#[derive(Debug, Clone, Default)]
pub struct MockBidOptimizer {
    /// 下一次 optimize 返回的优化结果（fail=false 且为 None 时返回 OptimizeFailed）.
    next_opt: Option<BidOptimization>,
    /// 恒失败开关.
    fail: bool,
}

impl MockBidOptimizer {
    /// 构造返回指定优化结果的 Mock 优化器.
    pub fn new(opt: BidOptimization) -> Self {
        Self {
            next_opt: Some(opt),
            fail: false,
        }
    }

    /// 构造恒失败的 Mock 优化器.
    pub fn new_failing() -> Self {
        Self {
            next_opt: None,
            fail: true,
        }
    }
}

impl BidOptimizer for MockBidOptimizer {
    fn optimize(
        &mut self,
        _intent: &BidIntent,
        _feed: &MarketFeed,
        _soc: f32,
        _capacity: f32,
    ) -> Result<BidOptimization, BidError> {
        if self.fail {
            return Err(BidError::OptimizeFailed);
        }
        match self.next_opt {
            Some(o) => Ok(o),
            None => Err(BidError::OptimizeFailed),
        }
    }
}

/// 报价发布抽象（D11：trait 替代 DdsNode；不要求 Send + Sync，no_std 单线程）.
pub trait BidPublisher {
    /// 发布报价单列表.
    fn publish_bids(&mut self, bids: &[Bid]) -> Result<(), BidError>;
}

/// Mock 发布器：记录已发布报价或恒失败.
#[derive(Debug, Clone, Default)]
pub struct MockBidPublisher {
    /// 已发布的报价记录.
    published: Vec<Bid>,
    /// 恒失败开关.
    fail: bool,
}

impl MockBidPublisher {
    /// 构造正常发布的 Mock 发布器.
    pub fn new() -> Self {
        Self {
            published: Vec::new(),
            fail: false,
        }
    }

    /// 构造恒失败的 Mock 发布器.
    pub fn new_failing() -> Self {
        Self {
            published: Vec::new(),
            fail: true,
        }
    }
}

impl BidPublisher for MockBidPublisher {
    fn publish_bids(&mut self, bids: &[Bid]) -> Result<(), BidError> {
        if self.fail {
            return Err(BidError::PublishFailed);
        }
        self.published.extend_from_slice(bids);
        Ok(())
    }
}

/// 规则策略意图（蓝图 §4.4 意图级回退）：恒卖出，电量取
/// `min(strategy.max_quantity, capacity * max(soc, 0))`.
pub fn rule_intent(
    _feed: &MarketFeed,
    soc: f32,
    capacity: f32,
    strategy: &BidStrategy,
) -> BidIntent {
    BidIntent {
        side: BidSide::Sell,
        target_quantity: strategy.max_quantity.min(capacity * soc.max(0.0)),
    }
}

/// 保守报价优化（蓝图 §4.4 优化级回退）：价差取策略 margin，电量取
/// `min(intent.target_quantity, strategy.max_quantity)`.
pub fn conservative_optimize(intent: &BidIntent, strategy: &BidStrategy) -> BidOptimization {
    BidOptimization {
        price_adjust: strategy.margin,
        quantity: intent.target_quantity.min(strategy.max_quantity),
    }
}

/// 计算报价价格：Sell 加价，Buy 减价且不低于 0.
fn compute_price(side: BidSide, base: f32, adjust: f32) -> f32 {
    match side {
        BidSide::Sell => base + adjust,
        BidSide::Buy => (base - adjust).max(0.0),
    }
}

/// 电量截断：不超过 max_q 与 capacity，且不低于 0.
fn clamp_quantity(q: f32, max_q: f32, capacity: f32) -> f32 {
    q.min(max_q).min(capacity).max(0.0)
}

/// 报价生成器：「意图 → 优化 → 生成 → 发布」流水线.
pub struct BidGenerator {
    /// 报价策略.
    pub strategy: BidStrategy,
    /// 资源 ID.
    pub resource_id: u64,
    /// 意图来源（D5：Box 注入，替代 Arc<dyn LlmEngine>）.
    pub intent_source: Box<dyn BidIntentSource>,
    /// 优化器（D6：Box 注入，替代 Arc<dyn Solver>）.
    pub optimizer: Box<dyn BidOptimizer>,
    /// 发布器（D11：Box 注入，替代 DdsNode）.
    pub publisher: Box<dyn BidPublisher>,
    /// 下一条报价 ID（单调递增）.
    pub next_bid_id: u64,
}

impl BidGenerator {
    /// 构造报价生成器（next_bid_id 从 1 开始）.
    pub fn new(
        strategy: BidStrategy,
        resource_id: u64,
        intent_source: Box<dyn BidIntentSource>,
        optimizer: Box<dyn BidOptimizer>,
        publisher: Box<dyn BidPublisher>,
    ) -> Self {
        Self {
            strategy,
            resource_id,
            intent_source,
            optimizer,
            publisher,
            next_bid_id: 1,
        }
    }

    /// 执行一次报价生成流水线（D1：sync，now_ms 参数注入）.
    ///
    /// 流程：输入校验 → 空电价短路 → 意图（失败回退规则策略）→ SOC 门控 →
    /// 优化（失败回退保守报价）→ 逐电价点生成报价 → 非空则发布.
    pub fn generate(
        &mut self,
        feed: &MarketFeed,
        soc: f32,
        capacity: f32,
        now_ms: u64,
    ) -> Result<Vec<Bid>, BidError> {
        if capacity <= 0.0 {
            return Err(BidError::InvalidInput);
        }
        if feed.prices.is_empty() {
            return Ok(Vec::new());
        }
        let intent = match self.intent_source.generate_intent(feed, soc) {
            Ok(i) => i,
            Err(_) => rule_intent(feed, soc, capacity, &self.strategy),
        };
        if intent.side == BidSide::Sell && soc < self.strategy.soc_threshold {
            return Ok(Vec::new());
        }
        let opt = match self.optimizer.optimize(&intent, feed, soc, capacity) {
            Ok(o) => o,
            Err(_) => conservative_optimize(&intent, &self.strategy),
        };
        let mut bids = Vec::new();
        for point in feed.prices.iter() {
            let bid = Bid {
                bid_id: self.next_bid_id,
                market_type: feed.market_type,
                resource_id: self.resource_id,
                price: compute_price(intent.side, point.price, opt.price_adjust),
                quantity: clamp_quantity(opt.quantity, self.strategy.max_quantity, capacity),
                side: intent.side,
                period: point.period,
                timestamp: now_ms,
            };
            self.next_bid_id += 1;
            bids.push(bid);
        }
        if !bids.is_empty() {
            self.publisher.publish_bids(&bids)?;
        }
        Ok(bids)
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::vec;
    use alloc::vec::Vec;

    use super::*;
    use crate::market_feed::{DrSignal, PricePoint};

    /// 辅助：默认报价策略（margin=0.1 / max=5.0 / threshold=0.2）.
    fn make_strategy() -> BidStrategy {
        BidStrategy {
            margin: 0.1,
            max_quantity: 5.0,
            soc_threshold: 0.2,
        }
    }

    /// 辅助：含 2 条电价点（峰 0.9 / 谷 0.3）的现货市场馈送.
    fn make_feed_2prices() -> MarketFeed {
        MarketFeed {
            market_type: MarketType::Spot,
            timestamp: 1000,
            prices: vec![
                PricePoint {
                    time: 1000,
                    price: 0.9,
                    period: Period::Peak,
                },
                PricePoint {
                    time: 2000,
                    price: 0.3,
                    period: Period::Valley,
                },
            ],
            dr_signals: Vec::new(),
        }
    }

    /// 辅助：组装 BidGenerator（resource_id=42，默认策略）.
    fn make_generator(
        intent: MockBidIntentSource,
        opt: MockBidOptimizer,
        pub_: MockBidPublisher,
    ) -> BidGenerator {
        BidGenerator::new(
            make_strategy(),
            42,
            Box::new(intent),
            Box::new(opt),
            Box::new(pub_),
        )
    }

    /// 辅助：Sell happy path 三件套 Mock.
    fn make_sell_generator() -> BidGenerator {
        make_generator(
            MockBidIntentSource::new(BidIntent {
                side: BidSide::Sell,
                target_quantity: 5.0,
            }),
            MockBidOptimizer::new(BidOptimization {
                price_adjust: 0.1,
                quantity: 5.0,
            }),
            MockBidPublisher::new(),
        )
    }

    // ===== T43: BidSide 默认值 Buy + 2 变体 Debug 非空 =====
    #[test]
    fn t43_bid_side_default_and_debug() {
        assert_eq!(BidSide::default(), BidSide::Buy);
        assert!(!format!("{:?}", BidSide::Buy).is_empty());
        assert!(!format!("{:?}", BidSide::Sell).is_empty());
    }

    // ===== T44: BidStrategy 默认全 0 + 显式构造字段断言 =====
    #[test]
    fn t44_bid_strategy_default_and_fields() {
        let d = BidStrategy::default();
        assert!((d.margin - 0.0).abs() < f32::EPSILON);
        assert!((d.max_quantity - 0.0).abs() < f32::EPSILON);
        assert!((d.soc_threshold - 0.0).abs() < f32::EPSILON);
        let s = BidStrategy {
            margin: 0.1,
            max_quantity: 5.0,
            soc_threshold: 0.2,
        };
        assert!((s.margin - 0.1).abs() < f32::EPSILON);
        assert!((s.max_quantity - 5.0).abs() < f32::EPSILON);
        assert!((s.soc_threshold - 0.2).abs() < f32::EPSILON);
    }

    // ===== T45: Bid::default 全零 + 默认枚举值 =====
    #[test]
    fn t45_bid_default() {
        let b = Bid::default();
        assert_eq!(b.bid_id, 0);
        assert_eq!(b.market_type, MarketType::Spot);
        assert_eq!(b.resource_id, 0);
        assert!((b.price - 0.0).abs() < f32::EPSILON);
        assert!((b.quantity - 0.0).abs() < f32::EPSILON);
        assert_eq!(b.side, BidSide::Buy);
        assert_eq!(b.period, Period::Flat);
        assert_eq!(b.timestamp, 0);
    }

    // ===== T46: Bid 8 字段显式构造与访问 =====
    #[test]
    fn t46_bid_fields() {
        let b = Bid {
            bid_id: 7,
            market_type: MarketType::Spot,
            resource_id: 42,
            price: 1.0,
            quantity: 5.0,
            side: BidSide::Sell,
            period: Period::Peak,
            timestamp: 1000,
        };
        assert_eq!(b.bid_id, 7);
        assert_eq!(b.market_type, MarketType::Spot);
        assert_eq!(b.resource_id, 42);
        assert!((b.price - 1.0).abs() < f32::EPSILON);
        assert!((b.quantity - 5.0).abs() < f32::EPSILON);
        assert_eq!(b.side, BidSide::Sell);
        assert_eq!(b.period, Period::Peak);
        assert_eq!(b.timestamp, 1000);
    }

    // ===== T47: Bid 是 Copy =====
    #[test]
    fn t47_bid_copy() {
        let b1 = Bid {
            bid_id: 7,
            market_type: MarketType::Spot,
            resource_id: 42,
            price: 1.0,
            quantity: 5.0,
            side: BidSide::Sell,
            period: Period::Peak,
            timestamp: 1000,
        };
        let b2 = b1;
        assert_eq!(b2, b1);
    }

    // ===== T48: BidIntent / BidOptimization 构造与字段访问 =====
    #[test]
    fn t48_intent_and_optimization_fields() {
        let intent = BidIntent {
            side: BidSide::Sell,
            target_quantity: 2.0,
        };
        assert_eq!(intent.side, BidSide::Sell);
        assert!((intent.target_quantity - 2.0).abs() < f32::EPSILON);
        let opt = BidOptimization {
            price_adjust: 0.1,
            quantity: 3.0,
        };
        assert!((opt.price_adjust - 0.1).abs() < f32::EPSILON);
        assert!((opt.quantity - 3.0).abs() < f32::EPSILON);
    }

    // ===== T49: BidError 4 变体 PartialEq + Debug 非空 =====
    #[test]
    fn t49_bid_error_variants() {
        assert_eq!(BidError::InvalidInput, BidError::InvalidInput);
        assert_eq!(BidError::IntentFailed, BidError::IntentFailed);
        assert_eq!(BidError::OptimizeFailed, BidError::OptimizeFailed);
        assert_eq!(BidError::PublishFailed, BidError::PublishFailed);
        assert_ne!(BidError::InvalidInput, BidError::IntentFailed);
        assert_ne!(BidError::IntentFailed, BidError::OptimizeFailed);
        assert_ne!(BidError::OptimizeFailed, BidError::PublishFailed);
        assert_ne!(BidError::InvalidInput, BidError::PublishFailed);
        assert!(!format!("{:?}", BidError::InvalidInput).is_empty());
        assert!(!format!("{:?}", BidError::IntentFailed).is_empty());
        assert!(!format!("{:?}", BidError::OptimizeFailed).is_empty());
        assert!(!format!("{:?}", BidError::PublishFailed).is_empty());
    }

    // ===== T50: MockBidIntentSource::new 返回预置意图 =====
    #[test]
    fn t50_mock_intent_source_new() {
        let intent = BidIntent {
            side: BidSide::Sell,
            target_quantity: 2.0,
        };
        let mut src = MockBidIntentSource::new(intent);
        let feed = make_feed_2prices();
        assert_eq!(src.generate_intent(&feed, 0.5), Ok(intent));
    }

    // ===== T51: MockBidIntentSource 恒失败与缺省（next=None）均返回 IntentFailed =====
    #[test]
    fn t51_mock_intent_source_failing_and_default() {
        let feed = make_feed_2prices();
        let mut failing = MockBidIntentSource::new_failing();
        assert_eq!(
            failing.generate_intent(&feed, 0.5),
            Err(BidError::IntentFailed)
        );
        let mut dflt = MockBidIntentSource::default();
        assert_eq!(
            dflt.generate_intent(&feed, 0.5),
            Err(BidError::IntentFailed)
        );
    }

    // ===== T52: MockBidIntentSource::with_intent builder =====
    #[test]
    fn t52_mock_intent_source_with_intent() {
        let intent = BidIntent {
            side: BidSide::Buy,
            target_quantity: 1.5,
        };
        let mut src = MockBidIntentSource::default().with_intent(intent);
        let feed = make_feed_2prices();
        assert_eq!(src.generate_intent(&feed, 0.5), Ok(intent));
    }

    // ===== T53: MockBidOptimizer::new 返回预置优化结果 =====
    #[test]
    fn t53_mock_optimizer_new() {
        let opt = BidOptimization {
            price_adjust: 0.1,
            quantity: 3.0,
        };
        let mut optimizer = MockBidOptimizer::new(opt);
        let feed = make_feed_2prices();
        let intent = BidIntent {
            side: BidSide::Sell,
            target_quantity: 3.0,
        };
        assert_eq!(optimizer.optimize(&intent, &feed, 0.5, 10.0), Ok(opt));
    }

    // ===== T54: MockBidOptimizer 恒失败与缺省均返回 OptimizeFailed =====
    #[test]
    fn t54_mock_optimizer_failing_and_default() {
        let feed = make_feed_2prices();
        let intent = BidIntent {
            side: BidSide::Sell,
            target_quantity: 3.0,
        };
        let mut failing = MockBidOptimizer::new_failing();
        assert_eq!(
            failing.optimize(&intent, &feed, 0.5, 10.0),
            Err(BidError::OptimizeFailed)
        );
        let mut dflt = MockBidOptimizer::default();
        assert_eq!(
            dflt.optimize(&intent, &feed, 0.5, 10.0),
            Err(BidError::OptimizeFailed)
        );
    }

    // ===== T55: MockBidPublisher 记录已发布报价 =====
    #[test]
    fn t55_mock_publisher_records() {
        let mut publisher = MockBidPublisher::new();
        let bids = vec![Bid::default(), Bid::default()];
        assert_eq!(publisher.publish_bids(&bids), Ok(()));
        assert_eq!(publisher.published.len(), 2);
    }

    // ===== T56: MockBidPublisher 恒失败 → PublishFailed 且不记录 =====
    #[test]
    fn t56_mock_publisher_failing() {
        let mut publisher = MockBidPublisher::new_failing();
        let bids = vec![Bid::default()];
        assert_eq!(publisher.publish_bids(&bids), Err(BidError::PublishFailed));
        assert!(publisher.published.is_empty());
    }

    // ===== T57: rule_intent —— max_quantity 更小时取 max_quantity =====
    #[test]
    fn t57_rule_intent_max_quantity_bound() {
        let feed = make_feed_2prices();
        let s = BidStrategy {
            margin: 0.1,
            max_quantity: 3.0,
            soc_threshold: 0.2,
        };
        let intent = rule_intent(&feed, 0.5, 10.0, &s);
        assert_eq!(intent.side, BidSide::Sell);
        assert!((intent.target_quantity - 3.0).abs() < f32::EPSILON);
    }

    // ===== T58: rule_intent —— capacity*soc 更小 / 负 SOC 归零 =====
    #[test]
    fn t58_rule_intent_soc_bound_and_negative_soc() {
        let feed = make_feed_2prices();
        let s = BidStrategy {
            margin: 0.1,
            max_quantity: 3.0,
            soc_threshold: 0.2,
        };
        let intent = rule_intent(&feed, 0.2, 10.0, &s);
        assert!((intent.target_quantity - 2.0).abs() < f32::EPSILON);
        let neg = rule_intent(&feed, -0.5, 10.0, &s);
        assert!((neg.target_quantity - 0.0).abs() < f32::EPSILON);
    }

    // ===== T59: conservative_optimize —— margin 透传 + 电量截断 =====
    #[test]
    fn t59_conservative_optimize() {
        let s = make_strategy();
        let i1 = BidIntent {
            side: BidSide::Sell,
            target_quantity: 2.0,
        };
        let o1 = conservative_optimize(&i1, &s);
        assert!((o1.price_adjust - 0.1).abs() < f32::EPSILON);
        assert!((o1.quantity - 2.0).abs() < f32::EPSILON);
        let i2 = BidIntent {
            side: BidSide::Sell,
            target_quantity: 8.0,
        };
        let o2 = conservative_optimize(&i2, &s);
        assert!((o2.quantity - 5.0).abs() < f32::EPSILON);
    }

    // ===== T60: compute_price —— Sell 加价 / Buy 减价 / Buy 地板 0 =====
    #[test]
    fn t60_compute_price() {
        assert!((compute_price(BidSide::Sell, 0.9, 0.1) - 1.0).abs() < 1e-6);
        assert!((compute_price(BidSide::Buy, 0.3, 0.1) - 0.2).abs() < 1e-6);
        assert!((compute_price(BidSide::Buy, 0.05, 0.1) - 0.0).abs() < f32::EPSILON);
    }

    // ===== T61: clamp_quantity —— max / capacity / 负值归零 =====
    #[test]
    fn t61_clamp_quantity() {
        assert!((clamp_quantity(8.0, 5.0, 10.0) - 5.0).abs() < f32::EPSILON);
        assert!((clamp_quantity(8.0, 10.0, 4.0) - 4.0).abs() < f32::EPSILON);
        assert!((clamp_quantity(-1.0, 5.0, 10.0) - 0.0).abs() < f32::EPSILON);
    }

    // ===== T62: generate 容量非正 → InvalidInput =====
    #[test]
    fn t62_generate_invalid_capacity() {
        let feed = make_feed_2prices();
        let mut gen = make_sell_generator();
        assert_eq!(
            gen.generate(&feed, 0.8, 0.0, 5000),
            Err(BidError::InvalidInput)
        );
        assert_eq!(
            gen.generate(&feed, 0.8, -1.0, 5000),
            Err(BidError::InvalidInput)
        );
    }

    // ===== T63: generate 电价点为空（含 DR 信号）→ 空 Vec =====
    #[test]
    fn t63_generate_empty_prices() {
        let feed = MarketFeed {
            market_type: MarketType::Spot,
            timestamp: 1000,
            prices: Vec::new(),
            dr_signals: vec![DrSignal::default()],
        };
        let mut gen = make_sell_generator();
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        assert!(result.unwrap_or_default().is_empty());
    }

    // ===== T64: generate happy path —— Sell 2 条报价 =====
    #[test]
    fn t64_generate_happy_path_sell() {
        let feed = make_feed_2prices();
        let mut gen = make_sell_generator();
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert_eq!(bids.len(), 2);
        assert!((bids[0].price - 1.0).abs() < 1e-6);
        assert!((bids[1].price - 0.4).abs() < 1e-6);
        assert!((bids[0].quantity - 5.0).abs() < f32::EPSILON);
        assert!((bids[1].quantity - 5.0).abs() < f32::EPSILON);
        assert_eq!(bids[0].period, Period::Peak);
        assert_eq!(bids[1].period, Period::Valley);
        assert_eq!(bids[0].market_type, MarketType::Spot);
        assert_eq!(bids[0].side, BidSide::Sell);
        assert_eq!(bids[1].side, BidSide::Sell);
    }

    // ===== T65: bid_id 跨次单调递增 =====
    #[test]
    fn t65_bid_id_increments() {
        let feed = make_feed_2prices();
        let mut gen = make_sell_generator();
        let r1 = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(r1.is_ok());
        let b1 = r1.unwrap_or_default();
        assert_eq!(b1[0].bid_id, 1);
        assert_eq!(b1[1].bid_id, 2);
        let feed1 = MarketFeed {
            market_type: MarketType::Spot,
            timestamp: 2000,
            prices: vec![PricePoint {
                time: 2000,
                price: 0.5,
                period: Period::Flat,
            }],
            dr_signals: Vec::new(),
        };
        let r2 = gen.generate(&feed1, 0.8, 10.0, 6000);
        assert!(r2.is_ok());
        let b2 = r2.unwrap_or_default();
        assert_eq!(b2[0].bid_id, 3);
    }

    // ===== T66: timestamp（now_ms）与 resource_id 传播 =====
    #[test]
    fn t66_timestamp_and_resource_id_propagation() {
        let feed = make_feed_2prices();
        let mut gen = make_sell_generator();
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert_eq!(bids[0].timestamp, 5000);
        assert_eq!(bids[1].timestamp, 5000);
        assert_eq!(bids[0].resource_id, 42);
        assert_eq!(bids[1].resource_id, 42);
    }

    // ===== T67: Buy 路径 —— 减价 =====
    #[test]
    fn t67_generate_buy_path() {
        let feed = make_feed_2prices();
        let mut gen = make_generator(
            MockBidIntentSource::new(BidIntent {
                side: BidSide::Buy,
                target_quantity: 2.0,
            }),
            MockBidOptimizer::new(BidOptimization {
                price_adjust: 0.1,
                quantity: 2.0,
            }),
            MockBidPublisher::new(),
        );
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert_eq!(bids.len(), 2);
        assert!((bids[0].price - 0.8).abs() < 1e-6);
        assert_eq!(bids[0].side, BidSide::Buy);
    }

    // ===== T68: Buy 价格地板 0 =====
    #[test]
    fn t68_generate_buy_price_floor() {
        let feed = MarketFeed {
            market_type: MarketType::Spot,
            timestamp: 1000,
            prices: vec![PricePoint {
                time: 1000,
                price: 0.05,
                period: Period::Valley,
            }],
            dr_signals: Vec::new(),
        };
        let mut gen = make_generator(
            MockBidIntentSource::new(BidIntent {
                side: BidSide::Buy,
                target_quantity: 1.0,
            }),
            MockBidOptimizer::new(BidOptimization {
                price_adjust: 0.1,
                quantity: 1.0,
            }),
            MockBidPublisher::new(),
        );
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert!((bids[0].price - 0.0).abs() < f32::EPSILON);
    }

    // ===== T69: 电量截断 max_quantity =====
    #[test]
    fn t69_generate_quantity_clamp_max() {
        let feed = make_feed_2prices();
        let mut gen = make_generator(
            MockBidIntentSource::new(BidIntent {
                side: BidSide::Sell,
                target_quantity: 8.0,
            }),
            MockBidOptimizer::new(BidOptimization {
                price_adjust: 0.1,
                quantity: 8.0,
            }),
            MockBidPublisher::new(),
        );
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert!((bids[0].quantity - 5.0).abs() < f32::EPSILON);
    }

    // ===== T70: 电量截断 capacity =====
    #[test]
    fn t70_generate_quantity_clamp_capacity() {
        let feed = make_feed_2prices();
        let strategy = BidStrategy {
            margin: 0.1,
            max_quantity: 10.0,
            soc_threshold: 0.2,
        };
        let mut gen = BidGenerator::new(
            strategy,
            42,
            Box::new(MockBidIntentSource::new(BidIntent {
                side: BidSide::Sell,
                target_quantity: 8.0,
            })),
            Box::new(MockBidOptimizer::new(BidOptimization {
                price_adjust: 0.1,
                quantity: 8.0,
            })),
            Box::new(MockBidPublisher::new()),
        );
        let result = gen.generate(&feed, 0.8, 4.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert!((bids[0].quantity - 4.0).abs() < f32::EPSILON);
    }

    // ===== T71: SOC 门控 —— Sell 且 SOC < 门限 → 空（门控先于 optimize）=====
    #[test]
    fn t71_generate_soc_gate_blocks_sell() {
        let feed = make_feed_2prices();
        // 优化器恒失败：若门控未先生效，会走保守回退仍生成非空 bids
        let mut gen = make_generator(
            MockBidIntentSource::new(BidIntent {
                side: BidSide::Sell,
                target_quantity: 5.0,
            }),
            MockBidOptimizer::new_failing(),
            MockBidPublisher::new(),
        );
        let result = gen.generate(&feed, 0.1, 10.0, 5000);
        assert!(result.is_ok());
        assert!(result.unwrap_or_default().is_empty());
    }

    // ===== T72: SOC 边界 —— SOC == 门限 → 正常生成 =====
    #[test]
    fn t72_generate_soc_gate_boundary() {
        let feed = make_feed_2prices();
        let mut gen = make_sell_generator();
        let result = gen.generate(&feed, 0.2, 10.0, 5000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap_or_default().len(), 2);
    }

    // ===== T73: Buy 不受 SOC 门控 =====
    #[test]
    fn t73_generate_buy_not_gated() {
        let feed = make_feed_2prices();
        let mut gen = make_generator(
            MockBidIntentSource::new(BidIntent {
                side: BidSide::Buy,
                target_quantity: 2.0,
            }),
            MockBidOptimizer::new(BidOptimization {
                price_adjust: 0.1,
                quantity: 2.0,
            }),
            MockBidPublisher::new(),
        );
        let result = gen.generate(&feed, 0.1, 10.0, 5000);
        assert!(result.is_ok());
        assert_eq!(result.unwrap_or_default().len(), 2);
    }

    // ===== T74: 意图失败 → 规则回退（Sell）=====
    #[test]
    fn t74_generate_intent_failure_rule_fallback() {
        let feed = make_feed_2prices();
        let mut gen = make_generator(
            MockBidIntentSource::new_failing(),
            MockBidOptimizer::new(BidOptimization {
                price_adjust: 0.1,
                quantity: 5.0,
            }),
            MockBidPublisher::new(),
        );
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0].side, BidSide::Sell);
    }

    // ===== T75: 优化失败 → 保守回退（margin 加价 + 电量截断）=====
    #[test]
    fn t75_generate_optimize_failure_conservative_fallback() {
        let feed = make_feed_2prices();
        let mut gen = make_generator(
            MockBidIntentSource::new(BidIntent {
                side: BidSide::Sell,
                target_quantity: 2.0,
            }),
            MockBidOptimizer::new_failing(),
            MockBidPublisher::new(),
        );
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert!((bids[0].price - 1.0).abs() < 1e-6);
        assert!((bids[0].quantity - 2.0).abs() < f32::EPSILON);
    }

    // ===== T76: 意图 + 优化双失败 → 规则意图 + 保守优化仍生成 =====
    #[test]
    fn t76_generate_double_failure_still_generates() {
        let feed = make_feed_2prices();
        let mut gen = make_generator(
            MockBidIntentSource::new_failing(),
            MockBidOptimizer::new_failing(),
            MockBidPublisher::new(),
        );
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0].side, BidSide::Sell);
        assert!((bids[0].price - 1.0).abs() < 1e-6);
        assert!((bids[1].price - 0.4).abs() < 1e-6);
    }

    // ===== T77: 发布失败 → PublishFailed 传播 =====
    #[test]
    fn t77_generate_publish_failure_propagates() {
        let feed = make_feed_2prices();
        let mut gen = make_generator(
            MockBidIntentSource::new(BidIntent {
                side: BidSide::Sell,
                target_quantity: 5.0,
            }),
            MockBidOptimizer::new(BidOptimization {
                price_adjust: 0.1,
                quantity: 5.0,
            }),
            MockBidPublisher::new_failing(),
        );
        assert_eq!(
            gen.generate(&feed, 0.8, 10.0, 5000),
            Err(BidError::PublishFailed)
        );
    }

    // ===== T78: 规则回退后 SOC 门控仍生效 =====
    #[test]
    fn t78_generate_rule_fallback_still_gated() {
        let feed = make_feed_2prices();
        let mut gen = make_generator(
            MockBidIntentSource::new_failing(),
            MockBidOptimizer::new(BidOptimization {
                price_adjust: 0.1,
                quantity: 5.0,
            }),
            MockBidPublisher::new(),
        );
        let result = gen.generate(&feed, 0.1, 10.0, 5000);
        assert!(result.is_ok());
        assert!(result.unwrap_or_default().is_empty());
    }

    // ===== T79: market_type 传播（DemandResponse）=====
    #[test]
    fn t79_generate_market_type_propagation() {
        let feed = MarketFeed {
            market_type: MarketType::DemandResponse,
            timestamp: 1000,
            prices: vec![PricePoint {
                time: 1000,
                price: 0.9,
                period: Period::Peak,
            }],
            dr_signals: Vec::new(),
        };
        let mut gen = make_sell_generator();
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        let bids = result.unwrap_or_default();
        assert_eq!(bids[0].market_type, MarketType::DemandResponse);
    }

    // ===== T80: 发布调用证据 —— Ok 且非空即证明 publish_bids 已执行 =====
    #[test]
    fn t80_publish_called_evidence() {
        // 注：publisher 装箱为 Box<dyn BidPublisher> 后无法读取 Mock 内部 published，
        // 以「generate 返回 Ok 且 bids 非空」作为 publish_bids 已执行的旁证
        // （若 publish 未执行或失败则不可能返回 Ok 非空）。与 v0.85.0 T36 一致。
        let feed = make_feed_2prices();
        let mut gen = make_sell_generator();
        let result = gen.generate(&feed, 0.8, 10.0, 5000);
        assert!(result.is_ok());
        assert!(!result.unwrap_or_default().is_empty());
    }
}
