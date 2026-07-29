//! Kitty-graphics startup probe policy and XTVERSION parsing.

use std::env;

use crate::terminal_image::KittyGraphicsTerminal;
use crate::terminal_image::MULTIPLEXER_ENV_VARS;

const XTVERSION_QUERY: &[u8] = b"\x1B[>0q";

/// State for the optional Kitty version query during the batched startup probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StartupVersionProbe {
    should_query: bool,
    terminal: Option<KittyGraphicsTerminal>,
}

impl StartupVersionProbe {
    pub(super) const fn query() -> Self {
        Self {
            should_query: true,
            terminal: None,
        }
    }

    const fn skip() -> Self {
        Self {
            should_query: false,
            terminal: None,
        }
    }

    /// Returns the XTVERSION query to prepend to the rest of the startup query batch.
    ///
    /// Requiring its reply before early completion prevents a split or reordered DCS reply from
    /// leaking into crossterm. The shared startup deadline remains the bounded fallback.
    pub(super) fn query_bytes(self) -> &'static [u8] {
        if self.should_query {
            XTVERSION_QUERY
        } else {
            b""
        }
    }

    pub(super) fn observe(&mut self, buffer: &[u8]) {
        if self.terminal.is_none() {
            self.terminal = parse_terminal(buffer);
        }
    }

    pub(super) fn is_complete(self) -> bool {
        !self.should_query || self.terminal.is_some()
    }

    pub(super) fn terminal(self) -> Option<KittyGraphicsTerminal> {
        self.terminal
    }
}

#[derive(Clone, Copy, Default)]
struct StartupVersionProbeEnvironment {
    no_color: bool,
    multiplexer: bool,
    kitty_window_id: bool,
    kitty_term: bool,
    kitty_term_program: bool,
    ghostty_term: bool,
    ghostty_term_program: bool,
}

pub(super) fn startup_version_probe() -> StartupVersionProbe {
    startup_version_probe_for(StartupVersionProbeEnvironment {
        no_color: env::var_os("NO_COLOR").is_some(),
        multiplexer: MULTIPLEXER_ENV_VARS
            .into_iter()
            .any(|name| env::var_os(name).is_some()),
        kitty_window_id: env::var_os("KITTY_WINDOW_ID").is_some(),
        kitty_term: env::var_os("TERM").is_some_and(|value| {
            value
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("kitty")
        }),
        kitty_term_program: env::var_os("TERM_PROGRAM")
            .is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case("kitty")),
        ghostty_term: env::var_os("TERM").is_some_and(|value| {
            value
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("ghostty")
        }),
        ghostty_term_program: env::var_os("TERM_PROGRAM")
            .is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case("ghostty")),
    })
}

fn startup_version_probe_for(environment: StartupVersionProbeEnvironment) -> StartupVersionProbe {
    if environment.no_color || environment.multiplexer {
        return StartupVersionProbe::skip();
    }
    if environment.kitty_window_id
        || environment.kitty_term
        || environment.kitty_term_program
        || environment.ghostty_term
        || environment.ghostty_term_program
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
