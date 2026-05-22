use thiserror::Error;

/// All recoverable failures surfaced to the user.
#[derive(Debug, Error)]
pub enum YoloError {
    #[error("this command must be run on Windows with Administrator privileges")]
    NotWindows,

    #[error("administrator elevation is required")]
    NotElevated,

    #[error("operation cancelled by user")]
    Cancelled,

    #[error("disk {disk_index} is MBR; yolomover only supports GPT")]
    MbrDisk { disk_index: u32 },

    #[error("no Windows Recovery partition (type DE94BBA4-...) found on disk {disk_index}")]
    RecoveryNotFound { disk_index: u32 },

    #[error("multiple recovery partitions on disk {disk_index}; refusing to guess")]
    MultipleRecovery { disk_index: u32 },

    #[error("recovery partition is already at the end of the disk; nothing to do")]
    AlreadyAtEnd,

    #[error("planned relocation overlaps another partition or metadata (partition {partition})")]
    RelocationOverlap { partition: u32 },

    #[error(
        "recovery partition is {bytes} bytes; in-memory staging supports at most {max_bytes} bytes"
    )]
    PartitionTooLarge { bytes: u64, max_bytes: u64 },

    #[error("disk {disk_index} is too small for relocation (need {need_bytes} bytes at end)")]
    InsufficientSpace {
        disk_index: u32,
        need_bytes: u64,
    },

    #[error("GPT validation failed: {detail}")]
    GptInvalid { detail: String },

    #[error("WinRE / reagentc error: {detail}")]
    WinRe { detail: String },

    #[error("I/O error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Windows API error: {detail}")]
    WindowsApi { detail: String },

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, YoloError>;

impl YoloError {
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
