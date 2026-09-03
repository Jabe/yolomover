//! Read/write GPT partition entry arrays and headers (primary + backup).

use crate::error::Result;
use crate::gpt::{GptTable, SectorIo};
use crate::platform::windows::disk::PhysicalDisk;

pub type GptOnDisk = GptTable;

impl SectorIo for PhysicalDisk {
    fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    fn sector_size(&self) -> u64 {
        self.sector_size
    }

    fn read_sectors(&mut self, start_lba: u64, count: u64, buf: &mut [u8]) -> Result<()> {
        PhysicalDisk::read_sectors(self, start_lba, count, buf)
    }

    fn write_sectors(&mut self, start_lba: u64, count: u64, buf: &[u8]) -> Result<()> {
        PhysicalDisk::write_sectors(self, start_lba, count, buf)
    }
}
