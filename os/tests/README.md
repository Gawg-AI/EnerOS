# EnerOS Boot Tests

This directory contains tests for verifying that EnerOS Power-Native OS boots correctly.

## Test Types

### 1. Unit Tests (`boot_test.rs`)

Rust unit tests that verify the boot logic without requiring QEMU:
- Service graph construction
- Configuration loading
- Startup order validation
- Signal handling setup
- Rootfs structure documentation
- Kernel boot parameters documentation

Run on any platform (Windows/Linux/macOS):
```bash
cargo test --test boot_test
```

Or as part of the workspace:
```bash
cargo test -p eneros-os-tests
```

### 2. Integration Tests (`boot_test.sh`)

Shell script that boots the EnerOS image in QEMU and verifies:
- Kernel boots successfully
- eneros-init starts as PID 1
- Service startup order is correct
- Application layer (eneros-api) starts
- HTTP health check passes

Run on Linux with QEMU installed:
```bash
# First build the image
cd os/image-builder && ./build.sh

# Then run the test
cd os/tests && ./boot_test.sh
```

## Prerequisites

### For Unit Tests
- Rust toolchain (stable)

### For Integration Tests
- Linux environment
- QEMU installed (`apt install qemu-system`)
- EnerOS image built (`os/image-builder/build.sh`)
- Optional: KVM for faster boot (`/dev/kvm`)

## Test Flow

1. **Unit Tests** (development time):
   - Verify init logic is correct
   - Verify configuration parsing
   - Verify service dependency graph

2. **Integration Tests** (CI/CD):
   - Build kernel with PREEMPT_RT
   - Build rootfs with musl libc
   - Build initramfs
   - Build bootable image
   - Boot image in QEMU
   - Verify eneros-init starts
   - Verify application layer starts
   - Verify HTTP health check passes

## CI/CD Integration

In GitHub Actions:
```yaml
- name: Run unit tests
  run: cargo test -p eneros-os-tests

- name: Build image
  run: cd os/image-builder && ./build.sh

- name: Boot test
  run: cd os/tests && ./boot_test.sh
```
