use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};

// M3TO is read by Json.NET upstream; keep its comments and trailing commas out of
// serde_json without changing quoted values or weakening the helper protocol.
pub(super) fn normalize(text: &str) -> Result<String> {
    let mut bytes = text.trim_start_matches('\u{feff}').as_bytes().to_vec();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => index = string_end(&bytes, index)?,
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                while index < bytes.len() && !matches!(bytes[index], b'\r' | b'\n') {
                    bytes[index] = b' ';
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let start = index;
                index += 2;
                while index + 1 < bytes.len() && bytes[index..index + 2] != *b"*/" {
                    index += 1;
                }
                ensure!(index + 1 < bytes.len(), "Unterminated M3TO comment");
                index += 2;
                for byte in &mut bytes[start..index] {
                    if !matches!(*byte, b'\r' | b'\n') {
                        *byte = b' ';
                    }
                }
            }
            _ => index += 1,
        }
    }
    let mut output = String::with_capacity(bytes.len());
    let mut index = 0;
    let mut previous = None;
    let mut objects = Vec::<BTreeSet<String>>::new();
    let mut depth = 0_u32;
    while index < bytes.len() {
        let start = index;
        let byte = bytes[index];
        if byte == b'"' {
            index = string_end(&bytes, index)?;
            let token = std::str::from_utf8(&bytes[start..index])?;
            if bytes[index..].iter().find(|ch| !ch.is_ascii_whitespace()) == Some(&b':') {
                let key: String = serde_json::from_str(token)?;
                let key = key.to_ascii_lowercase();
                ensure!(
                    objects
                        .last_mut()
                        .context("M3TO property outside object")?
                        .insert(key.clone()),
                    "Duplicate M3TO property '{key}'"
                );
                output.push_str(&serde_json::to_string(&key)?);
            } else {
                output.push_str(token);
            }
            previous = Some(b'"');
            continue;
        }
        match byte {
            b'{' | b'[' => {
                depth += 1;
                ensure!(depth <= 8, "M3TO nesting exceeds its limit");
                if byte == b'{' {
                    objects.push(BTreeSet::new());
                }
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1).context("Unmatched M3TO delimiter")?;
                if byte == b'}' {
                    objects.pop();
                }
            }
            _ => {}
        }
        if byte == b','
            && !matches!(previous, None | Some(b'{' | b'[' | b',' | b':'))
            && matches!(
                bytes[index + 1..]
                    .iter()
                    .find(|ch| !ch.is_ascii_whitespace()),
                Some(b'}' | b']')
            )
        {
            output.push(' ');
        } else {
            // Non-ASCII text is valid only inside JSON strings, handled above.
            ensure!(byte.is_ascii(), "Invalid unquoted M3TO character");
            output.push(char::from(byte));
        }
        if !byte.is_ascii_whitespace() {
            previous = Some(byte);
        }
        index += 1;
    }
    Ok(output)
}

fn string_end(bytes: &[u8], start: usize) -> Result<usize> {
    let mut index = start + 1;
    while let Some(byte) = bytes.get(index) {
        match byte {
            b'"' => return Ok(index + 1),
            b'\\' => index += 2,
            _ => index += 1,
        }
    }
    None.context("Unterminated M3TO string")
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn preserves_strings_and_accepts_comments_trailing_commas_and_property_case() -> Result<()> {
        let text = "\u{feff}{/* ü */\"VaLue\":\"é \\\" // /* ,} ,] \\\\\",// comment\n\"Array\":[true,null,],}";
        let value: serde_json::Value = serde_json::from_str(&normalize(text)?)?;
        assert_eq!(value["value"], "é \" // /* ,} ,] \\");
        assert_eq!(value["array"], serde_json::json!([true, null]));
        Ok(())
    }

    #[test]
    fn does_not_repair_invalid_json() {
        for text in [
            "{,}",
            "[,]",
            "[1,,]",
            "[1 2,]",
            "[tru,]",
            "/* open",
            "{\"x\":\"open}",
            "[1]garbage",
            "[1/* gap */2]",
        ] {
            assert!(
                normalize(text)
                    .and_then(|text| Ok(serde_json::from_str::<serde_json::Value>(&text)?))
                    .is_err(),
                "{text}"
            );
        }
    }
}
