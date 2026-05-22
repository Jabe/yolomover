use crate::error::{Result, YoloError};
use crate::types::{DiskLayout, RelocationPlan, RelocateSummary, WinReStatus};

pub fn inspect_system_disk(_disk_index: Option<u32>) -> Result<(DiskLayout, WinReStatus)> {
    Err(YoloError::NotWindows)
}

pub fn query_winre() -> Result<WinReStatus> {
    Err(YoloError::NotWindows)
}

pub fn inspect_winre_partition(_disk_index: u32, _partition_number: u32) -> crate::types::WinRePartitionInspect {
    crate::types::WinRePartitionInspect {
        windows_path: String::new(),
        winre_wim_bytes: None,
        boot_sdi_bytes: None,
    }
}

pub fn verify_winre_partition(_disk_index: u32, _partition_number: u32) -> Result<bool> {
    Err(YoloError::NotWindows)
}

#[allow(dead_code)]
pub fn boot_partition_sectors(_layout: &DiskLayout) -> Option<u64> {
    None
}

pub fn confirm_relocate(_plan: &RelocationPlan) -> Result<()> {
    Err(YoloError::NotWindows)
}

pub fn confirm_extend(_layout: &DiskLayout) -> Result<()> {
    Err(YoloError::NotWindows)
}

pub fn extend_boot_partition(_layout: &DiskLayout) -> Result<()> {
    Err(YoloError::NotWindows)
}

pub fn extendable_sectors_after_boot(_layout: &DiskLayout) -> u64 {
    0
}

pub fn relocate_workflow(_plan: &RelocationPlan, _dry_run: bool) -> Result<RelocateSummary> {
    Err(YoloError::NotWindows)
}
