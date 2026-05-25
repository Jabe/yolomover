//! Extend the system boot volume into adjacent unallocated space.

use crate::error::{Result, YoloError};
use crate::gpt::SECTOR_SIZE;
use crate::platform::windows::disk::{system_volume_device_path, PhysicalDisk};
use crate::platform::windows::layout::read_disk_layout;
use crate::platform::windows::win32_code::win32_code;
use crate::types::{DiskLayout, ExtendSummary};
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

pub fn extend_boot_volume(layout: &DiskLayout) -> Result<ExtendSummary> {
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
    let extend_bytes = extendable
        .checked_mul(SECTOR_SIZE)
        .ok_or_else(|| YoloError::other("extend size overflow"))?;
    let win_part = layout.windows_partition_number(boot);
    let vol_before = volume_total_bytes()?;

    info!(
        drive = %letter,
        gpt_index = boot.index,
        windows_partition = win_part,
        before_sectors,
        vol_before,
        extendable_mib = extendable * SECTOR_SIZE / (1024 * 1024),
        "extending boot volume"
    );

    // Use the storage driver to grow the live boot partition. Raw GPT edits on C: while
    // mounted can crash the system; recovery relocate targets a separate partition instead.
    let disk = PhysicalDisk::open(layout.disk_index)?;
    disk.grow_partition(win_part, extend_bytes)?;
    disk.update_properties()?;
    extend_filesystem_if_needed(vol_before, extend_bytes)?;

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

    let extendable_after = extendable_sectors_after_boot(&after_layout);
    info!(
        before_sectors,
        after_sectors,
        grown_sectors = after_sectors - before_sectors,
        extendable_after,
        "boot volume extend verified via GPT"
    );
    Ok(ExtendSummary {
        before_sectors,
        after_sectors,
        extendable_after_sectors: extendable_after,
    })
}

/// Grow NTFS when partition grow did not already expand the mounted filesystem.
fn extend_filesystem_if_needed(vol_before: u64, extend_bytes: u64) -> Result<()> {
    let vol_after_grow = volume_total_bytes()?;
    if filesystem_grew_enough(vol_before, vol_after_grow, extend_bytes) {
        info!(
            vol_before,
            vol_after_grow,
            extend_bytes,
            "filesystem already extended with partition grow"
        );
        return Ok(());
    }

    match extend_ntfs_volume(extend_bytes)? {
        FsExtendResult::Extended => Ok(()),
        FsExtendResult::RejectedInvalidParameter => {
            let vol_now = volume_total_bytes()?;
            if filesystem_grew_enough(vol_before, vol_now, extend_bytes) {
                info!(
                    vol_before,
                    vol_now,
                    extend_bytes,
                    "filesystem extended with partition grow (FSCTL_EXTEND_VOLUME not needed)"
                );
                Ok(())
            } else {
                Err(YoloError::other(format!(
                    "FSCTL_EXTEND_VOLUME rejected ERROR_INVALID_PARAMETER and filesystem size unchanged (before {vol_before} bytes, now {vol_now} bytes, need +{extend_bytes})"
                )))
            }
        }
    }
}

enum FsExtendResult {
    Extended,
    RejectedInvalidParameter,
}

/// True when reported volume capacity increased by at least the requested extend (minus cluster slack).
fn filesystem_grew_enough(before: u64, after: u64, extend_bytes: u64) -> bool {
    filesystem_grew_by_at_least(before, after, extend_bytes, volume_grow_slack())
}

fn filesystem_grew_by_at_least(before: u64, after: u64, extend_bytes: u64, slack: u64) -> bool {
    after.saturating_sub(before).saturating_add(slack) >= extend_bytes
}

fn volume_grow_slack() -> u64 {
    volume_cluster_bytes().unwrap_or(SECTOR_SIZE)
}

fn volume_cluster_bytes() -> Option<u64> {
    let root = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let root = format!(r"{root}\");
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();

    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceW;

    let mut sectors_per_cluster = 0u32;
    let mut bytes_per_sector = 0u32;
    unsafe {
        GetDiskFreeSpaceW(
            PCWSTR(wide.as_ptr()),
            Some(&mut sectors_per_cluster),
            Some(&mut bytes_per_sector),
            None,
            None,
        )
        .ok()?;
    }
    Some(sectors_per_cluster as u64 * bytes_per_sector as u64)
}

fn volume_total_bytes() -> Result<u64> {
    let root = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let root = format!(r"{root}\");
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();

    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut total = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            None,
            Some(&mut total),
            None,
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("GetDiskFreeSpaceExW on {root:?}: {}", e.code().0),
        })?;
    }
    Ok(total)
}

/// Grow the mounted NTFS volume on `%SystemDrive%` into space added to its partition.
fn extend_ntfs_volume(bytes_to_add: u64) -> Result<FsExtendResult> {
    if bytes_to_add == 0 {
        return Ok(FsExtendResult::Extended);
    }

    let path = system_volume_device_path();
    let grow: i64 = bytes_to_add.try_into().map_err(|_| {
        YoloError::other(format!("extend size {bytes_to_add} bytes exceeds i64::MAX"))
    })?;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Ioctl::FSCTL_EXTEND_VOLUME;
    use windows::Win32::System::IO::DeviceIoControl;

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let handle = CreateFileW(
            PCWSTR(wide.as_ptr()),
            (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("CreateFileW({path:?}) for extend: {}", e.code().0),
        })?;
        if handle == INVALID_HANDLE_VALUE {
            return Err(YoloError::WindowsApi {
                detail: format!("invalid handle for {path:?}"),
            });
        }

        let result = DeviceIoControl(
            handle,
            FSCTL_EXTEND_VOLUME,
            Some(&grow as *const i64 as *const _),
            std::mem::size_of::<i64>() as u32,
            None,
            0,
            None,
            None,
        );
        let _ = CloseHandle(handle);

        match result {
            Ok(()) => {
                info!(bytes_to_add, "NTFS volume extended via FSCTL_EXTEND_VOLUME");
                Ok(FsExtendResult::Extended)
            }
            Err(e) if win32_code(e.code().0) == ERROR_INVALID_PARAMETER.0 => {
                Ok(FsExtendResult::RejectedInvalidParameter)
            }
            Err(e) => Err(YoloError::WindowsApi {
                detail: format!(
                    "FSCTL_EXTEND_VOLUME on {path:?} (+{bytes_to_add} bytes): {}",
                    e.code().0
                ),
            }),
        }
    }
}

fn system_drive_letter() -> String {
    std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".into())
        .trim_end_matches(':')
        .to_ascii_uppercase()
}
