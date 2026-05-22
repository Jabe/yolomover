use crate::error::{Result, YoloError};
use crate::gpt::SECTOR_SIZE;
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
}

fn probe_size(file: &File, path: &str) -> Result<u64> {
    // Primary GPT header at LBA 1 contains backup_lba (= last sector of disk).
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
    let share = if write {
        // Exclusive access while relocating partitions.
        windows::Win32::Storage::FileSystem::FILE_SHARE_MODE(0)
    } else {
        FILE_SHARE_READ | FILE_SHARE_WRITE
    };

    unsafe {
        let handle = CreateFileW(
            PCWSTR(wide.as_ptr()),
            access.0,
            share,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
        .map_err(|e| YoloError::Io {
            path: path.to_string(),
            source: std::io::Error::from_raw_os_error(e.code().0),
        })?;
        Ok(File::from_raw_handle(handle.0 as _))
    }
}

fn io_err(path: &str, e: std::io::Error) -> YoloError {
    YoloError::Io {
        path: path.to_string(),
        source: e,
    }
}

/// Resolve system boot disk index via `\\.\C:` device number IOCTL.
pub fn system_disk_index() -> Result<u32> {
    use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_READ,
        OPEN_EXISTING,
    };
    use windows::Win32::System::Ioctl::{
        IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    let path = std::env::var("SystemDrive")
        .map(|d| format!(r"\\.\{}", d.trim_end_matches(':')))
        .unwrap_or_else(|_| r"\\.\C:".to_string());

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let handle = CreateFileW(
            windows::core::PCWSTR(wide.as_ptr()),
            FILE_GENERIC_READ.0,
            FILE_SHARE_READ,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
        .map_err(|e| YoloError::WindowsApi {
            detail: format!("CreateFileW({path:?}): {e}"),
        })?;
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
