# EnerOS Linux Kernel

EnerOS uses Linux kernel with PREEMPT_RT patch for real-time performance.

## Building

### Prerequisites
- Linux build environment (Debian/Ubuntu recommended)
- GCC cross-compiler (for ARM64)
- Build dependencies: `bc bison flex libssl-dev libelf-dev`

### Build for x86_64
```bash
./build.sh
```

### Build for ARM64
```bash
ARCH=aarch64 ./build.sh
```

### Custom kernel version
```bash
KERNEL_VERSION=6.6 RT_PATCH_VERSION=6.6-rt23 ./build.sh
```

## Configuration

Kernel configs are in:
- `config-x86_64` - x86_64 configuration
- `config-aarch64` - ARM64 configuration

Key features enabled:
- PREEMPT_RT (real-time preemption)
- CPU isolation (isolcpus)
- High-resolution timers
- No-HZ full (tickless kernel)
- Hardware watchdog support
- AF_PACKET (for GOOSE/SV protocols)
- AppArmor security
- Module signing

## Boot parameters

Recommended kernel boot parameters for RT performance:
```
isolcpus=2,3 nohz_full=2,3 rcu_nocbs=2,3 irqaffinity=0,1 mlock=1
```
