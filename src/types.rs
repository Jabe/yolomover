use crate::gpt::{CopyStrategy, GptPartitionEntry};

/// Output of `reagentc /info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinReStatus {
    pub raw_output: String,
}

/// Marker files on the recovery partition (`\\.\GLOBALROOT\...\Recovery\WindowsRE`).
#[derive(Debug, Clone)]
pub struct WinRePartitionInspect {
    pub windows_path: String,
    pub winre_wim_bytes: Option<u64>,
    pub boot_sdi_bytes: Option<u64>,
}

impl WinRePartitionInspect {
    /// Smallest plausible `winre.wim` (real images are hundreds of MiB).
    pub const MIN_WINRE_WIM_BYTES: u64 = 1_048_576;

    pub fn image_present(&self) -> bool {
        self.winre_wim_bytes
            .is_some_and(|b| b >= Self::MIN_WINRE_WIM_BYTES)
    }
}

/// One physical disk with parsed GPT layout.
#[derive(Debug, Clone)]
pub struct DiskLayout {
    pub disk_index: u32,
    pub disk_path: String,
    pub is_gpt: bool,
    pub disk_size_bytes: u64,
    pub sector_size: u64,
    pub header_first_usable: u64,
    pub header_last_usable: u64,
    /// Primary GPT header describes a smaller disk than the device.
    pub stale_primary_gpt: bool,
    /// Partition list came from the backup GPT because the primary copy failed CRC.
    pub used_backup_gpt: bool,
    pub partitions: Vec<GptPartitionEntry>,
    pub recovery: Option<GptPartitionEntry>,
    pub boot_partition: Option<GptPartitionEntry>,
}

impl DiskLayout {
    pub fn recovery_index(&self) -> Option<u32> {
        self.recovery.as_ref().map(|p| p.index)
    }

    /// 1-based partition number for `\\.\harddiskN\partitionM` (disk offset order, not GPT slot index).
    pub fn windows_partition_number(&self, entry: &GptPartitionEntry) -> u32 {
        let mut used: Vec<_> = self.partitions.iter().filter(|p| !p.is_unused()).collect();
        used.sort_by_key(|p| p.first_lba);
        for (i, p) in used.iter().enumerate() {
            if p.index == entry.index {
                return (i + 1) as u32;
            }
        }
        entry.index.saturating_add(1)
    }
}

/// Planned relocation for the recovery partition.
#[derive(Debug, Clone)]
pub struct RelocationPlan {
    pub disk: DiskLayout,
    pub recovery: GptPartitionEntry,
    pub current_first_lba: u64,
    pub current_last_lba: u64,
    pub target_first_lba: u64,
    pub target_last_lba: u64,
    pub sector_count: u64,
    pub already_at_end: bool,
    pub copy_strategy: CopyStrategy,
}

impl RelocationPlan {
    pub fn needs_move(&self) -> bool {
        !self.already_at_end
            && (self.current_first_lba != self.target_first_lba
                || self.current_last_lba != self.target_last_lba)
    }
}

/// Result of a completed relocation.
#[derive(Debug, Clone)]
pub struct RelocateSummary {
    pub relocated: bool,
    pub winre_verified: bool,
}

/// Result of a completed boot volume extend (sizes from GPT).
#[derive(Debug, Clone)]
pub struct ExtendSummary {
    pub before_sectors: u64,
    pub after_sectors: u64,
    /// Contiguous unallocated sectors still after the boot partition.
    pub extendable_after_sectors: u64,
}

impl ExtendSummary {
    pub fn grown_sectors(&self) -> u64 {
        self.after_sectors.saturating_sub(self.before_sectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpt::{GptGuid, GptPartitionEntry, RECOVERY_TYPE_GUID};

    #[test]
    fn windows_partition_number_orders_by_lba_not_gpt_slot() {
        let recovery = GptPartitionEntry {
            index: 3,
            type_guid: GptGuid::parse_str(RECOVERY_TYPE_GUID).unwrap(),
            unique_guid: GptGuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap(),
            first_lba: 10_000,
            last_lba: 11_000,
            attributes: 0,
            name: "Recovery".into(),
        };
        let layout = DiskLayout {
            disk_index: 0,
            disk_path: "\\\\.\\PhysicalDrive0".into(),
            is_gpt: true,
            disk_size_bytes: 0,
            sector_size: 512,
            header_first_usable: 34,
            header_last_usable: 0,
            stale_primary_gpt: false,
            used_backup_gpt: false,
            partitions: vec![
                GptPartitionEntry {
                    index: 0,
                    type_guid: GptGuid::parse_str(crate::gpt::ESP_TYPE_GUID).unwrap(),
                    unique_guid: GptGuid::parse_str("11111111-1111-1111-1111-111111111111")
                        .unwrap(),
                    first_lba: 2048,
                    last_lba: 3000,
                    attributes: 0,
                    name: "EFI".into(),
                },
                GptPartitionEntry {
                    index: 1,
                    type_guid: GptGuid::parse_str("E3C9E316-0B5C-4DB8-817D-F92DF00215AE").unwrap(),
                    unique_guid: GptGuid::parse_str("22222222-2222-2222-2222-222222222222")
                        .unwrap(),
                    first_lba: 3001,
                    last_lba: 3100,
                    attributes: 0,
                    name: "MSR".into(),
                },
                GptPartitionEntry {
                    index: 2,
                    type_guid: GptGuid::parse_str(crate::gpt::MS_BASIC_DATA_GUID).unwrap(),
                    unique_guid: GptGuid::parse_str("44444444-4444-4444-4444-444444444444")
                        .unwrap(),
                    first_lba: 3101,
                    last_lba: 9000,
                    attributes: 0,
                    name: "OS".into(),
                },
                recovery.clone(),
            ],
            recovery: Some(recovery.clone()),
            boot_partition: None,
        };
        let r = layout.recovery.as_ref().unwrap();
        assert_eq!(layout.windows_partition_number(r), 4);
    }
}
