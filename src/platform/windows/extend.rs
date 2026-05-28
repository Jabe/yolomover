//! Extend the system boot volume into adjacent unallocated space.

use crate::error::{Result, YoloError};
use crate::gpt::{GptPartitionEntry, SECTOR_SIZE};
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
use windows::Win32::System::Ioctl::{
    DISK_GEOMETRY, FSCTL_EXTEND_VOLUME, FSCTL_GET_NTFS_VOLUME_DATA,
    IOCTL_DISK_GET_PARTITION_INFO_EX, IOCTL_DISK_UPDATE_DRIVE_SIZE, NTFS_VOLUME_DATA_BUFFER,
    PARTITION_INFORMATION_EX,
};
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
        return extend_ntfs_only(layout, boot, before_sectors);
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
    extend_filesystem_to_partition(layout, boot, partition_bytes)?;

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
fn extend_ntfs_only(
    layout: &DiskLayout,
    boot: &GptPartitionEntry,
    before_sectors: u64,
) -> Result<ExtendSummary> {
    let partition_bytes_gpt = before_sectors
        .checked_mul(SECTOR_SIZE)
        .ok_or_else(|| YoloError::other("partition size overflow"))?;
    refresh_partition_and_volume(layout.disk_index, layout.windows_partition_number(boot))?;
    let ntfs_bytes = ntfs_volume_bytes(&system_volume_device_path())?;
    let target = target_partition_bytes(
        layout.disk_index,
        layout.windows_partition_number(boot),
        partition_bytes_gpt,
    )?;
    if filesystem_covers_partition(ntfs_bytes, target) {
        return Err(YoloError::other(
            "no contiguous unallocated space after the boot partition; run inspect to verify layout",
        ));
    }
    info!(
        ntfs_bytes,
        target, "partition already grown; extending NTFS only"
    );
    extend_filesystem_to_partition(layout, boot, partition_bytes_gpt)?;
    Ok(ExtendSummary {
        before_sectors,
        after_sectors: before_sectors,
        extendable_after_sectors: 0,
    })
}

/// Grow NTFS to fill the partition and verify Explorer-visible capacity.
fn extend_filesystem_to_partition(
    layout: &DiskLayout,
    boot: &GptPartitionEntry,
    partition_bytes_gpt: u64,
) -> Result<()> {
    let win_part = layout.windows_partition_number(boot);
    let vol_path = system_volume_device_path();
    let part_path = partition_device_path(layout.disk_index, win_part);

    refresh_partition_and_volume(layout.disk_index, win_part)?;

    let target = target_partition_bytes(layout.disk_index, win_part, partition_bytes_gpt)?;
    let before = ntfs_volume_bytes(&vol_path)?;
    if filesystem_covers_partition(before, target) {
        info!(
            ntfs_bytes = before,
            target, "NTFS volume already fills the partition"
        );
        return Ok(());
    }

    info!(
        ntfs_bytes = before,
        target,
        need_bytes = target.saturating_sub(before),
        "extending NTFS into grown partition"
    );

    let delta = target.saturating_sub(before);
    let delta_i64 = i64::try_from(delta).map_err(|_| {
        YoloError::other(format!("NTFS extend delta {delta} bytes exceeds i64::MAX"))
    })?;

    // 0 = extend NTFS to the current partition size (MSDN FSCTL_EXTEND_VOLUME).
    for (path, grow) in [
        (vol_path.as_str(), 0i64),
        (part_path.as_str(), 0i64),
        (vol_path.as_str(), delta_i64),
        (part_path.as_str(), delta_i64),
    ] {
        refresh_partition_and_volume(layout.disk_index, win_part)?;
        match fsctl_extend_volume(path, grow) {
            Ok(()) => {
                let after = ntfs_volume_bytes(&vol_path)?;
                if filesystem_covers_partition(after, target) {
                    info!(
                        path,
                        grow,
                        ntfs_before = before,
                        ntfs_after = after,
                        target,
                        "NTFS volume extended via FSCTL_EXTEND_VOLUME"
                    );
                    return Ok(());
                }
                warn!(
                    path,
                    grow,
                    ntfs_after = after,
                    target,
                    "FSCTL_EXTEND_VOLUME succeeded but NTFS size unchanged"
                );
            }
            Err(e) => warn!(path, grow, error = %e, "FSCTL_EXTEND_VOLUME failed"),
        }
    }

    warn!("FSCTL_EXTEND_VOLUME did not grow NTFS; trying diskpart extend filesystem");
    extend_filesystem_via_diskpart(layout.disk_index, win_part)?;
    refresh_partition_and_volume(layout.disk_index, win_part)?;

    let after = ntfs_volume_bytes(&vol_path)?;
    if filesystem_covers_partition(after, target) {
        info!(
            ntfs_before = before,
            ntfs_after = after,
            target,
            "NTFS volume extended via diskpart"
        );
        return Ok(());
    }

    Err(YoloError::other(format!(
        "partition is {target} bytes but NTFS stayed at {after} bytes (was {before}); try `diskpart` → select volume C → extend filesystem"
    )))
}

fn partition_device_path(disk_index: u32, partition_number: u32) -> String {
    format!(r"\\?\GLOBALROOT\device\harddisk{disk_index}\partition{partition_number}")
}

fn target_partition_bytes(
    disk_index: u32,
    partition_number: u32,
    partition_bytes_gpt: u64,
) -> Result<u64> {
    let driver = query_partition_length_bytes(disk_index, partition_number)?;
    Ok(partition_bytes_gpt.max(driver))
}

fn refresh_partition_and_volume(disk_index: u32, partition_number: u32) -> Result<()> {
    sync_partition_drive_size(disk_index, partition_number)?;
    volume_update_properties(&system_volume_device_path())
}

fn sync_partition_drive_size(disk_index: u32, partition_number: u32) -> Result<()> {
    let path = partition_device_path(disk_index, partition_number);
    let handle = open_device(&path)?;
    let mut geometry = DISK_GEOMETRY::default();
    unsafe {
        DeviceIoControl(
            HANDLE(handle.as_raw_handle()),
            IOCTL_DISK_UPDATE_DRIVE_SIZE,
            None,
            0,
            Some(&mut geometry as *mut _ as *mut _),
            std::mem::size_of::<DISK_GEOMETRY>() as u32,
            None,
            None,
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("IOCTL_DISK_UPDATE_DRIVE_SIZE on {path:?}: {}", e.code().0),
        })?;
    }
    Ok(())
}

fn query_partition_length_bytes(disk_index: u32, partition_number: u32) -> Result<u64> {
    let path = partition_device_path(disk_index, partition_number);
    let handle = open_device(&path)?;
    let mut info = PARTITION_INFORMATION_EX::default();
    unsafe {
        DeviceIoControl(
            HANDLE(handle.as_raw_handle()),
            IOCTL_DISK_GET_PARTITION_INFO_EX,
            None,
            0,
            Some(&mut info as *mut _ as *mut _),
            std::mem::size_of::<PARTITION_INFORMATION_EX>() as u32,
            None,
            None,
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!(
                "IOCTL_DISK_GET_PARTITION_INFO_EX on {path:?}: {}",
                e.code().0
            ),
        })?;
    }
    Ok(info.PartitionLength as u64)
}

fn filesystem_covers_partition(fs_bytes: u64, partition_bytes: u64) -> bool {
    let Ok(cluster) = ntfs_cluster_bytes() else {
        return fs_bytes + SECTOR_SIZE >= partition_bytes;
    };
    fs_bytes.saturating_add(cluster.saturating_sub(1)) >= partition_bytes
}

fn open_device(path: &str) -> Result<OwnedHandle> {
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
    let handle = open_device(path)?;
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
    let handle = open_device(path)?;
    let mut data = NTFS_VOLUME_DATA_BUFFER::default();
    unsafe {
        DeviceIoControl(
            HANDLE(handle.as_raw_handle()),
            FSCTL_GET_NTFS_VOLUME_DATA,
            None,
            0,
            Some(&mut data as *mut _ as *mut _),
            std::mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
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
    ntfs_volume_data(&system_volume_device_path()).map(|d| d.BytesPerCluster as u64)
}

fn ntfs_volume_data(path: &str) -> Result<NTFS_VOLUME_DATA_BUFFER> {
    let handle = open_device(path)?;
    let mut data = NTFS_VOLUME_DATA_BUFFER::default();
    unsafe {
        DeviceIoControl(
            HANDLE(handle.as_raw_handle()),
            FSCTL_GET_NTFS_VOLUME_DATA,
            None,
            0,
            Some(&mut data as *mut _ as *mut _),
            std::mem::size_of::<NTFS_VOLUME_DATA_BUFFER>() as u32,
            None,
            None,
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("FSCTL_GET_NTFS_VOLUME_DATA on {path:?}: {}", e.code().0),
        })?;
    }
    Ok(data)
}

fn fsctl_extend_volume(path: &str, grow_bytes: i64) -> Result<()> {
    let handle = open_device(path)?;
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

/// When the partition is already full, `extend` is a no-op — grow the filesystem explicitly.
fn extend_filesystem_via_diskpart(disk_index: u32, partition_number: u32) -> Result<()> {
    let letter = system_drive_letter();
    let scripts = [
        format!("select volume={letter}:\nextend filesystem\nexit\n"),
        format!(
            "select disk {disk_index}\nselect partition {partition_number}\nextend filesystem\nexit\n"
        ),
        format!("select volume={letter}:\nextend\nextend filesystem\nexit\n"),
    ];

    let mut last_err = None;
    for script in &scripts {
        match run_diskpart(script) {
            Ok(output) => {
                info!(output = output.trim(), "diskpart extend filesystem");
                return Ok(());
            }
            Err(e) => {
                warn!(error = %e, "diskpart extend filesystem attempt failed");
                last_err = Some(e);
            }
        }
    }
    Err(last_err
        .unwrap_or_else(|| YoloError::other("diskpart extend filesystem failed with no detail")))
}

fn system_drive_letter() -> String {
    std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".into())
        .trim_end_matches(':')
        .to_ascii_uppercase()
}
