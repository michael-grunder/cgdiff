use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};

/// Returns whether standard output is connected to an interactive terminal.
///
/// Used to decide whether stdio output should be colorized and paginated;
/// piping to a file or another process suppresses both.
pub(crate) fn stdout_is_terminal() -> bool {
    io::stdout().is_terminal()
}

/// Renders stdio output either straight to stdout or through a terminal pager.
///
/// When `paginate` is set and a pager process can be spawned, `body` writes to
/// the pager's standard input; otherwise it writes to a locked stdout handle.
/// A `BrokenPipe` error (the reader quit early) is treated as success, matching
/// the behavior of common command-line tools.
pub(crate) fn paginate<F>(paginate: bool, body: F) -> Result<()>
where
    F: FnOnce(&mut dyn Write) -> Result<()>,
{
    if paginate && let Some(mut child) = spawn_pager() {
        let mut stdin = child.stdin.take().expect("pager stdin is piped");
        let result = ignore_broken_pipe(body(&mut stdin));
        drop(stdin);
        child.wait().context("failed to wait for pager process")?;
        return result;
    }

    let stdout = io::stdout();
    let mut handle = stdout.lock();
    ignore_broken_pipe(body(&mut handle))
}

fn ignore_broken_pipe(result: Result<()>) -> Result<()> {
    if let Err(error) = &result
        && let Some(io_error) = error.downcast_ref::<io::Error>()
        && io_error.kind() == io::ErrorKind::BrokenPipe
    {
        return Ok(());
    }
    result
}

fn spawn_pager() -> Option<Child> {
    let command_line = env::var("PAGER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "less".to_owned());

    let mut parts = shlex::split(&command_line)?;
    if parts.is_empty() {
        return None;
    }
    let program = parts.remove(0);

    let mut command = Command::new(&program);
    command.args(&parts).stdin(Stdio::piped());
    if is_less(&program) && env::var_os("LESS").is_none() {
        // F: quit if the output fits one screen, R: pass through ANSI color,
        // X: skip terminal init so short output stays visible. Matches git.
        command.env("LESS", "FRX");
    }

    command.spawn().ok()
}

fn is_less(program: &str) -> bool {
    Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem == "less")
}
