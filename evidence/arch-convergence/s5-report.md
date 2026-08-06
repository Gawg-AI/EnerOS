# S5 阶段报告：验证基建与商用证据候选

## 工作包结果

| 工作包 | 结果 | 证据 |
|---|---|---|
| VRF-001 | ✅ 6 解析面 fuzz harness + 120000 次 0 崩溃 | vrf-001.md；libFuzzer 实际执行 BLOCKED（缺 clang/nightly rustc） |
| VRF-002 | ✅ 双构建 bit-identical（sha256 相同） | vrf-002.md；`cargo xtask build-compare` PASS |
| VRF-003 | ✅ CI 门禁 fail-closed（clippy 0 warning + 310 全绿 + g1 job） | vrf-003.md；ci.yml 无 \|\| true |
| VRF-004 | ✅ loom 捕获并修复 ArcHotSwap UAF race | vrf-004.md；loom 1 passed（全调度无 UB） |
| VRF-005 | ✅ QEMU campaign 定义（4 类）+ host 检测面映射 | vrf-005.md；QEMU/HIL/长稳 BLOCKED |
| VRF-006 | ✅ compliance 动态检测 + aiops 正名 | vrf-006.md；27+1 passed |
| VRF-007 | ✅ SBOM/发布验证器 evidence 计算（零硬编码 Passed） | vrf-007.md；执行 BLOCKED（产物依赖） |
| VRF-008 | ✅ 08 缺陷 A/B/C 全量追溯矩阵（28 项） | vrf-008.md |
| VRF-009 | ✅ HIL/制造/密钥预置/试点规格冻结（不实测） | vrf-009.md；7 处 BLOCKED |

## S5 出口判定

- **evidence bundle 完整且机器可复算**：evidence/arch-convergence/ 15+ 文件
  （vrf-001~009 + s4-report + arc-* + krn-*），每项含命令/测试/计数；
- **硬件专属项显式 BLOCKED**：QEMU 运行时（vrf-001/005）、HIL/长稳（vrf-005/009）、
  libFuzzer 执行（vrf-001）、SBOM 产物执行（vrf-007）——全部精确原因 + 解锁条件，
  无静默降级；
- **其余全部 PASS**：host 可复算项（fuzz 崩溃清零/双构建/loom/动态检测/追溯矩阵）
  均有机器输出；
- **明确"商用资格候选"而非发布版**：本阶段未创建任何 stable/v1.0.0 标签；
  S5 出口结论 = 真实性清偿完成 + 运行时闭环（代码层）+ 商用资格候选。

## 全量状态

- `cargo test --workspace`：310 个 test result ok，0 FAILED
- `cargo clippy --workspace --all-targets`：0 warning 0 error
- 所有改动未提交（基线策略，未自动 commit/push）
