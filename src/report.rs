use crate::gpt::SECTOR_SIZE;
use crate::types::{DiskLayout, ExtendSummary, RelocationPlan, WinRePartitionInspect, WinReStatus};

pub fn print_banner() {
    eprintln!(
        r#"
╔══════════════════════════════════════════════════════════════════╗
║  yolomover - HIGH RISK disk partition operation                  ║
║  Moving the Windows Recovery partition can brick WinRE or boot.  ║
║  Ensure a full backup. Run inspect/plan before relocate.         ║
╚══════════════════════════════════════════════════════════════════╝
"#
    );
}

pub fn print_disk_layout(layout: &DiskLayout) {
    println!("Disk {} ({})", layout.disk_index, layout.disk_path);
    println!(
        "  Style:       {}",
        if layout.is_gpt { "GPT" } else { "MBR" }
    );
    println!(
        "  Size:        {} bytes ({} GiB)",
        layout.disk_size_bytes,
        layout.disk_size_bytes / (1024 * 1024 * 1024)
    );
    println!(
        "  Usable LBA:  {} .. {}",
        layout.header_first_usable, layout.header_last_usable
    );
    if layout.stale_primary_gpt {
        println!("  Note:        primary GPT header is stale; using device size for planning");
    }
    println!("  Partitions:");
    for p in &layout.partitions {
        if p.is_unused() {
            continue;
        }
        let flags = partition_flags(p);
        println!(
            "    [{:>2}] {:>36}  LBA {:>12} .. {:>12}  ({:.1} MiB)  {}{}",
            p.index,
            p.type_guid,
            p.first_lba,
            p.last_lba,
            p.byte_size() as f64 / (1024.0 * 1024.0),
            p.name,
            flags
        );
    }
    if let Some(r) = &layout.recovery {
        println!(
            "  Recovery:    GPT slot {} (Windows partition {})",
            r.index,
            layout.windows_partition_number(r)
        );
    } else {
        println!("  Recovery:    NOT FOUND");
    }
    if let Some(b) = &layout.boot_partition {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        println!("  Boot volume: partition {} ({drive})", b.index);
    }
}

pub fn print_extend_summary(summary: &ExtendSummary) {
    let letter = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let grown = summary.grown_sectors();
    println!("Extend complete:");
    println!("  Boot volume:  {letter}");
    println!(
        "  GPT extent:   {} sectors ({:.1} MiB) -> {} sectors ({:.1} MiB)",
        summary.before_sectors,
        sectors_to_mib(summary.before_sectors),
        summary.after_sectors,
        sectors_to_mib(summary.after_sectors),
    );
    println!(
        "  Grown:        {} sectors ({:.1} MiB)",
        grown,
        sectors_to_mib(grown)
    );
    if summary.extendable_after_sectors == 0 {
        println!("  Unallocated:  none contiguous after boot volume");
    } else {
        println!(
            "  Unallocated:  {} sectors ({:.1} MiB) still after boot volume",
            summary.extendable_after_sectors,
            sectors_to_mib(summary.extendable_after_sectors)
        );
    }
}

fn sectors_to_mib(sectors: u64) -> f64 {
    sectors as f64 * SECTOR_SIZE as f64 / (1024.0 * 1024.0)
}

pub fn print_extend_plan(layout: &DiskLayout) {
    let letter = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let sectors = crate::platform::extendable_sectors_after_boot(layout);
    println!("Extend plan for disk {}", layout.disk_index);
    println!("  Boot volume: {letter}");
    if sectors == 0 {
        println!("  Status:      no contiguous unallocated space after boot volume");
        println!("  Note:        if Explorer is smaller than Disk Management, run extend anyway");
    } else {
        println!(
            "  Status:      CAN EXTEND by approx {} sectors ({:.1} MiB)",
            sectors,
            sectors as f64 * SECTOR_SIZE as f64 / (1024.0 * 1024.0)
        );
        println!("  Command:     yolomover extend --yes");
    }
}

fn partition_flags(p: &crate::gpt::GptPartitionEntry) -> String {
    let mut s = String::new();
    if p.is_recovery() {
        s.push_str(" [RECOVERY]");
    }
    if p.type_guid.is_esp() {
        s.push_str(" [ESP]");
    }
    s
}

pub fn print_winre(status: &WinReStatus) {
    if !status.raw_output.trim().is_empty() {
        println!("--- reagentc /info ---");
        println!("{}", status.raw_output.trim());
        println!("----------------------");
    }
}

pub fn print_winre_partition(layout: &DiskLayout) {
    let Some(recovery) = layout.recovery.as_ref() else {
        println!("Recovery partition files: (no recovery partition in GPT)");
        return;
    };
    let part = layout.windows_partition_number(recovery);
    let inspect = crate::platform::inspect_winre_partition(layout.disk_index, part);
    print_winre_partition_inspect(&inspect);
}

fn print_winre_partition_inspect(inspect: &WinRePartitionInspect) {
    println!("Recovery partition files:");
    println!("  Path:       {}", inspect.windows_path);
    match inspect.winre_wim_bytes {
        Some(b) => println!(
            "  winre.wim:  {} bytes ({})",
            b,
            if inspect.image_present() {
                "ok"
            } else {
                "too small"
            }
        ),
        None => println!("  winre.wim:  (missing)"),
    }
    match inspect.boot_sdi_bytes {
        Some(b) => println!("  boot.sdi:   {} bytes", b),
        None => println!("  boot.sdi:   (missing)"),
    }
}

pub fn print_plan(plan: &RelocationPlan) {
    println!("Relocation plan for disk {}", plan.disk.disk_index);
    println!(
        "  Recovery partition: {} ({} sectors)",
        plan.recovery.index, plan.sector_count
    );
    println!(
        "  Current LBA:  {} .. {}",
        plan.current_first_lba, plan.current_last_lba
    );
    println!(
        "  Target LBA:   {} .. {}",
        plan.target_first_lba, plan.target_last_lba
    );
    let slack = crate::plan::slack_after_recovery(&plan.disk, &plan.recovery);
    println!(
        "  Unallocated after recovery: {} sectors ({:.1} MiB)",
        slack,
        slack as f64 * SECTOR_SIZE as f64 / (1024.0 * 1024.0)
    );
    if plan.already_at_end {
        println!("  Status:       nothing to do - recovery already at disk tail");
        if plan.current_first_lba != plan.target_first_lba {
            println!(
                "  Note:         skipped ~{} MiB alignment-only nudge (no space would be freed for C:)",
                (plan.target_first_lba - plan.current_first_lba) * SECTOR_SIZE / (1024 * 1024)
            );
        }
    } else if plan.needs_move() {
        let freed = estimate_freed_sectors(plan);
        println!(
            "  Status:       WILL MOVE {} sectors ({:?})",
            plan.sector_count, plan.copy_strategy
        );
        if freed > 0 {
            println!(
                "  Extend C: into (approx): {} sectors ({:.1} MiB)",
                freed,
                freed as f64 * SECTOR_SIZE as f64 / (1024.0 * 1024.0)
            );
        }
    }
}

fn estimate_freed_sectors(plan: &RelocationPlan) -> u64 {
    let recovery = &plan.recovery;

    // [C:][Recovery][unallocated...] — extending C: uses the recovery extent once it moves.
    if plan
        .disk
        .boot_partition
        .as_ref()
        .is_some_and(|b| b.last_lba + 1 == recovery.first_lba)
    {
        return recovery.sector_count();
    }

    // [C:][gap][Recovery] — gap between prior partition and recovery.
    let mut prev_end = plan.disk.header_first_usable;
    for p in &plan.disk.partitions {
        if p.is_unused() || p.index == recovery.index {
            continue;
        }
        if p.last_lba < recovery.first_lba && p.last_lba >= prev_end {
            prev_end = p.last_lba + 1;
        }
    }
    recovery.first_lba.saturating_sub(prev_end)
}
