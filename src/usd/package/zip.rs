// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! The ZIP subset usdz packages use (`pxr/usd/sdf/zipFile.cpp`): entries
//! stored uncompressed, each entry's data 64-byte aligned by padding the
//! local header's extra field, read in place from the archive's bytes.

use super::crc32::crc32;
use crate::usd::error::{Error, Result};

const LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const END_SIGNATURE: u32 = 0x0605_4b50;
/// The extra field id usdz uses for alignment padding.
const PADDING_ID: u16 = 0x1986;

/// One entry of an archive: its name and where its stored data lies.
#[derive(Clone, Debug)]
pub struct Entry {
    /// The entry's path in the archive.
    pub name: String,
    /// The data's offset in the archive.
    pub offset: usize,
    /// The data's size.
    pub size: usize,
    /// The compression method (0: stored).
    pub method: u16,
}

fn u16_at(data: &[u8], at: usize) -> Result<u16> {
    let bytes = data
        .get(at..at + 2)
        .ok_or_else(|| Error::invalid("a ZIP archive read past its end"))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    let bytes = data
        .get(at..at + 4)
        .ok_or_else(|| Error::invalid("a ZIP archive read past its end"))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// The entries of the archive `data`, in central directory order.
pub fn entries(data: &[u8]) -> Result<Vec<Entry>> {
    // The end of central directory record: 22 bytes, then up to 64 KiB of comment.
    let search_from = data.len().saturating_sub(22 + 65_535);
    let end = (search_from..=data.len().saturating_sub(22))
        .rev()
        .find(|&at| u32_at(data, at).ok() == Some(END_SIGNATURE))
        .ok_or_else(|| Error::invalid("not a ZIP archive: no end of central directory"))?;
    let count = usize::from(u16_at(data, end + 10)?);
    let directory = u32_at(data, end + 16)? as usize;
    if count == 0xFFFF || directory == 0xFFFF_FFFF_usize {
        return Err(Error::unsupported("ZIP64 packages are not supported"));
    }
    let mut entries = Vec::with_capacity(count);
    let mut at = directory;
    for _ in 0..count {
        if u32_at(data, at)? != CENTRAL_SIGNATURE {
            return Err(Error::invalid("a damaged ZIP central directory"));
        }
        let method = u16_at(data, at + 10)?;
        let compressed = u32_at(data, at + 20)? as usize;
        let name_length = usize::from(u16_at(data, at + 28)?);
        let extra_length = usize::from(u16_at(data, at + 30)?);
        let comment_length = usize::from(u16_at(data, at + 32)?);
        let local = u32_at(data, at + 42)? as usize;
        let name = data
            .get(at + 46..at + 46 + name_length)
            .ok_or_else(|| Error::invalid("a ZIP entry name past the archive's end"))?;
        let name = String::from_utf8(name.to_vec())
            .map_err(|_| Error::invalid("a ZIP entry name that is not UTF-8"))?;
        if u32_at(data, local)? != LOCAL_SIGNATURE {
            return Err(Error::invalid("a damaged ZIP local header"));
        }
        let local_name = usize::from(u16_at(data, local + 26)?);
        let local_extra = usize::from(u16_at(data, local + 28)?);
        let offset = local + 30 + local_name + local_extra;
        if offset
            .checked_add(compressed)
            .is_none_or(|end| end > data.len())
        {
            return Err(Error::invalid("a ZIP entry runs past the archive's end"));
        }
        entries.push(Entry {
            name,
            offset,
            size: compressed,
            method,
        });
        at += 46 + name_length + extra_length + comment_length;
    }
    Ok(entries)
}

/// The stored bytes of `entry`; fails for a compressed entry.
pub fn stored<'a>(data: &'a [u8], entry: &Entry) -> Result<&'a [u8]> {
    if entry.method != 0 {
        return Err(Error::invalid(format!(
            "the package entry {} is compressed; usdz entries must be stored",
            entry.name
        )));
    }
    Ok(&data[entry.offset..entry.offset + entry.size])
}

/// Write a stored archive of `files` (name, bytes), each entry's data
/// aligned to `alignment` bytes.
pub fn write_stored(files: &[(&str, &[u8])], alignment: usize) -> Result<Vec<u8>> {
    let total: usize = files
        .iter()
        .map(|(name, data)| 30 + name.len() + 4 + alignment + data.len() + 46 + name.len())
        .sum();
    if total + 22 > u32::MAX as usize || files.len() > 0xFFFE {
        return Err(Error::limit(
            "the package is too large for a ZIP without ZIP64",
        ));
    }
    let mut out = Vec::with_capacity(total + 22);
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32(data);
        let local = out.len();
        let header_end = local + 30 + name.len();
        // Pad the extra field so the data starts on the alignment.
        let mut padding = (alignment - header_end % alignment) % alignment;
        if padding > 0 && padding < 4 {
            padding += alignment;
        }
        let mut header = Vec::with_capacity(30 + name.len() + padding);
        header.extend_from_slice(&LOCAL_SIGNATURE.to_le_bytes());
        header.extend_from_slice(&10u16.to_le_bytes()); // version needed
        header.extend_from_slice(&0u16.to_le_bytes()); // flags
        header.extend_from_slice(&0u16.to_le_bytes()); // stored
        header.extend_from_slice(&0u16.to_le_bytes()); // time
        header.extend_from_slice(&0x21u16.to_le_bytes()); // date (1980-01-01)
        header.extend_from_slice(&crc.to_le_bytes());
        header.extend_from_slice(&(data.len() as u32).to_le_bytes());
        header.extend_from_slice(&(data.len() as u32).to_le_bytes());
        header.extend_from_slice(&(name.len() as u16).to_le_bytes());
        header.extend_from_slice(&(padding as u16).to_le_bytes());
        header.extend_from_slice(name.as_bytes());
        if padding > 0 {
            header.extend_from_slice(&PADDING_ID.to_le_bytes());
            header.extend_from_slice(&((padding - 4) as u16).to_le_bytes());
            header.resize(header.len() + padding - 4, 0);
        }
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        central.extend_from_slice(&CENTRAL_SIGNATURE.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&header[4..30 - 2]); // version needed .. name length
        central.extend_from_slice(&0u16.to_le_bytes()); // extra length
        central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&(local as u32).to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let directory = out.len();
    out.extend_from_slice(&central);
    out.extend_from_slice(&END_SIGNATURE.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&(directory as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_archives_round_trip_aligned() {
        let archive = write_stored(
            &[
                ("root.usdc", b"PXR-USDC....".as_slice()),
                ("tex/a.png", &[1, 2, 3]),
            ],
            64,
        )
        .unwrap();
        let found = entries(&archive).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "root.usdc");
        assert_eq!(found[0].offset % 64, 0);
        assert_eq!(found[1].offset % 64, 0);
        assert_eq!(stored(&archive, &found[1]).unwrap(), [1, 2, 3]);
        assert!(entries(&archive[..archive.len() - 3]).is_err());
    }
}
