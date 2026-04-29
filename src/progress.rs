use std::collections::HashMap;
use std::sync::mpsc;

use anyhow::{Context, Result, anyhow};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

const PROGRESS_RENDER_HZ: u8 = 10;

pub(crate) enum ProgressEvent {
    Started { label: String, total_bytes: u64 },
    Processed { label: String, bytes: u64 },
    Finished { label: String },
}

#[derive(Debug, Default)]
struct ProgressState {
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
    use_stderr: bool,
) -> Result<()> {
    let mut states = HashMap::new();
    let progress_bar = ProgressBar::new(0);
    progress_bar.set_draw_target(progress_target(use_stderr));
    progress_bar.set_style(progress_style()?);
    progress_bar.set_message("Processing binaries");

    while let Ok(event) = progress_rx.recv() {
        apply_progress_event(&mut states, event);
        progress_bar.set_length(total_bytes(&states));
        progress_bar.set_position(processed_bytes(&states));
    }

    if states.is_empty() {
        progress_bar.finish_and_clear();
    } else if all_binaries_completed(&states) {
        progress_bar.set_length(total_bytes(&states));
        progress_bar.set_position(total_bytes(&states));
        progress_bar.finish_with_message("Processed binaries");
    } else {
        progress_bar.abandon_with_message("Stopped processing binaries");
    }

    Ok(())
}

fn progress_target(use_stderr: bool) -> ProgressDrawTarget {
    if use_stderr {
        ProgressDrawTarget::stderr_with_hz(PROGRESS_RENDER_HZ)
    } else {
        ProgressDrawTarget::stdout_with_hz(PROGRESS_RENDER_HZ)
    }
}

fn progress_style() -> Result<ProgressStyle> {
    ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
         {bytes}/{total_bytes} {percent:>3}% {msg}",
    )
    .context("failed to configure progress bar")
    .map(|style| style.progress_chars("=> "))
}

fn apply_progress_event(
    states: &mut HashMap<String, ProgressState>,
    event: ProgressEvent,
) {
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
                state.processed_bytes = bytes.min(state.total_bytes);
            }
        }
        ProgressEvent::Finished { label } => {
            if let Some(state) = states.get_mut(&label) {
                state.processed_bytes = state.total_bytes;
                state.completed = true;
            }
        }
    }
}

fn total_bytes(states: &HashMap<String, ProgressState>) -> u64 {
    states.values().map(|state| state.total_bytes).sum()
}

fn processed_bytes(states: &HashMap<String, ProgressState>) -> u64 {
    states.values().map(|state| state.processed_bytes).sum()
}

fn all_binaries_completed(states: &HashMap<String, ProgressState>) -> bool {
    states.values().all(|state| state.completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn started_events_expand_the_unified_progress_total() {
        let mut states = HashMap::new();

        apply_progress_event(
            &mut states,
            ProgressEvent::Started {
                label: "A old".to_owned(),
                total_bytes: 10,
            },
        );
        apply_progress_event(
            &mut states,
            ProgressEvent::Started {
                label: "B new".to_owned(),
                total_bytes: 20,
            },
        );

        assert_eq!(total_bytes(&states), 30);
        assert_eq!(processed_bytes(&states), 0);
    }

    #[test]
    fn processed_events_update_the_unified_progress_position() {
        let mut states = HashMap::new();
        apply_progress_event(
            &mut states,
            ProgressEvent::Started {
                label: "A old".to_owned(),
                total_bytes: 10,
            },
        );
        apply_progress_event(
            &mut states,
            ProgressEvent::Started {
                label: "B new".to_owned(),
                total_bytes: 20,
            },
        );

        apply_progress_event(
            &mut states,
            ProgressEvent::Processed {
                label: "A old".to_owned(),
                bytes: 6,
            },
        );
        apply_progress_event(
            &mut states,
            ProgressEvent::Processed {
                label: "B new".to_owned(),
                bytes: 100,
            },
        );

        assert_eq!(processed_bytes(&states), 26);
    }

    #[test]
    fn finished_events_complete_that_binary_share_of_the_progress() {
        let mut states = HashMap::new();
        apply_progress_event(
            &mut states,
            ProgressEvent::Started {
                label: "A old".to_owned(),
                total_bytes: 10,
            },
        );
        apply_progress_event(
            &mut states,
            ProgressEvent::Processed {
                label: "A old".to_owned(),
                bytes: 6,
            },
        );

        apply_progress_event(
            &mut states,
            ProgressEvent::Finished {
                label: "A old".to_owned(),
            },
        );

        assert_eq!(processed_bytes(&states), 10);
        assert!(states["A old"].completed);
        assert!(all_binaries_completed(&states));
    }
}
