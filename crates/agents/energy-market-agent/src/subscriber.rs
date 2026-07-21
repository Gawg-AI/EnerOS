//! v0.85.0 市场数据订阅 —— 订阅管理.
//!
//! 本模块实现 v0.85.0 市场数据订阅版本的订阅管理核心：轮询门控（`poll_interval_ms`
//! 防止过频拉取）、缓存降级（数据源失败时回退 last-good 缓存）、订阅过滤（仅处理
//! 已订阅的 `MarketType`，未订阅类型不缓存不发布）、发布抽象（电价点与 DR 信号分
//! 通道发布）。
//!
//! # 偏差声明（D8~D11）
//!
//! - **D8**：蓝图 `MarketSource { HttpApi, File, Simulated }` 枚举 → `MarketFeedSource`
//!   trait + `MockMarketFeedSource`。no_std 无 HTTP/文件系统；trait 抽象数据源（沿用
//!   v0.82.0 D5 `GridSampler` 模式），真实 HTTP/File 适配器后续注入。
//! - **D9**：蓝图 `run(&mut self, bus: &DdsNode)` + `dds::publish` → `MarketFeedPublisher`
//!   trait + `MockMarketFeedPublisher`。避免 `eneros-agent-bus-dds` 重依赖（沿用
//!   v0.82.0 `GridPublisher` 模式），DDS 适配器后续注入。
//! - **D10**：蓝图 `MarketCache` 引用但未定义 → `MarketFeedCache` 结构体
//!   （`last: Option<MarketFeed>` + store/get/is_empty）。蓝图 §4.4 "接口超时 → 使用
//!   缓存" 需要缓存语义；最小实现：单条 last-good。
//! - **D11**：蓝图轮询周期 60s → `poll_interval_ms: u64` 构造参数 +
//!   `last_poll_ms: Option<u64>` 门控。60s 作为推荐默认值；`Option<u64>` 使首次
//!   poll 立即执行。

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::market_feed::{MarketError, MarketFeed, MarketType};

/// 市场数据源抽象（D8：trait 替代 MarketSource 枚举；不要求 Send + Sync，no_std 单线程）.
pub trait MarketFeedSource {
    /// 拉取一次市场数据（同步语义，now_ms 参数注入）.
    fn fetch(&mut self, now_ms: u64) -> Result<MarketFeed, MarketError>;
}

/// Mock 市场数据源：返回预置 feed 或恒失败.
#[derive(Debug, Clone, Default)]
pub struct MockMarketFeedSource {
    /// 下一次 fetch 返回的 feed（fail=false 且为 None 时返回 SourceFailed）.
    next_feed: Option<MarketFeed>,
    /// 恒失败开关.
    fail: bool,
}

impl MockMarketFeedSource {
    /// 构造返回指定 feed 的 Mock 源.
    pub fn new(feed: MarketFeed) -> Self {
        Self {
            next_feed: Some(feed),
            fail: false,
        }
    }

    /// 构造恒失败的 Mock 源.
    pub fn new_failing() -> Self {
        Self {
            next_feed: None,
            fail: true,
        }
    }

    /// Builder：设置下一次 fetch 返回的 feed.
    pub fn with_feed(mut self, feed: MarketFeed) -> Self {
        self.next_feed = Some(feed);
        self
    }
}

impl MarketFeedSource for MockMarketFeedSource {
    fn fetch(&mut self, _now_ms: u64) -> Result<MarketFeed, MarketError> {
        if self.fail {
            return Err(MarketError::SourceFailed);
        }
        match &self.next_feed {
            Some(feed) => Ok(feed.clone()),
            None => Err(MarketError::SourceFailed),
        }
    }
}

/// 市场数据发布抽象（D9：trait 替代 DdsNode；不要求 Send + Sync，no_std 单线程）.
pub trait MarketFeedPublisher {
    /// 发布电价点列表.
    fn publish_prices(&mut self, feed: &MarketFeed) -> Result<(), MarketError>;
    /// 发布 DR 信号列表.
    fn publish_dr_signals(&mut self, feed: &MarketFeed) -> Result<(), MarketError>;
}

/// Mock 市场发布器：记录已发布 feed 或恒失败.
#[derive(Debug, Clone, Default)]
pub struct MockMarketFeedPublisher {
    /// 已发布的 feed 记录.
    published: Vec<MarketFeed>,
    /// 恒失败开关.
    fail: bool,
}

impl MockMarketFeedPublisher {
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

impl MarketFeedPublisher for MockMarketFeedPublisher {
    fn publish_prices(&mut self, feed: &MarketFeed) -> Result<(), MarketError> {
        if self.fail {
            return Err(MarketError::PublishFailed);
        }
        self.published.push(feed.clone());
        Ok(())
    }

    fn publish_dr_signals(&mut self, feed: &MarketFeed) -> Result<(), MarketError> {
        if self.fail {
            return Err(MarketError::PublishFailed);
        }
        self.published.push(feed.clone());
        Ok(())
    }
}

/// 市场数据 last-good 缓存（D10：蓝图 §4.4 "接口超时 → 使用缓存"）.
#[derive(Debug, Clone, Default)]
pub struct MarketFeedCache {
    /// 最近一次成功获取的 feed.
    last: Option<MarketFeed>,
}

impl MarketFeedCache {
    /// 构造空缓存.
    pub fn new() -> Self {
        Self { last: None }
    }

    /// 存储一条 feed（覆盖旧值）.
    pub fn store(&mut self, feed: MarketFeed) {
        self.last = Some(feed);
    }

    /// 获取缓存的 feed 引用.
    pub fn get(&self) -> Option<&MarketFeed> {
        self.last.as_ref()
    }

    /// 缓存是否为空.
    pub fn is_empty(&self) -> bool {
        self.last.is_none()
    }
}

/// 市场订阅器：订阅过滤 + 轮询门控 + 缓存降级 + 发布编排.
pub struct MarketSubscriber {
    /// 数据源（D8）.
    source: Box<dyn MarketFeedSource>,
    /// 发布器（D9）.
    publisher: Box<dyn MarketFeedPublisher>,
    /// last-good 缓存（D10）.
    cache: MarketFeedCache,
    /// 已订阅的市场类型列表.
    subscribed: Vec<MarketType>,
    /// 轮询间隔（ms，D11）.
    poll_interval_ms: u64,
    /// 上次轮询时间（ms；None 表示从未轮询，首次 poll 立即执行）.
    last_poll_ms: Option<u64>,
}

impl MarketSubscriber {
    /// 构造订阅器（subscribed 空 / cache 空 / last_poll_ms None）.
    pub fn new(
        source: Box<dyn MarketFeedSource>,
        publisher: Box<dyn MarketFeedPublisher>,
        poll_interval_ms: u64,
    ) -> Self {
        Self {
            source,
            publisher,
            cache: MarketFeedCache::new(),
            subscribed: Vec::new(),
            poll_interval_ms,
            last_poll_ms: None,
        }
    }

    /// 订阅一个市场类型（幂等：已存在则不重复追加）.
    pub fn subscribe(&mut self, mt: MarketType) {
        if !self.subscribed.contains(&mt) {
            self.subscribed.push(mt);
        }
    }

    /// 是否已订阅指定市场类型.
    pub fn is_subscribed(&self, mt: MarketType) -> bool {
        self.subscribed.contains(&mt)
    }

    /// 获取 last-good 缓存引用.
    pub fn cache(&self) -> &MarketFeedCache {
        &self.cache
    }

    /// 轮询一次市场数据.
    ///
    /// 流程（严格按序）：
    /// 1. 轮询门控（D11）：距上次 poll 不足 `poll_interval_ms` → `Ok(None)`；
    /// 2. 记录本次 poll 时间；
    /// 3. fetch 失败 → 有缓存返回缓存克隆（缓存降级，蓝图 §4.4），无缓存返回
    ///    `Err(MarketError::SourceFailed)`；
    /// 4. fetch 成功 → 未订阅该类型返回 `Ok(None)`（不缓存不发布）；已订阅则存入
    ///    缓存，prices / dr_signals 非空时分别发布（失败传播
    ///    `Err(MarketError::PublishFailed)`），最后返回 `Ok(Some(feed))`。
    pub fn poll(&mut self, now_ms: u64) -> Result<Option<MarketFeed>, MarketError> {
        // 1. 轮询门控（D11）
        if let Some(last) = self.last_poll_ms {
            if now_ms - last < self.poll_interval_ms {
                return Ok(None);
            }
        }
        // 2. 记录轮询时间
        self.last_poll_ms = Some(now_ms);
        // 3. 拉取数据
        match self.source.fetch(now_ms) {
            Err(_) => {
                // 缓存降级（蓝图 §4.4 "接口超时 → 使用缓存"）
                match self.cache.get() {
                    Some(cached) => Ok(Some(cached.clone())),
                    None => Err(MarketError::SourceFailed),
                }
            }
            Ok(feed) => {
                // a. 订阅过滤：未订阅类型不缓存不发布
                if !self.is_subscribed(feed.market_type) {
                    return Ok(None);
                }
                // b. 更新 last-good 缓存
                self.cache.store(feed.clone());
                // c. 发布电价点（失败传播）
                if !feed.prices.is_empty() {
                    self.publisher.publish_prices(&feed)?;
                }
                // d. 发布 DR 信号（失败传播）
                if !feed.dr_signals.is_empty() {
                    self.publisher.publish_dr_signals(&feed)?;
                }
                // e. 返回本次 feed
                Ok(Some(feed))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::vec;

    use super::*;
    use crate::market_feed::{DrSignal, Period, PricePoint};

    /// 辅助：构造含 1 个 PricePoint + 1 个 DrSignal 的 feed（便于发布分支测试）.
    fn make_feed(mt: MarketType) -> MarketFeed {
        MarketFeed {
            market_type: mt,
            timestamp: 1000,
            prices: vec![PricePoint {
                time: 1000,
                price: 0.85,
                period: Period::Peak,
            }],
            dr_signals: vec![DrSignal {
                event_id: 1,
                target_mw: 2.5,
                start: 1000,
                end: 2000,
                reward: 500.0,
            }],
        }
    }

    // ===== T27: MockMarketFeedSource::new fetch 返回预置 feed =====
    #[test]
    fn t27_mock_source_new_fetch_ok() {
        let feed = make_feed(MarketType::Spot);
        let mut src = MockMarketFeedSource::new(feed.clone());
        let got = src.fetch(0).unwrap();
        assert_eq!(got, feed);
    }

    // ===== T28: MockMarketFeedSource::new_failing fetch 恒失败 =====
    #[test]
    fn t28_mock_source_failing_fetch_err() {
        let mut src = MockMarketFeedSource::new_failing();
        assert_eq!(src.fetch(0), Err(MarketError::SourceFailed));
    }

    // ===== T29: MockMarketFeedSource::default（next_feed=None）fetch 失败 =====
    #[test]
    fn t29_mock_source_default_fetch_err() {
        let mut src = MockMarketFeedSource::default();
        assert_eq!(src.fetch(0), Err(MarketError::SourceFailed));
    }

    // ===== T30: with_feed builder 后 fetch 成功 =====
    #[test]
    fn t30_mock_source_with_feed_fetch_ok() {
        let feed = make_feed(MarketType::AncillaryService);
        let mut src = MockMarketFeedSource::default().with_feed(feed.clone());
        let got = src.fetch(100).unwrap();
        assert_eq!(got, feed);
    }

    // ===== T31: MockMarketFeedPublisher::new 发布成功并记录 =====
    #[test]
    fn t31_mock_publisher_publish_ok() {
        let mut mock = MockMarketFeedPublisher::new();
        assert!(mock.published.is_empty());
        let feed = make_feed(MarketType::Spot);
        assert_eq!(mock.publish_prices(&feed), Ok(()));
        assert_eq!(mock.published.len(), 1);
    }

    // ===== T32: MockMarketFeedPublisher::new_failing 发布失败不记录 =====
    #[test]
    fn t32_mock_publisher_failing_publish_err() {
        let mut mock = MockMarketFeedPublisher::new_failing();
        let feed = make_feed(MarketType::Spot);
        assert_eq!(mock.publish_prices(&feed), Err(MarketError::PublishFailed));
        assert!(mock.published.is_empty());
    }

    // ===== T33: MarketFeedCache new/store/get/is_empty =====
    #[test]
    fn t33_cache_store_and_get() {
        let mut cache = MarketFeedCache::new();
        assert!(cache.is_empty());
        assert!(cache.get().is_none());
        cache.store(make_feed(MarketType::Spot));
        assert!(!cache.is_empty());
        assert!(cache.get().is_some());
    }

    // ===== T34: MarketSubscriber::new 初始状态（首次 poll 立即 fetch） =====
    #[test]
    fn t34_subscriber_new_initial_state() {
        let source: Box<dyn MarketFeedSource> =
            Box::new(MockMarketFeedSource::new(make_feed(MarketType::Spot)));
        let publisher: Box<dyn MarketFeedPublisher> = Box::new(MockMarketFeedPublisher::new());
        let mut sub = MarketSubscriber::new(source, publisher, 60_000);
        assert!(!sub.is_subscribed(MarketType::Spot));
        assert!(!sub.is_subscribed(MarketType::AncillaryService));
        assert!(!sub.is_subscribed(MarketType::DemandResponse));
        assert!(sub.cache().is_empty());
        // last_poll_ms 为私有字段：通过行为验证——首次 poll 不受门控、立即 fetch
        sub.subscribe(MarketType::Spot);
        assert!(sub.poll(0).unwrap().is_some());
    }

    // ===== T35: subscribe 幂等 =====
    #[test]
    fn t35_subscribe_idempotent() {
        let source: Box<dyn MarketFeedSource> =
            Box::new(MockMarketFeedSource::new(make_feed(MarketType::Spot)));
        let publisher: Box<dyn MarketFeedPublisher> = Box::new(MockMarketFeedPublisher::new());
        let mut sub = MarketSubscriber::new(source, publisher, 60_000);
        sub.subscribe(MarketType::Spot);
        assert!(sub.is_subscribed(MarketType::Spot));
        // 重复订阅幂等：不重复 push
        sub.subscribe(MarketType::Spot);
        assert_eq!(sub.subscribed.len(), 1);
        // 间接验证：再订阅 DemandResponse 后两者均为 true
        sub.subscribe(MarketType::DemandResponse);
        assert!(sub.is_subscribed(MarketType::Spot));
        assert!(sub.is_subscribed(MarketType::DemandResponse));
        assert!(!sub.is_subscribed(MarketType::AncillaryService));
        assert_eq!(sub.subscribed.len(), 2);
    }

    // ===== T36: 首次 poll 拉取成功 + 缓存 + 发布路径执行 =====
    #[test]
    fn t36_first_poll_fetches() {
        let feed = make_feed(MarketType::Spot);
        let source: Box<dyn MarketFeedSource> = Box::new(MockMarketFeedSource::new(feed.clone()));
        let publisher: Box<dyn MarketFeedPublisher> = Box::new(MockMarketFeedPublisher::new());
        let mut sub = MarketSubscriber::new(source, publisher, 60_000);
        sub.subscribe(MarketType::Spot);
        let got = sub.poll(0).unwrap();
        assert_eq!(got, Some(feed));
        assert!(sub.cache().get().is_some());
        // 变通说明：publisher 被 Box<dyn MarketFeedPublisher> 类型擦除后无法回读
        // published 字段计数；feed 含 1 条 prices + 1 条 dr_signals，poll 返回
        // Ok(Some) 即证明两条发布路径均已执行（发布失败传播由 T42 验证）.
    }

    // ===== T37: 轮询门控：未到周期返回 Ok(None) =====
    #[test]
    fn t37_poll_interval_gate() {
        let source: Box<dyn MarketFeedSource> =
            Box::new(MockMarketFeedSource::new(make_feed(MarketType::Spot)));
        let publisher: Box<dyn MarketFeedPublisher> = Box::new(MockMarketFeedPublisher::new());
        let mut sub = MarketSubscriber::new(source, publisher, 60_000);
        sub.subscribe(MarketType::Spot);
        assert!(sub.poll(0).unwrap().is_some());
        // 30s < 60s：门控拦截，不重新 fetch
        assert_eq!(sub.poll(30_000), Ok(None));
    }

    // ===== T38: 过期间隔后重新 fetch =====
    #[test]
    fn t38_poll_after_interval_refetches() {
        let feed = make_feed(MarketType::Spot);
        let source: Box<dyn MarketFeedSource> = Box::new(MockMarketFeedSource::new(feed.clone()));
        let publisher: Box<dyn MarketFeedPublisher> = Box::new(MockMarketFeedPublisher::new());
        let mut sub = MarketSubscriber::new(source, publisher, 60_000);
        sub.subscribe(MarketType::Spot);
        assert!(sub.poll(0).unwrap().is_some());
        // 60s 到达周期：门控放行，重新 fetch
        assert_eq!(sub.poll(60_000), Ok(Some(feed)));
    }

    // ===== T39: source 失败时缓存降级（last-good） =====
    //
    // 变通说明：MockMarketFeedSource 装箱为 Box<dyn MarketFeedSource> 后无法修改
    // fail 字段切换成功/失败，故在测试内定义 FlappingSource（首次 fetch 成功、之后
    // 恒失败）模拟"先成功后失败"场景；fetch 签名为 &mut self，普通 bool 字段即可
    // 计数，无需内部可变性.
    struct FlappingSource {
        feed: MarketFeed,
        served: bool,
    }

    impl FlappingSource {
        fn new(feed: MarketFeed) -> Self {
            Self {
                feed,
                served: false,
            }
        }
    }

    impl MarketFeedSource for FlappingSource {
        fn fetch(&mut self, _now_ms: u64) -> Result<MarketFeed, MarketError> {
            if self.served {
                Err(MarketError::SourceFailed)
            } else {
                self.served = true;
                Ok(self.feed.clone())
            }
        }
    }

    #[test]
    fn t39_source_failure_degrades_to_cache() {
        let feed = make_feed(MarketType::Spot);
        let source: Box<dyn MarketFeedSource> = Box::new(FlappingSource::new(feed.clone()));
        let publisher: Box<dyn MarketFeedPublisher> = Box::new(MockMarketFeedPublisher::new());
        let mut sub = MarketSubscriber::new(source, publisher, 60_000);
        sub.subscribe(MarketType::Spot);
        // 首次成功：缓存已存
        assert_eq!(sub.poll(0), Ok(Some(feed.clone())));
        // 第二次 source 失败：返回 last-good 缓存
        assert_eq!(sub.poll(60_000), Ok(Some(feed)));
    }

    // ===== T40: 无缓存时 source 失败 → Err(SourceFailed) =====
    #[test]
    fn t40_source_failure_no_cache_errors() {
        let source: Box<dyn MarketFeedSource> = Box::new(MockMarketFeedSource::new_failing());
        let publisher: Box<dyn MarketFeedPublisher> = Box::new(MockMarketFeedPublisher::new());
        let mut sub = MarketSubscriber::new(source, publisher, 60_000);
        sub.subscribe(MarketType::Spot);
        assert_eq!(sub.poll(0), Err(MarketError::SourceFailed));
    }

    // ===== T41: 未订阅类型过滤（不缓存不发布） =====
    #[test]
    fn t41_unsubscribed_type_filtered() {
        let source: Box<dyn MarketFeedSource> = Box::new(MockMarketFeedSource::new(make_feed(
            MarketType::DemandResponse,
        )));
        let publisher: Box<dyn MarketFeedPublisher> = Box::new(MockMarketFeedPublisher::new());
        let mut sub = MarketSubscriber::new(source, publisher, 60_000);
        // 仅订阅 Spot，source 返回 DemandResponse feed
        sub.subscribe(MarketType::Spot);
        assert_eq!(sub.poll(0), Ok(None));
        assert!(sub.cache().is_empty());
    }

    // ===== T42: 发布失败传播 Err(PublishFailed) =====
    #[test]
    fn t42_publish_failure_propagates() {
        let source: Box<dyn MarketFeedSource> =
            Box::new(MockMarketFeedSource::new(make_feed(MarketType::Spot)));
        let publisher: Box<dyn MarketFeedPublisher> =
            Box::new(MockMarketFeedPublisher::new_failing());
        let mut sub = MarketSubscriber::new(source, publisher, 60_000);
        sub.subscribe(MarketType::Spot);
        // feed 含 prices：publish_prices 失败传播
        assert_eq!(sub.poll(0), Err(MarketError::PublishFailed));
    }
}
