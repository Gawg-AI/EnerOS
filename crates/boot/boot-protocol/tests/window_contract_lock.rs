//! WP-74.2.2（蓝图 74 卷二 / R15-B）：BOOT-007 窗口契约对拍门。
//!
//! 锁定窗口三元组 + 内核加载址的跨面一致性——四事实源互不锁、注释已
//! 腐化的历史病灶（第十五轮体检 R15-B）：任何一面单点漂移即红，防止
//! 「dts 改了常量没改」「补丁改了 n 没改」「脚本改了 xtask 没改」复发。
//!
//! include_str 的被锁文件：
//! - third_party/sel4-top/overlay-init-bundle-reserved.dts（保留窗 reg）
//! - third_party/sel4-patches/0001-init-bundle-mapping.patch（C 侧字面
//!   + vspace.c 隐函数 n+=）
//! - third_party/sel4-build-common.sh / sel4-build.sh / sel4-build-rust.sh
//!   （构建面 IMAGE_START_ADDR）
//! - xtask/src/image_factory.rs（KERNEL_LOAD_ADDRESS + 手抄退役回归锁）
//!
//! R14 守卫自咬教训：期望串一律由常量运行期 format! 拼接，**测试文本
//! 禁止出现裸字面**（include_str 文本与针文本互数）。

use eneros_boot_protocol::{
    ENEROS_INIT_BUNDLE_PHYS, ENEROS_MAPPED_WINDOW_LEN, ENEROS_RESERVED_WINDOW_LEN,
};

const DTS: &str = include_str!("../../../../third_party/sel4-top/overlay-init-bundle-reserved.dts");
const PATCH: &str =
    include_str!("../../../../third_party/sel4-patches/0001-init-bundle-mapping.patch");
const BUILD_COMMON: &str = include_str!("../../../../third_party/sel4-build-common.sh");
const BUILD_C: &str = include_str!("../../../../third_party/sel4-build.sh");
const BUILD_RUST: &str = include_str!("../../../../third_party/sel4-build-rust.sh");
const IMAGE_FACTORY: &str = include_str!("../../../../xtask/src/image_factory.rs");
const BOOT_MANAGER: &str = include_str!("../../../../crates/boot/boot-manager-uefi/src/lib.rs");

/// 期望 `reg = <0x00 {base} 0x00 {len}>`（dts cell 为 8 位补零十六进制）。
fn dts_reg_expected() -> String {
    format!(
        "reg = <0x00 0x{:08x} 0x00 0x{:08x}>",
        ENEROS_INIT_BUNDLE_PHYS, ENEROS_RESERVED_WINDOW_LEN
    )
}

/// 从文本提取 vspace.c hunk 的 `n += {k};`（复查轮收紧：先锚定 vspace.c
/// 段再 find——补丁前文合法演化出别的 `n +=` 行时 find-first 会抓错行。
/// `---`/`+++` 两行各含一次路径锚点，取 **最后** 一段 = `+++` 之后的
/// hunk 本体）。
fn patch_n_plus_value() -> u64 {
    let vspace_hunk = PATCH
        .split("64/kernel/vspace.c")
        .last()
        .expect("补丁缺少 vspace.c hunk 头");
    let line = vspace_hunk
        .lines()
        .find(|l| l.contains("n += "))
        .expect("补丁 vspace.c hunk 缺少 n += 行（隐函数记账被移除？）");
    let v = line
        .split("n += ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .expect("n += 行无法解析出整数");
    v
}

/// 隐函数公式：映射窗口长度 / 2 MiB（PT 粒度）+ 1 PD。
/// 8 MiB → 5；未来扩窗 16 MiB → 9（漏改补丁即红）。
fn expected_paging_count() -> u64 {
    ENEROS_MAPPED_WINDOW_LEN / (2 * 1024 * 1024) + 1
}

/// 内核加载址：从 image_factory.rs 解析 KERNEL_LOAD_ADDRESS 字面
/// （跨文件锁只能走文本；下划线形态 0x4100_0000 须先剥）。
fn kernel_load_address() -> u64 {
    let line = IMAGE_FACTORY
        .lines()
        .find(|l| l.contains("const KERNEL_LOAD_ADDRESS"))
        .expect("image_factory.rs 缺少 KERNEL_LOAD_ADDRESS 定义行");
    let raw = line
        .split('=')
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .expect("KERNEL_LOAD_ADDRESS 行无值")
        .trim();
    u64::from_str_radix(&raw.replace('_', "").replace("0x", ""), 16)
        .expect("KERNEL_LOAD_ADDRESS 非十六进制字面")
}

#[test]
fn dts_reserved_window_matches_constants() {
    // 断言 1：dts reg 保留窗 ↔ boot-protocol 常量（base = bundle 物理址，
    // len = 保留窗全长）。
    assert!(
        DTS.contains(&dts_reg_expected()),
        "dts reg 与常量不一致：期望含 {:?}\n实际 reg 行：{:?}",
        dts_reg_expected(),
        DTS.lines()
            .find(|l| l.contains("reg = <"))
            .unwrap_or("<reg 行缺失>")
    );
}

#[test]
fn patch_literals_match_constants() {
    // 断言 2：补丁 C 侧字面 ↔ 常量（SIZE 锁映射窗长度非保留窗全长）。
    let phys = format!("ENEROS_INIT_BUNDLE_PHYS = {:#x}UL", ENEROS_INIT_BUNDLE_PHYS);
    let size = format!(
        "ENEROS_INIT_BUNDLE_SIZE = {:#x}UL",
        ENEROS_MAPPED_WINDOW_LEN
    );
    assert!(PATCH.contains(&phys), "补丁缺物理址字面：{phys}");
    assert!(PATCH.contains(&size), "补丁缺映射窗长字面：{size}");
}

#[test]
fn patch_paging_count_matches_window_formula() {
    // 断言 3：vspace.c 隐函数公式锁（8 MiB→5；扩窗漏改即红）。
    assert_eq!(
        patch_n_plus_value(),
        expected_paging_count(),
        "n += 与映射窗公式不一致：{} MiB 窗应 n += {}（PD + 每 2 MiB 一 PT）",
        ENEROS_MAPPED_WINDOW_LEN / (1024 * 1024),
        expected_paging_count()
    );
}

#[test]
fn kernel_load_address_consistent_across_build_surface() {
    // 断言 4：内核加载址三源一致——xtask 常量 ↔ 构建脚本 IMAGE_START_ADDR
    // （复查轮收紧：只认 cmake 旗标形态 `-DIMAGE_START_ADDR=`，注释/散文
    // 中的提法不再满足锁；出现 ≥1 次且处处同值——common 化后单点在
    // common，wrapper 不再持字面）。
    let v = kernel_load_address();
    let expected_arg = format!("-DIMAGE_START_ADDR={:#x}", v);
    let corpus = format!("{BUILD_COMMON}\n{BUILD_C}\n{BUILD_RUST}");
    let hits: Vec<&str> = corpus
        .lines()
        .filter(|l| l.contains("-DIMAGE_START_ADDR="))
        .collect();
    assert!(
        !hits.is_empty(),
        "构建脚本面无 -DIMAGE_START_ADDR（对拍面失联）"
    );
    for hit in &hits {
        assert!(
            hit.contains(&expected_arg),
            "IMAGE_START_ADDR 与 xtask KERNEL_LOAD_ADDRESS 不一致：行 {:?} 期望含 {:?}",
            hit,
            expected_arg
        );
    }
}

#[test]
fn init_load_address_literal_retired() {
    // 断言 5：WP-74.2.1 手抄退役回归锁——xtask 侧 0x4200_0000 字面定义
    // 不得复活，且加载址赋值必须消费 boot-protocol 常量（复查轮收紧：
    // 原断言只锁 use 行出现——import 在场而赋值硬编码字面可假绿；
    // 现锁消费点本体 `load_address: ENEROS_INIT_BUNDLE_PHYS`）。
    assert!(
        !IMAGE_FACTORY.contains("const INIT_LOAD_ADDRESS"),
        "INIT_LOAD_ADDRESS 手抄复活（bundle 加载址唯一事实源 = boot-protocol）"
    );
    assert!(
        IMAGE_FACTORY.contains("load_address: ENEROS_INIT_BUNDLE_PHYS"),
        "bundle load_address 未消费 ENEROS_INIT_BUNDLE_PHYS（单源断裂）"
    );
}

#[test]
fn kernel_load_address_consistent_across_rust_surface() {
    // 断言 6（复查轮补锁）：内核加载址 Rust 侧两字面互锁——xtask
    // KERNEL_LOAD_ADDRESS ↔ boot-manager-uefi KERNEL_LOAD_START（U830
    // 校验面）。两处同事实字面此前无任何机器对拍，一面漂移即静默断裂。
    let boot_manager_line = BOOT_MANAGER
        .lines()
        .find(|l| l.contains("pub const KERNEL_LOAD_START"))
        .expect("boot-manager-uefi 缺少 KERNEL_LOAD_START 定义行");
    let raw = boot_manager_line
        .split('=')
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .expect("KERNEL_LOAD_START 行无值")
        .trim();
    let v = u64::from_str_radix(&raw.replace('_', "").replace("0x", ""), 16)
        .expect("KERNEL_LOAD_START 非十六进制字面");
    assert_eq!(
        kernel_load_address(),
        v,
        "xtask KERNEL_LOAD_ADDRESS 与 boot-manager KERNEL_LOAD_START 漂移"
    );
}

#[test]
fn kernel_load_span_consistent_across_rust_surface() {
    // 断言 7（复查轮补锁）：内核加载窗跨度互锁——xtask 校验面的
    // `KERNEL_LOAD_ADDRESS + 8*1024*1024` 手抄跨度 ↔ boot-manager 的
    // KERNEL_LOAD_END - KERNEL_LOAD_START。跨度漂移时装载校验两面失配。
    let span_line = IMAGE_FACTORY
        .lines()
        .find(|l| l.contains("KERNEL_LOAD_ADDRESS + "))
        .expect("image_factory.rs 缺少内核加载窗跨度表达式");
    let span_xtask: u64 = span_line
        .split("+ ")
        .nth(1)
        .and_then(|rest| {
            rest.split('*')
                .map(str::trim)
                .collect::<Vec<_>>()
                .first()
                .copied()
        })
        .and_then(|s| s.replace('_', "").parse::<u64>().ok())
        .expect("跨度表达式无法解析出首因子")
        * 1024
        * 1024;
    let bm_start = BOOT_MANAGER
        .lines()
        .find(|l| l.contains("pub const KERNEL_LOAD_START"))
        .and_then(|l| l.split('=').nth(1))
        .and_then(|r| r.split(';').next())
        .map(|s| s.trim().replace('_', "").replace("0x", ""))
        .and_then(|s| u64::from_str_radix(&s, 16).ok())
        .expect("KERNEL_LOAD_START 解析失败");
    let bm_end = BOOT_MANAGER
        .lines()
        .find(|l| l.contains("pub const KERNEL_LOAD_END"))
        .and_then(|l| l.split('=').nth(1))
        .and_then(|r| r.split(';').next())
        .map(|s| s.trim().replace('_', "").replace("0x", ""))
        .and_then(|s| u64::from_str_radix(&s, 16).ok())
        .expect("KERNEL_LOAD_END 解析失败");
    assert_eq!(
        span_xtask,
        bm_end - bm_start,
        "内核加载窗跨度漂移：xtask {span_xtask:#x} vs boot-manager {:#x}",
        bm_end - bm_start
    );
}
