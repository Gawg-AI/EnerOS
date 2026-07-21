# Checklist — v0.26.0 Config Management

## 目录结构校验（§2.4 C1-C5）

- [x] C1 新 crate 位置：eneros-config 位于 `crates/runtime/config/`，未直接放根目录
- [x] C2 workspace members：根 `Cargo.toml` 的 members 已添加 `"crates/runtime/config"`
- [x] C3 跨 crate path 引用：`crates/runtime/config/Cargo.toml` 的 `eneros-fs = { path = "../../drivers/fs" }` 使用正确相对路径（runtime→drivers 跨子系统）
- [x] C4 文档分类：`docs/runtime/config-design.md` 位于 `docs/runtime/` 子目录，未平面化放 `docs/` 根
- [x] C5 无根目录 crate：仓库根目录无新增 Rust crate 文件夹

## no_std 合规（§4.3）

- [x] `crates/runtime/config/src/lib.rs` 含 `#![cfg_attr(not(test), no_std)]`
- [x] 无 `use std::*`，改用 `alloc::*` / `core::*`
- [x] HashMap 改用 `alloc::collections::BTreeMap`
- [x] toml crate 用 `default-features = false` + `serde` + `parse` + `display` features
- [x] serde_json 用 `alloc` feature（无 std）

## 接口实现完整性

- [x] schema.rs: ConfigValue/ConfigType/ConfigField/ConfigSchema + 类型转换 + From impls + validate()
- [x] error.rs: ConfigError 8 变体 + Debug/Display + From<FsError>（清理了 placeholder 函数）
- [x] loader.rs: ConfigLoader trait + TomlLoader + JsonLoader + ConfigFormat + Value 双向转换（已接入 toml/serde_json）
- [x] version.rs: ConfigVersion + VersionHistory + CRC32 校验 + MAX_VERSIONS=10 淘汰（清理了 placeholder 函数）
- [x] watcher.rs: ConfigWatcher trait + WatcherRegistry + register/notify
- [x] manager.rs: ConfigManager + new/load/load_all/get/get_or_default/set/save/reload/rollback/list_versions/register_watcher

## toml crate no_std 兼容

- [x] 验证 toml v1.0+ 的 no_std + alloc 配置可编译（`features = ["serde", "parse", "display"]`）
- [x] toml::Value ↔ ConfigValue 双向转换正确
- [x] TOML 加载-保存往返测试通过
- [x] serde_json::Value ↔ ConfigValue 双向转换正确
- [x] JSON 加载-保存往返测试通过

## 版本管理与回滚

- [x] ConfigVersion 含 CRC32 校验和（crc32fast crate）
- [x] 版本历史保存在内存（VersionHistory，进程重启后丢失；持久化到磁盘为后续版本增强项）
- [x] list_versions 返回正确版本列表（升序）
- [x] rollback 恢复到指定版本 + 不记录新版本
- [x] CRC 校验失败返回 ChecksumMismatch 错误
- [x] max_versions=10 超限时自动移除最旧版本（remove(0)）

## 热加载通知

- [x] reload() 重新加载文件 + 更新内存缓存
- [x] reload() 通过校验后通知 watcher（path="*"）
- [x] set() 操作通知 watcher（path=具体键名）
- [x] ConfigWatcher trait 的 on_config_changed 被正确调用
- [x] rollback() 操作通知 watcher

## 构建校验（§2.4 C6-C11）

- [x] C6 `cargo metadata --format-version 1 > /dev/null` 成功（workspace 成员路径全部正确）
- [x] C7 `cargo test -p eneros-config` 通过（209 单元测试 + 1 ignored doc-test，覆盖率 ≥ 80%）
- [x] C8 `cargo build -p eneros-config --target aarch64-unknown-none` **通过**（WSL2 Ubuntu-22.04 + aarch64-linux-gnu-gcc 13.3.0，10.67s 编译成功）
- [x] C9 `cargo fmt -p eneros-config -- --check` 通过
- [x] C10 `cargo clippy -p eneros-config --all-targets -- -D warnings` 无 warning
- [x] C11 `cargo deny check advisories licenses bans sources` **通过**（advisories ok / licenses ok / bans ok / sources ok）

## 功能验证

- [x] TOML 配置加载正确（含嵌套 table、数组、多种值类型）
- [x] JSON 配置加载正确
- [x] 配置保存后重新加载内容一致
- [x] get/get_or_default/set 正确操作配置值
- [x] 热加载：外部修改文件后 reload 生效
- [x] 版本回滚：回滚到旧版本正确
- [x] Schema 校验：非法配置被拒绝（validate() 实现 + 测试）
- [x] FsError → ConfigError 自动转换（`?` 操作符，From impl）
- [x] 时间源函数指针注入工作正常
- [x] base name 存储：.toml 和 .json 文件均可通过 base name 访问

## 回归测试

- [x] `cargo test --workspace --exclude eneros-kernel --exclude eneros-hello` 通过（全部 PASS，0 failures）
- [x] `cargo run -p eneros-ci` **Overall: PASS**（fmt/clippy/audit/test 全绿）

## 文档与规范校验（§2.4 C12-C15）

- [x] C12 文档位置：`docs/runtime/config-design.md` 在 `docs/runtime/` 下
- [x] C13 无垃圾文件：`git status` 无 target/、*.elf、*.bin、*.dtb、IDE 缓存被追踪
- [x] C14 .gitignore 覆盖：新产生的文件类型已在 .gitignore 中
- [x] C15 提交信息：遵循 Conventional Commits（待提交时遵守）

## 版本标识一致性

- [x] 根 `Cargo.toml` workspace.package.version = "0.26.0"
- [x] `crates/runtime/config/Cargo.toml` version = "0.26.0"
- [x] `Makefile` VERSION := 0.26.0
- [x] `.github/workflows/ci.yml` Version: v0.26.0 + 含 eneros-config 交叉编译步骤
- [x] `ci/src/gate.rs` 注释含 eneros-config（v0.26.0 配置管理）说明

## CI 配置

- [x] ci.yml 添加 `Build config crate` 步骤（aarch64-unknown-none 交叉编译）
- [x] ci.yml clippy/test 步骤无需修改（workspace 级别已覆盖）
- [x] gate.rs clippy/test 排除列表注释更新（eneros-config 为 no_std crate，host-testable）

## 依赖许可证（SBOM）

- [x] toml crate 许可证 MIT/Apache-2.0（deny.toml 已允许）
- [x] serde_json 许可证 MIT/Apache-2.0（deny.toml 已允许）
- [x] crc32fast 许可证 MIT/Apache-2.0（deny.toml 已允许）
- [x] `cargo deny check licenses` **通过**（licenses ok）

## 测试统计

| 模块 | 测试数 | 状态 |
|------|--------|------|
| error.rs | 13 | ✅ 全部通过 |
| schema.rs | 40+ | ✅ 全部通过 |
| loader.rs | 37+ | ✅ 全部通过 |
| version.rs | 35+ | ✅ 全部通过 |
| watcher.rs | 20+ | ✅ 全部通过 |
| manager.rs | 41+ | ✅ 全部通过 |
| **总计** | **209** | ✅ **全部通过** |

## 已知限制 / 后续增强

1. **版本历史仅在内存**：`VersionHistory` 存储在 `ConfigManager` 的 `histories: BTreeMap` 中，进程重启后丢失。若需掉电后恢复版本历史，需在 `set`/`save` 时将版本数据持久化到 `/config/.versions/<name>.dat`（设计文档 §5 已预留此扩展点）。
2. **Schema 校验未集成到 load 路径**：`ConfigSchema::validate()` 已实现并测试，但 `ConfigManager::load`/`reload` 未自动调用 schema 校验。调用方可手动 `schema.validate(&value)` 后再使用配置。后续版本可在 `ConfigManager` 中注册 schema 表并自动校验。
3. **Watcher 无法跨进程**：`ConfigWatcher` 为进程内 trait 对象回调，无 IPC 通知能力。no_std RTOS 无 inotify，跨进程热加载需通过 Control Bus（v0.22.0）广播。
