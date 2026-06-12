//! Read/write GPT partition entry arrays and headers (primary + backup).

use crate::error::{Result, YoloError};
use crate::gpt::{
    backup_header_lba_for_disk_sectors, disk_sector_count, efi_crc32, gpt_header_crc_valid,
    last_usable_lba_for_disk_sectors, GptHeader, GptPartitionEntry, PARTITION_COUNT,
    PARTITION_ENTRY_SIZE, SECTOR_SIZE,
};
use crate::platform::windows::disk::PhysicalDisk;
use tracing::info;

const PRIMARY_HEADER_LBA: u64 = 1;

pub struct GptOnDisk {
    pub primary_header: GptHeader,
    pub entries: Vec<GptPartitionEntry>,
    pub entry_lba: u64,
    pub backup_header_lba: u64,
    pub backup_entry_lba: u64,
    /// Sectors occupied by the partition entry array (`partition_count` entries).
    array_sectors: u64,
}

impl GptOnDisk {
    pub fn load(disk: &mut PhysicalDisk) -> Result<Self> {
        let sector = disk.read_one_sector(PRIMARY_HEADER_LBA)?;
        let primary_header = GptHeader::parse(&sector)?;

        if primary_header.partition_entry_size as usize != PARTITION_ENTRY_SIZE {
            return Err(YoloError::GptInvalid {
                detail: format!(
                    "unexpected partition entry size {}",
                    primary_header.partition_entry_size
                ),
            });
        }
        let count = primary_header.partition_count as usize;
        if count == 0 || count > PARTITION_COUNT {
            return Err(YoloError::GptInvalid {
                detail: format!("unsupported partition count {count}"),
            });
        }

        let entry_lba = primary_header.partition_entry_lba;
        let array_bytes = count * PARTITION_ENTRY_SIZE;
        let array_sectors = (array_bytes as u64).div_ceil(SECTOR_SIZE);
        let disk_sectors = disk_sector_count(disk.size_bytes, disk.sector_size);
        let backup_header_lba = backup_header_lba_for_disk_sectors(disk_sectors);
        let backup_entry_lba = backup_header_lba.saturating_sub(array_sectors);

        let mut raw = vec![0u8; (array_sectors * SECTOR_SIZE) as usize];
        disk.read_sectors(entry_lba, array_sectors, &mut raw)?;

        let mut entries = Vec::new();
        for i in 0..count {
            let off = i * PARTITION_ENTRY_SIZE;
            let slice = &raw[off..off + PARTITION_ENTRY_SIZE];
            let entry = GptPartitionEntry::parse(i as u32, slice)?;
            entries.push(entry);
        }

        Ok(Self {
            primary_header,
            entries,
            entry_lba,
            backup_header_lba,
            backup_entry_lba,
            array_sectors,
        })
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

    /// Write partition arrays and refreshed headers to disk.
    pub fn commit(&mut self, disk: &mut PhysicalDisk) -> Result<()> {
        self.sync_device_geometry(disk);
        let entries_raw = self.serialize_entries()?;
        // The array CRC covers exactly partition_count entries, not sector padding.
        self.primary_header = self
            .primary_header
            .clone()
            .with_partition_array_crc(&entries_raw[..self.array_bytes()]);

        info!("writing primary GPT partition array");
        disk.write_sectors(self.entry_lba, self.array_sectors, &entries_raw)?;

        info!("writing backup GPT partition array");
        disk.write_sectors(self.backup_entry_lba, self.array_sectors, &entries_raw)?;

        let mut primary_sector = vec![0u8; SECTOR_SIZE as usize];
        self.primary_header.write_to_sector(&mut primary_sector)?;
        disk.write_sectors(PRIMARY_HEADER_LBA, 1, &primary_sector)?;

        let backup_header = self.backup_header();
        let mut backup_sector = vec![0u8; SECTOR_SIZE as usize];
        backup_header.write_to_sector(&mut backup_sector)?;
        disk.write_sectors(self.backup_header_lba, 1, &backup_sector)?;

        self.verify_crc(disk, &entries_raw)?;
        info!("GPT tables committed with valid header CRCs");
        Ok(())
    }

    fn sync_device_geometry(&mut self, disk: &PhysicalDisk) {
        let disk_sectors = disk_sector_count(disk.size_bytes, disk.sector_size);
        self.backup_header_lba = backup_header_lba_for_disk_sectors(disk_sectors);
        self.backup_entry_lba = self.backup_header_lba.saturating_sub(self.array_sectors);
        self.primary_header.backup_lba = self.backup_header_lba;
        self.primary_header.last_usable_lba = last_usable_lba_for_disk_sectors(disk_sectors);
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

    fn verify_crc(&self, disk: &mut PhysicalDisk, entries_raw: &[u8]) -> Result<()> {
        let array_crc = efi_crc32(&entries_raw[..self.array_bytes()]);
        if array_crc != self.primary_header.partition_array_crc32 {
            return Err(YoloError::GptInvalid {
                detail: "partition array CRC mismatch after write".into(),
            });
        }
        let hdr_size = self.primary_header.header_size as usize;
        let primary_sector = disk.read_one_sector(PRIMARY_HEADER_LBA)?;
        if !gpt_header_crc_valid(&primary_sector, hdr_size) {
            return Err(YoloError::GptInvalid {
                detail: "primary header CRC mismatch after write".into(),
            });
        }
        let backup_sector = disk.read_one_sector(self.backup_header_lba)?;
        if !gpt_header_crc_valid(&backup_sector, hdr_size) {
            return Err(YoloError::GptInvalid {
                detail: "backup header CRC mismatch after write".into(),
            });
        }
        Ok(())
    }
}
