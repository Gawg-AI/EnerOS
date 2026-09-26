//! 蓝图 76 卷一 WP-76.1.4 · EL0 panic 声化全族守卫（单点）。
//!
//! 宏宿主 = 守卫宿主：`svc_panic!` 定义在本 crate，跨 crate `include_str!`
//! 扫五族 bin 源是**有意的设计**（守卫的单点性；路径写错 = 编译期错，安全）。
//! 覆盖 30 bin = 本卷点名的 21 目标 bin（protocol 11 + drv 5 + control 5）
//! + svc-storage 1 + business 8——business 旧守卫
//! （`eneros-business-services` `bins_panic_handler_vocalized_single_source`，
//! 覆盖面八 bin）随本守卫收编而退役，判据为其超集。
//!
//! 自咬纪律（R14 两教训）：所有针**运行期拼接**（本文件不出现连续针形，
//! 由 `guard_source_carries_no_contiguous_needles` 上锁），且断言方向选择上
//! 「误红不假绿」——宁可挡住注释里的巧合文本，不放过真实回流。
//!
//! 被扫对象同款纪律（独立复查轮补；变异 MD/ME 两条假绿各自实跑到绿后才上的锁）：
//! 判据① 调用针**宿主路径入针 + 独占一行**——调用点回指同名局部宏即单源失守，
//! 旧宽松形放过它而全绿；判据② 宏体针只在**宏定义之后**数且恰一次——只数次数
//! 不够（宏 doc 复述针形 + 宏体删半边是一增一减，计数仍为 1），须锚定扫描域。

// 测试断言允许 panic!/expect!（仓库惯例：elf-loader/ota-ab/board-facts 先例
// ——disallowed_macros 是生产代码纪律，本模块 cfg(test) 通篇是判据断言）。
#![allow(clippy::disallowed_macros)]

extern crate std;

use std::fs;

/// ① 宏调用针前段（**宏宿主路径** + 宏名 + 调用括号 + 开引号）——分段书写：本文件一旦出现
/// 连续针形，`guard_source_carries_no_contiguous_needles` 即红（守卫自咬锁）。
/// 宿主路径入针：只写宏名则调用点回指任一同名 `svc_panic!`（例如本 crate 复刻
/// 一份静默体，即本卷收编前的原病灶形）判据①仍绿——「单源」失守而全绿。
const CALL_OPEN: &str = concat!("eneros_runtime", "::svc_panic", "!(\"");
/// ② 自有 panic handler 属性——禁物。
const HANDLER_ATTR: &str = concat!("#", "[panic_handler]");
/// ③ 手抄 handler 函数头——禁物（挡住绕过属性拼写的回流形态）。
const PANIC_FN: &str = concat!("fn ", "panic(");
/// ④ 宏体打印半边词形模板。
const PRINT_TEMPLATE: &str = concat!("PANIC: {}", "\", $name");
/// ⑤ 宏体挂起半边（ADR-026 §1 不复位整机、自旋等监督面）。
/// 裸 `spin_loop` 亦出现在宏 doc 注释里（doc 遮蔽 ⇒ 删宏体自旋不红），
/// 故针取宏体独有全路径形。
const SUSPEND_TOKEN: &str = concat!("core::hint::spin_", "loop");
/// ⑥ A 族审计 hook 调用针（豁免族须保持的另一形态）。
const AUDIT_HOOK: &str = concat!("svc_panic", "_hook(");

/// 守卫表：`(bin 名, 源文件)` ——30 个宏调用点，逐一 compile-time 读入。
const TABLE: &[(&str, &str)] = &[
    // business-services（WP-75.2.3 先例族，本卷换宏路径）
    (
        "svc-market",
        include_str!("../../../agents/business-services/src/bin/svc-market.rs"),
    ),
    (
        "svc-ledger",
        include_str!("../../../agents/business-services/src/bin/svc-ledger.rs"),
    ),
    (
        "svc-agent-bus",
        include_str!("../../../agents/business-services/src/bin/svc-agent-bus.rs"),
    ),
    (
        "svc-cloud-sync",
        include_str!("../../../agents/business-services/src/bin/svc-cloud-sync.rs"),
    ),
    (
        "svc-config",
        include_str!("../../../agents/business-services/src/bin/svc-config.rs"),
    ),
    (
        "svc-mgmt",
        include_str!("../../../agents/business-services/src/bin/svc-mgmt.rs"),
    ),
    (
        "svc-observability",
        include_str!("../../../agents/business-services/src/bin/svc-observability.rs"),
    ),
    (
        "svc-ota",
        include_str!("../../../agents/business-services/src/bin/svc-ota.rs"),
    ),
    // control-services（WP-76.1.2 手抄收编）
    (
        "svc-telemetry",
        include_str!("../../../agents/control-services/src/bin/svc-telemetry.rs"),
    ),
    (
        "svc-energy-agent",
        include_str!("../../../agents/control-services/src/bin/svc-energy-agent.rs"),
    ),
    (
        "svc-device-agent",
        include_str!("../../../agents/control-services/src/bin/svc-device-agent.rs"),
    ),
    (
        "svc-control-dispatch",
        include_str!("../../../agents/control-services/src/bin/svc-control-dispatch.rs"),
    ),
    (
        "svc-ai-worker",
        include_str!("../../../agents/control-services/src/bin/svc-ai-worker.rs"),
    ),
    // drv-svc（storage 为 WP-76.1.2 同律收编，五 bin 为 WP-76.1.3 声化）
    (
        "svc-storage",
        include_str!("../../../drivers/drv-svc/src/bin/svc-storage.rs"),
    ),
    (
        "svc-drv-blk",
        include_str!("../../../drivers/drv-svc/src/bin/svc-drv-blk.rs"),
    ),
    (
        "svc-drv-net",
        include_str!("../../../drivers/drv-svc/src/bin/svc-drv-net.rs"),
    ),
    (
        "svc-drv-rng",
        include_str!("../../../drivers/drv-svc/src/bin/svc-drv-rng.rs"),
    ),
    (
        "svc-drv-console",
        include_str!("../../../drivers/drv-svc/src/bin/svc-drv-console.rs"),
    ),
    (
        "svc-drv-fault",
        include_str!("../../../drivers/drv-svc/src/bin/svc-drv-fault.rs"),
    ),
    // protocol-services（WP-76.1.3 静默黑洞声化，11 bin）
    (
        "svc-modbus-rtu",
        include_str!("../../../protocols/protocol-services/src/bin/svc-modbus-rtu.rs"),
    ),
    (
        "svc-modbus-tcp",
        include_str!("../../../protocols/protocol-services/src/bin/svc-modbus-tcp.rs"),
    ),
    (
        "svc-iec104",
        include_str!("../../../protocols/protocol-services/src/bin/svc-iec104.rs"),
    ),
    (
        "svc-iec101",
        include_str!("../../../protocols/protocol-services/src/bin/svc-iec101.rs"),
    ),
    (
        "svc-iec103",
        include_str!("../../../protocols/protocol-services/src/bin/svc-iec103.rs"),
    ),
    (
        "svc-goose",
        include_str!("../../../protocols/protocol-services/src/bin/svc-goose.rs"),
    ),
    (
        "svc-sv",
        include_str!("../../../protocols/protocol-services/src/bin/svc-sv.rs"),
    ),
    (
        "svc-mms",
        include_str!("../../../protocols/protocol-services/src/bin/svc-mms.rs"),
    ),
    (
        "svc-mqtt",
        include_str!("../../../protocols/protocol-services/src/bin/svc-mqtt.rs"),
    ),
    (
        "svc-dlt645",
        include_str!("../../../protocols/protocol-services/src/bin/svc-dlt645.rs"),
    ),
    (
        "svc-gbt27930",
        include_str!("../../../protocols/protocol-services/src/bin/svc-gbt27930.rs"),
    ),
];

/// 有意豁免（不在表内；登记册 REG-76-B 逐条理由）：
/// - svc-hang：静默挂起靶（心跳/监督面测试用），其 handler **必须**静默；
/// - oom-probe / panic-probe：panic/OOM 靶；
/// - hello / fault / hog / pi / pie：A 族，`eneros-services` 审计链
///   （ADR-026 §1 全文语义 = 审计 + 打印 + 挂起），本卷不收编；
/// - root-task：内核侧自身分层 handler；hal/board：非 bin。
const EXEMPT_AUDIT_FAMILY: &[(&str, &str)] = &[
    (
        "svc-hello",
        include_str!("../../services/src/bin/svc-hello.rs"),
    ),
    (
        "svc-fault",
        include_str!("../../services/src/bin/svc-fault.rs"),
    ),
    ("svc-hog", include_str!("../../services/src/bin/svc-hog.rs")),
    ("svc-pi", include_str!("../../services/src/bin/svc-pi.rs")),
    ("pie", include_str!("../../svc-pie/src/main.rs")),
];

/// 四族 bin 目录（完整性扫描域）——相对 `CARGO_MANIFEST_DIR`
/// （= `crates/runtime/runtime`，两级上跳到 `crates/`）。
const BIN_DIRS: &[&str] = &[
    "/../../agents/business-services/src/bin",
    "/../../agents/control-services/src/bin",
    "/../../drivers/drv-svc/src/bin",
    "/../../protocols/protocol-services/src/bin",
];

fn manifest_dir() -> &'static str {
    env!("CARGO_MANIFEST_DIR")
}

/// 主判据：30 bin 逐一「宏收编 + 自有 handler 零残留」。
#[test]
fn el0_bins_vocalized_via_single_macro() {
    assert_eq!(
        TABLE.len(),
        30,
        "守卫表条目数与卷一清点（21 目标 + storage + business 8）不符"
    );
    for &(name, src) in TABLE {
        // 冻结形 = 「bin 只留一行调用」（§3 步 1）：针含**宏宿主路径**，调用语句
        // 独占一行且恰一次。旧形（`contains` + 首处命中行前缀只要求以 `::` 结尾）
        // 放过变异 MD——调用点回指本 crate 复刻的同名静默宏 ⇒ 单源失守而全绿。
        // 残余：跨行块注释包住的整行语句仍能伪造正向命中，但该形下 bin 无 handler
        // ⇒ target 链接期硬错，不可能静默出货；登记不修（登记册 B76-1 二次复查轮）。
        let stmt = [CALL_OPEN, name, "\");"].concat();
        let hits = src.lines().filter(|l| l.trim() == stmt).count();
        assert_eq!(
            hits, 1,
            "{name} 未以独占一行的 {stmt} 收编 panic handler（命中 {hits} 次 \
             = 静默回流 / 注释伪造 / 宏宿主不是 eneros-runtime / 行内附加文本）"
        );
        assert!(
            !src.contains(HANDLER_ATTR),
            "{name} 残留自有 panic handler 属性（单源宏是唯一出口）"
        );
        assert!(
            !src.contains(PANIC_FN),
            "{name} 残留手抄 handler 函数头（WP-76.1.2/76.1.3 单源化回流）"
        );
    }
}

/// 宏本体两半边上锁：打印（死因可见）+ 挂起（不复位整机）。删任一半即红。
#[test]
fn macro_body_locks_print_and_suspend_halves() {
    let macro_src = include_str!("lib.rs");
    // 扫描域锚定在**宏定义之后**：宏 doc 在定义之前，若按整文件计数，「doc 复述
    // 针形」与「宏体半边已删」一增一减仍是恰一次（变异 ME 实证绿）——锚定后
    // doc 落在扫描域外，宏体改动必红。
    let Some(def_at) = macro_src.find(concat!("macro_rules! ", "svc_panic")) else {
        panic!("svc_panic! 宏定义不在 lib.rs（判据②失去被扫对象）");
    };
    let macro_body = &macro_src[def_at..];
    let handler_sig = ["fn ", "panic(info: &core::panic::PanicInfo)"].concat();
    for (label, needle) in [
        ("打印半边", PRINT_TEMPLATE),
        ("挂起半边", SUSPEND_TOKEN),
        ("handler 签名", handler_sig.as_str()),
    ] {
        let hits = macro_body.matches(needle).count();
        assert_eq!(
            hits, 1,
            "svc_panic! 宏体{label}针在宏定义之后命中 {hits} 次（0=半边被删=声化/挂起失效；≥2=宏体复述或双定义）"
        );
    }
}

/// 完整性：四族 bin 目录被守卫表**完全划分**——每个 bin 要么在表内（宏收编），
/// 要么在显式静默豁免名单里。新 bin 不入表 = 红；表内 bin 不真调用宏 = 红。
/// 清点实证（卷一基线）：business 8 + control 6 + drv 6 + protocol 11 = 31 文件
/// = 表 30 + svc-hang 1。
#[test]
fn bin_directories_are_fully_partitioned() {
    const EXEMPT_SILENT: &[&str] = &["svc-hang"];
    let mut tabled_callers = 0usize;
    let mut scanned = 0usize;
    for &dir in BIN_DIRS {
        let path = [manifest_dir(), dir].concat();
        let entries = fs::read_dir(&path).unwrap_or_else(|e| panic!("读 {path} 失败: {e}"));
        for entry in entries {
            let entry = entry.expect("目录项可读");
            let os_name = entry.file_name();
            let file: &str = os_name.to_str().expect("bin 文件名是合法 utf-8");
            if !file.ends_with(".rs") {
                continue;
            }
            scanned += 1;
            let stem = file.trim_end_matches(".rs");
            let src = fs::read_to_string(entry.path()).expect("bin 源可读");
            let uses_macro = src.contains(CALL_OPEN);
            if uses_macro {
                tabled_callers += 1;
            }
            assert!(
                TABLE.iter().any(|(n, _)| *n == stem) || EXEMPT_SILENT.contains(&stem),
                "{stem} 在目录里却不在守卫表（登记新 bin 须同步入表，见 REG-76-C）"
            );
        }
    }
    assert_eq!(
        scanned,
        TABLE.len() + EXEMPT_SILENT.len(),
        "四族 bin 文件数与「表 + 豁免」不符（清点漂移）"
    );
    assert_eq!(
        tabled_callers,
        TABLE.len(),
        "调用宏的 bin 数与守卫表不符（表内 bin 须真实调用宏）"
    );
}

/// 豁免族形态分立上锁：A 族保持审计链且**不**用本宏；svc-hang 保持静默。
/// 方向选择：挡住「好心把豁免族也收编」和「挂起靶被声化」两个误动。
#[test]
fn exempt_bins_keep_their_distinct_forms() {
    for &(name, src) in EXEMPT_AUDIT_FAMILY {
        assert!(
            src.contains(AUDIT_HOOK),
            "{name} 丢了 A 族审计 hook（ADR-026 §1 全文语义，本卷不动）"
        );
        assert!(
            !src.contains(CALL_OPEN),
            "{name} 被误收编进 svc_panic! 单源（豁免族，见守卫表 doc）"
        );
    }
    let hang = include_str!("../../../agents/control-services/src/bin/svc-hang.rs");
    assert!(
        hang.contains(HANDLER_ATTR),
        "svc-hang 的静默 handler 被删（它是挂起靶，必须静默）"
    );
    assert!(
        !hang.contains("PANIC: "),
        "svc-hang 被声化（挂起靶语义 = 心跳/监督面测试依赖无声）"
    );
    assert!(
        !TABLE.iter().any(|(n, _)| *n == "svc-hang"),
        "svc-hang 误入守卫表（守卫自咬）"
    );
}

/// 自咬锁：本文件不得出现任何**连续**针形——针一律 `concat!` 分段书写，
/// 否则守卫文本自身即命中判据（R14「守卫自咬」教训的可执行化）。
/// 方向选择：误红不假绿——日后想在本文件提到针形，须照样分段。
#[test]
fn guard_source_carries_no_contiguous_needles() {
    let own = include_str!("panic_guard.rs");
    for (label, needle) in [
        ("调用针", CALL_OPEN),
        ("handler 属性针", HANDLER_ATTR),
        ("函数头针", PANIC_FN),
        ("审计 hook 针", AUDIT_HOOK),
        ("宏体打印模板", PRINT_TEMPLATE),
        ("宏体挂起令牌", SUSPEND_TOKEN),
    ] {
        let hits = own.matches(needle).count();
        assert_eq!(hits, 0, "{label} 在本文件连续出现 {hits} 次（须分段书写）");
    }
}
