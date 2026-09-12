#!/usr/bin/env bash
set -uo pipefail
ROOT="/home/ahx/文档/EnerOS"
cd "$ROOT"
source tools/gate-env.sh
TEST_ROOT_KEY_HEX="1f58396696deec97a0090fa612eb5c4784f0b114693696916c01e9cff8ebc5c7ec0c18caf02a7478c73e54bf13a06fb11624fd1a233ab5e41217aa7bb3803e04"
export ENEROS_MGT_PROBE_TOKEN="$(head -c 64 /dev/zero | xxd -p -c 128)"
export ENEROS_MGT_PROBE_SIG="$(head -c 72 /dev/zero | xxd -p -c 144)"
export ENEROS_MGT_PROBE_BUNDLE="$ROOT/target/evidence70-vol6/config-bundle-gen2.bin"
export ENEROS_KMS_DIR=configs/kms-dev
ROOT_TASK_FEATURES="sel4-eipc,qemu-test-synth" bash third_party/sel4-build-rust.sh > /tmp/sel4-336.log 2>&1 || { echo "SEL4 FAIL"; tail -5 /tmp/sel4-336.log; exit 3; }
cargo xtask config compile --profile qemu-test --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" > /tmp/cc336.log 2>&1 || { echo "CC FAIL"; exit 2; }
gate_image_raw qemu-test /tmp/image-336.log || { echo "IMAGE FAIL"; exit 3; }
sha256sum target/eneros/artifacts/qemu-test/eneros-qemu-test.raw | tee evidence/70/70.6/image-t-0336.sha256
echo REBUILD336_OK
