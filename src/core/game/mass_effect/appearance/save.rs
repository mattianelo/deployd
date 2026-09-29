use std::io::{Read, Write};
use std::ops::Range;

use anyhow::{Context, Result, ensure};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};

use super::{
    binary::{self, MAX_FILE, Reader},
    morph::HeadMorph,
};
use crate::core::game::mass_effect::Target;

#[derive(Debug, Clone)]
pub(crate) struct Document {
    pub target: Target,
    pub name: String,
    pub level: u32,
    pub female: bool,
    pub morph: Option<HeadMorph>,
    original_morph: Option<HeadMorph>,
    original: Vec<u8>,
    payload: Vec<u8>,
    range: Range<usize>,
}

impl Document {
    pub fn read(bytes: &[u8], target: Target) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_FILE && bytes.len() >= 16,
            "Invalid save size (maximum 64 MiB)"
        );
        let payload = if target == Target::Le1 {
            decompress(bytes)?
        } else {
            let end = bytes.len() - 4;
            ensure!(
                binary::checksum(&bytes[..end]) == u32::from_le_bytes(bytes[end..].try_into()?),
                "Save checksum does not match; the file may be damaged"
            );
            bytes[..end].to_vec()
        };
        let mut r = Reader::new(&payload);
        let version = r.u32()?;
        ensure!(
            version
                == match target {
                    Target::Le1 => 50,
                    Target::Le2 => 30,
                    Target::Le3 => 59,
                },
            "Unsupported PC Legendary Edition save version {version}"
        );
        if target == Target::Le1 {
            le1_header(&mut r)?;
        } else {
            le23_header(&mut r, target)?;
        }
        let female = r.boolean()?;
        if target == Target::Le1 {
            r.take(5)?;
        } else {
            r.string()?;
            if target == Target::Le3 {
                for _ in 0..3 {
                    r.boolean()?;
                }
            }
        }
        let level = r.u32()?;
        r.take(4)?;
        let name = r.string()?;
        r.take(6)?;
        if target == Target::Le1 {
            r.take(13)?;
            r.string()?;
        } else {
            r.take(4)?;
            for _ in 0..3 {
                r.string()?;
            }
            r.take(1 + 13 * 4)?;
        }
        let start = r.position;
        let morph = if r.boolean()? {
            Some(HeadMorph::read(&mut r)?)
        } else {
            None
        };
        let end = r.position;
        super::layout::validate_suffix(&mut r, target)?;
        ensure!(
            end < payload.len(),
            "Save is missing data after the appearance"
        );
        Ok(Self {
            target,
            name,
            level,
            female,
            original_morph: morph.clone(),
            morph,
            original: bytes.to_vec(),
            range: start..end,
            payload,
        })
    }

    pub fn reset(&mut self) {
        self.morph = self.original_morph.clone();
    }
    pub fn changed(&self) -> bool {
        self.morph != self.original_morph
    }
    pub fn original(&self) -> &[u8] {
        &self.original
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        if !self.changed() {
            return Ok(self.original.clone());
        }
        let mut payload = self.payload[..self.range.start].to_vec();
        binary::word(&mut payload, u32::from(self.morph.is_some()));
        if let Some(morph) = &self.morph {
            morph.validate()?;
            morph.write(&mut payload);
        }
        payload.extend(&self.payload[self.range.end..]);
        ensure!(payload.len() <= MAX_FILE, "Edited save is too large");
        let bytes = if self.target == Target::Le1 {
            compress(&payload)?
        } else {
            let checksum = binary::checksum(&payload);
            binary::word(&mut payload, checksum);
            payload
        };
        let verified = Self::read(&bytes, self.target)?;
        ensure!(
            verified.morph == self.morph
                && verified.payload[..verified.range.start] == self.payload[..self.range.start]
                && verified.payload[verified.range.end..] == self.payload[self.range.end..],
            "Edited save failed integrity verification"
        );
        Ok(bytes)
    }
}

fn le23_header(r: &mut Reader<'_>, target: Target) -> Result<()> {
    r.string()?;
    r.take(8)?;
    r.string()?;
    if target == Target::Le3 {
        r.string()?;
    }
    r.take(1 + 4 + 16 + 12 + 12 + 4)?;
    for _ in 0..r.count(12)? {
        r.string()?;
        r.boolean()?;
        r.boolean()?;
    }
    for _ in 0..r.count(8)? {
        r.string()?;
        r.boolean()?;
    }
    r.array(20)?;
    r.array(18)?;
    if target == Target::Le3 {
        r.array(18)?;
    }
    r.array(16)?;
    Ok(())
}

fn le1_header(r: &mut Reader<'_>) -> Result<()> {
    r.string()?;
    r.take(16)?;
    for _ in 0..3 {
        r.array(4)?;
    }
    r.take(4)?;
    for _ in 0..r.count(12)? {
        r.take(4)?;
        r.boolean()?;
        r.array(4)?;
    }
    r.array(4)?;
    for _ in 0..r.count(4)? {
        r.array(8)?;
    }
    r.array(4)?;
    r.take(20)?;
    Ok(())
}

fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    let end = bytes.len() - 12;
    let mut trailer = Reader::new(&bytes[end..]);
    ensure!(
        trailer.u32()? == binary::checksum(&bytes[..end]),
        "Save checksum does not match; the file may be damaged"
    );
    ensure!(trailer.u32()? == 1, "Unsupported LE1 compression");
    let total = trailer.u32()? as usize;
    ensure!(
        total > 0 && total <= MAX_FILE,
        "Invalid LE1 decompressed size"
    );
    let mut r = Reader::new(&bytes[..end]);
    ensure!(r.u32()? == 0x9e2a83c1, "Invalid LE1 save header");
    let block = r.u32()? as usize;
    ensure!(
        (1..=1024 * 1024).contains(&block),
        "Invalid LE1 compression block size"
    );
    let compressed = r.u32()? as usize;
    ensure!(r.u32()? as usize == total, "Conflicting LE1 size headers");
    let mut chunks = Vec::new();
    let mut sum = 0usize;
    loop {
        ensure!(chunks.len() <= MAX_FILE / 4, "Too many LE1 blocks");
        let c = r.u32()? as usize;
        let u = r.u32()? as usize;
        ensure!(u <= block && c > 0 && c <= MAX_FILE, "Invalid LE1 block");
        sum = sum.checked_add(u).context("LE1 size overflow")?;
        ensure!(sum <= total, "LE1 blocks exceed declared size");
        chunks.push((c, u));
        if u < block {
            break;
        }
    }
    ensure!(
        sum == total && chunks.iter().map(|(c, _)| c).sum::<usize>() == compressed,
        "Invalid LE1 block totals"
    );
    let mut out = Vec::with_capacity(total);
    for (c, u) in chunks {
        let compressed = r.take(c)?;
        let mut decoder = ZlibDecoder::new(compressed);
        let start = out.len();
        (&mut decoder).take(u as u64 + 1).read_to_end(&mut out)?;
        ensure!(
            out.len() - start == u && decoder.total_in() == c as u64,
            "Invalid LE1 compressed block"
        );
    }
    ensure!(r.position == end, "Unexpected LE1 trailing bytes");
    Ok(out)
}

fn compress(payload: &[u8]) -> Result<Vec<u8>> {
    let block = 128 * 1024;
    let mut chunks = Vec::new();
    for part in payload
        .chunks(block)
        .chain((payload.len().is_multiple_of(block)).then_some(&[][..]))
    {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(part)?;
        chunks.push((encoder.finish()?, part.len()));
    }
    let mut out = Vec::new();
    for value in [
        0x9e2a83c1,
        block as u32,
        chunks.iter().map(|(c, _)| c.len()).sum::<usize>() as u32,
        payload.len() as u32,
    ] {
        binary::word(&mut out, value);
    }
    for (c, u) in &chunks {
        binary::word(&mut out, c.len() as u32);
        binary::word(&mut out, *u as u32);
    }
    for (c, _) in chunks {
        out.extend(c);
    }
    let crc = binary::checksum(&out);
    binary::word(&mut out, crc);
    binary::word(&mut out, 1);
    binary::word(&mut out, payload.len() as u32);
    Ok(out)
}
