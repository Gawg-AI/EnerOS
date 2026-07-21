# EnerOS Wear-Leveling Monitoring Design — v0.24.1

> **Scope**: Observability and lifespan-prediction layer for flash storage
> wear, layered on top of littlefs2's built-in dynamic wear leveling.
>
> **Module**: `eneros-fs::wear` (`crates/drivers/fs/src/wear/`)
> **Version**: v0.24.1 (Phase 1 — rigid sub-version)
> **Status**: Implemented — host tests pass, aarch64 cross-build verified.

---

## 1. Overview

v0.24.1 adds a **monitoring and control layer** for flash wear, complementing
littlefs2's internal wear leveling. The littlefs2 crate already performs
dynamic wear leveling at the block-allocation level; v0.24.1 provides:

- **Per-block erase counting** — explicit accounting independent of littlefs2.
- **Wear distribution analysis** — p50/p99/max percentile statistics.
- **Write amplification tracking** — ratio of flash writes to app writes.
- **Lifespan estimation** — remaining-years prediction under a write workload.
- **Victim block selection** — hot-block identification for proactive GC.

This is a **rigid sub-version** (蓝图 §刚性子版本列表) because it gates the
10-year maintenance-free deployment requirement (蓝图 §7.5 出口判定).

### Path Correction

The original blueprint placed wear leveling at `crates/kernel/mm/src/wear_level.rs`.
This was corrected to `crates/drivers/fs/src/wear/` per project rules §2.3.2
(subsystem attribution): wear leveling is a storage concept, not a memory
management concept.

---

## 2. Why Not Modify littlefs2?

littlefs2's internal wear leveling is opaque — it doesn't expose per-block
erase counts. Rather than fork littlefs2 or add fragile hooks, v0.24.1
maintains a **parallel accounting** that the storage layer updates whenever
an erase occurs. This:

- Avoids touching the third-party C code.
- Lets us define energy-specific metrics (e.g., daily write budget).
- Provides a clean Rust API for Agent Runtime health monitoring.

The trade-off is that our erase counts are only as accurate as our
instrumentation — callers must invoke `record_erase()` from their `BlockDevice`
implementation. The mock and future eMMC/NVMe drivers do this.

---

## 3. Module Structure

```
crates/drivers/fs/src/wear/
├── mod.rs        — Module exports + global interface (spin::Mutex)
├── status.rs     — WearStatus + WearDistribution
├── manager.rs    — WearLevelManager + WearLeveling trait
└── write_amp.rs  — WriteAmplificationTracker
```

---

## 4. Core Types

### `WearDistribution`

```rust
pub struct WearDistribution {
    pub p50: u32,      // median erases
    pub p99: u32,      // 99th percentile erases
    pub max_erases: u32,
}
```

Computed from a slice of per-block erase counts via `from_counts()`. Sorts
the counts and indexes:

- `p50_idx = len / 2`
- `p99_idx = (len * 99) / 100`

`balance_ratio() = max_erases / p50` — a value > 1.5 indicates imbalance.

### `WearStatus`

```rust
pub struct WearStatus {
    pub total_wear_cycles: u64,
    pub max_block_erases: u32,
    pub avg_block_erases: f64,
    pub wear_distribution: WearDistribution,
    pub write_amplification: f64,
    pub estimated_lifespan_years: f64,
}
```

Aggregated snapshot returned by `wear_level_status()`. Convenience predicates:

- `is_balanced()` — `max / avg < 1.5`
- `is_write_amp_healthy()` — `write_amplification < 2.0`

### `WearLevelManager`

```rust
pub struct WearLevelManager {
    block_erase_count: BTreeMap<u32, u32>,
    total_blocks: u32,
    block_size: u32,
    max_erase_cycles: u32,
    write_amp_tracker: WriteAmplificationTracker,
    gc_threshold: f64,
}
```

Implements the `WearLeveling` trait:

```rust
pub trait WearLeveling {
    fn record_erase(&mut self, block: u32);
    fn select_victim_block(&self) -> Option<u32>;
    fn estimate_lifespan(&self, daily_write_mb: f64) -> f64;
    fn wear_level_status(&self) -> WearStatus;
}
```

### `WriteAmplificationTracker`

```rust
pub struct WriteAmplificationTracker {
    app_bytes_written: u64,
    flash_bytes_written: u64,
    write_amp_limit: f64,
}
```

- `record_app_write(bytes)` — called when the application issues a write.
- `record_flash_write(bytes)` — called when the storage layer actually writes
  to flash (may be larger due to COW, metadata, GC).
- `write_amplification() = flash_bytes / app_bytes`
- `is_throttled()` — true when WA exceeds the configured limit.

---

## 5. Lifespan Estimation

The `estimate_lifespan(daily_write_mb)` formula:

```
total_remaining_erases = sum(max_erase_cycles - count[block] for all blocks)
daily_flash_writes = daily_write_mb * 1e6 * write_amplification
daily_erases_per_block = daily_flash_writes / (block_size * total_blocks)
lifespan_days = total_remaining_erases / daily_erases_per_block
lifespan_years = lifespan_days / 365.25
```

### 10-Year Requirement Verification

With SLC flash parameters (蓝图 §7.5):

- `total_blocks = 65536`
- `block_size = 4096`
- `max_erase_cycles = 100,000`
- `daily_write_mb = 500`
- `write_amplification = 2.0` (assumed worst-case healthy)

Computation:

```
total_remaining_erases = 65536 * 100000 = 6.55e9
daily_flash_writes = 500 * 1e6 * 2.0 = 1e9 bytes/day
daily_erases_per_block = 1e9 / (4096 * 65536) ≈ 3.725
lifespan_days = 6.55e9 / 3.725 ≈ 1.76e9
lifespan_years ≈ 4.8 million years
```

Even with 1000× worse parameters, the estimate comfortably exceeds 10 years.
The unit test `test_lifespan_10_year_requirement` verifies this with the
default SLC configuration.

---

## 6. Victim Block Selection

`select_victim_block()` returns the block with the highest erase count — the
"hot" block that should be migrated first. `trigger_wear_leveling(max)`
returns up to `max` victim blocks sorted by descending wear, but only if
`balance_ratio > gc_threshold` (default 1.5). When wear is balanced, it
returns an empty vector — no proactive migration needed.

This is a **cooperative** mechanism: the caller (a future GC task) is
responsible for actually migrating data. littlefs2's own allocator will
naturally avoid hot blocks; this API provides explicit guidance for
proactive leveling.

---

## 7. Global Interface

The global instance is behind `spin::Mutex<Option<WearLevelManager>>`:

```rust
static GLOBAL_MANAGER: Mutex<Option<WearLevelManager>> = Mutex::new(None);
```

Initialization:

```rust
eneros_fs::wear::init_global(65536, 4096, 100_000);
// or
eneros_fs::wear::init_default();  // 65536 blocks, 4KB, 100K cycles
```

Callers (storage drivers, GC task, monitoring agent) invoke:

- `record_erase(block)` — from `BlockDevice::erase_block` implementations.
- `record_app_write(bytes)` — from the filesystem write path.
- `record_flash_write(bytes)` — from the storage layer's actual flash writes.
- `wear_level_status()` — periodic health polling.
- `trigger_wear_leveling(max)` — proactive GC guidance.
- `set_write_amp_limit(limit)` — configure throttling threshold.

### no_std Compliance

- `spin::Mutex` instead of `std::sync::Mutex`.
- `alloc::collections::BTreeMap` instead of `std::collections::HashMap`.
- No `std::time` — lifespans are computed from caller-provided rates.

---

## 8. Configuration

See `configs/wear-level.toml`. Key parameters:

| Parameter           | Default | Description                                  |
| ------------------- | ------- | -------------------------------------------- |
| `gc_threshold`      | 1.5     | balance_ratio above which GC is triggered    |
| `write_amp_limit`   | 2.0     | WA above which writes are throttled          |
| `max_block_erases`  | 100000  | SLC erase cycle rating                       |
| `daily_write_mb`    | 500     | Expected daily write volume for lifespan est |

---

## 9. Relationship to littlefs2 WL

littlefs2's wear leveling is **implicit**: the allocator prefers blocks with
fewer erase cycles (it tracks this internally). v0.24.1's layer is
**explicit**: it provides queryable statistics and proactive victim selection.
They are complementary:

- littlefs2 handles the routine case efficiently.
- v0.24.1 handles monitoring, alerting, and proactive migration guidance.

There is no double-counting: v0.24.1 only counts erases that the storage
layer explicitly reports via `record_erase()`.

---

## 10. Test Coverage

The module has 55 unit tests across four files:

- `status.rs` — 15 tests (percentile computation, balance predicates).
- `manager.rs` — 18 tests (erase recording, victim selection, lifespan).
- `write_amp.rs` — 13 tests (ratio calculation, throttling).
- `mod.rs` — 9 tests (global interface, initialization).

Key scenarios tested:

- `test_lifespan_10_year_requirement` — verifies ≥10 years at 500 MB/day.
- `test_wear_distribution_skewed_large` — p99 falls on outlier at 200 blocks.
- `test_record_erase_saturating` — counts don't overflow at 200K erases.
- `test_global_trigger_wear_leveling` — victims returned when imbalanced.
- `test_global_trigger_balanced` — no victims when wear is uniform.

---

## 11. References

- EnerOS file system design: `docs/drivers/lfs-design.md`
- EnerOS storage driver design: `docs/drivers/storage-driver-design.md`
- littlefs wear leveling: https://github.com/ARMmbed/littlefs/blob/master/DESIGN.md
- Blueprint §7.5 (10-year maintenance-free requirement)
- Project rules §2.3.2 (subsystem attribution — why this is in drivers/fs)
