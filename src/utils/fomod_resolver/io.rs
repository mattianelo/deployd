use std::path::Path;

use anyhow::{Context, Result};

/// Read FOMOD XML with BOM and encoding handling.
/// Supports UTF-8 (with/without BOM), UTF-16LE, and UTF-16BE.
pub(super) fn read_fomod_xml(config_path: &Path) -> Result<String> {
    let raw = std::fs::read(config_path)
        .with_context(|| format!("Cannot read FOMOD config: {}", config_path.display()))?;

    let text = if raw.starts_with(&[0xFF, 0xFE]) {
        // UTF-16LE BOM
        decode_utf16le(&raw[2..])
    } else if raw.starts_with(&[0xFE, 0xFF]) {
        // UTF-16BE BOM
        decode_utf16be(&raw[2..])
    } else if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
        // UTF-8 BOM
        String::from_utf8_lossy(&raw[3..]).into_owned()
    } else if looks_utf16le(&raw) {
        // UTF-16LE without BOM (detected by null byte pattern)
        decode_utf16le(&raw)
    } else {
        String::from_utf8_lossy(&raw).into_owned()
    };

    Ok(text)
}

fn decode_utf16le(data: &[u8]) -> String {
    let iter = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    char::decode_utf16(iter)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn decode_utf16be(data: &[u8]) -> String {
    let iter = data
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]));
    char::decode_utf16(iter)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Heuristic: if the file has even length and every other byte is 0x00 in the
/// first few bytes, it's likely UTF-16LE without BOM.
fn looks_utf16le(data: &[u8]) -> bool {
    if data.len() < 4 || !data.len().is_multiple_of(2) {
        return false;
    }
    // Check first 8 byte pairs: in UTF-16LE ASCII, odd bytes are 0x00
    let check_len = data.len().min(16);
    let null_count = data[1..check_len]
        .iter()
        .step_by(2)
        .filter(|&&b| b == 0)
        .count();
    null_count >= 3
}
