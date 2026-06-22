# ADR-0001：记录架构决策

- **状态**：Accepted
- **日期**：2026-06-21
- **决策者**：EnerOS 核心团队

## 上下文

EnerOS 是一个面向电力与能源领域的原生智能体操作系统，涉及硬件、协议、安全、实时性等多个维度的架构决策。随着项目演进，许多决策的背景与理由容易在代码更迭中遗失，导致后续开发者难以理解"为什么这样设计"。

我们需要一种轻量级机制，记录重要的架构决策及其上下文，使决策过程可追溯、可审查。

## 决策

采用架构决策记录（Architecture Decision Record，ADR）模式管理架构决策。

### ADR 规范

1. **文件命名**：`docs/adr/NNNN-短描述.md`，NNNN 为 4 位顺序编号（0001、0002……）
2. **文件结构**：每个 ADR 包含以下章节：
   - 标题（编号 + 简述）
   - 状态（Proposed / Accepted / Deprecated / Superseded）
   - 上下文（问题背景、约束、考虑的方案）
   - 决策（选择方案及理由）
   - 后果（正面 / 负面 / 中性影响）
   - 参考（相关 ADR 或外部链接）
3. **编号不可复用**：即使某 ADR 被 Superseded，其编号也不可被新 ADR 复用
4. **状态流转**：Proposed → Accepted →（Deprecated 或 Superseded by ADR-NNNN）
5. **不可删除**：ADR 一旦 Accept 即不可删除，状态变更通过新增 ADR 标注 Superseded 关系

### 何时编写 ADR

- 引入新的核心抽象或设计模式
- 在多个可行方案中做出取舍
- 变更已有架构决策
- 引入或移除重要依赖
- 影响多 crate 的接口变更

### 何时不需要 ADR

- 单个函数的实现细节
- 纯代码格式调整
- 仅影响单个 crate 内部的重构

## 后果

### 正面

- 架构决策有据可查，新成员可快速理解决策背景
- 避免同一问题反复讨论
- 决策变更时可通过 Superseded 关系追溯演进路径

### 负面

- 编写 ADR 增加少量文档开销
- 需要维护 ADR 编号的连续性

### 中性

- ADR 记录决策而非实现细节，代码仍是唯一事实来源

## 参考

- [Documenting Architecture Decisions — Michael Nygard](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions)
- [ADR GitHub Organization](https://adr.github.io/)
