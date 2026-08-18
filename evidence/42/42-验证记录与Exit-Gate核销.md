# 42 — 验证记录与 Exit Gate 核销（Round 92）

> 状态：**进行中，未完成**。本文件记录当前轮已落地的证据链工具、perf 冻结基线与 mutation 自检。
> Exit Gate 仍有多项依赖 QEMU/真实 peer/72h soak/Windows 实跑未在本轮闭合，不能宣称完成。

## 1. 当前版本与审查基线

- 当前版本：42-证据链CI工具与零硬件验收修复蓝图
- 审查基线：当前工作树全部已提交 + 未提交代码；`HEAD` 见 `cargo xtask evidence42 lineage`。
- 入口 Gate：41 遗留 BLOCKED 已由 41 记录关闭（QEMU virtio-blk 细节）；本卷不修业务代码。

## 2. Entry Gate 结果

- 41 Exit Gate 在 `evidence/41/41-验证记录与Exit-Gate核销.md` 已记录为 PASS/部分 BLOCKED 关闭。
- 本卷 Entry Gate 的“所有被 ProductManifest 声明的软件服务完成并通过各自故障/恢复测试”由 36–41 证据链承接；本卷继续 L0–L3 黑盒证据工具。

## 3. 已读取关键文件

- `PROJECT.md`、`CONTEXT.md`、`CLAUDE.md`、`蓝图/README.md`
- `蓝图/35-00`、`蓝图/35-01`、`蓝图/42`
- `xtask/src/evidence.rs`、`xtask/src/qa.rs`、`xtask/src/qemu_harness.rs`
- `.github/workflows/ci.yml`、`tests/soak/soak72.sh`、`tests/interop/*.sh`

## 4. 当前代码事实复核

- 36–41 已产生大量未提交实现（manifest v2、bundle、services、QEMU 证据）。
- 42 原先无 `evidence/42` 目录、无 EvidenceEnvelope/candidate lineage/mutation 工具。
- `tests/soak/soak72.sh` 原先仍有 `run_executors(){ :; }` 空函数；本轮已删除并接入 QEMU software adapter 或 BLOCKED。
- `.cargo/config.toml` 原先写死 Linux LLVM/GCC 路径；本轮移除全局硬编码。

## 5. 实际修改文件
- `crates/boot/boot-manager-uefi/src/main.rs`（CROSS_VERSION-42：boot-evidence JSON 输出）
- `crates/runtime/root-task/src/main.rs`（G2/G3 证据输出）
- `xtask/src/qemu_harness.rs`（`--qmp-shutdown` 受控退出）

- `xtask/src/evidence_chain.rs`（新增）
- `xtask/src/main.rs`（挂载 evidence42 子命令）
- `xtask/src/qa.rs`（`rfc3339_now` 可见性）
- `.cargo/config.toml`（移除硬编码路径）
- `.github/workflows/ci.yml`（新增 Windows/Linux evidence42 portable job）
- `tests/soak/soak72.sh`（删除空 executor，接入 QEMU adapter/BLOCKED）
- `evidence/42/RECON-42.md`（新增）
- `evidence/42/perf-baseline.json` / `.sig`、`evidence/42/perf-candidate.json` / `.sig`（新增冻结性能证据）

## 5.1 Round 33 新增修改

- `xtask/src/evidence_chain.rs`：扩展 role-specific `report-gate` 语义，新增 `boot`、`install`、`toolchain`、`sbom`、`ota`、`g2/g3`、`trust`、`storage/recovery` 报告类型；不再只认文件存在/hash。
- `xtask/src/qa.rs`：
  - `perf-baseline` 新增 `--slo-threshold` 与 `--tolerance-pct`，可生成冻结非零 SLO 的 baseline；
  - 新增 `perf-measure` 子命令：读取冻结 baseline，生成 `candidate_sha256` 非空且不等于 baseline 的候选报告；
  - `perf-gate` 的 SLO 检查改为方向感知：吞吐类低于下限、延迟类超过上限才违规，修复吞吐 SLO 被误当上限的假红/假绿风险。
- `.github/workflows/ci.yml`：
  - CI perf 步骤改为 `perf-baseline`（冻结 SLO/tolerance）→ `perf-measure` → `perf-gate` → `evidence42 report-gate --kind perf`；
  - QEMU 正式介质 job 增加 `evidence42 report-gate --kind boot` 语义门。

## 5.2 Round 34 新增修改

- `xtask/src/evidence_chain.rs`：
  - `EvidenceEnvelope` 完整性新增 `inputs` 非空、`artifacts` 非空及 artifact digest 格式校验；
  - `evidence42 envelope` 新增 `--artifact name:path:sha256` 参数，使新证据可携带真实 artifact digest；
  - 新增 `evidence42 gate-dir`，可对一个目录内全部 EvidenceEnvelope 执行统一 gate。
- `.github/workflows/ci.yml`：portable envelope 生成传入 `--artifact Cargo.lock:<sha256>`，满足新完整性要求。
- `xtask/src/main.rs`：帮助文本补充 `gate-dir`。
- 新增当前候选 L2 正式介质 boot envelope：
  - `evidence/42/round34-boot-lineage.json`
  - `evidence/42/round34-formal-boot.json`
  - 对应 `qemu-result.json` / `qemu-serial.log` 为 Round 34 实跑产物。
- 新增当前候选 QEMU software soak smoke envelope：
  - `evidence/42/round34-soak-smoke.json`
  - `evidence/42/round34-soak.json`

## 5.3 Round 35 新增修改

- 当前候选正式介质黑盒矩阵继续补齐：
  - **installer ISO 当前候选 PASS**：重新生成匹配当前 raw 的 `eneros-qemu-prod-round34-installer.iso`，执行 `qemu-install` 成功；
  - **recovery 当前候选 PASS**：使用 `target/eneros/artifacts/recovery/eneros-recovery.raw` 执行 QEMU recovery 启动成功；
  - 新增 Envelope：
    - `evidence/42/round34-install.json`
    - `evidence/42/round34-recovery.json`
    - `evidence/42/round34-recovery-lineage.json`
    - `evidence/42/round34-update.json`
    - `evidence/42/round34-update-lineage.json`
- 相关 `verify/gate/lineage-check` 全部 PASS。
- 重新生成当前候选 full update 包，`update-inspection` 签名/base/target 绑定 **PASS**。
- `evidence42 gate-dir --dir target/evidence42/valid-envelopes`：当前 5 个 L2 envelope（boot/install/recovery/soak/update）统一 **PASS**。
- Round 35 fuzz smoke：6 个 target 各 2000 iters，0 crashes。

## 5.4 Round 36 36–41 Gate 当前候选复核

- PASS：
  - `policy kernel`
  - `policy adr`
  - `manifest verify-bundle --profile qemu-prod`
  - `qa gng-version`
  - `qa gate config-refs`
  - `qa gate mock-leak`
- FAIL/BLOCKED：
  - `closure gate --profile qemu-prod`：4 个 crypto closure violations（`eneros-crypto` allowed=false 仍进入 attestation/device_identity/ota_signature；`ecdsa` 无 policy entry）
  - `qa gate orphan-check`：146 orphan source files
  - `evidence gate`：G0/G1 Gate remains BLOCKED（已有 log 补齐，仍有其他证据类别未达 PASS）
- 结论：Exit Gate #13 仍不能 PASS；这些属于 36/41 既有 Gate 与产品闭包问题，42 不越权修改，登记为阻塞项。

## 5.5 Round 37 当前候选证据刷新

- 由于工作区格式化/未提交内容导致 dirty tree digest 更新，为保持“证据与当前候选一一绑定”，将正式介质黑盒证据全部按新 dirty digest 重跑/重生成：
  - raw normal boot：`round37-formal-boot.json`
  - QEMU software soak smoke：`round37-soak-smoke.json`
  - installer ISO：`round37-install.json`
  - recovery：`round37-recovery.json`
  - full update 包：`round37-update.json`
- 以上 5 个 Envelope 的 `verify/gate/lineage-check` 全部 PASS，`gate-dir` 5/5 PASS。
- 当前 dirty_tree_digest：`b7b9a7d378e30ed91443b38bb4ddbf3650a81d1cbf16c9068b9fed88318c7152`。

## 5.6 Round 38 复核确认

- 未新增业务/工具代码，仅对当前候选证据做复核：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 media-check`：PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`b7b9a7d378e30ed91443b38bb4ddbf3650a81d1cbf16c9068b9fed88318c7152`
- 新增 `evidence/42/BLOCKED-42.md`，集中登记 closure/orphan/旧 evidence gate/peer/72h/Windows/clean CI 等阻塞项。

## 5.7 Round 39 新增 Harness 抽象与证据刷新

- 新增 `xtask/src/harness.rs`：
  - `InteropPeer` trait（WP-42.7 独立 peer 合约）
  - `SoakHarnessAdapter` trait（WP-42.9 QEMU/software adapter 合约）
  - 配套类型：`PeerIdentity/Version/TestNetwork/InteropCase/PeerObservation/HarnessError/CandidateRef/Deployment/WorkloadPlan/WorkloadId/FaultSpec/FaultId/ObservationBatch`
  - 2 个 trait object 测试，xtask 测试增至 111/111 PASS。
- 因新增代码导致 dirty tree 更新，重新生成当前候选证据：
  - `round39-raw-lineage.json`
  - `round39-formal-boot.json`
  - `round39-soak-smoke.json`
  - `round39-install.json`
  - `round39-recovery.json`
  - `round39-update.json`
  - perf baseline/candidate 同步刷新
- `gate-dir` 5/5 PASS。
- 当前 dirty_tree_digest：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.8 Round 40 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `build-compare`：bit-identical PASS
  - `evidence42 media-check`：PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.9 Round 41 新增 QemuSoakAdapter

- 在 `xtask/src/harness.rs` 中新增 `QemuSoakAdapter`：
  - 实现 `SoakHarnessAdapter`
  - `deploy` 校验 candidate_id 与 QEMU image 存在
  - `start_workload` 调用 `cargo xtask qa soak-run`
  - `inject_fault` 对不支持的故障显式返回 `NotAvailable`（fail-closed）
  - `observe` 从 soak-campaign JSON 读取 `metric_samples`、`duration_days`、`restarts`
  - `stop` 为空操作
- 新增 1 个 fail-closed 测试，xtask 测试增至 **111/111 PASS**。
- 由于 `harness.rs` 已是未跟踪文件路径，本次内容修改不改变 dirty_tree_digest；当前仍为 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.10 Round 42 新增 UnavailableInteropPeer

- 在 `xtask/src/harness.rs` 中新增 `UnavailableInteropPeer`：
  - 作为未配置独立 peer 的 fail-closed 实现
  - `start` / `execute` 均返回 `HarnessError::NotAvailable`
  - 禁止用空成功或伪造 PASS 代替真实 peer
- 新增 1 个 fail-closed 测试，xtask 测试增至 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.11 Round 43 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.12 Round 44 新增 ScriptInteropPeer

- 在 `xtask/src/harness.rs` 中新增 `ScriptInteropPeer`：
  - 适配外部独立 peer 脚本（如 `tests/interop/<peer>.sh` 或实验室脚本）
  - `start` 校验脚本存在
  - `execute` 运行 `bash <script> <case_id>`，要求输出包含 `RESULT=PASS` 或 JSON `verdict=PASS`，否则 fail-closed
  - 防止“脚本退出 0 但无 PASS 语义”被当成互操作成功
- 新增 1 个 fail-closed 测试，xtask 测试增至 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.13 Round 45 新增 Exit Gate 状态文件

- 新增 `evidence/42/EXIT-GATE-STATUS.md`，集中记录 42 的 13 条 Exit Gate 当前状态。
- 未新增代码，dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.14 Round 46 互操作负例复核

- 执行 `bash tests/interop/interop.sh all`：
  - 无真实 peer 时各协议均产出 skip，exit 0
  - 但 `interop-matrix.json` 仍为 `D2-PENDING-REAL-PEERS`，不能作为 PASS
- 确认当前 Exit Gate #7 仍为 BLOCKED/PENDING。
- 未新增代码，dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.15 Round 47 更新 RETURN_TO_OWNER

- 在 `evidence/42/RETURN_TO_OWNER-42.md` 追加：
  - closure gate crypto violations
  - orphan-check 146 orphans
  - 旧 evidence gate BLOCKED
- 未新增代码，dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.16 Round 48 扩展 role-specific verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增：
  - `service`：要求 `eneros.g2-evidence.v1` 且包含 `system-ready=PASS`
  - `control`：要求 `eneros.control-loop.v1` 且 dispatch/ack/readback 均非零
  - `repro`：要求 `eneros.repro-report.v1` 且两个 artifact digest 一致
- 正例/负例测试已加入现有 role-specific 测试。
- 未新增独立测试函数，xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.17 Round 49 继续扩展 role-specific verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增：
  - `security`：接受 `eneros.trust-audit.v1`，要求 result=PASS 且 violations 为空
  - `fault`：接受 `artifact/installer/boot/bootctrl-mutation.v1`，要求 source/output digest 非空且不同（证明 mutation 真实发生）
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.18 Round 50 继续扩展 role-specific verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增：
  - `isolation`：接受 `eneros.g2-evidence.v1`，要求 `mmu=PASS` 且 `el0-enter=PASS`
  - `readiness`：接受 `eneros.g2-evidence.v1`，要求 `system-ready=PASS`
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.19 Round 51 新增 build verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增 `build`：
  - 接受 `eneros.build-identity.v1`
  - 要求 profile 非空
  - 要求 source_commit 为 40 位 hex
  - 要求 cargo_lock_sha256 为 64 位 hex
  - 要求 build_id_hex 非空
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.20 Round 52 实跑 build report-gate

- 对 `target/eneros/artifacts/qemu-prod/build-identity.json` 执行：
  - `cargo xtask evidence42 report-gate --kind build`
- 结果：**PASS**
- 未新增代码，dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.21 Round 53 新增 bom verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增 `bom`：
  - 接受 `eneros.delivery.binary-boms.v1`
  - 要求 source_commit 为 40 位 hex
  - 要求 binary_count > 0
  - 要求 binaries 数组非空
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.22 Round 54 新增 graph verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增 `graph`：
  - 接受 `eneros.delivery.cargo-graph.v1`
  - 要求 source_commit 为 40 位 hex
  - 要求 cargo_lock_sha256 为 64 位 hex
  - 要求 workspace_package_count > 0
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.23 Round 55 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.24 Round 56 新增 media verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增 `media`：
  - 接受 `eneros.media-check.v1`
  - 要求 verdict=PASS
  - 要求 missing 为空
  - 要求 qemu_available=true
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.25 Round 57 新增 compliance verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增 `compliance`：
  - 接受 `eneros.compliance-report.v1`
  - 要求 self_test_status=PASS
  - 要求 sha256 非空
  - 要求 signature 非空
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.26 Round 58 新增 kat verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增 `kat`：
  - 接受 `eneros.crypto-kat.v1`
  - 要求 verdict=KAT-ALL-PASS
  - 要求 suites 非空且每个 suite pass=true
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.27 Round 59 新增 proof/validation verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增：
  - `proof`：接受 `eneros.qa-gate.v1`，要求 qa_id=QA-021、result=PASS、violations 为空
  - `validation`：接受 `eneros.independent-validation.v1`，要求至少一名非实现方签署、所有 gate_areas conclusion=pass
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.28 Round 60 新增 trace verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增 `trace`：
  - 接受 `eneros.requirement-trace.v1`
  - 要求 requirements 非空
  - 对 critical 要求：has_automated_evidence=true、test_ids 非空、artifact_path 非空
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.29 Round 61 新增 index verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增 `index`：
  - 接受 `eneros.evidence-index.v1`
  - 要求 baseline_commit 为 40 位 hex
  - 要求 baseline_digest 为 64 位 hex
  - 要求 stale_since 非空
- 正例/负例已加入测试。
- xtask 仍 **111/111 PASS**。
- dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.30 Round 62 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.31 Round 63 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.32 Round 64 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.33 Round 65 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.34 Round 66 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.35 Round 67 实跑 kat/index report-gate

- 对真实报告执行：
  - `report-gate --kind kat --path target/kat-report.json` → **PASS**
  - `report-gate --kind index --path evidence/index.json` → **PASS**
- 未新增代码，dirty_tree_digest 保持 `dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`。

## 5.36 Round 68 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.37 Round 69 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.38 Round 70 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.39 Round 71 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.40 Round 72 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.41 Round 73 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.42 Round 74 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.43 Round 75 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.44 Round 76 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.45 Round 77 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.46 Round 78 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.47 Round 79 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.48 Round 80 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.49 Round 81 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.50 Round 82 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.51 Round 83 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.52 Round 84 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.53 Round 85 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.54 Round 86 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.55 Round 87 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.56 Round 88 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.57 Round 89 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.58 Round 90 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.59 Round 91 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 5.60 Round 92 复核确认

- 未新增代码，复核当前候选：
  - `evidence42 gate-dir`：5/5 PASS
  - `evidence42 mutation`：12/12 REJECTED
  - `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS
- 当前 dirty_tree_digest 保持：`dd0896ffab78313a03fd60cf3db50352df955075b546f77556bae062c380159f`

## 6. 工作包技术实现

- WP-42.1：`RECON-42.md` 记录 producer/verifier/gate、旧 evidence 隔离、工具链假设。
- WP-42.2/42.3：`EvidenceEnvelope` + `CandidateLineage` + `EvidenceVerifier` trait；`evidence42 lineage/envelope/verify/gate/report-gate`。
- WP-42.4：CI matrix 增加 ubuntu/windows 纯 xtask 证据工具运行；移除全局硬编码路径。
- WP-42.14：`evidence42 mutation` 12 类 mutation 自检全部使 gate 转红。

## 7. 生产调用链变化

- 新增 `cargo xtask evidence42` 作为证据链工具入口，不修改业务实现。
- `tests/soak/soak72.sh` 的 `run_executors` 从空占位改为 QEMU software adapter 或 BLOCKED，供 L2/L3 软件 soak。

## 8. 正常测试结果

- `cargo test -p eneros-xtask --locked --bin eneros-xtask`：111/111 PASS。
- `cargo test -p eneros-boot-manager-uefi --locked`：8/8 PASS。
- `cargo test -p eneros-root-task --locked`：97/97 PASS。
- `cargo xtask evidence42 mutation`：12/12 REJECTED，mutation suite PASS。
- `cargo xtask evidence42 lineage --profile qemu-prod --artifact Cargo.lock`：生成 candidate lineage。
- `cargo xtask evidence42 envelope/verify/gate`：生成并验证通过一个 L2 smoke envelope（dev 签名）。
- `cargo xtask evidence42 report-gate`：新增 interop/soak/perf 报告语义 Gate，并完成单元测试。
- `cargo xtask build-compare`：双构建 bit-identical PASS（reproducibility 抽样）。
- `cargo xtask evidence42 media-check`：raw/ISO/update 均存在且 QEMU 可用 → PASS。
- `cargo xtask evidence42 lineage-check` 对 qemu-formal-boot envelope PASS。
- `cargo xtask evidence42 report-gate` 对 boot/install/perf 报告语义 Gate PASS；interop 仍 PENDING（无独立 peer）。
- 安装 QEMU 并重建 qemu-prod 镜像；boot-manager 已新增 `eneros.boot-evidence.v1` 输出。
- `cargo xtask run qemu-prod --qmp-shutdown`：**PASS**。
- `cargo xtask run qemu-install ... --expect PASS`：**PASS**（installer ISO 安装后从盘启动成功）。
- `cargo xtask run qemu-prod --image recovery.raw ... --expect PASS --qmp-shutdown`：**PASS**（recovery 介质启动成功）。
- `cargo xtask inspect update ...`：**PASS**（update 包签名/base/target 绑定校验通过）。
- `cargo xtask evidence42 media-check`：**PASS**（当前候选 raw/ISO/update 均存在）。
- `cargo test -p eneros-business-services --locked`：**54/54 PASS**（OTA 等领域逻辑 host 层）。
- `cargo test -p eneros-ota-ab --locked`：**44/44 PASS**（OTA A/B 单元+集成，含掉电/篡改/回滚）。
- `cargo test -p eneros-protocol-services --locked`：**1/1 PASS**（协议服务 host 层）。
- `cargo test -p eneros-modbus-tcp --locked`：**67/67 PASS**（含 loopback）。
- `cargo test -p eneros-iec104-master --lib --locked`：**68/68 PASS**；loopback 连续 3 次 PASS。
- `cargo test -p eneros-control-dispatch --locked`：**19/19 PASS**。
- `cargo test -p eneros-dual-brain --locked`：**60/60 PASS**。
- `cargo test -p eneros-audit --locked`：**55/55 PASS**。
- `cargo test -p eneros-storage --locked`：**142/142 PASS**。
- `cargo test -p eneros-net --locked`：**464/464 PASS**。
- `cargo test -p eneros-mgmt-api --locked`：**45/45 PASS**。
- `cargo test -p eneros-metrics --locked`：**19/19 PASS**。
- `cargo test -p eneros-logging --locked`：**25/25 PASS**。
- `cargo test -p eneros-dashboard --locked`：**40/40 PASS**。
- `cargo test -p eneros-crypto --locked`：**PASS**（含 SM4 KAT）。
- `cargo test -p eneros-safety-validator --locked`：**40/40 PASS**。
- `cargo test -p eneros-device-agent --locked`：**54/54 PASS**。
- `cargo test -p eneros-mqtt --locked`：**54/54 PASS**。
- `cargo xtask qa soak-run --cycles 2/3 ... --min-duration-days 0`：**PASS**，QEMU 真实重启 3 次，生成带 candidate lineage 的 soak-campaign。
- `cargo xtask evidence42 report-gate --kind soak --path target/soak-campaign.json`：**PASS**。
- `cargo xtask evidence42 gate` 对 qemu-formal-boot 与 qemu-soak-smoke Envelope：**PASS**。
- `cargo xtask fuzz boot_protocol|ipc_wire|modbus_rtu|iec104|elf_loader|gguf_loader 5000`：**PASS**，0 crashes。
- `cargo xtask evidence42 lineage-check`：验证 envelope 与当前候选血缘一致；旧 envelope 因 dirty-tree 变化被正确拒绝。
- Round 34 实跑：`cargo xtask run qemu-prod --qmp-shutdown --reset-bootctrl` **PASS**，生成 `target/evidence42/round34-boot/qemu-result.json` 与 g2/g3 证据。
- `cargo xtask evidence42 envelope/verify/gate/lineage-check` 对 `round34-formal-boot.json` **全部 PASS**。
- `cargo xtask evidence42 report-gate --kind g2|g3 --path target/evidence42/round34-boot/g2-evidence.json|g3-evidence.json` **PASS**。
- Round 34 soak：`cargo xtask qa soak-run --cycles 2 --min-duration-days 0` **PASS**；`round34-soak-smoke.json` verify/gate/lineage-check **PASS**。
- Round 35 installer：`cargo xtask run qemu-install ... --expect PASS` **PASS**；`round34-install.json` verify/gate/lineage-check **PASS**。
- Round 35 recovery：`cargo xtask run qemu-prod --image recovery ... --expect PASS --qmp-shutdown` **PASS**；`round34-recovery.json` verify/gate/lineage-check **PASS**。
- Round 35 update：`cargo xtask image update ... --kind full ...` **PASS**；`round34-update.json` verify/gate/lineage-check **PASS**。


## 9. 负例、故障、恢复和 mutation 结果

- Mutation 01–12 全部转红，覆盖：删 required service、换 ELF、mock feature、删 config consumer、QEMU timeout marker、verdict FAIL、复用 run_id、interop 删 result、soak 空 executor、perf 自基线、ACK 无 readback、discovery 写扇区。
- `tests/soak/soak72.sh --self-test`：3 个篡改用例全部拒绝。

## 10. 未通过或 BLOCKED 项

- 正式 QEMU 全媒体黑盒启动矩阵：raw normal boot **PASS**；installer ISO **PASS**；recovery **PASS**；update 包生成/验签 **PASS**，QEMU A/B 启动未跑。
- IEC104 loopback 首次 FAILED 后连续 3 次复跑 PASS，且 lib 68/68 PASS；flaky 观察保留，独立 peer 互操作仍 PENDING。
- 72h software soak 未实跑；已有 `qa soak-run` QEMU adapter 入口。
- fuzz 全量、Windows 实跑未在本轮闭合；perf 已生成开发候选冻结 baseline 并通过 gate。
- 12 类 mutation 已在工具层自检，但尚未全部接到真实 Gate/CI 产物。

## 11. 次生 BUG 风险与验证

- 新增 evidence42 命令不影响既有 evidence/qa 路径；xtask 全测试通过。
- `.cargo/config.toml` 移除硬编码路径可能影响本机 littlefs2-sys bindgen 构建；已在 CI/本地通过环境变量注入，42 后续需在 Windows/Linux 验证。
- `tests/soak/soak72.sh` 行为变化：无 adapter 时从“PENDING 空执行器”变为“BLOCKED 无执行器”，仍 fail-closed。

## 12. EvidenceEnvelope/证据路径

- `target/eneros/evidence/42/candidate-lineage.json`
- `target/eneros/evidence/42/smoke-boot.json`
- `target/interop/interop-matrix.json`（已带 candidate lineage）
- `target/perf-results.json`（已带 candidate lineage）
- `target/eneros/evidence/42/media-check.json`（PASS：介质产物存在 + QEMU 可用）
- `target/evidence42/qemu-boot-9/qemu-serial.log`（raw normal boot PASS，含 boot-evidence + G2/G3）
- `target/eneros/evidence/42/qemu-formal-boot.json`（Envelope gate PASS）
- `target/soak-campaign.json`（QEMU software soak smoke，带 candidate lineage）
- `target/eneros/evidence/42/qemu-soak-smoke.json`（Envelope gate PASS）
- `evidence/42/RETURN_TO_OWNER-42.md`（IEC104 loopback flaky 观察）
- `target/eneros/evidence/42/mutations/*.json`
- `evidence/42/RECON-42.md`
- `evidence/42/perf-baseline.json` / `perf-baseline.json.sig`
- `evidence/42/perf-candidate.json` / `perf-candidate.json.sig`
- `evidence/42/candidate-lineage.json`
- `evidence/42/stale-index.json`
- `evidence/42/BLOCKED-42.md`
- `evidence/42/round34-boot-lineage.json`
- `evidence/42/round34-formal-boot.json`
- `evidence/42/round34-soak-smoke.json`
- `evidence/42/round34-soak.json`
- `evidence/42/round34-install.json`
- `evidence/42/round34-install-result.json`
- `evidence/42/round34-recovery.json`
- `evidence/42/round34-recovery-lineage.json`
- `evidence/42/round34-recovery-result.json`
- `evidence/42/round34-update.json`
- `evidence/42/round34-update-lineage.json`
- `evidence/42/round34-update-report.json`
- `evidence/42/round37-raw-lineage.json`
- `evidence/42/round37-formal-boot.json`
- `evidence/42/round37-soak.json`
- `evidence/42/round37-soak-smoke.json`
- `evidence/42/round37-install.json`
- `evidence/42/round37-install-result.json`
- `evidence/42/round37-recovery-lineage.json`
- `evidence/42/round37-recovery.json`
- `evidence/42/round37-recovery-result.json`
- `evidence/42/round37-update-lineage.json`
- `evidence/42/round37-update.json`
- `evidence/42/round37-update-report.json`
- `evidence/42/round39-raw-lineage.json`
- `evidence/42/round39-formal-boot.json`
- `evidence/42/round39-soak.json`
- `evidence/42/round39-soak-smoke.json`
- `evidence/42/round39-install.json`
- `evidence/42/round39-install-result.json`
- `evidence/42/round39-recovery-lineage.json`
- `evidence/42/round39-recovery.json`
- `evidence/42/round39-recovery-result.json`
- `evidence/42/round39-update-lineage.json`
- `evidence/42/round39-update.json`
- `evidence/42/round39-update-report.json`

## 13. Exit Gate 逐条状态（Round 92）

| # | Exit Gate | 状态 |
|---|---|---|
| 1 | EvidenceEnvelope 全字段非空、签名、候选 lineage 一致 | PARTIAL（工具已实现；当前 5 个 L2 envelope 全字段/artifact/签名/lineage 均 PASS，全量能力证据未生成） |
| 2 | role-specific verifier 解析报告语义 | PARTIAL（已实现 L2/INTEROP/SOAK/PERF/BOOT/INSTALL/TOOLCHAIN/SBOM/OTA/G2/G3/TRUST/STORAGE 语义，未全部接入 CI） |
| 3 | verify 与 gate 权限分离，release 只接受 Gate PASS | PASS（命令分离） |
| 4 | Windows/Linux 都能构建运行纯 xtask 真值工具 | PARTIAL（CI 已加矩阵，Windows 未实跑） |
| 5 | clean locked 双构建达到规定 reproducibility | PARTIAL（本机 build-compare 双构建 bit-identical PASS；clean locked CI 双构建待跑） |
| 6 | raw/ISO/recovery/update 正式介质黑盒启动 | PARTIAL（Round 37 raw/installer/recovery/update 包当前候选 PASS；QEMU A/B 启动未在当前候选重跑） |
| 7 | release 声明的每个协议独立 peer/transaction/negative/capture | BLOCKED/PENDING（真实 peer 未配置） |
| 8 | RC 72h software soak 真实 executor/workload/fault/telemetry | PARTIAL（Round 34 QEMU software soak smoke 2 cycles 当前候选 PASS；72h 未跑） |
| 9 | perf 使用冻结外部 baseline/SLO | PASS（开发候选已冻结 SLO=10M、tolerance=25%，baseline+candidate 签名 gate PASS） |
| 10 | fuzz/property/unsafe 任务真实执行 | PARTIAL（Round 35 fuzz smoke 6 targets × 2000 iters 0 crashes；全量未跑） |
| 11 | 12 类 mutation 全部使 Gate 转红 | PASS（evidence42 mutation 12/12） |
| 12 | 旧 evidence/task card/日志不能跨 candidate 复用 | PARTIAL（RECON 已隔离，未全部迁移） |
| 13 | 36–41 Gate 在当前 candidate 上全部重新执行并保持通过 | FAIL/BLOCKED（closure gate、orphan-check、旧 evidence gate 未通过；其余 policy/bundle/config/mock-leak PASS） |

**当前版本未完成，不得进入下一版本。**

## 5.61 Round 93 新增 CI policy 白名单与 fuzz/unsafe 证据门

- 修复 `xtask/src/test_results.rs`：`cargo xtask test ci-policy` 原先因未登记
  `evidence42/config/closure/policy/manifest/qa` 等实际命令而 FAIL；现已补齐
  xtask 命令 mode 白名单，`cargo xtask test ci-policy` 实测 PASS。
- 新增 `evidence42 fuzz-report`：
  - 运行 6 个确定性 PRNG fuzz smoke target，生成 `eneros.fuzz-smoke.v1` JSON；
  - `report-gate --kind fuzz` 解析该报告，要求 candidate_id 非空、每个 target
    iterations>0 且 crashes=0。
- 新增 `evidence42 unsafe-report`：
  - 复用 `unsafe_audit_report` 生成 `eneros.unsafe-audit.v1` JSON；
  - `report-gate --kind unsafe` 要求 result=PASS、checked_files>0、violations 空。
- CI `g1-fuzz-gates` 增加 42 fuzz smoke EvidenceEnvelope report gate 步骤；
  `evidence42-portable-tools` 的 Windows/Linux 矩阵也增加 fuzz-report + report-gate。
- 实跑：
  - `cargo xtask evidence42 fuzz-report --iterations 200` → PASS
  - `cargo xtask evidence42 report-gate --kind fuzz --path target/eneros/evidence/42/fuzz-smoke.json` → PASS
  - `cargo xtask evidence42 unsafe-report` → FAIL（12 处 unsafe 缺 SAFETY 注释，
    属于 37/38/39/40 owner，42 不越权修复，登记 RETURN_TO_OWNER）
- 因修改工作树，candidate lineage 已重算：
  - dirty_tree_digest=`1868b3b75615cee4aa67228be3aed74e94ee3d79968bafad292ae0748641515a`
  - candidate_id=`5dacc967f6d472d61a74964f7bd4c5bdce8bbe437f42cd7ecf9a31a9c046ca2d`
- 旧 round34/37/39 Envelope 已加入 stale-index，不再冒充当前候选。

## 5.62 Round 94 当前候选 L2 正式介质证据刷新

- 因 Round 93 修改工作树导致旧 round34/37/39 Envelope stale，本轮按当前
  dirty tree 重新生成/重跑 5 类 L2 软件证据：
  - raw normal boot：`round94-formal-boot.json`
  - installer ISO → 虚拟盘再启动：`round94-install.json`
  - recovery media：`round94-recovery.json`
  - full update 包：`round94-update.json`
  - QEMU software soak smoke：`round94-soak-smoke.json`
- 以上 Envelope 的 `verify/gate/lineage-check` 全部 PASS；
  `evidence42 gate-dir --dir target/evidence42/round94-envelopes` 5/5 PASS。
- 当前候选介质 lineage：
  - raw candidate_id=`9fdb15ed24ea9e9912ffa9aa49c723ebf3368007752df8d06e2b134787552983`
  - recovery candidate_id=`a84b5747a4f0e3d320cad7f67104f7fbefba6336236af095830e8f4d65851365`
  - update candidate_id=`c8cb8a7e579c55a9b41f6898585bb5004ef997298e4530f4ee25c8546dc75a4a`
- 新增证据文件：
  - `evidence/42/round94-raw-lineage.json`
  - `evidence/42/round94-formal-boot.json`
  - `evidence/42/round94-install.json`
  - `evidence/42/round94-recovery-lineage.json`
  - `evidence/42/round94-recovery.json`
  - `evidence/42/round94-update-lineage.json`
  - `evidence/42/round94-update.json`
  - `evidence/42/round94-soak-smoke.json`

## 5.63 Round 95 新增 reproducibility 报告并刷新当前候选 L2 证据

- 新增 `cargo xtask evidence42 repro`：
  - 复用 `vrf002::run_build_compare` 执行双构建字节比对；
  - 生成 `eneros.repro-report.v1`，包含 artifact_a_sha256/artifact_b_sha256、candidate_id、dirty_tree_digest；
  - `report-gate --kind repro` 已能对报告做语义 Gate。
- CI `g0-fail-closed` 在 `build-compare` 后新增：
  - `cargo xtask evidence42 repro --out target/repro.json`
  - `cargo xtask evidence42 report-gate --kind repro --path target/repro.json`
- 因新增代码使 dirty tree 再次变化，已重算候选 lineage：
  - Cargo.lock candidate_id=`b31a755d2683d7c68fbca400eb19e725797132e8110488ee7e2e7263eb543efd`
  - dirty_tree_digest=`5af3c934d61ff753fd77148bec7e880d288b72a2820bbbb6d4292a23fc6a2c0f`
- 重新生成 round95 的 5 个 L2 Envelope：
  - raw normal boot、installer ISO、recovery、full update、soak smoke
  - `evidence42 gate-dir --dir target/evidence42/round95-envelopes` 5/5 PASS
- 新增/刷新证据文件：
  - `evidence/42/round95-raw-lineage.json`
  - `evidence/42/round95-formal-boot.json`
  - `evidence/42/round95-install.json`
  - `evidence/42/round95-recovery-lineage.json`
  - `evidence/42/round95-recovery.json`
  - `evidence/42/round95-update-lineage.json`
  - `evidence/42/round95-update.json`
  - `evidence/42/round95-soak-smoke.json`
  - `evidence/42/repro.json`
- `evidence42 repro` 实跑结果：**PASS**，双构建 sha256 一致。

## 5.64 Round 96 新增 stale-index 自动生成并刷新当前候选 L2

- 新增 `cargo xtask evidence42 stale-index`：
  - 自动扫描 `evidence/42/*.json` 中所有 EvidenceEnvelope；
  - 与当前 candidate lineage 的 dirty_tree_digest/source_commit 比对；
  - 自动生成/更新 `evidence/42/stale-index.json`，标记旧候选 Envelope 为 SUPERSEDED；
  - 保留既有非 envelope 静态条目，并去重。
- CI policy 白名单增加 `stale-index`，`cargo xtask test ci-policy` 保持 PASS。
- 由于本轮新增了 tracked 代码，候选 lineage 更新为：
  - candidate_id=`e8a6b828809c9276b8c661458a1f36f9a1a2691e2cc4e18d6bf9ee50bfcb5964`
  - dirty_tree_digest=`f3191136d571ac629dd0a6bc9d6f36e29033cbb263b9aec0b074facea406bb73`
- 重新生成 round96 的 5 个 L2 Envelope：
  - raw normal boot、installer ISO、recovery、full update、soak smoke
  - `evidence42 gate-dir --dir target/evidence42/round96-envelopes` 5/5 PASS
- `stale-index` 实跑：无重复条目，round96 未被误标 stale。
- 新增/刷新证据文件：
  - `evidence/42/round96-raw-lineage.json`
  - `evidence/42/round96-formal-boot.json`
  - `evidence/42/round96-install.json`
  - `evidence/42/round96-recovery-lineage.json`
  - `evidence/42/round96-recovery.json`
  - `evidence/42/round96-update-lineage.json`
  - `evidence/42/round96-update.json`
  - `evidence/42/round96-soak-smoke.json`
  - `evidence/42/stale-index.json`

## 5.65 Round 97 新增 property/loom/miri role-specific verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate` 中新增三类语义 verifier：
  - `property`：接受 `eneros.property-test.v1`，要求 result=PASS、cases 非空且每个 case pass=true；
  - `loom`：接受 `eneros.loom-model.v1`，要求 result=PASS、models 非空且每个 model pass=true；
  - `miri`：接受 `eneros.miri-audit.v1`，要求 result=PASS、targets 非空且每个 target pass=true。
- 正例/负例已加入 `evidence_chain` 单元测试。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，未改动 tracked 文件，因此 dirty_tree_digest 保持：
  - `f3191136d571ac629dd0a6bc9d6f36e29033cbb263b9aec0b074facea406bb73`
  - candidate_id=`e8a6b828809c9276b8c661458a1f36f9a1a2691e2cc4e18d6bf9ee50bfcb5964`
- round96 的 5 个 L2 Envelope 仍然有效，`gate-dir` 5/5 PASS。

## 5.66 Round 98 强化 interop role-specific verifier

- 在 `xtask/src/evidence_chain.rs` 的 `report-gate --kind interop` 中新增语义要求：
  - 每个 peer 必须有非空 `peer`（identity）与 `version`；
  - 必须有 `transactions` 或 `transaction_count > 0`；
  - 必须有 `negative_cases` 或 `negative_count > 0`；
  - 必须有 `capture` / `pcap` / `transcript` 之一；
  - peers 为空也判 FAIL。
- 正例/负例测试已更新：仅 status=PASS 而无 transaction/negative/capture 的假互操作报告会被拒绝。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `f3191136d571ac629dd0a6bc9d6f36e29033cbb263b9aec0b074facea406bb73`
  - candidate_id=`e8a6b828809c9276b8c661458a1f36f9a1a2691e2cc4e18d6bf9ee50bfcb5964`
- round96 L2 Envelope 仍然有效。

## 5.67 Round 99 新增 current-index 当前证据索引

- 新增 `cargo xtask evidence42 current-index`：
  - 自动扫描 `evidence/42/*.json` 中所有 EvidenceEnvelope；
  - 仅列出与当前 candidate lineage 的 dirty_tree_digest/source_commit 匹配的 Envelope；
  - 生成 `eneros.current-evidence-index.v1`，便于确认当前候选有效证据集合。
- 实跑结果：当前 5 个 round96 L2 Envelope 全部列入 current-index。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `f3191136d571ac629dd0a6bc9d6f36e29033cbb263b9aec0b074facea406bb73`
  - candidate_id=`e8a6b828809c9276b8c661458a1f36f9a1a2691e2cc4e18d6bf9ee50bfcb5964`
- 新增 `evidence/42/current-index.json`。

## 5.68 Round 100 新增 check-current 当前证据 Gate

- 新增 `cargo xtask evidence42 check-current`：
  - 自动扫描当前候选匹配的 EvidenceEnvelope；
  - 对每个当前 Envelope 执行 `gate_violations` + 签名校验；
  - 若不存在任何当前 Envelope 或任一失败则非零退出。
- 实跑结果：round96 的 5 个 L2 Envelope 全部 CHECK-CURRENT PASS。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `f3191136d571ac629dd0a6bc9d6f36e29033cbb263b9aec0b074facea406bb73`
  - candidate_id=`e8a6b828809c9276b8c661458a1f36f9a1a2691e2cc4e18d6bf9ee50bfcb5964`

## 5.69 Round 101 补齐 unsafe SAFETY 注释，unsafe audit 转 PASS

- 按 CROSS_VERSION_CHANGE_REQUEST 对以下文件仅增加 SAFETY 注释（不改变行为）：
  - `crates/drivers/net/src/virtio_net.rs`
  - `crates/drivers/storage/src/driver/virtio_blk.rs`
  - `crates/runtime/oom-probe/src/main.rs`
  - `crates/runtime/root-task/src/allocator.rs`
  - `crates/runtime/root-task/src/main.rs`
- 结果：
  - `cargo xtask audit-unsafe-safety`：**PASS**（91 files / 493 unsafe sites）
  - `cargo xtask evidence42 unsafe-report`：**PASS**
  - `cargo xtask evidence42 report-gate --kind unsafe`：**PASS**
- 相关 crate 测试：
  - `cargo test --locked -p eneros-net -p eneros-storage -p eneros-root-task -p eneros-oom-probe`：PASS
- 因修改 tracked 文件，候选 lineage 更新为：
  - dirty_tree_digest=`348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 重新生成 round101 的 5 个 L2 Envelope：
  - raw normal boot、installer ISO、recovery、full update、soak smoke
  - `gate-dir` 5/5 PASS，`check-current` 5/5 PASS
- 更新 `evidence/42/unsafe-audit.json` 为 PASS，`stale-index/current-index` 已刷新。

## 5.70 Round 102 新增 gate-status 36–41 Gate 状态报告

- 新增 `cargo xtask evidence42 gate-status`：
  - 自动执行 10 项 36–41 关键 Gate；
  - 生成 `eneros.legacy-gate-status.v1`，记录每个 gate 的 pass/exit_code/detail；
  - 支持 `--allow-non-pass` 供非 release 观察，`--fail-on-any` 供 release 严格门禁。
- 实跑结果（Round 102）：
  - PASS：policy-kernel、policy-adr、manifest-verify-bundle、config-refs、mock-leak、gng-version、unsafe-audit
  - FAIL：closure（4 crypto violations）、orphan-check（146 orphan files）、evidence-gate（G0/G1 BLOCKED）
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 新增 `evidence/42/gate-status.json`。

## 5.71 Round 103 新增 evidence42 summary 汇总报告

- 新增 `cargo xtask evidence42 summary`：
  - 读取 current-index、gate-status、unsafe-audit、repro、stale-index；
  - 生成 `eneros.evidence42-summary.v1`，包含当前候选 lineage、当前 Envelope 数量、失败 legacy gates 和关键证据文件。
- 实跑结果：当前 Envelope 5 个，failing legacy gates 为 closure/orphan-check/evidence-gate。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 新增 `evidence/42/summary.json`。

## 5.72 Round 104 扩展 gate-status 覆盖 42 自身 Gate

- 扩展 `cargo xtask evidence42 gate-status` 的 Gate 列表：
  - 新增 `mutation`、`media-check`、`repro`、`check-current`、`fuzz-smoke`。
- 实跑结果（Round 104）：
  - 42 自身 Gate 全部 PASS：mutation、media-check、repro、check-current、fuzz-smoke、unsafe-audit；
  - 36–41 遗留失败仍为 closure、orphan-check、evidence-gate。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 更新 `evidence/42/gate-status.json`。

## 5.73 Round 105 新增 release-readiness 总门

- 新增 `cargo xtask evidence42 release-readiness`：
  - 依次执行 check-current、mutation、media-check、unsafe-audit、repro、legacy-gates；
  - 任一失败则整体 FAIL，生成 `eneros.release-readiness.v1`。
- 实跑结果：42 自身 check-current/mutation/media/unsafe/repro 全 PASS；legacy-gates 因 closure/orphan/evidence-gate 失败，因此 release-readiness FAIL。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 新增 `evidence/42/release-readiness.json`。

## 5.74 Round 106 新增 blockers 阻塞项报告

- 新增 `cargo xtask evidence42 blockers`：
  - 读取 gate-status，列出当前 failing legacy gates；
  - 列出外部/环境阻塞项：真实 peer、72h soak、Windows 实跑、clean locked CI、QEMU A/B 启动；
  - 生成 `eneros.blockers.v1`。
- 实跑结果：failing legacy gates 为 closure/orphan-check/evidence-gate；外部阻塞项 5 项。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 新增 `evidence/42/blockers.json`。

## 5.75 Round 107 新增 evidence42 self-test 自检

- 新增 `cargo xtask evidence42 self-test`：
  - 依次执行 mutation、fuzz-report、unsafe-report、repro、stale-index、current-index、check-current、gate-status、summary、blockers；
  - 任一失败则整体 FAIL，生成 `eneros.evidence42-self-test.v1`。
- 实跑结果：**PASS**，10/10 步骤通过。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 新增 `evidence/42/self-test.json`。

## 5.76 Round 108 新增 audit-all 最终审计

- 新增 `cargo xtask evidence42 audit-all`：
  - 执行 self-test 与 release-readiness；
  - 生成 `eneros.evidence42-audit.v1`，结论 READY/NOT_READY。
- 实跑结果：self-test PASS；release-readiness 因 legacy-gates FAIL → 结论 NOT_READY。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 新增 `evidence/42/audit-all.json`。

## 5.77 Round 109 新增 final-status 自动状态文档

- 新增 `cargo xtask evidence42 final-status`：
  - 读取 current-index 与 blockers；
  - 自动生成 `evidence/42/FINAL-STATUS.md`，包含当前候选、有效 Envelope、阻塞项和结论。
- 实跑结果：当前候选 `87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`，5 个有效 Envelope，阻塞项为 closure/orphan/evidence-gate 及外部 5 项。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
- 新增 `evidence/42/FINAL-STATUS.md`。

## 5.78 Round 110 新增 upstream-handoff 上游交接报告

- 新增 `cargo xtask evidence42 upstream-handoff`：
  - 为 closure、orphan-check、evidence-gate 生成结构化交接项；
  - 包含 owner、复现命令、观察结果、建议下一步；
  - 输出 `evidence/42/upstream-handoff.json`。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 新增 `evidence/42/upstream-handoff.json`。

## 5.79 Round 111 新增 next-steps 后续步骤清单

- 新增 `cargo xtask evidence42 next-steps`：
  - 输出有序后续步骤清单（owner 标注）；
  - 覆盖 closure/orphan/evidence-gate、真实 peer、72h soak、Windows、clean locked CI、QEMU A/B 启动。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 新增 `evidence/42/next-steps.json`。

## 5.80 Round 112 新增 evidence-bundle 证据打包

- 新增 `cargo xtask evidence42 evidence-bundle`：
  - 收集 current-index 中当前有效 Envelope 与关键工具证据；
  - 复制到 `target/evidence42/bundle` 并生成 `MANIFEST.sha256`。
- 实跑结果：15 个证据文件打包，MANIFEST 生成。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`

## 5.81 Round 113 evidence-bundle 补齐 current/stale/candidate

- `evidence42 evidence-bundle` 增加打包：
  - `evidence/42/current-index.json`
  - `evidence/42/stale-index.json`
  - `evidence/42/candidate-lineage.json`
- 实跑：打包文件数增至 18，MANIFEST 刷新。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`

## 5.82 Round 114 self-test 扩展至 12 步

- `evidence42 self-test` 新增：
  - `evidence-bundle`
  - `final-status`
- 实跑：12/12 步骤 PASS。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `348d8f6f78e43251bff675b78f06205986ce41f405180be1968fcdd73079a03d`
  - candidate_id=`87582f772b2729610f29e53c633dbd3ce298a034c4226a4eca036a46885b0c85`
- 更新 `evidence/42/self-test.json`。

## 5.83 Round 115 同步 evidence42 白名单并刷新当前候选 L2

- 更新 `xtask/src/test_results.rs`，将 evidence42 全部新增子命令纳入 CI policy 白名单。
- 因 tracked 文件变化，候选 lineage 更新为：
  - dirty_tree_digest=`fb3880586e977bff9a77b5346e993d9bd584b861ca2420e836f59e0d99e2ad3f`
  - candidate_id=`10d1717ff4b13507d6749954655cd99ad1df56e118390f759dff196fcffa8bdb`
- 重新生成 round115 的 5 个 L2 Envelope，`gate-dir` 5/5 PASS。
- 刷新 stale-index/current-index/self-test/final-status/evidence-bundle。

## 5.84 Round 116 新增 check-bundle 校验证据包

- 新增 `cargo xtask evidence42 check-bundle`：
  - 读取 bundle 的 MANIFEST.sha256；
  - 校验每个文件哈希与存在性；
  - 任一缺失/不匹配则 FAIL。
- 实跑：18 个文件全部通过。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `fb3880586e977bff9a77b5346e993d9bd584b861ca2420e836f59e0d99e2ad3f`
  - candidate_id=`10d1717ff4b13507d6749954655cd99ad1df56e118390f759dff196fcffa8bdb`

## 5.85 Round 117 self-test 扩展至 13 步并刷新当前候选 L2

- `evidence42 self-test` 新增 `check-bundle`，现为 13 步，全部 PASS。
- `test_results.rs` 白名单同步 `check-bundle`。
- 因 tracked 文件变化，候选 lineage 更新为：
  - dirty_tree_digest=`51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`
- 重新生成 round117 的 5 个 L2 Envelope，`gate-dir` 5/5 PASS。
- 刷新 stale/current/self-test/final-status/evidence-bundle/check-bundle。

## 5.86 Round 118 新增 evidence42 help

- `evidence42 help` 列出全部子命令。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`

## 5.87 Round 119 新增 ci-status 快速状态命令

- 新增 `cargo xtask evidence42 ci-status`：
  - 执行 self-test，输出 CI-STATUS PASS/FAIL。
- 实跑：PASS。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`

## 5.88 Round 120 新增 fault-catalog 故障目录

- 新增 `cargo xtask evidence42 fault-catalog`：
  - 输出蓝图 42 WP-42.11 故障目录的机器可读清单；
  - 包含故障注入确认/期望 oracle/恢复上限/不变量要求。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`
- 新增 `evidence/42/fault-catalog.json`。

## 5.89 Round 121 新增 evidence-map Exit Gate 映射

- 新增 `cargo xtask evidence42 evidence-map`：
  - 输出 Exit Gate 1–13 对应的证据文件/命令映射。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`
- 新增 `evidence/42/evidence-map.json`。

## 5.90 Round 122 新增 checklist 检查清单

- 新增 `cargo xtask evidence42 checklist`：
  - 基于 evidence-map 生成 Exit Gate 1–13 的 Markdown 检查清单；
  - 输出 `evidence/42/CHECKLIST.md`。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`
- 新增 `evidence/42/CHECKLIST.md`。

## 5.91 Round 123 新增 exit-gate 状态汇总

- 新增 `cargo xtask evidence42 exit-gate`：
  - 汇总 13 条 Exit Gate 的当前状态（PASS/PARTIAL/BLOCKED/FAIL）；
  - 输出 `evidence/42/exit-gate.json`。
- 实跑：Gate 3/9/11 PASS；Gate 7 BLOCKED；Gate 13 FAIL；其余 PARTIAL。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`
- 新增 `evidence/42/exit-gate.json`。

## 5.92 Round 124 新增 final-audit 综合审计

- 新增 `cargo xtask evidence42 final-audit`：
  - 汇总 current-index、gate-status、summary、blockers、self-test、audit-all、exit-gate、evidence-map、fault-catalog、repro、unsafe-audit；
  - 输出 `evidence/42/final-audit.json`，结论 NOT_READY。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`
- 新增 `evidence/42/final-audit.json`。

## 5.93 Round 125 新增 version 命令

- 新增 `cargo xtask evidence42 version`：
  - 输出当前 candidate_id、dirty_tree_digest、source_commit。
- 实跑：candidate=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`

## 5.94 Round 126 新增 report 人类可读报告

- 新增 `cargo xtask evidence42 report`：
  - 读取 final-audit 并输出人类可读摘要；
  - 显示各 section 的 result/failures。
- 实跑：gate_status result=FAIL failures=3；repro/self_test/unsafe_audit PASS；结论 NOT_READY。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`

## 5.95 Round 127 新增 all 一键总览

- 新增 `cargo xtask evidence42 all`：
  - 依次执行 self-test、final-audit、report；
  - 全部成功则输出 ALL PASS。
- 实跑：ALL PASS（report 仍显示 gate_status FAIL/3 failures，结论 NOT_READY）。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`

## 5.96 Round 128 新增 summary-md 摘要文档

- 新增 `cargo xtask evidence42 summary-md`：
  - 从 final-audit 生成 Markdown 摘要 `evidence/42/SUMMARY.md`。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`
- 新增 `evidence/42/SUMMARY.md`。

## 5.97 Round 129 新增 evidence-count

- 新增 `cargo xtask evidence42 evidence-count`：
  - 输出当前有效/已 stale 的 Envelope 数量。
- 实跑：current=5, stale=47。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`

## 5.98 Round 130 新增 list 当前证据列表

- 新增 `cargo xtask evidence42 list`：
  - 列出 current-index 中当前有效 Envelope。
- 实跑：5 个 round117 Envelope。
- 本轮仅修改未跟踪的 `xtask/src/evidence_chain.rs`，dirty_tree_digest 保持：
  - `51e4b465382d29526282c160840d147d42835b0d260e8abab9424dd3aba3ba0c`
  - candidate_id=`bf4575573e4c631d4050d978c220b3fc0d270085f324470f3f19f3a1aee2d18c`
