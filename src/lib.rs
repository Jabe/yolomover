//! yolomover library — Windows recovery partition relocation.
//!
//! # Workflow
//!
//! 1. [`platform::inspect_system_disk`] — GPT layout + WinRE
//! 2. [`plan::build_relocation_plan`] — validate and compute target LBAs
//! 3. [`platform::relocate_workflow`] — disable WinRE, relocate, re-enable
//! 4. [`platform::extend_boot_partition`] — grow boot volume (separate command)

pub mod cli;
pub mod error;
pub mod gpt;
pub mod plan;
pub mod platform;
pub mod report;
pub mod safety;
pub mod types;

pub use error::{Result, YoloError};

use crate::plan::build_relocation_plan;
use crate::report::{
    print_banner, print_disk_layout, print_extend_plan, print_plan, print_winre,
};
use crate::types::RelocateSummary;
use cli::{Cli, Command};
use tracing::{error, info};

pub fn init_logging(level: cli::LogLevel) {
    tracing_subscriber::fmt()
        .with_env_filter(level.as_filter())
        .with_target(false)
        .init();
}

pub fn execute(cli: Cli) -> Result<()> {
    print_banner();

    match cli.command {
        Command::Inspect => cmd_inspect(cli.disk),
        Command::Plan => cmd_plan(cli.disk),
        Command::Relocate { yes, dry_run } => cmd_relocate(cli.disk, yes, dry_run),
        Command::Extend { yes } => cmd_extend(cli.disk, yes),
    }
}

fn cmd_inspect(disk: Option<u32>) -> Result<()> {
    let (layout, winre) = platform::inspect_system_disk(disk)?;
    safety::validate_layout(&layout)?;
    print_disk_layout(&layout);
    print_winre(&winre);
    print_extend_plan(&layout);
    Ok(())
}

fn cmd_plan(disk: Option<u32>) -> Result<()> {
    let (layout, winre) = platform::inspect_system_disk(disk)?;
    safety::validate_layout(&layout)?;
    print_disk_layout(&layout);
    print_winre(&winre);
    let plan = build_relocation_plan(&layout)?;
    print_plan(&plan);
    print_extend_plan(&layout);
    if plan.already_at_end {
        info!("nothing to do for relocation");
    }
    Ok(())
}

fn cmd_relocate(disk: Option<u32>, yes: bool, dry_run: bool) -> Result<()> {
    if !dry_run && !yes {
        error!("refusing to relocate without --yes");
        eprintln!();
        eprintln!("  Run `yolomover plan` first, then:");
        eprintln!("  yolomover relocate --yes");
        eprintln!();
        eprintln!("  After relocation succeeds, extend the boot volume:");
        eprintln!("  yolomover extend --yes");
        eprintln!();
        return Err(YoloError::Cancelled);
    }

    let (layout, winre) = platform::inspect_system_disk(disk)?;
    safety::validate_layout(&layout)?;
    print_disk_layout(&layout);
    print_winre(&winre);

    let plan = build_relocation_plan(&layout)?;
    print_plan(&plan);

    if plan.already_at_end {
        info!("recovery already at end; nothing to relocate");
        if !dry_run {
            eprintln!();
            eprintln!("  Boot volume can be extended separately:");
            eprintln!("  yolomover extend --yes");
            eprintln!();
        }
        return Ok(());
    }

    if !dry_run {
        platform::confirm_relocate(&plan)?;
    }

    let summary = platform::relocate_workflow(&plan, dry_run)?;
    print_relocate_summary(&summary, dry_run);

    if !dry_run && !summary.winre_verified {
        error!("WinRE verification failed after relocation");
        return Err(YoloError::WinRe {
            detail: "reagentc /info does not show enabled after /enable".into(),
        });
    }

    if !dry_run && summary.relocated {
        eprintln!();
        eprintln!("  Next step — extend the boot volume into freed space:");
        eprintln!("  yolomover extend --yes");
        eprintln!();
    }

    let _ = winre;
    Ok(())
}

fn cmd_extend(disk: Option<u32>, yes: bool) -> Result<()> {
    if !yes {
        error!("refusing to extend without --yes");
        eprintln!();
        eprintln!("  Run `yolomover inspect` first, then:");
        eprintln!("  yolomover extend --yes");
        eprintln!();
        return Err(YoloError::Cancelled);
    }

    let (layout, _winre) = platform::inspect_system_disk(disk)?;
    safety::validate_layout(&layout)?;
    print_disk_layout(&layout);
    print_extend_plan(&layout);

    let sectors = platform::extendable_sectors_after_boot(&layout);
    if sectors == 0 {
        return Err(YoloError::other(
            "no contiguous unallocated space after the boot partition",
        ));
    }

    platform::confirm_extend(&layout)?;
    platform::extend_boot_partition(&layout)?;

    eprintln!();
    println!("Extend complete. Reboot or refresh Disk Management to confirm size.");
    Ok(())
}

fn print_relocate_summary(summary: &RelocateSummary, dry_run: bool) {
    if dry_run {
        println!("Dry run complete - no changes written.");
        return;
    }
    println!("Relocate complete:");
    println!("  Relocated recovery: {}", summary.relocated);
    println!("  WinRE verified:     {}", summary.winre_verified);
}
