#!/usr/bin/env bash
# G1 绑定产物一键生成（CI g1-fuzz-gates 与本地共用的唯一入口）。
#
# 产出 registry（product/evidence/g0-g1-requirements.json）绑定的全部产物：
#   toolchain-report / installer iso / recovery raw / full.enupd /
#   bound-flash-report / cdx+spdx SBOM / qemu-prod raw
#
# 纪律：
# - fail-closed：任一步失败即退出非零，证据缺失不得静默通过。
# - toolchain 特殊容错：pinned-tools.json 描述 Windows/WSL 发布环境，
#   Linux（CI/开发机）上 wsl.exe 不可用，report 如实 FAIL 但仍须落盘
#   （存在性由 evidence verify 把关；与 gkernel job `|| test -s` 先例一致）。
# - update 包要求 target.security_version > base：临时 bump profile 到 2
#   生成升级 target，用毕立即恢复（保证 v1.0.0 构建配置不被漂移）。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [ -z "${SOURCE_DATE_EPOCH:-}" ]; then
  echo "g1-artifacts: SOURCE_DATE_EPOCH is mandatory" >&2
  exit 1
fi

RAW="target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
ART="target/eneros/artifacts/qemu-prod"
RECOVERY_ART="target/eneros/artifacts/recovery"
KEY="product/security/dev-signing.key"
UEFI_KEY="product/security/uefi-test.key"
UEFI_CERT="product/security/uefi-test.pem"

echo "==> g1-artifacts: SOURCE_DATE_EPOCH=$SOURCE_DATE_EPOCH"

# 1. toolchain report（蓝图 36 WP-36.10 #4：禁止 `|| true` 静默吞失败；
#    显式捕获 rc 并记录；Linux 无 wsl.exe 为 BLOCKED_EVIDENCE，产物必须落盘）
echo "==> [1/8] audit toolchain (report must land; Linux wsl-unavailable is BLOCKED_EVIDENCE)"
set +e
cargo xtask audit toolchain >target/eneros/evidence/REL-001/toolchain-audit.log 2>&1
TOOLCHAIN_RC=$?
set -e
test -s target/eneros/evidence/REL-001/toolchain-report.json \
  || { echo "g1-artifacts: toolchain-report.json missing" >&2; exit 1; }
if [ "${TOOLCHAIN_RC}" -ne 0 ]; then
  echo "g1-artifacts: toolchain audit rc=${TOOLCHAIN_RC} (BLOCKED_EVIDENCE: Windows/WSL 发布环境检查) — 详见 toolchain-audit.log" >&2
fi

# 2. 干净 raw（BOOTCTRL seq=0；后续 recovery/iso/sbom 的基座）
echo "==> [2/8] image qemu-prod (clean raw)"
cargo xtask image qemu-prod

# 3. update full 包：base=sec1 raw → target=sec2 镜像（真实版本升级语义）
echo "==> [3/8] image update (full, sec1 -> sec2)"
cp "$RAW" "$ART/eneros-qemu-prod.sec1.raw"
PROFILE_BACKUP="$(mktemp)"
cp product/profiles/qemu-prod.json "$PROFILE_BACKUP"
# review nit（36 收尾）：中途失败也必须恢复 profile（set -e 下 trap 兜底）
trap 'cp "$PROFILE_BACKUP" product/profiles/qemu-prod.json 2>/dev/null; rm -f "$PROFILE_BACKUP"' EXIT
python3 - <<'PY'
import json
path = "product/profiles/qemu-prod.json"
profile = json.load(open(path))
profile["security"]["minimum_security_version"] = 2
json.dump(profile, open(path, "w"), indent=1, ensure_ascii=False)
PY
cargo xtask image qemu-prod
cargo xtask image update qemu-prod --kind full \
  --base "$ART/eneros-qemu-prod.sec1.raw" \
  --target "$RAW" \
  --signing-key "$KEY" \
  --output "$ART/eneros-qemu-prod-full.enupd"
cp "$PROFILE_BACKUP" product/profiles/qemu-prod.json
rm -f "$PROFILE_BACKUP"
trap - EXIT

# 4. 恢复 sec1 后的最终 raw（update 重建已改变 raw；重新构建最终基准）
echo "==> [4/8] image qemu-prod (final clean raw)"
cargo xtask image qemu-prod

# 5. recovery 镜像（要求 source BOOTCTRL seq=0）
echo "==> [5/8] image recovery"
cargo xtask image recovery recovery \
  --source "$RAW" \
  --release-public-key "$ART/release-public-key.bin"

# 6. SBOM（cdx + spdx；依赖 product/closure/binary-boms.json 含 sel4 外部条目）
echo "==> [6/8] release sbom"
cargo xtask release sbom --profile qemu-prod \
  --artifact "$RAW" \
  --cyclonedx "$ART/eneros-qemu-prod.cdx.json" \
  --spdx "$ART/eneros-qemu-prod.spdx.json"

# 7. installer ISO
echo "==> [7/8] image iso"
cargo xtask image iso qemu-prod \
  --raw "$RAW" \
  --signing-key "$KEY" \
  --uefi-key "$UEFI_KEY" \
  --uefi-cert "$UEFI_CERT"

# 8. flash 三步：lab-create → authorize-file → write（readback 全量校验）
echo "==> [8/8] flash write (file-lab, readback-verified)"
cargo xtask flash lab-create --image "$RAW" --target target/eneros/flash-lab/lab.raw
cargo xtask flash authorize-file --image "$RAW" \
  --target target/eneros/flash-lab/lab.raw \
  --operator dev-ci --station ci --candidate local \
  --output target/eneros/evidence/IMG-011/flash-token.json
cargo xtask flash write --image "$RAW" \
  --authorization target/eneros/evidence/IMG-011/flash-token.json \
  --output target/eneros/evidence/IMG-011/bound-flash-report.json
# WP-43.10：每台设备签名出生记录（factory 域）——先从 write 报告派生。
cargo xtask flash birth-record \
  --report target/eneros/evidence/IMG-011/bound-flash-report.json \
  --device-serial "dev-ci-0001" --device-model "qemu-virt" \
  --operator dev-ci --station ci \
  --out target/eneros/evidence/IMG-011/birth-record.json
cargo xtask flash verify-birth target/eneros/evidence/IMG-011/birth-record.json
cargo xtask flash key-domains product/factory/key-domains.json

# 9. evidence gate（蓝图 36 WP-36.10 #5：release 必须调用 evidence gate，
#    不能只调用 verify——gate 强制 verdict=PASS 才 exit 0）
echo "==> evidence gate"
cargo xtask evidence gate

# 收尾（36 security review 收尾）：toolchain BLOCKED_EVIDENCE 永远不是
# PASS——不得打印 ALL PASS 掩盖；显式非零并指向日志。
if [ "${TOOLCHAIN_RC:-0}" -ne 0 ]; then
  echo "==> g1-artifacts: COMPLETE-WITH-BLOCKED (toolchain_rc=${TOOLCHAIN_RC})" >&2
  echo "    BLOCKED_EVIDENCE: target/eneros/evidence/REL-001/toolchain-audit.log（非 PASS）" >&2
  exit 1
fi
echo "==> g1-artifacts: ALL PASS"
