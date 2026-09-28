//! WP-77.2.1（蓝图 77 卷二）：identity capsule 预留时序回归锁。
//!
//! 本体在 `src/main.rs`（bin host 不可测——uefi feature 与 std test 的
//! panic lang-item 冲突），故以**源序锁**承载 host 侧变异咬合面：
//! 把预留重新放到 `prepare_attempt` 之后（= 恢复「分配失败仍扣 trial」
//! 的旧形态）或复活 soft-degrade，本锁即红。物理前提与硬失败行为由
//! QEMU 注入窗（tools/run77-vol2-inject-gate.sh）实弹验证。
//!
//! R14 守卫自咬教训：期望串运行期拼接，测试文本不出现被锁裸字面。

const MAIN_SRC: &str = include_str!("../src/main.rs");
const MANIFEST_SRC: &str = include_str!("../Cargo.toml");

#[test]
fn identity_capsule_reservation_precedes_first_prepare_attempt() {
    let alloc_needle = ["AllocateType", "::Address(IDENTITY_CAPSULE_PHYS)"].concat();
    let prepare_needle = [".", "prepare_attempt", "()"].concat();
    // 真预留的唯一锚 = 绑定语句（fault-inject 占页块是 `match
    // boot::allocate_pages`，不带绑定）——`Address(IDENTITY_CAPSULE_PHYS)`
    // 的首个出现永远在占页块，用它定位会锁错对象：真预留被挪到
    // prepare_attempt 之后本测仍绿（变异实证 2026-09-28，判别力盲区）。
    let real_reserve_needle = ["let identity_capsule_page ", "= boot::allocate_pages("].concat();
    let reserve = MAIN_SRC
        .find(&real_reserve_needle)
        .expect("capsule 真预留绑定缺失（WP-77.2.1 主不变量面失联）");
    // 绑定语句之后必须紧跟 Address 类型（防绑定换成 AnyPages 等松散形态）。
    let after = &MAIN_SRC[reserve..];
    assert!(
        after.find(&alloc_needle).is_some_and(|offset| offset < 200),
        "真预留绑定未以 Address(IDENTITY_CAPSULE_PHYS) 分配"
    );
    let first_prepare = MAIN_SRC
        .find(&prepare_needle)
        .expect("prepare_attempt 调用缺失");
    assert!(
        reserve < first_prepare,
        "capsule 预留必须先于第一次 prepare_attempt（预留晚于持久扣次 = 分配失败仍扣 trial 的旧缺陷复活）"
    );
    // 占页注入（fault-inject）也必须在真实预留之前。
    let occupy_needle = ["eneros_fault_inject_occupy", "_capsule_page()"].concat();
    if let Some(occupy) = MAIN_SRC.find(&occupy_needle) {
        assert!(
            occupy < reserve,
            "占页注入必须先于真实预留（否则注入建模失真）"
        );
    }
}

#[test]
fn capsule_soft_degrade_retired() {
    // soft-degrade 词形退役：分配失败不得继续引导（继续引导 = 陈旧
    // capsule 复活窗口）。词形经拼接防自咬。
    let word = ["soft", " ", "degrade"].concat();
    assert!(
        !MAIN_SRC.contains(&word),
        "capsule 分配失败 soft-degrade 词形复活（WP-77.2.1 退役）"
    );
    let alloc_fail_word = ["identity", "-capsule-alloc-fail"].concat();
    assert!(
        !MAIN_SRC.contains(&alloc_fail_word),
        "capsule 分配失败的『继续引导』诊断行复活（现形态 = 引导失败）"
    );
}

#[test]
fn structured_rejection_code_present() {
    // 新失败码 + 结构化拒绝词（main 的 boot-rejection JSON 消费
    // `error.code()`）——QEMU 注入窗负例判据锚。
    assert!(
        MAIN_SRC.contains("IdentityCapsule"),
        "BootFailure::IdentityCapsule 变体缺失"
    );
    let code = ["\"", "IDENTITY_CAPSULE", "\""].concat();
    assert!(
        MAIN_SRC.contains(&code),
        "IDENTITY_CAPSULE 结构化拒绝码字面缺失"
    );
}

#[test]
fn fault_inject_is_optin_feature() {
    // 注入面与 release 构建互斥：feature 仅 image_factory 显式 opt-in
    //（ENEROS_BOOT_MANAGER_FAULT_INJECT=1）；Cargo.toml 声明存在且
    // main.rs 的注入代码全部 cfg 门内。
    assert!(
        MANIFEST_SRC.contains("fault-inject = [\"uefi-app\"]"),
        "fault-inject feature 声明缺失"
    );
    let cfg_needle = ["cfg(feature = ", "\"fault-inject\"", ")"].concat();
    assert!(
        MAIN_SRC.matches(&cfg_needle).count() >= 3,
        "注入面 cfg 门数量异常（注入代码必须全部 feature 门内）"
    );
    assert!(
        MAIN_SRC.contains("ENEROS_FAULT_INJECT_MODE"),
        "注入模式 env 门缺失（两模式构建期互斥选择）"
    );
}
