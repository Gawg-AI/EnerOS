//! AT command encapsulation for cellular modem (v0.30.1).
//!
//! Provides AT command construction, encoding, and response parsing
//! for communicating with cellular modems via AT commands.
//!
//! # no_std Compliance
//! Uses `alloc::string::String` / `alloc::vec::Vec` — no `std`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::CellularError;

/// An AT command to send to the modem.
#[derive(Debug, Clone)]
pub struct AtCommand {
    /// Command string, e.g. `"AT+CSQ"` or `"AT+CGDCONT"`.
    pub cmd: String,
    /// Arguments to append after `=` (joined with `,`).
    pub args: Vec<String>,
    /// Timeout in milliseconds for this command.
    pub timeout_ms: u32,
}

/// Response from the modem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtResponse {
    /// `OK` response with optional extracted content (e.g. value from `+CMD: value`).
    Ok(String),
    /// `ERROR` response with optional detail.
    Error(String),
    /// No response received within timeout.
    Timeout,
}

/// AT command parser/encoder.
#[derive(Debug, Clone, Copy)]
pub struct AtParser;

/// Signal strength report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalStrength {
    /// Received Signal Strength Indicator (0-31, 99 = unknown).
    pub rssi: i8,
    /// Bit Error Rate (0-7, 99 = unknown).
    pub ber: u8,
    /// Network type (CSQ does not report this, so defaults to `Unknown`).
    pub network_type: NetworkType,
}

/// Network type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkType {
    /// Unknown or not reported.
    Unknown,
    /// GSM (2G).
    Gsm,
    /// WCDMA (3G).
    Wcdma,
    /// LTE (4G).
    Lte,
    /// 5G NR.
    Nr5g,
}

impl AtCommand {
    /// Create a new AT command without arguments.
    pub fn new(cmd: &str, timeout_ms: u32) -> Self {
        Self {
            cmd: String::from(cmd),
            args: Vec::new(),
            timeout_ms,
        }
    }

    /// Create a new AT command with arguments.
    pub fn with_args(cmd: &str, args: Vec<String>, timeout_ms: u32) -> Self {
        Self {
            cmd: String::from(cmd),
            args,
            timeout_ms,
        }
    }
}

impl AtParser {
    /// Encode an AT command into a wire-ready string.
    ///
    /// - No args: `AT+CMD\r\n`
    /// - With args: `AT+CMD=arg1,arg2\r\n`
    pub fn encode(cmd: &AtCommand) -> String {
        if cmd.args.is_empty() {
            let mut s = cmd.cmd.clone();
            s.push_str("\r\n");
            s
        } else {
            let mut s = cmd.cmd.clone();
            s.push('=');
            for (i, arg) in cmd.args.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push_str(arg);
            }
            s.push_str("\r\n");
            s
        }
    }

    /// Parse a raw modem response into an [`AtResponse`].
    ///
    /// - Empty/whitespace → [`AtResponse::Timeout`]
    /// - `+CMD: value` line → [`AtResponse::Ok(value)`] (takes priority)
    /// - Contains `ERROR` → [`AtResponse::Error`]
    /// - Contains `OK` → [`AtResponse::Ok`]
    pub fn parse_response(raw: &str) -> Result<AtResponse, CellularError> {
        let trimmed = raw.trim_matches(|c| c == '\r' || c == '\n' || c == ' ' || c == '\t');
        if trimmed.is_empty() {
            return Ok(AtResponse::Timeout);
        }
        // Priority 1: "+CMD: value" data lines
        for line in trimmed.lines() {
            let line = line.trim();
            if line.starts_with('+') {
                if let Some(colon_idx) = line.find(':') {
                    let value = line[colon_idx + 1..].trim();
                    return Ok(AtResponse::Ok(String::from(value)));
                }
            }
        }
        // Priority 2: ERROR
        if trimmed.contains("ERROR") {
            return Ok(AtResponse::Error(String::new()));
        }
        // Priority 3: OK
        if trimmed.contains("OK") {
            return Ok(AtResponse::Ok(String::new()));
        }
        // Unknown: return as-is
        Ok(AtResponse::Ok(String::from(trimmed)))
    }

    /// Parse a `+CSQ` signal strength response.
    ///
    /// Expected format: `+CSQ: <rssi>,<ber>`
    /// - rssi: 0-31 (99 = unknown)
    /// - ber: 0-7 (99 = unknown)
    ///
    /// `network_type` is always [`NetworkType::Unknown`] since CSQ does not
    /// report network type.
    pub fn parse_signal(raw: &str) -> Result<SignalStrength, CellularError> {
        let trimmed = raw.trim();
        let data = if let Some(idx) = trimmed.find("+CSQ:") {
            &trimmed[idx + 5..]
        } else {
            trimmed
        };
        let data = data.trim_matches(|c| c == ' ' || c == '\r' || c == '\n' || c == '\t');

        let mut parts = data.split(',');
        let rssi_str = parts.next().ok_or(CellularError::AtCommandTimeout)?.trim();
        let ber_str = parts.next().ok_or(CellularError::AtCommandTimeout)?.trim();

        let rssi: i8 = rssi_str
            .parse()
            .map_err(|_| CellularError::AtCommandTimeout)?;
        let ber: u8 = ber_str
            .parse()
            .map_err(|_| CellularError::AtCommandTimeout)?;

        Ok(SignalStrength {
            rssi,
            ber,
            network_type: NetworkType::Unknown,
        })
    }

    /// Parse a `+COPS` operator response.
    ///
    /// Expected format: `+COPS: <mode>,<format>,<operator>`
    /// Returns the operator name (quotes stripped if present).
    pub fn parse_operator(raw: &str) -> Result<String, CellularError> {
        let trimmed = raw.trim();
        let data = if let Some(idx) = trimmed.find("+COPS:") {
            &trimmed[idx + 6..]
        } else {
            trimmed
        };
        let data = data.trim_matches(|c| c == ' ' || c == '\r' || c == '\n' || c == '\t');

        let parts: Vec<&str> = data.split(',').collect();
        if parts.len() < 3 {
            return Err(CellularError::AtCommandTimeout);
        }

        let operator = parts[2].trim().trim_matches('"');
        if operator.is_empty() {
            return Err(CellularError::AtCommandTimeout);
        }

        Ok(String::from(operator))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- AtCommand construction ---

    #[test]
    fn at_command_new_basic() {
        let cmd = AtCommand::new("AT+CSQ", 1000);
        assert_eq!(cmd.cmd, "AT+CSQ");
        assert!(cmd.args.is_empty());
        assert_eq!(cmd.timeout_ms, 1000);
    }

    #[test]
    fn at_command_with_args_basic() {
        let args = alloc::vec![String::from("arg1"), String::from("arg2")];
        let cmd = AtCommand::with_args("AT+CMD", args, 500);
        assert_eq!(cmd.cmd, "AT+CMD");
        assert_eq!(cmd.args.len(), 2);
        assert_eq!(cmd.args[0], "arg1");
        assert_eq!(cmd.args[1], "arg2");
        assert_eq!(cmd.timeout_ms, 500);
    }

    // --- AtParser::encode ---

    #[test]
    fn encode_no_args() {
        let cmd = AtCommand::new("AT+CSQ", 1000);
        let encoded = AtParser::encode(&cmd);
        assert_eq!(encoded, "AT+CSQ\r\n");
    }

    #[test]
    fn encode_with_args() {
        let args = alloc::vec![String::from("arg1"), String::from("arg2")];
        let cmd = AtCommand::with_args("AT+CMD", args, 1000);
        let encoded = AtParser::encode(&cmd);
        assert_eq!(encoded, "AT+CMD=arg1,arg2\r\n");
    }

    #[test]
    fn encode_multiple_args() {
        let args = alloc::vec![
            String::from("val1"),
            String::from("val2"),
            String::from("val3"),
        ];
        let cmd = AtCommand::with_args("AT+TEST", args, 1000);
        let encoded = AtParser::encode(&cmd);
        assert_eq!(encoded, "AT+TEST=val1,val2,val3\r\n");
    }

    #[test]
    fn encode_single_arg() {
        let args = alloc::vec![String::from("only")];
        let cmd = AtCommand::with_args("AT+X", args, 100);
        assert_eq!(AtParser::encode(&cmd), "AT+X=only\r\n");
    }

    // --- AtParser::parse_response ---

    #[test]
    fn parse_response_ok() {
        let resp = AtParser::parse_response("OK\r\n").unwrap();
        assert_eq!(resp, AtResponse::Ok(String::new()));
    }

    #[test]
    fn parse_response_error() {
        let resp = AtParser::parse_response("ERROR\r\n").unwrap();
        assert_eq!(resp, AtResponse::Error(String::new()));
    }

    #[test]
    fn parse_response_cmd_value() {
        let resp = AtParser::parse_response("+CSQ: 23,0\r\n").unwrap();
        assert_eq!(resp, AtResponse::Ok(String::from("23,0")));
    }

    #[test]
    fn parse_response_empty() {
        let resp = AtParser::parse_response("").unwrap();
        assert_eq!(resp, AtResponse::Timeout);
    }

    #[test]
    fn parse_response_empty_whitespace() {
        let resp = AtParser::parse_response("\r\n  \t  \r\n").unwrap();
        assert_eq!(resp, AtResponse::Timeout);
    }

    #[test]
    fn parse_response_cmd_then_ok() {
        // Real-world: data line followed by OK
        let resp = AtParser::parse_response("+CSQ: 23,0\r\n\r\nOK\r\n").unwrap();
        assert_eq!(resp, AtResponse::Ok(String::from("23,0")));
    }

    // --- AtParser::parse_signal ---

    #[test]
    fn parse_signal_normal() {
        let sig = AtParser::parse_signal("+CSQ: 23,0\r\n").unwrap();
        assert_eq!(sig.rssi, 23);
        assert_eq!(sig.ber, 0);
        assert_eq!(sig.network_type, NetworkType::Unknown);
    }

    #[test]
    fn parse_signal_unknown_rssi() {
        let sig = AtParser::parse_signal("+CSQ: 99,99\r\n").unwrap();
        assert_eq!(sig.rssi, 99);
        assert_eq!(sig.ber, 99);
    }

    #[test]
    fn parse_signal_without_prefix() {
        let sig = AtParser::parse_signal("23,0").unwrap();
        assert_eq!(sig.rssi, 23);
        assert_eq!(sig.ber, 0);
    }

    #[test]
    fn parse_signal_malformed() {
        let result = AtParser::parse_signal("+CSQ: invalid\r\n");
        assert!(result.is_err());
    }

    #[test]
    fn parse_signal_missing_ber() {
        let result = AtParser::parse_signal("+CSQ: 23\r\n");
        assert!(result.is_err());
    }

    // --- AtParser::parse_operator ---

    #[test]
    fn parse_operator_normal() {
        let op = AtParser::parse_operator("+COPS: 0,0,CHINA MOBILE\r\n").unwrap();
        assert_eq!(op, "CHINA MOBILE");
    }

    #[test]
    fn parse_operator_with_quotes() {
        let op = AtParser::parse_operator("+COPS: 0,0,\"CHINA MOBILE\"\r\n").unwrap();
        assert_eq!(op, "CHINA MOBILE");
    }

    #[test]
    fn parse_operator_malformed() {
        let result = AtParser::parse_operator("+COPS: 0,0\r\n");
        assert!(result.is_err());
    }

    // --- NetworkType ---

    #[test]
    fn network_type_variants_distinct() {
        assert_ne!(NetworkType::Unknown, NetworkType::Gsm);
        assert_ne!(NetworkType::Gsm, NetworkType::Wcdma);
        assert_ne!(NetworkType::Lte, NetworkType::Nr5g);
    }

    #[test]
    fn network_type_eq_self() {
        assert_eq!(NetworkType::Unknown, NetworkType::Unknown);
        assert_eq!(NetworkType::Lte, NetworkType::Lte);
    }
}
