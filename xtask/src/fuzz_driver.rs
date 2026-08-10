//! VRF-001 / 29 GV-04：**fuzz-smoke-prng** —— stable 可运行的确定性 PRNG
//! 冒烟 driver（sandbox 无 clang/nightly rustc 时 libFuzzer 不可执行）。
//! 本 driver 以固定种子 xorshift64* 变异驱动各解析器 + catch_unwind 崩溃
//! 检测；**无覆盖率反馈，每次运行输入序列相同**——仅作冒烟，不作深 fuzz
//! 证据。真实 fuzz 由 CI 的 cargo-fuzz job 执行 `fuzz/` 目录 libFuzzer
//! harness（消费 fuzz/corpus/ 语料，见 .github/workflows/ci.yml 与
//! nightly-fuzz.yml）。
//!
//! 用法：`cargo xtask fuzz <target> <iterations>`
//! target ∈ {boot_protocol, ipc_wire, modbus_rtu, iec104, elf_loader, gguf_loader}

use std::time::Instant;

use crate::DynResult;

/// 确定性 PRNG（xorshift64*）。
struct Prng(u64);
impl Prng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn byte(&mut self) -> u8 {
        (self.next() & 0xff) as u8
    }
}

/// 变异语料：以预置种子为基底，随机翻转/截断/插入字节。
fn mutate(seed: &[u8], rng: &mut Prng, len: usize) -> Vec<u8> {
    let mut buf = seed.to_vec();
    for _ in 0..len {
        match rng.next() % 4 {
            0 => {
                if !buf.is_empty() {
                    let i = (rng.next() as usize) % buf.len();
                    buf[i] = rng.byte();
                }
            }
            1 => {
                if !buf.is_empty() {
                    let cut = (rng.next() as usize) % (buf.len() + 1);
                    buf.truncate(cut);
                }
            }
            2 => buf.push(rng.byte()),
            _ => {
                if buf.len() > 1 {
                    let i = (rng.next() as usize) % buf.len();
                    buf[i] = buf[i].wrapping_add(rng.byte());
                }
            }
        }
    }
    buf
}

/// 预置语料种子（合法输入基线）。
fn seed_for(target: &str) -> &'static [u8] {
    match target {
        "boot_protocol" => &[
            0x45, 0x4e, 0x45, 0x52, // magic "ENER"
            0x01, 0x00, // abi_major
            0x00, 0x00, // abi_minor
            0x58, 0x00, 0x00, 0x00, // struct_size = 88
            0x00, 0x00, 0x00, 0x00, // flags
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // boot_id
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // kernel_entry
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // dt_phys
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // dt_size
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // reserved
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        "ipc_wire" => &[
            0x45, 0x4c, 0x30, 0x46, // EL0F magic
            0x01, 0x00, 0x00, 0x00, // type = 1
            0x04, 0x00, 0x00, 0x00, // len = 4
            0xde, 0xad, 0xbe, 0xef, // payload
            0x00, 0x00, 0x00, 0x00, // crc32（driver 不校验 crc，解析器校验）
        ],
        "modbus_rtu" => &[0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0a],
        "iec104" => &[0x68, 0x04, 0x07, 0x00, 0x00, 0x00],
        "elf_loader" => &[
            0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00, // ELF64 LE
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, // e_type..e_flags
            0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // e_ehsize=64
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // e_phentsize
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // e_phnum
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // e_shentsize
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // e_shnum
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // e_shstrndx
        ],
        "gguf_loader" => &[
            0x47, 0x47, 0x55, 0x46, // "GGUF"
            0x03, 0x00, 0x00, 0x00, // version
            0x00, 0x00, 0x00, 0x00, // tensor_count
            0x01, 0x00, 0x00, 0x00, // kv_count = 1
            0x02, 0x00, 0x00, 0x00, 0x00, 0x00, // key len=2 "" (空字符串容错)
        ],
        _ => &[],
    }
}

fn exercise(target: &str, data: &[u8]) {
    match target {
        "boot_protocol" => {
            let _ = eneros_boot_protocol::boot_info::BootInfo::decode(data);
        }
        "ipc_wire" => {
            let _ = eneros_ipc_wire::el0::decode_frame(data);
        }
        "modbus_rtu" => {
            let _ = eneros_modbus_rtu::frame::ModbusFrame::decode(data);
        }
        "iec104" => {
            let _ = eneros_iec104_slave::apdu::Apdu::decode(data);
        }
        "elf_loader" => {
            let _ = eneros_elf_loader::LoadedElf::parse(data);
        }
        "gguf_loader" => {
            if let Ok((header, consumed)) = eneros_gguf_loader::header::GgufHeader::parse(data) {
                let _ = eneros_gguf_loader::metadata::GgufMetadata::parse(
                    data,
                    consumed,
                    header.metadata_kv_count,
                );
            }
        }
        _ => {}
    }
}

/// 将崩溃复现输入写入 `target/fuzz-crashes/<target>-<iter>.bin`。
///
/// v0.250.0（C-250）：崩溃语料归档——nightly 长跑 fuzz 的崩溃输入
/// 上传为 CI artifact，供次日 triage 与防回归测试复用。
fn save_crash_input(target: &str, iter: usize, data: &[u8]) {
    use std::io::Write;
    let dir = std::path::Path::new("target").join("fuzz-crashes");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{target}-{iter}.bin"));
    if let Ok(mut f) = std::fs::File::create(&path) {
        let _ = f.write_all(data);
    }
}

pub fn command_fuzz(target: &str, iterations: usize) -> DynResult<()> {
    let seed = seed_for(target);
    let mut rng = Prng(0x4D595DF4D0F33173);
    let mut crashes: Vec<String> = Vec::new();
    let start = Instant::now();
    for i in 0..iterations {
        let mut data = mutate(seed, &mut rng, 32);
        if data.is_empty() {
            data = seed.to_vec();
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            exercise(target, &data);
        }));
        if result.is_err() {
            crashes.push(format!(
                "iter={i} len={} bytes={:02x?}",
                data.len(),
                &data[..data.len().min(64)]
            ));
            // v0.250.0（C-250）：崩溃复现输入落盘（target/fuzz-crashes/），
            // 供 nightly 归档与回归测试复用。
            save_crash_input(target, i, &data);
        }
    }
    let elapsed = start.elapsed();
    println!(
        "FUZZ target={} iters={} crashes={} elapsed={:?}",
        target,
        iterations,
        crashes.len(),
        elapsed
    );
    if crashes.is_empty() {
        println!("FUZZ_CRASH_CLEAN target={}", target);
        Ok(())
    } else {
        for c in &crashes {
            println!("CRASH {c}");
        }
        Err(format!("fuzz target {target} crashed {} times", crashes.len()).into())
    }
}
