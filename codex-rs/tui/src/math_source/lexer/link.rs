//! Conservative opacity for Markdown link metadata.

/// Returns the end of an inline destination and optional title beginning at `(`.
pub(super) fn inline_target_end(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    (bytes.first() == Some(&b'(')).then_some(())?;
    let mut cursor = 1;
    let mut depth = 1;
    let mut quote = None;
    let mut angle = false;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' if cursor + 1 < bytes.len() => cursor += 2,
            byte if quote == Some(byte) => {
                quote = None;
                cursor += 1;
            }
            _ if quote.is_some() => cursor += 1,
            b'\'' | b'"' if !angle => {
                quote = Some(bytes[cursor]);
                cursor += 1;
            }
            b'<' if !angle => {
                angle = true;
                cursor += 1;
            }
            b'>' if angle => {
                angle = false;
                cursor += 1;
            }
            b'(' if !angle => {
                depth += 1;
                cursor += 1;
            }
            b')' if !angle => {
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            b'\r' | b'\n' => {
                cursor = consume_line_ending(bytes, cursor);
                let end = line_end(bytes, cursor);
                if bytes[cursor..end].iter().all(u8::is_ascii_whitespace) {
                    return None;
                }
            }
            _ => cursor += 1,
        }
    }
    None
}

/// Returns the end of an explicit reference identifier beginning at `[`.
pub(super) fn reference_id_end(source: &str) -> Option<usize> {
    reference_label_end(source.as_bytes(), /*allow_empty*/ true)
}

/// Returns the conservative end of a link reference definition beginning at `[`.
pub(super) fn reference_definition_end(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let label_end = reference_label_end(bytes, /*allow_empty*/ false)?;
    (bytes.get(label_end) == Some(&b':')).then_some(())?;

    let first_end = line_end(bytes, label_end + 1);
    let mut end = first_end;
    let mut has_destination = bytes[label_end + 1..first_end]
        .iter()
        .any(|byte| !byte.is_ascii_whitespace());
    let mut next = consume_optional_line_ending(bytes, first_end);

    if !has_destination && next > first_end && !line_is_blank(bytes, next) {
        end = line_end(bytes, next);
        has_destination = true;
        next = consume_optional_line_ending(bytes, end);
    }
    if has_destination && next > end && title_line(bytes, next) {
        end = line_end(bytes, next);
    }
    Some(end)
}

fn reference_label_end(bytes: &[u8], allow_empty: bool) -> Option<usize> {
    (bytes.first() == Some(&b'[')).then_some(())?;
    let mut cursor = 1;
    let mut content = false;
    let mut line_breaks = 0;
    while cursor < bytes.len() && cursor <= 1_000 {
        match bytes[cursor] {
            b'\\' if cursor + 1 < bytes.len() => {
                content = true;
                cursor += 2;
            }
            b']' => return (allow_empty || content).then_some(cursor + 1),
            b'[' => return None,
            b'\r' | b'\n' => {
                cursor = consume_line_ending(bytes, cursor);
                line_breaks += 1;
                if line_breaks > 1 {
                    return None;
                }
            }
            byte => {
                content |= !byte.is_ascii_whitespace();
                cursor += 1;
            }
        }
    }
    None
}

fn title_line(bytes: &[u8], start: usize) -> bool {
    let indent = bytes[start..]
        .iter()
        .take_while(|byte| **byte == b' ')
        .count();
    indent <= 3 && matches!(bytes.get(start + indent), Some(b'"' | b'\'' | b'('))
}

fn line_is_blank(bytes: &[u8], start: usize) -> bool {
    let end = line_end(bytes, start);
    bytes[start..end].iter().all(u8::is_ascii_whitespace)
}

fn consume_optional_line_ending(bytes: &[u8], cursor: usize) -> usize {
    matches!(bytes.get(cursor), Some(b'\r' | b'\n'))
        .then(|| consume_line_ending(bytes, cursor))
        .unwrap_or(cursor)
}

fn consume_line_ending(bytes: &[u8], cursor: usize) -> usize {
    cursor
        + usize::from(bytes.get(cursor) == Some(&b'\r') && bytes.get(cursor + 1) == Some(&b'\n'))
        + 1
}

fn line_end(bytes: &[u8], cursor: usize) -> usize {
    cursor
        + bytes[cursor..]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
            .unwrap_or(bytes.len() - cursor)
}
