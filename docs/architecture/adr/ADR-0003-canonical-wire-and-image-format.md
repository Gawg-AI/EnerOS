# ADR-0003: Canonical wire and image encoding

- Status: Accepted for G0/G1
- Decision: BootInfo, measurement/entropy records, release/install manifests and BOOTCTRL use versioned, bounded, fixed-layout little-endian encodings.

Canonical means one byte representation for one value. Decoders reject unknown versions, non-zero reserved bytes, trailing bytes, duplicate/unsorted fields, overflow, overlap, unsupported permissions, inconsistent lengths and non-canonical re-encoding. Every security-relevant platform, slot, load address, memory length, permission, version, content digest and release identity is inside the Ed25519 signed region.

The disk format is raw GPT with audited GUIDs, sizes, attributes and alignment. `PLATFORM_BOOT` is FAT16 with the UEFI removable-media path. The installer is ISO9660 + El Torito and contains a signed install manifest binding the exact raw payload. Sidecars are informative and never trusted by the verifier.

Parser policy is fail-closed. Compatibility requires an explicit new format version and golden/adversarial vectors; heuristic fallback is forbidden.
