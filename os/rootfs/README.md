# EnerOS Rootfs

Minimal rootfs for EnerOS Power-Native OS, based on musl libc and statically linked Rust binaries.

## Building

### Prerequisites
- Linux build environment
- Rust with musl target: `rustup target add x86_64-unknown-linux-musl`

### Build for x86_64
```bash
./build.sh
```

### Build for ARM64
```bash
ARCH=aarch64 ./build.sh
```

## Contents

The rootfs contains:
- `/bin/eneros-init` — PID 1 init system
- `/bin/eneros-api` — Power application server
- `/etc/eneros/init.toml` — Service configuration
- `/etc/passwd`, `/etc/group` — Minimal user database
- `/dev/` — Essential device nodes
- `/var/lib/eneros/` — Persistent data directory
- `/var/log/eneros/` — Log directory

## Size

Target size: < 50MB (statically linked Rust binaries, no shared libraries)

## Design

- All binaries are statically linked with musl libc
- No package manager (apt/dpkg) — updates via A/B partition OTA
- No shell (except minimal busybox sh if needed for debugging)
- No systemd — eneros-init is PID 1
- No NetworkManager — eneros-netcfg handles networking
