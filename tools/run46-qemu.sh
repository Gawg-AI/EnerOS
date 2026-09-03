#!/bin/bash
set -x
source "$(dirname "$0")/gate-env.sh"
cd /home/ahx/文档/EnerOS
exec cargo xtask run qemu-prod \
  --image target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  --vars-template target/evidence46-l2/template.fd \
  --expect PASS \
  --evidence-dir target/evidence46/qemu-l2 \
  --net-mode present \
  --net-user \
  --net-protocols present \
  --reset-bootctrl \
  --qmp-shutdown \
  --ready-marker product-ready \
  --qmp-deadline-secs 300
