//! IEEE 802.3 CRC-32 checksum (`libdeflate_crc32`), computed eight bytes at a time.

/// Reflected IEEE 802.3 CRC-32 polynomial ($x^{32} + x^{26} + x^{23} + x^{22} + x^{16} + x^{12}
/// + x^{11} + x^{10} + x^8 + x^7 + x^5 + x^4 + x^2 + x + 1$).
const IEEE_POLYNOMIAL: u32 = 0xEDB8_8320;

/// Slice-by-8 lookup tables for IEEE CRC-32, built at compile time.
const CRC32_TABLE: [[u32; 256]; 8] = {
    let mut table = [[0u32; 256]; 8];
    let mut byte = 0usize;
    while byte < 256 {
        let mut crc = byte as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ IEEE_POLYNOMIAL
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[0][byte] = crc;
        byte += 1;
    }

    let mut slice = 1usize;
    while slice < 8 {
        let mut i = 0usize;
        while i < 256 {
            let prev = table[slice - 1][i];
            table[slice][i] = (prev >> 8) ^ table[0][(prev & 0xFF) as usize];
            i += 1;
        }
        slice += 1;
    }
    table
};

/// Computes the IEEE 802.3 CRC-32 checksum of `data`, as gzip streams end with
/// (`libdeflate_crc32` from `0`).
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0, data)
}

/// Continues the IEEE 802.3 CRC-32 checksum `crc` over `data` (`libdeflate_crc32`).
#[must_use]
pub fn crc32_update(crc: u32, data: &[u8]) -> u32 {
    let mut state = !crc;
    let (chunks, remainder) = data.as_chunks::<8>();
    for &chunk in chunks {
        let lo = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) ^ state;
        let hi = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        state = CRC32_TABLE[7][(lo & 0xFF) as usize]
            ^ CRC32_TABLE[6][((lo >> 8) & 0xFF) as usize]
            ^ CRC32_TABLE[5][((lo >> 16) & 0xFF) as usize]
            ^ CRC32_TABLE[4][(lo >> 24) as usize]
            ^ CRC32_TABLE[3][(hi & 0xFF) as usize]
            ^ CRC32_TABLE[2][((hi >> 8) & 0xFF) as usize]
            ^ CRC32_TABLE[1][((hi >> 16) & 0xFF) as usize]
            ^ CRC32_TABLE[0][(hi >> 24) as usize];
    }
    for &byte in remainder {
        state = (state >> 8) ^ CRC32_TABLE[0][usize::from((state as u8) ^ byte)];
    }
    !state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ieee_check_values_and_chunking() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32_update(0, b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"abc"), 0x3524_41C2);

        let full = b"the quick brown fox jumps over the lazy dog, repeatedly and across chunks!";
        let expected = crc32(full);
        assert_eq!(crc32_update(expected, b""), expected);
        for split in 0..=full.len() {
            let part1 = crc32(&full[..split]);
            let combined = crc32_update(part1, &full[split..]);
            assert_eq!(combined, expected, "split at {split}");
        }
    }
}
