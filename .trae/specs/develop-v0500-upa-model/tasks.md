# Tasks

- [x] Task 1: 同步 workspace 版本号与 members 列表
  - [x] 修改 `e:\eneros\Cargo.toml`：`version = "0.49.0"` → `version = "0.50.0"`
  - [x] 在 members 数组中 `"crates/protocols/iec104-master"` 之后增加 `"crates/protocols/upa-model"`
  - 验证：`cargo metadata --format-version 1` 成功

- [x] Task 2: 创建 crate 骨架（Cargo.toml + lib.rs）
  - [x] 创建 `e:\eneros\crates\protocols\upa-model\Cargo.toml`
    - package name = `eneros-upa-model`，workspace 继承
    - **零 dependencies**（D6）
  - [x] 创建 `e:\eneros\crates\protocols\upa-model\src\lib.rs`
    - `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`
    - 模块声明：`pub mod point; pub mod database;`
    - D1~D9 偏差声明表（doc comment）
    - 重导出公共 API
  - 验证：`cargo build -p eneros-upa-model` 编译通过

- [x] Task 3: 实现 point 模块（DataPoint + 所有类型）
  - [x] 创建 `src/point.rs`
    - `PointId = u32` 类型别名
    - `DeviceId = u16` 类型别名
    - `PointType` 枚举（Analog/Digital/Control/Setpoint/Counter），派生 Debug/Clone/Copy/PartialEq/Eq/Hash
    - `PointValue` 枚举（Float(f64)/Int(i64)/Bool(bool)/Enum(u16)/String(String)/Null），派生 Debug/Clone/PartialEq（不派生 Eq，D7）
    - `PointQuality` 结构体（valid/invalid/questionable/substituted/overflow/blocked/outdated，全 bool），派生 Debug/Clone/Copy/PartialEq/Eq/Default
    - `PointQuality::good()` / `PointQuality::invalid()` 构造函数
    - `DataSource` 枚举（ModbusRtu/ModbusTcp/Iec104/Can/Internal/Manual），派生 Debug/Clone/Copy/PartialEq/Eq
    - `DataPoint` 结构体（point_id/device_id/name/description/point_type/value/quality/timestamp_ms/source/unit），派生 Debug/Clone
  - 验证：编译通过

- [x] Task 4: 实现 database 模块（PointDatabase）
  - [x] 创建 `src/database.rs`
    - `PointDatabase` 结构体：
      - points: `BTreeMap<PointId, DataPoint>`
      - device_index: `BTreeMap<DeviceId, Vec<PointId>>`
      - type_index: `BTreeMap<PointType, Vec<PointId>>`
      - name_index: `BTreeMap<String, PointId>`
      - next_id: `u32`（D3 普通自增字段）
    - 方法实现：
      - `new() -> Self` — 空数据库
      - `register(&mut self, device_id, name: &str, point_type, now_ms: u64) -> PointId` — 注册新点，返回分配的 ID
      - `update(&mut self, point_id, value, quality, now_ms: u64) -> bool` — 更新点值，返回是否成功
      - `get_by_id(&self, point_id) -> Option<&DataPoint>`
      - `get_by_device(&self, device_id) -> Vec<&DataPoint>`
      - `get_by_type(&self, point_type) -> Vec<&DataPoint>`
      - `get_by_name(&self, name: &str) -> Option<&DataPoint>`
      - `remove(&mut self, point_id) -> bool` — 删除点，同步清理所有索引
      - `count(&self) -> usize`
      - `list_all(&self) -> Vec<&DataPoint>`
    - 模块内单元测试（`#[cfg(test)] mod tests`）
  - 验证：编译通过 + 单元测试通过

- [x] Task 5: 集成测试
  - [x] 在 `src/lib.rs` 的 `#[cfg(test)] mod tests` 中编写跨模块集成测试：
    - 测试 1：注册点 → 验证返回 ID + 初始值 Null + 品质 invalid
    - 测试 2：更新点值 → 验证 value/quality/timestamp 更新
    - 测试 3：按 ID 查询存在/不存在的点
    - 测试 4：按设备查询 → 多点返回
    - 测试 5：按类型查询 → 过滤正确
    - 测试 6：按名称查询 → 精确匹配
    - 测试 7：删除点 → 主存储 + 所有索引清理
    - 测试 8：PointValue 六种类型构造与比较
    - 测试 9：PointQuality::good() / invalid() 构造
    - 测试 10：点 ID 自增（注册 3 个点，ID 为 0/1/2）
    - 测试 11：重复名称注册（name_index 覆盖）
    - 测试 12：count() / list_all()
  - 验证：`cargo test -p eneros-upa-model` 全部通过

- [x] Task 6: 编写设计文档
  - [x] 创建 `e:\eneros\docs\protocols\upa-model-design.md`
    - 章节：1.概述 / 2.架构 / 3.核心类型 / 4.PointDatabase / 5.索引设计 / 6.值类型 / 7.品质标志 / 8.数据来源 / 9.no_std合规 / 10.测试策略 / 11.与协议栈的关系 / 12.偏差声明
    - 包含 Mermaid 架构图（UPA 在协议栈中的位置）
    - 包含类型关系图
  - 验证：文档位置在 `docs/protocols/` 下（C4 校验）

- [x] Task 7: 更新 Makefile / ci.yml / gate.rs 版本号
  - [x] `e:\eneros\Makefile`：0.49.0 → 0.50.0
  - [x] `e:\eneros\.github\workflows\ci.yml`：0.49.0 → 0.50.0
  - [x] `e:\eneros\ci\src\gate.rs`：补充 v0.50.0 upa-model 注释
  - 验证：版本号已同步

- [x] Task 8: 构建校验（C6~C11）
  - [x] `cargo metadata --format-version 1` — workspace 解析成功
  - [x] `cargo test -p eneros-upa-model` — 23 测试全部通过
  - [x] `cargo build -p eneros-upa-model --target aarch64-unknown-none -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem` — 交叉编译通过
  - [x] `cargo fmt -p eneros-upa-model -- --check` — 格式检查通过
  - [x] `cargo clippy -p eneros-upa-model --all-targets -- -D warnings` — lint 通过
  - [x] `cargo deny check advisories licenses bans sources` — 已知 GitHub 网络问题（advisory-db 无法获取）

# Task Dependencies
- Task 1 独立（workspace 准备）
- Task 2 依赖 Task 1（crate 骨架）
- Task 3 依赖 Task 2（point 模块）
- Task 4 依赖 Task 3（database 依赖 point 类型）
- Task 5 依赖 Task 3 + Task 4（集成测试依赖 point + database）
- Task 6 依赖 Task 4（文档依赖实现完成）
- Task 7 独立（版本号同步）
- Task 8 依赖全部完成
