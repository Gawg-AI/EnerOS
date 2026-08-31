//! # 蓝图 61 §12 v0.314.48：rendezvous 分片路由（§3.2）
//!
//! - 键 `(tenant, site, source)` 经 FNV-1a 哈希 → 虚拟节点环（vnode 数
//!   版本化：`ShardRing::with_version`）→ 路由到分片。同键恒路由同分片
//!   （同资源所有命令固定到一个 leader 的基石）。
//! - 重分片五步：copy → catch-up → dual-read compare → atomic route
//!   switch → old drain。本模块产出确定性 [`ReshardPlan`] 动作清单；
//!   模拟执行测试（`tests/scale_shard.rs`）验证与 reference 模型等价、
//!   无双重写设备副作用。
//! - 哈希：FNV-1a 64（仓库既有确定性惯例），`(tenant,site,source)` 以
//!   定长字节拼接，避免分隔符碰撞。

use alloc::vec::Vec;

/// 分片路由键。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShardKey {
    pub tenant: u32,
    pub site: u32,
    pub source: u32,
}

/// FNV-1a 64（确定性、非密码学——路由用途）。
struct Fnv1a64(u64);

impl Fnv1a64 {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }
    fn write_bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
}

fn hash_key(k: &ShardKey) -> u64 {
    let mut h = Fnv1a64::new();
    h.write_bytes(&k.tenant.to_be_bytes());
    h.write_bytes(&k.site.to_be_bytes());
    h.write_bytes(&k.source.to_be_bytes());
    h.0
}

/// 分片环（版本化虚拟节点）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardRing {
    /// vnode 数（版本化配置；重分片 = 新版本环）。
    pub vnodes: u32,
    /// 分片总数。
    pub shards: u32,
    /// 配置版本（单调递增；路由切换以版本为准）。
    pub version: u64,
}

/// 路由结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteResult {
    pub shard: u32,
    pub vnode: u32,
}

impl ShardRing {
    pub fn new(vnodes: u32, shards: u32, version: u64) -> Result<Self, ShardError> {
        if shards == 0 || vnodes == 0 || vnodes < shards {
            return Err(ShardError::InvalidRing);
        }
        Ok(Self {
            vnodes,
            shards,
            version,
        })
    }

    /// 确定性路由：`hash % vnodes → vnode % shards`。
    pub fn route(&self, k: &ShardKey) -> RouteResult {
        let h = hash_key(k);
        let vnode = (h % self.vnodes as u64) as u32;
        RouteResult {
            shard: vnode % self.shards,
            vnode,
        }
    }

    /// 判断环是否与目标配置等价（同 vnode/分片数即同路由）。
    pub fn equivalent(&self, other: &ShardRing) -> bool {
        self.vnodes == other.vnodes && self.shards == other.shards && self.version == other.version
    }
}

/// 分片错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShardError {
    InvalidRing,
}

/// 重分片五步规划（§3.2）：copy → catch-up → dual-read → switch → drain。
/// 动作由执行器（模拟或真实编排）逐步执行；任何一步失败 → 停在当前
/// 步（旧路由仍有效，无双重写设备副作用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReshardStep {
    /// Copy：旧分片全量复制，目标由执行器按新环逐键路由（计划不预设
    /// 目标分片——预设单目标会在分片扩张方向漏拷）。
    Copy {
        from_shard: u32,
        version_from: u64,
        version_to: u64,
    },
    /// Catch-up：增量追平（消费旧分片持续写入）。
    CatchUp { shard: u32, version_to: u64 },
    /// Dual-read compare：双读比对（等值断言在测试侧执行）。
    DualReadCompare { shard: u32 },
    /// Atomic route switch：切换环版本（一次性、原子）。
    Switch { version_to: u64 },
    /// Old drain：旧分片排空（只读、可观测）。
    Drain { shard: u32, version_from: u64 },
}

/// 重分片计划（确定性动作序列 + 各阶段已覆盖键集合）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReshardPlan {
    pub steps: Vec<ReshardStep>,
    pub old_version: u64,
    pub new_version: u64,
}

impl ReshardPlan {
    /// 从旧环生成到新环的五步计划（按 shard 序确定性展开）。
    pub fn plan(old: &ShardRing, new: &ShardRing) -> Result<Self, ShardError> {
        if old.equivalent(new) {
            return Ok(Self {
                steps: Vec::new(),
                old_version: old.version,
                new_version: new.version,
            });
        }
        // vnode 数变化也视为完整重分片（全部键重新路由）；计划按分片
        // 序确定性展开，执行器逐阶段推进。
        let mut steps = Vec::new();
        // P1-5 修复：Copy 不预设目标分片——旧分片键在新环的目标分片是
        // 键函数（`from % new.shards` 会在扩张方向漏拷），由执行器按新环
        // 路由逐键落位。
        for from in 0..old.shards {
            steps.push(ReshardStep::Copy {
                from_shard: from,
                version_from: old.version,
                version_to: new.version,
            });
        }
        for shard in 0..new.shards {
            steps.push(ReshardStep::CatchUp {
                shard,
                version_to: new.version,
            });
        }
        for shard in 0..new.shards {
            steps.push(ReshardStep::DualReadCompare { shard });
        }
        steps.push(ReshardStep::Switch {
            version_to: new.version,
        });
        for shard in 0..old.shards {
            steps.push(ReshardStep::Drain {
                shard,
                version_from: old.version,
            });
        }
        Ok(Self {
            steps,
            old_version: old.version,
            new_version: new.version,
        })
    }
}

/// 路由侧双读视图：切换前按旧环、切换后按新环；用于 dual-read compare。
pub fn dual_route(old: &ShardRing, new: &ShardRing, k: &ShardKey) -> (RouteResult, RouteResult) {
    (old.route(k), new.route(k))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;

    #[test]
    fn route_deterministic_and_stable() {
        let ring = ShardRing::new(1000, 4, 1).expect("ring");
        let k = ShardKey {
            tenant: 1,
            site: 2,
            source: 3,
        };
        let a = ring.route(&k);
        let b = ring.route(&k);
        assert_eq!(a, b);
        // 同键恒同分片（同资源所有命令固定到一个 leader）。
        let c = ring.route(&ShardKey {
            tenant: 1,
            site: 2,
            source: 3,
        });
        assert_eq!(a.shard, c.shard);
    }

    #[test]
    fn keys_spread_across_shards() {
        let ring = ShardRing::new(4096, 8, 1).expect("ring");
        let mut per_shard = [0u32; 8];
        for i in 0..100_000u32 {
            let k = ShardKey {
                tenant: 1,
                site: i % 100,
                source: i,
            };
            let r = ring.route(&k);
            per_shard[r.shard as usize] += 1;
        }
        let total: u32 = per_shard.iter().sum();
        let avg = total as f64 / 8.0;
        // 宽松平衡断言：每分片在均值 ±30% 内（rendezvous 分布的确定性特征）。
        for (i, c) in per_shard.iter().enumerate() {
            let d = (*c as f64 - avg).abs() / avg;
            assert!(d < 0.30, "shard {i} 偏差 {d:.2} 超 30%");
        }
    }

    #[test]
    fn reshard_plan_five_phases() {
        let old = ShardRing::new(1000, 4, 7).expect("old");
        let new = ShardRing::new(1000, 8, 8).expect("new");
        let plan = ReshardPlan::plan(&old, &new).expect("plan");
        // 五阶段顺序。
        let copy = plan
            .steps
            .iter()
            .filter(|s| matches!(s, ReshardStep::Copy { .. }))
            .count();
        let catchup = plan
            .steps
            .iter()
            .filter(|s| matches!(s, ReshardStep::CatchUp { .. }))
            .count();
        let dual = plan
            .steps
            .iter()
            .filter(|s| matches!(s, ReshardStep::DualReadCompare { .. }))
            .count();
        let switch = plan
            .steps
            .iter()
            .filter(|s| matches!(s, ReshardStep::Switch { .. }))
            .count();
        let drain = plan
            .steps
            .iter()
            .filter(|s| matches!(s, ReshardStep::Drain { .. }))
            .count();
        assert_eq!(copy, 4);
        assert_eq!(catchup, 8);
        assert_eq!(dual, 8);
        assert_eq!(switch, 1);
        assert_eq!(drain, 4);
        assert_eq!(plan.new_version, 8);
        // 阶段顺序不变式。
        let kinds: Vec<&str> = plan
            .steps
            .iter()
            .map(|s| match s {
                ReshardStep::Copy { .. } => "copy",
                ReshardStep::CatchUp { .. } => "catchup",
                ReshardStep::DualReadCompare { .. } => "dual",
                ReshardStep::Switch { .. } => "switch",
                ReshardStep::Drain { .. } => "drain",
            })
            .collect();
        let first_switch = kinds.iter().position(|k| *k == "switch").expect("switch");
        assert!(
            kinds[..first_switch].iter().all(|k| *k != "drain"),
            "drain 必须在 switch 之后"
        );
        assert!(
            kinds[first_switch..].iter().all(|k| *k != "copy"),
            "copy 必须在 switch 之前"
        );
    }

    #[test]
    fn equivalent_ring_yields_empty_plan() {
        let a = ShardRing::new(1000, 4, 1).expect("a");
        let b = ShardRing::new(1000, 4, 1).expect("b");
        let plan = ReshardPlan::plan(&a, &b).expect("plan");
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn invalid_ring_rejected() {
        assert_eq!(ShardRing::new(0, 4, 1), Err(ShardError::InvalidRing));
        assert_eq!(ShardRing::new(1000, 0, 1), Err(ShardError::InvalidRing));
        assert_eq!(ShardRing::new(2, 4, 1), Err(ShardError::InvalidRing));
    }
}
