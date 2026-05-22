//! EFI/UEFI CRC-32 used by GPT headers and partition entry arrays.

/// EFI CRC-32 (IEEE / PKZIP polynomial `0xEDB88320`).
#[inline]
pub fn efi_crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_efi_crc() {
        assert_eq!(efi_crc32(b""), 0);
    }

    #[test]
    fn known_ascii_vector() {
        assert_eq!(efi_crc32(b"123456789"), 0xCBF4_3926);
    }
}
