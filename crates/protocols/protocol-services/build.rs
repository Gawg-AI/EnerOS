//! 协议服务 ELF 链接配置：强制 4 KiB 页对齐（spawn 前提）。
//!
//! 与 eneros-drv-svc / eneros-services 同因：root-task 的
//! `configure_service` 按页映射 ELF 段（`seg.vaddr % PAGE_SIZE == 0`），
//! rust-lld 默认 64 KiB max-page-size 导致段 vaddr 非页对齐——39 收尾
//! 复验实测：7 个协议服务因此静默死于首指令（readiness 永不报告，
//! 全部超时降级）。与 drv-svc/build.rs 完全同参修复。
fn main() {
    if std::env::var("TARGET")
        .map(|t| t == "aarch64-unknown-none")
        .unwrap_or(false)
    {
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=max-page-size=0x1000");
        // separate-code：让代码/数据段分别页对齐（vaddr % 4096 == 0）。
        // 实测仅 max-page-size 不足以对齐段 vaddr（.text 起始偏移仍
        // 非页对齐）；separate-code 强制每个 PT_LOAD 页对齐。
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=separate-code");
    }

    // 70.5（WP-70.5.6）：目标 TLS 材料构建期嵌入（既有外部提供机制——
    // config-bundle build.rs 嵌入同型）。文件由部署/KMS/gate 生成到
    // `product/tls/`（不入仓——product/ 已 gitignore）；任一缺失 = 空
    // 材料 = svc-mqtt release 装配机检 fail-closed（不降级明文）。
    // mqtt-ca.der：broker CA 证书（DER）；mqtt-client.der：设备证书
    // （DER）；mqtt-client-key.der：设备 SEC1 私钥（DER，非机密性等同
    // config-bundle 签名钥注入链——按密钥注入纪律不入 git/证据）；
    // mqtt-server-name：期望 broker 名（UTF-8 文本）。
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("../../..");
    let tls_dir = root.join("product/tls");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("tls_material.rs");
    let mut body = String::from("/// TLS 材料嵌入面（构建期生成；70.5 WP-70.5.6）\n");
    for (name, const_name) in [
        ("mqtt-ca.der", "TLS_CA_DER"),
        ("mqtt-client.der", "TLS_CLIENT_DER"),
        ("mqtt-client-key.der", "TLS_CLIENT_KEY_DER"),
    ] {
        let path = tls_dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = std::fs::read(&path).unwrap_or_default();
        body.push_str(&format!("pub const {const_name}: &[u8] = &{bytes:?};\n"));
    }
    let name_path = tls_dir.join("mqtt-server-name");
    println!("cargo:rerun-if-changed={}", name_path.display());
    let name = std::fs::read_to_string(&name_path).unwrap_or_default();
    let name = name.trim();
    body.push_str(&format!("pub const TLS_SERVER_NAME: &str = {name:?};\n"));
    // 部署期熵种子（32B/次换发；实板 70.12–14 换硬件熵——登记项）。
    let seed_path = tls_dir.join("tls-seed.bin");
    println!("cargo:rerun-if-changed={}", seed_path.display());
    let seed = std::fs::read(&seed_path).unwrap_or_default();
    body.push_str(&format!("pub const TLS_SEED: &[u8] = &{seed:?};\n"));
    std::fs::write(&out, body).expect("write tls_material.rs");
}
