#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::redundant_pub_crate)]

pub(crate) mod cli;
pub(crate) mod compare;
pub(crate) mod config;
pub(crate) mod diff_view;
pub(crate) mod disassembly;
pub(crate) mod filter;
pub(crate) mod output;
pub(crate) mod pager;
pub(crate) mod progress;
pub(crate) mod theme;
pub(crate) mod tui;

#[cfg(test)]
mod tests;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use anyhow::{Result, anyhow};
use clap::Parser;

use crate::cli::Cli;
use crate::compare::{FunctionComparison, build_comparisons};
use crate::config::{Config, HighlightColor};
use crate::diff_view::DEFAULT_DIFF_CONTEXT;
use crate::disassembly::{BinaryAnalysis, analyze_binary};
use crate::filter::compile_cli_filter;
use crate::output::{
    RenderStyle, dump_comparison_diff, dump_comparisons, prepare_comparisons,
};
use crate::progress::render_progress;
use crate::theme::{
    DEFAULT_THEME_NAME, SyntaxColorOverrides, SyntaxTheme, write_theme_samples,
};
use crate::tui::{TuiOptions, run_tui};

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.list_themes {
        write_theme_samples(io::stdout())?;
        return Ok(());
    }

    let stdio = cli.stdio || cli.diff;
    let config = Config::load()?;
    let include = compile_cli_filter(cli.include.as_deref(), "--include")?;
    let exclude = compile_cli_filter(cli.exclude.as_deref(), "--exclude")?;
    let (binary1, binary2) = required_binaries(&cli);
    let objdump =
        select_objdump(cli.objdump.as_deref().or(config.objdump.as_deref()))?;
    let editor = cli
        .editor
        .as_deref()
        .or(config.editor.as_deref())
        .unwrap_or(cli::DEFAULT_EDITOR);
    let highlight_color = config
        .highlight_color
        .unwrap_or(HighlightColor::Color(ratatui::style::Color::Blue));
    let diff_context = cli
        .diff_context
        .or(config.diff_context)
        .unwrap_or(DEFAULT_DIFF_CONTEXT);
    let syntax_theme = resolve_syntax_theme(
        cli.theme.as_deref(),
        config.syntax_theme.as_deref(),
        config.syntax_colors.as_ref(),
    )?;

    let (progress_tx, progress_rx) = mpsc::channel();
    let binary1 = binary1.to_path_buf();
    let binary2 = binary2.to_path_buf();
    let binary_one_label = format!("A {}", binary1.display());
    let binary_two_label = format!("B {}", binary2.display());
    let binary1_worker = binary1.clone();
    let binary2_worker = binary2.clone();
    let objdump_one = objdump.clone();
    let progress_tx_one = progress_tx.clone();
    let progress_tx_two = progress_tx.clone();

    let handle_one = thread::spawn(move || {
        analyze_binary(
            &objdump_one,
            &binary1_worker,
            &binary_one_label,
            &progress_tx_one,
        )
    });
    let handle_two = thread::spawn(move || {
        analyze_binary(
            &objdump,
            &binary2_worker,
            &binary_two_label,
            &progress_tx_two,
        )
    });
    drop(progress_tx);

    render_progress(&progress_rx, stdio)?;

    let analysis_one = join_analysis(handle_one, "binary-1")?;
    let analysis_two = join_analysis(handle_two, "binary-2")?;

    let comparisons = build_comparisons(
        &analysis_one,
        &analysis_two,
        cli.include_unique_functions,
        cli.include_identical_functions,
        exclude.as_ref(),
        include.as_ref(),
    );
    if stdio {
        return dump_stdio(
            &cli,
            &comparisons,
            &syntax_theme,
            &binary1,
            &binary2,
        );
    }

    let prepared = prepare_comparisons(comparisons)?;
    run_tui(
        prepared,
        TuiOptions {
            diff_mode: cli.diff_mode,
            include_unique_functions: cli.include_unique_functions,
            include_identical_functions: cli.include_identical_functions,
            initial_exclude_query: cli.exclude.as_deref().unwrap_or_default(),
            initial_include_query: cli.include.as_deref().unwrap_or_default(),
            editor,
            highlight_color,
            diff_context,
            syntax_theme,
        },
    )?;

    Ok(())
}

/// Renders the non-interactive `--stdio`/`--diff` output.
///
/// Output is colorized and routed through a pager when stdout is a terminal;
/// piping to a file or another process yields plain, unpaged text.
fn dump_stdio(
    cli: &Cli,
    comparisons: &[FunctionComparison],
    syntax_theme: &SyntaxTheme,
    binary1: &Path,
    binary2: &Path,
) -> Result<()> {
    let is_terminal = pager::stdout_is_terminal();
    let color = is_terminal && std::env::var_os("NO_COLOR").is_none();
    let style = RenderStyle {
        color,
        theme: syntax_theme,
    };

    pager::paginate(is_terminal, |writer| {
        if cli.diff {
            dump_comparison_diff(
                writer,
                comparisons,
                cli.diff_mode,
                binary1,
                binary2,
                style,
            )
        } else {
            dump_comparisons(writer, comparisons, cli.diff_mode, style)
        }
    })
}

fn join_analysis(
    handle: thread::JoinHandle<Result<BinaryAnalysis>>,
    label: &str,
) -> Result<BinaryAnalysis> {
    handle
        .join()
        .map_err(|_| anyhow!("worker thread for {label} panicked"))?
}

fn resolve_syntax_theme(
    cli_theme: Option<&str>,
    config_theme: Option<&str>,
    color_overrides: Option<&SyntaxColorOverrides>,
) -> Result<SyntaxTheme> {
    let mut syntax_theme = SyntaxTheme::named(
        cli_theme.or(config_theme).unwrap_or(DEFAULT_THEME_NAME),
    )?;
    if let Some(overrides) = color_overrides {
        syntax_theme.apply_color_overrides(overrides);
    }
    Ok(syntax_theme)
}

fn required_binaries(cli: &Cli) -> (&Path, &Path) {
    let binary1 = cli
        .binary1
        .as_deref()
        .expect("clap should require binary1 unless --list-themes");
    let binary2 = cli
        .binary2
        .as_deref()
        .expect("clap should require binary2 unless --list-themes");
    (binary1, binary2)
}

fn select_objdump(configured: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = configured {
        return Ok(path.to_path_buf());
    }

    ["llvm-objdump", "objdump"]
        .into_iter()
        .find_map(|candidate| which::which(candidate).ok())
        .ok_or_else(|| {
            anyhow!("failed to find llvm-objdump or objdump in PATH")
        })
}
