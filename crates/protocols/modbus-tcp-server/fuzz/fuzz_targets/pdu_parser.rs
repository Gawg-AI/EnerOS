//! libFuzzer target for the Modbus TCP PDU parser (NET-016).
//!
//! Feeds arbitrary byte sequences to `handle_frame` under `WritePolicy::DenyAll`
//! and asserts the parser is total (no panic) and that the default-deny write
//! invariant holds (no register mutation for any input).
//!
//! Run (Linux, nightly + -Zsanitizer=address):
//! ```bash
//! cargo +nightly fuzz run pdu_parser -- -max_total_time=60
//! ```
//!
//! The proptest suite in `tests/fuzz.rs` is the primary, CI-portable fuzzing
//! evidence. This libFuzzer target is supplementary: it gives coverage-guided
//! fuzzing on CI runners that have libFuzzer + AddressSanitizer.

#![no_main]

use eneros_modbus_tcp_server::handler::handle_frame;
use eneros_modbus_tcp_server::{RegisterStore, WritePolicy};
use libfuzzer_sys::fuzz_target;

const REG_COUNT: usize = 32;

fuzz_target!(|data: &[u8]| {
    let mut store = RegisterStore::new(REG_COUNT);
    // Snapshot the store; under DenyAll no input may mutate it. This is the
    // control-gateway chokepoint invariant: a protocol input alone can never
    // write a device.
    let snapshot = store.regs.clone();

    let outcome = handle_frame(data, &mut store, WritePolicy::DenyAll);

    // Invariant 1 (totality): we reached here, so no panic.
    // Invariant 2 (default-deny): the store is unchanged.
    assert_eq!(
        store.regs, snapshot,
        "DEFAULT-DENY VIOLATION: input mutated the store under DenyAll"
    );

    // Invariant 3 (bounded allocation): any Response PDU is ≤ 252 bytes
    // (1 fc + 1 byte_count + 2*125 regs). This catches a parser that trusts an
    // attacker-controlled length field to size a Vec.
    if let eneros_modbus_tcp_server::HandleOutcome::Response(resp) = outcome {
        assert!(
            resp.pdu.len() <= 252,
            "unbounded PDU allocation: {} bytes",
            resp.pdu.len()
        );
    }
});
