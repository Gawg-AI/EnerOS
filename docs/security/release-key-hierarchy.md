# G1 release-candidate key hierarchy

The G1 QEMU chain has two independent trust layers. UEFI authenticated `db` variables trust an external RSA-3072 certificate used only for PE/COFF Authenticode on the boot manager and installer. The boot manager then trusts an external Ed25519 release public key compiled into that signed application; the corresponding private seed signs release, install and update manifests.

No private key or seed is stored in the repository or final media. The current values in `product/security/key-policy.json` are public fingerprints for a release-candidate ceremony. The raw seed and RSA private key remain outside the workspace. Build commands require their explicit path or a previously signed byte-identical EFI input. The artifact inspector receives public trust independently and verifies both layers.

Dev/test trust may be used only by `host-unit` and `qemu-dev`. Release-class profiles require `allow_test_keys=false`, `allow_unsigned=false` and an external key source. A filename, comment or key length never upgrades trust class.

Ceremony record: generate keys outside the repository; record public fingerprints and roles; enroll only the UEFI certificate into a copied vars template; sign one deterministic EFI input; verify and strip the signature to prove byte identity; sign canonical manifests; archive public evidence and keep private inputs offline. Rotation changes key ID and re-runs all install, boot, rollback and recovery tests.
