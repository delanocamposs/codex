use pretty_assertions::assert_eq;

use super::*;

#[test]
fn startup_version_probe_policy_is_conservative() {
    let kitty_window = StartupVersionProbeEnvironment {
        kitty_window_id: true,
        ..Default::default()
    };
    let kitty_term = StartupVersionProbeEnvironment {
        kitty_term: true,
        ..Default::default()
    };
    let kitty_term_program = StartupVersionProbeEnvironment {
        kitty_term_program: true,
        ..Default::default()
    };
    let ghostty_term = StartupVersionProbeEnvironment {
        ghostty_term: true,
        ..Default::default()
    };
    let ghostty_term_program = StartupVersionProbeEnvironment {
        ghostty_term_program: true,
        ..Default::default()
    };

    assert_eq!(
        [
            startup_version_probe_for(StartupVersionProbeEnvironment::default()),
            startup_version_probe_for(kitty_window),
            startup_version_probe_for(kitty_term),
            startup_version_probe_for(kitty_term_program),
            startup_version_probe_for(ghostty_term),
            startup_version_probe_for(ghostty_term_program),
            startup_version_probe_for(StartupVersionProbeEnvironment {
                no_color: true,
                ..kitty_window
            }),
            startup_version_probe_for(StartupVersionProbeEnvironment {
                multiplexer: true,
                ..kitty_window
            }),
        ],
        [
            StartupVersionProbe::skip(),
            StartupVersionProbe::query(),
            StartupVersionProbe::query(),
            StartupVersionProbe::query(),
            StartupVersionProbe::query(),
            StartupVersionProbe::query(),
            StartupVersionProbe::skip(),
            StartupVersionProbe::skip(),
        ],
    );
}

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

    let skipped = StartupVersionProbe::skip();
    assert_eq!(skipped.query_bytes(), b"");
    assert!(skipped.is_complete());
}

#[test]
fn parses_kitty_xtversion_response() {
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
}

#[test]
fn parses_ghostty_xtversion_response() {
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

#[test]
fn skips_many_malformed_dcs_prefixes_before_a_valid_response() {
    let cases = [
        (
            b"\x1BP>|kitty(".as_slice(),
            b"\x1BP>|kitty(0.48.1)\x1B\\".as_slice(),
            KittyGraphicsTerminal::Kitty {
                version: (0, 48, 1),
            },
        ),
        (
            b"\x1BP>|ghostty ".as_slice(),
            b"\x1BP>|ghostty 1.3.1-arch2\x1B\\".as_slice(),
            KittyGraphicsTerminal::Ghostty { version: (1, 3, 1) },
        ),
    ];

    for (malformed_prefix, valid_response, expected_terminal) in cases {
        let mut response = malformed_prefix.repeat(256);
        response.extend_from_slice(valid_response);

        assert_eq!(parse_terminal(&response), Some(expected_terminal));
    }
}
