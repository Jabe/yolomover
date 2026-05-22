use crate::error::{Result, YoloError};
use crate::types::WinReStatus;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

fn reagentc_path() -> PathBuf {
    let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(sysroot).join("System32").join("reagentc.exe")
}

fn run_reagentc(args: &[&str]) -> Result<String> {
    let exe = reagentc_path();
    if !exe.exists() {
        return Err(YoloError::WinRe {
            detail: format!("reagentc not found at {}", exe.display()),
        });
    }
    debug!(?args, "reagentc");
    let output = Command::new(&exe)
        .args(args)
        .output()
        .map_err(|e| YoloError::WinRe {
            detail: format!("failed to spawn reagentc: {e}"),
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let combined = format!("{stdout}{stderr}");
    if !output.status.success() {
        return Err(YoloError::WinRe {
            detail: format!(
                "reagentc {} failed (exit {:?}): {}",
                args.join(" "),
                output.status.code(),
                combined.trim()
            ),
        });
    }
    Ok(combined)
}

pub fn query_winre() -> Result<WinReStatus> {
    let raw = run_reagentc(&["/info"])?;
    let enabled = parse_enabled(&raw);
    Ok(WinReStatus { enabled, raw_output: raw })
}

fn parse_enabled(output: &str) -> bool {
    let lower = output.to_lowercase();
    // Typical: "Windows RE status: Enabled"
    if lower.contains("windows re status:") {
        return lower.contains("enabled") && !lower.contains("disabled");
    }
    lower.contains("enabled")
}

pub fn disable_winre() -> Result<()> {
    info!("disabling WinRE (reagentc /disable)");
    run_reagentc(&["/disable"]).map(|_| ())
}

pub fn enable_winre() -> Result<()> {
    info!("enabling WinRE (reagentc /enable)");
    run_reagentc(&["/enable"]).map(|_| ())
}

pub fn verify_winre_enabled() -> Result<bool> {
    let status = query_winre()?;
    Ok(status.enabled)
}

/// Point WinRE at recovery path after partition move.
pub fn set_reimage_path(disk_index: u32, partition_index: u32) -> Result<()> {
    let path = format!(
        r"\\?\GLOBALROOT\device\harddisk{disk_index}\partition{partition_index}\Recovery\WindowsRE"
    );
    info!(%path, "reagentc /setreimage");
    run_reagentc(&["/setreimage", "/path", &path]).map(|_| ())
}
