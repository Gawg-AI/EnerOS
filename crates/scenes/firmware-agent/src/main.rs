//! 固件闭包 Agent 场景（ARC-008 / S4 出口）：
//! 完整 Agent 场景启动器——设备注册 + 轮询（PointAccess 统一抽象）+ 市场
//! 结算闭包（settlement 清仓 + account 记账），把 device-agent/agent/
//! protocol-abstract/upa-model/market-types/settlement/account 接入固件
//! 依赖闭包（binary-boms unreachable → reachable）。
//!
//! 本 binary 是场景骨架（无硬件；适配器用 Mock 设备），验证闭包可达性；
//! 真机数据路径（modbus-rtu/iec104 等真实传输）由设备层注入（BLOCKED 缺硬件）。

use eneros_account::{create_entry, AccountStore, FlowDir};
use eneros_device_agent::{DeviceRegistry, DeviceType, MockPointDevice};
use eneros_market_types::{MilliYuan, TimeSlot, TradeDir, YuanPerKwh};
use eneros_protocol_abstract::PointAccess;
use eneros_settlement::{ClearingEngine, ClearingJob};

fn main() {
    // 1. 设备注册 + 点轮询（PointAccess 统一抽象）
    let mut registry = DeviceRegistry::new();
    let mut adapter: Box<dyn PointAccess> = Box::new(MockPointDevice::new(3));
    // 空 Mock 设备 → PointNotFound 容忍（闭包可达性验证）
    let _ = adapter.read_point(1);
    registry.register("battery-01", DeviceType::Battery, adapter, Default::default());

    // 2. 市场结算闭包：电量 × 电价 → 毫元定点
    let job = ClearingJob {
        deal_id: 1,
        delivered_kwh: 100.0,
        contract_kwh: 100.0,
        contract_price: YuanPerKwh(0.5),
        at: 1000,
    };
    let mut clearing = ClearingEngine::new();
    if let Ok(result) = clearing.clear(&job) {
        let amount: MilliYuan = result.amount_yuan;
        // 3. 账本记账（毫元定点）
        let mut ledger = AccountStore::new();
        use eneros_account::AccountManager;
        ledger.open(1).unwrap();
        let _ = ledger.post(create_entry(1, FlowDir::Credit, 100.0, amount.to_yuan_f64(), Some(1), "firmware-scene"));
        let _ = ledger.balance(1);
    }

    // 4. 场景词汇就绪（market-types 闭包引用）
    let _slot = TimeSlot::new(1000, 3600);
    let _dir = TradeDir::Sell;
}
