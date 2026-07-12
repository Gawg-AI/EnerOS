# DMA 保护方案

> **版本**：EnerOS v0.9.0 — 分区内存隔离验证
> **模块**：`mm/src/dma_guard.rs`
> **最后更新**：2026-07-12

---

## 1. 概述

### 1.1 DMA 威胁模型

DMA（Direct Memory Access，直接内存访问）允许外设绕过 CPU 直接读写系统物理内存。这一机制在提升 I/O 吞吐的同时，也引入了严重的安全威胁：

- **DMA 绕过 CPU 隔离**：CPU 的内存隔离依赖页表（虚拟地址 → 物理地址映射 + 权限位），但 DMA 不经过 CPU 页表，可直接以物理地址访问内存。
- **恶意/被入侵设备**：一个被入侵的 PCIe 设备、网卡或 USB 控制器可通过 DMA 读取内核代码、密钥、其他分区的数据，绕过所有软件层面的内存隔离。
- **配置错误**：设备固件 bug 可能导致 DMA 写入错误的物理地址，破坏其他分区内存。

### 1.2 保护目标

EnerOS 作为混合关键性系统，必须保证：

| 目标 | 说明 |
|------|------|
| 设备隔离 | 每个设备只能 DMA 访问其所属分区授权的物理区间 |
| 分区隔离 | 设备无法通过 DMA 跨分区访问（与 `Partition` 的物理隔离一致） |
| 可审计 | 每次 DMA 授权与检查可追溯 |
| 硬件支持 | 最终依赖 SMMU/IOMMU 硬件强制执行 |

### 1.3 v0.9.0 实现状态

v0.9.0 的 `DmaGuard`/`SmmuGuard` 是**软件层面的检查 stub**：

- 不配置 SMMU 硬件寄存器，仅在软件维护设备→物理区间的授权表。
- 用于验证授权/检查接口的正确性与集成方式。
- v0.22.0+ 将实现真正的 SMMUv3 硬件配置。

---

## 2. SMMU/IOMMU 概述

### 2.1 ARM SMMUv3

ARM SMMU（System Memory Management Unit）v3 是 ARMv8-A 架构的 IOMMU 实现，为 DMA 提供地址翻译与权限检查：

```
设备 ──DMA 请求──▶ SMMUv3 ──翻译+检查──▶ 物理内存
                    │
                    ├── Stream Table: 设备 → 域（ASID/VMID）
                    ├── Page Tables: IOVA → PA 翻译
                    └── 权限检查: R/W/Exec 权限
```

### 2.2 工作机制

1. **Stream ID 识别**：每个设备有唯一的 Stream ID，SMMU 据此查找 Stream Table Entry（STE）。
2. **域绑定**：STE 指向该设备所属的保护域（Translation Context），包含页表基址与权限。
3. **地址翻译**：DMA 请求的 IOVA（I/O Virtual Address）经页表翻译为 PA（Physical Address）。
4. **权限检查**：SMMU 检查该 PA 是否在页表允许范围内，权限是否匹配（读/写）。
5. **违规终止**：未授权访问被 SMMU 终止，触发 fault 中断。

### 2.3 与分区隔离的关系

SMMU 是 `Partition` 物理隔离的硬件强制层：即使软件检查被绕过，SMMU 硬件仍会阻止设备越权 DMA。v0.9.0 的 `SmmuGuard` 软件检查是 SMMU 硬件机制的预演与补充。

---

## 3. DmaGuard trait 接口

### 3.1 trait 定义

```rust
pub trait DmaGuard {
    /// Authorize a device to access a physical address range.
    fn authorize(&mut self, dev: DeviceId, range: PaddrRange) -> Result<(), MmError>;
    /// Check whether a device is authorized to access `pa`.
    fn check(&self, dev: DeviceId, pa: u64) -> Result<(), MmError>;
}
```

### 3.2 方法语义

| 方法 | 签名 | 语义 | 成功返回 | 失败返回 |
|------|------|------|---------|---------|
| `authorize` | `&mut self, dev, range` | 授权设备 `dev` 可 DMA 访问物理区间 `range` | `Ok(())` | `OutOfMemory`（域表已满） |
| `check` | `&self, dev, pa` | 检查设备 `dev` 是否被授权访问物理地址 `pa` | `Ok(())` | `PermissionDenied`（未授权） |

### 3.3 设计要点

- **不可变检查**：`check` 接收 `&self`，表明检查操作不修改状态，可在只读上下文中调用。
- **可变授权**：`authorize` 接收 `&mut self`，独占修改授权表。
- **trait 抽象**：允许未来有多种实现（SMMUv3、IOMMU、软件 stub），上层代码面向 trait 编程。

---

## 4. SmmuGuard 实现

### 4.1 结构

```rust
const MAX_DMA_DOMAINS: usize = 16;

pub struct SmmuGuard {
    /// DMA protection domains (None = empty slot).
    pub domains: [Option<(DeviceId, DmaDomain)>; MAX_DMA_DOMAINS],
}
```

- `domains`：固定 16 槽数组，每槽为 `Option<(DeviceId, DmaDomain)>`。
- `None` 表示空槽，`Some((dev, domain))` 表示该设备已授权。
- 16 槽满足 RTOS 场景下外设数量需求，避免动态分配（no_std 友好）。

### 4.2 构造

```rust
impl SmmuGuard {
    pub fn new() -> Self {
        Self {
            domains: [None; MAX_DMA_DOMAINS],
        }
    }
}

impl Default for SmmuGuard {
    fn default() -> Self {
        Self::new()
    }
}
```

初始化所有 16 个槽为 `None`。

### 4.3 authorize 实现

```rust
impl DmaGuard for SmmuGuard {
    fn authorize(&mut self, dev: DeviceId, range: PaddrRange) -> Result<(), MmError> {
        // 第一遍：若设备已授权，替换其域
        for slot in self.domains.iter_mut() {
            if let Some((id, _)) = slot {
                if *id == dev {
                    *slot = Some((
                        dev,
                        DmaDomain {
                            owner_partition: dev.0,
                            allowed_phys: range,
                        },
                    ));
                    return Ok(());
                }
            }
        }

        // 第二遍：写入首个空槽
        for slot in self.domains.iter_mut() {
            if slot.is_none() {
                *slot = Some((
                    dev,
                    DmaDomain {
                        owner_partition: dev.0,
                        allowed_phys: range,
                    },
                ));
                return Ok(());
            }
        }

        Err(MmError::OutOfMemory)   // 16 槽已满
    }
    // ...
}
```

**授权策略**：
1. **替换已有授权**：若设备 `dev` 已存在授权条目，用新 `range` 替换旧的（每设备仅保留一个域）。
2. **写入空槽**：若设备未授权，写入首个 `None` 槽。
3. **表满失败**：16 槽均已使用且无该设备条目，返回 `OutOfMemory`。

> **注意**：`owner_partition` 当前直接取 `dev.0`（即 `DeviceId` 的内层 `u32`）。这假设设备 ID 与分区 ID 一一对应，未来版本将支持显式指定 `owner_partition`。

### 4.4 check 实现

```rust
fn check(&self, dev: DeviceId, pa: u64) -> Result<(), MmError> {
    for slot in self.domains.iter() {
        if let Some((id, domain)) = slot {
            if *id == dev && domain.allowed_phys.contains(pa) {
                return Ok(());
            }
        }
    }
    Err(MmError::PermissionDenied)
}
```

**检查逻辑**：线性扫描 `domains`，匹配 `DeviceId` 且 `pa` 落在 `allowed_phys` 内即通过；否则返回 `PermissionDenied`。

- **线性扫描**：O(16) 复杂度，对 16 槽规模可接受。
- **点检查**：`contains(pa)` 只检查单点，不检查区间（DMA 检查逐地址进行）。

---

## 5. DeviceId 与 DmaDomain

### 5.1 DeviceId

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceId(pub u32);
```

- **newtype 模式**：包装 `u32`，提供类型安全，避免与普通 `u32` 混淆。
- `pub` 字段：允许直接访问 `dev.0`。
- `PartialEq`/`Eq`：支持 `==` 比较，用于 `authorize`/`check` 中匹配设备。
- `Clone`/`Copy`：值类型，可低成本复制（32 位）。

### 5.2 DmaDomain

```rust
#[derive(Clone, Copy, Debug)]
pub struct DmaDomain {
    /// The partition that owns this device.
    pub owner_partition: u32,
    /// The physical address range the device is allowed to access.
    pub allowed_phys: PaddrRange,
}
```

| 字段 | 类型 | 说明 |
|------|------|------|
| `owner_partition` | `u32` | 该设备所属的分区 ID（用于审计与跨分区检查） |
| `allowed_phys` | `PaddrRange` | 设备被授权 DMA 访问的物理区间 `[start, end)` |

**保护域语义**：一个 `DmaDomain` 绑定一个设备与一段物理区间，确保设备 DMA 只能触及所属分区的内存。

---

## 6. 授权流程

### 6.1 流程图

```
        ┌────────────────────────────────┐
        │ authorize(dev, range)         │
        └───────────────┬────────────────┘
                        │
                        ▼
        ┌────────────────────────────────┐
        │ 第一遍扫描：查找 dev 已有授权 │
        └───────────────┬────────────────┘
                        │
                ┌───────┴───────┐
                │ 找到 dev      │ 未找到
                ▼               │
        ┌──────────────┐        │
        │ 替换为       │        │
        │ (dev, new    │        │
        │  domain)     │        │
        └──────┬───────┘        │
               │ Ok(())         │
               ▼                ▼
        ┌────────────────────────────────┐
        │ 第二遍扫描：查找首个 None 槽  │
        └───────────────┬────────────────┘
                        │
                ┌───────┴───────┐
                │ 找到空槽      │ 无空槽
                ▼               │
        ┌──────────────┐        │
        │ 写入         │        │
        │ (dev, new    │        │
        │  domain)     │        │
        └──────┬───────┘        │
               │ Ok(())         │
               ▼                ▼
        ┌────────────┐  ┌────────────────┐
        │  完成      │  │ OutOfMemory    │
        └────────────┘  └────────────────┘
```

### 6.2 软件 stub 说明

v0.9.0 的 `authorize` **仅写入软件数组**，不配置 SMMU 硬件：

- 不写 SMMU Stream Table。
- 不分配 SMMU 页表。
- 不设置 SMMU 寄存器。

这是为了在硬件支持就绪前验证接口设计与软件流程。v0.22.0+ 将在 `authorize` 中加入：

1. SMMU STE 配置：将设备 Stream ID 绑定到域。
2. SMMU 页表构建：为设备构建 IOVA→PA 映射。
3. TLB 刷新：使新授权立即生效。

---

## 7. 检查流程

### 7.1 流程图

```
        ┌────────────────────────────────┐
        │ check(dev, pa)                │
        └───────────────┬────────────────┘
                        │
                        ▼
        ┌────────────────────────────────┐
        │ 遍历 domains 数组（0..16）     │
        └───────────────┬────────────────┘
                        │
                        ▼
              ┌──────────────────┐
              │ 槽为 Some?       │
              └────────┬─────────┘
                ┌──────┴──────┐
                │ 否          │ 是
                │             ▼
                │   ┌──────────────────────────┐
                │   │ id == dev 且             │
                │   │ domain.allowed_phys      │
                │   │   .contains(pa) ?        │
                │   └────────────┬─────────────┘
                │         ┌──────┴──────┐
                │         │ 是          │ 否
                │         ▼             │
                │  ┌──────────┐         │
                │  │ Ok(())   │         │
                │  └──────────┘         │
                │                       │
                ▼ (继续下一槽)          ▼ (继续下一槽)
        ┌────────────────────────────────┐
        │ 遍历结束，无匹配              │
        └───────────────┬────────────────┘
                        │
                        ▼
              ┌──────────────────────┐
              │ PermissionDenied     │
              └──────────────────────┘
```

### 7.2 检查规则

| 条件 | 结果 |
|------|------|
| 设备 `dev` 在 `domains` 中且 `pa` 在其 `allowed_phys` 内 | `Ok(())` |
| 设备 `dev` 在 `domains` 中但 `pa` 不在 `allowed_phys` 内 | `PermissionDenied` |
| 设备 `dev` 不在 `domains` 中（未授权） | `PermissionDenied` |
| `domains` 为空（无任何授权） | `PermissionDenied` |

### 7.3 与 Partition 检查的配合

`SmmuGuard::check` 与 `Partition::check_access` 是互补的两层检查：

| 层 | 检查对象 | 拒绝错误 |
|----|---------|---------|
| `Partition::check_access` | CPU 发起的物理访问 | `PermissionDenied` |
| `SmmuGuard::check` | 设备发起的 DMA 访问 | `PermissionDenied` |

两者共同确保：无论 CPU 还是设备，都无法越权访问其他分区的物理内存。

---

## 8. 与 seL4 capability 的关系

### 8.1 seL4 capability 机制

seL4 微内核的 capability 机制是其安全模型的核心：

- **能力即授权**：所有内核操作（映射页、发送 IPC、授权 DMA）都需持有对应 capability。
- **能力传递**：capability 只能通过显式的 `Mint`/`Copy`/`Move` 操作传递，无法伪造。
- **能力空间**：每个线程有独立的 CSpace（capability space），只能访问自己持有的能力。

### 8.2 seL4 对 DMA 的原生支持

seL4 通过以下机制支持 DMA 隔离：

- **SMMU capability**：seL4 将 SMMU 设备抽象为 capability，只有持有 SMMU cap 的线程才能配置 SMMU。
- **设备 DMA 控制**：设备驱动需持有该设备的 SMMU 配置 capability 才能授权 DMA。
- **硬件强制**：seL4 内核配置 SMMU 硬件，强制所有 DMA 经过 SMMU 检查。

### 8.3 SmmuGuard 的定位

`SmmuGuard` 是 **seL4 capability 机制之上的软件层补充检查**：

```
┌─────────────────────────────────────┐
│ 应用 / Agent                        │
├─────────────────────────────────────┤
│ SmmuGuard（软件检查层）              │  ← 本层：v0.9.0
│  · authorize / check 接口            │
│  · 软件维护设备→区间映射             │
├─────────────────────────────────────┤
│ seL4 SMMU capability（内核层）       │  ← 硬件强制：seL4 内核
│  · SMMU 配置 capability              │
│  · Stream Table / 页表               │
├─────────────────────────────────────┤
│ SMMUv3 硬件                          │  ← 硬件层
│  · 地址翻译 + 权限检查               │
└─────────────────────────────────────┘
```

| 层 | 作用 | v0.9.0 状态 |
|----|------|-----------|
| SmmuGuard | 软件层授权记录与检查 | 已实现（stub） |
| seL4 capability | 内核级能力管控 | seL4 已提供 |
| SMMUv3 硬件 | 硬件强制隔离 | v0.22.0+ 配置 |

**关系总结**：seL4 capability 天然支持 DMA 隔离（阻止未授权线程配置 SMMU），`SmmuGuard` 在用户态/管理信息大区提供额外的软件检查层，用于快速拒绝非法 DMA 请求、记录审计日志，减少陷入内核的开销。

---

## 9. 使用示例

### 9.1 完整示例

```rust
use mm::dma_guard::{DmaGuard, SmmuGuard, DeviceId};
use mm::partition::PaddrRange;

fn main() {
    // 1. 创建 SMMU 保护域管理器
    let mut guard = SmmuGuard::new();

    // 2. 定义两个设备
    let net_dev = DeviceId(1);   // 网卡，属分区 1
    let blk_dev = DeviceId(2);   // 块设备，属分区 2

    // 3. 授权网卡只能 DMA 访问分区 1 的物理区间 [0x40000000, 0x40100000)
    guard.authorize(
        net_dev,
        PaddrRange::new(0x40000000, 0x40100000),
    ).expect("授权网卡失败");

    // 4. 授权块设备只能 DMA 访问分区 2 的物理区间 [0x40200000, 0x40400000)
    guard.authorize(
        blk_dev,
        PaddrRange::new(0x40200000, 0x40400000),
    ).expect("授权块设备失败");

    // 5. 检查网卡访问其授权区间内的地址 —— 通过
    assert!(guard.check(net_dev, 0x40000000).is_ok());  // 起始
    assert!(guard.check(net_dev, 0x40000FFF).is_ok());  // 区间内
    assert!(guard.check(net_dev, 0x400FFFFF).is_ok());  // 末字节（end-1）

    // 6. 检查网卡访问授权区间外地址 —— 拒绝
    assert!(guard.check(net_dev, 0x40100000).is_err()); // end（越界）
    assert!(guard.check(net_dev, 0x3FFFFFFF).is_err()); // start-1（越界）

    // 7. 检查网卡访问分区 2 的内存 —— 拒绝（跨分区 DMA）
    assert!(guard.check(net_dev, 0x40200000).is_err());

    // 8. 检查未授权设备 —— 拒绝
    let unknown_dev = DeviceId(99);
    assert!(guard.check(unknown_dev, 0x40000000).is_err());

    // 9. 替换网卡授权区间
    guard.authorize(
        net_dev,
        PaddrRange::new(0x50000000, 0x50100000),
    ).expect("替换网卡授权失败");
    // 旧区间不再可访问
    assert!(guard.check(net_dev, 0x40000000).is_err());
    // 新区间可访问
    assert!(guard.check(net_dev, 0x50000000).is_ok());

    println!("DMA 保护示例全部通过");
}
```

### 9.2 关键场景说明

| 场景 | 预期结果 | 说明 |
|------|---------|------|
| 设备访问授权区间内地址 | `Ok(())` | 正常 DMA |
| 设备访问授权区间边界外 | `PermissionDenied` | 越界 DMA |
| 设备访问其他分区内存 | `PermissionDenied` | 跨分区 DMA |
| 未授权设备访问 | `PermissionDenied` | 未注册设备 |
| 替换设备授权 | 旧区间失效，新区间生效 | 每设备仅一个域 |

---

## 10. 未来扩展

| 版本 | 扩展内容 | 说明 |
|------|---------|------|
| v0.22.0 | SMMUv3 硬件配置 | `authorize` 中配置 SMMU Stream Table、构建 IOVA→PA 页表、刷新 TLB，实现硬件强制 DMA 隔离 |
| v0.22.0+ | 动态域管理 | 运行时添加/删除 DMA 域，支持设备热插拔；`revoke(dev)` 撤销设备授权 |
| v0.22.0+ | DMA 违规日志 | SMMU fault 中断处理程序记录违规设备 ID、目标地址、时间戳，供安全审计 |
| 后续 | 多区间授权 | 单设备授权多个不连续物理区间（当前仅 1 个 `PaddrRange`），支持设备访问分散缓冲区 |
| 后续 | IOVA 翻译 | 设备使用 IOVA 而非 PA 发起 DMA，SMMU 翻译 IOVA→PA，提供更细粒度的隔离 |
| 后续 | DMA 带宽限流 | 按设备/分区限制 DMA 带宽，防止单设备耗尽内存带宽 |
| 后续 | 与 Partition 联动 | `authorize` 时自动校验 `range` 是否在 `owner_partition` 的 `allowed_phys` 内，确保 DMA 授权不超出分区物理边界 |

---

## 11. 参考资料

- 源码：`mm/src/dma_guard.rs`
- 分区隔离：`docs/partition-isolation-design.md`
- 错误码：`mm/src/vspace.rs` (`MmError`)
- 蓝图：`蓝图/phase0.md`（Phase 0 内存管理）
- 路线图：`蓝图/Power_Native_Agent_OS_Version_Roadmap_v3.md`（v0.9.0、v0.22.0）
- ARM SMMUv3 规范：ARM IHI 0070
- seL4 手册：SMMU/Capability 章节
