use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};

use super::super::Target;
use super::super::package::SourceFile;

fn open(root: &Path, source: &SourceFile) -> Result<File> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join(&source.relative))?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1 && metadata.len() == source.size,
        "M3TO input changed or is not an independent regular file"
    );
    Ok(file)
}

fn bytes<const N: usize>(file: &mut File) -> Result<[u8; N]> {
    let mut value = [0; N];
    file.read_exact(&mut value)
        .context("Truncated BTP structure")?;
    Ok(value)
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn u64_at(data: &[u8], offset: usize) -> u64 {
    u64::from(u32_at(data, offset)) | (u64::from(u32_at(data, offset + 4)) << 32)
}

fn name(data: &[u8]) -> Result<String> {
    let words: Vec<_> = data
        .chunks_exact(2)
        .map(|part| u16::from_le_bytes([part[0], part[1]]))
        .collect();
    let end = words
        .iter()
        .position(|word| *word == 0)
        .context("Unterminated BTP name")?;
    ensure!(
        words[end..].iter().all(|word| *word == 0),
        "Nonzero BTP name padding"
    );
    String::from_utf16(&words[..end]).context("Invalid UTF-16 BTP name")
}

fn target_hash(target: Target, dlc: &str) -> u32 {
    let game = match target {
        Target::Le1 => "LE1",
        Target::Le2 => "LE2",
        Target::Le3 => "LE3",
    };
    format!("{game}{dlc}")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .fold(0x811c9dc5_u32, |hash, byte| {
            hash.wrapping_mul(0x01000193) ^ u32::from(byte)
        })
}

pub(super) fn inspect(
    root: &Path,
    package: &SourceFile,
    metadata: &SourceFile,
    dlc: &str,
    target: Target,
    resolve_tfc: impl Fn(&str) -> Result<SourceFile>,
    control: &super::super::operation::Control,
) -> Result<BTreeSet<String>> {
    control.check()?;
    ensure!(
        metadata.size >= 8 && metadata.size <= 16 * 1024 * 1024,
        "Invalid BTP metadata size"
    );
    let mut btm = Vec::new();
    open(root, metadata)?
        .take(metadata.size + 1)
        .read_to_end(&mut btm)?;
    ensure!(
        btm.len() as u64 == metadata.size
            && format!("{:x}", Sha256::digest(&btm)) == metadata.sha256,
        "BTP metadata changed during inspection"
    );
    let version = match target {
        Target::Le1 => (684, 171),
        Target::Le2 => (684, 168),
        Target::Le3 => (685, 205),
    };
    ensure!(
        btm[..4] == [0xc1, 0x83, 0x2a, 0x9e] && u32_at(&btm, 4) == (version.0 | version.1 << 16),
        "BTP metadata is not a package for the selected LE game"
    );
    let mut file = open(root, package)?;
    ensure!(
        file.metadata()?.len() == package.size && package.size >= 48,
        "BTP size changed or header is truncated"
    );
    let header = bytes::<48>(&mut file)?;
    ensure!(
        &header[..6] == b"LETEXM" && u16::from_le_bytes([header[6], header[7]]) == 2,
        "Only precompiled BTP version 2 is supported"
    );
    ensure!(
        u32_at(&header, 8) == target_hash(target, dlc),
        "BTP targets a different game or DLC folder"
    );
    ensure!(
        u32_at(&header, 28) == crc32fast::hash(&btm),
        "BTPMetadata.btm does not match this BTP"
    );
    ensure!(
        header[32..].iter().all(|byte| *byte == 0),
        "Unsupported BTP header flags"
    );
    let count = u32_at(&header, 12);
    let tfc_count = u32_at(&header, 16);
    let tfc_offset = u64_at(&header, 20);
    let data_start = 48 + u64::from(count) * 836;
    ensure!(
        (1..=100_000).contains(&count)
            && (1..=1024).contains(&tfc_count)
            && tfc_offset >= data_start
            && tfc_offset.checked_add(u64::from(tfc_count) * 144) == Some(package.size),
        "Invalid BTP texture or TFC table boundaries"
    );
    file.seek(SeekFrom::Start(tfc_offset))?;
    let mut tfcs = Vec::new();
    let mut tfc_names = BTreeSet::new();
    for index in 0..tfc_count {
        control.check()?;
        let record = bytes::<144>(&mut file)?;
        let name = name(&record[..128])?;
        ensure!(
            !name.is_empty()
                && name.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
                && tfc_names.insert(name.to_lowercase()),
            "Invalid or duplicate BTP TFC name"
        );
        if index == 0 {
            ensure!(
                name == "None" && record[128..].iter().all(|byte| *byte == 0),
                "BTP requires an empty first TFC slot"
            );
            tfcs.push(0);
        } else {
            let input = resolve_tfc(&name)?;
            let mut tfc = open(root, &input)?;
            ensure!(
                tfc.metadata()?.len() == input.size
                    && input.size >= 16
                    && bytes::<16>(&mut tfc)? == record[128..],
                "BTP TFC '{name}' is missing its matching GUID or changed size"
            );
            tfcs.push(input.size);
        }
    }
    file.seek(SeekFrom::Start(48))?;
    let mut names = BTreeSet::new();
    for _ in 0..count {
        control.check()?;
        let record = bytes::<836>(&mut file)?;
        let texture = name(&record[..512])?;
        ensure!(
            texture.contains('.')
                && texture.split('.').all(|part| !part.is_empty()
                    && part.chars().all(|ch| ch.is_alphanumeric() || ch == '_'))
                && names.insert(texture.to_lowercase()),
            "Invalid or duplicate BTP texture path"
        );
        let tfc = u32_at(&record, 512) as usize;
        let format = u32_at(&record, 516);
        let mips = usize::from(record[523]);
        ensure!(
            tfc < tfcs.len()
                && (1..54).contains(&format)
                && record[520] <= 1
                && record[522] <= 1
                && (1..=13).contains(&mips),
            "Invalid BTP texture metadata"
        );
        for index in 0..13 {
            let mip = &record[524 + index * 24..548 + index * 24];
            if index >= mips {
                ensure!(mip.iter().all(|byte| *byte == 0), "Nonempty unused BTP mip");
                continue;
            }
            let size = u32_at(mip, 0);
            let stored = u32_at(mip, 4);
            let offset = u64_at(mip, 8);
            let width = u16::from_le_bytes([mip[16], mip[17]]);
            let height = u16::from_le_bytes([mip[18], mip[19]]);
            let flags = u32_at(mip, 20);
            ensure!(
                size > 0
                    && size <= 256 * 1024 * 1024
                    && stored > 0
                    && stored <= 256 * 1024 * 1024
                    && (1..=4096).contains(&width)
                    && (1..=4096).contains(&height)
                    && matches!(flags, 0 | 4 | 8 | 12),
                "Unsupported BTP mip size, dimensions, or flags"
            );
            let (start, end) = if flags & 4 != 0 {
                ensure!(
                    tfc != 0 && offset < i32::MAX as u64,
                    "External BTP mip has no TFC or exceeds runtime offsets"
                );
                (16, tfcs[tfc])
            } else {
                (data_start, tfc_offset)
            };
            ensure!(
                offset >= start
                    && offset
                        .checked_add(u64::from(stored))
                        .is_some_and(|limit| limit <= end),
                "BTP mip points outside its declared payload"
            );
            ensure!(
                flags != 0 || stored >= size,
                "Raw BTP mip is shorter than its declared size"
            );
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests;
