//! Load and commit primary + backup GPT tables (UEFI write order).

use super::{
    backup_header_lba_for_disk_sectors, disk_sector_count, efi_crc32, gpt_header_crc_valid,
    last_usable_lba_for_disk_sectors, GptHeader, GptPartitionEntry, PARTITION_COUNT,
    PARTITION_ENTRY_SIZE, SECTOR_SIZE,
};
use crate::error::{Result, YoloError};
use tracing::{info, warn};

const PRIMARY_HEADER_LBA: u64 = 1;
const DEFAULT_PRIMARY_ENTRY_LBA: u64 = 2;

/// Sector-granular disk used by [`GptTable::load`] / [`GptTable::commit`].
pub trait SectorIo {
    fn size_bytes(&self) -> u64;
    fn sector_size(&self) -> u64;
    fn read_sectors(&mut self, start_lba: u64, count: u64, buf: &mut [u8]) -> Result<()>;
    fn write_sectors(&mut self, start_lba: u64, count: u64, buf: &[u8]) -> Result<()>;
}

/// In-memory GPT (primary-oriented) ready to inspect or commit.
#[derive(Debug)]
pub struct GptTable {
    pub primary_header: GptHeader,
    pub entries: Vec<GptPartitionEntry>,
    pub entry_lba: u64,
    pub backup_header_lba: u64,
    pub backup_entry_lba: u64,
    /// Sectors occupied by the partition entry array (`partition_count` entries).
    array_sectors: u64,
    /// Device is larger than the GPT copy we loaded was written for.
    pub stale_primary_gpt: bool,
    /// Partition list came from the backup GPT (primary header or array CRC failed).
    pub used_backup: bool,
}

impl GptTable {
    pub fn load(disk: &mut impl SectorIo) -> Result<Self> {
        if disk.sector_size() != SECTOR_SIZE {
            return Err(YoloError::GptInvalid {
                detail: format!(
                    "unsupported sector size {} (expected {SECTOR_SIZE})",
                    disk.sector_size()
                ),
            });
        }
        let device_sectors = disk_sector_count(disk.size_bytes(), disk.sector_size());
        let primary = read_primary_header(disk)?;

        if let Some((header, true)) = primary.as_ref() {
            match try_load_array(disk, header)? {
                Some((header, raw, entry_lba)) => {
                    return from_loaded(header, &raw, entry_lba, device_sectors);
                }
                None => {
                    warn!("primary GPT partition array CRC invalid; falling back to backup GPT");
                }
            }
        } else if primary.is_some() {
            warn!("primary GPT header CRC invalid; falling back to backup GPT");
        } else {
            warn!("primary GPT header unreadable; falling back to backup GPT");
        }

        let primary_header = primary.as_ref().map(|(h, crc_ok)| (h, *crc_ok));
        let mut last_error = YoloError::GptInvalid {
            detail: "primary and backup GPT header CRCs are both invalid".into(),
        };
        for backup_lba in backup_header_candidates(primary.as_ref().map(|(h, _)| h), device_sectors)
        {
            match try_load_backup(disk, backup_lba) {
                Ok(Some((header, raw, _entry_lba))) => {
                    warn!(
                        backup_lba,
                        "using backup GPT copy; primary GPT should be repaired"
                    );
                    return from_backup(header, &raw, primary_header, device_sectors);
                }
                Ok(None) => {
                    last_error = YoloError::GptInvalid {
                        detail: "GPT partition array CRC invalid on both primary and backup copies"
                            .into(),
                    };
                }
                Err(e) => last_error = e,
            }
        }
        Err(last_error)
    }

    pub fn update_recovery_lbas(
        &mut self,
        recovery_index: u32,
        new_first: u64,
        new_last: u64,
    ) -> Result<()> {
        let entry = self
            .entries
            .iter_mut()
            .find(|e| e.index == recovery_index)
            .ok_or_else(|| YoloError::other(format!("partition {recovery_index} missing")))?;
        entry.first_lba = new_first;
        entry.last_lba = new_last;
        Ok(())
    }

    /// Write backup table first, then primary (crash leaves a valid primary copy).
    pub fn commit(&mut self, disk: &mut impl SectorIo) -> Result<()> {
        self.sync_device_geometry(disk);
        let entries_raw = self.serialize_entries()?;
        // The array CRC covers exactly partition_count entries, not sector padding.
        self.primary_header = self
            .primary_header
            .clone()
            .with_partition_array_crc(&entries_raw[..self.array_bytes()]);

        info!("writing backup GPT partition array");
        disk.write_sectors(self.backup_entry_lba, self.array_sectors, &entries_raw)?;

        let backup_header = self.backup_header();
        let mut backup_sector = vec![0u8; SECTOR_SIZE as usize];
        backup_header.write_to_sector(&mut backup_sector)?;
        info!("writing backup GPT header");
        disk.write_sectors(self.backup_header_lba, 1, &backup_sector)?;

        info!("writing primary GPT partition array");
        disk.write_sectors(self.entry_lba, self.array_sectors, &entries_raw)?;

        let mut primary_sector = vec![0u8; SECTOR_SIZE as usize];
        self.primary_header.write_to_sector(&mut primary_sector)?;
        info!("writing primary GPT header");
        disk.write_sectors(PRIMARY_HEADER_LBA, 1, &primary_sector)?;

        self.verify_crc(disk, &entries_raw)?;
        self.used_backup = false;
        self.stale_primary_gpt = false;
        info!("GPT tables committed with valid header CRCs");
        Ok(())
    }

    fn sync_device_geometry(&mut self, disk: &impl SectorIo) {
        let disk_sectors = disk_sector_count(disk.size_bytes(), disk.sector_size());
        self.backup_header_lba = backup_header_lba_for_disk_sectors(disk_sectors);
        self.backup_entry_lba = self.backup_header_lba.saturating_sub(self.array_sectors);
        self.primary_header.backup_lba = self.backup_header_lba;
        self.primary_header.last_usable_lba = last_usable_lba_for_disk_sectors(disk_sectors);
        self.primary_header.current_lba = PRIMARY_HEADER_LBA;
        self.primary_header.partition_entry_lba = self.entry_lba;
    }

    fn array_bytes(&self) -> usize {
        self.primary_header.partition_count as usize * PARTITION_ENTRY_SIZE
    }

    fn backup_header(&self) -> GptHeader {
        let mut h = self.primary_header.clone();
        h.current_lba = self.backup_header_lba;
        h.backup_lba = PRIMARY_HEADER_LBA;
        h.partition_entry_lba = self.backup_entry_lba;
        h
    }

    fn serialize_entries(&self) -> Result<Vec<u8>> {
        let mut raw = vec![0u8; (self.array_sectors * SECTOR_SIZE) as usize];
        for entry in &self.entries {
            if entry.index as usize >= self.primary_header.partition_count as usize {
                return Err(YoloError::GptInvalid {
                    detail: format!("partition index {} out of range", entry.index),
                });
            }
            let off = entry.index as usize * PARTITION_ENTRY_SIZE;
            entry.write_raw(&mut raw[off..off + PARTITION_ENTRY_SIZE]);
        }
        Ok(raw)
    }

    fn verify_crc(&self, disk: &mut impl SectorIo, entries_raw: &[u8]) -> Result<()> {
        let array_crc = efi_crc32(&entries_raw[..self.array_bytes()]);
        if array_crc != self.primary_header.partition_array_crc32 {
            return Err(YoloError::GptInvalid {
                detail: "partition array CRC mismatch after write".into(),
            });
        }
        let hdr_size = self.primary_header.header_size as usize;
        let primary_sector = read_one(disk, PRIMARY_HEADER_LBA)?;
        if !gpt_header_crc_valid(&primary_sector, hdr_size) {
            return Err(YoloError::GptInvalid {
                detail: "primary header CRC mismatch after write".into(),
            });
        }
        let backup_sector = read_one(disk, self.backup_header_lba)?;
        if !gpt_header_crc_valid(&backup_sector, hdr_size) {
            return Err(YoloError::GptInvalid {
                detail: "backup header CRC mismatch after write".into(),
            });
        }
        Ok(())
    }
}

fn read_one(disk: &mut impl SectorIo, lba: u64) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; disk.sector_size() as usize];
    disk.read_sectors(lba, 1, &mut buf)?;
    Ok(buf)
}

fn read_primary_header(disk: &mut impl SectorIo) -> Result<Option<(GptHeader, bool)>> {
    let sector = read_one(disk, PRIMARY_HEADER_LBA)?;
    match GptHeader::parse(&sector) {
        Ok(header) => {
            let crc_ok = gpt_header_crc_valid(&sector, header.header_size as usize);
            Ok(Some((header, crc_ok)))
        }
        Err(_) => Ok(None),
    }
}

fn try_load_array(
    disk: &mut impl SectorIo,
    header: &GptHeader,
) -> Result<Option<(GptHeader, Vec<u8>, u64)>> {
    let (array_bytes, array_sectors) = partition_array_dims(header)?;
    let entry_lba = header.partition_entry_lba;
    let mut raw = vec![0u8; (array_sectors * SECTOR_SIZE) as usize];
    disk.read_sectors(entry_lba, array_sectors, &mut raw)?;
    if efi_crc32(&raw[..array_bytes]) != header.partition_array_crc32 {
        return Ok(None);
    }
    Ok(Some((header.clone(), raw, entry_lba)))
}

fn try_load_backup(
    disk: &mut impl SectorIo,
    backup_lba: u64,
) -> Result<Option<(GptHeader, Vec<u8>, u64)>> {
    let sector = match read_one(disk, backup_lba) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let header = match GptHeader::parse(&sector) {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };
    if !gpt_header_crc_valid(&sector, header.header_size as usize) {
        return Ok(None);
    }
    try_load_array(disk, &header)
}

fn backup_header_candidates(primary: Option<&GptHeader>, device_sectors: u64) -> Vec<u64> {
    let at_end = backup_header_lba_for_disk_sectors(device_sectors);
    let mut out = Vec::new();
    let mut push = |lba: u64| {
        if lba >= 33 && lba < device_sectors && !out.contains(&lba) {
            out.push(lba);
        }
    };
    push(at_end);
    if let Some(h) = primary {
        push(h.backup_lba);
        if h.current_lba > h.backup_lba {
            push(h.current_lba);
        }
    }
    out
}

fn partition_array_dims(header: &GptHeader) -> Result<(usize, u64)> {
    if header.partition_entry_size as usize != PARTITION_ENTRY_SIZE {
        return Err(YoloError::GptInvalid {
            detail: format!(
                "unexpected partition entry size {}",
                header.partition_entry_size
            ),
        });
    }
    let count = header.partition_count as usize;
    if count == 0 || count > PARTITION_COUNT {
        return Err(YoloError::GptInvalid {
            detail: format!("unsupported partition count {count}"),
        });
    }
    let array_bytes = count * PARTITION_ENTRY_SIZE;
    let array_sectors = (array_bytes as u64).div_ceil(SECTOR_SIZE);
    Ok((array_bytes, array_sectors))
}

/// True when the GPT copy was written for a smaller disk than `device_sectors`.
pub(crate) fn is_stale_gpt(header: &GptHeader, device_sectors: u64) -> bool {
    let gpt_sectors = header.current_lba.max(header.backup_lba).saturating_add(1);
    device_sectors > gpt_sectors
}

fn primary_array_lba(primary: Option<(&GptHeader, bool)>) -> u64 {
    match primary {
        Some((h, true)) if h.partition_entry_lba != 0 => h.partition_entry_lba,
        _ => DEFAULT_PRIMARY_ENTRY_LBA,
    }
}

fn parse_entries(raw: &[u8], count: usize) -> Result<Vec<GptPartitionEntry>> {
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let off = i * PARTITION_ENTRY_SIZE;
        entries.push(GptPartitionEntry::parse(
            i as u32,
            &raw[off..off + PARTITION_ENTRY_SIZE],
        )?);
    }
    Ok(entries)
}

fn from_loaded(
    header: GptHeader,
    raw: &[u8],
    entry_lba: u64,
    device_sectors: u64,
) -> Result<GptTable> {
    let stale_primary_gpt = is_stale_gpt(&header, device_sectors);
    let count = header.partition_count as usize;
    let (_, array_sectors) = partition_array_dims(&header)?;
    let backup_header_lba = backup_header_lba_for_disk_sectors(device_sectors);
    Ok(GptTable {
        entries: parse_entries(raw, count)?,
        primary_header: header,
        entry_lba,
        backup_header_lba,
        backup_entry_lba: backup_header_lba.saturating_sub(array_sectors),
        array_sectors,
        stale_primary_gpt,
        used_backup: false,
    })
}

fn from_backup(
    backup: GptHeader,
    raw: &[u8],
    primary: Option<(&GptHeader, bool)>,
    device_sectors: u64,
) -> Result<GptTable> {
    let stale_primary_gpt = is_stale_gpt(&backup, device_sectors);
    let count = backup.partition_count as usize;
    let (_, array_sectors) = partition_array_dims(&backup)?;
    let entry_lba = primary_array_lba(primary);
    let backup_header_lba = backup_header_lba_for_disk_sectors(device_sectors);
    let mut primary_header = backup;
    primary_header.current_lba = PRIMARY_HEADER_LBA;
    primary_header.backup_lba = backup_header_lba;
    primary_header.partition_entry_lba = entry_lba;
    primary_header.last_usable_lba = last_usable_lba_for_disk_sectors(device_sectors);
    Ok(GptTable {
        entries: parse_entries(raw, count)?,
        primary_header,
        entry_lba,
        backup_header_lba,
        backup_entry_lba: backup_header_lba.saturating_sub(array_sectors),
        array_sectors,
        stale_primary_gpt,
        used_backup: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpt::{GptGuid, RECOVERY_TYPE_GUID};

    struct MemDisk {
        sector_size: u64,
        data: Vec<u8>,
        writes: Vec<(u64, u64)>,
    }

    impl MemDisk {
        fn new(sectors: u64) -> Self {
            Self {
                sector_size: SECTOR_SIZE,
                data: vec![0u8; (sectors * SECTOR_SIZE) as usize],
                writes: Vec::new(),
            }
        }

        fn sectors(&self) -> u64 {
            self.data.len() as u64 / self.sector_size
        }
    }

    impl SectorIo for MemDisk {
        fn size_bytes(&self) -> u64 {
            self.data.len() as u64
        }
        fn sector_size(&self) -> u64 {
            self.sector_size
        }
        fn read_sectors(&mut self, start_lba: u64, count: u64, buf: &mut [u8]) -> Result<()> {
            let start = (start_lba * self.sector_size) as usize;
            let need = (count * self.sector_size) as usize;
            let end = start
                .checked_add(need)
                .ok_or_else(|| YoloError::other(format!("read LBA {start_lba} out of range")))?;
            if end > self.data.len() {
                return Err(YoloError::other(format!(
                    "read LBA {start_lba}+{count} past end of disk"
                )));
            }
            buf[..need].copy_from_slice(&self.data[start..end]);
            Ok(())
        }
        fn write_sectors(&mut self, start_lba: u64, count: u64, buf: &[u8]) -> Result<()> {
            let start = (start_lba * self.sector_size) as usize;
            let need = (count * self.sector_size) as usize;
            let end = start
                .checked_add(need)
                .ok_or_else(|| YoloError::other(format!("write LBA {start_lba} out of range")))?;
            if end > self.data.len() {
                return Err(YoloError::other(format!(
                    "write LBA {start_lba}+{count} past end of disk"
                )));
            }
            self.data[start..end].copy_from_slice(&buf[..need]);
            self.writes.push((start_lba, count));
            Ok(())
        }
    }

    fn recovery_entry(first: u64, last: u64) -> GptPartitionEntry {
        GptPartitionEntry {
            index: 3,
            type_guid: GptGuid::parse_str(RECOVERY_TYPE_GUID).unwrap(),
            unique_guid: GptGuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap(),
            first_lba: first,
            last_lba: last,
            attributes: 0,
            name: "Recovery".into(),
        }
    }

    fn test_header(
        current: u64,
        backup: u64,
        entry_lba: u64,
        last_usable: u64,
        array_crc: u32,
    ) -> GptHeader {
        GptHeader {
            signature: GptHeader::SIGNATURE,
            revision: 0x0001_0000,
            header_size: 92,
            crc32: 0,
            reserved: 0,
            current_lba: current,
            backup_lba: backup,
            first_usable_lba: 34,
            last_usable_lba: last_usable,
            disk_guid: GptGuid::from_uuid(uuid::Uuid::nil()),
            partition_entry_lba: entry_lba,
            partition_count: PARTITION_COUNT as u32,
            partition_entry_size: PARTITION_ENTRY_SIZE as u32,
            partition_array_crc32: array_crc,
        }
    }

    fn install_gpt(disk: &mut MemDisk, entries: &[GptPartitionEntry]) {
        let disk_sectors = disk.sectors();
        let backup_header_lba = backup_header_lba_for_disk_sectors(disk_sectors);
        let array_bytes = PARTITION_COUNT * PARTITION_ENTRY_SIZE;
        let array_sectors = (array_bytes as u64).div_ceil(SECTOR_SIZE);
        let backup_entry_lba = backup_header_lba - array_sectors;
        let last_usable = last_usable_lba_for_disk_sectors(disk_sectors);

        let mut raw = vec![0u8; (array_sectors * SECTOR_SIZE) as usize];
        for e in entries {
            let off = e.index as usize * PARTITION_ENTRY_SIZE;
            e.write_raw(&mut raw[off..off + PARTITION_ENTRY_SIZE]);
        }
        let crc = efi_crc32(&raw[..array_bytes]);

        let primary = test_header(1, backup_header_lba, 2, last_usable, crc);
        let backup = test_header(backup_header_lba, 1, backup_entry_lba, last_usable, crc);

        let mut psec = vec![0u8; 512];
        primary.write_to_sector(&mut psec).unwrap();
        disk.write_sectors(1, 1, &psec).unwrap();
        disk.write_sectors(2, array_sectors, &raw).unwrap();

        let mut bsec = vec![0u8; 512];
        backup.write_to_sector(&mut bsec).unwrap();
        disk.write_sectors(backup_entry_lba, array_sectors, &raw)
            .unwrap();
        disk.write_sectors(backup_header_lba, 1, &bsec).unwrap();
        disk.writes.clear();
    }

    fn corrupt_sector(disk: &mut MemDisk, lba: u64, offset: usize) {
        let mut buf = vec![0u8; SECTOR_SIZE as usize];
        disk.read_sectors(lba, 1, &mut buf).unwrap();
        buf[offset] ^= 0xff;
        disk.write_sectors(lba, 1, &buf).unwrap();
        disk.writes.clear();
    }

    #[test]
    fn load_prefers_valid_primary() {
        let mut disk = MemDisk::new(2_048);
        install_gpt(&mut disk, &[recovery_entry(100, 200)]);
        let gpt = GptTable::load(&mut disk).unwrap();
        assert!(!gpt.used_backup);
        assert!(!gpt.stale_primary_gpt);
        assert_eq!(gpt.entry_lba, 2);
        let rec = gpt.entries.iter().find(|e| e.is_recovery()).unwrap();
        assert_eq!(rec.first_lba, 100);
        assert_eq!(rec.last_lba, 200);
    }

    #[test]
    fn load_falls_back_when_primary_header_crc_bad() {
        let mut disk = MemDisk::new(2_048);
        install_gpt(&mut disk, &[recovery_entry(100, 200)]);
        corrupt_sector(&mut disk, 1, 24);
        let gpt = GptTable::load(&mut disk).unwrap();
        assert!(gpt.used_backup);
        assert_eq!(gpt.entry_lba, 2);
        let rec = gpt.entries.iter().find(|e| e.is_recovery()).unwrap();
        assert_eq!(rec.first_lba, 100);
    }

    #[test]
    fn load_falls_back_when_primary_array_crc_bad() {
        let mut disk = MemDisk::new(2_048);
        install_gpt(&mut disk, &[recovery_entry(100, 200)]);
        corrupt_sector(&mut disk, 2, 0);
        let gpt = GptTable::load(&mut disk).unwrap();
        assert!(gpt.used_backup);
        let rec = gpt.entries.iter().find(|e| e.is_recovery()).unwrap();
        assert_eq!(rec.first_lba, 100);
    }

    #[test]
    fn load_errors_when_both_copies_bad() {
        let mut disk = MemDisk::new(2_048);
        install_gpt(&mut disk, &[recovery_entry(100, 200)]);
        corrupt_sector(&mut disk, 1, 24);
        let backup_lba = backup_header_lba_for_disk_sectors(disk.sectors());
        corrupt_sector(&mut disk, backup_lba, 24);
        let err = GptTable::load(&mut disk).unwrap_err();
        assert!(matches!(err, YoloError::GptInvalid { .. }));
    }

    #[test]
    fn stale_primary_when_device_grew() {
        let mut small = MemDisk::new(2_048);
        install_gpt(&mut small, &[recovery_entry(100, 200)]);
        let mut grown = MemDisk::new(4_096);
        grown.data[..small.data.len()].copy_from_slice(&small.data);
        let gpt = GptTable::load(&mut grown).unwrap();
        assert!(!gpt.used_backup);
        assert!(gpt.stale_primary_gpt);
        let rec = gpt.entries.iter().find(|e| e.is_recovery()).unwrap();
        assert_eq!(rec.first_lba, 100);
    }

    #[test]
    fn is_stale_uses_covering_lba_for_backup_headers() {
        let backup = test_header(2_047, 1, 2_047 - 32, 2_014, 0);
        assert!(!is_stale_gpt(&backup, 2_048));
        assert!(is_stale_gpt(&backup, 4_096));
        let primary = test_header(1, 2_047, 2, 2_014, 0);
        assert!(!is_stale_gpt(&primary, 2_048));
        assert!(is_stale_gpt(&primary, 4_096));
    }

    #[test]
    fn commit_writes_backup_then_primary() {
        let mut disk = MemDisk::new(2_048);
        install_gpt(&mut disk, &[recovery_entry(100, 200)]);
        let mut gpt = GptTable::load(&mut disk).unwrap();
        gpt.update_recovery_lbas(3, 1_000, 1_100).unwrap();
        gpt.commit(&mut disk).unwrap();

        let array_sectors = 32u64;
        let backup_header = backup_header_lba_for_disk_sectors(disk.sectors());
        let backup_array = backup_header - array_sectors;
        assert_eq!(
            disk.writes,
            vec![
                (backup_array, array_sectors),
                (backup_header, 1),
                (2, array_sectors),
                (1, 1),
            ]
        );

        let gpt = GptTable::load(&mut disk).unwrap();
        assert!(!gpt.used_backup);
        let rec = gpt.entries.iter().find(|e| e.is_recovery()).unwrap();
        assert_eq!(rec.first_lba, 1_000);
        assert_eq!(rec.last_lba, 1_100);
    }

    #[test]
    fn commit_after_backup_load_repairs_primary() {
        let mut disk = MemDisk::new(2_048);
        install_gpt(&mut disk, &[recovery_entry(100, 200)]);
        corrupt_sector(&mut disk, 1, 24);
        let mut gpt = GptTable::load(&mut disk).unwrap();
        assert!(gpt.used_backup);
        gpt.commit(&mut disk).unwrap();
        assert!(!gpt.used_backup);

        let gpt = GptTable::load(&mut disk).unwrap();
        assert!(!gpt.used_backup);
        let rec = gpt.entries.iter().find(|e| e.is_recovery()).unwrap();
        assert_eq!(rec.first_lba, 100);
    }

    #[test]
    fn expanded_disk_backup_still_at_old_end() {
        // Primary CRC broken after a hypervisor grow; backup still at the old last sector.
        let mut small = MemDisk::new(2_048);
        install_gpt(&mut small, &[recovery_entry(100, 200)]);
        corrupt_sector(&mut small, 1, 24);
        let mut grown = MemDisk::new(4_096);
        grown.data[..small.data.len()].copy_from_slice(&small.data);
        let gpt = GptTable::load(&mut grown).unwrap();
        assert!(gpt.used_backup);
        assert!(gpt.stale_primary_gpt);
        assert_eq!(gpt.entry_lba, 2);
        let rec = gpt.entries.iter().find(|e| e.is_recovery()).unwrap();
        assert_eq!(rec.first_lba, 100);
    }
}
