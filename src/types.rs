use crate::gpt::{CopyStrategy, GptPartitionEntry};
use std::fmt;

/// WinRE status from `reagentc /info`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinReStatus {
    pub enabled: bool,
    pub raw_output: String,
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
    pub partitions: Vec<GptPartitionEntry>,
    pub recovery: Option<GptPartitionEntry>,
    pub boot_partition: Option<GptPartitionEntry>,
}

impl DiskLayout {
    pub fn recovery_index(&self) -> Option<u32> {
        self.recovery.as_ref().map(|p| p.index)
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

/// Result of a completed run.
#[derive(Debug, Clone)]
pub struct RunSummary {
    pub relocated: bool,
    pub winre_verified: bool,
    pub extended_c: bool,
}

impl fmt::Display for WinReStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WinRE {}",
            if self.enabled { "enabled" } else { "disabled" }
        )
    }
}
