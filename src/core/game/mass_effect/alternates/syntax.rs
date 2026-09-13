use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

pub(super) fn parse(text: &str) -> Result<Vec<BTreeMap<String, String>>> {
    let text = text
        .trim()
        .strip_prefix('(')
        .and_then(|text| text.strip_suffix(')'))
        .context("Alternate list must be parenthesized")?;
    let mut remaining = text.trim();
    let mut result = Vec::new();
    while !remaining.is_empty() {
        ensure!(result.len() < 1024, "Too many alternates");
        remaining = remaining
            .strip_prefix('(')
            .context("Expected alternate object")?;
        let mut fields = BTreeMap::new();
        loop {
            let (key, rest) = remaining
                .split_once('=')
                .context("Expected alternate field=value")?;
            let key = key.trim().to_ascii_lowercase();
            ensure!(
                !key.is_empty() && key.bytes().all(|ch| ch.is_ascii_alphanumeric()),
                "Invalid alternate field name"
            );
            remaining = rest.trim_start();
            let value;
            if let Some(quoted) = remaining.strip_prefix('"') {
                let end = quoted.find('"').context("Unterminated alternate string")?;
                value = quoted[..end].to_string();
                remaining = quoted[end + 1..].trim_start();
            } else {
                let end = value_end(remaining)?;
                value = remaining[..end].trim().to_string();
                ensure!(!value.contains('('), "Invalid alternate value");
                remaining = &remaining[end..];
            }
            ensure!(
                fields.insert(key, value).is_none(),
                "Duplicate alternate field"
            );
            if let Some(rest) = remaining.strip_prefix(')') {
                remaining = rest.trim_start();
                break;
            }
            remaining = remaining
                .strip_prefix(',')
                .context("Expected alternate field separator")?
                .trim_start();
        }
        result.push(fields);
        if remaining.is_empty() {
            break;
        }
        remaining = remaining
            .strip_prefix(',')
            .context("Expected alternate separator")?
            .trim_start();
        ensure!(!remaining.is_empty(), "Trailing alternate separator");
    }
    Ok(result)
}

fn value_end(text: &str) -> Result<usize> {
    let mut depth = 0_u32;
    let mut quoted = false;
    for (index, ch) in text.char_indices() {
        match ch {
            '"' if depth > 0 || quoted => quoted = !quoted,
            '[' if !quoted => {
                depth += 1;
                ensure!(depth <= 8, "Nested manifest values are too deep");
            }
            ']' if !quoted => depth = depth.checked_sub(1).context("Unmatched manifest bracket")?,
            ',' | ')' if depth == 0 && !quoted => return Ok(index),
            '"' => anyhow::bail!("Invalid alternate quotation"),
            _ => {}
        }
    }
    anyhow::bail!("Unterminated alternate object")
}

pub(in crate::core::game::mass_effect) fn split(text: &str, separator: char) -> Result<Vec<&str>> {
    let mut depth = 0_u32;
    let mut quoted = false;
    let mut start = 0;
    let mut result = Vec::new();
    for (index, ch) in text.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '[' if !quoted => {
                depth += 1;
                ensure!(depth <= 8, "Nested manifest values are too deep");
            }
            ']' if !quoted => depth = depth.checked_sub(1).context("Unmatched manifest bracket")?,
            ch if ch == separator && depth == 0 && !quoted => {
                result.push(text[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    ensure!(depth == 0 && !quoted, "Unterminated manifest value");
    result.push(text[start..].trim());
    ensure!(
        result.len() <= 1024 && result.iter().all(|value| !value.is_empty()),
        "Empty or excessive manifest values"
    );
    Ok(result)
}
