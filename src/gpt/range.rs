use std::ops::RangeInclusive;

/// Inclusive LBA range on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LbaRange {
    pub first: u64,
    pub last: u64,
}

impl LbaRange {
    pub const fn new(first: u64, last: u64) -> Self {
        Self { first, last }
    }

    pub fn sector_count(self) -> u64 {
        self.last.saturating_sub(self.first).saturating_add(1)
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.first <= other.last && other.first <= self.last
    }

    pub fn is_entirely_before(self, other: Self) -> bool {
        self.last < other.first
    }

    pub fn as_inclusive(self) -> RangeInclusive<u64> {
        self.first..=self.last
    }
}
