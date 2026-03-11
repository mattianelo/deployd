use std::io::Read;
use std::path::Path;

use anyhow::Result;

/// Read the list of master files required by a Bethesda plugin.
///
/// Parses the TES4 (or TES3) record header of an `.esp`/`.esm`/`.esl` file
/// and returns the filenames listed in its `MAST` subrecords.
///
/// Returns an empty `Vec` for non-plugin files or files that cannot be read.
pub fn read_masters(path: &Path) -> Result<Vec<String>> {
    let mut file = std::fs::File::open(path)?;

    // TES4/TES5 record layout (Skyrim and later, 24-byte header):
    //   [0..4]   record type (ASCII, e.g. "TES4")
    //   [4..8]   data_size: u32 LE  (bytes of subrecord data after this header)
    //   [8..12]  flags: u32 LE
    //   [12..16] formID: u32 LE
    //   [16..20] version control info 1: u32 LE
    //   [20..22] version: u16 LE
    //   [22..24] unknown: u16 LE
    // Oblivion used a 20-byte header (no vci1 field), but Skyrim/FO3/FO4/SSE all use 24.
    // We read 24 bytes so that `file` is positioned at the start of the subrecords.
    let mut header = [0u8; 24];
    if file.read_exact(&mut header).is_err() {
        return Ok(vec![]);
    }
    if &header[0..4] != b"TES4" && &header[0..4] != b"TES3" {
        return Ok(vec![]);
    }
    let data_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;

    let mut data = vec![0u8; data_size];
    if file.read_exact(&mut data).is_err() {
        return Ok(vec![]);
    }

    // Parse subrecords: [4 type][2 size LE][size bytes data] ...
    let mut masters = Vec::new();
    let mut i = 0usize;
    while i + 6 <= data.len() {
        let sub_type = &data[i..i + 4];
        let sub_size = u16::from_le_bytes([data[i + 4], data[i + 5]]) as usize;
        i += 6;
        if i + sub_size > data.len() {
            break;
        }
        if sub_type == b"MAST" {
            let raw = &data[i..i + sub_size];
            // NUL-terminated string
            let end = raw.iter().position(|&b| b == 0).unwrap_or(sub_size);
            if let Ok(name) = std::str::from_utf8(&raw[..end])
                && !name.is_empty()
            {
                masters.push(name.to_string());
            }
        }
        i += sub_size;
    }

    Ok(masters)
}
