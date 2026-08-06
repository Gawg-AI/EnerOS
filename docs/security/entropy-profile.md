# EnerOS G0/G1 entropy profile

The boot manager accepts only firmware RNG protocols backed by the product profile (`virtio-rng` in QEMU). It reads 128 raw bytes, applies repetition, repeated-block and bit-balance health checks, then derives a 64-byte boot seed with SHA-512 domain separation. Raw input is cleared before handoff.

The seed record identifies the source and health result. Callers derive per-purpose material with a distinct domain label and context; direct reuse as a signing, encryption or nonce key is forbidden. Missing or unhealthy entropy returns `ENTROPY_UNAVAILABLE` before BOOTCTRL or protected-anchor writes. There is no timer, address, fixed seed or unsigned fallback.

This G0/G1 service provides boot entropy only. A long-lived user-space DRBG/key service remains a later system-service capability and must not be inferred from this boot seed.
