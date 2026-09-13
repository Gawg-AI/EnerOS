//! 蓝图 61 [06] §3.5.1/§3.6：编译失败测试——阶段不可跳跃的类型学证明。
//!
//! trybuild 逐例编译：期望失败案例**必须**编译不过，期望通过案例必须
//! 编译过。构造权收口（[06] §3.3）由此机器化：
//! - 任何外部 crate 都不能凭空构造 `CertifiedPlan`（字段私有 + seal）；
//! - 命令批只从 `CommandBatch::compile` 出（持 `&CertifiedPlan`）——
//!   蓝图 65 WP-65.2 起直填死默认的免检构造门不复存在；
//! - `CandidatePlan` 不能直接交给"执行"（execute 只接受 AuthorizedCommand
//!   ——候选类型没有该签名，传参即编译失败）。

/// feature 态门控：trybuild stderr 快照依赖 feature 状态（workspace
/// feature 合一恒含 `certifier`——control-dispatch 依赖 eneros-certifier
/// 验证面）。快照在该状态下生成并锁定；无 feature 态的构造收口由 lib
/// 单测（decode_rejects_version_and_layout + 源码审计）覆盖，发放面
/// 唯一性另由 certifier crate 的源码扫描门执法。
#[test]
#[cfg(feature = "certifier")]
fn compile_fail_stage_safety() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compilefail/*.rs");
}

#[test]
#[cfg(feature = "certifier")]
fn compile_pass_sealed_api_usable() {
    let t = trybuild::TestCases::new();
    t.pass("tests/compilepass/*.rs");
}
