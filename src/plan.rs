use crate::error::{Result, YoloError};
use crate::gpt::{
    align_down, copy_strategy, end_aligned_start, LbaRange, CopyStrategy, ALIGN_SECTORS,
    MAX_BUFFERED_COPY_BYTES, SECTOR_SIZE,
};
use crate::types::{DiskLayout, RelocationPlan};

/// Build a relocation plan for the recovery partition on the system disk.
pub fn build_relocation_plan(layout: &DiskLayout) -> Result<RelocationPlan> {
    if !layout.is_gpt {
        return Err(YoloError::MbrDisk {
            disk_index: layout.disk_index,
        });
    }

    let recovery = layout.recovery.clone().ok_or(YoloError::RecoveryNotFound {
        disk_index: layout.disk_index,
    })?;

    let current_first_lba = recovery.first_lba;
    let current_last_lba = recovery.last_lba;
    let sector_count = recovery.sector_count();
    let target_first = end_aligned_start(
        layout.header_last_usable,
        sector_count,
        ALIGN_SECTORS,
    );
    let target_last = target_first + sector_count - 1;

    let already_at_end = recovery.first_lba == target_first && recovery.last_lba == target_last;

    let src = LbaRange::new(current_first_lba, current_last_lba);
    let dst = LbaRange::new(target_first, target_last);
    let strategy = copy_strategy(src, dst);

    if already_at_end {
        return Ok(RelocationPlan {
            disk: layout.clone(),
            recovery,
            current_first_lba,
            current_last_lba,
            target_first_lba: target_first,
            target_last_lba: target_last,
            sector_count,
            already_at_end: true,
            copy_strategy: strategy,
        });
    }

    validate_target(layout, &recovery, dst, strategy)?;

    Ok(RelocationPlan {
        disk: layout.clone(),
        recovery,
        current_first_lba,
        current_last_lba,
        target_first_lba: target_first,
        target_last_lba: target_last,
        sector_count,
        already_at_end: false,
        copy_strategy: strategy,
    })
}

fn validate_target(
    layout: &DiskLayout,
    recovery: &crate::gpt::GptPartitionEntry,
    target: LbaRange,
    strategy: CopyStrategy,
) -> Result<()> {
    for part in &layout.partitions {
        if part.is_unused() || part.index == recovery.index {
            continue;
        }
        if target.overlaps(part.lba_range()) {
            return Err(YoloError::RelocationOverlap {
                partition: part.index,
            });
        }
    }

    let byte_size = recovery.byte_size();
    if strategy == CopyStrategy::Buffered && byte_size > MAX_BUFFERED_COPY_BYTES {
        return Err(YoloError::PartitionTooLarge {
            bytes: byte_size,
            max_bytes: MAX_BUFFERED_COPY_BYTES,
        });
    }

    let need_bytes = target.sector_count() * SECTOR_SIZE;
    let disk_sectors = layout.disk_size_bytes / layout.sector_size.max(SECTOR_SIZE);
    if target.last >= disk_sectors {
        return Err(YoloError::InsufficientSpace {
            disk_index: layout.disk_index,
            need_bytes,
        });
    }

    let _ = align_down(target.first, ALIGN_SECTORS);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpt::{GptGuid, GptPartitionEntry, RECOVERY_TYPE_GUID};

    fn sample_layout(recovery_first: u64, recovery_sectors: u64, last_usable: u64) -> DiskLayout {
        let recovery_last = recovery_first + recovery_sectors - 1;
        let recovery = GptPartitionEntry {
            index: 4,
            type_guid: GptGuid::parse_str(RECOVERY_TYPE_GUID).unwrap(),
            unique_guid: GptGuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            first_lba: recovery_first,
            last_lba: recovery_last,
            attributes: 0,
            name: "Recovery".into(),
        };
        DiskLayout {
            disk_index: 0,
            disk_path: "\\\\.\\PhysicalDrive0".into(),
            is_gpt: true,
            disk_size_bytes: (last_usable + 64) * 512,
            sector_size: 512,
            header_first_usable: 34,
            header_last_usable: last_usable,
            partitions: vec![recovery.clone()],
            recovery: Some(recovery),
            boot_partition: None,
        }
    }

    #[test]
    fn move_to_end_uses_forward_copy() {
        let last = 2_000_000u64;
        let layout = sample_layout(1_500_000, 100_000, last);
        let plan = build_relocation_plan(&layout).unwrap();
        assert!(plan.needs_move());
        assert_eq!(plan.copy_strategy, CopyStrategy::Forward);
    }

    #[test]
    fn plan_targets_end() {
        let last = 1_000_000u64;
        let sectors = 500_000u64;
        let layout = sample_layout(100_000, sectors, last);
        let plan = build_relocation_plan(&layout).unwrap();
        assert!(plan.needs_move());
        assert!(plan.target_last_lba <= last);
        assert_eq!(plan.target_first_lba % crate::gpt::ALIGN_SECTORS, 0);
        assert_eq!(
            plan.target_last_lba - plan.target_first_lba + 1,
            sectors
        );
    }
}
