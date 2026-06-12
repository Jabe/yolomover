use crate::error::{Result, YoloError};
use crate::gpt::{CopyStrategy, SECTOR_SIZE};
use crate::platform::windows::disk::PhysicalDisk;
use crate::platform::windows::gpt_disk::GptOnDisk;
use crate::platform::windows::volume::VolumeGuard;
use crate::types::RelocationPlan;
use tracing::{info, warn};

const COPY_CHUNK_SECTORS: u64 = 2048; // 1 MiB

/// Copy recovery partition sectors to new LBAs and commit GPT tables.
pub fn execute_relocation(disk: &mut PhysicalDisk, plan: &RelocationPlan) -> Result<()> {
    if !plan.needs_move() {
        info!("relocation skipped - already at target");
        return Ok(());
    }

    let _guard = VolumeGuard::lock_overlapping(
        plan.disk.disk_index,
        plan.current_first_lba,
        plan.current_last_lba,
    )?;

    info!(
        from = plan.current_first_lba,
        to = plan.target_first_lba,
        sectors = plan.sector_count,
        strategy = ?plan.copy_strategy,
        "copying recovery partition"
    );

    copy_partition(
        disk,
        plan.current_first_lba,
        plan.target_first_lba,
        plan.sector_count,
        plan.copy_strategy,
    )?;

    let mut gpt = GptOnDisk::load(disk)?;
    gpt.update_recovery_lbas(
        plan.recovery.index,
        plan.target_first_lba,
        plan.target_last_lba,
    )?;
    gpt.commit(disk)?;

    // Tell the storage stack the partition table changed; diskpart rescan later
    // in the workflow is only best-effort.
    if let Err(e) = disk.update_properties() {
        warn!(error = %e, "IOCTL_DISK_UPDATE_PROPERTIES failed; OS view of the partition table may be stale until rescan");
    }

    info!("recovery partition relocated");
    Ok(())
}

fn copy_partition(
    disk: &mut PhysicalDisk,
    src_first: u64,
    dst_first: u64,
    sector_count: u64,
    strategy: CopyStrategy,
) -> Result<()> {
    match strategy {
        CopyStrategy::Forward => {
            copy_forward(disk, src_first, dst_first, sector_count)?;
        }
        CopyStrategy::Reverse => {
            copy_reverse(disk, src_first, dst_first, sector_count)?;
        }
        CopyStrategy::Buffered => {
            copy_buffered(disk, src_first, dst_first, sector_count)?;
        }
    }
    Ok(())
}

fn copy_forward(
    disk: &mut PhysicalDisk,
    src_first: u64,
    dst_first: u64,
    sector_count: u64,
) -> Result<()> {
    let mut remaining = sector_count;
    let mut offset = 0u64;
    while remaining > 0 {
        let chunk = remaining.min(COPY_CHUNK_SECTORS);
        copy_one_chunk(disk, src_first + offset, dst_first + offset, chunk)?;
        offset += chunk;
        remaining -= chunk;
        log_progress(offset, sector_count);
    }
    Ok(())
}

fn copy_reverse(
    disk: &mut PhysicalDisk,
    src_first: u64,
    dst_first: u64,
    sector_count: u64,
) -> Result<()> {
    let mut offset = sector_count;
    while offset > 0 {
        let chunk = COPY_CHUNK_SECTORS.min(offset);
        offset -= chunk;
        copy_one_chunk(disk, src_first + offset, dst_first + offset, chunk)?;
        log_progress(offset + chunk, sector_count);
    }
    Ok(())
}

fn copy_buffered(
    disk: &mut PhysicalDisk,
    src_first: u64,
    dst_first: u64,
    sector_count: u64,
) -> Result<()> {
    let bytes = (sector_count * SECTOR_SIZE) as usize;
    let mut data = vec![0u8; bytes];
    disk.read_sectors(src_first, sector_count, &mut data)?;
    disk.write_sectors(dst_first, sector_count, &data)?;
    log_progress(sector_count, sector_count);
    Ok(())
}

fn copy_one_chunk(disk: &mut PhysicalDisk, src: u64, dst: u64, sectors: u64) -> Result<()> {
    let bytes = (sectors * SECTOR_SIZE) as usize;
    let mut buf = vec![0u8; bytes];
    disk.read_sectors(src, sectors, &mut buf)?;
    disk.write_sectors(dst, sectors, &buf)?;
    Ok(())
}

fn log_progress(done: u64, total: u64) {
    info!(copied = done, total, "relocation progress");
}

/// Validate plan again immediately before writing.
pub fn preflight(disk: &mut PhysicalDisk, plan: &RelocationPlan) -> Result<()> {
    let layout = crate::platform::windows::layout::read_disk_layout(disk)?;
    let recovery = layout
        .recovery
        .as_ref()
        .ok_or(YoloError::RecoveryNotFound {
            disk_index: layout.disk_index,
        })?;
    if recovery.index != plan.recovery.index {
        return Err(YoloError::other(
            "recovery partition index changed since plan",
        ));
    }
    if recovery.first_lba != plan.current_first_lba || recovery.last_lba != plan.current_last_lba {
        return Err(YoloError::other(
            "recovery partition LBAs changed since plan",
        ));
    }
    Ok(())
}
