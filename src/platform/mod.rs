//! Platform-specific disk and WinRE operations.

#[cfg(windows)]
pub mod windows;

#[cfg(not(windows))]
mod stub;

#[cfg(windows)]
pub use windows::{
    confirm_run, extend_boot_partition, inspect_system_disk, query_winre, run_relocation,
    run_workflow,
};

#[cfg(not(windows))]
pub use stub::{
    confirm_run, extend_boot_partition, inspect_system_disk, query_winre, run_relocation,
    run_workflow,
};
