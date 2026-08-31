use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use image::imageops::FilterType;

use super::sixel;
pub(super) use crate::terminal_graphics::ImageProtocol;
pub(super) use crate::terminal_graphics::PetImageSupport;
#[cfg(test)]
pub(super) use crate::terminal_graphics::PetImageUnsupportedReason;

const SIXEL_CACHE_VERSION: &str = "v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolSelection {
    Auto,
    Kitty,
    Sixel,
}

impl ProtocolSelection {
    pub(crate) fn resolve(self) -> PetImageSupport {
        match self {
            Self::Kitty => PetImageSupport::Supported(ImageProtocol::Kitty),
            Self::Sixel => PetImageSupport::Supported(ImageProtocol::Sixel),
            Self::Auto => crate::terminal_graphics::detect_terminal_graphics_support().pet_images(),
        }
    }
}

impl FromStr for ProtocolSelection {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "kitty" => Ok(Self::Kitty),
            "sixel" => Ok(Self::Sixel),
            other => bail!("unknown protocol {other}; expected auto, kitty, or sixel"),
        }
    }
}

pub fn sixel_frame(frame_path: &Path, cache_dir: &Path, height_px: u16) -> Result<PathBuf> {
    fs::create_dir_all(cache_dir).with_context(|| format!("create {}", cache_dir.display()))?;

    let stem = frame_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("frame path has no valid file stem")?;
    let path = cache_dir.join(format!("{stem}_h{height_px}_{SIXEL_CACHE_VERSION}.six"));
    if path.exists() {
        return Ok(path);
    }

    let frame =
        image::open(frame_path).with_context(|| format!("read {}", frame_path.display()))?;
    let height = u32::from(height_px).max(1);
    let width = ((u64::from(frame.width()) * u64::from(height)) / u64::from(frame.height()))
        .try_into()
        .unwrap_or(u32::MAX)
        .max(1);
    let rgba = frame.resize(width, height, FilterType::Lanczos3).to_rgba8();
    let (width, height) = rgba.dimensions();
    let sixel = sixel::encode_rgba(&rgba.into_raw(), width, height)?;

    fs::write(&path, sixel).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_protocol_selection() {
        assert_eq!(
            "auto".parse::<ProtocolSelection>().unwrap(),
            ProtocolSelection::Auto
        );
        assert_eq!(
            "kitty".parse::<ProtocolSelection>().unwrap(),
            ProtocolSelection::Kitty
        );
        assert_eq!(
            "sixel".parse::<ProtocolSelection>().unwrap(),
            ProtocolSelection::Sixel
        );
    }

    #[test]
    fn explicit_protocol_resolves_without_auto_detection() {
        assert_eq!(
            ProtocolSelection::Kitty.resolve(),
            PetImageSupport::Supported(ImageProtocol::Kitty)
        );
        assert_eq!(
            ProtocolSelection::Sixel.resolve(),
            PetImageSupport::Supported(ImageProtocol::Sixel)
        );
    }

    #[test]
    fn sixel_frame_encodes_without_external_crate() {
        let dir = tempfile::tempdir().unwrap();
        let frame_path = dir.path().join("frame.png");
        let rgba = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        rgba.save(&frame_path).unwrap();

        let sixel_path =
            sixel_frame(&frame_path, &dir.path().join("sixel"), /*height_px*/ 1).unwrap();
        let sixel = fs::read_to_string(sixel_path).unwrap();

        assert!(sixel.starts_with("\x1bP9;1;0q\"1;1;1;1"));
        assert!(sixel.contains("#224;2;100;0;0"));
        assert!(sixel.contains("#224@"));
        assert!(sixel.ends_with("\x1b\\"));
    }
}
