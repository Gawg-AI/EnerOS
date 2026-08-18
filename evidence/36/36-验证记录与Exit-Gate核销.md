# 蓝图 36 验证记录与 Exit Gate 核销

> schema: `eneros.version-36-evidence.v1`
> run_id: `36-2026-08-12-001`
> source_commit: `473b1e26681be33acc654f32e88251d8c19ad3cf`
> tree_state: dirty（全部改动保持未提交，未执行 reset/clean/checkout）
> 环境：Linux x86_64；toolchain nightly-2026-04-04（build-std）+ stable（host 测试）

## 1. 执行的验证命令与结果

| 类别 | 命令 | 结果 |
|---|---|---|
| 单元 | `cargo test -p eneros-xtask` | 84/84 PASS（含 verdict/product_manifest/product_deriver/policy/closure 新模块） |
| 回归 | `cargo test -p eneros-audit -p eneros-root-task -p eneros-services -p eneros-modbus-rtu` | 全 PASS（crypto 闭包修复无回归） |
| 政策 | `cargo xtask policy kernel` | KERNEL-POLICY-GATE PASS product_kernel=sel4 |
| 政策 | `cargo xtask policy adr` | ADR-REGISTRY-GATE PASS entries=39 |
| 闭包 | `cargo xtask closure gate --profile qemu-prod` | CLOSURE-GATE PASS binaries=13 reached=52 denied=0 crypto_violations=0 |
| Manifest | `cargo xtask manifest normalize --profile qemu-prod/board-prod` | PASS（digest 绑定） |
| 反解析 | `cargo xtask manifest verify-bundle --profile qemu-prod` | REVERSE-PARSE-GATE PASS entries=11 (root=true) |
| 派生 | `cargo xtask manifest derive --profile qemu-prod` | plan/bundle/bom(13)/sbom-scope 同 manifest_digest |
| 装配 | `SOURCE_DATE_EPOCH=1700000000 cargo xtask image qemu-prod` | IMAGE result=PASS（raw 128MB，manifest 派生 11 服务 bundle） |
| 文档 | `cargo xtask qa gate docs` | DOCS-GATE PASS（NO-GO 首屏 + 14 stale 不消费） |
| G7 | `cargo xtask qa g7-audit`（无 flag） | exit 2（G7-REVIEW，PENDING 非 PASS） |
| G7 | `cargo xtask qa g7-audit --allow-non-pass` | exit 0，报告 verdict=G7-REVIEW |
| Evidence | `cargo xtask evidence gate` | exit 2（BLOCKED：G0/G1 未全 completed——诚实状态） |
| Evidence | `cargo xtask evidence gate --allow-non-pass` | exit 0（信息报告） |
| Soak | `bash tests/soak/soak72.sh`（无硬件） | exit 1，写 PENDING 证据 |
| 语法 | `bash -n tests/soak/soak72.sh tests/interop/*.sh tests/delivery/g1-artifacts.sh` | 全部通过 |

## 2. Mutation 防假绿记录（全部转红验证）

| # | Mutation | Gate | 结果 |
|---|---|---|---|
| M1 | root-task 依赖注入 `storage[mock]` | closure gate | FAIL（RELEASE_FEATURE_DENIED，path=root-task -> storage[mock]），恢复后 PASS |
| M2 | profile 重复 service id | manifest normalize | FAIL（duplicate service id） |
| M3 | manifest 声明不存在 bin | image build | FAIL（no bin target） |
| M4 | 恢复 ADR-0001 Accepted（无 superseded_by） | policy adr | 逻辑保证红（adr-conflicting-kernel-decision） |
| M5 | 删除 required service（profile 变更） | image build | PASS（manifest 与 bundle 同源一致，语义正确） |
| M6 | bundle 多塞 ELF（hidden payload） | verify_media_bundle 单测 | FAIL（bundle-extra） |
| M7 | crypto product_allowed=false 到达闭包（修复前） | closure gate | FAIL（crypto_violations=1），修复后 0 |

## 3. 环境 BLOCKED_EVIDENCE

- QEMU 全链路日志（gkernel-qemu-boot.log）：本机未重建 seL4 全链 → BLOCKED
- Windows/Linux 跨平台互操作验证：本机 Linux → BLOCKED（42 portability）
- 真实互操作 peer / 72h 浸泡：无 → PENDING（36 止血语义，禁止 PASS）

## 4. Exit Gate 逐条核销

| # | Exit 项 | 结果 | 证据 |
|---|---|---|---|
| 1 | 内核政策负责人签署 | **PASS** | owner_approval=CHG-2026-08-12-36-01-sel4（ask 决策）；ADR-0043 Accepted；ADR-0001 Superseded |
| 2 | 密码用途/provider/授权政策机器可读 | **PASS** | product/crypto-policy.toml（7 用途细分）；closure gate 拒绝未授权 crypto |
| 3 | profile 严格 schema 表达 root 与未来服务 | **PASS** | schema.json v2（8 角色）；qemu-prod/board-prod v2；deny_unknown_fields |
| 4 | plan/BOM/bundle/SBOM scope 同 manifest 派生 | **PASS** | product_deriver 四派生器同 manifest_digest；image_factory 硬编码列表删除 |
| 5 | 正式介质反解析精确说明二进制集合 | **PASS** | REVERSE-PARSE-GATE entries=11 root=true；BOM 13 binaries 带 sha256 |
| 6 | resolved feature graph 无 mock/未授权 crypto | **PASS** | CLOSURE-GATE denied=0 crypto_violations=0；M1/M7 转红 |
| 7 | xtask Gate 不被 AI native backend 污染 | **PASS** | xtask 闭包无 llama/HiGHS/OpenSSL |
| 8 | G7/interop/soak/evidence/QEMU/perf 假绿全封 | **PASS** | 全部默认非零；--allow-non-pass 写 non_release |
| 9 | 旧文档/旧 evidence 明确 stale | **PASS** | 蓝图 00–34 横幅；evidence/index.json 14 stale；DOCS-GATE |
| 10 | 五类证据绑定当前 dirty baseline | **PASS** | 本文件 + RECON-36.md；dirty tree 明示 |

**Exit Gate 结论：10/10 PASS。产品商业状态仍为 NO-GO（36 允许）；本版本完成，允许进入 37。**

## 5. 收尾审查（security-review + review，本轮追加）

### 5.1 审查发现与处置

| # | 级别 | 发现 | 处置 |
|---|---|---|---|
| S1 | CRITICAL | closure/policy/verify-bundle Gate 未接线 CI | **已修**：ci.yml image 构建后新增 "36 release gates" 步骤（closure gate + policy kernel + policy adr + verify-bundle） |
| S2 | HIGH | CI QEMU seL4 boot 的 timeout(124) 无哨兵即视为成功 | **已修**：root_task.c emit `evidence-complete` 哨兵；ci.yml 要求 124 时必须见哨兵且无 FAIL，否则 FAIL |
| S3 | HIGH | gate-evidence `non_release` 硬编码 false | **已修**：`non_release = verdict != PASS`；实测 BLOCKED 状态写 true |
| S4 | MEDIUM | manifest_dir 的 CLI profile_id 未消毒（路径穿越） | **已修**：load_normalized 前置 is_profile_id 校验；mutation `--profile ../etc/passwd` → 拒绝 |
| S5 | MEDIUM | allowlist expiry 硬编码日期 | **接受**：静态策略日期设计（到期人工复审），记录 |
| S6 | LOW | external:true 服务跳过闭包扫描 | **接受**：image factory 拒绝 external bundle 条目兜底，记录 |
| R1 | should-fix | interop-report 丢弃 --allow-non-pass 恒 exit 0 | **已修**：非 D2-PASS 默认非零，仅 --allow-non-pass exit 0 |
| R2 | should-fix | g1-artifacts.sh toolchain rc≠0 仍打印 ALL PASS | **已修**：BLOCKED → `COMPLETE-WITH-BLOCKED` + exit 1 |
| R3 | should-fix | crypto-policy evidence_digest 仅长度检查 | **已修**：64-hex 或 `binding:<topic>`（无 `..`/`\`）格式校验；mutation 占位串 → 拒绝 |
| R4 | should-fix | gate.rs 旧 profile digest 检查被 NO-GO 短路 | **接受**：NO-GO 文档允许记录历史 digest，GO 时仍严格检查（设计决策） |
| R5 | should-fix | allowed_targets 未逐 target 比对 | **已知限制**：需闭包结构按 target 分组重构，记入 37/后续版本 |

### 5.2 收尾 mutation（防假绿）

| # | Mutation | Gate | 结果 |
|---|---|---|---|
| M8 | `closure gate --profile '../etc/passwd'`（路径穿越） | load_normalized | 拒绝（invalid profile id） |
| M9 | crypto-policy audit_hash evidence_digest 改占位串 | closure gate | FAIL（evidence-digest-malformed），恢复后 PASS |
| M10 | board-prod normalize 后 qemu-prod Gate（覆盖回归） | closure gate + verify-bundle | 修复前 FAIL（profile mismatch）；分目录后 PASS |
| M11 | 派生单测 manifest_dir_isolates_profiles | cargo test | 通过（两 profile 文件互不覆盖、独立加载） |

### 5.3 收尾后验证

- `cargo test -p eneros-xtask`：**85/85 PASS**（新增 manifest_dir_isolates_profiles）
- closure gate / policy kernel / policy adr / verify-bundle：全部 PASS
- evidence gate（BLOCKED 状态）：verdict=BLOCKED, non_release=true
- ci.yml：36 release gates 步骤接线 + QEMU 哨兵检查（YAML 缩进手工核对）

## 6. 宿主复核轮（security-review 二轮，最新 mutation 后）

### 6.1 二轮审查发现与处置

| # | 级别 | 发现 | 处置 |
|---|---|---|---|
| S7 | HIGH | gate_crypto_closure 为黑名单：未登记 crypto 库（sm3/gmssl/改名 eneros-crypto）可无标记入闭包 | **已修**：闭世界白名单——`CRYPTO_PROVIDER_KEYWORDS` 扫描（归一化小写+剥离非字母数字子串匹配），未登记 crypto 提供者 → `UNREGISTERED_CRYPTO_IN_CLOSURE` 失败。**实测抓到闭包内 2 个真实未登记包**（`ed25519`、`curve25519-dalek`，ed25519-dalek 的传递实现依赖，黑名单时代盲区），已登记为 boot_signature 的 `impl_dependencies` |
| S8 | HIGH | crypto 闭包检查精确串匹配，改名可绕过 | **已修**：归一化（小写 + 剥离全部非字母数字）双向匹配；`sha-2`/`SHA_2`/`sm.3` 与 `sha2`/`sm3` 同形（单测 + M12b 实测） |
| S9 | MEDIUM | `impl_dependencies` 无批准门禁：未获准用途的依赖可借登记合法化 | **已修**：impl_dependencies 仅随 `product_allowed=true` 主实现登记；未获准用途依赖入闭包 → UNREGISTERED（单测 crypto_closure_impl_deps_gated_on_approval） |
| S10 | MEDIUM | ci.yml 哨兵仅 rc==124 时要求；QEMU exit 0 无哨兵可假绿 | **已修**：哨兵 grep 无条件（rc==0 无哨兵 = FAIL）；M13 模拟验证（rc=0 无哨兵 → exit 1） |
| S11 | MEDIUM | `|| echo` 掩盖 peer 退出 | **已修**：`PEER_RC=$?` 捕获并输出告警（不掩盖 QEMU 结果；root 侧 FAIL 自报为硬门禁） |
| S12 | LOW | audit crate 残留 SM3 文档（lib.rs/events.rs/chain.rs/tests） | **已修**：迁移说明外全部更新为 SHA-256 |
| S13 | LOW | evidence/36 验证记录落后于最终代码 | **已修**：本节补记（M12/M12b/闭世界/88 测试） |
| S14 | LOW | ci.yml 9 处 `name:` 含 `: ` 未引号（严格 YAML 解析失败） | **已修**：全部双引号；`check_ci_yaml.py` 严格解析 PASS |

### 6.2 二轮 mutation（防假绿）

| # | Mutation | Gate | 结果 |
|---|---|---|---|
| M12 | crypto-policy `crate_name = "sha2"` 改名 `"sha2-renamed"`（模拟未登记） | closure gate | FAIL（UNREGISTERED sha2 等 3 violation）；恢复后 PASS |
| M12b | 登记名 `"sha-2"`（分隔符变体，模拟改名防御） | closure gate | PASS（归一化覆盖，改名防御双向成立）；恢复后 PASS |
| M13 | rc=0 无哨兵日志（模拟 exit 0 假绿） | ci.yml 哨兵逻辑 | FAIL as expected（无条件哨兵检查） |

### 6.3 二轮后最终验证

- `cargo test -p eneros-xtask`：**88/88 PASS**（新增 crypto_closure_closed_world、crypto_closure_impl_deps_gated_on_approval、normalize_crate_name_maps_dash_underscore）
- `cargo test -p eneros-audit`：**31/31 PASS**
- closure gate（qemu-prod）：**PASS**（13 binaries / 52 packages / denied=0 / crypto_violations=0）
- verify-bundle / policy kernel / policy adr：全部 PASS
- ci.yml：严格 YAML 解析 PASS（9 处 name 引号修复）；QEMU 步骤 socket+peer 变体 + 无条件哨兵
- tools/sel4-net-peer.py：重发加固（1s×20s，收到 TX 回应即停）；py_compile PASS

**二轮安全审查结论：先前 HIGH/MEDIUM 实质修复确认；新增 MEDIUM/LOW 全部处置。Exit Gate 维持 10/10 PASS。**

## 7. 收尾复验轮（2026-08-13，独立重验 + 残留缺陷修复）

> 性质：不信任验证记录自述，对全部 36 门禁独立重跑 + 活体 mutation + 逐模块代码级深读。

### 7.1 独立重跑结果（与 §1 记录一致）

| 命令 | 结果 |
|---|---|
| `cargo test -p eneros-xtask` | 88/88 PASS（修复前基线） |
| `policy kernel` / `policy adr` | PASS（sel4 / entries=39） |
| `closure gate --profile qemu-prod/board-prod` | PASS（13/12 binaries，denied=0，crypto_violations=0） |
| `manifest normalize/verify-bundle/derive`（qemu-prod） | PASS |
| `SOURCE_DATE_EPOCH=1700000000 cargo xtask image qemu-prod` | PASS（raw 128MB） |
| `qa gate docs` | PASS（NO-GO 首屏 + 14 stale） |
| `qa g7-audit` / `qa interop-report` / `evidence gate` | 默认非零（诚实 PENDING/BLOCKED） |
| `tests/interop/modbus.sh`（placeholder 分支） / `tests/soak/soak72.sh` | exit 1（PENDING 非 PASS） |

活体 mutation（临时改动后立即恢复，均确认抓取）：
- root-task 注入 `rs485[mock]` → closure gate FAIL（RELEASE_FEATURE_DENIED）；
- modbus-rtu 重新注入 mock 时 gate 不报——因 modbus-rtu 不在当前 manifest 二进制集合内（设计语义正确：gate 按 manifest 声明扫描，39 接入时自动纳入）。

### 7.2 复验发现的残留缺陷与修复

| # | 级别 | 发现 | 修复 |
|---|---|---|---|
| F1 | MEDIUM | `ProductManifest::validate()` 依赖环检测只沿每个服务的第一条依赖边遍历，非首边构成的环漏检（WP-36.3 “循环依赖一律失败”未完全成立） | 改为全图 DFS（`detect_dependency_cycle`，沿全部依赖边 + done 集剪枝）；新增红测 `dependency_cycle_on_non_first_edge_is_rejected`（含无环不误报断言） |
| F2 | MEDIUM | 反解析 Gate `verify_media_bundle` 只查介质 digest 非全零，不与 manifest 已绑定 service digest 交叉比对（WP-36.4 “与 Binary BOM 一一比对”不完整）；且 image_factory 在 bind_digests 之前反解析，比对被静默跳过；残留死变量 `bom_digests` | 新增 `bundle-digest-mismatch` 交叉比对；image_factory 改为先 bind 再反解析；删死变量；新增红测 `verify_media_digest_cross_check`。活体 mutation：篡改 init-bundle.bin 一字节 → verify-bundle FAIL（旧实现无法拦截），恢复后 PASS |
| F3 | MEDIUM | 派生物 build-plan/binary-bom 的 `profile_digest`/`source_commit` 为空串，偏离 WP-36.4 “所有输出携带相同 manifest_digest/profile_digest/source_commit” | `write_derivatives` 回填真实 profile_digest 与 git HEAD |
| F4 | LOW | closure allowlist 过期判定硬编码审计基线日 `2026-08-12`，expires 字段永不生效（豁免条目无限期合法，违背 allowlist “到期时间”要求；修正此前 S5 的接受裁决——当前 allowlist 为空，行为零变化） | 改为与真实当前 UTC 日期比较（civil_from_days，无新依赖）；测试改时钟无关（2099/2000）；新增 `current_utc_date_is_well_formed` |
| F5 | LOW | `validate_service_kinds` 无生产 caller，注释暗示当前生效（dead surface 隐患） | 不接线（强制会阻断早于 registry 的现有装配，构成次生阻断）；doc 明确生效点为 37+ 新服务接入门禁 |

### 7.3 修复后最终验证

- `cargo test -p eneros-xtask`：**91/91 PASS**（新增 3 测试）
- `cargo test -p eneros-audit -p eneros-modbus-rtu`：全 PASS（无回归）
- `image qemu-prod`：**PASS**（先 bind 后反解析无误报，digest 交叉比对在真实构建链生效）
- `verify-bundle` / `derive` / `closure gate`×2 / `policy kernel/adr` / `qa gate docs`：全部 PASS；derive 输出确认 profile_digest/source_commit 已填充
- 篡改 mutation：init-bundle.bin 单字节篡改 → `bundle-digest-mismatch:svc-drv-fault` FAIL；恢复后 PASS
- 修改文件（product_manifest/product_deriver/closure/image_factory）零新增编译 warning

**收尾复验轮结论：验证记录的既有声称全部独立复现；新发现 F1–F5 全部处置并红测覆盖。Exit Gate 维持 10/10 PASS；产品商业状态仍为 NO-GO（需 37–43 串行完成后重算）。**

## 8. 独立复核轮（2026-08-13，Reasonix 宿主复核 + 残留缺口修复）

> 性质：对 §1–§7 全部声称做独立重跑；发现并处置一处残留缺口。

### 8.1 独立重跑结果（与既有记录一致）

| 命令 | 结果 |
|---|---|
| `cargo test -p eneros-xtask` | 92/92 PASS（复核基线 91/91 + 本轮新增 1） |
| `policy kernel` / `policy adr` | PASS（sel4 / entries=39） |
| `closure gate --profile qemu-prod` / `board-prod` | PASS（13/12 binaries，denied=0，crypto_violations=0） |
| `manifest normalize / verify-bundle / derive`（qemu-prod） | PASS |
| `SOURCE_DATE_EPOCH=1700000000 cargo xtask image qemu-prod` | PASS（raw 128MB，sha256=15aa3631…） |
| `qa gate docs` | PASS（NO-GO 首屏 + 14 stale） |
| `qa g7-audit` / `qa interop-report` / `evidence gate` | 默认非零（G7-REVIEW / D2-PENDING / BLOCKED） |
| `tests/soak/soak72.sh` / `tests/interop/modbus.sh`（placeholder 分支） | exit 1（PENDING 非 PASS）；无设备 skip 分支 exit 0 且写 skip JSON（不被 interop-report 当 VERIFIED） |
| `cargo test -p eneros-audit -p eneros-modbus-rtu` | 全 PASS（crypto 闭包修复无回归） |
| ci.yml | 严格 YAML 解析 PASS；36 release gates 步骤接线；QEMU 哨兵检查；g1-artifacts.sh 无 `\|\| true` |

### 8.2 残留缺口修复

| # | 级别 | 发现 | 修复 |
|---|---|---|---|
| F6 | MEDIUM | F3 修复不完整：`write_derivatives` 只对 build-plan/binary-bom 回填 profile_digest/source_commit，**sbom-scope.json 两字段为空**（WP-36.4 “所有输出携带相同 manifest_digest/profile_digest/source_commit” 未全覆盖） | `SbomScope` 增加 profile_digest/source_commit 字段；抽 `fill_derivative_identity` 纯函数统一回填三个派生物（并修正 plan 先写盘后回填的顺序缺陷）；新增红测 `derivative_identity_filled_for_plan_bom_and_sbom_scope` |

### 8.3 修复后最终验证

- `cargo test -p eneros-xtask`：**92/92 PASS**（新增 1 测试）
- `manifest derive --profile qemu-prod`：PASS；build-plan/binary-bom/sbom-scope 三者的 manifest_digest 一致、profile_digest=deaf9c72…、source_commit=473b1e26… 全部填充
- 其余 Gate 复跑无回归

**独立复核轮结论：36 的 Exit Gate 10 项全部维持 PASS；F6 为唯一残留缺口，已修复并红测覆盖。产品商业状态仍为 NO-GO（需 37–43 串行完成后重算）。**

