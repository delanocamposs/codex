use std::io;
use std::io::Write;
use std::path::PathBuf;

use base64::Engine as _;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn kitty_png_transmission_encodes_inline_data() {
    let command = kitty_png_command(
        b"png",
        /*columns*/ 4,
        /*rows*/ 3,
        /*image_id*/ None,
        KittyPlacement::Direct,
    );

    assert_eq!(command, "\x1b_Ga=T,t=d,f=100,c=4,r=3,q=2,m=0;cG5n\x1b\\");
}

#[test]
fn kitty_png_transmission_chunks_large_payloads() {
    let command = kitty_png_command(
        &vec![0x5a; 4096],
        /*columns*/ 1,
        /*rows*/ 1,
        /*image_id*/ Some(7),
        KittyPlacement::Direct,
    );

    assert!(command.starts_with("\x1b_Ga=T,t=d,f=100,c=1,r=1,q=2,i=7,m=1;"));
    assert_eq!(command.matches("\x1b_Gm=").count(), 1);
    assert!(command.contains("\x1b_Gm=0;"));
    assert!(command.ends_with("\x1b\\"));
}

#[test]
fn kitty_file_png_transmission_encodes_local_file_reference() {
    let path = PathBuf::from("frame.png");
    let payload = general_purpose::STANDARD.encode(path.to_string_lossy().as_bytes());

    assert_eq!(
        kitty_png_file_command(
            &path,
            /*columns*/ 4,
            /*rows*/ 3,
            /*image_id*/ Some(7),
        ),
        format!("\x1b_Ga=T,t=f,f=100,c=4,r=3,q=2,i=7;{payload}\x1b\\")
    );
}

#[test]
fn tmux_passthrough_wraps_and_escapes_control_sequence() {
    assert_eq!(
        wrap_for_tmux("\x1b_Gx;\x1b\\"),
        "\x1bPtmux;\x1b\x1b_Gx;\x1b\x1b\\\x1b\\"
    );
}

#[test]
fn unicode_placeholders_require_a_supported_probe_without_a_multiplexer() {
    let cases = [
        (
            KittyCapabilities {
                ansi_color_metadata: true,
                multiplexer: false,
                queried_terminal: Some(KittyGraphicsTerminal::Kitty {
                    version: (0, 28, 0),
                }),
            },
            true,
        ),
        (
            KittyCapabilities {
                ansi_color_metadata: true,
                multiplexer: false,
                queried_terminal: Some(KittyGraphicsTerminal::Kitty {
                    version: (0, 27, 1),
                }),
            },
            false,
        ),
        (
            KittyCapabilities {
                ansi_color_metadata: true,
                multiplexer: false,
                queried_terminal: Some(KittyGraphicsTerminal::Ghostty { version: (1, 3, 1) }),
            },
            true,
        ),
        (
            KittyCapabilities {
                ansi_color_metadata: true,
                multiplexer: false,
                queried_terminal: Some(KittyGraphicsTerminal::Ghostty {
                    version: (0, 99, 0),
                }),
            },
            false,
        ),
        (KittyCapabilities::default(), false),
        (
            KittyCapabilities {
                ansi_color_metadata: true,
                multiplexer: true,
                queried_terminal: Some(KittyGraphicsTerminal::Kitty {
                    version: (0, 48, 1),
                }),
            },
            false,
        ),
        (
            KittyCapabilities {
                ansi_color_metadata: false,
                multiplexer: false,
                queried_terminal: Some(KittyGraphicsTerminal::Kitty {
                    version: (0, 48, 1),
                }),
            },
            false,
        ),
    ];

    for (capabilities, expected) in cases {
        assert_eq!(
            kitty_unicode_placeholders_supported_for_terminal(capabilities),
            expected,
        );
    }
}

#[test]
fn image_data_is_transmitted_once_and_each_geometry_gets_one_placement() {
    let png = Arc::<[u8]>::from(b"png".as_slice());
    let image = KittyImage::new(
        Arc::clone(&png),
        /*image_id*/ 42,
        /*columns*/ 4,
        /*rows*/ 2,
    );
    let resized = KittyImage::new(
        Arc::clone(&png),
        /*image_id*/ 42,
        /*columns*/ 2,
        /*rows*/ 1,
    );
    let other = KittyImage::new(
        b"other".to_vec(),
        /*image_id*/ 43,
        /*columns*/ 2,
        /*rows*/ 1,
    );
    let mut output = Vec::new();
    let mut registry = KittyImageRegistry::default();

    registry
        .write_definitions(&mut output, [&image, &resized, &other])
        .expect("write definitions");
    registry
        .write_definitions(&mut output, [&image, &resized])
        .expect("reuse definitions");
    let output = String::from_utf8(output).expect("Kitty command is UTF-8");

    assert_eq!(
        output,
        [
            kitty_png_command(
                &image.png,
                image.columns(),
                image.rows(),
                Some(image.image_id()),
                KittyPlacement::UnicodePlaceholder {
                    placement_id: image.placement_id(),
                },
            ),
            kitty_unicode_placeholder_placement(
                resized.image_id(),
                resized.placement_id(),
                resized.columns(),
                resized.rows(),
            ),
            kitty_png_command(
                &other.png,
                other.columns(),
                other.rows(),
                Some(other.image_id()),
                KittyPlacement::UnicodePlaceholder {
                    placement_id: other.placement_id(),
                },
            ),
        ]
        .concat(),
    );
    assert_eq!(
        (
            image.placement_id(),
            resized.placement_id(),
            other.placement_id(),
        ),
        (516, 258, 258),
    );

    let mut deletions = Vec::new();
    registry
        .delete_all(&mut deletions)
        .expect("delete definitions");
    registry
        .delete_all(&mut deletions)
        .expect("repeat deletion is empty");
    let deletions = String::from_utf8(deletions).expect("Kitty command is UTF-8");
    assert_eq!(deletions.matches("a=d,d=I").count(), 2);
    assert_eq!(deletions.matches("i=42").count(), 1);
    assert_eq!(deletions.matches("i=43").count(), 1);
}

#[test]
fn failed_deletion_forgets_only_images_with_complete_delete_commands() {
    struct FailSecondWrite {
        writes: usize,
    }

    impl Write for FailSecondWrite {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if self.writes == 2 {
                Err(io::Error::other("injected write failure"))
            } else {
                Ok(bytes.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let first = KittyImage::new(
        b"first".to_vec(),
        /*image_id*/ 42,
        /*columns*/ 2,
        /*rows*/ 1,
    );
    let second = KittyImage::new(
        b"second".to_vec(),
        /*image_id*/ 43,
        /*columns*/ 2,
        /*rows*/ 1,
    );
    let mut registry = KittyImageRegistry::default();
    registry
        .write_definitions(&mut Vec::new(), [&first, &second])
        .expect("write initial definitions");
    let mut writer = FailSecondWrite { writes: 0 };

    let error = registry
        .delete_all(&mut writer)
        .expect_err("second deletion should fail");
    assert_eq!(error.kind(), io::ErrorKind::Other);

    let mut retry = Vec::new();
    registry
        .delete_all(&mut retry)
        .expect("retry remaining deletion");
    assert_eq!(
        String::from_utf8(retry).expect("Kitty command is UTF-8"),
        kitty_delete_image(second.image_id()),
    );
}
