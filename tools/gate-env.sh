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

# ===========================================================================
# build 前导单源（蓝图 64 卷 64.3 WP-64.3.6）。
#
# 收编 14 个 gate/QEMU 脚本各自复制的五件前导：dev root key 推导
# （`5a`×64——原 14 处字面，单点于此）、ENEROS_IMAGE_SIGNING_KEY 导出、
# xtask config compile、image raw 三键参块、QEMU/pgrep 清场环。
#
# no-key 语义（本文件头注登记维持）：负例脚本（run-a-negative-nokey 等）
# 必须无签名钥（fail-closed 门验证）——`gate_build_prelude no-key` 选择性
# 旁路钥导出，compile 照跑（负例判据面自行验证）。
# ci.yml 不消费本文件（GitHub 环境自建 toolchain——头注差异登记维持）。
#
# 依赖调用方变量：ROOT（仓库根；调用方已 cd "$ROOT"）。
# 失败语义与原脚本一致：compile 失败 exit 2 / image 失败 exit 3 /
# 端口占用 exit 4。
# ===========================================================================

# dev root key 推导单点（原 14 处 `printf '5a%.0s' {1..64}` 字面收口）。
gate_dev_root_key_hex() {
  printf '5a%.0s' {1..64}
}

# QEMU/pgrep 清场环：杀对端脚本与 qemu-system + 默认端口占用检查。
# 用法：gate_cleanup_env [port...]（缺省 5577 5582）。
gate_cleanup_env() {
  # 对端/仿真进程清杀 superset（部分脚本原只杀 modbus 对端——并集无害：
  # 进程不在时 kill 为 no-op）。
  kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
  kill -9 $(pgrep -f "iec104_slave6") 2>/dev/null || true
  kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
  sleep 2
  local PORT
  local PORTS="${*:-5577 5582}"
  for PORT in $PORTS; do
    if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
      echo "PORT $PORT 仍被占——清理失败"
      exit 4
    fi
  done
}

# 签名钥导出（with-key 路径专用；no-key 脚本不得调用）。
gate_export_signing_key() {
  export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
}

# config compile（dev root key 单点推导；KMS 目录单点）。
# 用法：gate_compile_config <compile.log 路径> [profile=qemu-prod]
gate_compile_config() {
  local LOG="$1"
  local PROFILE="${2:-qemu-prod}"
  echo "=== config compile（$PROFILE）==="
  ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile "$PROFILE" \
    --root-key-hex "$(gate_dev_root_key_hex)" >"$LOG" 2>&1
  local RC=$?
  [ $RC -eq 0 ] || { echo "CONFIG-COMPILE FAIL rc=$RC"; tail -5 "$LOG"; exit 2; }
  tail -2 "$LOG"
}

# image raw 三键参块（dev-signing / uefi-test / uefi-test.pem 单点）。
# 用法：gate_image_raw <profile> <image.log 路径> [append]
#   append 模式（多 case 脚本共享同一日志时）以 >> 追加。
gate_image_raw() {
  local PROFILE="$1"
  local LOG="$2"
  local MODE="${3:-truncate}"
  local K="$ROOT/product/security/keys-local"
  echo "=== image raw $PROFILE ==="
  if [ "$MODE" = "append" ]; then
    cargo xtask image raw "$PROFILE" \
      --signing-key "$K/dev-signing.key" --uefi-key "$K/uefi-test.key" --uefi-cert "$K/uefi-test.pem" \
      >>"$LOG" 2>&1
  else
    cargo xtask image raw "$PROFILE" \
      --signing-key "$K/dev-signing.key" --uefi-key "$K/uefi-test.key" --uefi-cert "$K/uefi-test.pem" \
      >"$LOG" 2>&1
  fi
  local RC=$?
  [ $RC -eq 0 ] || { echo "IMAGE FAIL rc=$RC"; tail -20 "$LOG"; exit 3; }
}

# build 前导组合（清场 + 钥导出 + compile + image 一步）。
# 用法：gate_build_prelude <with-key|no-key> <build 日志目录> [profile]
# 标准脚本一行调用；compile 与 image 之间需插重链接步骤的脚本
# （run62-vol3/run63-vol1/run63-vol2/run64-vol1）改用分相调用：
#   gate_cleanup_env + gate_export_signing_key + gate_compile_config
#   + [sel4 重链接] + gate_image_raw。
gate_build_prelude() {
  local MODE="${1:?with-key|no-key}"
  local DIR="$2"
  local PROFILE="${3:-qemu-prod}"
  mkdir -p "$DIR"
  gate_cleanup_env
  if [ "$MODE" = "with-key" ]; then
    gate_export_signing_key
  fi
  gate_compile_config "$DIR/compile.log" "$PROFILE"
  gate_image_raw "$PROFILE" "$DIR/image.log"
}
