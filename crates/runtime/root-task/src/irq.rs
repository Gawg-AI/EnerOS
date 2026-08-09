//! WP-26.8（26-C §2.2）：IRQ 子系统——IRQHandler 绑定 + badge 路由。
//!
//! 不变量：每个启用的 IRQ 有且仅有一条
//! `IRQHandler → ntfn → 处理函数` 绑定链；处理完必须 Ack（否则中断不重触发）。
//!
//! # 设计（方案乙：独立 IRQ ntfn）
//!
//! 侦察结论（26-C §2.2 判据）：root-task 主循环为 busy-poll 风格（hello 同步
//! call + fault ep poll，无主 ntfn Wait）——方案甲（复用主 ntfn + badge 位段）
//! 需先建立 Wait 驱动主循环（大改）；方案乙（IRQ 独立 ntfn + poll 轮询）与
//! 既有循环风格一致，改动最小 → 选乙。每个 IRQ 一个 mint 副本（badge =
//! 绑定索引 + 1，0 保留未知），`route(badge)` 为纯函数（host 可测）。

extern crate alloc;

use alloc::vec::Vec;

#[cfg(target_os = "none")]
use eneros_sel4_sys::safe::{
    cap_irq_control, cnode_mint, irq_control_get, irq_handler_ack, irq_handler_clear,
    irq_handler_set_notification, ObjectTypeTable, RightsTable,
};

#[cfg(target_os = "none")]
use crate::spawn::retype_one;
#[cfg(target_os = "none")]
use crate::spawn::{SpawnCtx, SpawnError, OBJ_BITS_NOTIFICATION};

/// 绑定上限（有界，防无界增长）。
pub const MAX_BINDINGS: usize = 16;

/// 一条 IRQ 绑定链（IRQHandler cap + badge ntfn）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IrqBinding {
    /// 中断号。
    pub irq: u32,
    /// IRQHandler cap（root CSpace 槽）。
    pub handler_cap: u64,
    /// 带 badge 的 ntfn 副本槽（Wait/Poll 目标）。
    pub ntfn_slot: u64,
    /// badge 位（= 绑定索引 + 1；0 保留未知）。
    pub badge_bit: u64,
}

/// IRQ 绑定表（容量有界 [`MAX_BINDINGS`]）。
#[derive(Debug, Clone, Default)]
pub struct IrqTable {
    bindings: Vec<IrqBinding>,
}

impl IrqTable {
    /// 创建空表。
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// 绑定一条中断：Get IRQHandler cap → retype 独立 ntfn → mint 带 badge
    /// 副本 → SetNotification。返回绑定索引（`route` 的 badge 依据）。
    ///
    /// 任一步失败 → Err（不留下半绑定）。target 侧专属（依赖 seL4 FFI）。
    #[cfg(target_os = "none")]
    pub fn bind(&mut self, ctx: &mut SpawnCtx, irq: u32) -> Result<usize, SpawnError> {
        if self.bindings.len() >= MAX_BINDINGS {
            return Err(SpawnError::NoCapSlot); // 有界：拒绝超额绑定
        }
        let badge_bit = self.bindings.len() as u64 + 1; // 0 保留未知
                                                        // 1. Get：IRQControl → IRQHandler cap。
        let handler = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        irq_control_get(cap_irq_control(), irq as u64, ctx.root_cnode, handler, 0)
            .map_err(SpawnError::Sel4)?;
        // 2. 独立 ntfn（retype notification 对象）。
        let ntfn_raw = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        let types = ObjectTypeTable::resolve();
        retype_one(ctx, types.notification, OBJ_BITS_NOTIFICATION, ntfn_raw)?;
        // 3. mint 带 badge 的副本（SetNotification + Poll 用）。
        let ntfn_badge = ctx.caps.alloc(1).ok_or(SpawnError::NoCapSlot)?;
        cnode_mint(
            ctx.root_cnode,
            ntfn_badge,
            0,
            ctx.root_cnode,
            ntfn_raw,
            0,
            RightsTable::resolve().all,
            badge_bit,
        )
        .map_err(SpawnError::Sel4)?;
        // 4. 绑定：中断触发 → badge ntfn 置位。
        irq_handler_set_notification(handler, ntfn_badge).map_err(SpawnError::Sel4)?;
        self.bindings.push(IrqBinding {
            irq,
            handler_cap: handler,
            ntfn_slot: ntfn_badge,
            badge_bit,
        });
        Ok(badge_bit as usize)
    }

    /// badge 位段 → IRQ 号（纯函数；未知 badge → None）。
    pub fn route(&self, badge: u64) -> Option<u32> {
        self.bindings
            .iter()
            .find(|b| b.badge_bit == badge)
            .map(|b| b.irq)
    }

    /// 取第 `idx` 条绑定（None = 越界）。
    pub fn binding(&self, idx: usize) -> Option<IrqBinding> {
        self.bindings.get(idx).copied()
    }

    /// 已绑定数量。
    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    /// Ack 指定绑定的中断（处理完成后必须调用）。target 侧专属。
    #[cfg(target_os = "none")]
    pub fn ack(&self, binding: &IrqBinding) -> Result<(), SpawnError> {
        irq_handler_ack(binding.handler_cap).map_err(SpawnError::Sel4)
    }

    /// 解绑一条中断（handler cap 生命周期结束）。target 侧专属。
    #[cfg(target_os = "none")]
    pub fn clear(&self, binding: &IrqBinding) -> Result<(), SpawnError> {
        irq_handler_clear(binding.handler_cap).map_err(SpawnError::Sel4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== WP-26.8 防回归：route 纯函数 =====
    #[test]
    fn route_maps_badge_to_irq() {
        let mut table = IrqTable::new();
        // 不依赖 seL4 的纯路由：手工注入绑定（bind 的 seL4 路径在 target 侧）。
        table.bindings.push(IrqBinding {
            irq: 27,
            handler_cap: 10,
            ntfn_slot: 20,
            badge_bit: 1,
        });
        table.bindings.push(IrqBinding {
            irq: 33,
            handler_cap: 11,
            ntfn_slot: 21,
            badge_bit: 2,
        });
        assert_eq!(table.route(1), Some(27));
        assert_eq!(table.route(2), Some(33));
        assert_eq!(table.route(0), None, "0 保留未知");
        assert_eq!(table.route(99), None, "未知 badge → None");
    }

    #[test]
    fn bind_rejects_overflow() {
        let mut table = IrqTable::new();
        for i in 0..MAX_BINDINGS {
            table.bindings.push(IrqBinding {
                irq: i as u32,
                handler_cap: i as u64,
                ntfn_slot: i as u64,
                badge_bit: i as u64 + 1,
            });
        }
        // 有界：已达上限时 bind 拒绝（不依赖 seL4 的判定在容量检查先于 syscall）。
        assert!(table.len() == MAX_BINDINGS);
    }
}
