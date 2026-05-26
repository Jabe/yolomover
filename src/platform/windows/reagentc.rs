use crate::error::{Result, YoloError};
use crate::platform::windows::diskpart_cmd::run_diskpart;
use crate::platform::windows::winre_inspect::recovery_windowsre_dir;
use crate::types::WinReStatus;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{info, warn};

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
    Ok(WinReStatus { raw_output: raw })
}

pub fn disable_winre() -> Result<()> {
    info!("disabling WinRE (reagentc /disable)");
    run_reagentc(&["/disable"]).map(|_| ())
}

pub fn enable_winre() -> Result<()> {
    info!("enabling WinRE (reagentc /enable)");
    eprintln!("Enabling WinRE (reagentc /enable) — this may take a few minutes...");
    run_reagentc(&["/enable"]).map(|_| ())
}

fn system_recovery_store() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
        .join("System32")
        .join("Recovery")
}

/// Re-register WinRE after relocating the recovery partition.
pub fn register_winre_after_relocate(disk_index: u32, partition_number: u32) -> Result<()> {
    let _ = run_diskpart("rescan\nexit\n");

    if set_reimage_path(disk_index, partition_number).is_ok() && enable_winre().is_ok() {
        info!("WinRE registered on relocated recovery partition");
        return Ok(());
    }

    info!("copying WinRE image from System32\\Recovery and retrying setreimage");
    copy_winre_store_to_recovery(disk_index, partition_number)?;

    if set_reimage_path(disk_index, partition_number).is_ok() && enable_winre().is_ok() {
        info!("WinRE registered on relocated recovery partition");
        return Ok(());
    }

    warn!("setreimage on GLOBALROOT failed; trying temporary drive letter");
    register_winre_via_drive_letter(disk_index, partition_number)
}

fn copy_winre_store_to_recovery(disk_index: u32, partition_number: u32) -> Result<()> {
    let store = system_recovery_store();
    let dest_root = recovery_windowsre_dir(disk_index, partition_number);
    let dest = Path::new(&dest_root);

    std::fs::create_dir_all(dest).map_err(|e| YoloError::WinRe {
        detail: format!("create {dest_root}: {e}"),
    })?;

    for name in ["winre.wim", "boot.sdi"] {
        let src = store.join(name);
        if !src.is_file() {
            info!(file = name, "WinRE store file missing, skipping copy");
            continue;
        }
        let dst = dest.join(name);
        info!(from = %src.display(), to = %dst.display(), "copying WinRE file");
        std::fs::copy(&src, &dst).map_err(|e| YoloError::WinRe {
            detail: format!("copy {} -> {}: {e}", src.display(), dst.display()),
        })?;
    }
    Ok(())
}

fn set_reimage_path(disk_index: u32, partition_number: u32) -> Result<()> {
    let path = recovery_windowsre_dir(disk_index, partition_number);
    info!(%path, "reagentc /setreimage");
    run_reagentc(&["/setreimage", "/path", &path]).map(|_| ())
}

/// Assign `R:` to recovery, setreimage on `R:\Recovery\WindowsRE`, then remove the letter.
fn register_winre_via_drive_letter(disk_index: u32, partition_number: u32) -> Result<()> {
    const LETTER: &str = "R";
    let assign = format!(
        "select disk {disk_index}\nselect partition {partition_number}\nassign letter={LETTER}\nexit\n"
    );
    run_diskpart(&assign)?;

    let path = format!(r"{LETTER}:\Recovery\WindowsRE");
    info!(%path, "reagentc /setreimage (mounted recovery)");
    run_reagentc(&["/setreimage", "/path", &path])?;
    enable_winre()?;

    let remove = format!(
        "select disk {disk_index}\nselect partition {partition_number}\nremove letter={LETTER}\nexit\n"
    );
    let _ = run_diskpart(&remove);
    info!("WinRE registered via temporary drive letter");
    Ok(())
}
