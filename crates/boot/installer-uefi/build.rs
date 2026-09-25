use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;

// B75-1 附带发现（预存，74-03 c64c3747 遗留）：build script 曾用
// panic!/expect/assert——仓规 disallowed_macros 禁非测试代码直接 panic，
// host clippy --workspace -D warnings 在案即红。改为 Result 传播
//（cargo 对 build script 非零退出同样失败并把消息上屏，可诊断性不变）。
type BuildResult = Result<(), Box<dyn Error>>;

fn decode_hex<const N: usize>(name: &str) -> Result<[u8; N], String> {
    let value = env::var(name).map_err(|_| {
        format!("{name} is required for the production installer; there is no built-in fallback")
    })?;
    if value.len() != N * 2 {
        return Err(format!("{name} has the wrong length"));
    }
    let mut bytes = [0u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text =
            std::str::from_utf8(pair).map_err(|_| format!("{name} trust input must be ASCII"))?;
        bytes[index] =
            u8::from_str_radix(text, 16).map_err(|_| format!("{name} is not hexadecimal"))?;
    }
    Ok(bytes)
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

fn main() -> BuildResult {
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
        return Ok(());
    }
    let public_key = decode_hex::<32>("ENEROS_RELEASE_PUBLIC_KEY_HEX")?;
    let key_id = decode_hex::<16>("ENEROS_RELEASE_KEY_ID_HEX")?;
    let platform_id = decode_hex::<16>("ENEROS_PLATFORM_ID_HEX")?;
    let minimum = env::var("ENEROS_MINIMUM_SECURITY_VERSION")
        .map_err(|_| "ENEROS_MINIMUM_SECURITY_VERSION is required")?
        .parse::<u64>()
        .map_err(|_| "ENEROS_MINIMUM_SECURITY_VERSION must be an integer")?;
    if minimum == 0 {
        return Err("minimum security version cannot be zero".into());
    }
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
        PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is set")?).join("installer_trust.rs");
    fs::write(output, source).map_err(|e| format!("write installer trust anchor: {e}"))?;
    Ok(())
}
