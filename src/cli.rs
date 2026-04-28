use std::fmt;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use crate::compare::FunctionComparison;

const VERSION: &str = concat!(
    env!("CARGO_PKG_NAME"),
    " ",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("CGDIFF_BUILD_DATE"),
    ", ",
    env!("CGDIFF_GIT_SHA"),
    ")"
);
pub(crate) const DEFAULT_EDITOR: &str = "nvim -d {file1} {file2}";
#[derive(Clone, Debug, Parser)]
#[command(version = VERSION, about = "Compare codegen between two binaries")]
pub(crate) struct Cli {
    /// First binary to compare.
    pub(crate) binary1: PathBuf,
    /// Second binary to compare.
    pub(crate) binary2: PathBuf,
    /// Path to objdump program.
    #[arg(short = 'o', long = "objdump")]
    pub(crate) objdump: Option<PathBuf>,
    /// Command used to launch the diff editor.
    #[arg(short = 'e', long = "editor", default_value = DEFAULT_EDITOR)]
    pub(crate) editor: String,
    /// Sort mode for similarity results.
    #[arg(short = 'd', long = "diff-mode", default_value_t = DiffMode::Combined)]
    pub(crate) diff_mode: DiffMode,
    /// Include functions that only exist in one binary in the TUI.
    #[arg(long = "include-unique-functions")]
    pub(crate) include_unique_functions: bool,
    /// Include shared functions with identical instruction text or a perfect
    /// 1.000 similarity score in the TUI.
    #[arg(long = "include-identical-functions")]
    pub(crate) include_identical_functions: bool,
    /// Pre-filter functions by case-insensitive substring or `/regex/`.
    #[arg(long = "filter")]
    pub(crate) filter: Option<String>,
    /// Dump the sorted comparison table to stdout instead of opening the TUI.
    #[arg(long = "stdio")]
    pub(crate) stdio: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum DiffMode {
    Combined,
    Count,
    Order,
}

impl fmt::Display for DiffMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

impl DiffMode {
    pub(crate) const fn score(self, comparison: &FunctionComparison) -> f64 {
        match self {
            Self::Combined => comparison.combined_score,
            Self::Count => comparison.count_score,
            Self::Order => comparison.order_score,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Combined => "combined",
            Self::Count => "count",
            Self::Order => "ops",
        }
    }
}
