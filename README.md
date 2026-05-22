# yolomover

**Move the Windows Recovery partition to the end of the disk** so you can extend the boot volume — without manually chaining `reagentc /disable`, a partition GUI, `reagentc /enable`, and `extend` every time.

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

# Step 1: move recovery to disk tail (disable WinRE → relocate → re-enable)
yolomover relocate --yes

# Step 2: grow the boot volume into freed space (after relocate succeeds)
yolomover extend --yes
```

## What it does

1. **Detect** physical system disk, GPT layout, recovery partition, and `reagentc /info` status.
2. **Validate** preconditions (GPT, recovery GUID, alignment, free space at end).
3. **`relocate`** — disable WinRE, move recovery data to the tail, update GPT, re-enable WinRE.
4. **`extend`** — grow the system boot volume (`%SystemDrive%`) into contiguous unallocated space via diskpart.

Relocation and extend are **separate commands** on purpose: confirm recovery/WinRE before growing the OS volume.

## Safety model

- Subcommands: `inspect` (read-only), `plan` (dry-run), `relocate` (mutating), `extend` (mutating).
- `relocate` and `extend` require `--yes` plus typing `YES` at the confirmation prompt.
- Refuses MBR, missing recovery partition, overlapping targets, and non-512-byte sectors.
- Uses sector-granular copy with overlap checks; updates primary and backup GPT entry arrays.

### Known limitations (v0.2)

- Boot volume extend uses `diskpart` on `%SystemDrive%` (not GPT entry index).
- Buffered overlap copies are capped at 2 GiB (typical recovery partitions are smaller).
- Must be tested on real hardware/VMs before production use.

## Development

```bash
cargo build
cargo test
```

### CI binary (GitHub Actions)

Every push to `master` builds on `windows-2025` and uploads an artifact:

**`yolomover-windows-x86_64`** → `yolomover-x86_64-pc-windows-msvc.exe`

Download: GitHub repo → **Actions** → latest **Windows binary** run → **Artifacts**.

Cross-compile locally:

```bash
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

## License

MIT
