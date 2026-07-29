use pretty_assertions::assert_eq;

use super::*;

#[test]
fn query_state_waits_for_a_complete_version_response() {
    let mut probe = StartupVersionProbe::query();
    assert_eq!(probe.query_bytes(), XTVERSION_QUERY);
    assert!(!probe.is_complete());

    let mut buffer = b"\x1BP>|kitty(0.".to_vec();
    probe.observe(&buffer);
    assert_eq!(probe.terminal(), None);
    assert!(!probe.is_complete());

    buffer.extend_from_slice(b"48.1)\x1B\\");
    probe.observe(&buffer);
    assert_eq!(
        probe.terminal(),
        Some(KittyGraphicsTerminal::Kitty {
            version: (0, 48, 1)
        })
    );
    assert!(probe.is_complete());

    let mut skipped = StartupVersionProbe::skip();
    assert_eq!(skipped.query_bytes(), b"");
    assert!(skipped.is_complete());
    skipped.observe(b"\x1BP>|kitty(0.48.1)\x1B\\");
    assert_eq!(skipped.terminal(), None);
}

#[test]
fn parses_supported_xtversion_responses() {
    assert_eq!(
        parse_terminal(b"\x1BP>|kitty(0.48.1)\x1B\\"),
        Some(KittyGraphicsTerminal::Kitty {
            version: (0, 48, 1)
        })
    );
    assert_eq!(
        parse_terminal(b"prefix\x1BP>|kitty(0.28)\x1B\\suffix"),
        Some(KittyGraphicsTerminal::Kitty {
            version: (0, 28, 0)
        })
    );
    assert_eq!(parse_terminal(b"\x1BP>|kitty(not-a-version)\x1B\\"), None);
    assert_eq!(
        parse_terminal(b"\x1BP>|kitty()\x1B\\noise\x1BP>|kitty(0.48.1)\x1B\\"),
        Some(KittyGraphicsTerminal::Kitty {
            version: (0, 48, 1)
        })
    );
    assert_eq!(
        parse_terminal(b"\x1BP>|ghostty 1.3.1-arch2\x1B\\"),
        Some(KittyGraphicsTerminal::Ghostty { version: (1, 3, 1) })
    );
    assert_eq!(
        parse_terminal(b"prefix\x1BP>|ghostty 1.2.3+release\x1B\\suffix"),
        Some(KittyGraphicsTerminal::Ghostty { version: (1, 2, 3) })
    );
    assert_eq!(parse_terminal(b"\x1BP>|ghostty not-a-version\x1B\\"), None);
}
