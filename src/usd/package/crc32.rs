//! CRC-32 (ISO 3309, the polynomial ZIP uses), slice-by-eight: the check
//! every usdz entry carries.

/// The eight tables slice-by-eight reads: `TABLES[0]` the byte-at-a-time
/// table, `TABLES[k]` a byte's remainder `k` bytes further on.
const TABLES: [[u32; 256]; 8] = tables();

const fn tables() -> [[u32; 256]; 8] {
    let mut tables = [[0u32; 256]; 8];
    let mut byte = 0;
    while byte < 256 {
        let mut crc = byte as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        tables[0][byte] = crc;
        byte += 1;
    }
    let mut k = 1;
    while k < 8 {
        let mut byte = 0;
        while byte < 256 {
            let previous = tables[k - 1][byte];
            tables[k][byte] = (previous >> 8) ^ tables[0][(previous & 0xFF) as usize];
            byte += 1;
        }
        k += 1;
    }
    tables
}

/// The CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    let (blocks, rest) = data.as_chunks::<8>();
    for block in blocks {
        let low = crc ^ u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        crc = TABLES[7][(low & 0xFF) as usize]
            ^ TABLES[6][((low >> 8) & 0xFF) as usize]
            ^ TABLES[5][((low >> 16) & 0xFF) as usize]
            ^ TABLES[4][(low >> 24) as usize]
            ^ TABLES[3][usize::from(block[4])]
            ^ TABLES[2][usize::from(block[5])]
            ^ TABLES[1][usize::from(block[6])]
            ^ TABLES[0][usize::from(block[7])];
    }
    for &byte in rest {
        crc = (crc >> 8) ^ TABLES[0][((crc ^ u32::from(byte)) & 0xFF) as usize];
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn matches_the_standard_check_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
        // Every length around the eight-byte blocks agrees with a byte at a time.
        let data: Vec<u8> = (0..40u32).map(|i| (i * 37 + 11) as u8).collect();
        for length in 0..data.len() {
            let mut slow = !0u32;
            for &byte in &data[..length] {
                slow ^= u32::from(byte);
                for _ in 0..8 {
                    slow = if slow & 1 != 0 {
                        (slow >> 1) ^ 0xEDB8_8320
                    } else {
                        slow >> 1
                    };
                }
            }
            assert_eq!(crc32(&data[..length]), !slow);
        }
    }
}
