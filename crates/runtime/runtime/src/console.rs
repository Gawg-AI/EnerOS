//! Console output with `print!`/`println!` macros.
//!
//! Implements `core::fmt::Write` on a `ConsoleWriter` that routes
//! formatted output through the `SeL4Serial` driver.

use core::fmt;

use crate::serial::{SeL4Serial, SerialOut};

static CONSOLE: SeL4Serial = SeL4Serial::new();

/// Writer that implements `core::fmt::Write` by forwarding to the serial console.
struct ConsoleWriter;

impl fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        CONSOLE.puts(s);
        Ok(())
    }
}

/// Initialize the user-space runtime console.
///
/// Currently a no-op: seL4 handles serial port initialization during boot.
/// This function exists for future expansion (e.g., setting up buffered I/O).
pub fn init() {
    // No-op for v0.4.0
}

/// Internal print function used by the `print!`/`println!` macros.
///
/// Public so that `#[macro_export]` macros can reference it via `$crate::console::_print`,
/// but the leading underscore signals "internal use only".
pub fn _print(args: fmt::Arguments) {
    let _ = fmt::write(&mut ConsoleWriter, args);
}

/// Prints formatted text to the seL4 serial console.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::_print(core::format_args!($($arg)*));
    };
}

/// Prints formatted text with a trailing newline.
#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::print!($($arg)*);
        $crate::print!("\n");
    };
}

#[cfg(test)]
mod tests {
    use core::fmt::Write;

    use super::*;

    #[test]
    fn test_init_does_not_panic() {
        init();
    }

    #[test]
    fn test_console_writer_write_str() {
        let mut writer = ConsoleWriter;
        assert!(writer.write_str("hello").is_ok());
        assert!(writer.write_str("with\nnewline").is_ok());
    }

    #[test]
    fn test_console_writer_write_fmt() {
        let mut writer = ConsoleWriter;
        let n = 42;
        let flag = true;
        let _ = write!(writer, "format: {} {}", n, flag);
    }

    #[test]
    fn test_print_macro() {
        // Just verify it doesn't panic
        print!("test print: {}", 42);
    }

    #[test]
    fn test_println_macro() {
        println!("test println: {}", "hello");
    }

    #[test]
    fn test_println_empty() {
        println!();
    }
}
