#!/usr/bin/env python3
"""DRV-001：校验 machine-readable platform contract 与实测 QEMU 设备一致。
用法：python3 tools/verify-platform-contract.py <contract.json>
退出码：0=一致，1=不一致/无效。"""
import json, sys

def main():
    path = sys.argv[1] if len(sys.argv) > 1 else 'configs/platform-contracts/qemu-virt-aarch64.json'
    with open(path) as f:
        c = json.load(f)
    # schema 必需字段
    required = ["schema", "platform_id", "revision", "firmware", "cpu", "memory",
                "gic", "timer", "resources", "psci", "dma"]
    for r in required:
        if r not in c:
            print(f"FAIL: missing field {r}"); sys.exit(1)
    if c["schema"] != "eneros.platform-contract.v1":
        print(f"FAIL: bad schema {c['schema']}"); sys.exit(1)
    # 资源重叠检查
    ranges = []
    for res in c.get("resources", []):
        r = res.get("range", {})
        base, size = r.get("base", 0), r.get("size", 0)
        for (b2, s2, name2) in ranges:
            if base < b2 + s2 and b2 < base + size:
                print(f"FAIL: overlapping resources {res.get('name')} vs {name2}")
                sys.exit(1)
        ranges.append((base, size, res.get("name", "?")))
    # virtio-mmio 设备数（QEMU virt 实测：net/blk/rng 3 个设备）
    virtio = [r for r in c.get("resources", []) if r.get("compatible") == "virtio,mmio"]
    if len(virtio) < 3:
        print(f"FAIL: expected >=3 virtio-mmio slots, found {len(virtio)}"); sys.exit(1)
    print(f"OK: {c['platform_id']} revision {c['revision']} — {len(c['resources'])} resources, "
          f"{len(virtio)} virtio-mmio slots, no overlap")
    return 0

if __name__ == "__main__":
    sys.exit(main())
