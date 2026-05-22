//! Extend the system boot volume into adjacent unallocated space using diskpart.

use crate::error::{Result, YoloError};
use crate::gpt::SECTOR_SIZE;
use crate::platform::windows::disk::PhysicalDisk;
use crate::platform::windows::diskpart_cmd::run_diskpart;
use crate::platform::windows::layout::read_disk_layout;
use crate::types::DiskLayout;
use tracing::info;

/// Contiguous unallocated sectors immediately after the boot partition.
pub fn extendable_sectors_after_boot(layout: &DiskLayout) -> u64 {
    let Some(boot) = layout.boot_partition.as_ref() else {
        return 0;
    };
    let boot_end = boot.last_lba;
    let mut next_start = layout.header_last_usable.saturating_add(1);
    for p in &layout.partitions {
        if p.is_unused() || p.index == boot.index {
            continue;
        }
        if p.first_lba > boot_end && p.first_lba < next_start {
            next_start = p.first_lba;
        }
    }
    next_start.saturating_sub(boot_end + 1)
}

/// Boot partition extent in sectors (from primary GPT), for before/after extend checks.
pub fn boot_partition_sectors(layout: &DiskLayout) -> Option<u64> {
    layout.boot_partition.as_ref().map(|p| p.sector_count())
}

pub fn extend_boot_volume(layout: &DiskLayout) -> Result<()> {
    let boot = layout.boot_partition.as_ref().ok_or_else(|| {
        YoloError::other("could not identify boot partition to extend")
    })?;

    let extendable = extendable_sectors_after_boot(layout);
    if extendable == 0 {
        return Err(YoloError::other(
            "no contiguous unallocated space after the boot partition; run inspect to verify layout",
        ));
    }

    let before_sectors = boot_partition_sectors(layout).ok_or_else(|| {
        YoloError::other("could not read boot partition size from GPT before extend")
    })?;

    let letter = system_drive_letter();
    info!(
        drive = %letter,
        gpt_index = boot.index,
        before_sectors,
        extendable_mib = extendable * SECTOR_SIZE / (1024 * 1024),
        "extending boot volume via diskpart"
    );

    let script = format!("select volume {letter}\nextend\nexit\n");
    run_diskpart(&script)?;

    let mut disk = PhysicalDisk::open_readonly(layout.disk_index)?;
    let after_layout = read_disk_layout(&mut disk)?;
    let after_sectors = boot_partition_sectors(&after_layout).ok_or_else(|| {
        YoloError::other("could not read boot partition size from GPT after extend")
    })?;

    if after_sectors <= before_sectors {
        return Err(YoloError::other(format!(
            "boot partition GPT extent did not grow (before {before_sectors} sectors, after {after_sectors} sectors); check Disk Management"
        )));
    }

    info!(
        before_sectors,
        after_sectors,
        grown_sectors = after_sectors - before_sectors,
        "boot volume extend verified via GPT"
    );
    Ok(())
}

fn system_drive_letter() -> String {
    std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".into())
        .trim_end_matches(':')
        .to_ascii_uppercase()
}
