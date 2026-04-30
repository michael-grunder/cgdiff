use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use ratatui::style::Color;
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    /// Path to the objdump-compatible disassembler.
    pub(crate) objdump: Option<PathBuf>,
    /// Command used to launch the diff editor.
    pub(crate) editor: Option<String>,
    /// Background color for the selected TUI row.
    pub(crate) highlight_color: Option<HighlightColor>,
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

        let color = match normalized.as_str() {
            "black" => Color::Black,
            "red" => Color::Red,
            "green" => Color::Green,
            "yellow" => Color::Yellow,
            "blue" => Color::Blue,
            "magenta" => Color::Magenta,
            "cyan" => Color::Cyan,
            "gray" | "grey" => Color::Gray,
            "dark-gray" | "dark-grey" | "darkgray" | "darkgrey" => {
                Color::DarkGray
            }
            "light-red" | "lightred" => Color::LightRed,
            "light-green" | "lightgreen" => Color::LightGreen,
            "light-yellow" | "lightyellow" => Color::LightYellow,
            "light-blue" | "lightblue" => Color::LightBlue,
            "light-magenta" | "lightmagenta" => Color::LightMagenta,
            "light-cyan" | "lightcyan" => Color::LightCyan,
            "white" => Color::White,
            _ => bail!("unsupported highlight_color `{value}`"),
        };

        Ok(Self::Color(color))
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
