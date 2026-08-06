use std::env;
use std::fs;
use std::path::PathBuf;

fn decode_hex<const N: usize>(name: &str) -> [u8; N] {
    let value = env::var(name).unwrap_or_else(|_| {
        panic!("{name} is required for the production installer; there is no built-in fallback")
    });
    assert_eq!(value.len(), N * 2, "{name} has the wrong length");
    let mut bytes = [0u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).expect("trust input must be ASCII");
        bytes[index] =
            u8::from_str_radix(text, 16).unwrap_or_else(|_| panic!("{name} is not hexadecimal"));
    }
    bytes
}

fn literal(bytes: &[u8]) -> String {
    format!(
        "[{}]",
        bytes
            .iter()
            .map(|byte| format!("0x{byte:02x}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for name in [
        "ENEROS_RELEASE_PUBLIC_KEY_HEX",
        "ENEROS_RELEASE_KEY_ID_HEX",
        "ENEROS_PLATFORM_ID_HEX",
        "ENEROS_MINIMUM_SECURITY_VERSION",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("uefi") {
        return;
    }
    let public_key = decode_hex::<32>("ENEROS_RELEASE_PUBLIC_KEY_HEX");
    let key_id = decode_hex::<16>("ENEROS_RELEASE_KEY_ID_HEX");
    let platform_id = decode_hex::<16>("ENEROS_PLATFORM_ID_HEX");
    let minimum = env::var("ENEROS_MINIMUM_SECURITY_VERSION")
        .expect("ENEROS_MINIMUM_SECURITY_VERSION is required")
        .parse::<u64>()
        .expect("ENEROS_MINIMUM_SECURITY_VERSION must be an integer");
    assert!(minimum != 0, "minimum security version cannot be zero");
    let source = format!(
        "pub const TRUSTED_PUBLIC_KEY: [u8; 32] = {};\n\
         pub const TRUSTED_KEY_ID: [u8; 16] = {};\n\
         pub const PLATFORM_ID: [u8; 16] = {};\n\
         pub const MINIMUM_SECURITY_VERSION: u64 = {minimum};\n",
        literal(&public_key),
        literal(&key_id),
        literal(&platform_id),
    );
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set")).join("installer_trust.rs");
    fs::write(output, source).expect("write installer trust anchor");
}
