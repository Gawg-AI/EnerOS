//! HAL public types shared across all trait definitions.

/// Memory mapping flags for [`crate::HalMem::map`].
#[derive(Clone, Copy, Debug)]
pub struct MemFlags {
    /// Read access permitted.
    pub readable: bool,
    /// Write access permitted.
    pub writable: bool,
    /// Execute access permitted.
    pub executable: bool,
    /// Device memory (ARM Device-nGnRE). Implies non-cacheable.
    pub device: bool,
    /// Cacheable (Normal memory, write-back).
    pub cacheable: bool,
}

impl MemFlags {
    /// Device memory flags: readable + writable, non-cacheable, non-executable.
    pub const fn device() -> Self {
        Self {
            readable: true,
            writable: true,
            executable: false,
            device: true,
            cacheable: false,
        }
    }

    /// Normal read/write memory flags: readable + writable + cacheable.
    pub const fn normal() -> Self {
        Self {
            readable: true,
            writable: true,
            executable: false,
            device: false,
            cacheable: true,
        }
    }

    /// Read-only code memory: readable + executable + cacheable.
    pub const fn code() -> Self {
        Self {
            readable: true,
            writable: false,
            executable: true,
            device: false,
            cacheable: true,
        }
    }
}

/// Interrupt trigger type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqTrigger {
    /// Edge-triggered.
    Edge,
    /// Level-triggered.
    Level,
}

/// HAL error codes.
#[derive(Debug)]
pub enum HalError {
    /// Invalid parameter supplied.
    InvalidParam,
    /// Out of hardware resources (e.g. IRQ table full).
    OutOfResource,
    /// Operation not supported by this HAL implementation.
    NotSupported,
    /// Hardware fault detected.
    HardwareFault,
    /// Caller lacks required privilege/capability.
    PermissionDenied,
}

impl core::fmt::Display for HalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HalError::InvalidParam => write!(f, "invalid parameter"),
            HalError::OutOfResource => write!(f, "out of resource"),
            HalError::NotSupported => write!(f, "not supported"),
            HalError::HardwareFault => write!(f, "hardware fault"),
            HalError::PermissionDenied => write!(f, "permission denied"),
        }
    }
}

/// GPIO direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpioDir {
    /// Input pin.
    Input,
    /// Output pin.
    Output,
}

/// GPIO pull resistor mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullMode {
    /// No pull resistor.
    None,
    /// Pull-up.
    Up,
    /// Pull-down.
    Down,
}

/// GPIO pin configuration.
#[derive(Clone, Copy)]
pub struct GpioConfig {
    /// Pin number.
    pub pin: u32,
    /// Direction.
    pub dir: GpioDir,
    /// Pull resistor mode.
    pub pull: PullMode,
}

/// Result of an interrupt handler invocation.
#[derive(Debug, PartialEq, Eq)]
pub enum IrqAction {
    /// Interrupt was handled.
    Handled,
    /// Wake a waiting thread.
    WakeThread,
    /// Disable this interrupt.
    Disabled,
}

/// Interrupt handler function signature.
pub type IrqHandler = fn(irq: u32) -> IrqAction;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_flags_device() {
        let f = MemFlags::device();
        assert!(f.readable && f.writable);
        assert!(!f.executable && f.device && !f.cacheable);
    }

    #[test]
    fn mem_flags_normal() {
        let f = MemFlags::normal();
        assert!(f.readable && f.writable && f.cacheable);
        assert!(!f.executable && !f.device);
    }

    #[test]
    fn mem_flags_code() {
        let f = MemFlags::code();
        assert!(f.readable && f.executable && f.cacheable);
        assert!(!f.writable && !f.device);
    }

    #[test]
    fn mem_flags_custom() {
        let f = MemFlags {
            readable: true,
            writable: false,
            executable: false,
            device: false,
            cacheable: true,
        };
        assert!(f.readable && f.cacheable);
        assert!(!f.writable && !f.executable && !f.device);
    }

    #[test]
    fn irq_trigger_variants() {
        assert_eq!(IrqTrigger::Edge, IrqTrigger::Edge);
        assert_eq!(IrqTrigger::Level, IrqTrigger::Level);
        assert_ne!(IrqTrigger::Edge, IrqTrigger::Level);
    }

    #[test]
    fn hal_error_variants() {
        assert!(matches!(HalError::InvalidParam, HalError::InvalidParam));
        assert!(matches!(HalError::OutOfResource, HalError::OutOfResource));
        assert!(matches!(HalError::NotSupported, HalError::NotSupported));
        assert!(matches!(HalError::HardwareFault, HalError::HardwareFault));
        assert!(matches!(
            HalError::PermissionDenied,
            HalError::PermissionDenied
        ));
    }

    #[test]
    fn hal_error_display() {
        assert_eq!(format!("{}", HalError::InvalidParam), "invalid parameter");
        assert_eq!(format!("{}", HalError::NotSupported), "not supported");
    }

    #[test]
    fn gpio_dir_variants() {
        assert_eq!(GpioDir::Input, GpioDir::Input);
        assert_ne!(GpioDir::Input, GpioDir::Output);
    }

    #[test]
    fn pull_mode_variants() {
        assert_eq!(PullMode::None, PullMode::None);
        assert_ne!(PullMode::Up, PullMode::Down);
    }

    #[test]
    fn gpio_config_construction() {
        let cfg = GpioConfig {
            pin: 42,
            dir: GpioDir::Output,
            pull: PullMode::Up,
        };
        assert_eq!(cfg.pin, 42);
        assert_eq!(cfg.dir, GpioDir::Output);
        assert_eq!(cfg.pull, PullMode::Up);
    }

    #[test]
    fn irq_action_variants() {
        assert_eq!(IrqAction::Handled, IrqAction::Handled);
        assert_ne!(IrqAction::Handled, IrqAction::WakeThread);
        assert_ne!(IrqAction::WakeThread, IrqAction::Disabled);
    }
}
