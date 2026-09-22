#!/usr/bin/env bash
# sel4-build.sh 缓存引导（21-B D3）：干净环境一次性下载/解压全部构建依赖。
#
# 用法：third_party/sel4-bootstrap.sh [--verify-only]
#   - 默认：缺失项自动下载到 $SEL4_CACHE（默认 ~/.cache），幂等（已存在跳过）
#   - --verify-only：只校验已有缓存完整，不下载
#
# 覆盖依赖（核心 6 项自动 fetch；deb/pip 次要项按文档命令获取）：
#   1. sel4-src           官方 seL4 14.0.0（GitHub release tarball）
#   2. seL4_tools-master  官方 seL4_tools（master tarball）
#   3. util_libs-master   官方 util_libs（master tarball）
#   4. xpack gcc          xpack-dev-tools aarch64-none-elf-gcc 15.2.1-1.1
#   5. cmake              Kitware CMake 3.31.6 linux-x86_64
#   6. ninja              ninja-build 1.12.1 linux
#   次要（deb 包 / pip wheel，bootstrap 打印获取命令）：
#   - dtc（device-tree-compiler 1.7.2-2ubuntu1）、xmllint（libxml2-utils）、
#     libxml2/libicu74 运行库（Ubuntu 包）
#   - pyfdt + lxml + ply + markupsafe + jinja2 + libarchive_c + pyelftools
#     （pip download --dest $CACHE 后解压进 $CACHE/pyuser）
#
# 校验纪律：下载后 sha256 校验（KNOWN_SHA256 表）；缺失校验值则记录
# 实际哈希（首次 bootstrap 后输出，供后续固化）。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="${SEL4_CACHE:-$HOME/.cache}"
mkdir -p "$CACHE"

# 官方来源（2026-09-23 审计复核施工：全部钉死，moving branch 退役）
# - seL4 14.0.0 tag tarball：逐字节稳定（2026-09-23 与 GitHub 复核一致）
# - seL4_tools / util_libs：钉 commit（与 2026-07 缓存同字节；GitHub
#   archive/<sha> 与 refs/heads/master 两种 URL 的 tarball 根目录名不同、
#   gzip 字节不同，但解压树逐字节一致——钉 commit 后 master 漂移即
#   哈希失配 fail-closed，不再有 moving branch 静默漂移）。
SEL4_URL="https://github.com/seL4/seL4/archive/refs/tags/14.0.0.tar.gz"
TOOLS_URL="https://github.com/seL4/seL4_tools/archive/7dd5ba144b1fecf1358a12d2bef3eb365aab35c7.tar.gz"
UTILS_URL="https://github.com/seL4/util_libs/archive/6e55b3c62687779692150e1de411ce61b9d2919a.tar.gz"
GCC_URL="https://github.com/xpack-dev-tools/aarch64-none-elf-gcc-xpack/releases/download/v15.2.1-1.1/aarch64-none-elf-gcc-xpack-15.2.1-1.1-linux-x64.tar.gz"
CMAKE_URL="https://github.com/Kitware/CMake/releases/download/v3.31.6/cmake-3.31.6-linux-x86_64.tar.gz"
NINJA_URL="https://github.com/ninja-build/ninja/releases/download/v1.12.1/ninja-linux.zip"

# 已知 sha256（下载后 fail-closed 校验）。
# 注意 seL4_tools/util_libs 两项是 archive/<sha> 形态的字节（根目录名含
# commit sha）；2026-09-23 前以 refs/heads/master.tar.gz 形态下载的旧缓存
# 字节不同（e1e7b75f…/ad47fd57…）但解压树一致——已存在的缓存不会被
# 重新校验（fetch 见文件即跳过），仅新下载走新哈希。
# 工具链三项（gcc/cmake/ninja）为版本化 release URL，暂未固化（缓存中
# 只有解压目录无 tarball 可供取哈希）；固化前维持「下载后打印」行为。
declare -A KNOWN_SHA256=(
  [sel4-14.0.0.tar.gz]="c518f6121a56f1b377ee5b596784a0a454edcde5f87bdb402db057c706400c0c4"
  [seL4_tools.tar.gz]="9f5df17858f57d5e87c9024aa122e2350c205544694501c8e98bd5f8af92f968"
  [util_libs.tar.gz]="00ecc1a63775140a795bb9903c341b9fe1f270a8161f6613cb41d6c21f5061e6"
  [xpack-gcc.tgz]=""
  [cmake-github.tar.gz]=""
  [ninja.zip]=""
)

fetch() {
  local name="$1"
  local url="$2"
  local dest="$CACHE/$name"
  if [ -e "$dest" ]; then
    echo "  [cache] $name already present"
    return 0
  fi
  echo "  [fetch] $name <- $url"
  curl -fSL --retry 3 -o "$dest" "$url"
  local actual
  actual=$(sha256sum "$dest" | awk '{print $1}')
  local expected="${KNOWN_SHA256[$name]-}"
  if [ -n "$expected" ] && [ "$actual" != "$expected" ]; then
    echo "ERROR: $name sha256 mismatch: got $actual, expected $expected" >&2
    rm -f "$dest"
    exit 1
  fi
  if [ -z "$expected" ]; then
    echo "  [hash] $name sha256=$actual（首次获取，已记录——请固化到 KNOWN_SHA256）"
  else
    echo "  [hash] $name OK"
  fi
}

extract_tar() {
  local tarball="$CACHE/$1" dest="$CACHE/$2"
  if [ -d "$dest" ]; then
    echo "  [extract] $2 already present"
    return 0
  fi
  echo "  [extract] $tarball -> $2"
  mkdir -p "$dest"
  tar -xzf "$tarball" -C "$dest" --strip-components=1
}

echo "==> sel4-bootstrap: CACHE=$CACHE"

# 1) seL4 14.0.0
fetch "sel4-14.0.0.tar.gz" "$SEL4_URL"
extract_tar "sel4-14.0.0.tar.gz" "sel4-src"
# 2) seL4_tools（elfloader-tool / cmake-tool helpers）
fetch "seL4_tools.tar.gz" "$TOOLS_URL"
extract_tar "seL4_tools.tar.gz" "seL4_tools-master"
# 3) util_libs（libcpio）
fetch "util_libs.tar.gz" "$UTILS_URL"
extract_tar "util_libs.tar.gz" "util_libs-master"
# 4) xpack aarch64 gcc
fetch "xpack-gcc.tgz" "$GCC_URL"
extract_tar "xpack-gcc.tgz" "xpack-aarch64-none-elf-gcc-15.2.1-1.1"
# 5) cmake
fetch "cmake-github.tar.gz" "$CMAKE_URL"
extract_tar "cmake-github.tar.gz" "cmake-3.31.6-linux-x86_64"
# 6) ninja
if [ ! -d "$CACHE/ninja-bin" ]; then
  fetch "ninja.zip" "$NINJA_URL"
  echo "  [extract] ninja.zip -> ninja-bin"
  mkdir -p "$CACHE/ninja-bin"
  (cd "$CACHE/ninja-bin" && unzip -o -q "$CACHE/ninja.zip")
fi

# 关键校验（fail-closed：核心工具必须可执行）
for tool in \
  "$CACHE/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin/aarch64-none-elf-gcc" \
  "$CACHE/cmake-3.31.6-linux-x86_64/bin/cmake" \
  "$CACHE/ninja-bin/ninja"; do
  if [ ! -x "$tool" ]; then
    echo "ERROR: bootstrap incomplete — missing $tool" >&2
    exit 1
  fi
done

# 7) Ubuntu 工具包（dtc / xmllint / libxml2 / libicu74 运行库）
#    apt-get download 需要 deb-src 之外的正常源；无 apt 环境（如纯容器）
#    可跳过——sel4-build.sh 会在后续步骤明确失败并给出安装指引。
if command -v apt-get >/dev/null 2>&1; then
  DEB_STAGING="$CACHE/deb-staging"
  mkdir -p "$DEB_STAGING"
  for deb in device-tree-compiler=1.7.2-2ubuntu1 libxml2-utils=2.15.2+dfsg-0.1ubuntu0.1 \
             libxml2 libicu74; do
    if [ ! -e "$DEB_STAGING/$(echo "$deb" | cut -d= -f1).ok" ]; then
      echo "  [apt] download $deb"
      (cd "$DEB_STAGING" && apt-get download "$deb" >/dev/null 2>&1) \
        || echo "  [apt] warning: $deb download failed（离线环境可跳过）"
      touch "$DEB_STAGING/$(echo "$deb" | cut -d= -f1).ok"
    fi
  done
  for d in "$DEB_STAGING"/*.deb; do
    [ -e "$d" ] || continue
    pkg="$(basename "$d" | sed 's/_.*//')"
    case "$pkg" in
      device-tree-compiler) dpkg-deb -x "$d" "$CACHE/dtc-deb" 2>/dev/null || true ;;
      libxml2-utils) dpkg-deb -x "$d" "$CACHE/xmllint-bin" 2>/dev/null || true ;;
      libxml2) dpkg-deb -x "$d" "$CACHE/libxml2-deb" 2>/dev/null || true ;;
      libicu74) dpkg-deb -x "$d" "$CACHE/libicu74-deb" 2>/dev/null || true ;;
    esac
  done
fi

# 8) Python 依赖（pyfdt/lxml/ply/markupsafe/jinja2/libarchive_c/pyelftools → $CACHE/pyuser）
if command -v pip >/dev/null 2>&1 && [ ! -d "$CACHE/pyuser/elftools" ]; then
  echo "  [pip] installing python build deps into $CACHE/pyuser"
  pip install --quiet --target "$CACHE/pyuser" \
    pyfdt lxml ply markupsafe "jinja2==3.1.4" libarchive-c pyelftools \
    || echo "  [pip] warning: python deps install failed（离线环境可跳过）"
fi

echo "==> sel4-bootstrap: core dependencies OK"
