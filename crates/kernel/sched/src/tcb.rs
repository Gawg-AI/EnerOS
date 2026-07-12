//! Thread Control Block and state machine — Phase 0 P0-F (v0.18.0).
//!
//! Provides:
//! - [`ThreadState`] — five-state thread lifecycle
//! - [`Tcb`] — thread control block with stack/sp/pc
//! - Global thread management API ([`thread_create`]/[`thread_block`]/[`thread_resume`]/[`thread_destroy`]/[`thread_exit`]/[`thread_yield`]/[`thread_state`])
//!
//! Per D2, this module uses `alloc` (Rust built-in, not an external dep)
//! for `Box<Tcb>` allocation. Per D3, aarch64 inline asm is cfg-gated.

use alloc::boxed::Box;
use core::cell::UnsafeCell;

use crate::percore::Spinlock;
use crate::Tid;
use crate::MAX_THREADS;

/// Five-state thread lifecycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreadState {
    Ready,
    Running,
    Blocked,
    Suspended,
    Dead,
}

/// Thread control block.
///
/// Holds the saved stack pointer, program counter, priority, and stack
/// bounds. The stack is heap-allocated (`alloc::alloc::alloc`); the `Tcb`
/// itself is stored in the global [`THREAD_TABLE`].
///
/// Per D5, this struct contains raw pointers (`stack`, `stack_top`) and
/// therefore does NOT auto-implement `Send`/`Sync`. Access is serialized
/// through the global `THREAD_TABLE` spinlock.
pub struct Tcb {
    pub tid: Tid,
    pub state: ThreadState,
    pub priority: u8,       // 0 = highest
    pub stack: *mut u8,     // stack base (low address)
    pub stack_top: *mut u8, // stack top (initial sp, high address)
    pub stack_size: usize,
    pub sp: u64, // saved stack pointer
    pub pc: u64, // saved program counter
    pub entry: fn() -> !,
    pub partition: u32,
}

impl Tcb {
    /// Construct a new Tcb.
    ///
    /// The caller must ensure `stack` points to a valid buffer of at least
    /// `size` bytes. The stack is not owned by the Tcb — it must be freed
    /// separately when the thread is destroyed.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn new(tid: Tid, entry: fn() -> !, stack: *mut u8, size: usize, priority: u8) -> Self {
        let stack_top = unsafe { stack.add(size) };
        // SAFETY: `stack_top` is derived from `stack + size` which points to the
        // upper bound of the caller-provided stack buffer.
        let sp = unsafe { init_stack_frame(stack_top, entry as *const () as u64) };
        Self {
            tid,
            state: ThreadState::Ready,
            priority,
            stack,
            stack_top,
            stack_size: size,
            sp,
            pc: entry as *const () as u64,
            entry,
            partition: 0,
        }
    }

    /// Transition the thread to `next` state.
    ///
    /// Returns `Err("invalid transition")` if the transition is not in the
    /// legal set. Legal transitions:
    /// - Ready → Running
    /// - Running → Ready
    /// - Running → Blocked
    /// - Blocked → Ready
    /// - Ready → Suspended
    /// - Suspended → Ready
    /// - Running → Dead
    /// - Ready → Dead
    pub fn transition(&mut self, next: ThreadState) -> Result<(), &'static str> {
        use ThreadState::*;
        let ok = matches!(
            (self.state, next),
            (Ready, Running)
                | (Running, Ready)
                | (Running, Blocked)
                | (Blocked, Ready)
                | (Ready, Suspended)
                | (Suspended, Ready)
                | (Running, Dead)
                | (Ready, Dead)
        );
        if ok {
            self.state = next;
            Ok(())
        } else {
            Err("invalid transition")
        }
    }
}

/// Initialize a stack frame for a new thread (aarch64).
///
/// Layout: 272 bytes = 31 general-purpose registers (x0-x30) × 8 + spsr_el1 + elr_el1.
/// - x30 (lr) = entry address
/// - elr_el1 = entry address
/// - spsr_el1 = 0x3C5 (EL1h, IRQ unmasked)
///
/// # Safety
///
/// `stack_top` must point to the upper bound of a valid writable stack
/// buffer with at least 272 bytes of space below it. The caller must
/// ensure the stack memory is valid and properly aligned.
#[cfg(target_arch = "aarch64")]
pub unsafe fn init_stack_frame(stack_top: *mut u8, entry: u64) -> u64 {
    let mut sp = stack_top as u64;
    sp -= 272; // 31 regs + spsr + elr
    let frame = sp as *mut u64;
    *frame.add(30) = entry; // x30 (lr)
    *frame.add(31) = entry; // elr_el1
    *frame.add(32) = 0x3C5; // spsr_el1
    sp
}

/// Host-side stub: no real stack frame, just return stack_top.
///
/// # Safety
///
/// `stack_top` must be a valid pointer. On host, no stack frame is
/// written — this is a no-op stub that returns `stack_top as u64`.
#[cfg(not(target_arch = "aarch64"))]
pub unsafe fn init_stack_frame(stack_top: *mut u8, _entry: u64) -> u64 {
    stack_top as u64
}

// ---------------------------------------------------------------------------
// Global thread table
// ---------------------------------------------------------------------------

/// Wrapper around the global thread table.
///
/// Combines a [`Spinlock`] with an `UnsafeCell`-protected array of
/// `Option<Box<Tcb>>`. The `UnsafeCell` provides interior mutability; the
/// spinlock serializes access. `Sync` is implemented manually because the
/// inner type contains raw pointers (via `Tcb`) and is not auto-`Sync`.
///
/// # Safety
///
/// `Sync` is sound because all access to `entries` is gated by `lock`.
/// Callers must acquire `lock` before touching `entries`.
struct ThreadTable {
    lock: Spinlock,
    entries: UnsafeCell<[Option<Box<Tcb>>; MAX_THREADS]>,
}

// SAFETY: Access to `entries` is serialized by `lock`. All public API
// functions acquire the spinlock before reading/writing `entries`.
unsafe impl Sync for ThreadTable {}

static THREAD_TABLE: ThreadTable = ThreadTable {
    lock: Spinlock::new(),
    entries: UnsafeCell::new([const { None }; MAX_THREADS]),
};

/// Create a new thread with the given entry function, stack size, and priority.
///
/// Allocates a stack via `alloc::alloc::alloc`, finds a free slot in the
/// global thread table, and inserts a `Box<Tcb>`.
///
/// Returns the new `Tid` (≥ 1) on success, or `Tid(0)` on failure (table full
/// or stack allocation failed). `Tid(0)` is reserved as "invalid".
pub fn thread_create(entry: fn() -> !, stack_size: usize, priority: u8) -> Tid {
    use alloc::alloc::{alloc, dealloc};
    use core::alloc::Layout;

    let layout = match Layout::from_size_align(stack_size, 16) {
        Ok(l) => l,
        Err(_) => return Tid(0),
    };
    let stack = unsafe { alloc(layout) };
    if stack.is_null() {
        return Tid(0);
    }

    THREAD_TABLE.lock.lock();
    // SAFETY: We hold the lock, so exclusive access is guaranteed.
    let entries = unsafe { &mut *THREAD_TABLE.entries.get() };
    let mut found_idx: Option<usize> = None;
    for (i, entry) in entries.iter().enumerate().take(MAX_THREADS) {
        if entry.is_none() {
            found_idx = Some(i);
            break;
        }
    }

    let tid = match found_idx {
        Some(idx) => {
            let tcb = Tcb::new(Tid((idx + 1) as u32), entry, stack, stack_size, priority);
            entries[idx] = Some(Box::new(tcb));
            Tid((idx + 1) as u32)
        }
        None => {
            // Table full — free the stack we just allocated.
            unsafe { dealloc(stack, layout) };
            Tid(0)
        }
    };
    THREAD_TABLE.lock.unlock();
    tid
}

/// Destroy a thread, freeing its stack and removing it from the table.
///
/// Returns `Err` if the thread does not exist or is currently `Running`
/// (a running thread must be stopped first). On success, the stack is
/// deallocated and the table slot is cleared.
pub fn thread_destroy(tid: Tid) -> Result<(), &'static str> {
    use alloc::alloc::dealloc;
    use core::alloc::Layout;

    if tid.0 == 0 || tid.0 as usize > MAX_THREADS {
        return Err("invalid tid");
    }
    let idx = (tid.0 - 1) as usize;

    THREAD_TABLE.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &mut *THREAD_TABLE.entries.get() };

    let can_destroy = matches!(
        &entries[idx],
        Some(tcb) if tcb.state != ThreadState::Running
    );

    if !can_destroy {
        THREAD_TABLE.lock.unlock();
        return Err("cannot destroy: thread is running or does not exist");
    }

    // Take the Tcb out and free its stack.
    if let Some(tcb_box) = entries[idx].take() {
        if !tcb_box.stack.is_null() && tcb_box.stack_size > 0 {
            if let Ok(layout) = Layout::from_size_align(tcb_box.stack_size, 16) {
                unsafe { dealloc(tcb_box.stack, layout) };
            }
        }
        // tcb_box dropped here — frees the Tcb allocation.
    }

    THREAD_TABLE.lock.unlock();
    Ok(())
}

/// Block a thread (transition to `Blocked`).
///
/// Returns `Err` if the thread does not exist or the transition is invalid
/// (e.g. already `Blocked` or `Dead`).
pub fn thread_block(tid: Tid) -> Result<(), &'static str> {
    if tid.0 == 0 || tid.0 as usize > MAX_THREADS {
        return Err("invalid tid");
    }
    let idx = (tid.0 - 1) as usize;

    THREAD_TABLE.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &mut *THREAD_TABLE.entries.get() };
    let result = match &mut entries[idx] {
        Some(tcb) => tcb.transition(ThreadState::Blocked),
        None => Err("no such thread"),
    };
    THREAD_TABLE.lock.unlock();
    result
}

/// Resume a blocked/suspended thread (transition to `Ready`).
///
/// Returns `Err` if the thread does not exist or the transition is invalid.
pub fn thread_resume(tid: Tid) -> Result<(), &'static str> {
    if tid.0 == 0 || tid.0 as usize > MAX_THREADS {
        return Err("invalid tid");
    }
    let idx = (tid.0 - 1) as usize;

    THREAD_TABLE.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &mut *THREAD_TABLE.entries.get() };
    let result = match &mut entries[idx] {
        Some(tcb) => tcb.transition(ThreadState::Ready),
        None => Err("no such thread"),
    };
    THREAD_TABLE.lock.unlock();
    result
}

/// Exit the current thread.
///
/// Marks the thread as dead, frees its stack, and removes it from the table.
/// This function never returns — after cleanup, it spins forever (in a real
/// OS, it would context-switch to the next runnable thread).
pub fn thread_exit(tid: Tid) -> ! {
    use alloc::alloc::dealloc;
    use core::alloc::Layout;

    if tid.0 != 0 && tid.0 as usize <= MAX_THREADS {
        let idx = (tid.0 - 1) as usize;
        THREAD_TABLE.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &mut *THREAD_TABLE.entries.get() };
        if let Some(tcb_box) = entries[idx].take() {
            if !tcb_box.stack.is_null() && tcb_box.stack_size > 0 {
                if let Ok(layout) = Layout::from_size_align(tcb_box.stack_size, 16) {
                    unsafe { dealloc(tcb_box.stack, layout) };
                }
            }
            // tcb_box dropped here.
        }
        THREAD_TABLE.lock.unlock();
    }
    // No more runnable thread — spin forever.
    loop {
        core::hint::spin_loop();
    }
}

/// Yield the current thread's time slice.
///
/// On host, this is a no-op (no actual context switch). In a real OS, this
/// would transition the current thread to `Ready` and switch to the next
/// runnable thread.
pub fn thread_yield() {
    // Host stub: no actual context switch.
    // In a real OS, this would:
    // 1. Save the current thread's context
    // 2. Transition it to Ready
    // 3. Pick the next runnable thread
    // 4. Restore the next thread's context
    // Here, we simply return (no-op for host testing).
}

/// Query the state of a thread.
///
/// Returns [`ThreadState::Dead`] for `Tid(0)`, out-of-range tids, or
/// threads not present in the table.
pub fn thread_state(tid: Tid) -> ThreadState {
    if tid.0 == 0 || tid.0 as usize > MAX_THREADS {
        return ThreadState::Dead;
    }
    let idx = (tid.0 - 1) as usize;

    THREAD_TABLE.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &*THREAD_TABLE.entries.get() };
    let state = match &entries[idx] {
        Some(tcb) => tcb.state,
        None => ThreadState::Dead,
    };
    THREAD_TABLE.lock.unlock();
    state
}

/// Returns the partition id of the thread identified by `tid`, or `None`
/// if the thread does not exist.
///
/// `Tid(0)` is invalid and always returns `None`.
pub fn thread_partition(tid: Tid) -> Option<u32> {
    if tid.0 == 0 || tid.0 as usize > MAX_THREADS {
        return None;
    }
    let idx = (tid.0 - 1) as usize;
    THREAD_TABLE.lock.lock();
    // SAFETY: We hold the lock.
    let entries = unsafe { &*THREAD_TABLE.entries.get() };
    let partition = entries[idx].as_ref().map(|tcb| tcb.partition);
    THREAD_TABLE.lock.unlock();
    partition
}

// ---------------------------------------------------------------------------
// Current thread tracking
// ---------------------------------------------------------------------------

/// Interior-mutable wrapper for the current thread Tid.
struct CurrentTid {
    lock: Spinlock,
    tid: UnsafeCell<Tid>,
}

// SAFETY: Access to `tid` is serialized by `lock`.
unsafe impl Sync for CurrentTid {}

static CURRENT_TID: CurrentTid = CurrentTid {
    lock: Spinlock::new(),
    tid: UnsafeCell::new(Tid(0)),
};

/// Returns the Tid of the currently executing thread.
///
/// On host, this returns the value set by [`set_current_tid`] (default `Tid(0)`).
/// On aarch64, a real implementation would read from a per-CPU register (e.g.
/// TPIDR_EL0); for Phase 0 the static variable is used for both host and target.
pub fn current_tid() -> Tid {
    CURRENT_TID.lock.lock();
    // SAFETY: We hold the lock.
    let tid = unsafe { *CURRENT_TID.tid.get() };
    CURRENT_TID.lock.unlock();
    tid
}

/// Set the current thread's Tid.
///
/// Used by the scheduler on context switch and by host tests to simulate
/// "who is calling". On aarch64, a real implementation would write to
/// TPIDR_EL0 instead of the static variable.
pub fn set_current_tid(tid: Tid) {
    CURRENT_TID.lock.lock();
    // SAFETY: We hold the lock.
    unsafe {
        *CURRENT_TID.tid.get() = tid;
    }
    CURRENT_TID.lock.unlock();
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

    /// Test entry function that never returns (spins).
    fn test_entry() -> ! {
        loop {
            core::hint::spin_loop();
        }
    }

    /// Clear the global thread table between tests.
    fn cleanup_thread_table() {
        use alloc::alloc::dealloc;
        use core::alloc::Layout;

        THREAD_TABLE.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &mut *THREAD_TABLE.entries.get() };
        for entry in entries.iter_mut() {
            if let Some(tcb_box) = entry.take() {
                if !tcb_box.stack.is_null() && tcb_box.stack_size > 0 {
                    if let Ok(layout) = Layout::from_size_align(tcb_box.stack_size, 16) {
                        unsafe { dealloc(tcb_box.stack, layout) };
                    }
                }
            }
        }
        THREAD_TABLE.lock.unlock();
    }

    /// Directly set a thread's state (test helper).
    fn set_thread_state(tid: Tid, state: ThreadState) {
        if tid.0 == 0 || tid.0 as usize > MAX_THREADS {
            return;
        }
        let idx = (tid.0 - 1) as usize;
        THREAD_TABLE.lock.lock();
        // SAFETY: We hold the lock.
        let entries = unsafe { &mut *THREAD_TABLE.entries.get() };
        if let Some(tcb) = &mut entries[idx] {
            tcb.state = state;
        }
        THREAD_TABLE.lock.unlock();
    }

    #[test]
    fn test_thread_state_derive() {
        let s = ThreadState::Ready;
        assert_eq!(s, ThreadState::Ready);
        assert_ne!(s, ThreadState::Running);
        // Copy semantics: can be reused.
        let s2 = s;
        assert_eq!(s, s2);
        // Debug formatting works.
        let _ = format!("{:?}", ThreadState::Blocked);
    }

    #[test]
    fn test_tcb_new_initializes_state_ready() {
        let _g = lock();
        cleanup_thread_table();

        let mut stack = [0u8; 4096];
        let stack_ptr = stack.as_mut_ptr();
        let tcb = Tcb::new(Tid(1), test_entry, stack_ptr, 4096, 5);
        assert_eq!(tcb.state, ThreadState::Ready);
        assert_eq!(tcb.tid, Tid(1));
        assert_eq!(tcb.priority, 5);
        assert_eq!(tcb.stack_size, 4096);
        assert_eq!(tcb.stack, stack_ptr);
        assert_eq!(tcb.pc, test_entry as *const () as u64);
        assert_eq!(
            tcb.entry as *const () as u64,
            test_entry as *const () as u64
        );
        assert_eq!(tcb.partition, 0);
        // stack_top = stack + size
        assert_eq!(tcb.stack_top, unsafe { stack_ptr.add(4096) });
    }

    #[test]
    fn test_transition_legal_paths() {
        let _g = lock();
        cleanup_thread_table();

        let mut stack = [0u8; 4096];
        let tcb = Tcb::new(Tid(1), test_entry, stack.as_mut_ptr(), 4096, 0);

        // Ready → Running
        let mut t = tcb;
        assert_eq!(t.transition(ThreadState::Running), Ok(()));
        assert_eq!(t.state, ThreadState::Running);

        // Running → Ready
        assert_eq!(t.transition(ThreadState::Ready), Ok(()));
        assert_eq!(t.state, ThreadState::Ready);

        // Ready → Running (for next tests)
        assert_eq!(t.transition(ThreadState::Running), Ok(()));
        // Running → Blocked
        assert_eq!(t.transition(ThreadState::Blocked), Ok(()));
        assert_eq!(t.state, ThreadState::Blocked);

        // Blocked → Ready
        assert_eq!(t.transition(ThreadState::Ready), Ok(()));
        assert_eq!(t.state, ThreadState::Ready);

        // Ready → Suspended
        assert_eq!(t.transition(ThreadState::Suspended), Ok(()));
        assert_eq!(t.state, ThreadState::Suspended);

        // Suspended → Ready
        assert_eq!(t.transition(ThreadState::Ready), Ok(()));
        assert_eq!(t.state, ThreadState::Ready);

        // Ready → Dead
        assert_eq!(t.transition(ThreadState::Dead), Ok(()));
        assert_eq!(t.state, ThreadState::Dead);
    }

    #[test]
    fn test_transition_illegal_paths() {
        let _g = lock();
        cleanup_thread_table();

        // Dead → Running (illegal)
        let mut stack = [0u8; 4096];
        let mut t = Tcb::new(Tid(1), test_entry, stack.as_mut_ptr(), 4096, 0);
        t.state = ThreadState::Dead;
        assert!(t.transition(ThreadState::Running).is_err());
        assert!(t.transition(ThreadState::Ready).is_err());
        assert!(t.transition(ThreadState::Blocked).is_err());

        // Blocked → Running (illegal)
        t.state = ThreadState::Blocked;
        assert!(t.transition(ThreadState::Running).is_err());

        // Suspended → Running (illegal)
        t.state = ThreadState::Suspended;
        assert!(t.transition(ThreadState::Running).is_err());
        assert!(t.transition(ThreadState::Blocked).is_err());

        // Running → Suspended (illegal)
        t.state = ThreadState::Running;
        assert!(t.transition(ThreadState::Suspended).is_err());

        // Dead → Dead (illegal, already dead)
        t.state = ThreadState::Dead;
        assert!(t.transition(ThreadState::Dead).is_err());
    }

    #[test]
    fn test_thread_create_returns_valid_tid() {
        let _g = lock();
        cleanup_thread_table();

        let tid = thread_create(test_entry, 4096, 5);
        assert_ne!(tid, Tid(0), "thread_create should return non-zero Tid");
        assert!(tid.0 >= 1, "Tid should be >= 1");
        assert_eq!(thread_state(tid), ThreadState::Ready);

        cleanup_thread_table();
    }

    #[test]
    fn test_thread_create_invalid_zero() {
        let _g = lock();
        cleanup_thread_table();

        // Tid(0) is invalid and returns Dead state.
        assert_eq!(thread_state(Tid(0)), ThreadState::Dead);
        assert!(thread_block(Tid(0)).is_err());
        assert!(thread_resume(Tid(0)).is_err());
        assert!(thread_destroy(Tid(0)).is_err());

        cleanup_thread_table();
    }

    #[test]
    fn test_thread_block_and_resume() {
        let _g = lock();
        cleanup_thread_table();

        let tid = thread_create(test_entry, 4096, 3);
        assert!(tid.0 > 0);
        assert_eq!(thread_state(tid), ThreadState::Ready);

        // Ready → Blocked (must go through Running first)
        set_thread_state(tid, ThreadState::Running);
        assert_eq!(thread_block(tid), Ok(()));
        assert_eq!(thread_state(tid), ThreadState::Blocked);

        // Blocked → Ready
        assert_eq!(thread_resume(tid), Ok(()));
        assert_eq!(thread_state(tid), ThreadState::Ready);

        cleanup_thread_table();
    }

    #[test]
    fn test_thread_destroy_non_running() {
        let _g = lock();
        cleanup_thread_table();

        let tid = thread_create(test_entry, 4096, 3);
        assert!(tid.0 > 0);
        assert_eq!(thread_state(tid), ThreadState::Ready);

        // Destroy a Ready thread — should succeed.
        assert_eq!(thread_destroy(tid), Ok(()));
        // Thread is now gone — state queries return Dead.
        assert_eq!(thread_state(tid), ThreadState::Dead);
        // Double destroy fails.
        assert!(thread_destroy(tid).is_err());

        cleanup_thread_table();
    }

    #[test]
    fn test_thread_destroy_running_rejected() {
        let _g = lock();
        cleanup_thread_table();

        let tid = thread_create(test_entry, 4096, 3);
        assert!(tid.0 > 0);

        // Force the thread into Running state.
        set_thread_state(tid, ThreadState::Running);
        assert_eq!(thread_state(tid), ThreadState::Running);

        // Destroying a Running thread is rejected.
        assert!(thread_destroy(tid).is_err());
        // Thread is still alive.
        assert_eq!(thread_state(tid), ThreadState::Running);

        // Transition to Ready first, then destroy succeeds.
        set_thread_state(tid, ThreadState::Ready);
        assert_eq!(thread_destroy(tid), Ok(()));

        cleanup_thread_table();
    }

    #[test]
    fn test_thread_state_query() {
        let _g = lock();
        cleanup_thread_table();

        // No thread → Dead.
        assert_eq!(thread_state(Tid(1)), ThreadState::Dead);

        let tid = thread_create(test_entry, 4096, 3);
        assert_eq!(thread_state(tid), ThreadState::Ready);

        // Out-of-range tid → Dead.
        assert_eq!(thread_state(Tid(999)), ThreadState::Dead);

        cleanup_thread_table();
    }

    #[test]
    fn test_thread_block_requires_running() {
        let _g = lock();
        cleanup_thread_table();

        let tid = thread_create(test_entry, 4096, 3);
        // Ready → Blocked is illegal (must be Running → Blocked).
        assert!(thread_block(tid).is_err());
        assert_eq!(thread_state(tid), ThreadState::Ready);

        // Transition to Running, then block succeeds.
        set_thread_state(tid, ThreadState::Running);
        assert_eq!(thread_block(tid), Ok(()));
        assert_eq!(thread_state(tid), ThreadState::Blocked);

        // Blocked → Blocked (illegal).
        assert!(thread_block(tid).is_err());

        cleanup_thread_table();
    }

    #[test]
    fn test_current_tid_default_zero() {
        let _g = lock();
        cleanup_thread_table();
        set_current_tid(Tid(0));
        assert_eq!(current_tid(), Tid(0));
    }

    #[test]
    fn test_set_and_get_current_tid() {
        let _g = lock();
        cleanup_thread_table();
        set_current_tid(Tid(42));
        assert_eq!(current_tid(), Tid(42));
        set_current_tid(Tid(7));
        assert_eq!(current_tid(), Tid(7));
        // Reset for other tests
        set_current_tid(Tid(0));
    }
}
