//! Platform-specific disk and WinRE operations.

#[cfg(windows)]
pub mod windows;

#[cfg(not(windows))]
mod stub;

#[cfg(windows)]
pub use windows::{
    confirm_extend, confirm_relocate, extend_boot_partition, extendable_sectors_after_boot,
    inspect_system_disk, query_winre, relocate_workflow,
};

#[cfg(not(windows))]
pub use stub::{
    confirm_extend, confirm_relocate, extend_boot_partition, extendable_sectors_after_boot,
    inspect_system_disk, query_winre, relocate_workflow,
};
