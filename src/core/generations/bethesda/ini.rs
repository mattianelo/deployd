use anyhow::{Context, Result, ensure};

const SETTINGS: [(&[u8], &[u8]); 2] = [
    (b"bInvalidateOlderFiles", b"1"),
    (b"sResourceDataDirsFinal", b""),
];

pub(super) fn render(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let little = bytes[0] == 0xff;
        ensure!(
            bytes.len().is_multiple_of(2),
            "Managed INI contains incomplete UTF-16 text"
        );
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| {
                if little {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect::<Vec<_>>();
        let decoded =
            String::from_utf16(&units).context("Managed INI contains invalid UTF-16 text")?;
        let merged = merge(decoded.as_bytes())?;
        let text =
            std::str::from_utf8(&merged).context("Managed INI encoding could not be preserved")?;
        let mut encoded = bytes[..2].to_vec();
        for unit in text.encode_utf16() {
            encoded.extend_from_slice(&if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        return Ok(encoded);
    }
    if let Some(content) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        let mut output = bytes[..3].to_vec();
        output.extend(merge(content)?);
        return Ok(output);
    }
    merge(bytes)
}

fn merge(bytes: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        !bytes.contains(&0),
        "Managed INI contains unsupported binary text; it was preserved"
    );
    let newline = if bytes.windows(2).any(|pair| pair == b"\r\n") {
        b"\r\n".as_slice()
    } else {
        b"\n".as_slice()
    };
    let mut output = Vec::new();
    let mut archive = false;
    let mut archive_end = None;
    let mut seen = [false; 2];
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        ensure!(
            !body.contains(&b'\r'),
            "Managed INI has unsupported line endings; it was preserved"
        );
        let trimmed = body.trim_ascii();
        if let Some(section) = trimmed.strip_prefix(b"[") {
            let closing = section
                .iter()
                .position(|byte| *byte == b']')
                .context("Managed INI has an incomplete section header")?;
            let suffix = section[closing + 1..].trim_ascii();
            ensure!(
                suffix.is_empty() || suffix.starts_with(b";") || suffix.starts_with(b"#"),
                "Managed INI has an ambiguous section header"
            );
            if archive {
                archive_end = Some(output.len());
            }
            archive = section[..closing]
                .trim_ascii()
                .eq_ignore_ascii_case(b"Archive");
        }
        let setting = if archive
            && !trimmed.starts_with(b";")
            && !trimmed.starts_with(b"#")
            && let Some(equals) = body.iter().position(|byte| *byte == b'=')
        {
            SETTINGS
                .iter()
                .position(|(key, _)| body[..equals].trim_ascii().eq_ignore_ascii_case(key))
                .map(|index| (equals, index))
        } else {
            None
        };
        if let Some((equals, index)) = setting {
            seen[index] = true;
            output.extend_from_slice(&body[..=equals]);
            let value = &body[equals + 1..];
            let leading = value
                .iter()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count();
            let mut tail = value
                .iter()
                .position(|byte| matches!(byte, b';' | b'#'))
                .unwrap_or(value.len());
            while tail > leading && matches!(value[tail - 1], b' ' | b'\t') {
                tail -= 1;
            }
            output.extend_from_slice(&value[..leading]);
            output.extend_from_slice(SETTINGS[index].1);
            output.extend_from_slice(&value[tail..]);
            output.extend_from_slice(&line[body.len()..]);
        } else {
            output.extend_from_slice(line);
        }
    }
    if seen.iter().all(|seen| *seen) {
        return Ok(output);
    }
    if archive {
        archive_end = Some(output.len());
    }
    let insertion = archive_end.unwrap_or(output.len());
    let mut missing = Vec::new();
    if insertion > 0 && output[insertion - 1] != b'\n' {
        missing.extend_from_slice(newline);
    }
    if archive_end.is_none() {
        missing.extend_from_slice(b"[Archive]");
        missing.extend_from_slice(newline);
    }
    for (index, (key, value)) in SETTINGS.iter().enumerate() {
        if !seen[index] {
            missing.extend_from_slice(key);
            missing.push(b'=');
            missing.extend_from_slice(value);
            missing.extend_from_slice(newline);
        }
    }
    output.splice(insertion..insertion, missing);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_unrelated_settings_comments_and_crlf_lines() -> Result<()> {
        let input = b"; personal settings\r\n[Display]\r\nbInvalidateOlderFiles=0\r\n[Archive]\r\n bInvalidateOlderFiles = 0 ; annotation\r\nsResourceDataDirsFinal=STRINGS\\\r\nsArchiveList=custom.bsa\r\n";
        let expected = b"; personal settings\r\n[Display]\r\nbInvalidateOlderFiles=0\r\n[Archive]\r\n bInvalidateOlderFiles = 1 ; annotation\r\nsResourceDataDirsFinal=\r\nsArchiveList=custom.bsa\r\n";
        assert_eq!(render(input)?, expected);
        assert_eq!(render(expected)?, expected);
        Ok(())
    }

    #[test]
    fn inserts_missing_keys_inside_archive_without_moving_other_sections() -> Result<()> {
        assert_eq!(render(b"[Archive]\nCustom=keep\n[Display]\nWidth=1920")?,
            b"[Archive]\nCustom=keep\nbInvalidateOlderFiles=1\nsResourceDataDirsFinal=\n[Display]\nWidth=1920");
        assert_eq!(
            render(b"[Display]\nWidth=1920")?,
            b"[Display]\nWidth=1920\n[Archive]\nbInvalidateOlderFiles=1\nsResourceDataDirsFinal=\n"
        );
        Ok(())
    }

    #[test]
    fn handles_duplicate_case_insensitive_keys_and_sections_consistently() -> Result<()> {
        assert_eq!(render(b"[archive]\nBINVALIDATEOLDERFILES=0\n[Display]\nOther=yes\n[Archive]\nbInvalidateOlderFiles=0\nsResourceDataDirsFinal=old\n")?,
            b"[archive]\nBINVALIDATEOLDERFILES=1\n[Display]\nOther=yes\n[Archive]\nbInvalidateOlderFiles=1\nsResourceDataDirsFinal=\n");
        Ok(())
    }

    #[test]
    fn preserves_byte_order_marks_utf16_and_non_utf8_comments() -> Result<()> {
        let text = "[Display]\nLabel=é\n";
        for little in [true, false] {
            let mut bytes = if little {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&if little {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            let output = render(&bytes)?;
            assert!(output.starts_with(&bytes));
            assert_eq!(render(&output)?, output);
        }
        for bytes in [b"\xef\xbb\xbf; comment\n".as_slice(), b"; comment \xe9\n"] {
            assert!(render(bytes)?.starts_with(bytes));
        }
        Ok(())
    }

    #[test]
    fn rejects_incomplete_or_ambiguous_text_without_guessing_sections() {
        for bytes in [
            b"[Archive".as_slice(),
            b"[Archive] trailing",
            b"binary\0text",
            &[0xff, 0xfe, 0x42],
            &[0xff, 0xfe, 0x00, 0xd8],
        ] {
            assert!(render(bytes).is_err());
        }
    }
}
