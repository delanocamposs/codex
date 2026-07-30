//! Column-aware indentation helpers for CommonMark code and list prefixes.

pub(super) struct ListMarker {
    pub(super) content_bytes: usize,
    pub(super) indent_columns: usize,
    pub(super) residual_columns: usize,
    pub(super) can_interrupt: bool,
}

pub(super) fn list_marker(source: &str, marker_column: usize) -> Option<ListMarker> {
    let bytes = source.as_bytes();
    if is_thematic_break(source) {
        return None;
    }
    let (marker, can_interrupt) = match bytes {
        [b'-' | b'+' | b'*'] | [b'-' | b'+' | b'*', b' ' | b'\t', ..] => (1, true),
        _ => {
            let digits = bytes
                .iter()
                .take(9)
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            let marker = digits + 1;
            (digits > 0
                && matches!(bytes.get(digits), Some(b'.' | b')'))
                && bytes
                    .get(marker)
                    .is_none_or(|byte| matches!(byte, b' ' | b'\t')))
            .then_some((marker, &source[..digits] == "1"))?
        }
    };
    let whitespace_bytes = bytes[marker..]
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let padding_columns = leading_indent(
        &source[marker..marker + whitespace_bytes],
        marker_column + marker,
    )
    .1;
    let item_is_empty = marker + whitespace_bytes == bytes.len();
    let (indent_columns, residual_columns) = if item_is_empty {
        (marker + 1, 0)
    } else if padding_columns <= 4 {
        (marker + padding_columns, 0)
    } else {
        (marker + 1, padding_columns - 1)
    };
    Some(ListMarker {
        content_bytes: marker + whitespace_bytes,
        indent_columns,
        residual_columns,
        can_interrupt: can_interrupt && !item_is_empty,
    })
}

pub(super) fn is_thematic_break(source: &str) -> bool {
    let bytes = source.as_bytes();
    let Some(marker @ (b'-' | b'*' | b'_')) = bytes.first() else {
        return false;
    };
    bytes
        .iter()
        .all(|byte| byte == marker || matches!(byte, b' ' | b'\t'))
        && bytes.iter().filter(|byte| *byte == marker).count() >= 3
}

pub(in crate::math_source) fn strip_indent(
    source: &str,
    start_column: usize,
    columns: usize,
) -> (&str, usize) {
    let mut bytes = 0;
    let mut column = start_column;
    for byte in source
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
    {
        if column - start_column >= columns {
            break;
        }
        bytes += 1;
        column += if byte == b'\t' { 4 - column % 4 } else { 1 };
    }
    (&source[bytes..], bytes)
}

pub(super) fn leading_indent(source: &str, start_column: usize) -> (usize, usize) {
    let mut bytes = 0;
    let mut column = start_column;
    for byte in source
        .bytes()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
    {
        bytes += 1;
        column += if byte == b'\t' { 4 - column % 4 } else { 1 };
    }
    (bytes, column - start_column)
}

#[derive(Clone, Copy)]
pub(in crate::math_source) struct SourceLine<'a> {
    pub(in crate::math_source) start: usize,
    pub(in crate::math_source) text: &'a str,
    pub(super) next_start: usize,
}

pub(in crate::math_source) fn source_lines(source: &str) -> impl Iterator<Item = SourceLine<'_>> {
    let mut start = 0;
    std::iter::from_fn(move || {
        if start == source.len() {
            return None;
        }
        let line_start = start;
        let end = start
            + source.as_bytes()[start..]
                .iter()
                .position(|byte| matches!(byte, b'\r' | b'\n'))
                .unwrap_or(source.len() - start);
        start = match source.as_bytes().get(end..) {
            Some([b'\r', b'\n', ..]) => end + 2,
            Some([b'\r' | b'\n', ..]) => end + 1,
            Some(_) | None => end,
        };
        Some(SourceLine {
            start: line_start,
            text: &source[line_start..end],
            next_start: start,
        })
    })
}

pub(super) struct LineView<'a> {
    pub(super) content: &'a str,
    pub(super) content_start: usize,
    pub(super) quote_depth: usize,
    pub(super) leading_columns: usize,
    pub(super) list_indent: Option<usize>,
    pub(super) list_residual_columns: Option<usize>,
}

pub(super) fn line_view(
    line: SourceLine<'_>,
    list_context: Option<(usize, usize)>,
    at_block_start: bool,
) -> LineView<'_> {
    let (after_quotes, quote_bytes, quote_depth) = strip_quote_prefixes(line.text, usize::MAX);
    let (leading, columns) = leading_indent(after_quotes, quote_bytes);
    let container_indent = list_context
        .filter(|(depth, _)| *depth == quote_depth)
        .map_or(0, |(_, indent)| indent);
    let mut offset = leading;
    let mut list_indent = None;
    let mut list_residual_columns = None;
    if columns.saturating_sub(container_indent) <= 3
        && let Some(marker) = list_marker(&after_quotes[leading..], quote_bytes + columns)
        && (at_block_start
            || marker.can_interrupt
            || list_context.is_some_and(|(depth, _)| depth == quote_depth))
    {
        offset += marker.content_bytes;
        list_indent = Some(columns + marker.indent_columns);
        list_residual_columns = Some(marker.residual_columns);
    }
    LineView {
        content: &after_quotes[offset..],
        content_start: line.start + quote_bytes + offset,
        quote_depth,
        leading_columns: columns,
        list_indent,
        list_residual_columns,
    }
}

pub(in crate::math_source) fn strip_quote_prefixes(
    mut line: &str,
    limit: usize,
) -> (&str, usize, usize) {
    let original_len = line.len();
    let mut depth = 0;
    while depth < limit {
        let spaces = line.bytes().take_while(|byte| *byte == b' ').count().min(3);
        let Some(rest) = line[spaces..].strip_prefix('>') else {
            break;
        };
        line = rest.strip_prefix(' ').unwrap_or(rest);
        depth += 1;
    }
    (line, original_len - line.len(), depth)
}
