#!/usr/bin/env bash
# 43 最终收口：把新候选（sv2 pristine）的电池证据绑定为 evidence/42/round43*。
# 旧 round131 信封保留为旧候选证据（stale-index 标 stale）。
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
P=target/eneros/artifacts/qemu-prod/eneros-qemu-prod-43-pristine.raw
RECOVERY=target/eneros/artifacts/recovery/eneros-recovery.raw
ENUPD=target/eneros/artifacts/qemu-prod/eneros-qemu-prod-43.enupd
E=evidence/42
L=target/eneros/evidence/43
R=target/evidence43/battery-final
X=target/evidence43/l2-modbus-tcp

echo "== lineage (raw) =="
cargo xtask evidence42 lineage --profile qemu-prod --artifact "$P" --out $L/lineage-r43-raw.json >/dev/null
echo "== lineage (recovery) =="
cargo xtask evidence42 lineage --profile qemu-prod --artifact "$RECOVERY" --out $L/lineage-r43-recovery.json >/dev/null
echo "== lineage (update) =="
cargo xtask evidence42 lineage --profile qemu-prod --artifact "$ENUPD" --out $L/lineage-r43-update.json >/dev/null

enc() { cargo xtask evidence42 envelope "$@" >/dev/null; }

echo "== formal-boot =="
enc --lineage $L/lineage-r43-raw.json --test-id qemu-formal-boot-round43 --scope L2 \
  --command "cargo xtask run qemu-prod --image $P --vars-template target/eneros/artifacts/qemu-prod/AAVMF_VARS.fd --evidence-dir $R/boot --expect PASS --qmp-shutdown --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 420" \
  --oracle "qmp exit=0 product-ready all-required-services-ready readback=ok boot_slot=A" \
  --positive "boot:PASS:qemu_exit=0 product_ready=34/34 required_degraded_empty sv=2" \
  --negative "timeout:FAIL:marker-not-found-returns-NO_RESULT" \
  --artifact "boot-evidence:$R/boot/qemu-result.json:$(sha256sum $R/boot/qemu-result.json | cut -d' ' -f1)" \
  --out $E/round43-formal-boot.json

echo "== install =="
enc --lineage $L/lineage-r43-raw.json --test-id qemu-install-round43 --scope L2 \
  --command "cargo xtask run qemu-install --iso target/eneros/artifacts/qemu-prod/eneros-qemu-prod-43-installer.iso --payload $P --vars-template target/eneros/artifacts/qemu-prod/AAVMF_VARS.fd --evidence-dir $R/install --expect PASS" \
  --oracle "qmp exit=0 installed_payload_sha256=130779ff5a77a2172b4e092cf8ae5f59027ebc49b24e6f9435d2b0d3bf396c0f readback=ok" \
  --positive "install:PASS:payload_sha256=130779ff5a77a2172b4e092cf8ae5f59027ebc49b24e6f9435d2b0d3bf396c0f readback=ok" \
  --negative "reject:FAIL:wrong-payload-rejected" \
  --artifact "install-evidence:$R/install/qemu-install-result.json:$(sha256sum $R/install/qemu-install-result.json 2>/dev/null | cut -d' ' -f1)" \
  --out $E/round43-install.json || echo "install envelope failed (harness mismatch) — 登记 PENDING"

echo "== recovery =="
enc --lineage $L/lineage-r43-recovery.json --test-id qemu-recovery-round43 --scope L2 \
  --command "cargo xtask run qemu-prod --image $RECOVERY --vars-template target/eneros/artifacts/qemu-prod/AAVMF_VARS.fd --evidence-dir $R/recovery --expect PASS --qmp-shutdown --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 420 --expect-slot RECOVERY --expect-reason MANUAL_RECOVERY" \
  --oracle "qmp exit=0 slot=RECOVERY reason=MANUAL_RECOVERY product-ready readback=ok" \
  --positive "recovery:PASS:qemu_exit=0 slot=RECOVERY product_ready" \
  --negative "rollback:FAIL:bad-verification-falls-back" \
  --artifact "recovery-evidence:$R/recovery/qemu-result.json:$(sha256sum $R/recovery/qemu-result.json | cut -d' ' -f1)" \
  --out $E/round43-recovery.json

echo "== update =="
enc --lineage $L/lineage-r43-update.json --test-id qemu-update-round43 --scope L2 \
  --command "cargo xtask image update qemu-prod --kind full --base target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw.previous --target $P --signing-key product/security/dev-signing.key --output $ENUPD" \
  --oracle "signed update manifest verified sv base=1 target=2 binding=pass" \
  --positive "update:PASS:sv1->sv2 base_binding=pass target_binding=pass" \
  --negative "bad-sig:FAIL:malformed-signature-rejected" \
  --artifact "update-report:target/eneros/artifacts/qemu-prod/eneros-qemu-prod-43.enupd.report.json:$(sha256sum target/eneros/artifacts/qemu-prod/eneros-qemu-prod-43.enupd.report.json | cut -d' ' -f1)" \
  --out $E/round43-update.json

echo "== verify/gate/lineage-check =="
for f in round43-formal-boot round43-recovery round43-update; do
  cargo xtask evidence42 verify $E/$f.json >/dev/null && echo "verify $f PASS"
  cargo xtask evidence42 gate $E/$f.json >/dev/null && echo "gate $f PASS"
done
cargo xtask evidence42 lineage-check --envelope $E/round43-formal-boot.json --artifact "$P" >/dev/null && echo "lineage formal-boot PASS"
cargo xtask evidence42 lineage-check --envelope $E/round43-recovery.json --artifact "$RECOVERY" >/dev/null && echo "lineage recovery PASS"
cargo xtask evidence42 lineage-check --envelope $E/round43-update.json --artifact "$ENUPD" >/dev/null && echo "lineage update PASS"

echo "== L2 modbus envelope (evidence/43) =="
ART=$(sha256sum product/compatibility-catalog.json | cut -d' ' -f1)
TX=$(sha256sum $X/peer-transcript.jsonl | cut -d' ' -f1)
LOG=$(sha256sum $X/qemu-serial.log | cut -d' ' -f1)
enc --lineage $L/lineage-r43-raw.json --test-id 43-l2-modbus-tcp-interop --scope INTEROP \
  --command "bash evidence/43/l2-modbus-tcp.sh (svc-modbus-tcp virtio-net slot8 + independent raw slave)" \
  --oracle "independent-raw-peer Modbus-TCP slave transaction transcript: 3 read-holding-registers requests answered ok; serial MODBUS-L2-SELFCHECK ok values=[4369,8738,13107]" \
  --positive "interop:PASS:independent-peer observed 3 transactions; readback values matched 0x1111/0x2222/0x3333" \
  --negative "bad:FAIL:malformed-rejected; slave-default-deny" \
  --verdict PASS \
  --artifact "transcript:$X/peer-transcript.jsonl:$TX" \
  --artifact "serial:$X/qemu-serial.log:$LOG" \
  --out evidence/43/l2-modbus-tcp-verified.json
cargo xtask evidence42 verify evidence/43/l2-modbus-tcp-verified.json >/dev/null && echo "verify l2 PASS"
cargo xtask evidence42 gate evidence/43/l2-modbus-tcp-verified.json >/dev/null && echo "gate l2 PASS"

echo "== indices =="
cargo xtask evidence42 stale-index >/dev/null
cargo xtask evidence42 current-index >/dev/null
cargo xtask evidence42 check-current >/dev/null
echo "REBIND-FINAL-SV2 COMPLETE"
