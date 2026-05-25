use crate::error::{Result, YoloError};
use crate::gpt::SECTOR_SIZE;
use crate::platform::windows::win32_code::win32_code;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::io::FromRawHandle;
use tracing::debug;

/// Handle to `\\.\PhysicalDriveN` with sector-aligned I/O.
pub struct PhysicalDisk {
    pub index: u32,
    pub path: String,
    file: File,
    pub size_bytes: u64,
    pub sector_size: u64,
}

impl PhysicalDisk {
    pub fn path_for_index(index: u32) -> String {
        format!(r"\\.\PhysicalDrive{index}")
    }

    pub fn open(index: u32) -> Result<Self> {
        let path = Self::path_for_index(index);
        let file = open_handle(&path, true)?;
        let size_bytes = probe_size(&file, &path)?;
        debug!(disk = index, size_bytes, "opened physical disk");

        Ok(Self {
            index,
            path,
            file,
            size_bytes,
            sector_size: SECTOR_SIZE,
        })
    }

    pub fn open_readonly(index: u32) -> Result<Self> {
        let path = Self::path_for_index(index);
        let file = open_handle(&path, false)?;
        let size_bytes = probe_size(&file, &path)?;
        Ok(Self {
            index,
            path,
            file,
            size_bytes,
            sector_size: SECTOR_SIZE,
        })
    }

    pub fn read_sectors(&mut self, start_lba: u64, count: u64, buf: &mut [u8]) -> Result<()> {
        let need = (count * self.sector_size) as usize;
        if buf.len() < need {
            return Err(YoloError::other(format!(
                "buffer too small: need {need}, have {}",
                buf.len()
            )));
        }
        self.file
            .seek(SeekFrom::Start(start_lba * self.sector_size))
            .map_err(|e| io_err(&self.path, e))?;
        self.file
            .read_exact(&mut buf[..need])
            .map_err(|e| io_err(&self.path, e))?;
        Ok(())
    }

    pub fn write_sectors(&mut self, start_lba: u64, count: u64, buf: &[u8]) -> Result<()> {
        let need = (count * self.sector_size) as usize;
        if buf.len() < need {
            return Err(YoloError::other("write buffer too small"));
        }
        self.file
            .seek(SeekFrom::Start(start_lba * self.sector_size))
            .map_err(|e| io_err(&self.path, e))?;
        self.file
            .write_all(&buf[..need])
            .map_err(|e| io_err(&self.path, e))?;
        self.file.sync_all().map_err(|e| io_err(&self.path, e))?;
        Ok(())
    }

    pub fn read_one_sector(&mut self, lba: u64) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; self.sector_size as usize];
        self.read_sectors(lba, 1, &mut buf)?;
        Ok(buf)
    }

    /// Notify the storage stack that the on-disk partition table changed.
    pub fn update_properties(&self) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Ioctl::IOCTL_DISK_UPDATE_PROPERTIES;
        use windows::Win32::System::IO::DeviceIoControl;

        let handle = HANDLE(self.file.as_raw_handle() as _);
        unsafe {
            DeviceIoControl(
                handle,
                IOCTL_DISK_UPDATE_PROPERTIES,
                None,
                0,
                None,
                0,
                None,
                None,
            )
            .map_err(|e| YoloError::WindowsApi {
                detail: format!(
                    "IOCTL_DISK_UPDATE_PROPERTIES on {}: {}",
                    self.path,
                    e.code().0
                ),
            })?;
        }
        Ok(())
    }

    /// Grow a partition through the storage driver (safe for live system volumes).
    pub fn grow_partition(&self, partition_number: u32, bytes_to_grow: u64) -> Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Ioctl::{DISK_GROW_PARTITION, IOCTL_DISK_GROW_PARTITION};
        use windows::Win32::System::IO::DeviceIoControl;

        let bytes: i64 = bytes_to_grow.try_into().map_err(|_| {
            YoloError::other(format!("grow size {bytes_to_grow} bytes exceeds i64::MAX"))
        })?;
        let input = DISK_GROW_PARTITION {
            PartitionNumber: partition_number,
            BytesToGrow: bytes,
        };

        let handle = HANDLE(self.file.as_raw_handle() as _);
        unsafe {
            DeviceIoControl(
                handle,
                IOCTL_DISK_GROW_PARTITION,
                Some(&input as *const _ as *const _),
                std::mem::size_of::<DISK_GROW_PARTITION>() as u32,
                None,
                0,
                None,
                None,
            )
            .map_err(|e| YoloError::WindowsApi {
                detail: format!(
                    "IOCTL_DISK_GROW_PARTITION partition {partition_number} (+{bytes_to_grow} bytes) on {}: {}",
                    self.path,
                    e.code().0
                ),
            })?;
        }
        Ok(())
    }
}

fn probe_size(file: &File, path: &str) -> Result<u64> {
    // Prefer IOCTL device length; the primary GPT header may lag behind an expanded disk.
    if let Ok(len) = probe_size_ioctl(file, path) {
        return Ok(len);
    }
    probe_size_from_primary_gpt(file, path)
}

fn probe_size_ioctl(file: &File, path: &str) -> Result<u64> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Ioctl::{GET_LENGTH_INFORMATION, IOCTL_DISK_GET_LENGTH_INFO};
    use windows::Win32::System::IO::DeviceIoControl;

    let mut info = GET_LENGTH_INFORMATION::default();
    let mut returned = 0u32;
    let handle = HANDLE(file.as_raw_handle() as _);
    unsafe {
        DeviceIoControl(
            handle,
            IOCTL_DISK_GET_LENGTH_INFO,
            None,
            0,
            Some(&mut info as *mut _ as *mut _),
            std::mem::size_of::<GET_LENGTH_INFORMATION>() as u32,
            Some(&mut returned),
            None,
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("IOCTL_DISK_GET_LENGTH_INFO on {path:?}: {}", e.code().0),
        })?;
    }
    if info.Length <= 0 {
        return Err(YoloError::WindowsApi {
            detail: format!("IOCTL_DISK_GET_LENGTH_INFO returned non-positive length on {path:?}"),
        });
    }
    Ok(info.Length as u64)
}

fn probe_size_from_primary_gpt(file: &File, path: &str) -> Result<u64> {
    let mut sector = vec![0u8; SECTOR_SIZE as usize];
    let mut f = file;
    f.seek(SeekFrom::Start(SECTOR_SIZE))
        .map_err(|e| io_err(path, e))?;
    f.read_exact(&mut sector)
        .map_err(|e| io_err(path, e))?;
    let header = crate::gpt::GptHeader::parse(&sector)?;
    let sectors = header.backup_lba + 1;
    Ok(sectors * SECTOR_SIZE)
}

fn open_handle(path: &str, write: bool) -> Result<File> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let access = if write {
        FILE_GENERIC_READ | FILE_GENERIC_WRITE
    } else {
        FILE_GENERIC_READ
    };
    // The boot disk always has open handles (volumes, system). Exclusive open fails with
    // ERROR_SHARING_VIOLATION (0x80070020). Volume locking is handled separately.
    // Do not use FILE_FLAG_NO_BUFFERING here: CreateFileW on \\.\PhysicalDriveN
    // often returns ERROR_INVALID_PARAMETER (0x57) with it on NVMe/QEMU; sector I/O
    // remains 512-byte aligned in read_sectors/write_sectors.
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE;
    let flags = FILE_ATTRIBUTE_NORMAL;

    unsafe {
        let handle = CreateFileW(
            PCWSTR(wide.as_ptr()),
            access.0,
            share,
            None,
            OPEN_EXISTING,
            flags,
            HANDLE::default(),
        )
        .map_err(|e| win32_create_err(path, e.code().0 as u32))?;
        Ok(File::from_raw_handle(handle.0 as _))
    }
}

fn io_err(path: &str, e: std::io::Error) -> YoloError {
    YoloError::Io {
        path: path.to_string(),
        source: e,
    }
}

fn win32_create_err(path: &str, code: u32) -> YoloError {
    let hint = sharing_violation_hint(code);
    YoloError::WindowsApi {
        detail: format!("CreateFileW({path:?}): {hint} (0x{code:08X})"),
    }
}

fn sharing_violation_hint(code: u32) -> &'static str {
    match win32_code(code as i32) {
        0x20 => "ERROR_SHARING_VIOLATION - disk or volume is in use; close other disk tools",
        0x05 => "ERROR_ACCESS_DENIED - run as Administrator",
        0x02 => "ERROR_FILE_NOT_FOUND - check device path",
        0x57 => "ERROR_INVALID_PARAMETER - check device path and open flags",
        _ => "CreateFileW failed",
    }
}

/// `\\.\X:` device path for the system volume (`SystemDrive`, e.g. `\\.\C:`).
///
/// The trailing colon is required by `CreateFileW`; `\\.\C` is invalid.
pub(crate) fn system_volume_device_path() -> String {
    match std::env::var("SystemDrive") {
        Ok(drive) => {
            let letter = drive.trim_end_matches(':');
            format!(r"\\.\{letter}:")
        }
        Err(_) => r"\\.\C:".to_string(),
    }
}

/// Resolve system boot disk index via `\\.\C:` device number IOCTL.
pub fn system_disk_index() -> Result<u32> {
    use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Ioctl::{
        IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    let path = system_volume_device_path();

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        // Access 0 + shared open: IOCTL-only per MSDN for STORAGE_DEVICE_NUMBER.
        let handle = CreateFileW(
            windows::core::PCWSTR(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
        .map_err(|e| win32_create_err(&path, e.code().0 as u32))?;
        if handle == INVALID_HANDLE_VALUE {
            return Err(YoloError::WindowsApi {
                detail: format!("invalid handle for {path:?}"),
            });
        }

        let mut num = STORAGE_DEVICE_NUMBER::default();
        let mut returned = 0u32;
        let ok = DeviceIoControl(
            handle,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            None,
            0,
            Some(&mut num as *mut _ as *mut _),
            std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
            Some(&mut returned),
            None,
        );
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        if ok.is_err() {
            return Err(YoloError::WindowsApi {
                detail: format!("IOCTL_STORAGE_GET_DEVICE_NUMBER on {path:?}"),
            });
        }
        Ok(num.DeviceNumber)
    }
}

pub fn is_elevated() -> bool {
    // Admin check: open physical drive 0 for write would also fail, but explicit is clearer.
    unsafe {
        use windows::Win32::UI::Shell::IsUserAnAdmin;
        IsUserAnAdmin().as_bool()
    }
}
