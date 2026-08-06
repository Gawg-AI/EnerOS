//! VRF-002：双构建字节比对门禁。
//!
//! 两个隔离构建环境（不同 `CARGO_TARGET_DIR`）构建同一未签名 payload，
//! 比对产物 sha256 —— 证明构建确定性（bit-identical）。
//! payload 选用纯 Rust 固件场景 binary `eneros-firmware-agent`（不依赖
//! sel4 构建链，sandbox 可复算）；CI 中同一步骤扩展到全 profile。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::DynResult;

fn sha256_file(p: &Path) -> String {
    let data = fs::read(p).expect("read payload");
    format!("{:x}", Sha256::digest(&data))
}

fn build_in(dir: &Path, target_dir: &str, bin: &str) -> DynResult<PathBuf> {
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "eneros-hello"])
        .env("CARGO_TARGET_DIR", target_dir)
        .current_dir(dir)
        .status()?;
    if !status.success() {
        return Err(format!("build failed in {target_dir}").into());
    }
    Ok(Path::new(target_dir).join("release").join(bin))
}

pub fn command_build_compare(root: &Path) -> DynResult<()> {
    let bin = "eneros-hello";
    let ta = root.join("target/compare-a");
    let tb = root.join("target/compare-b");
    let pa = build_in(root, ta.to_str().unwrap(), bin)?;
    let pb = build_in(root, tb.to_str().unwrap(), bin)?;
    let ha = sha256_file(&pa);
    let hb = sha256_file(&pb);
    let la = fs::metadata(&pa)?.len();
    let lb = fs::metadata(&pb)?.len();
    println!(
        "BUILD_COMPARE payload={bin} env_a=target/compare-a env_b=target/compare-b sha256_a={ha} sha256_b={hb} len_a={la} len_b={lb}"
    );
    if ha == hb && la == lb {
        println!("BUILD_COMPARE result=PASS bit-identical");
        Ok(())
    } else {
        Err(format!("BUILD_COMPARE result=FAIL: sha256 diverge ({ha} != {hb})").into())
    }
}
