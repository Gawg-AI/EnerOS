# Kev、NanoJev、Laya 设备端决策模型调研

> 调研日期：2026-09-29。本文只记录官方仓库、模型卡和 EnerOS 已有证据支持的事实；模型优劣须在 EnerOS 的目标任务、硬件和输入语言上重新测量。源码快照：[Kev `fdfdfd2`](https://github.com/jaredpalmer/kev/tree/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c)、[NanoJev `76fdfc9`](https://github.com/TianyuCodings/NanoJev/tree/76fdfc9ecdca45a9bcef17991a07d3041a87685a)、[Laya `9d95567`](https://github.com/NandhaKishorM/laya/tree/9d955671415fc19f069b9cc998928075c1f255ec)。

## 结论摘要

三者都是输入状态和已定义问题、输出 `choice`、`score`、`noul` 或相近概率分布的**类型化决策模型**，并非直接生成能源调度计划的求解器。[Kev 接口](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/README.md#api)、[NanoJev 能力](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/README.md#what-nanojev-does)、[Laya 接口](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#quickstart)。因此可以把它们作为**候选动作选择、告警分类、风险评分**的参考或后端候选；模型输出仍应进入 EnerOS 的独立约束认证和控制授权链。后一句是对 EnerOS [总体架构设计](../architecture/heterogeneous-data-decision-control-architecture.md)的适配推断，不是这些项目声称具备能源控制安全证明。

目前这三个仓库的现成运行时均不适合直接编入 EnerOS 的 seL4/Rust `no_std` 目标侧：Kev 和 Laya 依赖 Python、PyTorch、Transformers；NanoJev 的发布示例同样依赖 Python/PyTorch，明确以 CUDA 启动服务。[Kev 依赖](https://github.com/jaredpalmer/kev/blob/fdfdfd2c7a98225715a9cc8f4d9e1ef29ebbe79c/pyproject.toml)、[Laya 依赖](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/pyproject.toml)、[NanoJev 依赖](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/requirements-toy.txt)、[NanoJev 服务启动](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/README.md#quick-start)。这是从官方依赖和目标环境作出的工程判断；并不等于否定未来移植或板载独立推理单元的可能性。

EnerOS [PROJECT.md](../../PROJECT.md)规定目标为 seL4 ARM64 + Rust `no_std`，目标侧和 EL0 crate 禁用 `std`；既有[本地推理 spike](./local-inference-spike.md)对旧的 0.5B Q4 模型在当前 128 MiB QEMU 装配给出 NO-GO，2 GiB 目标板的纸面余量未经实机验证。新增决策模型须重新过静态内存、运行时移植、延迟与任务质量四项验证；不能把在 Linux/Mac 上“本机运行”等同于在 EnerOS 目标设备上可运行。

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

1. **当前 QEMU 产品装配：不能直接容纳。** 既有 spike 记录 guest RAM 128 MiB，而三者最小已发布权重文件均远大于该数；即使未来量化，也必须以目标侧实测 RAM 峰值而非文件大小重新裁决。[EnerOS 本地推理 spike](./local-inference-spike.md)、[三模型文件 API](https://huggingface.co/api/models/convaiinnovations/laya?blobs=true)。
2. **按 2 GiB 目标板规划：不能从文件大小推出可行。** Laya 多语言 644 MB 文件在纸面上最小，但运行时、激活、并发、操作系统余量与持续延迟未知；Kev 0.8B 基座文件约 1.75 GB，NanoJev checkpoint 约 2.39 GB。EnerOS 原 spike 对旧模型的 2 GiB 目标板也明确标为未经真机验证。[EnerOS 本地推理 spike](./local-inference-spike.md)、[文件 API](https://huggingface.co/api/models/convaiinnovations/laya?blobs=true)。
3. **直接 seL4/`no_std` 移植：三个官方仓均无即用目标 runtime。** 需要另作模型算子、tokenizer、内存分配、并行/加速器、模型加载及确定性时限的移植或重写；也可以探索在同一物理设备的独立 Linux/NPU 域运行并通过受控 IPC 通信，但这要求硬件和系统分区支持，不能称为已验证的 EnerOS 目标侧能力。[运行时依赖](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/pyproject.toml)、[EnerOS 原 spike](./local-inference-spike.md)。
4. **质量要以能源任务重测。** Kev-0.8B [模型卡](https://huggingface.co/jaredpalmer/kev-0.8b)明确提示其工具调用路由测试失败，不宜直接用于工具路由；Laya [README 的限制与校准段](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/README.md#honest-limits)指出基础 checkpoint 在 typed-decisions 测试近随机，出厂概率过度自信，多语言 checkpoint 未附拟合温度，否定词和多选项存在已知缺陷。NanoJev 已发布评估集中于游戏环境，[模型卡](https://huggingface.co/C-Tianyu/NanoJev)未给出能源控制任务证据。不同项目的公开基准不可直接横向排出 EnerOS 最优型号。

## 下一步需要裁决的研究问题

- “设备内”是 seL4 EL0 原生推理，还是同一物理板上的独立 Linux/NPU 域？两者的可信边界、RAM 和软件移植量不同。[EnerOS ADR-0045](../adr/ADR-0045-system2-supply-architecture.md)把板载协处理器列为独立备选，现行主线是站端 peer。
- 先选一个**有标签的能源场景**：例如告警/工单分类、控制候选风险评级，定义正确答案、误判成本、延迟预算、拒判与规则回退，再决定是否需要模型以及哪一种模型。[EnerOS 总体架构](../architecture/heterogeneous-data-decision-control-architecture.md)要求模型仅在授权范围给出候选，硬约束由独立模块认证。
- 若未来做可替换后端，需为同一冻结测试集测量各模型的准确率、校准、拒判覆盖率、最大 RAM、冷/热切换延迟、断电恢复和失联降级。模型版本、权重摘要、问题 schema 版本及最终认证结果应可追溯；这属于建议验收条件，尚无三模型在 EnerOS 目标板的验证结果。
