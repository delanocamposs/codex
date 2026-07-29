//! Kitty-graphics startup probe policy and XTVERSION parsing.

use std::env;

use codex_terminal_detection::TerminalName;
use codex_terminal_detection::terminal_info;

use crate::terminal_image::KittyGraphicsTerminal;
use crate::terminal_image::multiplexer_environment_present;

const XTVERSION_QUERY: &[u8] = b"\x1B[>0q";

/// State for the optional Kitty version query during the batched startup probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StartupVersionProbe {
    Skip,
    Query(Option<KittyGraphicsTerminal>),
}

impl StartupVersionProbe {
    pub(super) const fn query() -> Self {
        Self::Query(None)
    }

    const fn skip() -> Self {
        Self::Skip
    }

    /// Returns the XTVERSION query to prepend to the rest of the startup query batch.
    ///
    /// Requiring its reply before early completion prevents a split or reordered DCS reply from
    /// leaking into crossterm. The shared startup deadline remains the bounded fallback.
    pub(super) fn query_bytes(self) -> &'static [u8] {
        match self {
            Self::Skip => b"",
            Self::Query(_) => XTVERSION_QUERY,
        }
    }

    pub(super) fn observe(&mut self, buffer: &[u8]) {
        if let Self::Query(terminal @ None) = self {
            *terminal = parse_terminal(buffer);
        }
    }

    pub(super) fn is_complete(self) -> bool {
        matches!(self, Self::Skip | Self::Query(Some(_)))
    }

    pub(super) fn terminal(self) -> Option<KittyGraphicsTerminal> {
        match self {
            Self::Skip => None,
            Self::Query(terminal) => terminal,
        }
    }
}

pub(super) fn startup_version_probe() -> StartupVersionProbe {
    if env::var_os("NO_COLOR").is_some() || multiplexer_environment_present() {
        return StartupVersionProbe::skip();
    }

    let info = terminal_info();
    let terminal_hint = env::var_os("TERM").is_some_and(|value| {
        let value = value.to_string_lossy().to_ascii_lowercase();
        value.contains("kitty") || value.contains("ghostty")
    });
    if matches!(info.name, TerminalName::Kitty | TerminalName::Ghostty)
        || env::var_os("KITTY_WINDOW_ID").is_some()
        || terminal_hint
    {
        StartupVersionProbe::query()
    } else {
        StartupVersionProbe::skip()
    }
}

pub(super) fn parse_terminal(buffer: &[u8]) -> Option<KittyGraphicsTerminal> {
    let mut dcs_start = None;
    let mut ghostty = None;
    let mut index = 0;
    while index + 1 < buffer.len() {
        match &buffer[index..index + 2] {
            b"\x1BP" => {
                dcs_start = Some(index + 2);
                index += 2;
            }
            b"\x1B\\" => {
                let Some(start) = dcs_start.take() else {
                    index += 2;
                    continue;
                };
                let payload = &buffer[start..index];
                if let Some(version) = payload
                    .strip_prefix(b">|kitty(")
                    .and_then(|payload| payload.strip_suffix(b")"))
                    .and_then(|version| std::str::from_utf8(version).ok())
                    .and_then(parse_version)
                {
                    return Some(KittyGraphicsTerminal::Kitty { version });
                }
                if ghostty.is_none()
                    && let Some(version) = payload
                        .strip_prefix(b">|ghostty ")
                        .and_then(|version| std::str::from_utf8(version).ok())
                        .and_then(|version| version.split(['-', '+']).next())
                        .and_then(parse_version)
                {
                    ghostty = Some(KittyGraphicsTerminal::Ghostty { version });
                }
                index += 2;
            }
            _ => index += 1,
        }
    }

    ghostty
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

#[cfg(test)]
#[path = "kitty_tests.rs"]
mod tests;
