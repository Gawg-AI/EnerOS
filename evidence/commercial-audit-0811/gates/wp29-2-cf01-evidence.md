# WP-29.2 配置事实化（CF-01）修复与证据

执行日期：2026-08-11。蓝图坐标：29 §4（CF-01，P0）。

## 三步修复（顺序不可反）

1. **语法门禁先行**：`xtask/src/gate.rs` 新增 `command_config_syntax`（gate
   分发新增子命令）。host 侧 toml crate 解析 configs/** 全部文件（递归含
   compliance/、power/、time/ 子目录），语法错 FAIL。xtask/Cargo.toml 增
   toml = "0.8" 依赖。
   - 首跑：82 文件 checked，errors 0（存量语法合法，无隐藏损坏）。
   - 负例：临时破坏 configs/logging.toml（`[[broken`）→ exit=2 被拦。

2. **consumer 注释批量**：82 个 toml 全部头部添加 `# consumer:` 声明
   （脚本批量，零键值改动）：
   - control-write.toml → device-agent::assembly_demo（唯一真实加载方）；
   - dengbao.toml → compliance（测试证据指针）；
   - 其余 80 个 → 集成层占位（产线注入）。
   - 验证：`grep -L "consumer:" configs/*.toml` 输出为空（0 个缺失）。

3. **config-refs 扩展校验**（依赖 WP-29.1 已转阻断）：每个 toml 头部 12 行
   内必须含 `# consumer:` 声明，否则 FAIL（S6 显式声明纪律）。
   - 负例：删除 configs/lfs.toml 的 consumer 注释 → exit=2 被拦；
     恢复后 exit 0。
   - 深度校验（声明的消费者存在性）留给 34 的 GV-15。

## 事实登记

docs/dead-surface-audit.md 新增 CF-01 章节：82 个 toml 仅 control-write.toml
被真实加载（且仅测试路径）；go-no-go 不得宣称活配置；产线注入为集成层动作。

## 回归

- `cargo test -p eneros-xtask`：59 passed 全绿。
- `cargo xtask gate config-syntax` / `config-refs`：全绿（missing=0、
  no-consumer=0）。
