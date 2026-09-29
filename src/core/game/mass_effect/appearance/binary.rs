use anyhow::{Context, Result, ensure};

pub(super) const MAX_FILE: usize = 64 * 1024 * 1024;
pub(super) const MAX_ITEMS: usize = 1_000_000;

pub(super) struct Reader<'a> {
    pub bytes: &'a [u8],
    pub position: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(size)
            .context("Save offset overflow")?;
        let result = self
            .bytes
            .get(self.position..end)
            .context("Truncated save or headmorph")?;
        self.position = end;
        Ok(result)
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    pub fn boolean(&mut self) -> Result<bool> {
        let value = self.u32()?;
        ensure!(value <= 1, "Invalid save boolean");
        Ok(value == 1)
    }

    pub fn count(&mut self, minimum_size: usize) -> Result<usize> {
        let count = self.u32()? as usize;
        ensure!(count <= MAX_ITEMS, "Save collection is too large");
        ensure!(
            count <= (self.bytes.len() - self.position) / minimum_size.max(1),
            "Truncated save collection"
        );
        Ok(count)
    }

    pub fn array(&mut self, size: usize) -> Result<()> {
        let count = self.count(size)?;
        self.take(count * size)?;
        Ok(())
    }

    pub fn float(&mut self) -> Result<f32> {
        let value = f32::from_bits(self.u32()?);
        ensure!(value.is_finite(), "Non-finite appearance value");
        Ok(value)
    }

    pub fn string(&mut self) -> Result<String> {
        let length = self.u32()? as i32;
        let count = length.checked_abs().context("Invalid string length")? as usize;
        ensure!(count <= 65536, "Save string is too long");
        if count == 0 {
            return Ok(String::new());
        }
        if length < 0 {
            let bytes = self.take(count * 2)?;
            ensure!(bytes.ends_with(&[0, 0]), "Unterminated save string");
            let units: Vec<_> = bytes[..bytes.len() - 2]
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            Ok(String::from_utf16(&units).context("Invalid UTF-16 save string")?)
        } else {
            let bytes = self.take(count)?;
            ensure!(bytes.last() == Some(&0), "Unterminated save string");
            const CP1252: [char; 32] = [
                '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}',
                'Ž', '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›',
                'œ', '\u{9d}', 'ž', 'Ÿ',
            ];
            Ok(bytes[..count - 1]
                .iter()
                .map(|&b| {
                    if (0x80..0xa0).contains(&b) {
                        CP1252[(b - 0x80) as usize]
                    } else {
                        char::from(b)
                    }
                })
                .collect())
        }
    }
}

pub(super) fn word(out: &mut Vec<u8>, value: u32) {
    out.extend(value.to_le_bytes());
}
pub(super) fn string(out: &mut Vec<u8>, value: &str) {
    if value.is_empty() {
        word(out, 0);
    } else if value.is_ascii() {
        word(out, (value.len() + 1) as u32);
        out.extend(value.as_bytes());
        out.push(0);
    } else {
        let units: Vec<_> = value.encode_utf16().collect();
        word(out, (-(units.len() as i32 + 1)) as u32);
        for unit in units {
            out.extend(unit.to_le_bytes());
        }
        out.extend([0, 0]);
    }
}

pub(super) fn checksum(bytes: &[u8]) -> u32 {
    let mut hash = crc32fast::Hasher::new();
    let mut buffer = [0u8; 4096];
    for chunk in bytes.chunks(buffer.len()) {
        for (target, source) in buffer.iter_mut().zip(chunk) {
            *target = source.reverse_bits();
        }
        hash.update(&buffer[..chunk.len()]);
    }
    hash.finalize().reverse_bits()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_the_crc32_bzip2_check_vector() {
        assert_eq!(checksum(b"123456789"), 0xfc891918);
    }
    #[test]
    fn rejects_invalid_string_lengths_and_terminators() {
        assert!(Reader::new(&i32::MIN.to_le_bytes()).string().is_err());
        assert!(Reader::new(&[1, 0, 0, 0, 65]).string().is_err());
    }
}
