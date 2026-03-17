use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

/// Route a file's relative path for Eclipse (Dragon Age: Origins) deployment.
///
/// DAZIP mods are already expanded into `AddIns/<UID>/` by the installer, so their
/// paths start with `AddIns/` and are left unchanged. Loose override files that
/// don't carry a recognised DA-layout prefix are routed into
/// `packages/core/override/` where the game's override scanner will pick them up.
pub fn route_path(rel: &str) -> String {
    let lower = rel.to_lowercase();
    if lower.starts_with("addins/")
        || lower.starts_with("packages/")
        || lower.starts_with("settings/")
    {
        rel.to_string()
    } else {
        format!("packages/core/override/{rel}")
    }
}

/// Regenerate `Settings/AddIns.xml` from every `AddIns/*/manifest.xml` present
/// under `da_dir` (the Dragon Age user-data root).
///
/// Existing entries in the file that are not managed by deployd (e.g. game-
/// generated campaign and DLC entries) are preserved. Called after every deploy
/// so the game's add-in registry stays in sync with whichever mods are enabled.
pub fn write_addins_xml(da_dir: &Path) -> Result<()> {
    let addins_dir = da_dir.join("AddIns");
    let settings_dir = da_dir.join("Settings");
    fs::create_dir_all(&settings_dir)
        .with_context(|| format!("Cannot create Settings dir: {}", settings_dir.display()))?;

    // Collect managed entries from AddIns/*/manifest.xml (sorted by UID).
    let mut managed: BTreeMap<String, String> = BTreeMap::new();
    if addins_dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&addins_dir)
            .with_context(|| format!("Cannot read AddIns dir: {}", addins_dir.display()))?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let manifest_path = entry.path().join("manifest.xml");
            if !manifest_path.is_file() {
                continue;
            }
            let data = match fs::read(&manifest_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!(
                        "[deployd] WARNING: cannot read {}: {e}",
                        manifest_path.display()
                    );
                    continue;
                }
            };
            if let Some((uid, block)) = extract_addin_block_with_uid(&data) {
                managed.insert(uid, block);
            }
        }
    }

    // Read existing AddIns.xml; keep entries whose UIDs we do not manage
    // (game-generated campaigns, DLC, etc.).
    let addins_xml = settings_dir.join("AddIns.xml");
    // Remove stale lowercase variant written by older deployd versions.
    let _ = fs::remove_file(settings_dir.join("Addins.xml"));
    let mut preserved: Vec<String> = Vec::new();
    if addins_xml.is_file() {
        if let Ok(data) = fs::read(&addins_xml) {
            for (uid, block) in extract_all_addin_blocks_with_uids(&data) {
                if !managed.contains_key(&uid) {
                    preserved.push(block);
                }
            }
        }
    }

    let all_blocks: Vec<&str> = preserved
        .iter()
        .map(String::as_str)
        .chain(managed.values().map(String::as_str))
        .collect();

    let content = if all_blocks.is_empty() {
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<AddInsList/>\n".to_string()
    } else {
        let inner = all_blocks.join("\n  ");
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<AddInsList>\n  {inner}\n</AddInsList>\n"
        )
    };

    fs::write(&addins_xml, &content)
        .with_context(|| format!("Cannot write Addins.xml: {}", addins_xml.display()))?;

    Ok(())
}

/// Extract the first `<AddIn*>` block from a manifest.xml, returning `(uid, xml_block)`.
/// Handles both `<AddIn>` (legacy) and `<AddInItem>` (standard) elements.
/// Returns `None` if no block with a UID attribute is found.
fn extract_addin_block_with_uid(data: &[u8]) -> Option<(String, String)> {
    let src = std::str::from_utf8(data).ok()?;
    let mut reader = Reader::from_str(src);
    reader.config_mut().trim_text(false);

    let mut buf = Vec::new();
    let mut depth: u32 = 0;
    let mut capturing = false;
    let mut block = String::new();
    let mut uid = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref().starts_with(b"AddIn") && !capturing {
                    capturing = true;
                    depth = 1;
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"UID" {
                            if let Ok(val) = attr.unescape_value() {
                                uid = val.into_owned();
                            }
                        }
                    }
                    let tag = std::str::from_utf8(e.as_ref()).unwrap_or("");
                    block.push('<');
                    block.push_str(tag);
                    block.push('>');
                } else if capturing {
                    depth += 1;
                    let tag = std::str::from_utf8(e.as_ref()).unwrap_or("");
                    block.push('<');
                    block.push_str(tag);
                    block.push('>');
                }
            }
            Ok(Event::End(ref e)) => {
                if capturing {
                    depth -= 1;
                    let name_bytes = e.name().as_ref().to_vec();
                    let name = String::from_utf8_lossy(&name_bytes);
                    block.push_str("</");
                    block.push_str(&name);
                    block.push('>');
                    if depth == 0 {
                        if uid.is_empty() {
                            return None;
                        }
                        return Some((uid, block));
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                if capturing {
                    let tag = std::str::from_utf8(e.as_ref()).unwrap_or("");
                    block.push('<');
                    block.push_str(tag);
                    block.push_str("/>");
                }
            }
            Ok(Event::Text(ref e)) => {
                if capturing {
                    block.push_str(e.unescape().unwrap_or_default().as_ref());
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    None
}

/// Extract all `<AddIn*>` blocks from an `AddIns.xml` file, returning `(uid, xml_block)` pairs.
fn extract_all_addin_blocks_with_uids(data: &[u8]) -> Vec<(String, String)> {
    let src = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut reader = Reader::from_str(src);
    reader.config_mut().trim_text(false);

    let mut buf = Vec::new();
    let mut depth: u32 = 0;
    let mut capturing = false;
    let mut block = String::new();
    let mut uid = String::new();
    let mut results = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.name().as_ref().starts_with(b"AddIn") && !capturing {
                    capturing = true;
                    depth = 1;
                    uid.clear();
                    block.clear();
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"UID" {
                            if let Ok(val) = attr.unescape_value() {
                                uid = val.into_owned();
                            }
                        }
                    }
                    let tag = std::str::from_utf8(e.as_ref()).unwrap_or("");
                    block.push('<');
                    block.push_str(tag);
                    block.push('>');
                } else if capturing {
                    depth += 1;
                    let tag = std::str::from_utf8(e.as_ref()).unwrap_or("");
                    block.push('<');
                    block.push_str(tag);
                    block.push('>');
                }
            }
            Ok(Event::End(ref e)) => {
                if capturing {
                    depth -= 1;
                    let name_bytes = e.name().as_ref().to_vec();
                    let name = String::from_utf8_lossy(&name_bytes);
                    block.push_str("</");
                    block.push_str(&name);
                    block.push('>');
                    if depth == 0 {
                        if !uid.is_empty() {
                            results.push((uid.clone(), block.clone()));
                        }
                        capturing = false;
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                if e.name().as_ref().starts_with(b"AddIn") && !capturing {
                    let mut entry_uid = String::new();
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"UID" {
                            if let Ok(val) = attr.unescape_value() {
                                entry_uid = val.into_owned();
                            }
                        }
                    }
                    if !entry_uid.is_empty() {
                        let tag = std::str::from_utf8(e.as_ref()).unwrap_or("");
                        results.push((entry_uid, format!("<{tag}/>")));
                    }
                } else if capturing {
                    let tag = std::str::from_utf8(e.as_ref()).unwrap_or("");
                    block.push('<');
                    block.push_str(tag);
                    block.push_str("/>");
                }
            }
            Ok(Event::Text(ref e)) => {
                if capturing {
                    block.push_str(e.unescape().unwrap_or_default().as_ref());
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    results
}
