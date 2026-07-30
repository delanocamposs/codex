//! Conservative opacity for HTML and CommonMark autolinks.

#[derive(Clone, Copy)]
pub(super) enum HtmlCodeTag {
    Open,
    Close,
}

#[derive(Clone, Copy)]
pub(super) enum HtmlBlock {
    Until(&'static str),
    RawTag(&'static str),
    BlankLine,
}

impl HtmlBlock {
    pub(super) fn closes_on(self, line: &str) -> bool {
        match self {
            Self::Until(terminator) => line.contains(terminator),
            Self::RawTag(tag) => contains_closing_tag(line, tag),
            Self::BlankLine => line.trim().is_empty(),
        }
    }
}

pub(super) fn block_start(source: &str) -> Option<HtmlBlock> {
    let source = source.trim_start();
    if source.starts_with("<!-->") || source.starts_with("<!--->") {
        return None;
    }
    for (prefix, block) in [
        ("<!--", HtmlBlock::Until("-->")),
        ("<?", HtmlBlock::Until("?>")),
        ("<![CDATA[", HtmlBlock::Until("]]>")),
    ] {
        if source.starts_with(prefix) {
            return Some(block);
        }
    }
    if source
        .as_bytes()
        .get(2)
        .is_some_and(|byte| source.starts_with("<!") && byte.is_ascii_uppercase())
    {
        return Some(HtmlBlock::Until(">"));
    }
    for tag in ["script", "pre", "style", "textarea"] {
        if starts_tag(source, tag) {
            return Some(HtmlBlock::RawTag(tag));
        }
    }
    (block_tag(source).is_some() || complete_tag_line(source)).then_some(HtmlBlock::BlankLine)
}

pub(super) fn angle_construct(source: &str) -> Option<(usize, Option<HtmlCodeTag>)> {
    let bytes = source.as_bytes();
    debug_assert_eq!(bytes.first(), Some(&b'<'));

    if let Some(comment) = ["<!-->", "<!--->"]
        .into_iter()
        .find(|comment| source.starts_with(comment))
    {
        return Some((comment.len(), None));
    }
    for (prefix, terminator) in [
        ("<!--", "-->"),
        ("<![CDATA[", "]]>"),
        ("<?", "?>"),
        ("<!", ">"),
    ] {
        if let Some(after_prefix) = source.strip_prefix(prefix) {
            let end = after_prefix
                .find(terminator)
                .map_or(source.len(), |offset| {
                    prefix.len() + offset + terminator.len()
                });
            return Some((end, None));
        }
    }
    uri_autolink_end(bytes)
        .map(|end| (end, None))
        .or_else(|| html_tag(source))
}

fn html_tag(source: &str) -> Option<(usize, Option<HtmlCodeTag>)> {
    let bytes = source.as_bytes();
    let closing = bytes.get(1) == Some(&b'/');
    let name_start = 1 + usize::from(closing);
    if !bytes.get(name_start)?.is_ascii_alphabetic() {
        return None;
    }
    let name_end = name_start
        + bytes[name_start..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'-')
            .count();
    if bytes
        .get(name_end)
        .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(*byte, b'/' | b'>'))
    {
        return None;
    }

    let Some(end) = tag_end(bytes) else {
        return Some((source.len(), None));
    };
    let before_close = source[..end - 1].trim_end().as_bytes();
    let self_closing = before_close.ends_with(b"/") && !before_close.ends_with(b"=/");
    let code_tag = source[name_start..name_end]
        .eq_ignore_ascii_case("code")
        .then_some(if closing {
            HtmlCodeTag::Close
        } else {
            HtmlCodeTag::Open
        });
    Some((end, (!self_closing).then_some(code_tag).flatten()))
}

fn starts_tag(source: &str, tag: &str) -> bool {
    let bytes = source.as_bytes();
    let name_start = 1 + usize::from(bytes.get(1) == Some(&b'/'));
    let name_end = name_start + tag.len();
    source
        .get(name_start..name_end)
        .is_some_and(|name| name.eq_ignore_ascii_case(tag))
        && bytes
            .get(name_end)
            .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(*byte, b'/' | b'>'))
}

fn block_tag(source: &str) -> Option<&str> {
    const TAGS: &str = "
        address article aside base basefont blockquote body caption center col colgroup dd details
        dialog dir div dl dt fieldset figcaption figure footer form frame frameset h1 h2 h3 h4 h5
        h6 head header hr html iframe legend li link main menu menuitem nav noframes ol optgroup
        option p param search section summary table tbody td tfoot th thead title tr track ul
    ";
    TAGS.split_ascii_whitespace()
        .find(|tag| starts_tag(source, tag))
}

fn complete_tag_line(source: &str) -> bool {
    html_tag(source).is_some_and(|(end, _)| {
        source.as_bytes().get(end - 1) == Some(&b'>') && source[end..].trim().is_empty()
    })
}

fn contains_closing_tag(source: &str, tag: &str) -> bool {
    let bytes = source.as_bytes();
    bytes
        .windows(tag.len() + 2)
        .enumerate()
        .any(|(index, window)| {
            window[..2] == *b"</"
                && window[2..].eq_ignore_ascii_case(tag.as_bytes())
                && bytes
                    .get(index + window.len())
                    .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'>')
        })
}

fn tag_end(bytes: &[u8]) -> Option<usize> {
    let mut quote = None;
    for (index, &byte) in bytes.iter().enumerate().skip(1) {
        match quote {
            Some(delimiter) if byte == delimiter => quote = None,
            Some(_) => {}
            None if matches!(byte, b'\'' | b'"') => quote = Some(byte),
            None if byte == b'>' => return Some(index + 1),
            None => {}
        }
    }
    None
}

fn uri_autolink_end(source: &[u8]) -> Option<usize> {
    let tail = &source[1..];
    let end = tail
        .iter()
        .position(|byte| *byte <= b' ' || matches!(*byte, b'<' | b'>'))?;
    let inner = &tail[..end];
    let colon = inner.iter().position(|byte| *byte == b':')?;
    let scheme = &inner[..colon];
    (tail[end] == b'>'
        && (2..=32).contains(&scheme.len())
        && scheme.first().is_some_and(u8::is_ascii_alphabetic)
        && scheme
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'+' | b'.' | b'-')))
    .then_some(end + 2)
}
