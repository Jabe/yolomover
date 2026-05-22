//! Run diskpart scripts non-interactively.

use crate::error::{Result, YoloError};
use std::io::Write;
use std::process::{Command, Stdio};

pub fn run_diskpart(script: &str) -> Result<String> {
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
