# Kev、NanoJev、Laya 设备端决策模型调研

> 调研日期：2026-09-29。本文只记录官方仓库、模型卡和 EnerOS 已有证据支持的事实；模型优劣须在 EnerOS 的目标任务、硬件和输入语言上重新测量。源码快照：[Kev `fdfdfd2`](https://github.com/jaredpalmer/kev/tree/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c)、[NanoJev `76fdfc9`](https://github.com/TianyuCodings/NanoJev/tree/76fdfc9ecdca45a9bcef17991a07d3041a87685a)、[Laya `9d95567`](https://github.com/NandhaKishorM/laya/tree/9d955671415fc19f069b9cc998928075c1f255ec)。

## 结论摘要

三者都是输入状态和已定义问题、输出 `choice`、`score`、`noul` 或相近概率分布的**类型化决策模型**。它们均不生成自由文本，也不能单独提供用户要求的本机生成、开放式推理、步骤规划或能源调度计划；若这些属于目标能力，仍需另有生成模型和/或求解器。[Kev 接口与无文本生成架构](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/README.md#how-it-works)、[NanoJev 零输出 token 设计](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/README.md#what-nanojev-does)、[Laya 无文本生成说明](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#quickstart)。三者可作为**候选动作选择、告警分类、风险评分**的参考或后端候选；模型输出仍应进入 EnerOS 的独立约束认证和控制授权链。后一句是对 EnerOS [总体架构设计](../architecture/heterogeneous-data-decision-control-architecture.md)的适配推断，不是这些项目声称具备能源控制安全证明。

目前这三个仓库的现成运行时均不适合直接编入 EnerOS 的 seL4/Rust `no_std` 目标侧：Kev 和 Laya 依赖 Python、PyTorch、Transformers；NanoJev 的发布示例同样依赖 Python/PyTorch，明确以 CUDA 启动服务。[Kev 依赖](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/pyproject.toml)、[Laya 依赖](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/pyproject.toml)、[NanoJev 依赖](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/requirements-toy.txt)、[NanoJev 服务启动](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/README.md#quick-start)。这是从官方依赖和目标环境作出的工程判断；原生 seL4 移植须单独验证。

EnerOS [PROJECT.md](../../PROJECT.md)规定目标为 seL4 ARM64 + Rust `no_std`，目标侧和 EL0 crate 禁用 `std`。2026-08-22 的[本地推理 spike](./local-inference-spike.md)在**当时的 128 MiB QEMU 装配**下对另一个 0.5B Q4 模型给出 NO-GO；这不是当前内存配置。现行 [QEMU 启动参数](../../xtask/src/qemu_harness.rs)和[板事实](../../product/skus/qemu-virt-aarch64.json)均为 **1 GiB**。新增决策模型须基于当前装配重新过静态内存、原生运行时移植、延迟与任务质量四项验证；不能把在 Linux/Mac 上“本机运行”等同于在 EnerOS 目标设备上可运行。

## 仓库身份与许可

| 项目 | 官方来源与现状 | 许可 |
|---|---|---|
| Kev | [Jared Palmer 的代码仓](https://github.com/jaredpalmer/kev)，[模型卡](https://huggingface.co/jaredpalmer/kev-0.8b)说明 0.8B 是 Qwen3.5 基座加 LoRA 和指针头。 | [代码 Apache-2.0](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/LICENSE)；[模型卡标 Apache-2.0](https://huggingface.co/jaredpalmer/kev-0.8b)，基座另有其许可。 |
| NanoJev | [TianyuCodings 的代码仓](https://github.com/TianyuCodings/NanoJev)，[模型卡](https://huggingface.co/C-Tianyu/NanoJev)把已发布 checkpoint 定位为 Maze、Snake、ViZDoom 四项游戏任务的 0.6B 模型。 | [源码 MIT](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/LICENSE)；[模型卡](https://huggingface.co/C-Tianyu/NanoJev)说明 Qwen 基座保留上游许可，但没有为 NanoJev 权重明确列出单独 SPDX 许可；商用分发前须核实权重条款。 |
| Laya | [NandhaKishorM 的代码仓](https://github.com/NandhaKishorM/laya)，`pyproject.toml` 的作者为 Convai Innovations。另有 [`he-jev/laya`](https://github.com/he-jev/laya) 存放 checkpoint 文件，其 README 的“Links”明确把 GitHub 源码指向 NandhaKishorM 仓；不能把该仓当作当前代码主仓。 | [代码 Apache-2.0](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/LICENSE)；[官方模型卡标 Apache-2.0](https://huggingface.co/convaiinnovations/laya)。 |

## 运行、体积与接口

以下为**已发布文件的磁盘字节数**，不是完整的 RAM 峰值；内存还包括运行时、激活、缓存及可能同时驻留的多个模型。文件体积取自相应发布方的 Hugging Face 模型文件 API。

| 项目 | 最小/相关发布模型 | 已发布权重文件 | 现成运行时与本地接口 |
|---|---|---:|---|
| Kev | 0.8B 是最小当前型号；4B、9B、27B 另有发布。[型号表](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/README.md#models) | [Qwen3.5-0.8B-Base](https://huggingface.co/api/models/Qwen/Qwen3.5-0.8B-Base?blobs=true) 1,746,942,600 B，另加 [Kev adapter/head](https://huggingface.co/api/models/jaredpalmer/kev-0.8b?blobs=true) 43,338,624 + 2,103,999 B。 | Python 3.12/3.13、Torch、Transformers；CUDA/ROCm 或 Apple MLX 路径；`kev.serve` 提供本地 `POST /v1/systemone`。[启动与 API](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/README.md#run-it-locally)、[依赖](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/pyproject.toml)。 |
| NanoJev | Qwen3-0.6B，当前发布聚焦四种游戏任务。[模型卡](https://huggingface.co/C-Tianyu/NanoJev) | [`unified-games-v1` 的 `best.safetensors`](https://huggingface.co/api/models/C-Tianyu/NanoJev/revision/unified-games-v1?blobs=true) 2,385,039,280 B。 | 官方测试依赖 Python 3.14/PyTorch/Transformers，文档以 CUDA 环境运行 `scripts/serve_decisions.py`，本地 `POST /api/evaluate`。[依赖](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/requirements-toy.txt)、[启动](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/README.md#quick-start)。 |
| Laya | 英文/typed-decisions 421M，多语言 322M；中文输入应评估多语言 checkpoint。[型号表](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#quickstart) | [英文/typed-decisions](https://huggingface.co/api/models/convaiinnovations/laya?blobs=true) 分别 842,609,210 / 842,609,220 B；多语言 643,835,514 B。 | Python ≥3.10、Torch、Transformers；CPU、CUDA、MPS 等；`laya-serve` 提供 `POST /v1/systemone`，Python SDK、ONNX 导出等路径均仍需要有标准运行时。[安装](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#installation)、[HTTP 服务](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#self-hosting-http-server-jev-compatible)。 |

若把“模型随时可替换”定义为**业务逻辑不改即可替换后端**，EnerOS 应先定义自身的版本化、定界的决策输入/输出合同，再由 adapter 归一化各服务输出。这是架构建议，不是三个项目已互相兼容的事实：

- Kev 与 Laya 都提供 Jev 风格 `POST /v1/systemone`，但仍有实际语义差异。Laya 官方文档列出选项 token 预算、`score` 空级别和 `confidence` 算法与 Jev 不一致；不能只凭相同路径认为阈值可复用。[Kev API](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/README.md#api)、[Laya 差异](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#self-hosting-http-server-jev-compatible)。
- Kev 的 `--run` 在服务启动时加载一个 checkpoint；源码中的 `kev-latest` 与 `jev-latest` 只是这个已加载模型的两个请求名，**不是请求级热换模型**。[Kev 服务源码](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/kev/serve.py)。
- Laya 的 `Router` 能按请求语言或显式 `model` 在三种 checkpoint 间路由；内存不足时 `max_loaded=1` 会重载权重，官方记录每次切换约 7–10 秒，预加载则增加驻留内存。[Laya Router](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#quickstart-route-mode-recommended)、[重载成本](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#production-preload--memory)。
- NanoJev **还不是**完整 Jev HTTP 契约：本地服务是 `/api/evaluate`，本地类型为 `boolean`/`choice`/`score`；官方兼容审计指出 `noul` 与响应字段仍需映射，结构化输入也有差异。[HTTP 实现](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/scripts/serve_decisions.py)、[兼容审计](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/docs/TYPESAFE_CONTRACT.md)。

## 对 EnerOS 的可行性边界

1. **当前 QEMU 与服务预算要按现行配置审计。** QEMU guest RAM 是 **1 GiB**，不是旧 spike 的 128 MiB；Laya 发布的 644/843 MB 权重文件大小不能单独证明它能装入或装不入 1 GiB。运行时还需考虑模型展开格式、激活、tokenizer、系统和其他服务的占用。[QEMU 启动参数](../../xtask/src/qemu_harness.rs)、[板事实](../../product/skus/qemu-virt-aarch64.json)、[Laya 文件 API](https://huggingface.co/api/models/convaiinnovations/laya?blobs=true)。
2. **当前目标侧装配尚无模型装载能力。** `svc-ai-worker` 使用的 `heap-large` 分配器当前为 **1 MiB**；`qemu-v1` 磁盘布局的 `MODELS` 分区为 **16 MiB**，低于三种原版发布权重体积。当前目标侧代码没有 `MODELS` 分区运行时消费者；预留分区不等于可用的模型加载链。[服务依赖](../../crates/agents/control-services/Cargo.toml)、[分配器](../../crates/drivers/drv-svc/src/allocator.rs)、[分区布局](../../product/image-layouts/qemu-v1.json)。这几项是现有装配限制，不代表硬件总 RAM 的最终上限。
3. **原生 seL4/`no_std` 移植：三个官方仓均无即用目标 runtime。** 需要另作模型算子、tokenizer、内存分配、加速器驱动、模型加载及有界时限的移植或重写，并在 seL4 EL0 进程中实测；独立 Linux 服务不满足本次“原生 seL4”要求。[运行时依赖](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/pyproject.toml)、[EnerOS 原 spike](./local-inference-spike.md)。
4. **生成能力仍有独立缺口。** 三项目的类型化输出不能直接生成文本说明、开放式新计划或连续功率轨迹；若用户所需“本机生成”包含这些能力，需要在原生 seL4 约束下另选生成/求解路径，再和类型化判断组合。[Kev API](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/README.md#api)、[Laya 说明](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#quickstart)。
5. **质量要以能源任务重测。** Kev-0.8B [模型卡](https://huggingface.co/jaredpalmer/kev-0.8b)明确提示其工具调用路由测试失败，不宜直接用于工具路由；Laya [README 的限制与校准段](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#honest-limits)指出基础 checkpoint 在 typed-decisions 测试近随机，出厂概率过度自信，多语言 checkpoint 未附拟合温度，否定词和多选项存在已知缺陷。NanoJev 已发布评估集中于游戏环境，[模型卡](https://huggingface.co/C-Tianyu/NanoJev)未给出能源控制任务证据。不同项目的公开基准不可直接横向排出 EnerOS 最优型号。

## 下一步需要裁决的研究问题

- 在**原生 seL4 EL0** 范围内，先确定是否同时需要类型化判断与自由生成，再裁决各自的模型/求解器、可替换合同，以及目标设备的 RAM、存储、加速器和决策时限；这不能由普通主机端 benchmark 代替。[EnerOS PROJECT.md](../../PROJECT.md)、[原本地推理 spike](./local-inference-spike.md)。
- 先选一个**有标签的能源场景**：例如告警/工单分类、控制候选风险评级，定义正确答案、误判成本、延迟预算、拒判与规则回退，再决定是否需要模型以及哪一种模型。[EnerOS 总体架构](../architecture/heterogeneous-data-decision-control-architecture.md)要求模型仅在授权范围给出候选，硬约束由独立模块认证。
- 若未来做可替换后端，需为同一冻结测试集测量各模型的准确率、校准、拒判覆盖率、最大 RAM、冷/热切换延迟、断电恢复和失联降级。模型版本、权重摘要、问题 schema 版本及最终认证结果应可追溯；这属于建议验收条件，尚无三模型在 EnerOS 目标板的验证结果。
