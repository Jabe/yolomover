//! Platform-specific disk and WinRE operations.

#[cfg(windows)]
pub mod windows;

#[cfg(not(windows))]
mod stub;

#[cfg(windows)]
pub use windows::{
    boot_partition_sectors, confirm_extend, confirm_relocate, extend_boot_partition,
    extendable_sectors_after_boot, inspect_system_disk, inspect_winre_partition, query_winre,
    relocate_workflow, verify_winre_partition,
};

#[cfg(not(windows))]
pub use stub::{
    confirm_extend, confirm_relocate, extend_boot_partition, extendable_sectors_after_boot,
    inspect_system_disk, inspect_winre_partition, query_winre, relocate_workflow,
    verify_winre_partition,
};
