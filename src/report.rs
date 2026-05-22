use crate::plan::build_relocation_plan;
use crate::types::{DiskLayout, RelocationPlan, WinReStatus};
use std::fmt;

pub fn print_banner() {
    eprintln!(
        r#"
╔══════════════════════════════════════════════════════════════════╗
║  yolomover - HIGH RISK disk partition operation                  ║
║  Moving the Windows Recovery partition can brick WinRE or boot.  ║
║  Ensure a full backup. Run inspect/plan before run.              ║
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
        println!("  Recovery:    partition {}", r.index);
    } else {
        println!("  Recovery:    NOT FOUND");
    }
    if let Some(b) = &layout.boot_partition {
        println!("  Boot (C:):   partition {}", b.index);
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
    println!("WinRE: {}", status);
    if !status.raw_output.trim().is_empty() {
        println!("--- reagentc /info ---");
        println!("{}", status.raw_output.trim());
        println!("----------------------");
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
        slack as f64 * 512.0 / (1024.0 * 1024.0)
    );
    if plan.already_at_end {
        println!("  Status:       nothing to do - recovery already at disk tail");
        if plan.current_first_lba != plan.target_first_lba {
            println!(
                "  Note:         skipped ~{} MiB alignment-only nudge (no space would be freed for C:)",
                (plan.target_first_lba - plan.current_first_lba) * 512 / (1024 * 1024)
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
                freed as f64 * 512.0 / (1024.0 * 1024.0)
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

pub fn format_inspection(layout: &DiskLayout, winre: &WinReStatus) -> String {
    let mut out = String::new();
    let _ = fmt::write(&mut out, format_args!("{}\n", layout.disk_index));
    print_disk_layout(layout);
    print_winre(winre);
    if let Ok(plan) = build_relocation_plan(layout) {
        print_plan(&plan);
    }
    out
}
