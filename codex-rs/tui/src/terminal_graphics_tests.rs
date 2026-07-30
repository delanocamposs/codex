use codex_terminal_detection::Multiplexer;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn multiplexers_disable_all_automatic_graphics() {
    for (environment, multiplexer, expected_reason) in [
        (
            EnvironmentHints {
                tmux: true,
                ..EnvironmentHints::default()
            },
            None,
            PetImageUnsupportedReason::Tmux,
        ),
        (
            EnvironmentHints::default(),
            Some(Multiplexer::Tmux { version: None }),
            PetImageUnsupportedReason::Tmux,
        ),
        (
            EnvironmentHints {
                zellij: true,
                ..EnvironmentHints::default()
            },
            None,
            PetImageUnsupportedReason::Zellij,
        ),
    ] {
        assert_eq!(
            terminal_graphics_support_for(
                &terminal_info_for_test(
                    TerminalName::Kitty,
                    multiplexer,
                    Some("kitty"),
                    /*version*/ None,
                    /*term*/ None,
                ),
                environment,
                Some(KittyGraphicsTerminal::Kitty {
                    version: (0, 48, 1),
                }),
            ),
            TerminalGraphicsSupport {
                pet_images: PetImageSupport::Unsupported(expected_reason),
                transcript_images: None,
            },
        );
    }
}

#[test]
fn kitty_and_ghostty_need_a_supported_probe_for_transcript_images() {
    for (name, term_program, terminal, expected) in [
        (
            TerminalName::Kitty,
            "kitty",
            KittyGraphicsTerminal::Kitty {
                version: (0, 28, 0),
            },
            true,
        ),
        (
            TerminalName::Kitty,
            "kitty",
            KittyGraphicsTerminal::Kitty {
                version: (0, 27, 1),
            },
            false,
        ),
        (
            TerminalName::Ghostty,
            "ghostty",
            KittyGraphicsTerminal::Ghostty { version: (1, 0, 0) },
            true,
        ),
        (
            TerminalName::Ghostty,
            "ghostty",
            KittyGraphicsTerminal::Ghostty {
                version: (0, 99, 0),
            },
            false,
        ),
    ] {
        let support = terminal_graphics_support_for(
            &terminal_info_for_test(
                name,
                /*multiplexer*/ None,
                Some(term_program),
                /*version*/ None,
                /*term*/ None,
            ),
            EnvironmentHints::default(),
            Some(terminal),
        );
        assert_eq!(
            support.transcript_images().is_some(),
            expected,
            "{terminal:?}",
        );
    }
}

#[test]
fn iterm2_support_tracks_kitty_features_by_version() {
    for (version, pet_protocol, transcript_images) in [
        ("3.6.0", ImageProtocol::Kitty, None),
        ("3.6.1", ImageProtocol::KittyLocalFile, None),
        (
            "3.6.3",
            ImageProtocol::KittyLocalFile,
            Some(TranscriptImageProtocol::KittyUnicodePlaceholders),
        ),
        (
            "3.6.10",
            ImageProtocol::KittyLocalFile,
            Some(TranscriptImageProtocol::KittyUnicodePlaceholders),
        ),
    ] {
        assert_eq!(
            terminal_graphics_support_for(
                &terminal_info_for_test(
                    TerminalName::Iterm2,
                    /*multiplexer*/ None,
                    Some("iTerm.app"),
                    Some(version),
                    Some("xterm-256color"),
                ),
                EnvironmentHints::default(),
                /*probed_terminal*/ None,
            ),
            TerminalGraphicsSupport {
                pet_images: PetImageSupport::Supported(pet_protocol),
                transcript_images,
            },
        );
    }
}

#[test]
fn old_or_unversioned_iterm2_is_rejected() {
    for version in [Some("3.5.14"), Some("3.6beta"), None] {
        assert_eq!(
            terminal_graphics_support_for(
                &terminal_info_for_test(
                    TerminalName::Iterm2,
                    /*multiplexer*/ None,
                    Some("iTerm.app"),
                    version,
                    Some("xterm-256color"),
                ),
                EnvironmentHints::default(),
                /*probed_terminal*/ None,
            ),
            TerminalGraphicsSupport {
                pet_images: PetImageSupport::Unsupported(PetImageUnsupportedReason::Iterm2TooOld),
                transcript_images: None,
            },
        );
    }
}

#[test]
fn non_placeholder_terminals_remain_pet_only_or_unsupported() {
    for (info, pet_images) in [
        (
            terminal_info_for_test(
                TerminalName::WezTerm,
                /*multiplexer*/ None,
                Some("WezTerm"),
                /*version*/ None,
                /*term*/ None,
            ),
            PetImageSupport::Supported(ImageProtocol::Kitty),
        ),
        (
            terminal_info_for_test(
                TerminalName::Unknown,
                /*multiplexer*/ None,
                /*term_program*/ None,
                /*version*/ None,
                Some("foot"),
            ),
            PetImageSupport::Supported(ImageProtocol::Sixel),
        ),
        (
            terminal_info_for_test(
                TerminalName::Unknown,
                /*multiplexer*/ None,
                /*term_program*/ None,
                /*version*/ None,
                Some("xterm-256color"),
            ),
            PetImageSupport::Unsupported(PetImageUnsupportedReason::Terminal),
        ),
    ] {
        assert_eq!(
            terminal_graphics_support_for(
                &info,
                EnvironmentHints::default(),
                /*probed_terminal*/ None,
            ),
            TerminalGraphicsSupport {
                pet_images,
                transcript_images: None,
            },
        );
    }
}

#[test]
fn environment_hints_share_the_same_pet_policy() {
    for (environment, protocol) in [
        (
            EnvironmentHints {
                kitty: true,
                ..EnvironmentHints::default()
            },
            ImageProtocol::Kitty,
        ),
        (
            EnvironmentHints {
                wezterm: true,
                ..EnvironmentHints::default()
            },
            ImageProtocol::Kitty,
        ),
    ] {
        assert_eq!(
            terminal_graphics_support_for(
                &terminal_info_for_test(
                    TerminalName::Unknown,
                    /*multiplexer*/ None,
                    /*term_program*/ None,
                    /*version*/ None,
                    Some("xterm-256color"),
                ),
                environment,
                /*probed_terminal*/ None,
            )
            .pet_images(),
            PetImageSupport::Supported(protocol),
        );
    }
}

#[test]
fn version_probe_is_limited_to_placeholder_capable_terminal_families() {
    for (name, term_program, term, environment, expected) in [
        (
            TerminalName::Kitty,
            Some("kitty"),
            None,
            EnvironmentHints::default(),
            true,
        ),
        (
            TerminalName::Ghostty,
            Some("ghostty"),
            None,
            EnvironmentHints::default(),
            true,
        ),
        (
            TerminalName::Unknown,
            None,
            Some("xterm-kitty"),
            EnvironmentHints::default(),
            true,
        ),
        (
            TerminalName::WezTerm,
            Some("WezTerm"),
            None,
            EnvironmentHints::default(),
            false,
        ),
        (
            TerminalName::Kitty,
            Some("kitty"),
            None,
            EnvironmentHints {
                tmux: true,
                ..EnvironmentHints::default()
            },
            false,
        ),
    ] {
        assert_eq!(
            kitty_version_probe_needed_for(
                &terminal_info_for_test(
                    name,
                    /*multiplexer*/ None,
                    term_program,
                    /*version*/ None,
                    term,
                ),
                environment,
            ),
            expected,
        );
    }
}

#[test]
fn no_color_disables_transcript_images_without_disabling_pets() {
    assert_eq!(
        terminal_graphics_support_for(
            &terminal_info_for_test(
                TerminalName::Kitty,
                /*multiplexer*/ None,
                Some("kitty"),
                /*version*/ None,
                /*term*/ None,
            ),
            EnvironmentHints {
                no_color: true,
                ..EnvironmentHints::default()
            },
            Some(KittyGraphicsTerminal::Kitty {
                version: (0, 48, 1),
            }),
        ),
        TerminalGraphicsSupport {
            pet_images: PetImageSupport::Supported(ImageProtocol::Kitty),
            transcript_images: None,
        },
    );
}

fn terminal_info_for_test(
    name: TerminalName,
    multiplexer: Option<Multiplexer>,
    term_program: Option<&str>,
    version: Option<&str>,
    term: Option<&str>,
) -> TerminalInfo {
    TerminalInfo {
        name,
        term_program: term_program.map(str::to_string),
        version: version.map(str::to_string),
        term: term.map(str::to_string),
        multiplexer,
    }
}
