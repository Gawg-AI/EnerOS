use eneros_core::{Result, EnerOSError};
use super::command::Command;

/// Trait for safety checks
pub trait SafetyCheck: Send + Sync {
    /// Validate a command
    fn validate(&self, command: &Command) -> Result<()>;

    /// Get check name
    fn name(&self) -> &str;

    /// Get check description
    fn description(&self) -> &str;
}

/// Voltage safety check - validates voltage setpoints remain within safe limits
pub struct VoltageSafetyCheck {
    min_voltage_pu: f64,
    max_voltage_pu: f64,
}

impl VoltageSafetyCheck {
    pub fn new(min_voltage_pu: f64, max_voltage_pu: f64) -> Self {
        Self {
            min_voltage_pu,
            max_voltage_pu,
        }
    }
}

impl SafetyCheck for VoltageSafetyCheck {
    fn validate(&self, command: &Command) -> Result<()> {
        if let Some(&voltage_setpoint) = command.parameters.get("voltage_setpoint") {
            if voltage_setpoint < self.min_voltage_pu || voltage_setpoint > self.max_voltage_pu {
                return Err(EnerOSError::Gateway(format!(
                    "Voltage setpoint {} p.u. out of safe range [{}, {}]",
                    voltage_setpoint, self.min_voltage_pu, self.max_voltage_pu
                )));
            }
        }

        if let Some(&target_voltage) = command.parameters.get("target_voltage") {
            if target_voltage < self.min_voltage_pu || target_voltage > self.max_voltage_pu {
                return Err(EnerOSError::Gateway(format!(
                    "Target voltage {} p.u. out of safe range [{}, {}]",
                    target_voltage, self.min_voltage_pu, self.max_voltage_pu
                )));
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "VoltageSafetyCheck"
    }

    fn description(&self) -> &str {
        "Validates voltage remains within safe limits"
    }
}

/// Thermal safety check - validates loading doesn't exceed thermal limits
pub struct ThermalSafetyCheck {
    max_loading_percent: f64,
}

impl ThermalSafetyCheck {
    pub fn new(max_loading_percent: f64) -> Self {
        Self {
            max_loading_percent,
        }
    }
}

impl SafetyCheck for ThermalSafetyCheck {
    fn validate(&self, command: &Command) -> Result<()> {
        if let Some(&loading_percent) = command.parameters.get("loading_percent") {
            if loading_percent > self.max_loading_percent {
                return Err(EnerOSError::Gateway(format!(
                    "Loading {}% exceeds thermal limit {}%",
                    loading_percent, self.max_loading_percent
                )));
            }
        }

        if let Some(&power_mw) = command.parameters.get("power_mw") {
            if let Some(&rated_mw) = command.parameters.get("rated_mw") {
                if rated_mw > 0.0 {
                    let loading = (power_mw / rated_mw) * 100.0;
                    if loading > self.max_loading_percent {
                        return Err(EnerOSError::Gateway(format!(
                            "Calculated loading {:.1}% exceeds thermal limit {}%",
                            loading, self.max_loading_percent
                        )));
                    }
                }
            }
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "ThermalSafetyCheck"
    }

    fn description(&self) -> &str {
        "Validates thermal loading remains within safe limits"
    }
}

/// Frequency safety check - validates frequency deviations
pub struct FrequencySafetyCheck {
    min_frequency_hz: f64,
    max_frequency_hz: f64,
}

impl FrequencySafetyCheck {
    pub fn new(min_frequency_hz: f64, max_frequency_hz: f64) -> Self {
        Self {
            min_frequency_hz,
            max_frequency_hz,
        }
    }
}

impl SafetyCheck for FrequencySafetyCheck {
    fn validate(&self, command: &Command) -> Result<()> {
        if let Some(&frequency) = command.parameters.get("frequency") {
            if frequency < self.min_frequency_hz || frequency > self.max_frequency_hz {
                return Err(EnerOSError::Gateway(format!(
                    "Frequency {} Hz out of safe range [{}, {}]",
                    frequency, self.min_frequency_hz, self.max_frequency_hz
                )));
            }
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "FrequencySafetyCheck"
    }

    fn description(&self) -> &str {
        "Validates frequency remains within safe limits"
    }
}
