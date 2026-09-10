//! 70.5（WP-70.5.6）：目标 TLS 材料嵌入面——build.rs 从 `product/tls/`
//! 读入（部署/KMS/gate 生成，不入仓），构造 [`TlsMaterial`]。
//!
//! 缺文件 = 空字节 = `None`——svc-mqtt release 装配机检 fail-closed
//! （能力壳），不降级明文。熵 = **部署期熵种子**（`tls-seed.bin` 32B，
//! 与证书同批换发——实板硬件熵在 70.12–14 复验替换，本卷如实登记）；
//! 时间 = X6 墙钟链派生（调用方传入 Unix 秒）。
//!
//! 本模块仅 `mqtt-tls` feature（目标装配）编译；host 环回验证走
//! `eneros-tls` 自身测试面。

#![cfg(feature = "mqtt-tls")]

use alloc::string::String;

use eneros_tls::embedded::SharedCsRng;

include!(concat!(env!("OUT_DIR"), "/tls_material.rs"));

/// 嵌入材料 → TLS 材料（全部字段在位才 Some；否则 None = 未部署）。
pub fn embedded_material(now_unix_sec: u64) -> Option<eneros_tls::target_factory::TlsMaterial> {
    if TLS_CA_DER.is_empty() || TLS_CLIENT_DER.is_empty() || TLS_CLIENT_KEY_DER.is_empty() {
        return None;
    }
    if TLS_SERVER_NAME.is_empty() {
        return None;
    }
    if TLS_SEED.len() != 32 || TLS_SEED.iter().all(|b| *b == 0) {
        return None;
    }
    let ca = eneros_crypto::pki::parser::parse_der(TLS_CA_DER).ok()?;
    let mut seed = [0u8; 32];
    seed.copy_from_slice(TLS_SEED);
    let rng = eneros_crypto::rng::CsRng::from_seed(&seed);
    let mut client_chain = alloc::vec::Vec::new();
    client_chain.push(alloc::vec::Vec::from(TLS_CLIENT_DER));
    Some(eneros_tls::target_factory::TlsMaterial {
        ca,
        client_chain,
        client_key: alloc::vec::Vec::from(TLS_CLIENT_KEY_DER),
        server_name: String::from(TLS_SERVER_NAME),
        now: now_unix_sec,
        rng: SharedCsRng(alloc::rc::Rc::new(core::cell::RefCell::new(rng))),
    })
}
