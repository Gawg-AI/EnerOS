# formal/Makefile — Isabelle 证明检查入口
#
# EnerOS v0.148.0 形式化规范框架搭建 — Phase 3 P3-F 首版本
# 依据：蓝图 phase3.md §v0.148.0 §3 测试计划「formal/Makefile：证明检查通过」
#
# 降级口径（D1）：本机/CI 未安装 Isabelle2024，Makefile 为工具链可用时的检查入口；
#                当前 host 集成测试 formal_framework_test.rs 验证框架结构。
#
# Isabelle2024+ 安装指引：
#   1. 下载：https://isabelle.in.tum.de/
#   2. 解压并配置 PATH（指向 Isabelle2024/bin）
#   3. 验证：isabelle version
#
# 触发证明检查：
#   make -C formal/         # 等价于 isabelle build -D formal/
#   make -C formal/ clean   # 清理构建产物

ISABELLE = isabelle

.PHONY: all build clean

all: build

build:
	$(ISABELLE) build -D .

clean:
	rm -rf output/
