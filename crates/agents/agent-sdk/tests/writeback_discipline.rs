//! 蓝图 68 卷 68.1 WP-68.1.1 t7：ChunkedSession 复制计数器违纪形态——
//! 编译期锁（endpoint.rs:131 注释纪律的测试化）。
//!
//! 纪律：同一函数内两个逻辑会话必须**顺序复用同一 `&mut u64`**（obs_report
//! enqueue/flush 形态）；为绕借用检查复制计数器 = 两个会话同时可变借用
//! 同一共享计数器，被借用检查器结构性拒绝（E0499）。trybuild 逐例编译
//! （decision-contract compile_fail 先例）；无 feature 态依赖（chunked
//! 面 host 默认可见），stderr 快照不随 feature 漂移。

#[test]
fn compile_fail_session_counter_copy_fence() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compilefail/chunked_session_counter_double_borrow.rs");
}
