// Build scripts legitimately `panic!` on environment misconfiguration
// to fail the build fast with a clear message.
#![allow(clippy::disallowed_macros)]

use std::env;
use std::fs;
use std::path::PathBuf;

fn decode_hex<const N: usize>(name: &str) -> [u8; N] {
    let value = env::var(name).unwrap_or_else(|_| {
        panic!(
            "{name} is required for the production UEFI target; there is no built-in key fallback"
        )
    });
    assert_eq!(
        value.len(),
        N * 2,
        "{name} must contain exactly {} hexadecimal characters",
        N * 2
    );
    let mut bytes = [0u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).expect("hex input must be ASCII");
        bytes[index] = u8::from_str_radix(text, 16).unwrap_or_else(|_| panic!("{name} is not hex"));
    }
    bytes
}

fn array_literal(bytes: &[u8]) -> String {
    let body = bytes
        .iter()
        .map(|byte| format!("0x{byte:02x}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{body}]")
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for name in [
        "ENEROS_RELEASE_PUBLIC_KEY_HEX",
        "ENEROS_RELEASE_KEY_ID_HEX",
        "ENEROS_PLATFORM_ID_HEX",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    // BOOT-001：EXPECTED_DISK_GUID 构建期注入（host 与 uefi 均生成）——
    // 从 `ENEROS_DISK_GUID_HEX`（32 个 hex 字符 = 16 字节 GUID）读取；
    // 未设置时回退默认开发 GUID 并打印 warning（生产构建必须显式注入，
    // 避免与镜像工厂 GUID 漂移）。
    println!("cargo:rerun-if-env-changed=ENEROS_DISK_GUID_HEX");
    let disk_guid = match env::var("ENEROS_DISK_GUID_HEX") {
        Ok(_v) => decode_hex::<16>("ENEROS_DISK_GUID_HEX"),
        Err(_) => {
            eprintln!(
                "warning: ENEROS_DISK_GUID_HEX not set; using default dev disk GUID                  (BOOT-001: production builds must inject the disk GUID at build time)"
            );
            [
                0x45, 0x4e, 0x45, 0x52, 0x4f, 0x53, 0x40, 0x01, //
                0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            ]
        }
    };
    let guid_source = format!(
        "pub const EXPECTED_DISK_GUID_BYTES: [u8; 16] = {};\n",
        array_literal(&disk_guid)
    );
    let guid_output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is provided by Cargo"))
        .join("expected_disk_guid.rs");
    fs::write(guid_output, guid_source).expect("write generated disk guid");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("uefi") {
        // Host/dev 构建：生成占位信任锚，使 bin 可编译（生产 uefi 构建
        // 走下方强制环境变量路径）。
        let placeholder = "pub const TRUSTED_PUBLIC_KEY: [u8; 32] = [0u8; 32];\n\
            pub const TRUSTED_KEY_ID: [u8; 16] = [0u8; 16];\n\
            pub const PLATFORM_ID: [u8; 16] = [0u8; 16];\n";
        let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is provided by Cargo"))
            .join("trust_anchor.rs");
        fs::write(output, placeholder).expect("write placeholder trust anchor");
        return;
    }

    let public_key = decode_hex::<32>("ENEROS_RELEASE_PUBLIC_KEY_HEX");
    let key_id = decode_hex::<16>("ENEROS_RELEASE_KEY_ID_HEX");
    let platform_id = decode_hex::<16>("ENEROS_PLATFORM_ID_HEX");
    let source = format!(
        "pub const TRUSTED_PUBLIC_KEY: [u8; 32] = {};\n\
         pub const TRUSTED_KEY_ID: [u8; 16] = {};\n\
         pub const PLATFORM_ID: [u8; 16] = {};\n",
        array_literal(&public_key),
        array_literal(&key_id),
        array_literal(&platform_id)
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is provided by Cargo"))
        .join("trust_anchor.rs");
    fs::write(output, source).expect("write generated public trust anchor");

    // BOOT-001：EXPECTED_DISK_GUID 构建期注入——从 `ENEROS_DISK_GUID_HEX`
    // （32 个 hex 字符 = 16 字节 GUID）读取；未设置时回退到默认开发 GUID
    // 并打印 warning（生产构建必须显式注入，避免与镜像工厂 GUID 漂移）。
    println!("cargo:rerun-if-env-changed=ENEROS_DISK_GUID_HEX");
    let disk_guid = match env::var("ENEROS_DISK_GUID_HEX") {
        Ok(_v) => decode_hex::<16>("ENEROS_DISK_GUID_HEX"),
        Err(_) => {
            eprintln!(
                "warning: ENEROS_DISK_GUID_HEX not set; using default dev disk GUID                  (BOOT-001: production builds must inject the disk GUID at build time)"
            );
            [0x45, 0x4e, 0x45, 0x52, 0x4f, 0x53, 0x40, 0x01, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01]
        }
    };
    let guid_source = format!(
        "pub const EXPECTED_DISK_GUID_BYTES: [u8; 16] = {};\n",
        array_literal(&disk_guid)
    );
    let guid_output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is provided by Cargo"))
        .join("expected_disk_guid.rs");
    fs::write(guid_output, guid_source).expect("write generated disk guid");
}
