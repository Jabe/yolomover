use crate::error::{Result, YoloError};
use crate::platform::windows::disk::{is_elevated, system_disk_index, PhysicalDisk};
use crate::platform::windows::extend::{extend_boot_volume, extendable_sectors_after_boot};
use crate::platform::windows::layout::read_disk_layout;
use crate::platform::windows::reagentc::{self, disable_winre, register_winre_after_relocate};
use crate::platform::windows::winre_inspect::verify_winre_partition;
use crate::platform::windows::relocation::{execute_relocation, preflight};
use crate::types::{DiskLayout, ExtendSummary, RelocationPlan, RelocateSummary, WinReStatus};
use tracing::info;

pub fn inspect_system_disk(disk_index: Option<u32>) -> Result<(DiskLayout, WinReStatus)> {
    ensure_elevated()?;
    let index = match disk_index {
        Some(i) => i,
        None => system_disk_index()?,
    };
    let mut disk = PhysicalDisk::open_readonly(index)?;
    let layout = read_disk_layout(&mut disk)?;
    let winre = reagentc::query_winre()?;
    Ok((layout, winre))
}

pub fn query_winre() -> Result<WinReStatus> {
    ensure_elevated()?;
    crate::platform::windows::reagentc::query_winre()
}

pub fn confirm_relocate(plan: &RelocationPlan) -> Result<()> {
    eprintln!();
    eprintln!("About to:");
    eprintln!("  1. Disable WinRE (reagentc /disable)");
    eprintln!(
        "  2. Move recovery partition {} from LBA {}..{} to {}..{} ({:?})",
        plan.recovery.index,
        plan.current_first_lba,
        plan.current_last_lba,
        plan.target_first_lba,
        plan.target_last_lba,
        plan.copy_strategy
    );
    eprintln!("  3. Re-enable WinRE and verify winre.wim on recovery partition");
    eprintln!();
    eprintln!("After success, extend the boot volume in a second step:");
    eprintln!("  yolomover extend --yes");
    eprintln!();
    eprintln!("Type YES to continue:");
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| YoloError::other(e.to_string()))?;
    if line.trim() != "YES" {
        return Err(YoloError::Cancelled);
    }
    Ok(())
}

pub fn confirm_extend(layout: &DiskLayout) -> Result<()> {
    let letter = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let sectors = extendable_sectors_after_boot(layout);
    eprintln!();
    eprintln!("About to:");
    eprintln!("  Extend boot volume {letter} (GPT grow + NTFS extend)");
    eprintln!(
        "  Contiguous unallocated after boot (approx): {:.1} MiB",
        sectors as f64 * 512.0 / (1024.0 * 1024.0)
    );
    eprintln!();
    eprintln!("Type YES to continue:");
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| YoloError::other(e.to_string()))?;
    if line.trim() != "YES" {
        return Err(YoloError::Cancelled);
    }
    Ok(())
}

pub fn run_relocation(plan: &RelocationPlan, dry_run: bool) -> Result<()> {
    if dry_run {
        info!("dry run - skipping relocation");
        return Ok(());
    }
    let mut disk = PhysicalDisk::open(plan.disk.disk_index)?;
    preflight(&mut disk, plan)?;
    execute_relocation(&mut disk, plan)
}

pub fn extend_boot_partition(layout: &DiskLayout) -> Result<ExtendSummary> {
    extend_boot_volume(layout)
}

pub fn relocate_workflow(plan: &RelocationPlan, dry_run: bool) -> Result<RelocateSummary> {
    ensure_elevated()?;

    if dry_run {
        info!("dry run workflow");
        let win_part = plan.disk.windows_partition_number(&plan.recovery);
        let winre_verified =
            verify_winre_partition(plan.disk.disk_index, win_part).unwrap_or(false);
        return Ok(RelocateSummary {
            relocated: plan.needs_move(),
            winre_verified,
        });
    }

    if plan.needs_move() {
        disable_winre()?;
        run_relocation(plan, false)?;
        let win_part = plan.disk.windows_partition_number(&plan.recovery);
        register_winre_after_relocate(plan.disk.disk_index, win_part)?;
    } else {
        info!("recovery already at end - skipping relocation");
    }

    let win_part = plan.disk.windows_partition_number(&plan.recovery);
    let winre_verified = verify_winre_partition(plan.disk.disk_index, win_part)?;

    Ok(RelocateSummary {
        relocated: plan.needs_move(),
        winre_verified,
    })
}

fn ensure_elevated() -> Result<()> {
    if !is_elevated() {
        return Err(YoloError::NotElevated);
    }
    Ok(())
}
