//! Cross-platform safety checks used during planning.

use crate::error::{Result, YoloError};
use crate::types::DiskLayout;

/// Refuse obviously unsafe layouts before any destructive step.
pub fn validate_layout(layout: &DiskLayout) -> Result<()> {
    if !layout.is_gpt {
        return Err(YoloError::MbrDisk {
            disk_index: layout.disk_index,
        });
    }
    if layout.recovery.is_none() {
        return Err(YoloError::RecoveryNotFound {
            disk_index: layout.disk_index,
        });
    }
    if layout.sector_size != crate::gpt::SECTOR_SIZE {
        return Err(YoloError::other(format!(
            "unsupported sector size {} (expected 512)",
            layout.sector_size
        )));
    }
    Ok(())
}
