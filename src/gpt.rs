//! Portable GPT structures (UEFI spec) for parsing and planning.
//!
//! Windows-specific disk I/O lives under `platform::windows`.

mod crc;
mod range;

pub use crc::efi_crc32;
pub use range::LbaRange;

use crate::error::{Result, YoloError};
use std::fmt;
use uuid::Uuid;

/// Recovery partition type GUID on disk (little-endian field layout).
const RECOVERY_TYPE_BYTES: [u8; 16] = [
    0xA4, 0xBB, 0x94, 0xDE, 0xD1, 0x06, 0x40, 0x4D, 0xA1, 0x6A, 0xBF, 0xD5, 0x01, 0x79, 0xD6, 0xAC,
];

/// ESP type GUID on disk.
const ESP_TYPE_BYTES: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0xF8, 0x1F, 0x11, 0xD2, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

fn bytes_at<const N: usize>(data: &[u8], offset: usize, field: &'static str) -> Result<[u8; N]> {
    data.get(offset..offset + N)
        .and_then(|slice| slice.try_into().ok())
        .ok_or_else(|| YoloError::GptInvalid {
            detail: format!("missing {field} at offset {offset}"),
        })
}

pub const SECTOR_SIZE: u64 = 512;
pub const PARTITION_ENTRY_SIZE: usize = 128;
pub const PARTITION_COUNT: usize = 128;
pub const PARTITION_ARRAY_SECTORS: u64 = 32; // 128 * 128 / 512

/// Windows Recovery Environment partition type (Microsoft).
pub const RECOVERY_TYPE_GUID: &str = "DE94BBA4-06D1-4D40-A16A-BFD50179D6AC";

/// EFI System Partition.
pub const ESP_TYPE_GUID: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";

/// Microsoft basic data.
pub const MS_BASIC_DATA_GUID: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";

/// Required platform + hidden (typical recovery attributes).
pub const RECOVERY_GPT_ATTRIBUTES: u64 = 0x8000_0000_0000_0001;

/// 1 MiB alignment in sectors.
pub const ALIGN_SECTORS: u64 = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GptGuid {
    pub bytes: [u8; 16],
}

impl GptGuid {
    pub fn from_uuid(u: Uuid) -> Self {
        let b = u.as_bytes();
        Self {
            bytes: [
                b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11],
                b[12], b[13], b[14], b[15],
            ],
        }
    }

    pub fn from_le_bytes(raw: [u8; 16]) -> Self {
        Self { bytes: raw }
    }

    pub fn to_uuid(self) -> Uuid {
        let b = self.bytes;
        Uuid::from_bytes([
            b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12],
            b[13], b[14], b[15],
        ])
    }

    pub fn parse_str(s: &str) -> Result<Self> {
        let u = Uuid::parse_str(s)
            .map_err(|e| YoloError::GptInvalid { detail: e.to_string() })?;
        Ok(Self::from_uuid(u))
    }

    pub fn is_recovery(self) -> bool {
        self.bytes == RECOVERY_TYPE_BYTES
    }

    pub fn is_esp(self) -> bool {
        self.bytes == ESP_TYPE_BYTES
    }
}

impl fmt::Display for GptGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_uuid())
    }
}

#[derive(Debug, Clone)]
pub struct GptHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
    pub current_lba: u64,
    pub backup_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub disk_guid: GptGuid,
    pub partition_entry_lba: u64,
    pub partition_count: u32,
    pub partition_entry_size: u32,
    pub partition_array_crc32: u32,
}

impl GptHeader {
    /// Bytes on disk: `45 46 49 20 50 41 52 54` → `"EFI PART"`.
    pub const SIGNATURE: u64 = 0x5452_4150_2049_4645;

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 92 {
            return Err(YoloError::GptInvalid {
                detail: "header shorter than 92 bytes".into(),
            });
        }
        let sig = u64::from_le_bytes(bytes_at(bytes, 0, "signature")?);
        if sig != Self::SIGNATURE {
            return Err(YoloError::GptInvalid {
                detail: format!("bad signature {sig:#018x}"),
            });
        }
        Ok(Self {
            signature: sig,
            revision: u32::from_le_bytes(bytes_at(bytes, 8, "revision")?),
            header_size: u32::from_le_bytes(bytes_at(bytes, 12, "header_size")?),
            crc32: u32::from_le_bytes(bytes_at(bytes, 16, "crc32")?),
            reserved: u32::from_le_bytes(bytes_at(bytes, 20, "reserved")?),
            current_lba: u64::from_le_bytes(bytes_at(bytes, 24, "current_lba")?),
            backup_lba: u64::from_le_bytes(bytes_at(bytes, 32, "backup_lba")?),
            first_usable_lba: u64::from_le_bytes(bytes_at(bytes, 40, "first_usable_lba")?),
            last_usable_lba: u64::from_le_bytes(bytes_at(bytes, 48, "last_usable_lba")?),
            disk_guid: GptGuid::from_le_bytes(bytes_at(bytes, 56, "disk_guid")?),
            partition_entry_lba: u64::from_le_bytes(bytes_at(bytes, 72, "partition_entry_lba")?),
            partition_count: u32::from_le_bytes(bytes_at(bytes, 80, "partition_count")?),
            partition_entry_size: u32::from_le_bytes(bytes_at(bytes, 84, "partition_entry_size")?),
            partition_array_crc32: u32::from_le_bytes(bytes_at(bytes, 88, "partition_array_crc32")?),
        })
    }

    /// Serialize header fields into a 512-byte sector buffer and refresh CRC32.
    pub fn write_to_sector(&self, sector: &mut [u8]) -> Result<()> {
        if sector.len() < 512 {
            return Err(YoloError::GptInvalid {
                detail: "sector buffer too small".into(),
            });
        }
        let hdr_size = self.header_size.min(512) as usize;
        sector[..hdr_size].fill(0);
        sector[0..8].copy_from_slice(&self.signature.to_le_bytes());
        sector[8..12].copy_from_slice(&self.revision.to_le_bytes());
        sector[12..16].copy_from_slice(&self.header_size.to_le_bytes());
        // CRC32 field zeroed during checksum.
        sector[20..24].copy_from_slice(&self.reserved.to_le_bytes());
        sector[24..32].copy_from_slice(&self.current_lba.to_le_bytes());
        sector[32..40].copy_from_slice(&self.backup_lba.to_le_bytes());
        sector[40..48].copy_from_slice(&self.first_usable_lba.to_le_bytes());
        sector[48..56].copy_from_slice(&self.last_usable_lba.to_le_bytes());
        sector[56..72].copy_from_slice(&self.disk_guid.bytes);
        sector[72..80].copy_from_slice(&self.partition_entry_lba.to_le_bytes());
        sector[80..84].copy_from_slice(&self.partition_count.to_le_bytes());
        sector[84..88].copy_from_slice(&self.partition_entry_size.to_le_bytes());
        sector[88..92].copy_from_slice(&self.partition_array_crc32.to_le_bytes());

        let crc = efi_crc32(&sector[..hdr_size]);
        sector[16..20].copy_from_slice(&crc.to_le_bytes());
        Ok(())
    }

    pub fn with_partition_array_crc(mut self, entries_raw: &[u8]) -> Self {
        self.partition_array_crc32 = efi_crc32(entries_raw);
        self
    }
}

#[derive(Debug, Clone)]
pub struct GptPartitionEntry {
    pub index: u32,
    pub type_guid: GptGuid,
    pub unique_guid: GptGuid,
    pub first_lba: u64,
    pub last_lba: u64,
    pub attributes: u64,
    pub name: String,
}

impl GptPartitionEntry {
    pub fn parse(index: u32, raw: &[u8]) -> Result<Self> {
        if raw.len() < PARTITION_ENTRY_SIZE {
            return Err(YoloError::GptInvalid {
                detail: format!("entry {index} too short"),
            });
        }
        let type_guid = GptGuid::from_le_bytes(bytes_at(raw, 0, "type_guid")?);
        let unique_guid = GptGuid::from_le_bytes(bytes_at(raw, 16, "unique_guid")?);
        let first_lba = u64::from_le_bytes(bytes_at(raw, 32, "first_lba")?);
        let last_lba = u64::from_le_bytes(bytes_at(raw, 40, "last_lba")?);
        let attributes = u64::from_le_bytes(bytes_at(raw, 48, "attributes")?);
        let name_utf16: Vec<u16> = raw[56..128]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        let name = String::from_utf16_lossy(&name_utf16);
        Ok(Self {
            index,
            type_guid,
            unique_guid,
            first_lba,
            last_lba,
            attributes,
            name,
        })
    }

    pub fn is_unused(&self) -> bool {
        self.first_lba == 0 && self.last_lba == 0
    }

    pub fn sector_count(&self) -> u64 {
        self.last_lba.saturating_sub(self.first_lba) + 1
    }

    pub fn byte_size(&self) -> u64 {
        self.sector_count() * SECTOR_SIZE
    }

    pub fn is_recovery(&self) -> bool {
        self.type_guid.is_recovery()
    }

    pub fn lba_range(&self) -> LbaRange {
        LbaRange::new(self.first_lba, self.last_lba)
    }

    pub fn write_raw(&self, out: &mut [u8]) {
        out[0..16].copy_from_slice(&self.type_guid.bytes);
        out[16..32].copy_from_slice(&self.unique_guid.bytes);
        out[32..40].copy_from_slice(&self.first_lba.to_le_bytes());
        out[40..48].copy_from_slice(&self.last_lba.to_le_bytes());
        out[48..56].copy_from_slice(&self.attributes.to_le_bytes());
        let mut name = [0u8; 72];
        for (i, ch) in self.name.encode_utf16().take(36).enumerate() {
            name[i * 2..i * 2 + 2].copy_from_slice(&ch.to_le_bytes());
        }
        out[56..128].copy_from_slice(&name);
    }
}

/// How to copy partition contents when source and destination may overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyStrategy {
    /// Destination starts after source ends.
    Forward,
    /// Destination ends before source starts (copy chunks in reverse).
    Reverse,
    /// Ranges overlap: copy through an in-memory buffer.
    Buffered,
}

/// Align `value` down to `align` (must be power of two).
pub fn align_down(value: u64, align: u64) -> u64 {
    value & !(align - 1)
}

/// Align `value` up to `align`.
pub fn align_up(value: u64, align: u64) -> u64 {
    align_down(value + align - 1, align)
}

/// Compute the last aligned starting LBA for a partition of `sector_count` sectors.
pub fn end_aligned_start(
    last_usable_lba: u64,
    sector_count: u64,
    align_sectors: u64,
) -> u64 {
    let end = last_usable_lba;
    let start = end.saturating_sub(sector_count - 1);
    align_down(start, align_sectors)
}

/// Maximum recovery partition size we will buffer in RAM for overlap moves.
pub const MAX_BUFFERED_COPY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Pick a copy strategy for relocating `sector_count` sectors from `src` to `dst`.
pub fn copy_strategy(src: LbaRange, dst: LbaRange) -> CopyStrategy {
    if src.is_entirely_before(dst) {
        CopyStrategy::Forward
    } else if dst.is_entirely_before(src) {
        CopyStrategy::Reverse
    } else {
        CopyStrategy::Buffered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt_signature_matches_efi_part() {
        let on_disk = [0x45u8, 0x46, 0x49, 0x20, 0x50, 0x41, 0x52, 0x54];
        let sig = u64::from_le_bytes(on_disk);
        assert_eq!(sig, GptHeader::SIGNATURE);
        assert_eq!(sig, 0x5452_4150_2049_4645);
    }

    #[test]
    fn recovery_guid_matches() {
        let g = GptGuid::from_le_bytes(RECOVERY_TYPE_BYTES);
        assert!(g.is_recovery());
        assert_eq!(GptGuid::parse_str(RECOVERY_TYPE_GUID).unwrap(), g);
    }

    #[test]
    fn copy_strategy_forward_to_end() {
        let src = LbaRange::new(100, 200);
        let dst = LbaRange::new(500, 600);
        assert_eq!(copy_strategy(src, dst), CopyStrategy::Forward);
    }

    #[test]
    fn copy_strategy_buffered_on_overlap() {
        let src = LbaRange::new(100, 500);
        let dst = LbaRange::new(300, 700);
        assert_eq!(copy_strategy(src, dst), CopyStrategy::Buffered);
    }

    #[test]
    fn alignment() {
        assert_eq!(align_up(2047, ALIGN_SECTORS), 2048);
        assert_eq!(align_down(2049, ALIGN_SECTORS), 2048);
    }

    #[test]
    fn header_crc_roundtrip() {
        let mut sector = vec![0u8; 512];
        sector[0..8].copy_from_slice(&GptHeader::SIGNATURE.to_le_bytes());
        sector[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        sector[12..16].copy_from_slice(&92u32.to_le_bytes());
        sector[24..32].copy_from_slice(&1u64.to_le_bytes());
        sector[32..40].copy_from_slice(&100u64.to_le_bytes());
        sector[40..48].copy_from_slice(&34u64.to_le_bytes());
        sector[48..56].copy_from_slice(&99u64.to_le_bytes());
        sector[72..80].copy_from_slice(&2u64.to_le_bytes());
        sector[80..84].copy_from_slice(&128u32.to_le_bytes());
        sector[84..88].copy_from_slice(&(PARTITION_ENTRY_SIZE as u32).to_le_bytes());

        let parsed = GptHeader::parse(&sector).expect("parse synthetic header");
        let mut out = vec![0u8; 512];
        parsed.write_to_sector(&mut out).unwrap();
        let stored = u32::from_le_bytes(out[16..20].try_into().unwrap());
        let mut check = out.clone();
        check[16..20].fill(0);
        assert_eq!(stored, efi_crc32(&check[..92]));
    }
}
