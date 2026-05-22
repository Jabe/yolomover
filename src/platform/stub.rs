use crate::error::{Result, YoloError};
use crate::types::{DiskLayout, RelocationPlan, RunSummary, WinReStatus};

pub fn inspect_system_disk(_disk_index: Option<u32>) -> Result<(DiskLayout, WinReStatus)> {
    Err(YoloError::NotWindows)
}

pub fn query_winre() -> Result<WinReStatus> {
    Err(YoloError::NotWindows)
}

pub fn confirm_run(_plan: &RelocationPlan) -> Result<()> {
    Err(YoloError::NotWindows)
}

pub fn run_relocation(_plan: &RelocationPlan, _dry_run: bool) -> Result<()> {
    Err(YoloError::NotWindows)
}

pub fn extend_boot_partition(_layout: &DiskLayout) -> Result<()> {
    Err(YoloError::NotWindows)
}

pub fn run_workflow(
    _plan: &RelocationPlan,
    _dry_run: bool,
    _extend_c: bool,
) -> Result<RunSummary> {
    Err(YoloError::NotWindows)
}
