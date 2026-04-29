#![warn(clippy::all, clippy::nursery, clippy::pedantic)]
#![allow(clippy::redundant_pub_crate)]

pub(crate) mod cli;
pub(crate) mod compare;
pub(crate) mod disassembly;
pub(crate) mod filter;
pub(crate) mod output;
pub(crate) mod progress;
pub(crate) mod tui;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use anyhow::{Result, anyhow};
use clap::Parser;

use crate::cli::Cli;
use crate::compare::build_comparisons;
use crate::disassembly::{BinaryAnalysis, analyze_binary};
use crate::filter::compile_cli_filter;
use crate::output::{dump_comparisons, prepare_comparisons};
use crate::progress::render_progress;
use crate::tui::run_tui;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let filter = compile_cli_filter(cli.filter.as_deref(), "--filter")?;
    let filter_out =
        compile_cli_filter(cli.filter_out.as_deref(), "--filter-out")?;
    let objdump = select_objdump(cli.objdump.as_deref())?;

    let (progress_tx, progress_rx) = mpsc::channel();
    let binary1 = cli.binary1.clone();
    let binary2 = cli.binary2.clone();
    let binary_one_label = format!("A {}", cli.binary1.display());
    let binary_two_label = format!("B {}", cli.binary2.display());
    let objdump_one = objdump.clone();
    let progress_tx_one = progress_tx.clone();
    let progress_tx_two = progress_tx.clone();

    let handle_one = thread::spawn(move || {
        analyze_binary(
            &objdump_one,
            &binary1,
            &binary_one_label,
            &progress_tx_one,
        )
    });
    let handle_two = thread::spawn(move || {
        analyze_binary(&objdump, &binary2, &binary_two_label, &progress_tx_two)
    });
    drop(progress_tx);

    let mut states = HashMap::new();
    render_progress(&progress_rx, &mut states, cli.stdio)?;

    let analysis_one = join_analysis(handle_one, "binary-1")?;
    let analysis_two = join_analysis(handle_two, "binary-2")?;

    let comparisons = build_comparisons(
        &analysis_one,
        &analysis_two,
        cli.include_unique_functions,
        cli.include_identical_functions,
        filter_out.as_ref(),
        filter.as_ref(),
    );
    if cli.stdio {
        dump_comparisons(io::stdout(), &comparisons, cli.diff_mode)?;
        return Ok(());
    }

    let prepared =
        prepare_comparisons(comparisons, &cli.binary1, &cli.binary2)?;
    run_tui(
        prepared,
        cli.diff_mode,
        cli.include_unique_functions,
        cli.include_identical_functions,
        cli.filter_out.as_deref().unwrap_or_default(),
        cli.filter.as_deref().unwrap_or_default(),
        &cli.editor,
    )?;

    Ok(())
}

fn join_analysis(
    handle: thread::JoinHandle<Result<BinaryAnalysis>>,
    label: &str,
) -> Result<BinaryAnalysis> {
    handle
        .join()
        .map_err(|_| anyhow!("worker thread for {label} panicked"))?
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
