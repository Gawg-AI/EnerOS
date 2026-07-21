# Tasks

- [x] Task 1: Workspace 同步
  - [x] 根 `Cargo.toml` 版本号 `0.60.0` → `0.61.0`
  - [x] members 添加 `crates/ai/model-deploy`
  - [x] 验证：`cargo metadata --format-version 1` 成功（待 crate 骨架创建后）

- [x] Task 2: 创建 `eneros-model-deploy` crate 骨架
  - [x] 新建 `crates/ai/model-deploy/Cargo.toml`，package name = `eneros-model-deploy`
  - [x] dependencies 添加 `eneros-llm-engine = { path = "../llm-engine" }` + `eneros-gguf-loader = { path = "../gguf-loader" }`（D11 复用 v0.59.0/v0.60.0 类型）
  - [x] features 声明：`[features] llama-cpp = []`（默认关闭，D3）
  - [x] no_std 配置：`#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
  - [x] 新建 `src/lib.rs`，模块声明：config / error / report / prompts / verifier / backend / mock_backend
  - [x] lib.rs 包含 D1~D12 偏差声明表
  - [x] 验证：`cargo metadata --format-version 1` 成功

- [x] Task 3: 实现 `error.rs` — DeployError 错误类型
  - [x] `DeployError` 枚举：HardwareInsufficient / ModelLoadFailed / InferenceFailed / InvalidResult / Timeout / BackendError / NotDeployed
  - [x] 派生 `Debug` + `Clone`，实现 `core::fmt::Display`
  - [x] 实现 `From<LlmError>` 转换（复用 v0.59.0 错误）
  - [x] 验证：`cargo build -p eneros-model-deploy` 通过

- [x] Task 4: 实现 `config.rs` — QuantConfig7B 量化配置
  - [x] `QuantConfig7B` 结构体：model_name / model_path / quantization / file_size_gb / min_ram_gb / min_vram_gb / context_length / device / infer_params（D11 复用 v0.59.0）
  - [x] 派生 `Debug / Clone`
  - [x] `QuantConfig7B::default() -> Self`（Qwen2.5-7B, Q4_K_M, 4.0GB, 8.0GB RAM, 6.0GB VRAM, 4096 context, Cpu, temperature=0.1）
  - [x] `QuantConfig7B::new(model_name: &str) -> Self`（自定义模型名，其余默认）
  - [x] `with_device(device: ComputeDevice) -> Self`（builder 设置设备，D4）
  - [x] `with_path(path: &str) -> Self`（builder 设置模型路径）
  - [x] 验证：单元测试 — 默认值 / 自定义模型名 / builder

- [x] Task 5: 实现 `prompts.rs` — PowerPromptSet 电力场景测试用例
  - [x] `PowerPrompt` 结构体：prompt: String / expected_keywords: Vec<String> / description: String
  - [x] `PowerPromptSet` 结构体：prompts: Vec<PowerPrompt>
  - [x] `PowerPromptSet::default() -> Self`（包含储能策略/电价响应/异常处理等 5 个电力场景 prompt）
  - [x] `PowerPromptSet::prompts(&self) -> &[PowerPrompt]` 查询
  - [x] `PowerPrompt::validate_result(&self, result: &str) -> bool`（校验结果非空 + 包含任一关键词）
  - [x] 验证：单元测试 — 默认 prompt 数量 / validate_result

- [x] Task 6: 实现 `report.rs` — DeployReport 部署报告
  - [x] `DeployFailure` 结构体：prompt: String / error: DeployError（D5 普通 u64）
  - [x] `DeployReport` 结构体：device / n_gpu_layers / load_time_ns / inference_count / total_tokens / total_inference_ns / avg_tokens_per_sec / passed / failures
  - [x] 派生 `Debug / Clone`
  - [x] `DeployReport::new(device: ComputeDevice, n_gpu_layers: u32) -> Self`
  - [x] `record_load_time(&mut self, ns: u64)` 记录加载时间
  - [x] `record_inference(&mut self, tokens: u64, ns: u64)` 记录推理
  - [x] `add_failure(&mut self, prompt: String, error: DeployError)` 添加失败
  - [x] `finalize(&mut self)` 计算平均 token/s 并设 passed
  - [x] 验证：单元测试 — 构造 / 累加 / finalize

- [x] Task 7: 实现 `backend.rs` — DeployBackend trait + HardwareCheck（D2/D3）
  - [x] `HardwareCheck` 结构体：ram_gb: f64 / vram_gb: f64 / meets_requirements: bool
  - [x] `DeployBackend` trait：`fn check_hardware(&self, config: &QuantConfig7B) -> Result<HardwareCheck, DeployError>` + `fn load_model(&self, engine: &mut dyn LlmEngine, config: &QuantConfig7B) -> Result<u64, DeployError>`（返回加载时间 ns）
  - [x] 验证：trait 定义编译通过

- [x] Task 8: 实现 `mock_backend.rs` — MockDeployBackend（D12 默认可用）
  - [x] `MockDeployBackend` 结构体：无字段（或 `load_time_override: Option<u64>`）
  - [x] `MockDeployBackend::new() -> Self`
  - [x] 实现 `DeployBackend` for `MockDeployBackend`
  - [x] `check_hardware` 返回 `Ok(HardwareCheck { ram_gb: 16.0, vram_gb: 8.0, meets_requirements: true })`（Mock 通过）
  - [x] `load_model` 调用 `engine.load_model(config.model_path)`，返回固定时间（如 1_000_000 ns）
  - [x] 验证：单元测试 — check_hardware / load_model

- [x] Task 9: 实现 `verifier.rs` — DeployVerifier 部署验证器（D2 泛型化）
  - [x] `DeployVerifier<E: LlmEngine>` 结构体：engine: E / config: QuantConfig7B / backend: Box<dyn DeployBackend> / prompts: PowerPromptSet
  - [x] `DeployVerifier::new(engine: E, config: QuantConfig7B) -> Self`（带 MockDeployBackend + PowerPromptSet::default()）
  - [x] `DeployVerifier::with_backend(engine: E, config: QuantConfig7B, backend: Box<dyn DeployBackend>) -> Self`
  - [x] `deploy(&mut self) -> Result<DeployReport, DeployError>` — 完整部署验证流程
  - [x] `undeploy(&mut self) -> Result<(), DeployError>` — 卸载（调用 engine 清理，实际清理由 LlmEngine 实现）
  - [x] 验证：单元测试 — 完整部署 / 部署失败 / undeploy

- [x] Task 10: 实现 `llama_backend.rs` — LlamaDeployBackend（D3 feature-gated）
  - [x] `#[cfg(feature = "llama-cpp")]` 门控整个模块
  - [x] `LlamaDeployBackend` 结构体
  - [x] 实现 `DeployBackend` for `LlamaDeployBackend`
  - [x] `check_hardware` 实际校验 RAM/VRAM（通过 v0.59.0 stats 或 FFI，本版本简化为返回 Mock 值）
  - [x] `load_model` 调用 `engine.load_model` 并记录真实时间
  - [x] 验证：默认 feature 下不编译（`cargo build` 不报错）

- [x] Task 11: 集成测试模块（lib.rs `#[cfg(test)] mod tests`）
  - [x] T1 QuantConfig7B 默认值（model_name="Qwen2.5-7B", quantization=Q4_K_M, temperature=0.1）
  - [x] T2 QuantConfig7B::new 自定义模型名
  - [x] T3 QuantConfig7B with_device builder（设置 Cuda）
  - [x] T4 PowerPromptSet 默认 5 个 prompt
  - [x] T5 PowerPrompt validate_result 有效结果（包含关键词）
  - [x] T6 PowerPrompt validate_result 无效结果（空字符串）
  - [x] T7 DeployReport 构造 + record_load_time
  - [x] T8 DeployReport record_inference + finalize（计算 avg_tokens_per_sec）
  - [x] T9 DeployReport add_failure（passed=false）
  - [x] T10 MockDeployBackend check_hardware 返回 meets_requirements=true
  - [x] T11 MockDeployBackend load_model 成功（MockEngine）
  - [x] T12 DeployVerifier 完整部署（MockEngine + MockDeployBackend → Ok(DeployReport)）
  - [x] T13 DeployVerifier 部署失败（未加载模型的 MockEngine → Err(ModelLoadFailed)）
  - [x] T14 DeployVerifier GPU 优先逻辑（config.with_device(Cuda) → report.device=Cuda）
  - [x] T15 DeployVerifier undeploy 成功
  - [x] 验证：`cargo test -p eneros-model-deploy` 全部通过

- [x] Task 12: 设计文档 `docs/ai/model-deploy-design.md`
  - [x] 12 章节：版本目标 / 架构定位 / QuantConfig7B / DeployVerifier / DeployBackend / PowerPromptSet / DeployReport / GPU 策略 / 错误处理 / feature 门控 / 内存预算 / 偏差声明
  - [x] 2 Mermaid 图：DeployVerifier 类图 + 部署时序图
  - [x] D1~D12 偏差声明表
  - [x] 文档位置在 `docs/ai/` 下（复用 v0.59.0/v0.60.0 创建的目录）

- [x] Task 13: 版本号同步 + gate.rs 注释更新
  - [x] `Makefile` 版本号 `0.60.0` → `0.61.0`
  - [x] `.github/workflows/ci.yml` 版本号 `0.60.0` → `0.61.0`
  - [x] `ci/src/gate.rs` clippy 段 + test 段注释补充 `eneros-model-deploy` 说明
  - [x] 验证：`cargo build -p eneros-model-deploy` 通过

- [x] Task 14: 构建校验（§2.4.2 C6~C11）
  - [x] `cargo metadata --format-version 1` 成功
  - [x] `cargo test -p eneros-model-deploy` 全部通过（15 tests）
  - [x] `cargo build -p eneros-model-deploy --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` 交叉编译通过
  - [x] `cargo fmt -p eneros-model-deploy -- --check` 格式通过
  - [x] `cargo clippy -p eneros-model-deploy --all-targets -- -D warnings` lint 通过
  - [x] `cargo deny check licenses bans sources` 安全扫描通过

- [x] Task 15: 更新 tasks.md + checklist.md 所有项 → [x]
  - [x] tasks.md 15 任务全部 [x]
  - [x] checklist.md 所有检查点全部 [x]

# Task Dependencies

- Task 2 → Task 1（crate 骨架需先于 metadata 验证）
- Task 3~8 → Task 2（各模块依赖 crate 骨架）
- Task 3（error）→ Task 4~10（各模块返回 DeployError）
- Task 4（config）→ Task 7（backend 使用 QuantConfig7B）
- Task 5（prompts）→ Task 9（verifier 使用 PowerPromptSet）
- Task 6（report）→ Task 9（verifier 返回 DeployReport）
- Task 7（backend trait）→ Task 8（mock_backend 实现 trait）
- Task 7（backend trait）→ Task 10（llama_backend 实现 trait）
- Task 8（mock_backend）→ Task 9（verifier 默认使用 MockDeployBackend）
- Task 9（verifier）→ Task 11（集成测试依赖 verifier）
- Task 10（llama_backend）独立，feature-gated
- Task 11 → Task 3~10（集成测试依赖所有模块）
- Task 12 → Task 11（文档在测试通过后撰写）
- Task 13 → Task 12（版本同步在功能完成后）
- Task 14 → Task 13（构建校验在版本同步后）
- Task 15 → Task 14（更新文档在全部校验通过后）

# Parallelizable Work

- Task 3（error）+ Task 4（config）+ Task 5（prompts）+ Task 6（report）可并行（无依赖）
- Task 7（backend trait）依赖 Task 4（config）
- Task 8（mock_backend）+ Task 10（llama_backend）依赖 Task 7，可并行
- Task 9（verifier）依赖 Task 3~8 全部
- Task 11 → Task 9, 10
