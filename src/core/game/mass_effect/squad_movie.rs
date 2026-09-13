use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use swf::avm1::types::{Action, Value};
use swf::{FillStyle, PlaceObjectAction, ShapeRecord, ShapeStyles, Tag};

const LIMIT: usize = 16 * 1024 * 1024;
const MEMBERS: usize = 12;
const STATES: usize = 4;
const BLOCK: usize = MEMBERS * STATES;
const STATE_NAMES: [&str; STATES] = ["Selected", "Unselected", "Dead", "Silhouette"];

pub(super) struct Slot {
    pub(super) member: usize,
    pub(super) appearance: usize,
    pub(super) available: String,
    pub(super) highlight: String,
}

pub(super) struct Image {
    pub(super) source: String,
    pub(super) destination: String,
}

pub(super) struct Output {
    pub(super) movie: Vec<u8>,
    pub(super) images: Vec<Image>,
}

#[derive(Clone)]
struct Record {
    code: u16,
    data: Vec<u8>,
}

impl Record {
    fn read(mut bytes: &[u8]) -> Result<Vec<Self>> {
        let mut result = Vec::new();
        while !bytes.is_empty() {
            ensure!(result.len() < 100_000, "Too many squad UI records");
            let header = take(&mut bytes, 2)?;
            let header = u16::from_le_bytes([header[0], header[1]]);
            let size = if header & 63 == 63 {
                let size = take(&mut bytes, 4)?;
                u32::from_le_bytes(size.try_into()?) as usize
            } else {
                usize::from(header & 63)
            };
            result.push(Self {
                code: header >> 6,
                data: take(&mut bytes, size)?.to_vec(),
            });
            if header >> 6 == 0 {
                ensure!(size == 0 && bytes.is_empty(), "Invalid squad UI terminator");
            }
        }
        ensure!(
            result.last().is_some_and(|record| record.code == 0),
            "Unterminated squad UI timeline"
        );
        Ok(result)
    }

    fn write(&self, output: &mut Vec<u8>) -> Result<()> {
        ensure!(
            self.code < 1024
                && self.data.len() <= LIMIT
                && output
                    .len()
                    .checked_add(self.data.len() + 6)
                    .is_some_and(|size| size <= LIMIT),
            "Invalid squad UI record"
        );
        output.extend_from_slice(&((self.code << 6) | 63).to_le_bytes());
        output.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        output.extend_from_slice(&self.data);
        ensure!(
            output.len() <= LIMIT,
            "Generated squad UI exceeds its size limit"
        );
        Ok(())
    }

    fn encoded(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.write(&mut bytes)?;
        Ok(bytes)
    }

    fn from_tag(tag: Tag<'_>) -> Result<Self> {
        let mut bytes = Vec::new();
        swf::write_swf(
            &swf::Header::default_with_swf_version(8),
            &[tag],
            &mut bytes,
        )?;
        let offset = header_end(&bytes)?;
        Self::read(&bytes[offset..])?
            .into_iter()
            .next()
            .context("Missing serialized squad UI record")
    }

    fn id(&self) -> Result<u16> {
        let bytes = self
            .data
            .get(..2)
            .context("Truncated squad UI definition")?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn sprite(&self) -> Result<Vec<Self>> {
        ensure!(
            self.code == 39 && self.data.len() >= 4,
            "Expected a squad UI sprite"
        );
        Self::read(&self.data[4..])
    }

    fn replace_sprite(&mut self, records: &[Self], frames: usize) -> Result<()> {
        ensure!(frames <= u16::MAX as usize, "Too many squad UI frames");
        self.data.truncate(2);
        self.data.extend_from_slice(&(frames as u16).to_le_bytes());
        for record in records {
            record.write(&mut self.data)?;
        }
        Ok(())
    }
}

fn take<'a>(bytes: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    ensure!(length <= bytes.len(), "Truncated squad UI data");
    let (value, rest) = bytes.split_at(length);
    *bytes = rest;
    Ok(value)
}

fn header_end(bytes: &[u8]) -> Result<usize> {
    ensure!(
        bytes.len() >= 13 && bytes.len() <= LIMIT,
        "Invalid squad UI size"
    );
    ensure!(
        &bytes[..3] == b"GFX" || &bytes[..3] == b"FWS",
        "Squad UI requires an uncompressed game-owned GFx movie"
    );
    ensure!(
        bytes[3] == 8 && u32::from_le_bytes(bytes[4..8].try_into()?) as usize == bytes.len(),
        "Unsupported squad UI version or size"
    );
    let mut reader = swf::read::Reader::new(&bytes[8..], 8);
    reader.read_rectangle()?;
    let offset = bytes.len() - reader.get_ref().len() + 4;
    ensure!(offset < bytes.len(), "Truncated squad UI header");
    Ok(offset)
}

fn placed(record: &Record) -> Result<Option<(u16, u16, Option<String>)>> {
    if !matches!(record.code, 4 | 26 | 70 | 94) {
        return Ok(None);
    }
    let encoded = record.encoded()?;
    let Tag::PlaceObject(object) = swf::read::Reader::new(&encoded, 8).read_tag()? else {
        bail!("Invalid squad UI placement");
    };
    let id = match object.action {
        PlaceObjectAction::Place(id) | PlaceObjectAction::Replace(id) => id,
        PlaceObjectAction::Modify => return Ok(None),
    };
    Ok(Some((
        object.depth,
        id,
        object
            .name
            .map(|name| String::from_utf8_lossy(name.as_bytes()).into_owned()),
    )))
}

fn replace_placed(record: &mut Record, id: u16) -> Result<()> {
    let encoded = record.encoded()?;
    let Tag::PlaceObject(mut object) = swf::read::Reader::new(&encoded, 8).read_tag()? else {
        bail!("Invalid squad UI placement");
    };
    object.action = match object.action {
        PlaceObjectAction::Place(_) => PlaceObjectAction::Place(id),
        PlaceObjectAction::Replace(_) => PlaceObjectAction::Replace(id),
        PlaceObjectAction::Modify => bail!("Squad UI frame lacks its character definition"),
    };
    *record = Record::from_tag(Tag::PlaceObject(object))?;
    Ok(())
}

fn frame_records(sprite: &Record) -> Result<Vec<Vec<Record>>> {
    let records = sprite.sprite()?;
    let mut frames = Vec::new();
    let mut frame = Vec::new();
    for record in records {
        if record.code == 0 {
            break;
        }
        let end = record.code == 1;
        frame.push(record);
        if end {
            frames.push(std::mem::take(&mut frame));
        }
    }
    ensure!(
        frame.is_empty()
            && frames.len() == usize::from(u16::from_le_bytes(sprite.data[2..4].try_into()?)),
        "Squad UI frame count differs from its timeline"
    );
    Ok(frames)
}

fn character(frame: &[Record]) -> Result<u16> {
    let mut result = None;
    for record in frame {
        if let Some((10, id, _)) = placed(record)? {
            ensure!(
                result.replace(id).is_none(),
                "Ambiguous squad UI character frame"
            );
        }
    }
    result.context("Squad UI frame lacks its character image")
}

fn patch_count(data: &mut [u8], appearances: i32) -> Result<usize> {
    let mut reader = swf::avm1::read::Reader::new(data, 8);
    let mut constants = Vec::new();
    let mut patches = Vec::new();
    while !reader.get_ref().is_empty() {
        let start = data.len() - reader.get_ref().len();
        match reader.read_action()? {
            Action::ConstantPool(pool) => constants = pool.strings,
            Action::Push(mut push) if push.values.len() == 2 => {
                let name = match &push.values[0] {
                    Value::Str(name) => Some(*name),
                    Value::ConstantPool(index) => constants.get(usize::from(*index)).copied(),
                    _ => None,
                };
                if name.is_none_or(|name| name.as_bytes() != b"nAppearances") {
                    continue;
                }
                ensure!(
                    matches!(push.values[1], Value::Int(3..=32)),
                    "Unrecognized squad UI appearance count"
                );
                let end = data.len() - reader.get_ref().len();
                ensure!(
                    matches!(reader.get_ref().first(), Some(0x1d | 0x3c)),
                    "Squad UI appearance count is not an assignment"
                );
                push.values[1] = Value::Int(appearances);
                let mut bytes = Vec::new();
                swf::avm1::write::Writer::new(&mut bytes, 8).write_action(&Action::Push(push))?;
                ensure!(
                    bytes.len() == end - start,
                    "Squad UI count update changed action offsets"
                );
                patches.push((start, bytes));
            }
            _ => (),
        }
    }
    let count = patches.len();
    for (start, bytes) in patches {
        data[start..start + bytes.len()].copy_from_slice(&bytes);
    }
    Ok(count)
}

struct Copies<'a> {
    definitions: &'a BTreeMap<u16, Record>,
    next: u16,
    added: Vec<Record>,
    added_bytes: usize,
    images: Vec<Image>,
}

impl Copies<'_> {
    fn add(&mut self, record: Record) -> Result<()> {
        self.added_bytes = self
            .added_bytes
            .checked_add(record.data.len() + 6)
            .context("Squad UI artwork size overflow")?;
        ensure!(
            self.added_bytes <= LIMIT,
            "Generated squad UI artwork exceeds its size limit"
        );
        self.added.push(record);
        Ok(())
    }

    fn allocate(&mut self) -> Result<u16> {
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .context("Squad UI character identifiers exhausted")?;
        Ok(id)
    }

    fn image(&mut self, old: u16, source: &str) -> Result<u16> {
        let original = self
            .definitions
            .get(&old)
            .context("Squad UI lacks its original portrait texture")?;
        ensure!(
            original.code == 1009 && original.data.len() >= 13,
            "Unsupported squad UI texture definition"
        );
        let name = format!("TeamSelect_I{old:X}.tga");
        ensure!(
            original.data[10] == 0
                && usize::from(original.data[11]) == name.len()
                && original.data[12..] == *format!("{name}\0").as_bytes(),
            "Unrecognized squad UI external image layout"
        );
        let id = self.allocate()?;
        let destination = format!("TeamSelect_I{id:X}");
        let filename = format!("{destination}.tga");
        let mut data = u32::from(id).to_le_bytes().to_vec();
        data.extend_from_slice(&original.data[4..11]);
        data.push(u8::try_from(filename.len())?);
        data.extend_from_slice(filename.as_bytes());
        data.push(0);
        self.add(Record { code: 1009, data })?;
        self.images.push(Image {
            source: source.into(),
            destination,
        });
        Ok(id)
    }

    fn styles(&mut self, styles: &mut ShapeStyles, replacements: &BTreeMap<u16, u16>) -> bool {
        let mut changed = false;
        let replace = |fill: &mut FillStyle| {
            if let FillStyle::Bitmap { id, .. } = fill
                && let Some(new) = replacements.get(id)
            {
                *id = *new;
                true
            } else {
                false
            }
        };
        for fill in &mut styles.fill_styles {
            changed |= replace(fill);
        }
        for line in &mut styles.line_styles {
            let mut fill = line.fill_style().clone();
            if replace(&mut fill) {
                *line = line.clone().with_fill_style(fill);
                changed = true;
            }
        }
        changed
    }

    fn branch(
        &mut self,
        old: u16,
        replacements: &mut BTreeMap<u16, u16>,
        visiting: &mut BTreeSet<u16>,
    ) -> Result<u16> {
        if let Some(id) = replacements.get(&old) {
            return Ok(*id);
        }
        ensure!(
            visiting.len() < 32 && visiting.insert(old),
            "Cyclic or excessively nested squad UI artwork"
        );
        let Some(original) = self.definitions.get(&old) else {
            visiting.remove(&old);
            return Ok(old);
        };
        let mut record = original.clone();
        let changed = match record.code {
            39 => {
                let mut children = record.sprite()?;
                let mut changed = false;
                for child in &mut children {
                    if let Some((_, id, _)) = placed(child)? {
                        let new = self.branch(id, replacements, visiting)?;
                        if new != id {
                            replace_placed(child, new)?;
                            changed = true;
                        }
                    }
                }
                if changed {
                    let frames = usize::from(u16::from_le_bytes(record.data[2..4].try_into()?));
                    record.replace_sprite(&children, frames)?;
                }
                changed
            }
            2 | 22 | 32 | 83 => {
                let encoded = record.encoded()?;
                let Tag::DefineShape(mut shape) = swf::read::Reader::new(&encoded, 8).read_tag()?
                else {
                    bail!("Invalid squad UI shape");
                };
                let mut changed = self.styles(&mut shape.styles, replacements);
                for item in &mut shape.shape {
                    if let ShapeRecord::StyleChange(style) = item
                        && let Some(styles) = &mut style.new_styles
                    {
                        changed |= self.styles(styles, replacements);
                    }
                }
                if changed {
                    record = Record::from_tag(Tag::DefineShape(shape))?;
                }
                changed
            }
            _ => false,
        };
        visiting.remove(&old);
        let id = if changed {
            let id = self.allocate()?;
            record.data[..2].copy_from_slice(&id.to_le_bytes());
            self.add(record)?;
            id
        } else {
            old
        };
        replacements.insert(old, id);
        Ok(id)
    }
}

pub(super) fn extend(bytes: &[u8], slots: &[Slot]) -> Result<Output> {
    ensure!(
        !slots.is_empty() && slots.len() <= MEMBERS * 30,
        "Invalid squad UI outfit count"
    );
    let mut used = BTreeSet::new();
    for slot in slots {
        ensure!(
            slot.member < MEMBERS
                && (2..32).contains(&slot.appearance)
                && used.insert((slot.member, slot.appearance)),
            "Invalid or duplicate squad UI outfit slot"
        );
    }
    let offset = header_end(bytes)?;
    let mut records = Record::read(&bytes[offset..])?;
    let mut definitions = BTreeMap::new();
    let mut movie = None;
    let mut maximum = 0;
    for record in &records {
        if matches!(
            record.code,
            2 | 6
                | 7
                | 10
                | 11
                | 14
                | 20
                | 21
                | 22
                | 32
                | 33
                | 34
                | 35
                | 36
                | 37
                | 39
                | 46
                | 48
                | 60
                | 75
                | 83
                | 84
                | 87
                | 90
                | 91
                | 1009
        ) {
            let id = record.id()?;
            ensure!(
                definitions.insert(id, record.clone()).is_none(),
                "Duplicate squad UI definition"
            );
            maximum = maximum.max(id);
        }
        if record.code == 39 {
            for child in record.sprite()? {
                if let Some((_, id, Some(name))) = placed(&child)?
                    && name == "CharSelect01"
                {
                    ensure!(
                        movie.replace(id).is_none_or(|previous| previous == id),
                        "Ambiguous squad selection movie"
                    );
                }
            }
        }
    }
    let movie = movie.context("Game UI lacks its squad selection timeline")?;
    let sprite = definitions
        .get(&movie)
        .context("Missing squad selection timeline")?;
    let mut frames = frame_records(sprite)?;
    ensure!(
        frames.len() >= 4 * BLOCK && frames.len() <= 32 * BLOCK && frames.len() % BLOCK == 0,
        "Unsupported squad selection timeline layout"
    );
    let template = frames[3 * BLOCK..4 * BLOCK].to_vec();
    let frame_bytes = |frame: &Vec<Record>| {
        frame
            .iter()
            .map(|record| record.data.len() + 6)
            .sum::<usize>()
    };
    let expanded = frames.iter().map(frame_bytes).sum::<usize>()
        + template.iter().map(frame_bytes).sum::<usize>() * (32 - frames.len() / BLOCK);
    ensure!(
        expanded <= LIMIT - 10,
        "Expanded squad UI timeline exceeds its size limit"
    );
    while frames.len() < 32 * BLOCK {
        let appearance = frames.len() / BLOCK + 1;
        let mut added = template.clone();
        for (state, name) in STATE_NAMES.iter().enumerate() {
            let labels: Vec<_> = added[state * MEMBERS]
                .iter_mut()
                .filter(|record| record.code == 43)
                .collect();
            ensure!(
                labels.len() == 1,
                "Squad UI state lacks its navigation label"
            );
            for label in labels {
                ensure!(
                    label.data == format!("{name}4\0").as_bytes(),
                    "Unrecognized squad UI state navigation label"
                );
                label.data = format!("{name}{appearance}\0").into_bytes();
            }
        }
        frames.extend(added);
    }
    let mut copies = Copies {
        definitions: &definitions,
        next: maximum
            .checked_add(1)
            .context("Squad UI identifiers exhausted")?,
        added: Vec::new(),
        added_bytes: 0,
        images: Vec::new(),
    };
    let original_images = [
        0x4c, 0x61, 0x68, 0x6f, 0x78, 0x7f, 0x86, 0x8d, 0x96, 0x9d, 0xa6, 0xad,
    ];
    for slot in slots {
        let available = original_images[slot.member];
        let mut replacements = BTreeMap::from([
            (available, copies.image(available, &slot.available)?),
            (available + 3, copies.image(available + 3, &slot.highlight)?),
        ]);
        let old = character(&frames[slot.member])?;
        let new = copies.branch(old, &mut replacements, &mut BTreeSet::new())?;
        ensure!(
            new != old,
            "Squad UI portrait does not use the expected game textures"
        );
        for state in 0..STATES {
            let frame = &mut frames[slot.appearance * BLOCK + state * MEMBERS + slot.member];
            let mut found = false;
            for record in frame {
                if let Some((10, _, _)) = placed(record)? {
                    replace_placed(record, new)?;
                    found = true;
                }
            }
            ensure!(
                found,
                "Squad UI appearance frame lacks its character placement"
            );
        }
    }
    let mut timeline: Vec<_> = frames.into_iter().flatten().collect();
    timeline.push(Record {
        code: 0,
        data: Vec::new(),
    });
    let mut patched = sprite.clone();
    patched.replace_sprite(&timeline, 32 * BLOCK)?;
    let mut count = 0;
    let mut output = bytes[..offset].to_vec();
    for record in &mut records {
        if record.code == 12 {
            count += patch_count(&mut record.data, 32)?;
        }
        if record.code == 39 && record.id()? == movie {
            for added in &copies.added {
                added.write(&mut output)?;
            }
            patched.write(&mut output)?;
        } else {
            record.write(&mut output)?;
        }
    }
    ensure!(
        count == 1,
        "Game UI lacks an unambiguous appearance count assignment"
    );
    let size = u32::try_from(output.len())?;
    output[4..8].copy_from_slice(&size.to_le_bytes());
    header_end(&output)?;
    Ok(Output {
        movie: output,
        images: copies.images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncated_movies_and_duplicate_slots() {
        let slot = || Slot {
            member: 0,
            appearance: 3,
            available: "A".into(),
            highlight: "H".into(),
        };
        assert!(extend(b"GFX", &[slot()]).is_err());
        assert!(extend(b"GFX", &[slot(), slot()]).is_err());
        assert!(Record::read(&[0xff, 0xff, 0xff]).is_err());
        assert!(Record::read(&[0, 0, 0, 0]).is_err());
    }

    #[test]
    fn updates_only_the_appearance_assignment() -> Result<()> {
        use swf::avm1::types::{ConstantPool, Push};
        let mut bytes = Vec::new();
        let mut writer = swf::avm1::write::Writer::new(&mut bytes, 8);
        writer.write_action(&Action::ConstantPool(ConstantPool {
            strings: vec![swf::SwfStr::from_utf8_str("nAppearances")],
        }))?;
        writer.write_action(&Action::Push(Push {
            values: vec![Value::ConstantPool(0), Value::Int(9)],
        }))?;
        writer.write_action(&Action::DefineLocal)?;
        let length = bytes.len();
        assert_eq!(patch_count(&mut bytes, 32)?, 1);
        assert_eq!(bytes.len(), length);
        let mut reader = swf::avm1::read::Reader::new(&bytes, 8);
        reader.read_action()?;
        assert!(
            matches!(reader.read_action()?, Action::Push(push) if push.values == vec![Value::ConstantPool(0), Value::Int(32)])
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires the supplied game-owned squad UI corpus"]
    fn extends_game_owned_squad_images_to_thirty_two_slots() -> Result<()> {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".ci-artifacts/mele-analysis/ui.swf");
        let bytes = std::fs::read(&source)?;
        let output = extend(
            &bytes,
            &[
                Slot {
                    member: 0,
                    appearance: 3,
                    available: "Images.Available".into(),
                    highlight: "Images.Highlight".into(),
                },
                Slot {
                    member: 6,
                    appearance: 31,
                    available: "Other.Available".into(),
                    highlight: "Other.Highlight".into(),
                },
            ],
        )?;
        assert_eq!(std::fs::read(source)?, bytes);
        assert_eq!(output.images.len(), 4);
        assert_eq!(output.images[0].source, "Images.Available");
        assert!(output.images[0].destination.starts_with("TeamSelect_I"));
        let original = Record::read(&bytes[header_end(&bytes)?..])?;
        let extended = Record::read(&output.movie[header_end(&output.movie)?..])?;
        for tag in &original {
            if tag.code != 12 && !(tag.code == 39 && tag.id()? == 315) {
                assert!(
                    extended
                        .iter()
                        .any(|other| other.code == tag.code && other.data == tag.data)
                );
            }
        }
        let timeline = extended
            .iter()
            .find(|record| record.code == 39 && record.id().ok() == Some(315))
            .context("Missing extended timeline")?;
        let frames = frame_records(timeline)?;
        assert_eq!(frames.len(), 32 * BLOCK);
        let labels: Vec<_> = frames
            .iter()
            .flatten()
            .filter(|record| record.code == 43)
            .map(|record| record.data.clone())
            .collect();
        assert_eq!(labels.iter().collect::<BTreeSet<_>>().len(), 32 * STATES);
        for (state, name) in STATE_NAMES.iter().enumerate() {
            assert!(
                frames[31 * BLOCK + state * MEMBERS]
                    .iter()
                    .any(|record| record.code == 43
                        && record.data == format!("{name}32\0").as_bytes())
            );
        }
        let first = character(&frames[3 * BLOCK])?;
        let last = character(&frames[31 * BLOCK + 6])?;
        assert_ne!(first, character(&frames[0])?);
        assert_ne!(last, character(&frames[6])?);
        for state in 0..STATES {
            assert_eq!(character(&frames[3 * BLOCK + state * MEMBERS])?, first);
            assert_eq!(character(&frames[31 * BLOCK + state * MEMBERS + 6])?, last);
        }
        Ok(())
    }
}
