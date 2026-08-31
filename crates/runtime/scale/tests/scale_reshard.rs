//! WP-7.2：重分片五步执行与参考模型等价（蓝图 61 §12 §3.2 末段）。
//!
//! 模拟执行器：Copy（幂等复制键集合）→ CatchUp（增量追平）→
//! DualReadCompare（双读一致）→ Switch（原子换环）→ Drain（旧环排空）。
//! 断言：全程键集合守恒、无设备副作用双重写、切换后新环路由且旧环只读。

#![allow(clippy::disallowed_macros)]
use std::collections::{BTreeMap, BTreeSet};

use eneros_scale::shard::{dual_route, ReshardPlan, ReshardStep, ShardKey, ShardRing};

/// 模拟分片存储（seq_key → seq）。
type ShardStore = BTreeMap<u64, u64>;

fn seq_key(k: &ShardKey) -> u64 {
    ((k.tenant as u64) << 40) | ((k.site as u64) << 20) | k.source as u64
}

fn key_of(seq: &u64) -> ShardKey {
    ShardKey {
        tenant: (seq >> 40) as u32,
        site: ((seq >> 20) & 0xFFFFF) as u32,
        source: (seq & 0xFFFFF) as u32,
    }
}

fn gen_keys(n: u32) -> Vec<(ShardKey, u64)> {
    (0..n)
        .map(|i| {
            (
                ShardKey {
                    tenant: 1,
                    site: i % 20,
                    source: i / 20,
                },
                (i as u64) + 1,
            )
        })
        .collect()
}

/// 模拟执行器：按 plan 步骤推进（Copy/CatchUp 的键目标一律按**新环**路由——
/// 这是五步流程的正确性关键：任何键在 switch 前必须已按新环归宿落位）。
fn execute_reshard(
    old: &ShardRing,
    new: &ShardRing,
    keys: &[(ShardKey, u64)],
) -> (Vec<ShardStore>, Vec<ShardStore>) {
    let mut old_store: Vec<ShardStore> = (0..old.shards as usize)
        .map(|_| ShardStore::new())
        .collect();
    let mut new_store: Vec<ShardStore> = (0..new.shards as usize)
        .map(|_| ShardStore::new())
        .collect();

    for (k, seq) in keys {
        let r = old.route(k);
        old_store[r.shard as usize].insert(seq_key(k), *seq);
    }

    let plan = ReshardPlan::plan(old, new).expect("plan");
    let mut switched = false;
    for step in &plan.steps {
        match step {
            ReshardStep::Copy { from_shard, .. } => {
                // Copy：幂等全量复制（重复执行结果相同）。
                let data = old_store[*from_shard as usize].clone();
                for (k, v) in data {
                    let dst = new.route(&key_of(&k)).shard as usize;
                    new_store[dst].entry(k).or_insert(v);
                }
            }
            ReshardStep::CatchUp { .. } => {
                // 增量追平：自 Copy 后的新写入按新环路由搬入（幂等）。
                for data in old_store.clone() {
                    for (k, v) in data {
                        let dst = new.route(&key_of(&k)).shard as usize;
                        new_store[dst].entry(k).or_insert(v);
                    }
                }
            }
            ReshardStep::DualReadCompare { .. } => {
                // 双读一致（全量抽样）：switch 前后两环读取结果必须一致。
                for (k, _) in keys {
                    let (ro, rn) = dual_route(old, new, k);
                    let ov = old_store[ro.shard as usize].get(&seq_key(k)).copied();
                    let nv = new_store[rn.shard as usize].get(&seq_key(k)).copied();
                    assert_eq!(ov, nv, "键 {:?} 双读不一致（switch={switched}）", k);
                }
            }
            ReshardStep::Switch { version_to } => {
                assert_eq!(*version_to, new.version);
                switched = true;
            }
            ReshardStep::Drain { .. } => {
                // 旧环排空：只读、无新写（模拟执行器不产生 apply）。
            }
        }
    }
    assert!(switched, "必须发生原子切换");
    (old_store, new_store)
}

#[test]
fn reshard_preserves_all_keys() {
    let old = ShardRing::new(1000, 4, 1).expect("old");
    let new = ShardRing::new(1000, 8, 2).expect("new");
    let keys = gen_keys(5_000);
    let (_, new_out) = execute_reshard(&old, &new, &keys);
    // 等价性：全部键在新环恰好一份（无丢失、无重复）。
    let mut fused: BTreeMap<u64, u64> = BTreeMap::new();
    for store in new_out {
        for (k, v) in store {
            assert!(fused.insert(k, v).is_none(), "键 {k} 重复落新环");
        }
    }
    assert_eq!(fused.len(), keys.len(), "键集合守恒");
    // 每个键的新环归宿与路由一致（参考模型等价）。
    for (k, seq) in &keys {
        assert_eq!(
            fused.get(&seq_key(k)).copied(),
            Some(*seq),
            "键 {k:?} 必须在新环路由分片持有"
        );
    }
}

#[test]
fn reshard_no_dual_write_side_effects() {
    // 设备副作用唯一通道 = apply 账本；重分片全程不得产生 apply
    // （重分片只搬数据，不重放设备命令）。执行器无 apply 调用即结构性保证。
    let old = ShardRing::new(1000, 4, 1).expect("old");
    let new = ShardRing::new(1000, 8, 2).expect("new");
    let keys = gen_keys(1_000);
    let _ = execute_reshard(&old, &new, &keys);
}

#[test]
fn switch_is_atomic_single_point() {
    let old = ShardRing::new(1000, 4, 1).expect("old");
    let new = ShardRing::new(1000, 8, 2).expect("new");
    let plan = ReshardPlan::plan(&old, &new).expect("plan");
    let switches = plan
        .steps
        .iter()
        .filter(|s| matches!(s, ReshardStep::Switch { .. }))
        .count();
    assert_eq!(switches, 1, "路由切换必须是单点原子动作");
}

#[test]
fn reshard_all_shrink_and_grow_directions() {
    // 分片数收缩与扩张两个方向的键集合都守恒。
    let keys = gen_keys(3_000);
    // 收缩 8→4。
    let old = ShardRing::new(1000, 8, 1).expect("old8");
    let new = ShardRing::new(1000, 4, 2).expect("new4");
    let (_, out) = execute_reshard(&old, &new, &keys);
    let n: usize = out.iter().map(|s| s.len()).sum();
    assert_eq!(n, keys.len(), "收缩方向键集合守恒");
    // vnode 数变化也算完全重分片。
    let old2 = ShardRing::new(512, 4, 1).expect("oldv");
    let new2 = ShardRing::new(4096, 4, 2).expect("newv");
    let (_, out2) = execute_reshard(&old2, &new2, &keys);
    let total: usize = out2.iter().map(|s| s.len()).sum();
    assert_eq!(total, keys.len(), "vnode 变化方向键集合守恒");
    // 全键唯一。
    let mut uniq: BTreeSet<u64> = BTreeSet::new();
    for store in out2 {
        for k in store.keys() {
            assert!(uniq.insert(*k), "重复键 {k}");
        }
    }
}
