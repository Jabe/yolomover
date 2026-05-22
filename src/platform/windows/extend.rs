//! Extend the boot volume into adjacent unallocated space using diskpart.

use crate::error::{Result, YoloError};
use crate::types::DiskLayout;
use std::io::Write;
use std::process::{Command, Stdio};
use tracing::info;

pub fn extend_boot_volume(layout: &DiskLayout) -> Result<()> {
    let boot = layout.boot_partition.as_ref().ok_or_else(|| {
        YoloError::other("could not identify boot partition to extend")
    })?;

    info!(partition = boot.index, "extending boot partition via diskpart");

    let script = format!(
        "select disk {}\nselect partition {}\nextend\nexit\n",
        layout.disk_index, boot.index
    );

    let mut child = Command::new("diskpart")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| YoloError::other(format!("diskpart spawn failed: {e}")))?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| YoloError::other("diskpart stdin unavailable"))?
        .write_all(script.as_bytes())
        .map_err(|e| YoloError::other(format!("diskpart write: {e}")))?;

    let output = child
        .wait_with_output()
        .map_err(|e| YoloError::other(format!("diskpart wait: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(YoloError::other(format!(
            "diskpart extend failed: {stderr}{stdout}"
        )));
    }

    info!("boot partition extend completed");
    Ok(())
}
