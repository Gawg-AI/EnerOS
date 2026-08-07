//! 固件闭包 Agent 场景（ARC-008 / S4 出口）：
//! 完整 Agent 场景启动器——设备注册 + 轮询（PointAccess 统一抽象）+ 市场
//! 结算闭包（settlement 清仓 + account 记账），把 device-agent/agent/
//! protocol-abstract/upa-model/market-types/settlement/account 接入固件
//! 依赖闭包（binary-boms unreachable → reachable）。
//!
//! 本 binary 是场景骨架（无硬件；适配器用 Mock 设备），验证闭包可达性；
//! 真机数据路径（modbus-rtu/iec104 等真实传输）由设备层注入（BLOCKED 缺硬件）。

use eneros_account::{create_entry, AccountStore, FlowDir};
#[cfg(feature = "mock")]
use eneros_device_agent::MockPointDevice;
use eneros_device_agent::{DeviceRegistry, DeviceType};
use eneros_market_types::{MilliYuan, TimeSlot, TradeDir, YuanPerKwh};
use eneros_protocol_abstract::PointAccess;
use eneros_settlement::{ClearingEngine, ClearingJob};

fn main() {
    // 1. 设备注册 + 点轮询（PointAccess 统一抽象）
    let mut registry = DeviceRegistry::new();
    #[cfg(feature = "mock")]
    let mut adapter: Box<dyn PointAccess> = Box::new(MockPointDevice::new(3));
    #[cfg(not(feature = "mock"))]
    let mut adapter: Box<dyn PointAccess> = {
        // 真实固件闭包（蓝图/20 R2-1 + R2-7）：无真实设备驱动时 fail-closed
        // 拒绝启动——VRF-002 双构建字节比对验证的是闭包确定性。
        eprintln!("firmware-agent: no real device adapter injected (R2-1)");
        std::process::exit(2);
    };
    let _ = adapter.read_point(1);
    registry.register(
        "battery-01",
        DeviceType::Battery,
        adapter,
        Default::default(),
    );

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
        // 演示骨架：记账/查询失败可忽略（闭包可达性验证，非生产数据路径；
        // 真机场景由 account 服务的持久化事务处理错误）。
        let _ = ledger.post(create_entry(
            1,
            FlowDir::Credit,
            100.0,
            amount.to_yuan_f64(),
            Some(1),
            "firmware-scene",
        ));
        let _ = ledger.balance(1);
    }

    // 4. 场景词汇就绪（market-types 闭包引用）
    let _slot = TimeSlot::new(1000, 3600);
    let _dir = TradeDir::Sell;
}

#[cfg(test)]
mod tests {
    use super::*;
    use eneros_account::AccountManager;

    /// 场景端到端断言（v0.233.0 B3-10：固件闭包场景可验证）。
    #[test]
    fn test_firmware_scene_end_to_end() {
        // 1. 设备注册。
        let mut registry = DeviceRegistry::new();
        registry.register(
            "battery-01",
            DeviceType::Battery,
            Box::new(MockPointDevice::new(3)),
            Default::default(),
        );
        assert_eq!(registry.len(), 1, "设备注册成功");

        // 2. 市场结算：100 kWh × 0.5 元/kWh = 50 元 = 50_000 毫元。
        let job = ClearingJob {
            deal_id: 1,
            delivered_kwh: 100.0,
            contract_kwh: 100.0,
            contract_price: YuanPerKwh(0.5),
            at: 1000,
        };
        let mut clearing = ClearingEngine::new();
        let result = clearing.clear(&job).expect("结算成功");
        assert_eq!(result.amount_yuan, MilliYuan(50_000), "100×0.5=50 元定点");

        // 3. 账本记账后余额正确。
        let mut ledger = AccountStore::new();
        ledger.open(1).expect("开户");
        ledger
            .post(create_entry(
                1,
                FlowDir::Credit,
                100.0,
                result.amount_yuan.to_yuan_f64(),
                Some(1),
                "firmware-scene",
            ))
            .expect("记账");
        let (kwh, balance) = ledger.balance(1).expect("查询余额");
        assert!(kwh > 0.0, "电量累计");
        assert_eq!(balance, MilliYuan(50_000), "余额=结算额");

        // 4. 词汇就绪。
        let slot = TimeSlot::new(1000, 3600);
        assert_eq!(slot.start, 1000);
        assert_eq!(TradeDir::Sell as u8, 1);
    }
}
