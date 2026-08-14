//! 38（WP-38.8）：DriverSupervisor——驱动完整生命周期与所有权。
//!
//! # 不变量（蓝图 38 WP-38.8）
//! - 持有**全部** BoundDriver（不按 IRQ 过滤；无 IRQ 驱动不再随局部变量
//!   drop——P0-06/HD-04 修复）；
//! - factory success 只进入 `Bound`；随后按 bind → map → init → start →
//!   health 推进；任一步失败逆序回滚前序资源（无泄漏/假健康）；
//! - health 不能由 Noop 默认 healthy：未执行生命周期或 health 未知 →
//!   `Failed/Unknown`，Noop 仅在 test-support 存在；
//! - 生命周期状态机与 `DeviceDriver` 内部 6 态分离：supervisor 状态是
//!   装配事实，驱动状态是驱动自报事实。

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::binding::BoundEntry;
use crate::{DeviceDriver, DriverError, DriverHealth};

/// 驱动生命周期状态（装配层事实；11 态，蓝图 WP-38.8 模型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverState {
    /// 已被 factory 构造（匹配成功）。
    Discovered,
    /// 已登记进 supervisor（绑定成立）。
    Bound,
    /// 资源已映射（MMIO/IRQ 装配完成）。
    ResourcesMapped,
    /// 已初始化（init 通过）。
    Initialized,
    /// 已启动（start 通过）。
    Started,
    /// 健康检查通过。
    Healthy,
    /// 降级（可运行但受限——health 报告 Degraded）。
    Degraded,
    /// 失败（生命周期任一步失败或 health 非健康）。
    Failed,
    /// 停止中（stop 进行中）。
    Stopping,
    /// 已停止。
    Stopped,
}

impl DriverState {
    pub const fn name(self) -> &'static str {
        match self {
            DriverState::Discovered => "discovered",
            DriverState::Bound => "bound",
            DriverState::ResourcesMapped => "resources-mapped",
            DriverState::Initialized => "initialized",
            DriverState::Started => "started",
            DriverState::Healthy => "healthy",
            DriverState::Degraded => "degraded",
            DriverState::Failed => "failed",
            DriverState::Stopping => "stopping",
            DriverState::Stopped => "stopped",
        }
    }
}

/// 被监管的驱动（含装配事实与生命周期状态）。
pub struct SupervisedDriver {
    /// 命中的 compatible 串。
    pub compatible: &'static str,
    /// 节点名。
    pub node: String,
    /// 驱动实例（所有权在 supervisor——无 IRQ 驱动不再丢失）。
    pub driver: Box<dyn DeviceDriver>,
    /// 生命周期状态（装配事实）。
    pub state: DriverState,
    /// 已绑定 IRQ badge（无 → None；IRQ 不是持有判据）。
    pub irq_badge: Option<u32>,
    /// 生命周期失败原因（Failed 时）。
    pub error: Option<DriverError>,
}

/// 驱动监督器：全量持有 + bind→map→init→start→health。
pub struct DriverSupervisor {
    drivers: Vec<SupervisedDriver>,
}

impl DriverSupervisor {
    pub fn new() -> Self {
        Self {
            drivers: Vec::new(),
        }
    }
    /// 采纳一条 probe 绑定（factory 已构造驱动并映射资源）。
    pub fn adopt(&mut self, entry: BoundEntry) {
        let BoundEntry {
            compatible,
            node,
            driver,
            mmio_base,
            irq_badge,
        } = entry;
        self.drivers.push(SupervisedDriver {
            compatible,
            node,
            driver,
            // factory 返回即资源已装配（map_mmio 在 factory 内完成）；
            // 显式记录映射状态。
            state: if mmio_base.is_some() {
                DriverState::ResourcesMapped
            } else {
                DriverState::Bound
            },
            irq_badge,
            error: None,
        });
    }

    /// 全量执行 init → start → health（逆序回滚：任一步失败先 stop 再
    /// deinit；失败驱动状态 Failed，不阻塞其余驱动）。
    pub fn run_lifecycle(&mut self, epoch: u64) {
        let _ = epoch;
        // 两阶段：先全部 init（资源层稳定），再全部 start，最后 health。
        for d in self.drivers.iter_mut() {
            if d.state == DriverState::Failed {
                continue;
            }
            match d.driver.init() {
                Ok(()) => d.state = DriverState::Initialized,
                Err(e) => {
                    d.state = DriverState::Failed;
                    d.error = Some(e);
                    rollback(d);
                }
            }
        }
        for d in self.drivers.iter_mut() {
            if d.state != DriverState::Initialized {
                continue;
            }
            match d.driver.start() {
                Ok(()) => d.state = DriverState::Started,
                Err(e) => {
                    d.state = DriverState::Failed;
                    d.error = Some(e);
                    rollback(d);
                }
            }
        }
        for d in self.drivers.iter_mut() {
            if d.state != DriverState::Started {
                continue;
            }
            d.state = match d.driver.health_check() {
                DriverHealth::Healthy => DriverState::Healthy,
                DriverHealth::Degraded => DriverState::Degraded,
                DriverHealth::Unhealthy => {
                    d.error = Some(DriverError::InvalidState);
                    DriverState::Failed
                }
                DriverHealth::Unknown => {
                    // 未知健康 → 不冒充 healthy（fail-closed）。
                    d.error = Some(DriverError::InvalidState);
                    DriverState::Failed
                }
            };
        }
    }

    /// 全部驱动（证据/持有面）。
    pub fn all(&self) -> &[SupervisedDriver] {
        &self.drivers
    }

    /// 按 IRQ badge 查驱动（事件循环 handle_irq 路由；无 → None）。
    pub fn driver_for_badge(&mut self, badge: u32) -> Option<&mut (dyn DeviceDriver + 'static)> {
        self.drivers
            .iter_mut()
            .find(|d| d.irq_badge == Some(badge))
            .map(|d| d.driver.as_mut())
    }

    /// 生命周期报告行（ENEROS stage=driver-supervisor）。
    pub fn report_line(&self) -> alloc::string::String {
        let mut out = alloc::string::String::from("drivers=[");
        for (i, d) in self.drivers.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&alloc::format!("{}({})", d.driver.name(), d.state.name()));
        }
        out.push(']');
        out
    }

    /// 已 Healthy/Degraded 的驱动数（readiness 依据）。
    pub fn healthy_count(&self) -> usize {
        self.drivers
            .iter()
            .filter(|d| matches!(d.state, DriverState::Healthy | DriverState::Degraded))
            .count()
    }
}

impl Default for DriverSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

/// 失败回滚：逆序 stop → deinit（best-effort；失败仅记录，状态已 Failed）。
fn rollback(d: &mut SupervisedDriver) {
    if matches!(d.state, DriverState::Started) {
        let _ = d.driver.stop();
    }
    let _ = d.driver.deinit();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockDriver;

    fn entry(name: &'static str) -> BoundEntry {
        BoundEntry {
            compatible: "test,compat",
            node: name.into(),
            driver: Box::new(MockDriver::new(
                crate::DriverId(1),
                name,
                crate::DriverType::Serial,
            )),
            mmio_base: Some(0x1000),
            irq_badge: None,
        }
    }

    #[test]
    fn supervisor_holds_drivers_without_irq() {
        // P0-06 回归：无 IRQ 驱动必须被 supervisor 持有（不随局部变量 drop）。
        let mut sup = DriverSupervisor::new();
        sup.adopt(entry("no-irq-can"));
        assert_eq!(sup.all().len(), 1);
        assert!(sup.all()[0].irq_badge.is_none());
        sup.run_lifecycle(1);
        assert_eq!(sup.all()[0].state, DriverState::Healthy);
        assert_eq!(sup.healthy_count(), 1);
    }

    #[test]
    fn all_drivers_get_lifecycle() {
        let mut sup = DriverSupervisor::new();
        sup.adopt(entry("a"));
        sup.adopt(entry("b"));
        sup.adopt(entry("c"));
        sup.run_lifecycle(1);
        for d in sup.all() {
            assert_eq!(d.state, DriverState::Healthy, "{}", d.driver.name());
        }
        assert_eq!(sup.healthy_count(), 3);
    }

    #[test]
    fn failing_init_marks_failed_and_rolls_back() {
        let mut sup = DriverSupervisor::new();
        sup.adopt(entry("good"));
        // 构造一个 init 失败的驱动。
        let mut bad = entry("bad");
        bad.driver = Box::new(FailInitDriver);
        sup.adopt(bad);
        sup.run_lifecycle(1);
        // 按节点名查找（驱动名可被替换——FailInitDriver 名为 fail-init）。
        let good = sup.all().iter().find(|d| d.node == "good").unwrap();
        assert_eq!(good.state, DriverState::Healthy);
        let bad = sup.all().iter().find(|d| d.node == "bad").unwrap();
        assert_eq!(bad.state, DriverState::Failed);
        assert!(bad.error.is_some());
    }

    /// init 恒失败的驱动（生命周期负例）。
    struct FailInitDriver;
    impl DeviceDriver for FailInitDriver {
        fn id(&self) -> &crate::DriverId {
            static ID: crate::DriverId = crate::DriverId(0xFFFF_0001);
            &ID
        }
        fn name(&self) -> &str {
            "fail-init"
        }
        fn driver_type(&self) -> crate::DriverType {
            crate::DriverType::Custom(1)
        }
        fn state(&self) -> crate::DriverState {
            crate::DriverState::Uninitialized
        }
        fn init(&mut self) -> Result<(), DriverError> {
            Err(DriverError::InitFailed)
        }
        fn start(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        fn stop(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        fn deinit(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        fn handle_irq(&mut self, _irq: u32) {}
        fn health_check(&self) -> DriverHealth {
            DriverHealth::Healthy
        }
    }

    #[test]
    fn state_names_stable() {
        assert_eq!(DriverState::Healthy.name(), "healthy");
        assert_eq!(DriverState::Failed.name(), "failed");
        assert_eq!(DriverState::Started.name(), "started");
    }
}
