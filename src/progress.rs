use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

const PROGRESS_RENDER_INTERVAL: Duration = Duration::from_millis(100);

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
    let mut printed_line_count = 0;
    let mut last_rendered_at = None;

    while let Ok(event) = progress_rx.recv_timeout(PROGRESS_RENDER_INTERVAL) {
        let force_render = match event {
            ProgressEvent::Started { label, total_bytes } => {
                states.insert(
                    label,
                    ProgressState {
                        total_bytes,
                        processed_bytes: 0,
                        completed: false,
                    },
                );
                true
            }
            ProgressEvent::Processed { label, bytes } => {
                if let Some(state) = states.get_mut(&label) {
                    state.processed_bytes = bytes;
                }
                false
            }
            ProgressEvent::Finished { label } => {
                if let Some(state) = states.get_mut(&label) {
                    state.processed_bytes = state.total_bytes;
                    state.completed = true;
                }
                true
            }
        };

        let now = Instant::now();
        if should_render_progress(last_rendered_at, force_render, now) {
            printed_line_count =
                print_progress(states, use_stderr, printed_line_count)?;
            last_rendered_at = Some(now);
        }
    }

    if !states.is_empty() {
        print_progress(states, use_stderr, printed_line_count)?;
        if use_stderr {
            eprintln!();
        } else {
            println!();
        }
    }

    Ok(())
}

fn should_render_progress(
    last_rendered_at: Option<Instant>,
    force_render: bool,
    now: Instant,
) -> bool {
    force_render
        || last_rendered_at.is_none_or(|last_rendered_at| {
            now.duration_since(last_rendered_at) >= PROGRESS_RENDER_INTERVAL
        })
}

#[allow(clippy::cast_precision_loss)]
fn progress_lines(states: &HashMap<String, ProgressState>) -> Vec<String> {
    let mut labels = states.keys().collect::<Vec<_>>();
    labels.sort_unstable();

    labels
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
}

fn print_progress(
    states: &HashMap<String, ProgressState>,
    use_stderr: bool,
    previous_line_count: usize,
) -> Result<usize> {
    let lines = progress_lines(states);
    let rendered = render_progress_lines(&lines, previous_line_count);

    if use_stderr {
        eprint!("{rendered}");
        io::stderr()
            .flush()
            .context("failed to flush progress output")?;
    } else {
        print!("{rendered}");
        io::stdout()
            .flush()
            .context("failed to flush progress output")?;
    }

    Ok(lines.len())
}

fn render_progress_lines(
    lines: &[String],
    previous_line_count: usize,
) -> String {
    let mut output = String::new();
    if previous_line_count > 1 {
        write!(output, "\x1b[{}A", previous_line_count - 1)
            .expect("writing to a string must not fail");
    }

    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str("\r\x1b[2K");
        output.push_str(line);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_render_each_binary_on_its_own_line() {
        let states = HashMap::from([
            (
                "B /tmp/relay/ab/new.so".to_owned(),
                ProgressState {
                    total_bytes: 6_275_368,
                    processed_bytes: 6_275_368,
                    completed: true,
                },
            ),
            (
                "A /tmp/relay/ab/old.so".to_owned(),
                ProgressState {
                    total_bytes: 6_276_264,
                    processed_bytes: 6_276_264,
                    completed: true,
                },
            ),
        ]);

        assert_eq!(
            progress_lines(&states),
            vec![
                "A /tmp/relay/ab/old.so: 6276264/6276264 bytes 100.0% done",
                "B /tmp/relay/ab/new.so: 6275368/6275368 bytes 100.0% done",
            ]
        );
    }

    #[test]
    fn repeated_progress_output_rewinds_to_the_first_status_line() {
        let lines = vec![
            "A old:       1/10 bytes  10.0%".to_owned(),
            "B new:       2/20 bytes  10.0%".to_owned(),
        ];

        assert_eq!(
            render_progress_lines(&lines, 2),
            "\x1b[1A\r\x1b[2KA old:       1/10 bytes  10.0%\n\r\x1b[2KB new:       2/20 bytes  10.0%"
        );
    }

    #[test]
    fn processed_progress_renders_only_after_interval() {
        let first_rendered_at = Instant::now();

        assert!(should_render_progress(None, false, first_rendered_at));
        assert!(!should_render_progress(
            Some(first_rendered_at),
            false,
            first_rendered_at + (PROGRESS_RENDER_INTERVAL / 2),
        ));
        assert!(should_render_progress(
            Some(first_rendered_at),
            false,
            first_rendered_at + PROGRESS_RENDER_INTERVAL,
        ));
        assert!(should_render_progress(
            Some(first_rendered_at),
            true,
            first_rendered_at + (PROGRESS_RENDER_INTERVAL / 2),
        ));
    }
}
