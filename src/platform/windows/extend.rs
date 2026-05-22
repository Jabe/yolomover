//! Extend the system boot volume into adjacent unallocated space.

use crate::error::{Result, YoloError};
use crate::gpt::SECTOR_SIZE;
use crate::platform::windows::disk::{system_volume_device_path, PhysicalDisk};
use crate::platform::windows::layout::read_disk_layout;
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

    info!(
        drive = %letter,
        gpt_index = boot.index,
        windows_partition = win_part,
        before_sectors,
        extendable_mib = extendable * SECTOR_SIZE / (1024 * 1024),
        "extending boot volume via IOCTL_DISK_GROW_PARTITION + FSCTL_EXTEND_VOLUME"
    );

    // Use the storage driver to grow the live boot partition. Raw GPT edits on C: while
    // mounted can crash the system; recovery relocate targets a separate partition instead.
    let disk = PhysicalDisk::open(layout.disk_index)?;
    disk.grow_partition(win_part, extend_bytes)?;
    disk.update_properties()?;
    extend_ntfs_volume(extend_bytes)?;

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

/// Grow the mounted NTFS volume on `%SystemDrive%` into space added to its partition.
fn extend_ntfs_volume(bytes_to_add: u64) -> Result<()> {
    let path = system_volume_device_path();
    let grow: i64 = bytes_to_add.try_into().map_err(|_| {
        YoloError::other(format!("extend size {bytes_to_add} bytes exceeds i64::MAX"))
    })?;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
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
        result.map_err(|e| YoloError::WindowsApi {
            detail: format!(
                "FSCTL_EXTEND_VOLUME on {path:?} (+{bytes_to_add} bytes): {}",
                e.code().0
            ),
        })?;
    }

    info!(bytes_to_add, "NTFS volume extended");
    Ok(())
}

fn system_drive_letter() -> String {
    std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".into())
        .trim_end_matches(':')
        .to_ascii_uppercase()
}
