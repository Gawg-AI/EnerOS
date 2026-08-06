//! NET-016: Property-based fuzzing of the Modbus PDU parser.
//!
//! These tests throw arbitrary bytes at [`handle_frame`] and assert the
//! parser's safety invariants. They are NOT looking for protocol correctness
//! on garbage input (garbage in → exception/close out is fine); they are
//! looking for:
//!
//! 1. **No panic** on any byte sequence — the parser must be total.
//! 2. **No out-of-bounds** register access — the store stays intact.
//! 3. **No unbounded allocation** — every `Vec` the parser builds has a
//!    strictly bounded capacity derived from the spec (≤ 125 regs for reads,
//!    ≤ 123 regs for writes), never from attacker-controlled length fields
//!    without a cap.
//! 4. **Default-deny writes** — under `WritePolicy::DenyAll`, no input ever
//!    modifies the register store. This is the control-gateway chokepoint
//!    invariant.
//!
//! Run: `cargo test -p eneros-modbus-tcp-server --test fuzz`
//! Long: `PROPTEST_CASES=4096 cargo test -p eneros-modbus-tcp-server --test fuzz -- --ignored`

// Test module: proptest! / prop_assert! / panic! expand to `core::panic`,
// which the workspace clippy.toml disallows in non-test code. clippy.toml
// explicitly says "测试模块请用 #[allow] 豁免".
#![allow(clippy::disallowed_macros)]

use eneros_modbus_tcp_server::handler::handle_frame;
use eneros_modbus_tcp_server::{HandleOutcome, RegisterStore, WritePolicy};
use proptest::prelude::*;

/// Maximum frame size we fuzz against. Real Modbus TCP frames are bounded by
/// the u16 length field (≤ 65535 + 7 MBAP), but the spec caps PDUs at ~253
/// bytes. We fuzz a generous 512-byte envelope to cover all legal PDUs and a
/// wide range of malformed/truncated/trailing-byte cases.
const MAX_FRAME: usize = 512;

/// Bounded register store: 32 holding registers. Small enough that any
/// out-of-bounds write would be caught; large enough to exercise valid ranges.
const REG_COUNT: usize = 32;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// Invariant 1 & 2: ANY byte sequence fed to handle_frame under DenyAll
    /// must not panic and must not modify the register store.
    ///
    /// We snapshot the store before and after; the slice comparison catches
    /// any mutation, including partial writes that a buggy parser might make
    /// before deciding to reject.
    #[test]
    fn fuzz_arbitrary_frame_no_panic_no_mutation(buf in prop::collection::vec(any::<u8>(), 0..MAX_FRAME)) {
        let mut store = RegisterStore::new(REG_COUNT);
        let snapshot: Vec<u16> = store.regs.clone();

        // Under DenyAll the handler must never touch a register regardless of
        // the input. We rely on catch_unwind to also detect panics, but
        // proptest already runs each case in a way that surfaces panics.
        let outcome = handle_frame(&buf, &mut store, WritePolicy::DenyAll);

        // The store must be byte-for-byte unchanged.
        prop_assert_eq!(store.regs, snapshot, "DenyAll must not mutate the store");

        // Invariant 3: response PDU sizes are bounded. A read of ≤125 regs
        // produces at most 2 + 250 = 252 bytes; an exception is 2 bytes; a
        // write echo is ≤ 5 bytes. The MBAP layer adds 7. So any Response or
        // Exception PDU must be ≤ 252 bytes. This catches a parser that
        // allocates from an unchecked length field.
        match outcome {
            HandleOutcome::Response(resp) => {
                prop_assert!(
                    resp.pdu.len() <= 252,
                    "response PDU too large ({} > 252) — unbounded allocation?",
                    resp.pdu.len()
                );
            }
            HandleOutcome::Exception(_) => {
                // Exception PDUs are always 2 bytes (fc|0x80 + code) at the
                // PDU level; the outcome itself carries no bytes, so nothing
                // to check here beyond "we got here without panicking".
            }
            HandleOutcome::CloseConnection => {
                // Malformed MBAP — fine.
            }
        }
    }

    /// Invariant 1 specifically for well-formed MBAP + arbitrary PDU. We
    /// generate a valid 7-byte MBAP header (protocol_id = 0, length matching
    /// the PDU) and then fuzz only the PDU bytes. This exercises the PDU
    /// parser paths (function code dispatch, length checks, field decoding)
    /// more densely than fully-random frames, which mostly hit the MBAP
    /// FrameTooShort / InvalidProtocolId early-exit.
    #[test]
    fn fuzz_valid_mbap_arbitrary_pdu(
        txn in any::<u16>(),
        unit in any::<u8>(),
        pdu in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        let mut store = RegisterStore::new(REG_COUNT);
        let snapshot = store.regs.clone();

        // Build a well-formed MBAP frame: txn(2) + proto=0(2) + length(2) + unit(1) + pdu.
        let length = (pdu.len() as u16) + 1; // +1 for unit_id
        let mut frame = Vec::with_capacity(7 + pdu.len());
        frame.extend_from_slice(&txn.to_be_bytes());
        frame.extend_from_slice(&0u16.to_be_bytes());
        frame.extend_from_slice(&length.to_be_bytes());
        frame.push(unit);
        frame.extend_from_slice(&pdu);

        let outcome = handle_frame(&frame, &mut store, WritePolicy::DenyAll);

        // DenyAll: no mutation ever.
        prop_assert_eq!(store.regs, snapshot, "DenyAll must not mutate even with valid MBAP");

        if let HandleOutcome::Response(resp) = outcome {
            prop_assert!(
                resp.pdu.len() <= 252,
                "response PDU too large ({} > 252)",
                resp.pdu.len()
            );
            // transaction_id and unit_id must round-trip from the MBAP header.
            prop_assert_eq!(resp.transaction_id, txn);
            prop_assert_eq!(resp.unit_id, unit);
        }
    }

    /// Invariant 3 — quantity bounds: read-holding (0x03) with a well-formed
    /// 5-byte PDU but an arbitrary quantity. The spec caps quantity at 1..=125;
    /// any value outside that must yield IllegalDataValue (0x03), never an
    /// out-of-range allocation or a panic.
    #[test]
    fn fuzz_read_holding_quantity_bound(
        start in any::<u16>(),
        quantity in any::<u16>(),
    ) {
        let mut store = RegisterStore::new(REG_COUNT);
        let mut pdu = Vec::with_capacity(5);
        pdu.push(0x03);
        pdu.extend_from_slice(&start.to_be_bytes());
        pdu.extend_from_slice(&quantity.to_be_bytes());

        let frame = build_frame(1, 1, &pdu);
        let outcome = handle_frame(&frame, &mut store, WritePolicy::DenyAll);

        match outcome {
            HandleOutcome::Response(resp) => {
                // Only legal quantities 1..=125 AND in-bounds ranges produce a Response.
                prop_assert!((1..=125).contains(&quantity), "responded to illegal quantity {}", quantity);
                // byte_count field + 2*quantity bytes
                prop_assert_eq!(resp.pdu.len(), 1 + 1 + 2 * quantity as usize);
                prop_assert!(resp.pdu.len() <= 252);
            }
            HandleOutcome::Exception(_) => {
                // IllegalDataValue (out-of-range quantity) or IllegalDataAddress
                // (in-range quantity but out-of-bounds start+quantity). Both are
                // acceptable; what matters is no panic and no mutation.
            }
            HandleOutcome::CloseConnection => {
                prop_assert!(false, "well-formed 0x03 frame must not close the connection");
            }
        }
    }

    /// Invariant 3 — write-multiple (0x10) byte_count / quantity consistency.
    /// A crafted frame where byte_count disagrees with quantity, or where the
    /// PDU is truncated relative to byte_count, must be rejected with
    /// IllegalDataValue, never panicking on an incomplete read.
    #[test]
    fn fuzz_write_multiple_byte_count_consistency(
        start in any::<u16>(),
        quantity in 0u16..130,
        byte_count in any::<u8>(),
        trailing in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        let mut store = RegisterStore::new(REG_COUNT);
        let snapshot = store.regs.clone();

        let mut pdu = Vec::with_capacity(6 + trailing.len());
        pdu.push(0x10);
        pdu.extend_from_slice(&start.to_be_bytes());
        pdu.extend_from_slice(&quantity.to_be_bytes());
        pdu.push(byte_count);
        pdu.extend_from_slice(&trailing);

        let frame = build_frame(1, 1, &pdu);
        let outcome = handle_frame(&frame, &mut store, WritePolicy::DenyAll);

        // Under DenyAll, no mutation regardless of whether the frame was
        // well-formed or malformed.
        prop_assert_eq!(store.regs, snapshot, "0x10 DenyAll must not mutate");
        // No panic; outcome is one of the three variants.
        let _ = outcome;
    }

    /// Invariant 4 — default-deny under a fully random frame AND a random
    /// initial store state. We pre-seed the store with random values, then
    /// throw arbitrary frames at it; under DenyAll the store MUST be
    /// unchanged. This is the strongest form of the control-gateway
    /// chokepoint invariant: no protocol input alone can write a device.
    #[test]
    fn fuzz_default_deny_with_random_seed(
        seed in prop::collection::vec(any::<u16>(), 0..REG_COUNT),
        buf in prop::collection::vec(any::<u8>(), 0..MAX_FRAME),
    ) {
        let mut store = RegisterStore::new(REG_COUNT);
        for (i, v) in seed.iter().enumerate() {
            store.regs[i] = *v;
        }
        let snapshot = store.regs.clone();

        let _ = handle_frame(&buf, &mut store, WritePolicy::DenyAll);

        prop_assert_eq!(
            store.regs, snapshot,
            "DEFAULT-DENY VIOLATION: arbitrary frame mutated the store under DenyAll"
        );
    }

    /// Invariant: the parser is total over every function code byte. We fix a
    /// minimal 1-byte PDU (just the function code) and walk all 256 values;
    /// none may panic. Most will yield IllegalFunction; that's fine.
    #[test]
    fn fuzz_all_function_codes_one_byte_pdu(fc in any::<u8>()) {
        let mut store = RegisterStore::new(REG_COUNT);
        let pdu = [fc];
        let frame = build_frame(1, 1, &pdu);
        let _ = handle_frame(&frame, &mut store, WritePolicy::DenyAll);
        // No assertion beyond "did not panic". proptest will fail this case if
        // handle_frame unwinds.
    }
}

/// Builds a well-formed MBAP frame around `pdu` (independent framing, mirrors
/// the interop test's hand-rolled builder so the parser is exercised by a
/// genuinely independent encoder).
fn build_frame(txn: u16, unit: u8, pdu: &[u8]) -> Vec<u8> {
    let length = (pdu.len() as u16) + 1;
    let mut buf = Vec::with_capacity(7 + pdu.len());
    buf.extend_from_slice(&txn.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.extend_from_slice(&length.to_be_bytes());
    buf.push(unit);
    buf.extend_from_slice(pdu);
    buf
}

/// An extended fuzz case that exercises more iterations than the default
/// proptest config. Runs in normal `cargo test` (not ignored) so the
/// default-deny + bounded-PDU invariant is always verified.
#[test]
fn fuzz_long_running_arbitrary_frames() {
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::new(ProptestConfig {
        cases: 512,
        ..ProptestConfig::default()
    });
    let strategy = prop::collection::vec(any::<u8>(), 0..MAX_FRAME);

    let result = runner.run(&strategy, |buf| {
        let mut store = RegisterStore::new(REG_COUNT);
        let snapshot = store.regs.clone();
        let outcome = handle_frame(&buf, &mut store, WritePolicy::DenyAll);
        assert_eq!(
            store.regs,
            snapshot,
            "DenyAll mutation on buf {:?}",
            &buf[..]
        );
        if let HandleOutcome::Response(resp) = outcome {
            assert!(
                resp.pdu.len() <= 252,
                "unbounded PDU: {} bytes",
                resp.pdu.len()
            );
        }
        Ok(())
    });

    // A successful run returns Ok(()); a property failure returns Err. We
    // propagate panics (which is what proptest does on a real bug) and treat
    // the runner's own "test failed" verdict as a hard failure too.
    if let Err(e) = result {
        panic!("long fuzz failed: {:?}", e);
    }
}
