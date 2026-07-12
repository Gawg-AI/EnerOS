//! Serial output abstraction for seL4 user-space.
//!
//! Provides a `SerialOut` trait and `SeL4Serial` implementation that
//! routes output through `eneros-sel4-sys` syscall bindings.

use eneros_sel4_sys::seL4_put_char;

/// Trait for serial output devices.
pub trait SerialOut {
    /// Write a single byte.
    fn putc(&self, c: u8);
    /// Write a string, converting `\n` to `\r\n`.
    fn puts(&self, s: &str);
}

/// seL4 serial output via debug putchar syscall.
pub struct SeL4Serial;

impl SeL4Serial {
    /// Create a new SeL4Serial instance.
    pub const fn new() -> Self {
        Self
    }
}

impl Default for SeL4Serial {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialOut for SeL4Serial {
    fn putc(&self, c: u8) {
        let _ = seL4_put_char(c);
    }

    fn puts(&self, s: &str) {
        for &b in s.as_bytes() {
            if b == b'\n' {
                self.putc(b'\r');
            }
            self.putc(b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sel4_serial_construction() {
        let _serial = SeL4Serial::new();
    }

    #[test]
    fn test_putc_does_not_panic() {
        let serial = SeL4Serial::new();
        serial.putc(b'H');
        serial.putc(b'\n');
    }

    #[test]
    fn test_puts_without_newline() {
        let serial = SeL4Serial::new();
        serial.puts("hello world");
    }

    #[test]
    fn test_puts_with_newline() {
        let serial = SeL4Serial::new();
        serial.puts("line1\nline2\n");
    }

    #[test]
    fn test_puts_empty_string() {
        let serial = SeL4Serial::new();
        serial.puts("");
    }
}
