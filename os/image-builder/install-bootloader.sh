#!/bin/bash
# EnerOS bootloader installation script
# Installs GRUB UEFI bootloader to the EFI System Partition
# This file is sourced by build.sh

install_bootloader() {
    local efi_mount="$1"
    local root_mount="$2"
    local arch="$3"
    
    echo "  Installing GRUB UEFI bootloader..."
    
    # Determine GRUB target
    local grub_target
    case "$arch" in
        x86_64)  grub_target="x86_64-efi" ;;
        aarch64) grub_target="arm64-efi" ;;
        *) echo "Unsupported arch: $arch"; exit 1 ;;
    esac
    
    # Create EFI directory structure
    mkdir -p "$efi_mount/EFI/BOOT"
    
    # Install GRUB EFI binary
    # In production, this would use grub-install:
    # grub-install --target=$grub_target --efi-directory=$efi_mount \
    #              --bootloader-id=ENEROS --removable
    
    # For now, copy GRUB EFI binary if available
    if [ -f "/usr/lib/grub/$grub_target/grubx64.efi" ] && [ "$arch" = "x86_64" ]; then
        cp "/usr/lib/grub/$grub_target/grubx64.efi" "$efi_mount/EFI/BOOT/BOOTX64.EFI"
    elif [ -f "/usr/lib/grub/$grub_target/grubaa64.efi" ] && [ "$arch" = "aarch64" ]; then
        cp "/usr/lib/grub/$grub_target/grubaa64.efi" "$efi_mount/EFI/BOOT/BOOTAA64.EFI"
    else
        echo "  WARNING: GRUB EFI binary not found, using grub-install..."
        grub-install --target=$grub_target --efi-directory="$efi_mount" \
                     --bootloader-id=ENEROS --removable \
                     --boot-directory="$root_mount/boot" 2>/dev/null || \
        echo "  WARNING: grub-install failed, manual installation required"
    fi
    
    # Copy GRUB configuration
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    BOOT_DIR="$(dirname "$SCRIPT_DIR")/boot"
    
    mkdir -p "$root_mount/boot/grub"
    cp "$BOOT_DIR/grub.cfg" "$root_mount/boot/grub/grub.cfg"
    
    # Also copy to EFI partition for fallback
    mkdir -p "$efi_mount/EFI/ENEROS"
    cp "$BOOT_DIR/grub.cfg" "$efi_mount/EFI/ENEROS/grub.cfg" 2>/dev/null || true
    
    echo "  Bootloader installed"
}
