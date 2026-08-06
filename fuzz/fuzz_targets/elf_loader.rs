//! VRF-001：ELF loader 解析 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = eneros_elf_loader::LoadedElf::parse(data);
});
