//! Extend the system boot volume into adjacent unallocated space.

use crate::error::{Result, YoloError};
use crate::gpt::SECTOR_SIZE;
use crate::platform::windows::disk::{system_volume_device_path, PhysicalDisk};
use crate::platform::windows::diskpart_cmd::run_diskpart;
use crate::platform::windows::layout::read_disk_layout;
use crate::types::{DiskLayout, ExtendSummary};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use tracing::{info, warn};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, IOCTL_VOLUME_UPDATE_PROPERTIES, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{FSCTL_EXTEND_VOLUME, FSCTL_GET_NTFS_VOLUME_DATA};
use windows::Win32::System::IO::DeviceIoControl;

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

pub fn extend_boot_volume(layout: &DiskLayout) -> Result<ExtendSummary> {
    let boot = layout
        .boot_partition
        .as_ref()
        .ok_or_else(|| YoloError::other("could not identify boot partition to extend"))?;

    let extendable = extendable_sectors_after_boot(layout);
    let before_sectors = boot_partition_sectors(layout).ok_or_else(|| {
        YoloError::other("could not read boot partition size from GPT before extend")
    })?;

    if extendable == 0 {
        return extend_ntfs_only(layout, before_sectors);
    }

    let letter = system_drive_letter();
    let extend_bytes = extendable
        .checked_mul(SECTOR_SIZE)
        .ok_or_else(|| YoloError::other("extend size overflow"))?;
    let win_part = layout.windows_partition_number(boot);

    info!(
        drive = %letter,
        gpt_index = boot.index,
        windows_partition = win_part,
        before_sectors,
        extendable_mib = extendable * SECTOR_SIZE / (1024 * 1024),
        "extending boot volume"
    );

    // Use the storage driver to grow the live boot partition. Raw GPT edits on C: while
    // mounted can crash the system; recovery relocate targets a separate partition instead.
    let disk = PhysicalDisk::open(layout.disk_index)?;
    disk.grow_partition(win_part, extend_bytes)?;
    disk.update_properties()?;

    let mut disk = PhysicalDisk::open_readonly(layout.disk_index)?;
    let after_layout = read_disk_layout(&mut disk)?;
    let after_sectors = boot_partition_sectors(&after_layout).ok_or_else(|| {
        YoloError::other("could not read boot partition size from GPT after extend")
    })?;

    if after_sectors <= before_sectors {
        return Err(YoloError::other(format!(
            "boot partition GPT extent did not grow (before {before_sectors} sectors, after {after_sectors} sectors)"
        )));
    }

    let partition_bytes = after_sectors
        .checked_mul(SECTOR_SIZE)
        .ok_or_else(|| YoloError::other("partition size overflow"))?;
    extend_filesystem_to_partition(partition_bytes)?;

    let extendable_after = extendable_sectors_after_boot(&after_layout);
    info!(
        before_sectors,
        after_sectors,
        grown_sectors = after_sectors - before_sectors,
        extendable_after,
        "boot volume extend verified via GPT and NTFS"
    );
    Ok(ExtendSummary {
        before_sectors,
        after_sectors,
        extendable_after_sectors: extendable_after,
    })
}

/// Partition already fills available space but NTFS may still be short (e.g. after a prior run).
fn extend_ntfs_only(_layout: &DiskLayout, before_sectors: u64) -> Result<ExtendSummary> {
    let partition_bytes = before_sectors
        .checked_mul(SECTOR_SIZE)
        .ok_or_else(|| YoloError::other("partition size overflow"))?;
    let path = system_volume_device_path();
    volume_update_properties(&path)?;
    let ntfs_bytes = ntfs_volume_bytes(&path)?;
    if filesystem_covers_partition(ntfs_bytes, partition_bytes) {
        return Err(YoloError::other(
            "no contiguous unallocated space after the boot partition; run inspect to verify layout",
        ));
    }
    info!(
        ntfs_bytes,
        partition_bytes, "partition already grown; extending NTFS only"
    );
    extend_filesystem_to_partition(partition_bytes)?;
    Ok(ExtendSummary {
        before_sectors,
        after_sectors: before_sectors,
        extendable_after_sectors: 0,
    })
}

/// Grow NTFS to fill the GPT partition and verify Explorer-visible capacity.
fn extend_filesystem_to_partition(partition_bytes: u64) -> Result<()> {
    let path = system_volume_device_path();
    volume_update_properties(&path)?;

    let before = ntfs_volume_bytes(&path)?;
    if filesystem_covers_partition(before, partition_bytes) {
        info!(
            ntfs_bytes = before,
            partition_bytes, "NTFS volume already fills the partition"
        );
        return Ok(());
    }

    info!(
        ntfs_bytes = before,
        partition_bytes,
        need_bytes = partition_bytes.saturating_sub(before),
        "extending NTFS into grown partition"
    );

    let delta = partition_bytes.saturating_sub(before);
    // 0 means "extend to the current partition size" per FSCTL_EXTEND_VOLUME.
    for grow in [
        0i64,
        i64::try_from(delta).map_err(|_| {
            YoloError::other(format!("NTFS extend delta {delta} bytes exceeds i64::MAX"))
        })?,
    ] {
        volume_update_properties(&path)?;
        if fsctl_extend_volume(&path, grow).is_ok() {
            volume_update_properties(&path)?;
            let after = ntfs_volume_bytes(&path)?;
            if filesystem_covers_partition(after, partition_bytes) {
                info!(
                    ntfs_before = before,
                    ntfs_after = after,
                    partition_bytes,
                    grow,
                    "NTFS volume extended via FSCTL_EXTEND_VOLUME"
                );
                return Ok(());
            }
        }
    }

    warn!("FSCTL_EXTEND_VOLUME did not grow NTFS; trying diskpart extend");
    extend_via_diskpart()?;
    volume_update_properties(&path)?;

    let after = ntfs_volume_bytes(&path)?;
    if filesystem_covers_partition(after, partition_bytes) {
        info!(
            ntfs_before = before,
            ntfs_after = after,
            partition_bytes,
            "NTFS volume extended via diskpart"
        );
        return Ok(());
    }

    Err(YoloError::other(format!(
        "partition grew to {partition_bytes} bytes but NTFS stayed at {after} bytes (was {before}); try `diskpart` → select volume C → extend"
    )))
}

fn filesystem_covers_partition(fs_bytes: u64, partition_bytes: u64) -> bool {
    let Ok(cluster) = ntfs_cluster_bytes() else {
        return fs_bytes + SECTOR_SIZE >= partition_bytes;
    };
    fs_bytes.saturating_add(cluster.saturating_sub(1)) >= partition_bytes
}

fn open_volume_device(path: &str) -> Result<OwnedHandle> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let raw = CreateFileW(
            PCWSTR(wide.as_ptr()),
            (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("CreateFileW({path:?}): {}", e.code().0),
        })?;
        Ok(OwnedHandle::from_raw_handle(raw.0 as _))
    }
}

fn volume_update_properties(path: &str) -> Result<()> {
    let handle = open_volume_device(path)?;
    unsafe {
        DeviceIoControl(
            HANDLE(handle.as_raw_handle()),
            IOCTL_VOLUME_UPDATE_PROPERTIES,
            None,
            0,
            None,
            0,
            None,
            None,
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("IOCTL_VOLUME_UPDATE_PROPERTIES on {path:?}: {}", e.code().0),
        })?;
    }
    Ok(())
}

fn ntfs_volume_bytes(path: &str) -> Result<u64> {
    let handle = open_volume_device(path)?;
    let mut data = windows::Win32::System::Ioctl::NTFS_VOLUME_DATA_BUFFER::default();
    unsafe {
        DeviceIoControl(
            HANDLE(handle.as_raw_handle()),
            FSCTL_GET_NTFS_VOLUME_DATA,
            None,
            0,
            Some(&mut data as *mut _ as *mut _),
            std::mem::size_of::<windows::Win32::System::Ioctl::NTFS_VOLUME_DATA_BUFFER>() as u32,
            None,
            None,
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("FSCTL_GET_NTFS_VOLUME_DATA on {path:?}: {}", e.code().0),
        })?;
    }
    let bytes_per_sector = data.BytesPerSector as u64;
    let number_sectors = data.NumberSectors as u64;
    Ok(number_sectors.saturating_mul(bytes_per_sector))
}

fn ntfs_cluster_bytes() -> Result<u64> {
    let path = system_volume_device_path();
    let handle = open_volume_device(&path)?;
    let mut data = windows::Win32::System::Ioctl::NTFS_VOLUME_DATA_BUFFER::default();
    unsafe {
        DeviceIoControl(
            HANDLE(handle.as_raw_handle()),
            FSCTL_GET_NTFS_VOLUME_DATA,
            None,
            0,
            Some(&mut data as *mut _ as *mut _),
            std::mem::size_of::<windows::Win32::System::Ioctl::NTFS_VOLUME_DATA_BUFFER>() as u32,
            None,
            None,
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("FSCTL_GET_NTFS_VOLUME_DATA on {path:?}: {}", e.code().0),
        })?;
    }
    Ok(data.BytesPerCluster as u64)
}

fn fsctl_extend_volume(path: &str, grow_bytes: i64) -> Result<()> {
    let handle = open_volume_device(path)?;
    unsafe {
        DeviceIoControl(
            HANDLE(handle.as_raw_handle()),
            FSCTL_EXTEND_VOLUME,
            Some(&grow_bytes as *const i64 as *const _),
            std::mem::size_of::<i64>() as u32,
            None,
            0,
            None,
            None,
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!(
                "FSCTL_EXTEND_VOLUME on {path:?} (grow {grow_bytes}): {}",
                e.code().0
            ),
        })?;
    }
    Ok(())
}

fn extend_via_diskpart() -> Result<()> {
    let letter = system_drive_letter();
    let script = format!("select volume {letter}\nextend\nexit\n");
    run_diskpart(&script).map(|_| ())
}

fn system_drive_letter() -> String {
    std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".into())
        .trim_end_matches(':')
        .to_ascii_uppercase()
}
