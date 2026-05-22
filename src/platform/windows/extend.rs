//! Extend the system boot volume into adjacent unallocated space using diskpart.

use crate::error::{Result, YoloError};
use crate::gpt::SECTOR_SIZE;
use crate::types::DiskLayout;
use std::io::Write;
use std::process::{Command, Stdio};
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

    let letter = system_drive_letter();
    info!(
        drive = %letter,
        gpt_index = boot.index,
        extendable_mib = extendable * SECTOR_SIZE / (1024 * 1024),
        "extending boot volume via diskpart"
    );

    let script = format!("select volume {letter}\nextend\nexit\n");
    let output = run_diskpart(&script)?;
    if !diskpart_extend_succeeded(&output) {
        return Err(YoloError::other(format!(
            "diskpart extend did not report success:\n{output}"
        )));
    }

    info!("boot volume extend completed");
    Ok(())
}

fn system_drive_letter() -> String {
    std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".into())
        .trim_end_matches(':')
        .to_ascii_uppercase()
}

fn run_diskpart(script: &str) -> Result<String> {
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
    let combined = format!("{stdout}{stderr}");
    if !output.status.success() {
        return Err(YoloError::other(format!("diskpart failed: {combined}")));
    }
    Ok(combined)
}

/// Parse diskpart output (English or German) for a successful volume extend.
fn diskpart_extend_succeeded(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    if lower.contains("successfully extended")
        || (lower.contains("erfolgreich") && (lower.contains("verl") || lower.contains("erweit")))
    {
        return true;
    }
    // No contiguous space / wrong selection often surfaces as an error string with exit 0.
    !(lower.contains("error")
        || lower.contains("fehler")
        || lower.contains("failed")
        || lower.contains("nicht gen")
        || lower.contains("not enough")
        || lower.contains("not supported")
        || lower.contains("nicht unterst"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_english_extend_success() {
        assert!(diskpart_extend_succeeded(
            "DiskPart successfully extended the volume by 6 GB."
        ));
    }

    #[test]
    fn detects_german_extend_success() {
        assert!(diskpart_extend_succeeded(
            "Datenträgerpartition wurde erfolgreich verlängert."
        ));
    }

    #[test]
    fn rejects_extend_error_text() {
        assert!(!diskpart_extend_succeeded(
            "There is not enough usable free space on disk(s)."
        ));
    }
}
