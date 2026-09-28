#!/usr/bin/env bash
# ============================================================
# EnerOS seL4 补丁面收敛器（2026-09-23 审计复核施工）
#
# 语义：把 $CACHE 内全部「项目受管文件」**收敛到期望 sha256 终态**，
# 而非「把补丁应用一次」。任何起始状态——fresh 解压 / 旧版已打 /
# 部分应用（.rej 残留）/ DBG-TEMP 感染 / 上游 master 漂移——都收敛到
# 同一字节，收敛后逐文件 sha256 校验，fail-closed。
#
# 为什么取代旧的 marker-grep 幂等循环（DEF-52-01，2026-08-23 登记）：
# 旧循环用单一字符串（ENEROS_INIT_BUNDLE_PHYS）判定整批补丁已应用，
# 0002 无该串 → fresh clone 直接 exit 1；且 boot.c 打上、vspace.c hunk
# 被拒时仍判「已应用」（真实现场：~/.cache 曾存在 vspace.c.rej，
# 2026-08-09，n+=2 时代 hunk）。收敛语义对两者都响亮失败或修复。
#
# 受管文件（期望态 = 官方 tarball 原件 + third_party/sel4-patches/）：
#   sel4-src  src/arch/arm/kernel/boot.c       ← 0001 + 0004（C2/BOOT-007
#             映射 + WP-77.2.2 boot witness 快照/extra chunk 发布）
#   sel4-src  src/arch/arm/64/kernel/vspace.c  ← 0001（n+=5 记账）
#   elfloader src/arch-arm/sys_boot.c          ← 0002 + 0003（0002 期望态
#             =官方原件（DBG-TEMP 感染树等价修复，补丁留档）+ WP-77.2.2
#             boot witness 消费/信封发布）
#   elfloader src/linker.lds                   ← third_party/elfloader-linker.lds
#             （W^X PHDRS，xtask pack_elf64 硬门依赖；2026-09-23 前为
#              无任何消费者的孤儿副本——E1 收编）
#
# 升版内核补丁时必须同步重算下列 sha256：
#   干净树 + 新补丁 → sha256sum 两个目标文件，回填本表。
# ============================================================
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="${SEL4_CACHE:-$HOME/.cache}"
SEL4_SRC="$CACHE/sel4-src"
ELFLOADER_SRC="$CACHE/seL4_tools-master/elfloader-tool"
PATCHES_DIR="$ROOT/third_party/sel4-patches"
KERNEL_TB="$CACHE/sel4-14.0.0.tar.gz"
TOOLS_TB="$CACHE/seL4_tools.tar.gz"

# 期望 sha256（受管文件收敛终态）
# 蓝图 77 卷二（WP-77.2.2）：boot.c 终态 = 原件 + 0001 + 0004；
# sys_boot.c 终态 = 原件 + 0003。升版补丁后重算：
#   干净树 → patch 0001 → patch 0004 → sha256sum boot.c
#   干净树 → patch 0003 → sha256sum sys_boot.c
BOOT_C_SHA=6c1799a6a6dbce54dece3b347e2a2c8d6d242deb6ee8da08dbe9b19b036bcb7f
VSPACE_C_SHA=1a29efe4750a42e20e8f06339a4ec3f60334b8ec70c4001ee8bceab2460dcfc2
SYS_BOOT_C_SHA=5416afbd1963e2bc70fe79a68a4b19b54de7c2ab65f38bab616e7ea285ef4157
LINKER_LDS_SHA=722c55f4bfe0d650c8788549a0f7b389ae34f17967e8830a30b05d27257f2a50

sha_of() { sha256sum "$1" | awk '{print $1}'; }
converged() { [ "$(sha_of "$1")" = "$2" ]; }
require_tarball() {
  if [ ! -f "$1" ]; then
    echo "ERROR: converge 需要 $1（还原官方原件用）；请重跑 third_party/sel4-bootstrap.sh" >&2
    exit 1
  fi
}
restore_from_tarball() { # $1=tarball $2=member(可含通配) $3=dest
  tar -xzf "$1" -O --wildcards "$2" > "$3"
}
fail_verify() {
  echo "ERROR: 受管文件收敛校验失败：$1（期望 sha256=$2，实际=$(sha_of "$1")）" >&2
  exit 1
}

require_tarball "$KERNEL_TB"
require_tarball "$TOOLS_TB"

echo "==> sel4-converge-patches: SEL4_SRC=$SEL4_SRC ELFLOADER=$ELFLOADER_SRC"

# --- 1) 内核 0001（boot.c + vspace.c 由同一补丁产生，原子收敛）---
if converged "$SEL4_SRC/src/arch/arm/kernel/boot.c" "$BOOT_C_SHA" \
   && converged "$SEL4_SRC/src/arch/arm/64/kernel/vspace.c" "$VSPACE_C_SHA"; then
  echo "  [converge] 0001+0004: already converged（boot.c/vspace.c sha256 终态）"
else
  echo "  [converge] 0001+0004: 还原官方原件 + 应用补丁"
  for f in src/arch/arm/kernel/boot.c src/arch/arm/64/kernel/vspace.c; do
    restore_from_tarball "$KERNEL_TB" "seL4-14.0.0/$f" "$SEL4_SRC/$f"
  done
  (cd "$SEL4_SRC" && patch -p0 --fuzz=0 -f < "$PATCHES_DIR/0001-init-bundle-mapping.patch")
  (cd "$SEL4_SRC" && patch -p0 --fuzz=0 -f < "$PATCHES_DIR/0004-kernel-eneros-witness-extra-bi.patch")
fi
converged "$SEL4_SRC/src/arch/arm/kernel/boot.c" "$BOOT_C_SHA" \
  || fail_verify "$SEL4_SRC/src/arch/arm/kernel/boot.c" "$BOOT_C_SHA"
converged "$SEL4_SRC/src/arch/arm/64/kernel/vspace.c" "$VSPACE_C_SHA" \
  || fail_verify "$SEL4_SRC/src/arch/arm/64/kernel/vspace.c" "$VSPACE_C_SHA"
# 清理补丁残渣（部分应用时代的 .orig/.rej；vspace.c.rej 曾真实在盘）
rm -f "$SEL4_SRC/src/arch/arm/kernel/boot.c.orig" "$SEL4_SRC/src/arch/arm/kernel/boot.c.rej" \
      "$SEL4_SRC/src/arch/arm/64/kernel/vspace.c.orig" "$SEL4_SRC/src/arch/arm/64/kernel/vspace.c.rej"

# --- 2) elfloader sys_boot.c（0002 期望态 = 官方原件 + 0003 witness 消费）---
if ! converged "$ELFLOADER_SRC/src/arch-arm/sys_boot.c" "$SYS_BOOT_C_SHA"; then
  echo "  [converge] 0002+0003: 还原官方原件 sys_boot.c + witness 消费补丁"
  restore_from_tarball "$TOOLS_TB" '*/src/arch-arm/sys_boot.c' \
    "$ELFLOADER_SRC/src/arch-arm/sys_boot.c"
  (cd "$ELFLOADER_SRC" && patch -p0 --fuzz=0 -f < "$PATCHES_DIR/0003-elfloader-eneros-boot-witness.patch")
fi
converged "$ELFLOADER_SRC/src/arch-arm/sys_boot.c" "$SYS_BOOT_C_SHA" \
  || fail_verify "$ELFLOADER_SRC/src/arch-arm/sys_boot.c" "$SYS_BOOT_C_SHA"

# --- 3) elfloader linker.lds（W^X PHDRS 安装）---
if ! cmp -s "$ROOT/third_party/elfloader-linker.lds" "$ELFLOADER_SRC/src/linker.lds"; then
  echo "  [converge] linker.lds: 安装 W^X PHDRS 版本"
  cp "$ROOT/third_party/elfloader-linker.lds" "$ELFLOADER_SRC/src/linker.lds"
fi
converged "$ELFLOADER_SRC/src/linker.lds" "$LINKER_LDS_SHA" \
  || fail_verify "$ELFLOADER_SRC/src/linker.lds" "$LINKER_LDS_SHA"

echo "==> sel4-converge-patches: 全部受管文件已收敛"
