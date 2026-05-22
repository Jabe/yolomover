# yolomover

**Move the Windows Recovery partition to the end of the disk** so you can extend `C:` — without manually chaining `reagentc /disable`, a partition GUI, `reagentc /enable`, and `extend` every time.

> The name is intentional. Repartitioning the system disk is high-risk. yolomover checks aggressively, warns loudly, and defaults to dry-run / confirmation.

## Requirements

- **Windows 10/11**, **64-bit**
- **GPT** system disk (MBR is detected and rejected)
- **Administrator** elevation
- Recovery partition type `{DE94BBA4-06D1-4D40-A16A-BFD50179D6AC}` (Windows RE)
- `reagentc.exe` (ships with Windows)

Build on any host; **run only on Windows**.

## Quick start

```powershell
# Inspect layout + WinRE (safe, read-only)
yolomover inspect

# Show planned moves without writing
yolomover plan

# Full workflow (disable WinRE → relocate → enable → optional extend)
yolomover run --yes
yolomover run --yes --extend-c
```

## What it does

1. **Detect** physical system disk, GPT layout, recovery partition, and `reagentc /info` status.
2. **Validate** preconditions (GPT, recovery GUID, alignment, free space at end, not already at end).
3. **Disable WinRE** via `reagentc /disable`.
4. **Relocate** recovery partition data to the last aligned slot before backup GPT headers.
5. **Update GPT** primary + backup partition entry arrays.
6. **Re-enable WinRE** via `reagentc /enable` and verify `/info`.
7. **Optionally extend** the boot partition into freed space (`--extend-c`).

## Safety model

- Subcommands: `inspect` (read-only), `plan` (dry-run), `run` (mutating).
- `run` requires `--yes` plus typing `YES` at the confirmation prompt.
- Refuses MBR, missing recovery partition, overlapping targets, and non-512-byte sectors.
- Uses sector-granular copy with overlap checks; updates primary and backup GPT entry arrays.

### Known limitations (v0.2)

- Boot volume extend uses `diskpart` (delegated) while partition move is implemented in-tree.
- Buffered overlap copies are capped at 2 GiB (typical recovery partitions are smaller).
- Must be tested on real hardware/VMs before production use.

## Development

```bash
cargo build
cargo test
```

Cross-compile for Windows:

```bash
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

## License

MIT OR Apache-2.0
