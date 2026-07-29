//! Kitty terminal-image commands and metadata.
//!
//! Image payloads stay separate from ratatui spans so their control bytes never participate in
//! text measurement or wrapping. [`KittyImage`] owns the PNG bytes behind an `Arc`, which keeps
//! attaching the same image definition to every placeholder row inexpensive.

use std::collections::BTreeSet;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::Context;
use anyhow::Result;
use base64::Engine as _;
use base64::engine::general_purpose;
use codex_terminal_detection::terminal_info;

const ESC: &str = "\x1b";
const ST: &str = "\x1b\\";
const KITTY_CHUNK_SIZE: usize = 4096;
const KITTY_UNICODE_PLACEHOLDER_MIN_VERSION: (u64, u64, u64) = (0, 28, 0);
const GHOSTTY_UNICODE_PLACEHOLDER_MIN_VERSION: (u64, u64, u64) = (1, 0, 0);
const KITTY_PLACEHOLDER_MAX_COLUMNS: u16 = 240;
pub(crate) const MULTIPLEXER_ENV_VARS: [&str; 6] = [
    "TMUX",
    "TMUX_PANE",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_VERSION",
    "STY",
];
static STARTUP_KITTY_GRAPHICS_TERMINAL: OnceLock<Option<KittyGraphicsTerminal>> = OnceLock::new();

/// A directly probed terminal that implements the Kitty graphics protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KittyGraphicsTerminal {
    Kitty { version: (u64, u64, u64) },
    Ghostty { version: (u64, u64, u64) },
}

/// An in-memory PNG and the dimensions of its Kitty virtual placement.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct KittyImage {
    png: Arc<[u8]>,
    image_id: u32,
    placement_id: u32,
    columns: u16,
    rows: u16,
}

impl fmt::Debug for KittyImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KittyImage")
            .field("png_bytes", &self.png.len())
            .field("image_id", &self.image_id)
            .field("placement_id", &self.placement_id)
            .field("columns", &self.columns)
            .field("rows", &self.rows)
            .finish()
    }
}

impl KittyImage {
    pub(crate) fn new(png: impl Into<Arc<[u8]>>, image_id: u32, columns: u16, rows: u16) -> Self {
        debug_assert!((1..=KITTY_PLACEHOLDER_MAX_COLUMNS).contains(&columns));
        debug_assert!((1..=u16::from(u8::MAX)).contains(&rows));
        // Unicode placeholders select this value through a 24-bit underline color. Rows occupy
        // the high bytes and columns the low byte, yielding an immutable ID for every layout.
        let placement_id = (u32::from(rows) << 8) | u32::from(columns);
        Self {
            png: png.into(),
            image_id,
            placement_id,
            columns,
            rows,
        }
    }

    pub(crate) fn image_id(&self) -> u32 {
        self.image_id
    }

    pub(crate) fn placement_id(&self) -> u32 {
        self.placement_id
    }

    pub(crate) fn columns(&self) -> u16 {
        self.columns
    }

    pub(crate) fn rows(&self) -> u16 {
        self.rows
    }
}

/// Tracks immutable images and virtual placements uploaded for placeholder-backed transcript rows.
///
/// Re-uploading an existing Kitty image ID destroys every placement that refers to it. Keeping
/// this state alongside the terminal backend lets ordinary history insertion reuse the existing
/// definition while full transcript clears can explicitly delete and forget every definition.
/// Each PNG is transmitted once; resizing creates a lightweight placement with a geometry-derived
/// ID and leaves older scrollback placements intact. Display-math completions and terminal resizes
/// use a source-backed transcript rebuild that deletes this registry before replay, so evicted
/// renderer-cache images cannot accumulate here across rebuilds.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct KittyImageRegistry {
    transmitted: BTreeSet<u32>,
    placements: BTreeSet<(u32, u32)>,
}

impl KittyImageRegistry {
    pub(crate) fn write_definitions<'a>(
        &mut self,
        writer: &mut impl Write,
        images: impl IntoIterator<Item = &'a KittyImage>,
    ) -> io::Result<()> {
        for image in images {
            if !self.transmitted.contains(&image.image_id()) {
                let command = kitty_png_command(
                    &image.png,
                    image.columns(),
                    image.rows(),
                    Some(image.image_id()),
                    KittyPlacement::UnicodePlaceholder {
                        placement_id: image.placement_id(),
                    },
                );
                writer.write_all(command.as_bytes())?;
                self.transmitted.insert(image.image_id());
                self.placements
                    .insert((image.image_id(), image.placement_id()));
                continue;
            }

            let placement_key = (image.image_id(), image.placement_id());
            if self.placements.contains(&placement_key) {
                continue;
            }

            writer.write_all(
                kitty_unicode_placeholder_placement(
                    image.image_id(),
                    image.placement_id(),
                    image.columns(),
                    image.rows(),
                )
                .as_bytes(),
            )?;
            self.placements.insert(placement_key);
        }
        Ok(())
    }

    pub(crate) fn delete_all(&mut self, writer: &mut impl Write) -> io::Result<()> {
        while let Some(image_id) = self.transmitted.first().copied() {
            writer.write_all(kitty_delete_image(image_id).as_bytes())?;
            self.transmitted.remove(&image_id);
            self.placements
                .retain(|(placement_image_id, _)| *placement_image_id != image_id);
        }
        Ok(())
    }

    pub(crate) fn forget_all(&mut self) {
        self.transmitted.clear();
        self.placements.clear();
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
enum KittyScreen {
    #[default]
    Main,
    Alternate,
}

/// Keeps Kitty image definitions separate for the terminal's main and alternate screens.
///
/// Callers emit the terminal's screen-switching commands, then notify this state so subsequent
/// image definitions and deletions target the matching screen-local registry.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct KittyImageRegistries {
    main: KittyImageRegistry,
    alternate: KittyImageRegistry,
    active_screen: KittyScreen,
}

impl KittyImageRegistries {
    pub(crate) fn write_definitions<'a>(
        &mut self,
        writer: &mut impl Write,
        images: impl IntoIterator<Item = &'a KittyImage>,
    ) -> io::Result<()> {
        self.active_registry_mut().write_definitions(writer, images)
    }

    pub(crate) fn delete_active(&mut self, writer: &mut impl Write) -> io::Result<()> {
        self.active_registry_mut().delete_all(writer)
    }

    pub(crate) fn alternate_screen_active(&self) -> bool {
        self.active_screen == KittyScreen::Alternate
    }

    pub(crate) fn activate_alternate_screen(&mut self) {
        debug_assert!(!self.alternate_screen_active());
        self.active_screen = KittyScreen::Alternate;
    }

    pub(crate) fn activate_main_screen(&mut self) {
        debug_assert!(self.alternate_screen_active());
        self.active_screen = KittyScreen::Main;
    }

    pub(crate) fn forget_alternate_screen(&mut self) {
        debug_assert!(self.alternate_screen_active());
        self.alternate.forget_all();
    }

    fn active_registry_mut(&mut self) -> &mut KittyImageRegistry {
        match self.active_screen {
            KittyScreen::Main => &mut self.main,
            KittyScreen::Alternate => &mut self.alternate,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KittyPlacement {
    Direct,
    UnicodePlaceholder { placement_id: u32 },
}

#[derive(Clone, Copy, Default)]
struct KittyCapabilities {
    ansi_color_metadata: bool,
    multiplexer: bool,
    queried_terminal: Option<KittyGraphicsTerminal>,
}

pub(crate) fn set_kitty_graphics_terminal_from_startup_probe(
    terminal: Option<KittyGraphicsTerminal>,
) {
    let _ = STARTUP_KITTY_GRAPHICS_TERMINAL.set(terminal);
}

/// Returns whether this process is running directly in a terminal with Kitty Unicode placeholders.
///
/// Multiplexers are intentionally rejected: virtual placements can escape pane boundaries or
/// become detached from the scrollback rows that carry their placeholders.
pub(crate) fn kitty_unicode_placeholders_supported() -> bool {
    let info = terminal_info();
    kitty_unicode_placeholders_supported_for_terminal(KittyCapabilities {
        ansi_color_metadata: env::var_os("NO_COLOR").is_none(),
        multiplexer: info.multiplexer.is_some() || multiplexer_environment_present(),
        queried_terminal: STARTUP_KITTY_GRAPHICS_TERMINAL.get().copied().flatten(),
    })
}

fn kitty_unicode_placeholders_supported_for_terminal(capabilities: KittyCapabilities) -> bool {
    capabilities.ansi_color_metadata
        && !capabilities.multiplexer
        && capabilities
            .queried_terminal
            .is_some_and(|terminal| match terminal {
                KittyGraphicsTerminal::Kitty { version } => {
                    version >= KITTY_UNICODE_PLACEHOLDER_MIN_VERSION
                }
                KittyGraphicsTerminal::Ghostty { version } => {
                    version >= GHOSTTY_UNICODE_PLACEHOLDER_MIN_VERSION
                }
            })
}

fn multiplexer_environment_present() -> bool {
    MULTIPLEXER_ENV_VARS
        .into_iter()
        .any(|name| env::var_os(name).is_some())
}

pub(crate) fn kitty_delete_image(image_id: u32) -> String {
    wrap_for_tmux_if_needed(format!("{ESC}_Ga=d,d=I,i={image_id},q=2;{ST}"))
}

fn kitty_unicode_placeholder_placement(
    image_id: u32,
    placement_id: u32,
    columns: u16,
    rows: u16,
) -> String {
    wrap_for_tmux_if_needed(format!(
        "{ESC}_Ga=p,q=2,i={image_id},p={placement_id},U=1,c={columns},r={rows};{ST}"
    ))
}

pub(crate) fn kitty_transmit_png_with_id(
    path: &Path,
    columns: u16,
    rows: u16,
    image_id: Option<u32>,
) -> Result<String> {
    let png = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(wrap_for_tmux_if_needed(kitty_png_command(
        &png,
        columns,
        rows,
        image_id,
        KittyPlacement::Direct,
    )))
}

pub(crate) fn kitty_transmit_png_file_with_id(
    path: &Path,
    columns: u16,
    rows: u16,
    image_id: Option<u32>,
) -> Result<String> {
    let path = path
        .canonicalize()
        .with_context(|| format!("canonicalize {}", path.display()))?;
    let command = kitty_png_file_command(&path, columns, rows, image_id);
    Ok(wrap_for_tmux_if_needed(command))
}

fn kitty_png_command(
    png: &[u8],
    columns: u16,
    rows: u16,
    image_id: Option<u32>,
    placement: KittyPlacement,
) -> String {
    let payload = general_purpose::STANDARD.encode(png);
    let chunks = payload.as_bytes().chunks(KITTY_CHUNK_SIZE);
    let chunk_count = chunks.len();
    let mut command = String::new();
    for (index, chunk) in chunks.enumerate() {
        let chunk = String::from_utf8_lossy(chunk);
        let more_flag = u8::from(index + 1 < chunk_count);
        if index == 0 {
            let image_id = kitty_image_id_arg(image_id);
            let unicode_placeholder = match placement {
                KittyPlacement::Direct => String::new(),
                KittyPlacement::UnicodePlaceholder { placement_id } => {
                    format!(",p={placement_id},U=1")
                }
            };
            command.push_str(&format!(
                "{ESC}_Ga=T,t=d,f=100,c={columns},r={rows},q=2{image_id}{unicode_placeholder},m={more_flag};{chunk}{ST}",
            ));
        } else {
            command.push_str(&format!("{ESC}_Gm={more_flag};{chunk}{ST}"));
        }
    }
    command
}

fn kitty_png_file_command(path: &Path, columns: u16, rows: u16, image_id: Option<u32>) -> String {
    let payload = general_purpose::STANDARD.encode(path.to_string_lossy().as_bytes());
    let image_id = kitty_image_id_arg(image_id);
    format!("{ESC}_Ga=T,t=f,f=100,c={columns},r={rows},q=2{image_id};{payload}{ST}")
}

fn kitty_image_id_arg(image_id: Option<u32>) -> String {
    image_id
        .map(|image_id| format!(",i={image_id}"))
        .unwrap_or_default()
}

fn wrap_for_tmux_if_needed(command: String) -> String {
    if env::var_os("TMUX").is_none() {
        return command;
    }
    wrap_for_tmux(&command)
}

fn wrap_for_tmux(command: &str) -> String {
    let escaped = command.replace(ESC, "\x1b\x1b");
    format!("{ESC}Ptmux;{escaped}{ST}")
}

#[cfg(test)]
#[path = "terminal_image_tests.rs"]
mod tests;
