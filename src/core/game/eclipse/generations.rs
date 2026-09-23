use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use anyhow::{Context, Result, ensure};
use quick_xml::{Reader, XmlVersion, events::Event};

struct Entry {
    uid: String,
    range: Range<usize>,
}

fn entries(xml: &str) -> Result<Vec<Entry>> {
    let offset = xml.len() - xml.trim_start_matches('\u{feff}').len();
    let mut reader = Reader::from_str(&xml[offset..]);
    let mut depth = 0_usize;
    let mut active = None;
    let mut entries = Vec::new();
    loop {
        let start = offset + usize::try_from(reader.buffer_position())?;
        match reader.read_event()? {
            Event::Start(element) | Event::Empty(element)
                if matches!(element.name().as_ref(), b"AddInItem" | b"AddIn")
                    && active.is_none() =>
            {
                let uid = element
                    .attributes()
                    .map(|attribute| -> Result<_> {
                        let attribute = attribute?;
                        Ok(if attribute.key.as_ref() == b"UID" {
                            Some(
                                attribute
                                    .normalized_value(XmlVersion::Implicit1_0)?
                                    .into_owned(),
                            )
                        } else {
                            None
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .next()
                    .context("Add-in registration is missing its UID")?;
                ensure!(!uid.is_empty(), "Add-in registration has an empty UID");
                let end = offset + usize::try_from(reader.buffer_position())?;
                if xml[..end].ends_with("/>") {
                    entries.push(Entry {
                        uid,
                        range: start..end,
                    });
                } else {
                    active = Some((uid, start, depth));
                    depth += 1;
                }
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth = depth.checked_sub(1).context("Unbalanced add-in XML")?;
                if active
                    .as_ref()
                    .is_some_and(|(_, _, starting)| *starting == depth)
                {
                    let (uid, start, _) = active.take().context("Missing add-in XML entry")?;
                    entries.push(Entry {
                        uid,
                        range: start..offset + usize::try_from(reader.buffer_position())?,
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    ensure!(depth == 0 && active.is_none(), "Incomplete add-in XML");
    let mut seen = BTreeSet::new();
    ensure!(
        entries.iter().all(|entry| seen.insert(&entry.uid)),
        "Duplicate add-in registration UID"
    );
    Ok(entries)
}

pub(crate) fn registrations(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let xml = std::str::from_utf8(bytes).context("Add-in XML must be UTF-8")?;
    let entries = entries(xml)?;
    ensure!(!entries.is_empty(), "Add-in manifest has no registration");
    Ok(entries
        .into_iter()
        .map(|entry| (entry.uid, xml[entry.range].to_owned()))
        .collect())
}

pub(crate) fn render(
    current: Option<&str>,
    previous: &BTreeSet<String>,
    desired: &BTreeMap<String, String>,
) -> Result<String> {
    let current = current
        .unwrap_or("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<AddInsList>\n</AddInsList>\n");
    let mut output = current.to_owned();
    let mut pending = desired.clone();
    for entry in entries(current)?.into_iter().rev() {
        if desired
            .get(&entry.uid)
            .is_some_and(|block| block == &current[entry.range.clone()])
        {
            pending.remove(&entry.uid);
        } else if previous.contains(&entry.uid) || desired.contains_key(&entry.uid) {
            output.replace_range(entry.range, "");
        }
    }
    let offset = output.len() - output.trim_start_matches('\u{feff}').len();
    let mut reader = Reader::from_str(&output[offset..]);
    let mut depth = 0_usize;
    let mut insertion = None;
    let mut empty = None;
    loop {
        let start = offset + usize::try_from(reader.buffer_position())?;
        match reader.read_event()? {
            Event::Start(element) => {
                if depth == 0 {
                    ensure!(
                        element.name().as_ref() == b"AddInsList",
                        "AddIns.xml has an unsupported root"
                    );
                }
                depth += 1;
            }
            Event::Empty(element) if depth == 0 => {
                ensure!(
                    element.name().as_ref() == b"AddInsList" && empty.is_none(),
                    "AddIns.xml has an unsupported root"
                );
                empty = Some(start..offset + usize::try_from(reader.buffer_position())?);
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).context("Unbalanced AddIns.xml")?;
                if depth == 0 {
                    ensure!(
                        insertion.replace(start).is_none(),
                        "AddIns.xml contains multiple roots"
                    );
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    ensure!(depth == 0, "Incomplete AddIns.xml");
    let inner = pending
        .values()
        .map(|block| format!("  {block}\n"))
        .collect::<String>();
    if let Some(range) = empty {
        ensure!(insertion.is_none(), "AddIns.xml contains multiple roots");
        let opening = output[range.clone()].trim_end_matches("/>");
        output.replace_range(range, &format!("{opening}>\n{inner}</AddInsList>"));
    } else {
        let insertion = insertion.context("AddIns.xml has no AddInsList root")?;
        output.insert_str(insertion, &inner);
    }
    entries(&output)
        .context("Generated AddIns.xml is invalid; existing registrations were preserved")?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn removes_only_previous_managed_registrations_and_preserves_unrelated_xml() -> Result<()> {
        let xml = "<AddInsList><!--keep--><AddInItem UID=\"old\"><Name>Old</Name></AddInItem><AddInItem UID=\"user\" Enabled=\"0\"/></AddInsList>";
        let desired = registrations(
            br#"<Manifest><AddInItem UID="new&amp;id"><Name>New</Name></AddInItem></Manifest>"#,
        )?;
        let rendered = render(Some(xml), &BTreeSet::from(["old".into()]), &desired)?;
        assert!(!rendered.contains("UID=\"old\""));
        assert!(rendered.contains("<!--keep--><AddInItem UID=\"user\" Enabled=\"0\"/>"));
        assert!(rendered.contains("UID=\"new&amp;id\""));
        assert_eq!(
            render(Some(&rendered), &BTreeSet::new(), &desired)?,
            rendered
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn preserves_bom_and_dlc_when_registering_mods() -> Result<()> {
        let xml = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<AddInsList>\r\n  <AddInItem UID=\"official\"><Name>Édition</Name></AddInItem>\r\n</AddInsList>";
        let desired = registrations(
            b"\xef\xbb\xbf<Manifest><AddInItem UID=\"mod\"><Name>Mod</Name></AddInItem></Manifest>",
        )?;
        assert_eq!(
            desired["mod"],
            "<AddInItem UID=\"mod\"><Name>Mod</Name></AddInItem>"
        );
        let rendered = render(Some(xml), &BTreeSet::new(), &desired)?;
        assert!(rendered.starts_with('\u{feff}'));
        let actual = registrations(rendered.as_bytes())?;
        assert_eq!(actual.len(), 2);
        assert_eq!(
            actual["official"],
            "<AddInItem UID=\"official\"><Name>Édition</Name></AddInItem>"
        );
        assert_eq!(actual["mod"], desired["mod"]);
        assert_eq!(
            render(Some(&rendered), &BTreeSet::new(), &desired)?,
            rendered
        );
        let removed = render(
            Some(&rendered),
            &BTreeSet::from(["mod".into()]),
            &BTreeMap::new(),
        )?;
        assert_eq!(registrations(removed.as_bytes())?.len(), 1);
        Ok(())
    }

    // @variants: both
    #[test]
    fn expands_bom_prefixed_empty_lists_and_rejects_invalid_generated_entries() -> Result<()> {
        let desired = registrations(b"<Manifest><AddInItem UID=\"mod\"/></Manifest>")?;
        let rendered = render(Some("\u{feff}<AddInsList/>"), &BTreeSet::new(), &desired)?;
        assert_eq!(registrations(rendered.as_bytes())?, desired);
        let invalid = BTreeMap::from([("mod".into(), "<AddInItem UID=\"mod\">".into())]);
        assert!(render(None, &BTreeSet::new(), &invalid).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_duplicate_and_malformed_registrations() {
        assert!(
            registrations(br#"<AddInsList><AddIn UID="same"/><AddIn UID="same"/></AddInsList>"#)
                .is_err()
        );
        assert!(registrations(br#"<AddIn UID="open">"#).is_err());
        assert!(render(Some("<Other/>"), &BTreeSet::new(), &BTreeMap::new()).is_err());
    }
}
