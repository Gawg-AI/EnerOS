# EnerOS Boot Configuration

This directory contains boot configuration files for EnerOS Power-Native OS.

## Files

- `build-initramfs.sh` — Builds the initramfs image
- `grub.cfg` — GRUB UEFI boot menu configuration
- `systemd-boot.conf` — systemd-boot entry configuration (alternative to GRUB)

## Building initramfs

```bash
./build-initramfs.sh
```

The initramfs contains:
- `eneros-init` binary (PID 1)
- `eneros-api` binary (for emergency access)
- Essential kernel modules
- Init script that mounts root filesystem and switches to it

## Boot Flow

1. UEFI firmware loads GRUB from EFI System Partition
2. GRUB loads Linux kernel and initramfs
3. Kernel starts and executes initramfs `/init` script
4. Init script mounts essential filesystems (proc, sys, dev)
5. Init script finds and mounts the real root partition
6. Init script uses `switch_root` to switch to real root
7. `eneros-init` starts as PID 1 on the real root filesystem
8. eneros-init starts system services in dependency order

## Boot Parameters

The kernel boot parameters include RT optimizations:
- `isolcpus=2,3` — Isolate CPU cores 2,3 for RT tasks
- `nohz_full=2,3` — Tickless kernel on isolated cores
- `rcu_nocbs=2,3` — Move RCU callbacks to other cores
- `irqaffinity=0,1` — Route interrupts to non-RT cores
- `mlock=1` — Lock all kernel memory at boot

## GRUB vs systemd-boot

EnerOS supports both GRUB and systemd-boot:
- **GRUB** (default): More features, wider hardware support
- **systemd-boot**: Simpler, faster, but requires UEFI

Use `grub.cfg` for GRUB, `systemd-boot.conf` for systemd-boot.
