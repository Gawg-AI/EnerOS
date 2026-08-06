//! VRF-001：ELF loader 解析 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // v0.250.0（C-250）：性质断言升级——parse 已内置 W^X/entry 校验
    // （解析成功即满足），此处补防御性结构断言：
    // 1) 结构断言：每个段的 W^X 不变量（可执行段不得同时可写）；
    // 2) 结构断言：total_vmem 有界（≤ 1 GiB，防异常段大小）；
    // 3) 结构断言：段 memsz 非零时 vaddr 不为 0（合法装载地址）。
    if let Ok(elf) = eneros_elf_loader::LoadedElf::parse(data) {
        for seg in elf.segments() {
            assert!(
                !seg.flags.is_wx_violation(),
                "W^X violation segment must not pass parse"
            );
            assert!(
                seg.memsz == 0 || seg.vaddr != 0,
                "zero vaddr with non-zero memsz"
            );
        }
        let vmem = elf.total_vmem();
        assert!(vmem <= 1 << 30, "total_vmem {vmem} exceeds 1 GiB bound");
    }
});
