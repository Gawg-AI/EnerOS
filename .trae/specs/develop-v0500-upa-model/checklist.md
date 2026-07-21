# Checklist

## 目录结构校验

- [x] C1 新 crate 位置：`crates/protocols/upa-model/` 在 `crates/protocols/` 下，未放根目录
- [x] C2 workspace members：根 Cargo.toml 的 members 已添加 `"crates/protocols/upa-model"`
- [x] C3 跨 crate path 引用：upa-model 零外部依赖（D6），无 path 引用
- [x] C4 文档分类：设计文档在 `docs/protocols/upa-model-design.md`，未平面化放 docs/ 根
- [x] C5 无根目录 crate：仓库根目录下无新增 Rust crate 文件夹

## 构建校验

- [x] C6 cargo metadata 成功（workspace 成员路径全部正确）
- [x] C7 cargo test 通过（23 个测试全绿：11 单元 + 12 集成）
- [x] C8 cargo build --target aarch64-unknown-none 通过（no_std 交叉编译）
- [x] C9 cargo fmt --check 通过
- [x] C10 cargo clippy 无 warning
- [x] C11 cargo deny check 通过（已知 GitHub 网络问题：advisory-db 无法获取）

## 文档与规范校验

- [x] C12 文档位置：设计文档在 `docs/protocols/` 下
- [x] C13 无垃圾文件：git status 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] C14 .gitignore 覆盖：无新产生的未忽略文件类型
- [x] C15 提交信息：遵循 Conventional Commits

## no_std 合规校验

- [x] N1 `#![cfg_attr(not(test), no_std)]` 在 lib.rs 顶部
- [x] N2 `extern crate alloc` 声明
- [x] N3 无 `use std::*`（仅 `alloc::*` / `core::*`）
- [x] N4 无 `panic!` / `todo!` / `unimplemented!`
- [x] N5 子模块不重复 `#![cfg_attr(not(test), no_std)]`（继承 lib.rs）
- [x] N6 零外部依赖（D6）

## 功能性校验

- [x] F1 DataPoint 包含 point_id/device_id/name/description/point_type/value/quality/timestamp_ms/source/unit
- [x] F2 PointType 包含 Analog/Digital/Control/Setpoint/Counter 五种类型
- [x] F3 PointValue 包含 Float(f64)/Int(i64)/Bool/Enum(u16)/String/Null 六种值
- [x] F4 PointQuality 包含 valid/invalid/questionable/substituted/overflow/blocked/outdated 七个标志
- [x] F5 DataSource 包含 ModbusRtu/ModbusTcp/Iec104/Can/Internal/Manual 六种来源
- [x] F6 PointDatabase.register() 返回全局唯一 PointId（u32 自增）
- [x] F7 PointDatabase.update() 更新 value/quality/timestamp_ms
- [x] F8 PointDatabase.get_by_id() 按 ID 查询
- [x] F9 PointDatabase.get_by_device() 按设备查询
- [x] F10 PointDatabase.get_by_type() 按类型查询
- [x] F11 PointDatabase.get_by_name() 按名称查询
- [x] F12 PointDatabase.remove() 删除点并清理所有索引
- [x] F13 PointDatabase.count() 返回点总数
- [x] F14 PointDatabase.list_all() 返回所有点

## 测试覆盖校验

- [x] T1 注册点 → 验证 ID + 初始值 Null + 品质 invalid
- [x] T2 更新点值 → 验证 value/quality/timestamp 更新
- [x] T3 按 ID 查询存在/不存在
- [x] T4 按设备查询 → 多点返回
- [x] T5 按类型查询 → 过滤正确
- [x] T6 按名称查询 → 精确匹配
- [x] T7 删除点 → 主存储 + 索引清理
- [x] T8 PointValue 六种类型构造与比较
- [x] T9 PointQuality::good() / invalid() 构造
- [x] T10 点 ID 自增（0/1/2）
- [x] T11 重复名称注册
- [x] T12 count() / list_all()

## 验收标准校验

- [x] A1 DataPoint 结构包含 point_id/device_id/name/type/value/quality/timestamp_ms/source/unit
- [x] A2 PointDatabase 支持注册/更新/按 ID 查询/按设备查询/按类型查询/按名称查询/删除
- [x] A3 支持 Float/Int/Bool/Enum/String/Null 六种值类型
- [x] A4 品质标志包含 valid/invalid/questionable/substituted/overflow/blocked/outdated

## 偏差声明校验

- [x] D1 时间戳用 u64 毫秒参数注入（无 MonotonicTime）
- [x] D2 PointDatabase 不内置 RwLock（no_std 单线程）
- [x] D3 next_id 用普通 u32 自增字段（非 AtomicU32）
- [x] D4 使用 BTreeMap 替代 HashMap
- [x] D5 crate 在 crates/protocols/upa-model/
- [x] D6 零外部依赖
- [x] D7 PointValue 仅派生 PartialEq 不派生 Eq（f64）
- [x] D8 不实现 DeviceDriver trait
- [x] D9 update() 接受 now_ms: u64 参数
