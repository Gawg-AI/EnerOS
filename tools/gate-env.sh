#!/bin/bash
# gate-env.sh —— gate/QEMU 脚本工具链环境单源（蓝图 63 卷 63.3 WP-63.3.5）。
#
# 单一事实源：此前 20 个 gate/QEMU 脚本各自复制 xpack 工具链 env 块
# （PATH/LD_LIBRARY_PATH/PYTHONPATH/BINDGEN sysroot）+ 11 个脚本复制
# RELEASE_KEY/KEY_ID/PLATFORM_ID 三常量 + SOURCE_DATE_EPOCH——工具链缓存
# 升级 = 20 处同时漂移（第七轮体检 D-gate 脚本坐实）。本文件收编公共面；
# 各脚本 `source "$(dirname "$0")/gate-env.sh"` 后只保留判据差异。
#
# 版本号单源（升级改这里一处）：
#   xpack-aarch64-none-elf-gcc 15.2.1-1.1 / cmake 3.31.6 / pyfdt 0.3 /
#   dtc + xmllint + libxml2 + libicu（deb 解包布局）
#
# 刻意**不**导出（判据差异，留在各脚本）：
#   - ENEROS_IMAGE_SIGNING_KEY：run-a-negative-nokey 等负例脚本必须无签名
#     钥（fail-closed 门验证），单源导出会破坏其语义；
#   - 端口清理 / EVROOT / peer 编排：各脚本判据本体。
# 与 ci.yml 的差异登记：GitHub 环境的 toolchain 缓存由 ci.yml 自建
# （actions/cache + setup 步骤），不消费本文件——环境不同，非漂移。

# 工具链 PATH（xpack 交叉工具链 + cmake + dtc + xmllint）。
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"

# xpack 工具链动态库（libxml2/libicu deb 解包）。
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"

# sel4-build 树 FDT 解析（pyfdt）。
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"

# bindgen target 限定 sysroot（newlib 头文件；`_` 分隔的 target 键）。
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_none="--sysroot=$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/aarch64-none-elf"

# 复现性 epoch（62 卷基线值；外部已设则尊重——确定性重跑入口）。
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1755523200}"

# 发布验证三常量（镜像验证路径；与 product/security 发布钥面配套）。
export ENEROS_RELEASE_PUBLIC_KEY_HEX="8e5150375a22ae2ad55ea85a3e7a8faee8196315a413163fd4778c82c59af9a7"
export ENEROS_RELEASE_KEY_ID_HEX="4e28c578ebd8c3c68b83ff1d0b1a16df"
export ENEROS_PLATFORM_ID_HEX="6afc39cf058a1ca52a5e36d8eefa1463"
