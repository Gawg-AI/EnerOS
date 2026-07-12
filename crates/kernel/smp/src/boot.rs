//! Multi-core boot support.
//!
//! Tracks per-core state, wakes secondaries via PSCI `CPU_ON`, and provides
//! the secondary-core entry point that brings a core to the `Online` state
//! and then parks in a `wfe` loop (real scheduler hookup comes in a later
//! version).

use core::sync::atomic::{AtomicU8, Ordering};

use spin::Mutex;

/// Maximum number of CPU cores supported.
const MAX_CORES: usize = 8;

/// Lifecycle state of a CPU core.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreState {
    Offline = 0,
    Booting = 1,
    Online = 2,
    Halted = 3,
}

/// Static description of a CPU core.
#[derive(Debug, Clone, Copy)]
pub struct CoreInfo {
    pub id: u32,
    pub entry: u64,
    pub stack_base: u64,
    pub state: CoreState,
}

// ---------------------------------------------------------------------------
// Globals
// ---------------------------------------------------------------------------

static CORES: Mutex<[CoreInfo; MAX_CORES]> = Mutex::new([
    CoreInfo {
        id: 0,
        entry: 0,
        stack_base: 0,
        state: CoreState::Offline,
    },
    CoreInfo {
        id: 1,
        entry: 0,
        stack_base: 0,
        state: CoreState::Offline,
    },
    CoreInfo {
        id: 2,
        entry: 0,
        stack_base: 0,
        state: CoreState::Offline,
    },
    CoreInfo {
        id: 3,
        entry: 0,
        stack_base: 0,
        state: CoreState::Offline,
    },
    CoreInfo {
        id: 4,
        entry: 0,
        stack_base: 0,
        state: CoreState::Offline,
    },
    CoreInfo {
        id: 5,
        entry: 0,
        stack_base: 0,
        state: CoreState::Offline,
    },
    CoreInfo {
        id: 6,
        entry: 0,
        stack_base: 0,
        state: CoreState::Offline,
    },
    CoreInfo {
        id: 7,
        entry: 0,
        stack_base: 0,
        state: CoreState::Offline,
    },
]);

static CORE_STATES: [AtomicU8; MAX_CORES] = [
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
    AtomicU8::new(0),
];

static CORE_COUNT: Mutex<u32> = Mutex::new(1);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read the current CPU core id.
///
/// On aarch64 reads `MPIDR_EL1` and masks to Aff0. On host (test) returns 0.
#[cfg(target_arch = "aarch64")]
pub fn read_core_id() -> u32 {
    let id: u64;
    unsafe {
        core::arch::asm!(
            "mrs {}, mpidr_el1",
            out(reg) id,
            options(nostack, preserves_flags),
        );
    }
    (id & 0xff) as u32
}

#[cfg(not(target_arch = "aarch64"))]
pub fn read_core_id() -> u32 {
    0
}

/// Initialize the SMP subsystem with the given number of cores.
///
/// Sets `CORE_COUNT` and resets the `CORES` table for cores `0..core_count`.
/// Cores beyond `MAX_CORES` are silently ignored.
pub fn smp_init(core_count: u32) {
    *CORE_COUNT.lock() = core_count;
    let mut cores = CORES.lock();
    let count = core::cmp::min(core_count as usize, MAX_CORES);
    for i in 0..count {
        cores[i].id = i as u32;
        cores[i].state = CoreState::Offline;
        CORE_STATES[i].store(CoreState::Offline as u8, Ordering::Release);
    }
}

/// Wake a secondary core via PSCI `CPU_ON`.
///
/// On aarch64 issues an `hvc #0` call with:
/// - x0 = `0x8400_000E` (PSCI `CPU_ON` function id)
/// - x1 = target MPIDR (aff0 = core_id)
/// - x2 = entry address
///
/// Sets the target core's state to `Booting` before issuing the call.
/// On host the state is still updated and the call is a no-op.
#[cfg(target_arch = "aarch64")]
pub fn wake_secondary(core_id: u32, entry: u64) {
    set_core_state(core_id, CoreState::Booting);
    let psci_fn: u64 = 0x8400_000E;
    let target_mpidr: u64 = core_id as u64;
    let entry_addr: u64 = entry;
    unsafe {
        core::arch::asm!(
            "hvc #0",
            in("x0") psci_fn,
            in("x1") target_mpidr,
            in("x2") entry_addr,
        );
    }
}

#[cfg(not(target_arch = "aarch64"))]
pub fn wake_secondary(core_id: u32, _entry: u64) {
    set_core_state(core_id, CoreState::Booting);
}

/// Secondary core entry point.
///
/// Called (indirectly) by a secondary core after PSCI `CPU_ON` releases it.
/// 1. Reads the core id.
/// 2. Marks the core as `Booting`.
/// 3. GIC redistributor init stub (real init deferred to hardware bring-up).
/// 4. Marks the core as `Online`.
/// 5. Parks in a `wfe` loop (real scheduler hookup comes in a later version).
#[cfg(target_arch = "aarch64")]
pub fn secondary_entry() -> ! {
    let id = read_core_id();
    set_core_state(id, CoreState::Booting);
    // GIC redistributor initialization stub.
    // Real redistributor wake-up will be performed on actual hardware in a
    // later version; this placeholder keeps the secondary core's state
    // machine correct without touching MMIO.
    set_core_state(id, CoreState::Online);
    loop {
        // SAFETY: `wfe` is a hint instruction that does not touch memory or
        // flags; it simply waits for an event. Inline asm still requires an
        // unsafe block on aarch64.
        unsafe {
            core::arch::asm!("wfe", options(nostack, preserves_flags));
        }
    }
}

#[cfg(not(target_arch = "aarch64"))]
pub fn secondary_entry() -> ! {
    let id = read_core_id();
    set_core_state(id, CoreState::Booting);
    set_core_state(id, CoreState::Online);
    loop {
        core::hint::spin_loop();
    }
}

/// Query the state of a core.
///
/// Returns `None` for core ids >= `MAX_CORES`. Unknown discriminants map to
/// `Offline` (defensive — should not occur in practice).
pub fn core_state(id: u32) -> Option<CoreState> {
    if id as usize >= MAX_CORES {
        return None;
    }
    let raw = CORE_STATES[id as usize].load(Ordering::Acquire);
    let state = match raw {
        1 => CoreState::Booting,
        2 => CoreState::Online,
        3 => CoreState::Halted,
        _ => CoreState::Offline,
    };
    Some(state)
}

/// Set the state of a core.
///
/// Updates both the per-core atomic state and the `CORES` table. Core ids
/// >= `MAX_CORES` are silently ignored.
pub fn set_core_state(id: u32, state: CoreState) {
    if id as usize >= MAX_CORES {
        return;
    }
    CORE_STATES[id as usize].store(state as u8, Ordering::Release);
    let mut cores = CORES.lock();
    cores[id as usize].state = state;
}

/// Returns the configured number of CPU cores.
pub fn core_count() -> u32 {
    *CORE_COUNT.lock()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_core_state_variants() {
        assert_ne!(CoreState::Offline, CoreState::Booting);
        assert_ne!(CoreState::Booting, CoreState::Online);
        assert_ne!(CoreState::Online, CoreState::Halted);
        assert_ne!(CoreState::Offline, CoreState::Halted);
    }

    #[test]
    fn test_core_state_repr_u8() {
        assert_eq!(CoreState::Offline as u8, 0);
        assert_eq!(CoreState::Booting as u8, 1);
        assert_eq!(CoreState::Online as u8, 2);
        assert_eq!(CoreState::Halted as u8, 3);
    }

    #[test]
    fn test_core_info_construction() {
        let info = CoreInfo {
            id: 2,
            entry: 0x4000_0000,
            stack_base: 0x8000_0000,
            state: CoreState::Online,
        };
        assert_eq!(info.id, 2);
        assert_eq!(info.entry, 0x4000_0000);
        assert_eq!(info.stack_base, 0x8000_0000);
        assert_eq!(info.state, CoreState::Online);
    }

    #[test]
    fn test_core_state_query() {
        let _g = lock();
        set_core_state(0, CoreState::Offline);
        assert_eq!(core_state(0), Some(CoreState::Offline));
    }

    #[test]
    fn test_set_core_state() {
        let _g = lock();
        set_core_state(0, CoreState::Booting);
        assert_eq!(core_state(0), Some(CoreState::Booting));
        set_core_state(0, CoreState::Online);
        assert_eq!(core_state(0), Some(CoreState::Online));
        // Restore
        set_core_state(0, CoreState::Offline);
    }

    #[test]
    fn test_smp_init() {
        let _g = lock();
        smp_init(4);
        assert_eq!(core_count(), 4);
        // Restore
        smp_init(1);
        assert_eq!(core_count(), 1);
    }

    #[test]
    fn test_read_core_id_host_returns_zero() {
        assert_eq!(read_core_id(), 0);
    }
}
