//! EFI/UEFI CRC-32 used by GPT headers and partition entry arrays.

/// EFI CRC-32 (IEEE / PKZIP polynomial `0xEDB88320`).
#[inline]
pub fn efi_crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// True when `sector[..header_size]` matches the stored GPT header CRC32 field.
pub fn gpt_header_crc_valid(sector: &[u8], header_size: usize) -> bool {
    if header_size < 20 || sector.len() < header_size {
        return false;
    }
    let stored = u32::from_le_bytes(sector[16..20].try_into().expect("header crc field"));
    let mut hdr = sector[..header_size].to_vec();
    hdr[16..20].fill(0);
    efi_crc32(&hdr) == stored
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

    #[test]
    fn gpt_header_crc_roundtrip() {
        let mut sector = [0u8; 512];
        sector[0..8].copy_from_slice(&0x5452_4150_2049_4645u64.to_le_bytes());
        sector[12..16].copy_from_slice(&92u32.to_le_bytes());
        let hdr_size = 92usize;
        let crc = efi_crc32(&{
            let mut h = sector[..hdr_size].to_vec();
            h[16..20].fill(0);
            h
        });
        sector[16..20].copy_from_slice(&crc.to_le_bytes());
        assert!(super::gpt_header_crc_valid(&sector, hdr_size));
    }
}
