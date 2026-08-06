# Secure Boot root binding

The QEMU product uses immutable AAVMF code plus a copied authenticated-vars template that contains the release-candidate `db` certificate. The raw disk contains a signed `EFI/BOOT/BOOTAA64.EFI`, but contains no setting capable of replacing the firmware root. The boot manager embeds the Ed25519 release public key inside its Authenticode-covered PE image.

The harness mounts firmware code read-only, copies vars per run, and never uses QEMU `-kernel` or `-loader`. The inspector extracts EFI applications from FAT/El Torito and validates them against the independently supplied certificate. An ESP byte mutation fails Authenticode; a wrong certificate fails verification; an unsigned application is rejected by firmware before EnerOS entry.

Production boards must bind the analogous root through authenticated firmware variables or an equivalent immutable platform mechanism. A normal CONFIG/DATA update cannot change it.
