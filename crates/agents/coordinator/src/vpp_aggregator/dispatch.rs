//! `VppAggregator::aggregate`/`dispatch`/`market_bid` 实现（ARCH-G03：interface 与 implementation 分离）。

use super::*;

impl VppAggregator {
    /// 聚合容量画像（D4：aggregate_count += 1 后调内部免计数版）.
    pub fn aggregate(&mut self) -> VppProfile {
        self.aggregate_count += 1;
        self.compute_profile()
    }

    /// 同步在线资源为 DomainOptimizer 的单设备 EdgeBoxState（D8）.
    ///
    /// 先清后填：仅写入 `online && sanitize_capacity 有效` 的资源；
    /// box_id = device_id = resource_id，`p_min=0`、`p_max=box capacity=sanitize(available)`，
    /// `soc=1.0` 恒通过合格过滤，离线/无效 capacity 不写入。
    fn sync_boxes(&mut self) {
        self.optimizer.edge_boxes.clear();
        for (rid, r) in self.resources.iter() {
            if !r.online {
                continue;
            }
            if sanitize_capacity(r.capacity_mw).is_none() {
                continue;
            }
            let avail = sanitize_available(r.available_mw, r.capacity_mw);
            let mut devices = DevicePool::new();
            devices.add_device(
                *rid,
                DeviceCapability {
                    p_min: 0.0,
                    p_max: avail,
                    ramp_rate: sanitize_ramp(r.ramp_rate),
                    efficiency: sanitize_efficiency(r.efficiency),
                },
            );
            let mut socs = BTreeMap::new();
            socs.insert(*rid, 1.0f32);
            self.optimizer.edge_boxes.insert(
                *rid,
                EdgeBoxState {
                    box_id: *rid,
                    devices,
                    socs,
                    capacity_mw: avail,
                    online: true,
                },
            );
        }
    }

    /// 聚合出力分配（D3/D5/D8/D10）.
    ///
    /// 流程：dispatch_count += 1 → target 非有限拒绝（InvalidTarget）→
    /// `|target| > profile.available` 拒绝（InsufficientCapacity，D10）→
    /// sync_boxes（D8）→ `optimizer.optimize`：Ok → flat_map assignments 为
    /// allocations（device_id 即 resource_id）；Err(EmptyDomain) → 拒绝（NoResource）；
    /// Err(InvalidTarget) → 拒绝（InvalidTarget，防御分支）。`timestamp = now_ms`。
    pub fn dispatch(
        &mut self,
        market: &MarketData,
        target_mw: f32,
        now_ms: u64,
    ) -> Result<AggregatedDispatch, VppError> {
        self.dispatch_count += 1;
        if !target_mw.is_finite() {
            self.reject_count += 1;
            return Err(VppError::InvalidTarget);
        }
        let profile = self.compute_profile();
        if target_mw.abs() > profile.available_mw {
            self.reject_count += 1;
            return Err(VppError::InsufficientCapacity);
        }
        self.sync_boxes();
        match self.optimizer.optimize(market, target_mw, now_ms) {
            Ok(plan) => {
                // box_plans BTreeMap 天然 box_id 升序 = resource_id 升序（D2）
                let allocations: Vec<Allocation> = plan
                    .box_plans
                    .values()
                    .flat_map(|p| {
                        p.assignments.iter().map(|a| Allocation {
                            resource_id: a.device_id,
                            setpoint_mw: a.setpoint,
                        })
                    })
                    .collect();
                Ok(AggregatedDispatch {
                    target_mw,
                    allocations,
                    timestamp: now_ms,
                })
            }
            Err(OptError::EmptyDomain) => {
                self.reject_count += 1;
                Err(VppError::NoResource)
            }
            Err(OptError::InvalidTarget) => {
                // 防御分支：target 已在上方校验，理论上不可达
                self.reject_count += 1;
                Err(VppError::InvalidTarget)
            }
        }
    }

    /// 市场申报（纯查询无计数器更新，D12）.
    ///
    /// 按 resource_id 升序遍历在线资源，跳过 sanitize(available) ≤ 0 者；
    /// `quantity = min(available, strategy.max_quantity)`（max_quantity 非有限或 ≤0 →
    /// 按 available 全额）；`price = sanitize_price(market.current_price) + strategy.margin`；
    /// bid_id 从 1 顺序递增；空聚合器/全离线 → 空 Vec。
    pub fn market_bid(&self, market: &MarketData, strategy: &BidStrategy, now_ms: u64) -> Vec<ResourceBid> {
        let mut bids = Vec::new();
        let mut bid_id = 1u64;
        for (rid, r) in self.resources.iter() {
            if !r.online {
                continue;
            }
            let avail = sanitize_available(r.available_mw, r.capacity_mw);
            if avail <= 0.0 {
                continue;
            }
            let quantity = if strategy.max_quantity.is_finite() && strategy.max_quantity > 0.0 {
                avail.min(strategy.max_quantity)
            } else {
                avail
            };
            let price = sanitize_price(market.current_price as f32) + strategy.margin;
            // ARCH-M03：共享 Bid（market-types）单段表达；价格 YuanPerKwh（feed 元/kWh 单位）。
            use eneros_market_types::{Bid, BidSegment, TimeSlot, TradeDir, YuanPerKwh};
            bids.push(ResourceBid {
                resource_id: *rid,
                period: Period::Flat,
                bid: Bid::new(bid_id, MarketType::Spot, *rid, now_ms).with_segment(BidSegment {
                    period: TimeSlot::new(now_ms, now_ms + 3_600_000),
                    volume_kwh: quantity as f64 * 1000.0,
                    price: YuanPerKwh(price as f64),
                    direction: TradeDir::Sell,
                }),
            });
            bid_id += 1;
        }
        bids
    }
}
