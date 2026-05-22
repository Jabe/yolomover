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
    long_about = None
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
    /// Read-only: disk layout + WinRE status
    Inspect,
    /// Dry-run: show relocation plan and validation
    Plan,
    /// Execute: disable WinRE, relocate recovery, re-enable, optional extend
    Run {
        /// Skip interactive confirmation (still requires explicit flag)
        #[arg(long)]
        yes: bool,
        /// Extend the boot partition into space freed before recovery
        #[arg(long)]
        extend_c: bool,
        /// Plan only; do not write to disk or call reagentc disable/enable
        #[arg(long)]
        dry_run: bool,
    },
}
