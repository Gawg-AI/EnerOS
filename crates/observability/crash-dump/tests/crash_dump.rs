//! OBS-007 崩溃转储集成测试。
//!
//! 验证：
//! 1. encode/decode round-trip 保持所有字段。
//! 2. CRC32 损坏被拒绝。
//! 3. build_id 精确匹配（核心要求）。
//! 4. 从合成 ELF 提取 build_id。
//! 5. 空 build_id 拒绝匹配。
//! 6. build_id 不匹配拒绝符号化。

use eneros_crash_dump::{
    extract_build_id, verify_build_id_match, Arch, BuildId, CrashDump, CrashFrame,
};

fn sample_dump() -> CrashDump {
    let bid = BuildId::from_bytes(&[
        0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67,
        0x89, 0xab, 0xcd, 0xef, 0x01,
    ])
    .unwrap();
    let mut dump = CrashDump::new(bid);
    dump.crash_ts_ns = 1_700_000_000_000_000_000;
    dump.thread_id = 42;
    dump.pc = 0x4000_1000;
    dump.sp = 0x7fff_0000;
    dump.lr = 0x4000_0800;
    dump.arch = Arch::Aarch64;
    dump.panic_msg = "kernel panic at sched::dispatch".to_owned();
    dump.frames = vec![
        CrashFrame { pc: 0x4000_1000 },
        CrashFrame { pc: 0x4000_0800 },
        CrashFrame { pc: 0x4000_0500 },
    ];
    dump
}

#[test]
fn encode_decode_roundtrip_preserves_all_fields() {
    let original = sample_dump();
    let bytes = eneros_crash_dump::encode(&original).unwrap();
    let decoded = eneros_crash_dump::decode(&bytes).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn crc_corruption_is_rejected() {
    let dump = sample_dump();
    let mut bytes = eneros_crash_dump::encode(&dump).unwrap();
    // 翻转最后一个字节（CRC32 的一部分）
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    let result = eneros_crash_dump::decode(&bytes);
    assert_eq!(result, Err(eneros_crash_dump::CrashDumpError::CrcMismatch));
}

#[test]
fn payload_corruption_is_detected_by_crc() {
    let dump = sample_dump();
    let mut bytes = eneros_crash_dump::encode(&dump).unwrap();
    // 翻转 payload 中间的一个字节（不是 CRC）
    bytes[50] ^= 0xFF;
    let result = eneros_crash_dump::decode(&bytes);
    assert_eq!(result, Err(eneros_crash_dump::CrashDumpError::CrcMismatch));
}

#[test]
fn bad_magic_is_rejected() {
    let dump = sample_dump();
    let mut bytes = eneros_crash_dump::encode(&dump).unwrap();
    bytes[0] = b'X';
    // 重新计算 CRC 使其通过 CRC 检查但 magic 错误
    let payload_len = bytes.len() - 4;
    let crc = eneros_crash_dump::crc32(&bytes[..payload_len]);
    bytes[payload_len..].copy_from_slice(&crc.to_le_bytes());
    let result = eneros_crash_dump::decode(&bytes);
    assert_eq!(result, Err(eneros_crash_dump::CrashDumpError::BadMagic));
}

#[test]
fn empty_build_id_is_rejected_at_construction() {
    // BuildId::from_bytes 对空切片返回 None — 空建 ID 在 API 层即被拒绝，
    // 不可能构造出带空 build_id 的 CrashDump。
    assert!(BuildId::from_bytes(&[]).is_none());
}

#[test]
fn build_id_mismatch_on_different_lengths() {
    // 20 字节 build_id 不应匹配 32 字节 build_id（即使前 20 字节相同）
    let mut id20 = [0x11u8; 20];
    let mut id32 = [0x11u8; 32];
    id32[..20].copy_from_slice(&id20);
    // 确保它们不同（长度不同）
    let mut dump = sample_dump();
    dump.build_id = BuildId::from_bytes(&id20).unwrap();
    let elf = make_minimal_elf_with_build_id(&id32);
    // build-id 长度不同 → 不匹配
    assert!(!verify_build_id_match(&dump, &elf));
    // 反向也不匹配
    let mut dump2 = sample_dump();
    dump2.build_id = BuildId::from_bytes(&id32).unwrap();
    let elf2 = make_minimal_elf_with_build_id(&id20);
    assert!(!verify_build_id_match(&dump2, &elf2));
    // 防止编译器警告
    let _ = &mut id20;
}

#[test]
fn build_id_mismatch_rejects_symbolization() {
    let mut dump = sample_dump();
    dump.build_id = BuildId::from_bytes(&[0xAA; 20]).unwrap();
    let elf = make_minimal_elf_with_build_id(&[0xBB; 20]);
    assert!(!verify_build_id_match(&dump, &elf));
}

#[test]
fn build_id_exact_match_allows_symbolization() {
    let id_bytes = [0x11; 20];
    let mut dump = sample_dump();
    dump.build_id = BuildId::from_bytes(&id_bytes).unwrap();
    let elf = make_minimal_elf_with_build_id(&id_bytes);
    assert!(verify_build_id_match(&dump, &elf));
}

#[test]
fn extract_build_id_from_synthetic_elf() {
    let id_bytes = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE];
    let elf = make_minimal_elf_with_build_id(&id_bytes);
    let extracted = extract_build_id(&elf).unwrap();
    assert_eq!(extracted.as_bytes(), &id_bytes);
}

#[test]
fn extract_build_id_rejects_non_elf() {
    let not_elf = b"This is not an ELF file, just some text data.";
    assert!(extract_build_id(not_elf).is_none());
}

#[test]
fn extract_build_id_rejects_truncated_elf() {
    let id_bytes = [0x42; 20];
    let elf = make_minimal_elf_with_build_id(&id_bytes);
    // 截断到只有 ELF 头
    let truncated = &elf[..64];
    assert!(extract_build_id(truncated).is_none());
}

#[test]
fn build_id_to_hex_format() {
    let id = BuildId::from_bytes(&[0xAB, 0xCD, 0xEF]).unwrap();
    assert_eq!(id.to_hex(), "abcdef");
}

#[test]
fn build_id_too_long_is_rejected() {
    let too_long = [0u8; 33];
    assert!(BuildId::from_bytes(&too_long).is_none());
}

#[test]
fn empty_frames_encode_decode_correctly() {
    let mut dump = sample_dump();
    dump.frames.clear();
    let bytes = eneros_crash_dump::encode(&dump).unwrap();
    let decoded = eneros_crash_dump::decode(&bytes).unwrap();
    assert!(decoded.frames.is_empty());
    assert_eq!(dump, decoded);
}

#[test]
fn empty_panic_msg_encodes_correctly() {
    let mut dump = sample_dump();
    dump.panic_msg.clear();
    let bytes = eneros_crash_dump::encode(&dump).unwrap();
    let decoded = eneros_crash_dump::decode(&bytes).unwrap();
    assert!(decoded.panic_msg.is_empty());
    assert_eq!(dump, decoded);
}

#[test]
fn max_panic_msg_is_accepted() {
    let mut dump = sample_dump();
    dump.panic_msg = "A".repeat(512);
    let bytes = eneros_crash_dump::encode(&dump).unwrap();
    let decoded = eneros_crash_dump::decode(&bytes).unwrap();
    assert_eq!(dump, decoded);
}

#[test]
fn oversized_panic_msg_is_rejected() {
    let mut dump = sample_dump();
    dump.panic_msg = "A".repeat(513);
    let result = eneros_crash_dump::encode(&dump);
    assert_eq!(
        result,
        Err(eneros_crash_dump::CrashDumpError::BadPanicMsgLen)
    );
}

// ============================================================================
// 合成 ELF 构造器（带 .note.gnu.build-id 节）
// ============================================================================

/// 构造一个最小化的 ELF64 文件，包含 `.note.gnu.build-id` 节。
///
/// 布局：
/// - ELF header (64 bytes)
/// - .note.gnu.build-id section data
/// - .shstrtab section data
/// - Section header table (3 entries: null + .note + .shstrtab)
fn make_minimal_elf_with_build_id(id: &[u8]) -> Vec<u8> {
    // .note.gnu.build-id 内容
    let name = b"GNU\0";
    let namesz = name.len() as u32; // 4
    let descsz = id.len() as u32;
    let ntype: u32 = 3; // NT_GNU_BUILD_ID
    let name_padded = (namesz as usize + 3) & !3; // 4
    let desc_padded = (descsz as usize + 3) & !3;
    let note_len = 12 + name_padded + desc_padded;

    let mut note = Vec::with_capacity(note_len);
    note.extend_from_slice(&namesz.to_le_bytes());
    note.extend_from_slice(&descsz.to_le_bytes());
    note.extend_from_slice(&ntype.to_le_bytes());
    note.extend_from_slice(name);
    // name padding (already 4-aligned, no padding needed for "GNU\0")
    note.extend_from_slice(id);
    // desc padding
    let pad = desc_padded - id.len();
    note.extend(core::iter::repeat(0u8).take(pad));

    // .shstrtab 内容
    let shstrtab = b"\0.note.gnu.build-id\0.shstrtab\0";
    let note_name_off = 1u32; // ".note.gnu.build-id" 在 shstrtab 中的偏移
    let shstrtab_name_off = (1 + b".note.gnu.build-id\0".len()) as u32;

    // 布局偏移
    let ehdr_size = 64usize;
    let note_off = ehdr_size;
    let shstrtab_off = note_off + note.len();
    let shdr_off = shstrtab_off + shstrtab.len();
    let shentsize = 64u16;
    let shnum = 3u16;
    let shstrndx = 2u16;

    let mut elf = Vec::with_capacity(shdr_off + shnum as usize * shentsize as usize);

    // ELF header
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // e_ident magic
    elf.push(2); // ELFCLASS64
    elf.push(1); // ELFDATA2LSB
    elf.push(1); // EV_CURRENT
    elf.push(0); // ELFOSABI_NONE
    elf.extend_from_slice(&[0u8; 8]); // padding
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&183u16.to_le_bytes()); // e_machine = EM_AARCH64
    elf.extend_from_slice(&1u32.to_le_bytes()); // e_version
    elf.extend_from_slice(&0u64.to_le_bytes()); // e_entry
    elf.extend_from_slice(&0u64.to_le_bytes()); // e_phoff
    elf.extend_from_slice(&(shdr_off as u64).to_le_bytes()); // e_shoff
    elf.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_phnum
    elf.extend_from_slice(&shentsize.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&shnum.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&shstrndx.to_le_bytes()); // e_shstrndx
    assert_eq!(elf.len(), ehdr_size);

    // .note.gnu.build-id data
    elf.extend_from_slice(&note);

    // .shstrtab data
    elf.extend_from_slice(shstrtab);

    // Section header table
    // SHT_NULL (index 0)
    elf.extend_from_slice(&[0u8; 64]);

    // .note.gnu.build-id (index 1)
    elf.extend_from_slice(&note_name_off.to_le_bytes()); // sh_name
    elf.extend_from_slice(&7u32.to_le_bytes()); // sh_type = SHT_NOTE
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_flags
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_addr
    elf.extend_from_slice(&(note_off as u64).to_le_bytes()); // sh_offset
    elf.extend_from_slice(&(note.len() as u64).to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&4u64.to_le_bytes()); // sh_addralign
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_entsize
    assert_eq!(elf.len(), shdr_off + 64 * 2);

    // .shstrtab (index 2)
    elf.extend_from_slice(&shstrtab_name_off.to_le_bytes()); // sh_name
    elf.extend_from_slice(&3u32.to_le_bytes()); // sh_type = SHT_STRTAB
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_flags
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_addr
    elf.extend_from_slice(&(shstrtab_off as u64).to_le_bytes()); // sh_offset
    elf.extend_from_slice(&(shstrtab.len() as u64).to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&1u64.to_le_bytes()); // sh_addralign
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_entsize

    elf
}
