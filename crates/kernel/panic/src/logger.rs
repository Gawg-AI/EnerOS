//! Panic logger — formats `PanicContext` to a serial sink without allocation.
//!
//! The logger uses a fixed 256-byte stack buffer and `core::fmt::Write` so it
//! works in `no_std` without a heap.

use core::fmt::{self, Write};

use spin::Mutex;

use crate::{PanicContext, PanicLevel};

/// Abstraction over a byte-oriented serial output.
pub trait SerialSink {
    /// Emit a single byte.
    fn putc(&self, c: u8);
    /// Emit a string slice.
    fn puts(&self, s: &str);
}

/// No-op sink used before a real sink is registered.
pub struct NullSink;

impl SerialSink for NullSink {
    fn putc(&self, _c: u8) {}
    fn puts(&self, _s: &str) {}
}

/// Test-only sink that captures output into a fixed buffer for assertions.
pub struct CaptureSink {
    pub captured: Mutex<heapless::Vec<u8, 512>>,
}

impl SerialSink for CaptureSink {
    fn putc(&self, c: u8) {
        let _ = self.captured.lock().push(c);
    }
    fn puts(&self, s: &str) {
        let mut cap = self.captured.lock();
        for b in s.as_bytes() {
            let _ = cap.push(*b);
        }
    }
}

/// Globally registered serial sink (defaults to `None` → no-op).
static SERIAL_SINK: Mutex<Option<&'static (dyn SerialSink + Sync)>> = Mutex::new(None);

/// Register the serial sink used for panic output.
pub fn set_serial_sink(sink: &'static (dyn SerialSink + Sync)) {
    *SERIAL_SINK.lock() = Some(sink);
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Stack-backed `core::fmt::Write` adapter.
struct StackBufWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> StackBufWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn as_str(&self) -> &str {
        // All written bytes originate from `write_str`, which only accepts
        // valid UTF-8 slices, so the buffer contents are valid UTF-8.
        core::str::from_utf8(&self.buf[..self.pos]).unwrap_or("")
    }
}

impl<'a> fmt::Write for StackBufWriter<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.pos + bytes.len() > self.buf.len() {
            // Truncate to fit rather than erroring, so we always emit a newline.
            let remaining = self.buf.len() - self.pos;
            self.buf[self.pos..self.pos + remaining].copy_from_slice(&bytes[..remaining]);
            self.pos = self.buf.len();
        } else {
            self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
            self.pos += bytes.len();
        }
        Ok(())
    }
}

/// Format `ctx` and emit it through the registered serial sink.
///
/// Format:
/// `[PANIC] level=KERNEL loc=<loc> msg=<msg> core=<id> t=<ns>ns\n`
/// (or `level=Partition(n)` for partition panics)
pub fn panic_log(ctx: &PanicContext) {
    let mut buf = [0u8; 256];
    let mut w = StackBufWriter::new(&mut buf);
    let _ = w.write_str("[PANIC] ");
    match ctx.level {
        PanicLevel::Kernel => {
            let _ = w.write_str("level=KERNEL ");
        }
        PanicLevel::Partition(n) => {
            let _ = write!(w, "level=Partition({}) ", n);
        }
    }
    let _ = writeln!(
        w,
        "loc={} msg={} core={} t={}ns",
        ctx.location, ctx.message, ctx.core_id, ctx.timestamp_ns
    );
    let s = w.as_str();
    if let Some(sink) = *SERIAL_SINK.lock() {
        sink.puts(s);
    }
}

/// Emit a raw string through the registered serial sink.
///
/// (Blueprint interface `pub fn panic_log(msg: &str)`.)
pub fn panic_log_raw(msg: &str) {
    if let Some(sink) = *SERIAL_SINK.lock() {
        sink.puts(msg);
    }
}

/// Flush pending log output. Stub for now; reserved for blueprint §4.3.
pub fn flush() {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{PanicContext, PanicLevel};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Shared capture sink — `set_serial_sink` requires `&'static`, so tests
    /// use this single static instance (serialized via `TEST_LOCK`).
    static TEST_SINK: CaptureSink = CaptureSink {
        captured: spin::Mutex::new(heapless::Vec::new()),
    };

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset_sink() {
        TEST_SINK.captured.lock().clear();
    }

    fn captured() -> String {
        let cap = TEST_SINK.captured.lock();
        String::from_utf8(cap.as_slice().to_vec()).unwrap_or_default()
    }

    #[test]
    fn test_null_sink_no_panic() {
        let sink = NullSink;
        sink.putc(b'x');
        sink.puts("hello");
        // No assertions needed — just ensure it does not panic.
    }

    #[test]
    fn test_panic_log_format_kernel() {
        let _g = lock();
        reset_sink();
        set_serial_sink(&TEST_SINK);
        let ctx = PanicContext::new(PanicLevel::Kernel, "test.rs", "boom");
        super::panic_log(&ctx);
        let s = captured();
        assert!(s.contains("[PANIC]"), "missing [PANIC] prefix: {s}");
        assert!(s.contains("level=KERNEL"), "missing KERNEL level: {s}");
        assert!(s.contains("loc=test.rs"), "missing location: {s}");
        assert!(s.contains("msg=boom"), "missing message: {s}");
        assert!(s.contains("core="), "missing core field: {s}");
        assert!(s.contains("t="), "missing timestamp field: {s}");
        assert!(s.contains("ns\n"), "missing ns suffix: {s}");
        *SERIAL_SINK.lock() = None;
    }

    #[test]
    fn test_panic_log_format_partition() {
        let _g = lock();
        reset_sink();
        set_serial_sink(&TEST_SINK);
        let ctx = PanicContext::new(PanicLevel::Partition(3), "drv.rs", "fault");
        super::panic_log(&ctx);
        let s = captured();
        assert!(
            s.contains("level=Partition(3)"),
            "missing Partition level: {s}"
        );
        assert!(s.contains("loc=drv.rs"), "missing location: {s}");
        assert!(s.contains("msg=fault"), "missing message: {s}");
        *SERIAL_SINK.lock() = None;
    }

    #[test]
    fn test_panic_log_raw() {
        let _g = lock();
        reset_sink();
        set_serial_sink(&TEST_SINK);
        panic_log_raw("raw line\n");
        let s = captured();
        assert_eq!(s, "raw line\n");
        *SERIAL_SINK.lock() = None;
    }

    #[test]
    fn test_unregistered_sink_noop() {
        let _g = lock();
        *SERIAL_SINK.lock() = None;
        let ctx = PanicContext::new(PanicLevel::Kernel, "x", "y");
        // Should not panic when no sink is registered.
        super::panic_log(&ctx);
        panic_log_raw("z");
    }

    #[test]
    fn test_set_serial_sink() {
        let _g = lock();
        reset_sink();
        set_serial_sink(&TEST_SINK);
        assert!(SERIAL_SINK.lock().is_some());
        panic_log_raw("check");
        let s = captured();
        assert_eq!(s, "check");
        *SERIAL_SINK.lock() = None;
    }
}
