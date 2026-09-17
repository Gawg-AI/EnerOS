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
# gate=测试链，产物锚=测试锚（在册事实，蓝图 66 §3.4 冻结）——签名钥
# fail-closed（蓝图 66 WP-66.1.1）后须显式 --accept-test-signing-key。
# 用法：gate_compile_config <compile.log 路径> [profile=qemu-prod]
gate_compile_config() {
  local LOG="$1"
  local PROFILE="${2:-qemu-prod}"
  echo "=== config compile（$PROFILE）==="
  ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile "$PROFILE" \
    --accept-test-signing-key \
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
# 一行调用仅 run62-vol1 使用；其余脚本按需分相调用（compile 与 image
# 之间需插重链接步骤的脚本 run62-vol3/run63-vol1/run63-vol2/run64-vol1，
# 及负例脚本 no-key 直调 compile/image 两函数不调钥导出）：
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

# ===========================================================================
# 构建缓存戳（REG-70.15-BUILD-NONDETERMINISM 治理，2026-09-15）：
# 同树 + 同 ROOT_TASK_FEATURES 的重链接/镜像只做一次——后续门共享同一
# 份镜像字节（"门间共享同一镜像副本"路线），候选身份在单次收口扫描内
# 单产物闭合。树状态 = HEAD + tracked/untracked 工作树摘要 + 重链接脚本摘要 +
# feature 集；任一变化 → 戳失效 → 重建。
# 注意：跨 feature 交替（qemu-prod ↔ qemu-test-synth）必然使戳失效并
# 重链接（合成装配与生产装配是不同闭包）——扫描编排按 feature 分组
# 排序即可最大化共享。
# ===========================================================================
gate_build_stamp() {  # $1 = features 字符串（可空）
  # **必须并入 build.rs 消费的 ENEROS_* 环境**（PROBE_BUNDLE/PROBE_SIG/
  # IMAGE_SIGNING_KEY 等在编译期烘进 ELF——2026-09-15 vol6 实弹：缓存命中
  # 了无探针束的 vol5-B 构建 ⇒ 探针 step3 empty-bundle 级联失败。环境签名
  # 变化 ⇒ 戳失效，诚实重建）。
  # 树摘要用 **git diff HEAD 内容哈希**（2026-09-15 vol89 实弹教训：porcelain
  # 行摘要对"已修改文件的内容再编辑"不敏感 ⇒ 缓存吞掉诊断构建）。
  local tree envsig untracked file
  # New source files must invalidate the cache even before git add. Evidence and
  # build output are excluded, matching candidate tree identity.
  untracked="$(
    while IFS= read -r -d '' file; do
      sha256sum -- "$file"
    done < <(git ls-files --others --exclude-standard -z -- . ':(exclude)evidence/**' ':(exclude)target/**')
  )"
  tree="$(git rev-parse HEAD 2>/dev/null)-$(git diff HEAD -- . ':(exclude)evidence/**' ':(exclude)target/**' | sha256sum | cut -d' ' -f1)-$untracked"
  envsig="$(printenv 2>/dev/null | grep -E '^ENEROS_' | sort | sha256sum | cut -d' ' -f1)"
  printf '%s-%s-%s-%s' "$tree" "${1:-none}" "$envsig" \
    "$(sha256sum "$ROOT/third_party/sel4-build-rust.sh" 2>/dev/null | cut -d' ' -f1)" \
    | sha256sum | cut -d' ' -f1
}

gate_relink_cached() {  # $1 = log 路径；$2 = features（可空）
  local stamp expected
  stamp="$(gate_build_stamp "${2:-}")"
  expected="$(cat "$ROOT/target/eneros/.relink-stamp" 2>/dev/null || true)"
  if [ "$stamp" = "$expected" ]; then
    echo "=== sel4-build-rust 重链接（缓存命中，跳过：stamp=${stamp:0:12}）==="
    return 0
  fi
  echo "=== sel4-build-rust 重链接（stamp=${stamp:0:12}）==="
  if [ -n "${2:-}" ]; then
    ROOT_TASK_FEATURES="$2" bash "$ROOT/third_party/sel4-build-rust.sh" >"$1" 2>&1 || {
      tail -20 "$1"; return 1; }
  else
    bash "$ROOT/third_party/sel4-build-rust.sh" >"$1" 2>&1 || {
      tail -20 "$1"; return 1; }
  fi
  mkdir -p "$ROOT/target/eneros"
  printf '%s' "$stamp" > "$ROOT/target/eneros/.relink-stamp"
}

gate_image_raw_cached() {  # $1 = profile; $2 = log; $3 = features（可空）
  # 镜像戳额外并入已编译配置束摘要——每轮新鲜锚/钥材料（如 vol5 OTA 锚）
  # 会使束摘要变化 ⇒ 缓存正确失效，不共享。
  local stamp expected img="$ROOT/target/eneros/artifacts/$1/eneros-$1.raw"
  local bundle=""
  [ -f "$ROOT/product/config-bundle.bin" ] && bundle="$(sha256sum "$ROOT/product/config-bundle.bin" | cut -d' ' -f1)"
  stamp="$(gate_build_stamp "${3:-}")-${bundle:-none}"
  expected="$(cat "$ROOT/target/eneros/.image-stamp-$1" 2>/dev/null || true)"
  if [ "$stamp" = "$expected" ] && [ -f "$img" ]; then
    echo "=== image raw $1（缓存命中，跳过）==="
    return 0
  fi
  gate_image_raw "$1" "$2"
  mkdir -p "$ROOT/target/eneros"
  printf '%s' "$stamp" > "$ROOT/target/eneros/.image-stamp-$1"
}
