#!/usr/bin/env bash
# 42（Round 131 final）：为当前候选 lineage 重新绑定全部 L2/L3 Envelope。
# 用途：任何 tracked 工具改动使 dirty digest 变化后，仅重生成 Envelope
# （QEMU/soak 实跑证据文件不变；Envelope 是候选绑定的包装层）。
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

RAW=target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw
RECOVERY=target/eneros/artifacts/recovery/eneros-recovery.raw
ENUPD=target/eneros/artifacts/qemu-prod/eneros-qemu-prod-round131-full.enupd
ABDISK=target/evidence42/round131-ab/disk-trial.raw
E=evidence/42
L=target/eneros/evidence/42

echo "== lineage (raw) =="
cargo xtask evidence42 lineage --profile qemu-prod --artifact "$RAW" --out $L/lineage-r131-raw.json >/dev/null
echo "== lineage (recovery) =="
cargo xtask evidence42 lineage --profile qemu-prod --artifact "$RECOVERY" --out $L/lineage-r131-recovery.json >/dev/null
echo "== lineage (update) =="
cargo xtask evidence42 lineage --profile qemu-prod --artifact "$ENUPD" --out $L/lineage-r131-update.json >/dev/null
echo "== lineage (ab disk) =="
cargo xtask evidence42 lineage --profile qemu-prod --artifact "$ABDISK" --out $L/lineage-r131-ab.json >/dev/null

enc() { cargo xtask evidence42 envelope "$@" >/dev/null; }

echo "== formal-boot =="
enc --lineage $L/lineage-r131-raw.json --test-id qemu-formal-boot-round131 --scope L2 \
  --command "cargo xtask run qemu-prod --image $RAW --vars-template target/eneros/artifacts/qemu-prod/AAVMF_VARS.fd --evidence-dir target/evidence42/round131-boot-pr --expect PASS --qmp-shutdown --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 420" \
  --oracle "qmp exit=0 product-ready all-required-services-ready readback=ok boot_slot=B" \
  --positive "boot:PASS:qemu_exit=0 product_ready=34/34 required_degraded_empty" \
  --negative "timeout:FAIL:marker-not-found-returns-NO_RESULT" \
  --artifact "boot-evidence:target/evidence42/round131-boot-pr/qemu-result.json:$(sha256sum target/evidence42/round131-boot-pr/qemu-result.json | cut -d' ' -f1)" \
  --out $E/round131-formal-boot.json

echo "== install =="
enc --lineage $L/lineage-r131-raw.json --test-id qemu-install-round131 --scope L2 \
  --command "cargo xtask run qemu-install --iso target/eneros/artifacts/qemu-prod/eneros-qemu-prod-round34-installer.iso --payload $RAW --vars-template target/eneros/artifacts/qemu-prod/AAVMF_VARS.fd --evidence-dir target/evidence42/round131-install --expect PASS" \
  --oracle "qmp exit=0 installed_payload_sha256=raw readback=ok boot-after-iso-removal=pass" \
  --positive "install:PASS:payload_sha256=c8c9dd45cb3c65ede682ca63b30c9bc27e496b9bc3a5d2c968a5b7a5b82ea4ec readback=ok" \
  --negative "reject:FAIL:wrong-payload-rejected" \
  --artifact "install-evidence:target/evidence42/round131-install/qemu-install-result.json:$(sha256sum target/evidence42/round131-install/qemu-install-result.json | cut -d' ' -f1)" \
  --out $E/round131-install.json

echo "== recovery =="
enc --lineage $L/lineage-r131-recovery.json --test-id qemu-recovery-round131 --scope L2 \
  --command "cargo xtask run qemu-prod --image $RECOVERY --vars-template target/eneros/artifacts/qemu-prod/AAVMF_VARS.fd --evidence-dir target/evidence42/round131-recovery --expect PASS --qmp-shutdown --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 420 --expect-slot RECOVERY --expect-reason MANUAL_RECOVERY" \
  --oracle "qmp exit=0 slot=RECOVERY reason=MANUAL_RECOVERY product-ready readback=ok" \
  --positive "recovery:PASS:qemu_exit=0 slot=RECOVERY product_ready" \
  --negative "rollback:FAIL:bad-verification-falls-back" \
  --artifact "recovery-evidence:target/evidence42/round131-recovery/qemu-result.json:$(sha256sum target/evidence42/round131-recovery/qemu-result.json | cut -d' ' -f1)" \
  --out $E/round131-recovery.json

echo "== update =="
enc --lineage $L/lineage-r131-update.json --test-id qemu-update-round131 --scope L2 \
  --command "cargo xtask image update qemu-prod --kind full --base target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw.previous --target $RAW --signing-key product/security/dev-signing.key --output $ENUPD" \
  --oracle "signed update manifest verified payload_sha256=target raw binding=pass" \
  --positive "update:PASS:package_sha256=7b7a56703d3737928d06e12a8eca19e464f82a8127102305a92df3a395f5d6de base_binding=pass target_binding=pass" \
  --negative "bad-sig:FAIL:malformed-signature-rejected" \
  --artifact "update-report:target/eneros/artifacts/qemu-prod/eneros-qemu-prod-round131-full.enupd.report.json:$(sha256sum target/eneros/artifacts/qemu-prod/eneros-qemu-prod-round131-full.enupd.report.json | cut -d' ' -f1)" \
  --out $E/round131-update.json

echo "== soak-smoke =="
enc --lineage $L/lineage-r131-raw.json --test-id qemu-soak-smoke-round131 --scope SOAK \
  --command "cargo xtask qa soak-run --cycles 3 --min-duration-days 0 --image $RAW --vars-template target/eneros/artifacts/qemu-prod/AAVMF_VARS.fd --out target/round131-soak.json" \
  --oracle "qemu restart 3 cycles pass artifact_sha256=raw metric_samples=3" \
  --positive "soak:PASS:cycles=3 restarts=3 artifact_sha256=c8c9dd45cb3c65ede682ca63b30c9bc27e496b9bc3a5d2c968a5b7a5b82ea4ec metric_samples=3" \
  --negative "timeout:FAIL:marker-not-found-NO_RESULT" \
  --artifact "soak-evidence:target/round131-soak.json:$(sha256sum target/round131-soak.json | cut -d' ' -f1)" \
  --out $E/round131-soak-smoke.json

echo "== ab-update =="
enc --lineage $L/lineage-r131-ab.json --test-id qemu-ab-update-round131 --scope L3 \
  --command "ab_build.py base->trial disk; run qemu-prod disk-trial (expect UPDATE_TRIAL B); run qemu-prod disk-trial same-vars (expect COLD A); run qemu-prod disk-commit (expect COLD B)" \
  --oracle "boot1:slot=B reason=UPDATE_TRIAL sv=2 product-ready; boot2:slot=A reason=COLD sv=1 rollback-on-no-commit; boot3:slot=B reason=COLD sv=2 committed" \
  --positive "trial:PASS:slot=B UPDATE_TRIAL sv=2 bootctrl_generation=1 manifest=597cc65ce8cd26246644a42af0494ecbe7395ac77c83600f391e22c4efc968ad,rollback:PASS:slot=A COLD sv=1 trial-not-committed-falls-back,commit:PASS:slot=B COLD sv=2 active" \
  --negative "timeout:FAIL:marker-not-found-NO_RESULT,bad-manifest:FAIL:slot-verification-failed-code" \
  --artifact "trial-evidence:target/evidence42/round131-ab/boot1-trial/qemu-result.json:$(sha256sum target/evidence42/round131-ab/boot1-trial/qemu-result.json|cut -d' ' -f1)" \
  --out $E/round131-ab-update.json

echo "== verify/gate/lineage-check =="
for f in round131-formal-boot round131-install round131-recovery round131-update round131-soak-smoke round131-ab-update; do
  cargo xtask evidence42 verify $E/$f.json >/dev/null && echo "verify  $f PASS"
  cargo xtask evidence42 gate    $E/$f.json >/dev/null && echo "gate    $f PASS"
done
cargo xtask evidence42 lineage-check --envelope $E/round131-formal-boot.json --artifact "$RAW" >/dev/null && echo "lineage formal-boot PASS"
cargo xtask evidence42 lineage-check --envelope $E/round131-install.json --artifact "$RAW" >/dev/null && echo "lineage install PASS"
cargo xtask evidence42 lineage-check --envelope $E/round131-recovery.json --artifact "$RECOVERY" >/dev/null && echo "lineage recovery PASS"
cargo xtask evidence42 lineage-check --envelope $E/round131-update.json --artifact "$ENUPD" >/dev/null && echo "lineage update PASS"
cargo xtask evidence42 lineage-check --envelope $E/round131-ab-update.json --artifact "$ABDISK" >/dev/null && echo "lineage ab PASS"

echo "== indices =="
cargo xtask evidence42 stale-index >/dev/null
cargo xtask evidence42 current-index >/dev/null
cargo xtask evidence42 check-current >/dev/null
echo "REBIND-131 COMPLETE"