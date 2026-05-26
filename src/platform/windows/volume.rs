//! Lock volumes that overlap a partition range on a physical disk.

use crate::error::{Result, YoloError};
use crate::gpt::SECTOR_SIZE;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use tracing::{debug, warn};
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    DISK_EXTENT, FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, FSCTL_UNLOCK_VOLUME, VOLUME_DISK_EXTENTS,
};
use windows::Win32::System::IO::DeviceIoControl;

const VOLUME_NAME_CHARS: usize = 50;
const MAX_EXTENTS: usize = 32;

/// RAII guard that unlocks volumes on drop.
pub struct VolumeGuard {
    handles: Vec<OwnedHandle>,
}

impl VolumeGuard {
    /// Lock (and dismount) any mounted volumes overlapping `first_lba..=last_lba` on `disk_index`.
    pub fn lock_overlapping(disk_index: u32, first_lba: u64, last_lba: u64) -> Result<Self> {
        let mut handles = Vec::new();
        let mut name = [0u16; VOLUME_NAME_CHARS];

        let find = match unsafe { FindFirstVolumeW(&mut name) } {
            Ok(h) => h,
            Err(_) => {
                debug!("no volumes enumerated");
                return Ok(Self { handles });
            }
        };

        loop {
            if let Some(handle) = try_lock_volume(&name, disk_index, first_lba, last_lba)? {
                handles.push(handle);
            }
            if unsafe { FindNextVolumeW(find, &mut name) }.is_err() {
                break;
            }
        }
        unsafe {
            let _ = FindVolumeClose(find);
        }

        debug!(count = handles.len(), "volume lock pass complete");
        Ok(Self { handles })
    }
}

impl Drop for VolumeGuard {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            let raw = HANDLE(handle.as_raw_handle());
            let mut junk = 0u32;
            unsafe {
                let _ = DeviceIoControl(
                    raw,
                    FSCTL_UNLOCK_VOLUME,
                    None,
                    0,
                    None,
                    0,
                    Some(&mut junk),
                    None,
                );
            }
        }
    }
}

fn try_lock_volume(
    volume_name: &[u16],
    disk_index: u32,
    first_lba: u64,
    last_lba: u64,
) -> Result<Option<OwnedHandle>> {
    let handle = open_volume(volume_name)?;
    if !volume_overlaps(handle.as_raw_handle(), disk_index, first_lba, last_lba)? {
        return Ok(None);
    }
    lock_and_dismount(handle.as_raw_handle(), volume_name)?;
    Ok(Some(handle))
}

fn open_volume(volume_name: &[u16]) -> Result<OwnedHandle> {
    let wide: Vec<u16> = volume_name
        .iter()
        .copied()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let raw = CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS,
            HANDLE::default(),
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("CreateFileW volume: {e}"),
        })?;
        Ok(OwnedHandle::from_raw_handle(raw.0 as _))
    }
}

fn volume_overlaps(
    handle: std::os::windows::io::RawHandle,
    disk_index: u32,
    first_lba: u64,
    last_lba: u64,
) -> Result<bool> {
    let mut buf = [0u8; std::mem::size_of::<VOLUME_DISK_EXTENTS>()
        + MAX_EXTENTS * std::mem::size_of::<DISK_EXTENT>()];
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            HANDLE(handle),
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            None,
            0,
            Some(buf.as_mut_ptr() as *mut _),
            buf.len() as u32,
            Some(&mut returned),
            None,
        )
    };
    if ok.is_err() {
        return Ok(false);
    }
    let extents = unsafe { &*(buf.as_ptr() as *const VOLUME_DISK_EXTENTS) };
    for i in 0..extents.NumberOfDiskExtents as usize {
        let ext = extents.Extents[i];
        if ext.DiskNumber != disk_index {
            continue;
        }
        let start = sector_lba(ext.StartingOffset);
        let end = start.saturating_add(sector_lba(ext.ExtentLength).saturating_sub(1));
        if first_lba <= end && start <= last_lba {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sector_lba(byte_offset: i64) -> u64 {
    if byte_offset <= 0 {
        return 0;
    }
    (byte_offset as u64) / SECTOR_SIZE
}

fn lock_and_dismount(handle: std::os::windows::io::RawHandle, volume_name: &[u16]) -> Result<()> {
    let mut junk = 0u32;
    let lock = unsafe {
        DeviceIoControl(
            HANDLE(handle),
            FSCTL_LOCK_VOLUME,
            None,
            0,
            None,
            0,
            Some(&mut junk),
            None,
        )
    };
    if lock.is_err() {
        warn!(
            volume = %utf16_lossy(volume_name),
            "FSCTL_LOCK_VOLUME failed; continuing with physical disk exclusive access"
        );
        return Ok(());
    }
    let dismount = unsafe {
        DeviceIoControl(
            HANDLE(handle),
            FSCTL_DISMOUNT_VOLUME,
            None,
            0,
            None,
            0,
            Some(&mut junk),
            None,
        )
    };
    if dismount.is_err() {
        warn!(
            volume = %utf16_lossy(volume_name),
            "FSCTL_DISMOUNT_VOLUME failed"
        );
    }
    Ok(())
}

fn utf16_lossy(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}
