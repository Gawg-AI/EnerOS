//! WP-27.2（蓝图/27 §6）：驱动绑定框架——`DriverBinding`/`BindingTable`/
//! `ProbeReport`/`PlatformCtx`。
//!
//! # 不变量（蓝图/27 §6）
//! - 新增硬件 = 只增不改：一条 `compatible` 绑定 + 一个驱动 factory，
//!   不动装配层/root-task 主流程；
//! - 生产驱动必须实现 [`crate::DeviceDriver`] seam，未接 seam 禁进绑定表；
//! - 绑定表为**静态注册**（`&'static [DriverBinding]`），编译期可审计；
//! - `probe_all` 只负责按 compatible 匹配 + factory 实例化；驱动生命周期
//!   （init/start）由装配层在拿到 [`ProbeReport`] 后驱动。
//!
//! # no_std 合规
//! 仅 `core`/`alloc` + `eneros-board`（FDT 节点类型，零依赖叶子 crate）。

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_board::device_tree::{DeviceInventory, DeviceNode};

use crate::{DeviceDriver, DriverError};

/// 装配期平台资源（root-task 实现；host 测试用 mock 实现）。
///
/// 聚合装配所需的最小资源面：
/// - MMIO 映射（root-task 侧走 FIX-BUGS-0810 两件套：`alloc_device_at`
///   物理校验 + `node_offset=slot`，见 `SpawnCtx::map_device_mmio`）；
/// - IRQ 绑定（root-task 侧走 `IrqTable::bind`，badge 路由）；
/// - 配置加载（WP-27.6 扩展点；存储层就绪前禁假设备通道——不提供实现）。
pub trait PlatformCtx {
    /// 映射设备 MMIO（物理 → 虚拟，1:1），返回虚拟基址。
    ///
    /// `size_bytes` 由调用方按设备 `reg` 属性给出；实现负责页对齐。
    /// 平台未导出覆盖该物理地址的 device untyped → `Err`（fail-closed，
    /// 驱动不得以错页虚拟地址继续运行）。
    fn map_mmio(&mut self, phys: u64, size_bytes: u64) -> Result<u64, DriverError>;

    /// 绑定设备中断，返回 badge（事件循环 `route` 依据；0 保留未知）。
    fn bind_irq(&mut self, irq: u32) -> Result<u32, DriverError>;
}

/// 一条驱动绑定：`compatible` 串 → 驱动 factory。
#[derive(Clone, Copy)]
pub struct DriverBinding {
    /// FDT `compatible` 串（匹配 `DeviceNode::compatible` 首个值）。
    pub compatible: &'static str,
    /// 驱动构造器：从节点属性（reg/interrupts）装配硬件 + 驱动。
    pub factory:
        fn(&DeviceNode, &mut dyn PlatformCtx) -> Result<Box<dyn DeviceDriver>, DriverError>,
}

/// 静态绑定表（编译期可审计的 `&'static` 数组）。
pub struct BindingTable {
    entries: &'static [DriverBinding],
}

/// 一次 probe 的结果：绑定的驱动 / 无绑定匹配的节点 / 绑定失败的节点。
#[derive(Debug, Default)]
pub struct ProbeReport {
    bound: Vec<BoundEntry>,
    unbound: Vec<UnboundEntry>,
    failed: Vec<FailedEntry>,
}

/// 绑定成功的驱动（含装配资源，供 init/start）。
pub struct BoundEntry {
    /// 命中的 compatible 串。
    pub compatible: &'static str,
    /// 节点名（含 unit-address）。
    pub node: String,
    /// 实例化出的驱动。
    pub driver: Box<dyn DeviceDriver>,
    /// 已映射 MMIO 虚拟基址。
    pub mmio_base: Option<u64>,
    /// 已绑定 IRQ badge。
    pub irq_badge: Option<u32>,
}

impl core::fmt::Debug for BoundEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("BoundEntry")
            .field("compatible", &self.compatible)
            .field("node", &self.node)
            .field("driver_name", &self.driver.name())
            .field("mmio_base", &self.mmio_base)
            .field("irq_badge", &self.irq_badge)
            .finish()
    }
}

/// 无匹配绑定的节点（真机 bring-up 第一眼看到"哪些设备没驱动"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnboundEntry {
    /// 节点 compatible。
    pub compatible: String,
    /// 节点名。
    pub node: String,
}

/// 匹配但 factory 失败的节点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedEntry {
    /// 命中的 compatible 串。
    pub compatible: &'static str,
    /// 节点名。
    pub node: String,
    /// 失败原因。
    pub error: DriverError,
}

impl BindingTable {
    /// 构造静态绑定表（`&'static` 数组，编译期可审计）。
    pub const fn new(entries: &'static [DriverBinding]) -> Self {
        Self { entries }
    }

    /// 全部已注册 compatible 串（审计/日志用）。
    pub fn compatibles(&self) -> Vec<&'static str> {
        self.entries.iter().map(|entry| entry.compatible).collect()
    }

    /// 对设备清单逐节点 probe：首个 compatible 匹配成功即绑定；
    /// 匹配但 factory 失败 → failed（不尝试其他条目，同一节点只绑定一次）；
    /// 无匹配 → unbound。
    pub fn probe_all(&self, inventory: &DeviceInventory, ctx: &mut dyn PlatformCtx) -> ProbeReport {
        let mut report = ProbeReport::default();
        for node in &inventory.nodes {
            let Some(entry) = self
                .entries
                .iter()
                .find(|entry| entry.compatible == node.compatible)
            else {
                report.unbound.push(UnboundEntry {
                    compatible: node.compatible.clone(),
                    node: node.name.clone(),
                });
                continue;
            };
            match (entry.factory)(node, ctx) {
                Ok(driver) => report.bound.push(BoundEntry {
                    compatible: entry.compatible,
                    node: node.name.clone(),
                    driver,
                    mmio_base: None,
                    irq_badge: None,
                }),
                Err(error) => report.failed.push(FailedEntry {
                    compatible: entry.compatible,
                    node: node.name.clone(),
                    error,
                }),
            }
        }
        report
    }
}

impl ProbeReport {
    /// 绑定成功数。
    pub fn bound_count(&self) -> usize {
        self.bound.len()
    }

    /// 无匹配数。
    pub fn unbound_count(&self) -> usize {
        self.unbound.len()
    }

    /// 绑定失败数。
    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }

    /// 绑定的驱动（装配层据此 init/start）。
    pub fn bound(&self) -> &[BoundEntry] {
        &self.bound
    }

    /// 消费型：取出全部绑定条目（装配层移动驱动所有权——IRQ 接线/生命周期
    /// 由装配层持有）。
    pub fn into_bound(self) -> Vec<BoundEntry> {
        self.bound
    }

    /// 无匹配节点。
    pub fn unbound(&self) -> &[UnboundEntry] {
        &self.unbound
    }

    /// 失败绑定。
    pub fn failed(&self) -> &[FailedEntry] {
        &self.failed
    }

    /// 结构化指标行（启动日志 + 数值指标载体；`ENEROS stage=` 前缀由
    /// 装配层打印）。
    pub fn metrics_line(&self) -> String {
        format!(
            "bound={} unbound={} failed={}",
            self.bound_count(),
            self.unbound_count(),
            self.failed_count()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DriverHealth, DriverId, DriverState, DriverType};
    use alloc::boxed::Box;

    /// 测试驱动（实现 DeviceDriver seam 的最简形态）。
    struct TestDriver {
        id: DriverId,
        name: &'static str,
    }

    impl DeviceDriver for TestDriver {
        fn id(&self) -> &DriverId {
            &self.id
        }
        fn name(&self) -> &str {
            self.name
        }
        fn driver_type(&self) -> DriverType {
            DriverType::Custom(1)
        }
        fn state(&self) -> DriverState {
            DriverState::Ready
        }
        fn init(&mut self) -> Result<(), DriverError> {
            Ok(())
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
        fn handle_irq(&mut self, _irq_id: u32) {}
        fn health_check(&self) -> DriverHealth {
            DriverHealth::Healthy
        }
    }

    /// 记录调用轨迹的 mock ctx（断言 factory 通过 PlatformCtx 拿到资源）。
    #[derive(Default)]
    struct MockCtx {
        mapped: Vec<(u64, u64)>,
        irqs: Vec<u32>,
        fail_map: bool,
    }

    impl PlatformCtx for MockCtx {
        fn map_mmio(&mut self, phys: u64, size_bytes: u64) -> Result<u64, DriverError> {
            if self.fail_map {
                return Err(DriverError::NotFound);
            }
            self.mapped.push((phys, size_bytes));
            Ok(phys)
        }
        fn bind_irq(&mut self, irq: u32) -> Result<u32, DriverError> {
            self.irqs.push(irq);
            Ok(irq + 1)
        }
    }

    fn node(name: &str, compat: &str) -> DeviceNode {
        DeviceNode {
            name: name.into(),
            compatible: compat.into(),
            regs: alloc::vec![(0x0900_0000, 0x1000)],
            interrupts: alloc::vec![0, 1, 4],
        }
    }

    fn ok_factory(
        node: &DeviceNode,
        ctx: &mut dyn PlatformCtx,
    ) -> Result<Box<dyn DeviceDriver>, DriverError> {
        // factory 应通过 ctx 装配资源（MMIO + IRQ）
        ctx.map_mmio(node.regs[0].0, node.regs[0].1)?;
        ctx.bind_irq(33)?;
        Ok(Box::new(TestDriver {
            id: DriverId(1),
            name: "test",
        }))
    }

    fn fail_factory(
        _node: &DeviceNode,
        _ctx: &mut dyn PlatformCtx,
    ) -> Result<Box<dyn DeviceDriver>, DriverError> {
        Err(DriverError::InitFailed)
    }

    #[test]
    fn probe_all_classifies_bound_unbound_failed() {
        let table = BindingTable::new(&[
            DriverBinding {
                compatible: "arm,pl011",
                factory: ok_factory,
            },
            DriverBinding {
                compatible: "nxp,sja1000",
                factory: fail_factory,
            },
        ]);
        let inventory = DeviceInventory {
            nodes: alloc::vec![
                node("uart@9000000", "arm,pl011"),
                node("can@10000000", "nxp,sja1000"),
                node("timer", "arm,armv8-timer"),
            ],
        };
        let mut ctx = MockCtx::default();
        let report = table.probe_all(&inventory, &mut ctx);
        assert_eq!(report.bound_count(), 1);
        assert_eq!(report.unbound_count(), 1);
        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.bound()[0].compatible, "arm,pl011");
        assert_eq!(report.bound()[0].node, "uart@9000000");
        assert_eq!(report.bound()[0].driver.name(), "test");
        assert_eq!(report.unbound()[0].compatible, "arm,armv8-timer");
        assert_eq!(report.failed()[0].error, DriverError::InitFailed);
        // factory 经 ctx 装配了 MMIO + IRQ
        assert_eq!(ctx.mapped, alloc::vec![(0x0900_0000, 0x1000)]);
        assert_eq!(ctx.irqs, alloc::vec![33]);
        assert_eq!(report.metrics_line(), "bound=1 unbound=1 failed=1");
    }

    #[test]
    fn map_failure_is_failed_not_bound() {
        let table = BindingTable::new(&[DriverBinding {
            compatible: "arm,pl011",
            factory: ok_factory,
        }]);
        let inventory = DeviceInventory {
            nodes: alloc::vec![node("uart@9000000", "arm,pl011")],
        };
        let mut ctx = MockCtx {
            fail_map: true,
            ..MockCtx::default()
        };
        let report = table.probe_all(&inventory, &mut ctx);
        assert_eq!(report.bound_count(), 0);
        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.failed()[0].error, DriverError::NotFound);
    }

    #[test]
    fn binding_table_is_compile_time_auditable() {
        // 静态表（&'static）可在 const 上下文构造——编译期审计面。
        const TABLE: BindingTable = BindingTable::new(&[]);
        assert!(TABLE.compatibles().is_empty());
        const ENTRIES: &[DriverBinding] = &[DriverBinding {
            compatible: "arm,pl011",
            factory: ok_factory,
        }];
        const TABLE2: BindingTable = BindingTable::new(ENTRIES);
        assert_eq!(TABLE2.compatibles(), alloc::vec!["arm,pl011"]);
    }

    #[test]
    fn empty_inventory_yields_empty_report() {
        let table = BindingTable::new(&[DriverBinding {
            compatible: "arm,pl011",
            factory: ok_factory,
        }]);
        let inventory = DeviceInventory::default();
        let mut ctx = MockCtx::default();
        let report = table.probe_all(&inventory, &mut ctx);
        assert_eq!(report.metrics_line(), "bound=0 unbound=0 failed=0");
    }
}
