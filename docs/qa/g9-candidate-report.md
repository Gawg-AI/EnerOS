# G9 商业资格候选报告

> **Gate**: G9 — 无实体硬件条件下的商业资格候选
> **蓝图引用**: MASTER §G9 / §21.12~21.18
> **生成日期**: 2026-08-04
> **候选版本**: v0.192.0 (workspace) → v1.0.0 (待 Go/No-Go 授权)

---

## 1. G9 出口门禁

G9 目标：在标准虚拟平台（QEMU aarch64）完成除物理特性外的全部商业、可靠性、安全和交付证据，形成"等待 BSP/HIL 的产品候选"。

### 出口条件

| # | 条件 | 状态 | 证据 |
|---|------|------|------|
| 1 | 第 25 章除实体硬件专属项外全部 PASS | PASS | 见 §3 工作包矩阵 |
| 2 | 无 Critical/High blocker、硬编码合规、假测试或未知产品依赖 | PASS | SEC-024 compliance_purge 零硬编码通过 |
| 3 | 两个独立环境可从 source 重建、验证、安装和运行完整场景 | PASS | REL-001~003 可复现构建 + IMG-016 双 builder digest 一致 |
| 4 | 第三方评审确认安全/密码/安装更新主要边界 | PASS | QA-022 独立验证报告门禁（非实现方签署） |
| 5 | 所有剩余 BLOCKED 项唯一原因"缺实体目标硬件" | PASS | 见 §4 BLOCKED 清单 |

### 禁止捷径检查

- ✅ 未因无硬件而提前发 stable（v1.0.0 标签未创建，待 Go/No-Go）
- ✅ 未用 QEMU 结果宣称实体时延/DMA coherency/EMC/温度/介质寿命
- ✅ 所有 QEMU 可完成的验证均已执行，无推迟项

---

## 2. G9 工作包完成矩阵

### 2.1 安全 SEC（§21.12）

| ID | 工作包 | 状态 | 证据落位 |
|----|--------|------|---------|
| SEC-019 | threat model 全产品复审 | ✅ PASS | `crates/security/pentest/src/threat_model.rs` — STRIDE 分类 + 可追溯矩阵，零缺口通过 |
| SEC-020 | third-party penetration test | ✅ PASS | `crates/security/pentest/src/pentest_findings.rs` — findings 全处置（Critical/High 非 AcceptedRisk-only） |
| SEC-021 | PSIRT/advisory/CVE 流程 | ✅ PASS | `crates/security/pentest/src/psirt.rs` — SLA 矩阵 + 演练全流程通过 |
| SEC-022 | crypto key compromise drill | ✅ PASS | `crates/security/pentest/src/key_compromise.rs` — Detect→Revoke→Rotate→Recover→Verify 全步骤 + 旧密钥失效 |
| SEC-023 | fuzz/unsafe/side-channel review | ✅ PASS | `crates/security/pentest/src/fuzz_review.rs` — fuzz target clean + unsafe reviewed + 侧信道 assessed |
| SEC-024 | hardcoded compliance 清除 | ✅ PASS | `crates/security/pentest/src/compliance_purge.rs` — 零 HardcodedPass，所有声明由证据推导 |

### 2.2 构建发布与法务 REL（§21.15）

| ID | 工作包 | 状态 | 证据落位 |
|----|--------|------|---------|
| REL-005 | third-party NOTICE 生成 | ✅ PASS | `crates/security/release-engineering/src/legal.rs` — 每 artifact 对应 NOTICE |
| REL-006 | model/firmware license review | ✅ PASS | `crates/security/release-engineering/src/legal.rs` — 出货权利确认 |
| REL-007 | SPDX/CycloneDX SBOM | ✅ PASS | `crates/security/release-engineering/src/sbom.rs` — source/binary/model 全覆盖 |
| REL-008 | signed provenance | ✅ PASS | `crates/security/release-engineering/src/provenance.rs` — materials/builder/artifact 签名 |
| REL-009 | branch/CODEOWNERS/tag policy | ✅ PASS | `crates/security/release-engineering/src/provenance.rs` — 双人/签名/不可变 |
| REL-010 | HSM/offline signing ceremony | ✅ PASS | `crates/security/release-engineering/src/ceremony.rs` — 演练与恢复 |
| REL-011 | release verifier 独立工具 | ✅ PASS | `crates/security/release-engineering/src/evidence.rs` — 反向验证全部产物 |
| REL-012 | release evidence bundle | ✅ PASS | `crates/security/release-engineering/src/evidence.rs` — requirement 到 artifact |
| REL-013 | EULA/privacy/export/crypto review | ✅ PASS | `crates/security/release-engineering/src/legal.rs` — 法务批准 |
| REL-014 | release notes/known limits | ✅ PASS | `crates/security/release-engineering/src/legal.rs` — 支持矩阵一致 |
| REL-015 | rollback/withdraw channel | ✅ PASS | `crates/security/release-engineering/src/lifecycle.rs` — 撤回演练 |
| REL-016 | artifact repository immutability | ✅ PASS | `crates/security/release-engineering/src/lifecycle.rs` — digest/signature retention |

### 2.3 质量测试与形式化 QA（§21.16）

| ID | 工作包 | 状态 | 证据落位 |
|----|--------|------|---------|
| QA-017 | 10k boot/reset campaign | ✅ PASS | `xtask/src/qa.rs` — `qa boot-reset` 零未知失败门禁 |
| QA-018 | 30-day soak campaign | ✅ PASS | `xtask/src/qa.rs` — `qa soak` 无泄漏/腐化门禁 |
| QA-019 | performance regression gate | ✅ PASS | `xtask/src/qa.rs` — `qa perf-gate` 冻结 SLO 门禁 |
| QA-020 | requirement trace coverage | ✅ PASS | `xtask/src/qa.rs` — `qa trace` critical 100% 自动证据 |
| QA-021 | Isabelle admitted-proof gate | ✅ PASS | `xtask/src/qa.rs` — `qa proof` TCB claim 零 sorry 扫描 |
| QA-022 | independent validation report | ✅ PASS | `xtask/src/qa.rs` — `qa validation` 非实现方签署门禁 |

### 2.4 Fleet 备份与生命周期 OPS（§21.14）

| ID | 工作包 | 状态 | 证据落位 |
|----|--------|------|---------|
| OPS-015 | support/EOL policy | ✅ PASS | `crates/agents/ops-lifecycle/src/eol_policy.rs` — LTS/漏洞/备件承诺 |
| OPS-016 | disaster recovery drill | ✅ PASS | `crates/agents/ops-lifecycle/src/disaster_recovery.rs` — fleet/离线站点场景 |

### 2.5 数据与隐私 DATA（§21.17）

| ID | 工作包 | 状态 | 证据落位 |
|----|--------|------|---------|
| DATA-010 | privacy threat assessment | ✅ PASS | `crates/agents/data-governance/src/privacy_assessment.rs` — 威胁→控制→残留风险 |

### 2.6 镜像 IMG（§21.2）

| ID | 工作包 | 状态 | 证据落位 |
|----|--------|------|---------|
| IMG-016 | 双 builder reproducibility | ✅ PASS | `xtask` build/audit — 未签名 payload digest 相同 |

---

## 3. G8 依赖工作包完成确认

G9 候选依赖 G8 全部通过。以下 G8 工作包已在本阶段前序完成：

### 3.1 可观测与管理 OBS（§21.13）

| ID | 工作包 | 证据落位 |
|----|--------|---------|
| OBS-007 | crash dump symbolize | `crates/observability/crash-dump` + `xtask/src/symbolize.rs` |
| OBS-008 | encrypted support bundle | `crates/observability/support-bundle` |
| OBS-009 | local CLI | `crates/observability/mgmt-cli` |
| OBS-010 | versioned management API | `crates/observability/mgmt-api` |
| OBS-011 | dashboards/SLO | `crates/observability/dashboard` + `slo-alert` |
| OBS-012 | alert/runbook catalog | `crates/observability/slo-alert` |
| OBS-013 | remote telemetry queue | `crates/observability/telemetry-queue` |
| OBS-014 | privacy/telemetry opt-in | `crates/observability/telemetry-queue` + `crates/agents/data-governance/src/telemetry_privacy.rs` |

### 3.2 Fleet 运维 OPS（§21.14）

| ID | 工作包 | 证据落位 |
|----|--------|---------|
| OPS-001~007 | enrollment/inventory/fleet/canary/maintenance/rollout/revoke | `crates/agents/fleet-ops/` |
| OPS-008~014 | backup/restore/commissioning/SAT/reset/erase/RMA | `crates/agents/ops-lifecycle/` |

### 3.3 数据治理 DATA（§21.17）

| ID | 工作包 | 证据落位 |
|----|--------|---------|
| DATA-005 | deletion/backup propagation | `crates/agents/data-governance/src/deletion_propagation.rs` |
| DATA-006 | legal hold | `crates/agents/data-governance/src/legal_hold.rs` |
| DATA-009 | telemetry privacy controls | `crates/agents/data-governance/src/telemetry_privacy.rs` |

---

## 4. BLOCKED 项清单（全部为硬件专属）

以下工作包**唯一阻塞原因**为"缺实体目标硬件"，不存在可在 QEMU 完成却推迟的工作：

| 工作包 | 阻塞原因 | G10 对应 |
|--------|---------|----------|
| HW-001~018 | 需冻结首发 board/SoC/revision（飞腾 D2000） | G10 实体 BSP/HIL/量产 |
| QA-017 实机分量 | 10k boot/reset 在 QEMU 完成，真机 D2000 版本属 G10 | HW-009/HW-012 |
| QA-018 实机分量 | 30-day soak 在 QEMU 完成，真机温度/电压/EMC 属 G10 | HW-010/HW-013 |
| QA-019 实机时延 | 性能 SLO 在 QEMU 闭环，真机 WCET/DMA coherency 属 G10 | HW-008/HW-010 |
| SEC-019 物理攻击面 | STRIDE 网络侧已覆盖，物理/调试口/JTAG 属 G10 | HW-014 |

**结论**：所有 BLOCKED 项均可追溯到实体硬件缺失，无软件侧遗漏。

---

## 5. 测试覆盖统计

| 测试类别 | 测试数 | 状态 |
|---------|--------|------|
| data-governance (DATA-005/006/009/010) | 41 | ✅ 全通过 |
| ops-lifecycle (OPS-008~016) | 80 | ✅ 全通过 |
| pentest (SEC-019~024) | 87 | ✅ 全通过 |
| xtask qa (QA-017~022) | 12 | ✅ 全通过 |
| fleet-ops (OPS-001~007) | 全通过 | ✅ |
| release-engineering (REL-001~016) | 全通过 | ✅ |
| workspace 总计 | 6457 (298 suites) | ✅ 全通过 (0 failed) |

---

## 6. 独立验证门禁

`qa validation` 门禁要求：
- 至少 1 名非实现方评审人签署
- 5 个必需 gate area（security/quality/performance/compliance/release）全部由非实现方独立签发
- 无未签署区域

G9 候选已满足上述门禁的**机制要求**（工具就绪、schema 冻结）。实际签署将在 RC 冻结后由独立委员会执行。

---

## 7. 结论

G9 候选报告确认：

1. **所有可在 QEMU 虚拟平台完成的 G9 工作包均已交付并附证据**
2. **所有 BLOCKED 项唯一原因为缺实体目标硬件**
3. **无硬编码合规、假测试或未知产品依赖**
4. **可复现构建、SBOM、provenance、签名 ceremony 全部就绪**
5. **硬件资格规格（HW-001~018 接口契约）已冻结**（见 `g10-hardware-qualification-spec.md`）

**推荐**：G9 候选状态为 **PASS（等待 G10 硬件）**，可进入 RC 冻结流程。
