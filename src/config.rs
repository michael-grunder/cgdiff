use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use ratatui::style::Color;
use serde::Deserialize;

use crate::theme::{SyntaxColorOverrides, parse_color};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    /// Path to the objdump-compatible disassembler.
    pub(crate) objdump: Option<PathBuf>,
    /// Command used to launch the diff editor.
    pub(crate) editor: Option<String>,
    /// Number of unchanged lines to keep around side-by-side diff changes.
    pub(crate) diff_context: Option<usize>,
    /// Background color for the selected TUI row.
    pub(crate) highlight_color: Option<HighlightColor>,
    /// Syntax highlighting theme name.
    #[serde(rename = "theme", alias = "syntax_theme")]
    pub(crate) syntax_theme: Option<String>,
    /// Per-token syntax color overrides applied after the theme.
    pub(crate) syntax_colors: Option<SyntaxColorOverrides>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HighlightColor {
    None,
    Color(Color),
}

impl Config {
    pub(crate) fn load() -> Result<Self> {
        for path in config_paths() {
            if !path.exists() {
                continue;
            }

            return Self::load_from_path(&path);
        }

        Ok(Self::default())
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("failed to parse {}", path.display()))
    }
}

impl<'de> Deserialize<'de> for HighlightColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

impl FromStr for HighlightColor {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized == "none" || normalized == "off" {
            return Ok(Self::None);
        }

        Ok(Self::Color(parse_color(value, "highlight_color")?))
    }
}

pub(crate) fn config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        paths.push(path.join("cgdiff.toml"));
    }
    if let Some(path) = env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        let path = path.join(".config").join("cgdiff.toml");
        if !paths.contains(&path) {
            paths.push(path);
        }
    }

    paths
}

#[cfg(test)]
pub(crate) fn parse_config(contents: &str) -> Result<Config> {
    toml::from_str(contents).context("failed to parse test config")
}
