//! yolomover library — Windows recovery partition relocation.
//!
//! # Workflow
//!
//! 1. [`platform::inspect_system_disk`] — GPT layout + WinRE
//! 2. [`plan::build_relocation_plan`] — validate and compute target LBAs
//! 3. [`platform::run_workflow`] — disable WinRE, relocate, enable, optional extend

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
use crate::report::{print_banner, print_disk_layout, print_plan, print_winre};
use crate::types::RunSummary;
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
        Command::Run {
            yes,
            extend_c,
            dry_run,
        } => cmd_run(cli.disk, yes, extend_c, dry_run),
    }
}

fn cmd_inspect(disk: Option<u32>) -> Result<()> {
    let (layout, winre) = platform::inspect_system_disk(disk)?;
    safety::validate_layout(&layout)?;
    print_disk_layout(&layout);
    print_winre(&winre);
    Ok(())
}

fn cmd_plan(disk: Option<u32>) -> Result<()> {
    let (layout, winre) = platform::inspect_system_disk(disk)?;
    safety::validate_layout(&layout)?;
    print_disk_layout(&layout);
    print_winre(&winre);
    let plan = build_relocation_plan(&layout)?;
    print_plan(&plan);
    if plan.already_at_end {
        info!("nothing to do");
    }
    Ok(())
}

fn cmd_run(disk: Option<u32>, yes: bool, extend_c: bool, dry_run: bool) -> Result<()> {
    if !dry_run && !yes {
        error!("refusing to run destructive operation without --yes");
        eprintln!();
        eprintln!("  Run `yolomover plan` first, then:");
        eprintln!("  yolomover run --yes");
        eprintln!();
        return Err(YoloError::Cancelled);
    }

    let (layout, winre) = platform::inspect_system_disk(disk)?;
    safety::validate_layout(&layout)?;
    print_disk_layout(&layout);
    print_winre(&winre);

    let plan = build_relocation_plan(&layout)?;
    print_plan(&plan);

    if plan.already_at_end && !extend_c {
        info!("recovery already at end; exiting without changes");
        return Ok(());
    }

    if !dry_run {
        platform::confirm_run(&plan)?;
    }

    let summary = platform::run_workflow(&plan, dry_run, extend_c)?;
    print_summary(&summary, dry_run);

    if !dry_run && !summary.winre_verified {
        error!("WinRE verification failed after relocation");
        return Err(YoloError::WinRe {
            detail: "reagentc /info does not show enabled after /enable".into(),
        });
    }

    let _ = winre; // shown above; re-checked in workflow
    Ok(())
}

fn print_summary(summary: &RunSummary, dry_run: bool) {
    if dry_run {
        println!("Dry run complete - no changes written.");
        return;
    }
    println!("Run complete:");
    println!("  Relocated recovery: {}", summary.relocated);
    println!("  WinRE verified:     {}", summary.winre_verified);
    println!("  Extended C:         {}", summary.extended_c);
}
