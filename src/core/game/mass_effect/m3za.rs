use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use quick_xml::{Reader, events::Event};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Target;
use super::m3m::decompress_padded;
use super::manifest::relative_path;
use super::package::SourceFile;

const MAX_HEADER: usize = 1024 * 1024;
const MAX_XML: usize = 4 * 1024 * 1024;
const MAX_TOTAL: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StringEdit {
    pub(crate) id: i32,
    pub(crate) data: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TlkUpdate {
    pub(crate) name: String,
    pub(crate) package: String,
    pub(crate) export: String,
    pub(crate) option_key: Option<String>,
    pub(crate) sha256: String,
    pub(crate) strings: Vec<StringEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct M3zaPlan {
    pub(crate) source: SourceFile,
    pub(crate) version: u8,
    pub(crate) option_keys: Vec<String>,
    pub(crate) updates: Vec<TlkUpdate>,
}

pub(super) fn read_input(root: &Path, source: &SourceFile, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        source.size > 0 && source.size <= limit as u64,
        "Merge input exceeds its size limit"
    );
    let path = root.join(relative_path(&source.relative)?);
    ensure!(
        path.is_file() && !path.is_symlink(),
        "Merge input must be a regular file"
    );
    let mut data = Vec::new();
    File::open(path)?
        .take(source.size + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 == source.size && format!("{:x}", Sha256::digest(&data)) == source.sha256,
        "Merge source changed after inspection"
    );
    Ok(data)
}

pub(super) fn inspect(root: &Path, source: &SourceFile, target: Target) -> Result<M3zaPlan> {
    ensure!(
        target == Target::Le1,
        "Embedded TLK inspection currently supports LE1 only"
    );
    parse(&read_input(root, source, MAX_TOTAL)?, source.clone())
        .context("Invalid embedded TLK archive")
}

struct Input<'a>(&'a [u8]);

impl<'a> Input<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        ensure!(count <= self.0.len(), "Truncated M3ZA data");
        let (value, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn length(&mut self, limit: usize) -> Result<usize> {
        let value = usize::try_from(i32::from_le_bytes(self.take(4)?.try_into()?))?;
        ensure!(value <= limit, "M3ZA field exceeds its size limit");
        Ok(value)
    }

    fn string(&mut self) -> Result<String> {
        let mut words = Vec::new();
        loop {
            let word = u16::from_le_bytes(self.take(2)?.try_into()?);
            if word == 0 {
                break;
            }
            ensure!(words.len() < 1024, "M3ZA name is too long");
            words.push(word);
        }
        Ok(String::from_utf16(&words)?)
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn parse(data: &[u8], source: SourceFile) -> Result<M3zaPlan> {
    let mut input = Input(data);
    ensure!(
        data.len() <= MAX_TOTAL && input.take(4)? == b"CTMD",
        "Invalid M3ZA magic or size"
    );
    let version = input.byte()?;
    ensure!(matches!(version, 1 | 2), "Unsupported M3ZA version");
    let size = input.length(MAX_HEADER)?;
    let stored = input.length(MAX_HEADER)?;
    // The upstream writer compresses MemoryStream capacity, including zero-filled unused bytes.
    let header = decompress_padded(input.take(stored)?, size, size.max(256))?;
    let mut header = Input(&header);
    let count = header.length(4096)?;
    ensure!(count > 0, "Empty M3ZA file table");
    let mut names = BTreeSet::new();
    let mut entries = Vec::new();
    let mut total = 0;
    for _ in 0..count {
        let name = header.string()?;
        let (stem, extension) = name
            .rsplit_once('.')
            .context("M3ZA entry must be an XML filename")?;
        ensure!(
            extension.eq_ignore_ascii_case("xml"),
            "M3ZA entry must be an XML filename"
        );
        let (package, export) = stem
            .split_once('.')
            .context("M3ZA filename requires a package and export")?;
        ensure!(
            identifier(package)
                && export.split('.').all(identifier)
                && names.insert(name.to_ascii_lowercase()),
            "Invalid or case-colliding M3ZA filename"
        );
        let offset = header.length(MAX_TOTAL)?;
        let size = header.length(MAX_XML)?;
        let stored = header.length(MAX_XML)?;
        total += size;
        ensure!(
            total <= MAX_TOTAL,
            "Expanded M3ZA data exceeds its size limit"
        );
        let option = if version == 2 { header.byte()? } else { 255 };
        entries.push((
            name.clone(),
            format!("{package}.pcc"),
            export.to_owned(),
            offset,
            size,
            stored,
            option,
        ));
    }
    let mut option_keys = Vec::new();
    let mut keys = BTreeSet::new();
    if version == 2 {
        for _ in 0..header.byte()? {
            let key = header.string()?;
            ensure!(
                identifier(&key) && keys.insert(key.to_ascii_lowercase()),
                "Invalid or duplicate M3ZA option key"
            );
            option_keys.push(key);
        }
    }
    ensure!(header.0.is_empty(), "Trailing M3ZA header fields");
    let block_size = input.length(MAX_TOTAL)?;
    let block = input.take(block_size)?;
    ensure!(input.0.is_empty(), "Trailing M3ZA archive data");
    let mut ranges: Vec<_> = entries.iter().map(|entry| (entry.3, entry.5)).collect();
    ranges.sort_unstable();
    let mut end = 0;
    for (offset, size) in ranges {
        ensure!(
            offset == end && size > 0,
            "Overlapping or unaccounted M3ZA data"
        );
        end = end.checked_add(size).context("M3ZA offset overflow")?;
    }
    ensure!(end == block.len(), "M3ZA data block size mismatch");
    let mut updates = Vec::new();
    for (name, package, export, offset, size, stored, option) in entries {
        let option_key = if option == 255 {
            None
        } else {
            Some(
                option_keys
                    .get(usize::from(option))
                    .context("Unknown M3ZA option key index")?
                    .clone(),
            )
        };
        let xml = decompress_padded(
            block
                .get(offset..offset + stored)
                .context("M3ZA block outside archive")?,
            size,
            0,
        )?;
        updates.push(TlkUpdate {
            name,
            package,
            export,
            option_key,
            sha256: format!("{:x}", Sha256::digest(&xml)),
            strings: parse_xml(&xml)?,
        });
    }
    Ok(M3zaPlan {
        source,
        version,
        option_keys,
        updates,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XmlTlk {
    #[serde(rename = "@Name")]
    _name: Option<String>,
    #[serde(rename = "string", default)]
    strings: Vec<XmlString>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XmlString {
    id: i32,
    #[serde(rename = "flags")]
    _flags: Option<i32>,
    data: String,
}

fn parse_xml(data: &[u8]) -> Result<Vec<StringEdit>> {
    ensure!(data.len() <= MAX_XML, "TLK XML exceeds its size limit");
    let text = std::str::from_utf8(data)?.trim_start_matches('\u{feff}');
    let mut reader = Reader::from_str(text);
    reader.config_mut().expand_empty_elements = true;
    let mut depth = 0;
    let mut roots = 0;
    loop {
        match reader.read_event()? {
            Event::Start(event) => {
                if depth == 0 {
                    ensure!(
                        event.name().as_ref() == b"tlkFile",
                        "Unexpected TLK XML root"
                    );
                    roots += 1;
                }
                depth += 1;
                ensure!(depth <= 4, "TLK XML nesting is too deep");
            }
            Event::End(_) => {
                ensure!(depth > 0, "Unexpected TLK XML closing element");
                depth -= 1;
            }
            Event::DocType(_) | Event::PI(_) => bail!("TLK XML directives are unsupported"),
            Event::Eof => break,
            _ => {}
        }
    }
    ensure!(roots == 1 && depth == 0, "Invalid TLK XML document");
    let parsed: XmlTlk = quick_xml::de::from_str(text)?;
    ensure!(parsed.strings.len() <= 16384, "Too many TLK string updates");
    let mut ids = BTreeSet::new();
    parsed
        .strings
        .into_iter()
        .map(|entry| {
            ensure!(
                ids.insert(entry.id) && !entry.data.contains('\0'),
                "Duplicate TLK ID or embedded null"
            );
            Ok(StringEdit {
                id: entry.id,
                data: entry.data,
            })
        })
        .collect()
}

#[cfg(test)]
pub(in crate::core::game::mass_effect) mod tests {
    use std::io::Write;

    use lzma_rust2::{LzmaOptions, LzmaWriter};
    use tempfile::tempdir;

    use super::*;

    fn compressed(data: &[u8]) -> Result<Vec<u8>> {
        let options = LzmaOptions {
            dict_size: 65536,
            ..LzmaOptions::default()
        };
        let mut writer = LzmaWriter::new_no_header(Vec::new(), &options, true)?;
        let mut output = vec![writer.props()];
        output.extend_from_slice(&options.dict_size.to_le_bytes());
        writer.write_all(data)?;
        output.extend_from_slice(&writer.finish()?);
        Ok(output)
    }

    fn string(output: &mut Vec<u8>, value: &str) {
        for word in value.encode_utf16().chain([0]) {
            output.extend_from_slice(&word.to_le_bytes());
        }
    }

    pub(in crate::core::game::mass_effect) fn archive(
        version: u8,
        entries: &[(&str, &str, u8)],
        keys: &[&str],
        padding: u8,
    ) -> Result<Vec<u8>> {
        let mut header = (entries.len() as i32).to_le_bytes().to_vec();
        let mut block = Vec::new();
        for (name, xml, option) in entries {
            string(&mut header, name);
            let stored = compressed(xml.as_bytes())?;
            for size in [block.len(), xml.len(), stored.len()] {
                header.extend_from_slice(&(size as i32).to_le_bytes());
            }
            if version == 2 {
                header.push(*option);
            }
            block.extend(stored);
        }
        if version == 2 {
            header.push(keys.len() as u8);
            for key in keys {
                string(&mut header, key);
            }
        }
        let size = header.len();
        header.extend_from_slice(&[padding; 16]);
        let stored = compressed(&header)?;
        let mut output = b"CTMD".to_vec();
        output.push(version);
        output.extend_from_slice(&(size as i32).to_le_bytes());
        output.extend_from_slice(&(stored.len() as i32).to_le_bytes());
        output.extend(stored);
        output.extend_from_slice(&(block.len() as i32).to_le_bytes());
        output.extend(block);
        Ok(output)
    }

    fn inspect_bytes(data: &[u8]) -> Result<M3zaPlan> {
        parse(
            data,
            SourceFile {
                relative: "input.m3za".into(),
                size: data.len() as u64,
                sha256: format!("{:x}", Sha256::digest(data)),
            },
        )
    }

    // @variants: both
    #[test]
    fn decodes_both_versions_and_preserves_text_and_option_identity() -> Result<()> {
        let xml = "<tlkFile Name=\"tlk\"><string><id>42</id><flags>0</flags><data>  Café &amp; 雪\n </data></string><string><id>43</id><data/></string></tlkFile>";
        for (version, name) in [(1, "Example.Dialog.tlk.xml"), (2, "Example.Dialog.tlk.XML")] {
            let plan = inspect_bytes(&archive(version, &[(name, xml, 0)], &["Selected"], 0)?)?;
            assert_eq!(plan.updates[0].package, "Example.pcc");
            assert_eq!(plan.updates[0].export, "Dialog.tlk");
            assert_eq!(plan.updates[0].strings[0].data, "  Café & 雪\n ");
            assert_eq!(plan.updates[0].strings[1].data, "");
            assert_eq!(
                plan.updates[0].option_key.as_deref(),
                if version == 2 { Some("Selected") } else { None }
            );
            assert_eq!(
                serde_json::from_str::<M3zaPlan>(&serde_json::to_string(&plan)?)?,
                plan
            );
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_unsafe_names_unknown_options_and_nonzero_padding() -> Result<()> {
        for name in [
            "../Example.tlk.xml",
            "~docs~/Example.tlk.xml",
            "Mods/Example.tlk.xml",
            "../system/Example.tlk.xml",
            "Example..tlk.xml",
            "Example.tlk.dll",
            "Example\\tlk.xml",
        ] {
            assert!(
                inspect_bytes(&archive(2, &[(name, "<tlkFile/>", 255)], &[], 0)?).is_err(),
                "{name}"
            );
        }
        assert!(
            inspect_bytes(&archive(
                2,
                &[("Example.tlk.xml", "<tlkFile/>", 0)],
                &[],
                0
            )?)
            .is_err()
        );
        assert!(
            inspect_bytes(&archive(
                2,
                &[("Example.tlk.xml", "<tlkFile/>", 255)],
                &[],
                1
            )?)
            .is_err()
        );
        assert!(
            inspect_bytes(&archive(
                2,
                &[
                    ("Example.tlk.xml", "<tlkFile/>", 255),
                    ("EXAMPLE.tlk.xml", "<tlkFile/>", 255)
                ],
                &[],
                0
            )?)
            .is_err()
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_corrupt_container_lengths_versions_and_trailing_bytes() -> Result<()> {
        let data = archive(2, &[("Example.tlk.xml", "<tlkFile/>", 255)], &[], 0)?;
        for length in [0, 4, 12, data.len() - 1] {
            assert!(inspect_bytes(&data[..length]).is_err());
        }
        let mut changed = data.clone();
        changed.push(0);
        assert!(inspect_bytes(&changed).is_err());
        for version in [0, 3, 255] {
            let mut changed = data.clone();
            changed[4] = version;
            assert!(inspect_bytes(&changed).is_err());
        }
        let mut changed = data;
        changed[5..9].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(inspect_bytes(&changed).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_xml_directives_unknown_fields_and_duplicate_ids() {
        for text in [
            "<!DOCTYPE tlkFile [<!ENTITY x SYSTEM 'file:///secret'>]><tlkFile/>",
            "<?unknown value?><tlkFile/>",
            "<unknown/>",
            "<tlkFile><future/></tlkFile>",
            "<tlkFile><string><id>1</id><data>x</data><extra/></string></tlkFile>",
            "<tlkFile><string><id>1</id><data>a</data></string><string><id>1</id><data>b</data></string></tlkFile>",
            "<tlkFile><string><id>1</id></string></tlkFile>",
            "<tlkFile/><tlkFile/>",
        ] {
            assert!(parse_xml(text.as_bytes()).is_err(), "{text}");
        }
    }

    // @variants: both
    #[test]
    fn verifies_tlk_source_identity_and_rejects_other_games() -> Result<()> {
        let root = tempdir()?;
        let bytes = archive(2, &[("Example.tlk.xml", "<tlkFile/>", 255)], &[], 0)?;
        let source = inspect_bytes(&bytes)?.source;
        std::fs::write(root.path().join(&source.relative), &bytes)?;
        inspect(root.path(), &source, Target::Le1)?;
        for game in [Target::Le2, Target::Le3] {
            assert!(inspect(root.path(), &source, game).is_err());
        }
        std::fs::write(root.path().join(&source.relative), vec![0; bytes.len()])?;
        assert!(inspect(root.path(), &source, Target::Le1).is_err());
        Ok(())
    }
}
