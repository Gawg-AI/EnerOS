#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod boot_control;
pub mod boot_info;
pub mod entropy;
pub mod install_manifest;
#[cfg(feature = "alloc")]
pub mod manifest;
pub mod measurement;
pub mod update_manifest;

pub use boot_control::{
    resolve_boot_control, BootControl, BootControlAnchor, BootControlCopy, BootControlError,
    BootControlTransaction, BootSelection, ResolvedBootControl, SlotError, SlotState,
    BOOT_CONTROL_ANCHOR_SIZE, BOOT_CONTROL_COPY_OFFSETS, BOOT_CONTROL_FLAG_MANUAL_RECOVERY,
    BOOT_CONTROL_RECORD_SIZE,
};
pub use boot_info::{
    dtb_window_for, validate_memory_map_bytes, BootInfo, BootInfoError, BootReason, BootSlot,
    MemoryMapEntry, MemoryRegionKind, BOOT_INFO_ABI_MAJOR, BOOT_INFO_ABI_MINOR, BOOT_INFO_SIZE,
    ENEROS_DTB_MAX_LEN, ENEROS_INIT_BUNDLE_PHYS, ENEROS_MAPPED_WINDOW_LEN,
};
pub use entropy::{
    condition_boot_entropy, EntropyError, CONDITIONED_BOOT_SEED_SIZE, RAW_BOOT_ENTROPY_SIZE,
};
pub use install_manifest::{
    InstallManifest, InstallManifestError, INSTALL_MANIFEST_SIGNATURE_SIZE,
    INSTALL_MANIFEST_SIGNED_SIZE, INSTALL_MANIFEST_SIZE,
};
#[cfg(feature = "alloc")]
pub use manifest::{
    Component, ComponentRole, ManifestError, ManifestHeader, ManifestSlot, ProductVersion,
    ReleaseManifest, COMPONENT_ENCODED_LEN, MANIFEST_HEADER_LEN, MANIFEST_MAX_COMPONENTS,
    MANIFEST_MAX_LEN, MANIFEST_SIGNATURE_LEN,
};
pub use measurement::{
    MeasurementError, MeasurementKind, MeasurementLog, MeasurementRecord, MEASUREMENT_LOG_SIZE,
    MEASUREMENT_MAX_RECORDS,
};
pub use update_manifest::{
    UpdateManifest, UpdateManifestError, UpdatePayloadKind, UPDATE_MANIFEST_SIGNATURE_SIZE,
    UPDATE_MANIFEST_SIGNED_SIZE, UPDATE_MANIFEST_SIZE,
};
