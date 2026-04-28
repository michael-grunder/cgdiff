use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

pub(crate) enum ProgressEvent {
    Started { label: String, total_bytes: u64 },
    Processed { label: String, bytes: u64 },
    Finished { label: String },
}

#[derive(Debug)]
pub(crate) struct ProgressState {
    total_bytes: u64,
    processed_bytes: u64,
    completed: bool,
}

pub(crate) fn send_progress_start(
    progress_tx: &mpsc::Sender<ProgressEvent>,
    label: &str,
    total_bytes: u64,
) -> Result<()> {
    progress_tx
        .send(ProgressEvent::Started {
            label: label.to_owned(),
            total_bytes,
        })
        .map_err(|_| anyhow!("failed to send progress update"))
}

pub(crate) fn send_progress_processed(
    progress_tx: &mpsc::Sender<ProgressEvent>,
    label: &str,
    bytes: u64,
) -> Result<()> {
    progress_tx
        .send(ProgressEvent::Processed {
            label: label.to_owned(),
            bytes,
        })
        .map_err(|_| anyhow!("failed to send progress update"))
}

pub(crate) fn send_progress_finished(
    progress_tx: &mpsc::Sender<ProgressEvent>,
    label: &str,
) -> Result<()> {
    progress_tx
        .send(ProgressEvent::Finished {
            label: label.to_owned(),
        })
        .map_err(|_| anyhow!("failed to send progress update"))
}

pub(crate) fn render_progress(
    progress_rx: &mpsc::Receiver<ProgressEvent>,
    states: &mut HashMap<String, ProgressState>,
    use_stderr: bool,
) -> Result<()> {
    while let Ok(event) = progress_rx.recv_timeout(Duration::from_millis(100)) {
        match event {
            ProgressEvent::Started { label, total_bytes } => {
                states.insert(
                    label,
                    ProgressState {
                        total_bytes,
                        processed_bytes: 0,
                        completed: false,
                    },
                );
            }
            ProgressEvent::Processed { label, bytes } => {
                if let Some(state) = states.get_mut(&label) {
                    state.processed_bytes = bytes;
                }
            }
            ProgressEvent::Finished { label } => {
                if let Some(state) = states.get_mut(&label) {
                    state.processed_bytes = state.total_bytes;
                    state.completed = true;
                }
            }
        }

        print_progress(states, use_stderr)?;
    }

    if !states.is_empty() {
        print_progress(states, use_stderr)?;
        if use_stderr {
            eprintln!();
        } else {
            println!();
        }
    }

    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn print_progress(
    states: &HashMap<String, ProgressState>,
    use_stderr: bool,
) -> Result<()> {
    let mut labels = states.keys().collect::<Vec<_>>();
    labels.sort_unstable();

    let line = labels
        .into_iter()
        .filter_map(|label| {
            states.get(label).map(|state| {
                let percentage = if state.total_bytes == 0 {
                    100.0
                } else {
                    (state.processed_bytes as f64 / state.total_bytes as f64)
                        * 100.0
                };
                format!(
                    "{label}: {:>7}/{} bytes {:>5.1}%{}",
                    state.processed_bytes,
                    state.total_bytes,
                    percentage.min(100.0),
                    if state.completed { " done" } else { "" }
                )
            })
        })
        .collect::<Vec<_>>()
        .join(" | ");

    if use_stderr {
        eprint!("\r{line}");
        io::stderr()
            .flush()
            .context("failed to flush progress output")
    } else {
        print!("\r{line}");
        io::stdout()
            .flush()
            .context("failed to flush progress output")
    }
}
