=== *mut dyn 跨边界导出 ===
crates/drivers/driver-sdk/src/lib.rs:321:        pub extern "C" fn eneros_driver_create() -> *mut dyn $crate::DriverPlugin {
crates/drivers/driver-sdk/src/lib.rs:337:        pub unsafe extern "C" fn eneros_driver_destroy(ptr: *mut dyn $crate::DriverPlugin) {
crates/protocols/protocol-sdk/src/lib.rs:303:        pub extern "C" fn eneros_protocol_create() -> *mut dyn $crate::ProtocolPlugin {
crates/protocols/protocol-sdk/src/lib.rs:319:        pub unsafe extern "C" fn eneros_protocol_destroy(ptr: *mut dyn $crate::ProtocolPlugin) {
crates/protocols/protocol-sdk/src/lib.rs:302:        #[no_mangle]
crates/protocols/protocol-sdk/src/lib.rs:309:        #[no_mangle]
crates/protocols/protocol-sdk/src/lib.rs:318:        #[no_mangle]
crates/drivers/driver-sdk/src/lib.rs:320:        #[no_mangle]
crates/drivers/driver-sdk/src/lib.rs:327:        #[no_mangle]
crates/drivers/driver-sdk/src/lib.rs:336:        #[no_mangle]

=== 重复定义 ===
-- TradeDir:
crates/agents/spot-market/src/types.rs
crates/agents/realtime-market/src/types.rs
-- Bid struct:
crates/agents/ancillary/src/types.rs
crates/agents/spot-market/src/bid.rs
crates/agents/spot-market/src/types.rs
crates/agents/energy-market-agent/src/bid_generator.rs
crates/agents/federation/src/bid_book.rs
-- DataPoint struct:
crates/protocols/protocol-sdk/src/lib.rs
crates/protocols/upa-model/src/point.rs
-- DataAggregator-like:
crates/agents/cloud-coordinator/src/aggregator.rs
=== 依赖闭包：driver-sdk / protocol-sdk / framework / protocol-abstract 消费者 ===
-- eneros-driver-sdk consumers:
   (none)
-- eneros-protocol-sdk consumers:
   (none)
-- eneros-driver-framework consumers:
crates/drivers/rs485
crates/drivers/can
crates/protocols/modbus-rtu
-- eneros-protocol-abstract consumers:
   (none)
-- eneros-upa-model consumers:
crates/protocols/soe-engine
crates/protocols/telemetry-model
crates/protocols/mqtt
crates/agents/alarm
-- eneros-agent-sdk consumers:
crates/agents/simulator

=== market crates 消费者 ===
-- eneros-spot-market:
-- eneros-realtime-market:
-- eneros-p2p:
-- eneros-p2p-match:
-- eneros-ancillary:
-- eneros-settlement:
-- eneros-dr:
-- eneros-energy-market-agent:
crates/agents/cloud-coordinator
crates/agents/device-agent
crates/agents/mvp-scenario
crates/agents/coordinator
crates/agents/grid_agent
=== 拆分目标文件字节数 ===
 54003 crates/runtime/config/src/manager.rs
 47039 crates/agents/coordinator/src/domain_optimizer.rs
 46734 crates/agents/coordinator/src/vpp_aggregator.rs
 33933 crates/protocols/control-gateway/src/chain.rs
 27829 crates/protocols/control-gateway/src/slo.rs
 38582 crates/agents/twin-agent/src/whatif.rs
248120 total
crates/agents/ancillary/src/types.rs:33:pub struct BidAck {
crates/agents/energy-market-agent/src/bid_generator.rs:41:pub struct Bid {
crates/agents/spot-market/src/types.rs:51:pub struct BidSegment {
crates/agents/spot-market/src/types.rs:64:pub struct Bid {
crates/agents/spot-market/src/types.rs:79:pub struct BidAck {
---
19://!   （side+target_quantity / price_adjust+quantity）。
49:    pub price: f32,
84:    pub price_adjust: f32,
crates/agents/realtime-market/src/types.rs:34:    /// 报价价格 (元/MWh)
crates/agents/realtime-market/src/types.rs:64:    /// 出清价格 (元/MWh)
crates/agents/spot-market/src/types.rs:56:    /// 价格 (元/kWh)
crates/agents/spot-market/src/types.rs:95:    /// 出清价格 (元/kWh)
crates/agents/spot-market/src/types.rs:106:    /// 统一出清价格 (元/kWh)
