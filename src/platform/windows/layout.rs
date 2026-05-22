use crate::error::{Result, YoloError};
use crate::gpt::{
    disk_sector_count, last_usable_lba_for_disk_sectors, GptHeader, GptPartitionEntry,
    PARTITION_ARRAY_SECTORS, PARTITION_COUNT, PARTITION_ENTRY_SIZE, SECTOR_SIZE,
};
use crate::platform::windows::disk::PhysicalDisk;
use crate::types::DiskLayout;
use tracing::debug;

const PRIMARY_GPT_LBA: u64 = 1;

pub fn read_disk_layout(disk: &mut PhysicalDisk) -> Result<DiskLayout> {
    let sector = disk.read_one_sector(PRIMARY_GPT_LBA)?;
    let header = GptHeader::parse(&sector)?;

    if header.partition_entry_size as usize != PARTITION_ENTRY_SIZE {
        return Err(YoloError::GptInvalid {
            detail: format!(
                "unexpected entry size {}",
                header.partition_entry_size
            ),
        });
    }

    let entry_lba = header.partition_entry_lba;
    let mut raw_entries = vec![0u8; (PARTITION_ARRAY_SECTORS * SECTOR_SIZE) as usize];
    disk.read_sectors(entry_lba, PARTITION_ARRAY_SECTORS, &mut raw_entries)?;

    let mut partitions = Vec::new();
    let mut recovery = Vec::new();
    let mut boot_candidates = Vec::new();

    for i in 0..PARTITION_COUNT.min(header.partition_count as usize) {
        let off = i * PARTITION_ENTRY_SIZE;
        let entry = GptPartitionEntry::parse(i as u32, &raw_entries[off..off + PARTITION_ENTRY_SIZE])?;
        if entry.is_unused() {
            continue;
        }
        if entry.is_recovery() {
            recovery.push(entry.clone());
        }
        if entry.type_guid.is_esp() {
            boot_candidates.push(entry.clone());
        }
        partitions.push(entry);
    }

    let recovery = pick_recovery(recovery, disk.index)?;
    let boot_partition = pick_boot_partition(&partitions, &boot_candidates);

    let is_gpt = is_gpt_disk(disk)?;
    if !is_gpt {
        return Err(YoloError::MbrDisk {
            disk_index: disk.index,
        });
    }

    debug!(
        disk = disk.index,
        parts = partitions.len(),
        recovery = recovery.as_ref().map(|p| p.index),
        "parsed layout"
    );

    let device_sectors = disk_sector_count(disk.size_bytes, disk.sector_size);
    let gpt_sectors = header.backup_lba.saturating_add(1);
    let stale_primary_gpt = device_sectors > gpt_sectors;
    let header_last_usable = last_usable_lba_for_disk_sectors(device_sectors);

    if stale_primary_gpt {
        debug!(
            disk = disk.index,
            gpt_sectors,
            device_sectors,
            primary_last_usable = header.last_usable_lba,
            effective_last_usable = header_last_usable,
            "primary GPT header smaller than device; using device size"
        );
    }

    Ok(DiskLayout {
        disk_index: disk.index,
        disk_path: disk.path.clone(),
        is_gpt: true,
        disk_size_bytes: disk.size_bytes,
        sector_size: disk.sector_size,
        header_first_usable: header.first_usable_lba,
        header_last_usable,
        stale_primary_gpt,
        partitions,
        recovery,
        boot_partition,
    })
}

fn is_gpt_disk(disk: &mut PhysicalDisk) -> Result<bool> {
    let mbr = disk.read_one_sector(0)?;
    // Protective MBR: boot signature 0xAA55 at offset 510, partition 1 type 0xEE
    let sig = u16::from_le_bytes([mbr[510], mbr[511]]);
    Ok(sig == 0xAA55 && mbr[450] == 0xEE)
}

fn pick_recovery(
    mut found: Vec<GptPartitionEntry>,
    disk_index: u32,
) -> Result<Option<GptPartitionEntry>> {
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found.remove(0))),
        _ => Err(YoloError::MultipleRecovery { disk_index }),
    }
}

/// Boot partition: largest Microsoft basic data before recovery, or following ESP.
fn pick_boot_partition(
    partitions: &[GptPartitionEntry],
    esp_list: &[GptPartitionEntry],
) -> Option<GptPartitionEntry> {
    use crate::gpt::{GptGuid, MS_BASIC_DATA_GUID};
    let basic = GptGuid::parse_str(MS_BASIC_DATA_GUID).ok()?;
    let recovery_idx = partitions.iter().find(|p| p.is_recovery()).map(|p| p.index);

    let mut best: Option<&GptPartitionEntry> = None;
    for p in partitions {
        if p.type_guid != basic {
            continue;
        }
        if recovery_idx == Some(p.index) {
            continue;
        }
        if let Some(r) = recovery_idx {
            if p.index == r {
                continue;
            }
        }
        if best.map(|b| p.byte_size() > b.byte_size()).unwrap_or(true) {
            best = Some(p);
        }
    }
    if best.is_some() {
        return best.cloned();
    }

    // Fallback: partition immediately after ESP
    if let Some(esp) = esp_list.first() {
        let esp_end = esp.last_lba;
        return partitions
            .iter()
            .find(|p| p.first_lba > esp_end)
            .cloned();
    }
    None
}
