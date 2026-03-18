use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
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

/// Update `Settings/AddIns.xml` with entries for every installed DAZIP mod found
/// under `AddIns/*/manifest.xml` in `da_dir`.
///
/// Existing file content is never replaced — missing entries are inserted before
/// `</AddInsList>` so game-generated entries (campaigns, DLC) are always preserved.
/// Only writes the file if something was actually added.
pub fn write_addins_xml(da_dir: &Path) -> Result<()> {
    let addins_dir = da_dir.join("AddIns");
    let settings_dir = da_dir.join("Settings");
    let addins_xml = settings_dir.join("AddIns.xml");

    // Remove stale wrong-case file written by older deployd versions.
    let _ = fs::remove_file(settings_dir.join("Addins.xml"));

    // Collect entries for currently installed DAZIP mods (sorted by UID).
    let managed = collect_managed_entries(&addins_dir);
    if managed.is_empty() {
        return Ok(());
    }

    if !addins_xml.is_file() {
        // Game hasn't created the file yet — create a minimal one.
        fs::create_dir_all(&settings_dir)?;
        let inner: String = managed
            .values()
            .map(|b| format!("  {b}\n"))
            .collect();
        let content = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<AddInsList>\n{inner}</AddInsList>\n"
        );
        fs::write(&addins_xml, content)?;
        return Ok(());
    }

    // File exists: only insert entries whose UID is not already registered.
    let raw = match fs::read_to_string(&addins_xml) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[deployd] WARNING: cannot read AddIns.xml: {e}");
            return Ok(());
        }
    };

    let mut content = raw.clone();
    let mut changed = false;

    for (uid, block) in &managed {
        if uid_present_in(&content, uid) {
            continue;
        }
        // Expand self-closing <AddInsList/> if needed.
        if let Some(pos) = content.find("<AddInsList/>") {
            let replacement = format!("<AddInsList>\n  {block}\n</AddInsList>");
            content.replace_range(pos..pos + "<AddInsList/>".len(), &replacement);
            changed = true;
        } else if let Some(pos) = content.rfind("</AddInsList>") {
            let prefix = if content[..pos].ends_with('\n') { "" } else { "\n" };
            content.insert_str(pos, &format!("{prefix}  {block}\n"));
            changed = true;
        }
    }

    if changed {
        fs::write(&addins_xml, &content)?;
    }

    Ok(())
}

fn uid_present_in(content: &str, uid: &str) -> bool {
    content.contains(&format!("UID=\"{uid}\"")) || content.contains(&format!("UID='{uid}'"))
}

fn collect_managed_entries(addins_dir: &Path) -> BTreeMap<String, String> {
    let mut managed = BTreeMap::new();
    if !addins_dir.is_dir() {
        return managed;
    }
    let mut entries: Vec<_> = fs::read_dir(addins_dir)
        .into_iter()
        .flatten()
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
    managed
}

/// Extract the first `<AddInItem>` (standard) or `<AddIn>` (legacy) block from a
/// manifest.xml, returning `(uid, xml_block)`.
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
                let name = e.name();
                let name_bytes = name.as_ref();
                let is_addin_item = name_bytes == b"AddInItem" || name_bytes == b"AddIn";
                if is_addin_item && !capturing {
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
            Ok(Event::CData(ref e)) => {
                if capturing {
                    let content = std::str::from_utf8(e.as_ref()).unwrap_or("");
                    block.push_str("<![CDATA[");
                    block.push_str(content);
                    block.push_str("]]>");
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    None
}
