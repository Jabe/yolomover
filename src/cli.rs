use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_filter(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "yolomover=warn",
            Self::Info => "yolomover=info",
            Self::Debug => "yolomover=debug",
            Self::Trace => "yolomover=trace",
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "yolomover",
    version,
    about = "Move Windows Recovery partition to end of disk (dangerous)",
    long_about = "Move the Windows Recovery (WinRE) partition to the disk tail so the boot volume can grow.\n\n\
        Verification uses on-disk checks (winre.wim on the recovery partition, GPT boot extent after extend), \
        not parsing reagentc or diskpart text. `reagentc /info` is shown for humans only.\n\n\
        Typical flow: inspect → plan → relocate --yes → extend --yes"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Physical disk index (default: system disk)
    #[arg(long, global = true)]
    pub disk: Option<u32>,

    #[arg(long, global = true, value_enum, default_value_t)]
    pub log_level: LogLevel,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Read-only: GPT layout, recovery files (winre.wim), reagentc /info, extend plan
    Inspect,
    /// Dry-run: relocation plan and validation (no disk or reagentc changes)
    Plan,
    /// Move recovery to disk tail (disable WinRE, relocate, re-enable; enable may take minutes)
    Relocate {
        /// Skip interactive confirmation (still requires explicit flag)
        #[arg(long)]
        yes: bool,
        /// Plan only; do not write to disk or call reagentc disable/enable
        #[arg(long)]
        dry_run: bool,
    },
    /// Extend boot volume (%SystemDrive%); success verified by GPT growth
    Extend {
        /// Skip interactive confirmation (still requires explicit flag)
        #[arg(long)]
        yes: bool,
    },
}
