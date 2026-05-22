//! Inspect the recovery partition on disk (no reagentc/diskpart output parsing).

use crate::error::{Result, YoloError};
use crate::types::WinRePartitionInspect;
use std::path::Path;
use tracing::debug;

pub fn recovery_windowsre_dir(disk_index: u32, partition_number: u32) -> String {
    format!(
        r"\\?\GLOBALROOT\device\harddisk{disk_index}\partition{partition_number}\Recovery\WindowsRE"
    )
}

/// Read WinRE marker files on the recovery partition via the volume device path.
pub fn inspect_winre_partition(disk_index: u32, partition_number: u32) -> WinRePartitionInspect {
    let windows_path = recovery_windowsre_dir(disk_index, partition_number);
    let base = Path::new(&windows_path);
    let winre_wim_bytes = file_size_if_exists(&base.join("winre.wim"));
    let boot_sdi_bytes = file_size_if_exists(&base.join("boot.sdi"));
    debug!(
        path = %windows_path,
        ?winre_wim_bytes,
        ?boot_sdi_bytes,
        "recovery partition file inspection"
    );
    WinRePartitionInspect {
        windows_path,
        winre_wim_bytes,
        boot_sdi_bytes,
    }
}

pub fn verify_winre_partition(disk_index: u32, partition_number: u32) -> Result<bool> {
    let inspect = inspect_winre_partition(disk_index, partition_number);
    if inspect.image_present() {
        return Ok(true);
    }
    Err(YoloError::WinRe {
        detail: format!(
            "recovery partition missing winre.wim (>= {} bytes) at {}",
            WinRePartitionInspect::MIN_WINRE_WIM_BYTES,
            inspect.windows_path
        ),
    })
}

fn file_size_if_exists(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then_some(meta.len())
}
