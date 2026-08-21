#!/bin/bash
set -x
cd /home/ahx/文档/EnerOS
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"
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
