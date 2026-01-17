use std::borrow::Cow;

use anyhow::{anyhow, Result};
use gpui::{AssetSource, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*"]
#[include = "fonts/**/*"]
#[exclude = "*.DS_Store"]
pub struct Assets;

/// Get embedded fonts for registration with GPUI
pub fn embedded_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Assets::get("fonts/JetBrainsMono-Regular.ttf")
            .expect("JetBrainsMono-Regular.ttf not found")
            .data,
        Assets::get("fonts/JetBrainsMono-Bold.ttf")
            .expect("JetBrainsMono-Bold.ttf not found")
            .data,
        Assets::get("fonts/JetBrainsMono-Italic.ttf")
            .expect("JetBrainsMono-Italic.ttf not found")
            .data,
        Assets::get("fonts/JetBrainsMono-BoldItalic.ttf")
            .expect("JetBrainsMono-BoldItalic.ttf not found")
            .data,
    ]
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow!("could not find asset at path \"{}\"", path))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}
