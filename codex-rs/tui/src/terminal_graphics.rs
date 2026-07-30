//! Terminal graphics capabilities shared by ambient pets and transcript images.

use std::env;
use std::sync::OnceLock;

use codex_terminal_detection::Multiplexer;
use codex_terminal_detection::TerminalInfo;
use codex_terminal_detection::TerminalName;
use codex_terminal_detection::terminal_info;

// iTerm2 3.6 introduced Kitty graphics, 3.6.1 added file transfer, and early 3.6
// releases needed further display fixes. Keep transcript placement on the
// conservative 3.6.3 floor while retaining the earlier pet transports.
const ITERM2_KITTY_MIN_VERSION: (u64, u64, u64) = (3, 6, 0);
const ITERM2_KITTY_FILE_MIN_VERSION: (u64, u64, u64) = (3, 6, 1);
const ITERM2_UNICODE_PLACEHOLDER_MIN_VERSION: (u64, u64, u64) = (3, 6, 3);
const KITTY_UNICODE_PLACEHOLDER_MIN_VERSION: (u64, u64, u64) = (0, 28, 0);
const GHOSTTY_UNICODE_PLACEHOLDER_MIN_VERSION: (u64, u64, u64) = (1, 0, 0);

static STARTUP_KITTY_GRAPHICS_TERMINAL: OnceLock<Option<KittyGraphicsTerminal>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageProtocol {
    Kitty,
    KittyLocalFile,
    Sixel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PetImageSupport {
    Supported(ImageProtocol),
    Unsupported(PetImageUnsupportedReason),
}

impl PetImageSupport {
    pub(crate) fn protocol(self) -> Option<ImageProtocol> {
        match self {
            Self::Supported(protocol) => Some(protocol),
            Self::Unsupported(_) => None,
        }
    }

    pub(crate) fn unsupported_message(self) -> Option<&'static str> {
        match self {
            Self::Supported(_) => None,
            Self::Unsupported(reason) => Some(reason.message()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PetImageUnsupportedReason {
    Tmux,
    Zellij,
    Iterm2TooOld,
    Terminal,
}

impl PetImageUnsupportedReason {
    fn message(self) -> &'static str {
        match self {
            Self::Tmux => {
                "Pets are disabled in tmux. Terminal images don’t stay pane-local in tmux and can corrupt scrollback or move between panes. Run Codex outside tmux to use pets."
            }
            Self::Zellij => {
                "Pets are disabled in Zellij. Terminal images don’t stay reliably pane-local in Zellij. Run Codex outside Zellij to use pets."
            }
            Self::Iterm2TooOld => {
                "Pets require iTerm2 3.6 or newer. Upgrade iTerm2 to use terminal pets."
            }
            Self::Terminal => {
                "Pets aren’t available in this terminal. Terminal pets need image support, and this terminal environment doesn’t expose a supported image protocol. Try a terminal with Kitty graphics or Sixel support, or run Codex outside tmux."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TranscriptImageProtocol {
    KittyUnicodePlaceholders,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TerminalGraphicsSupport {
    pet_images: PetImageSupport,
    transcript_images: Option<TranscriptImageProtocol>,
}

impl TerminalGraphicsSupport {
    pub(crate) fn pet_images(self) -> PetImageSupport {
        self.pet_images
    }

    pub(crate) fn transcript_images(self) -> Option<TranscriptImageProtocol> {
        self.transcript_images
    }
}

/// A directly probed terminal that implements the Kitty graphics protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KittyGraphicsTerminal {
    Kitty { version: (u64, u64, u64) },
    Ghostty { version: (u64, u64, u64) },
}

impl KittyGraphicsTerminal {
    fn unicode_placeholders_supported(self) -> bool {
        match self {
            Self::Kitty { version } => version >= KITTY_UNICODE_PLACEHOLDER_MIN_VERSION,
            Self::Ghostty { version } => version >= GHOSTTY_UNICODE_PLACEHOLDER_MIN_VERSION,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct EnvironmentHints {
    kitty: bool,
    wezterm: bool,
    tmux: bool,
    zellij: bool,
    screen: bool,
    no_color: bool,
}

impl EnvironmentHints {
    fn detect() -> Self {
        Self {
            kitty: env::var_os("KITTY_WINDOW_ID").is_some(),
            wezterm: env::var_os("WEZTERM_EXECUTABLE").is_some()
                || env::var_os("WEZTERM_VERSION").is_some(),
            tmux: env::var_os("TMUX").is_some() || env::var_os("TMUX_PANE").is_some(),
            zellij: env::var_os("ZELLIJ").is_some()
                || env::var_os("ZELLIJ_SESSION_NAME").is_some()
                || env::var_os("ZELLIJ_VERSION").is_some(),
            screen: env::var_os("STY").is_some(),
            no_color: env::var_os("NO_COLOR").is_some(),
        }
    }

    fn multiplexer_present(self) -> bool {
        self.tmux || self.zellij || self.screen
    }
}

pub(crate) fn detect_terminal_graphics_support() -> TerminalGraphicsSupport {
    terminal_graphics_support_for(
        &terminal_info(),
        EnvironmentHints::detect(),
        STARTUP_KITTY_GRAPHICS_TERMINAL.get().copied().flatten(),
    )
}

pub(crate) fn set_kitty_graphics_terminal_from_startup_probe(
    terminal: Option<KittyGraphicsTerminal>,
) {
    let _ = STARTUP_KITTY_GRAPHICS_TERMINAL.set(terminal);
}

pub(crate) fn kitty_version_probe_needed() -> bool {
    kitty_version_probe_needed_for(&terminal_info(), EnvironmentHints::detect())
}

fn terminal_graphics_support_for(
    info: &TerminalInfo,
    environment: EnvironmentHints,
    probed_terminal: Option<KittyGraphicsTerminal>,
) -> TerminalGraphicsSupport {
    let pet_images = pet_image_support_for(info, environment);
    let transcript_images = (!environment.no_color
        && !environment.multiplexer_present()
        && info.multiplexer.is_none()
        && (probed_terminal.is_some_and(KittyGraphicsTerminal::unicode_placeholders_supported)
            || supports_iterm2_unicode_placeholders(info)))
    .then_some(TranscriptImageProtocol::KittyUnicodePlaceholders);

    TerminalGraphicsSupport {
        pet_images,
        transcript_images,
    }
}

fn pet_image_support_for(info: &TerminalInfo, environment: EnvironmentHints) -> PetImageSupport {
    if environment.tmux || matches!(info.multiplexer, Some(Multiplexer::Tmux { .. })) {
        return PetImageSupport::Unsupported(PetImageUnsupportedReason::Tmux);
    }
    if environment.zellij || matches!(info.multiplexer, Some(Multiplexer::Zellij { .. })) {
        return PetImageSupport::Unsupported(PetImageUnsupportedReason::Zellij);
    }
    if environment.kitty {
        return PetImageSupport::Supported(ImageProtocol::Kitty);
    }
    if environment.wezterm {
        return PetImageSupport::Supported(ImageProtocol::Kitty);
    }

    if is_iterm2_terminal(info) {
        return match parse_dotted_version(info.version.as_deref()) {
            Some(version) if version >= ITERM2_KITTY_FILE_MIN_VERSION => {
                PetImageSupport::Supported(ImageProtocol::KittyLocalFile)
            }
            Some(version) if version >= ITERM2_KITTY_MIN_VERSION => {
                PetImageSupport::Supported(ImageProtocol::Kitty)
            }
            _ => PetImageSupport::Unsupported(PetImageUnsupportedReason::Iterm2TooOld),
        };
    }
    if supports_kitty_graphics(info) {
        return PetImageSupport::Supported(ImageProtocol::Kitty);
    }
    if supports_sixel(info) {
        return PetImageSupport::Supported(ImageProtocol::Sixel);
    }

    PetImageSupport::Unsupported(PetImageUnsupportedReason::Terminal)
}

fn kitty_version_probe_needed_for(info: &TerminalInfo, environment: EnvironmentHints) -> bool {
    !environment.no_color
        && !environment.multiplexer_present()
        && info.multiplexer.is_none()
        && (environment.kitty || is_kitty_or_ghostty_terminal(info))
}

fn supports_iterm2_unicode_placeholders(info: &TerminalInfo) -> bool {
    is_iterm2_terminal(info)
        && version_is_at_least(
            info.version.as_deref(),
            ITERM2_UNICODE_PLACEHOLDER_MIN_VERSION,
        )
}

fn is_iterm2_terminal(info: &TerminalInfo) -> bool {
    matches!(info.name, TerminalName::Iterm2)
        || terminal_field_contains(info.term_program.as_deref(), "iterm")
}

fn is_kitty_or_ghostty_terminal(info: &TerminalInfo) -> bool {
    matches!(info.name, TerminalName::Ghostty | TerminalName::Kitty)
        || terminal_field_contains(info.term.as_deref(), "kitty")
        || terminal_field_contains(info.term.as_deref(), "ghostty")
        || terminal_field_contains(info.term_program.as_deref(), "kitty")
        || terminal_field_contains(info.term_program.as_deref(), "ghostty")
}

fn supports_kitty_graphics(info: &TerminalInfo) -> bool {
    is_kitty_or_ghostty_terminal(info)
        || matches!(info.name, TerminalName::WezTerm)
        || terminal_field_contains(info.term.as_deref(), "wezterm")
        || terminal_field_contains(info.term_program.as_deref(), "wezterm")
}

fn supports_sixel(info: &TerminalInfo) -> bool {
    matches!(info.name, TerminalName::WindowsTerminal)
        || terminal_field_contains(info.term.as_deref(), "sixel")
        || terminal_field_contains(info.term.as_deref(), "mlterm")
        || terminal_field_contains(info.term.as_deref(), "foot")
}

fn terminal_field_contains(value: Option<&str>, needle: &str) -> bool {
    value.is_some_and(|value| value.to_ascii_lowercase().contains(needle))
}

fn version_is_at_least(version: Option<&str>, minimum: (u64, u64, u64)) -> bool {
    parse_dotted_version(version).is_some_and(|version| version >= minimum)
}

fn parse_dotted_version(version: Option<&str>) -> Option<(u64, u64, u64)> {
    let version = version?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

#[cfg(test)]
#[path = "terminal_graphics_tests.rs"]
mod tests;
