#![warn(clippy::all, clippy::nursery, clippy::pedantic)]

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, ValueEnum};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap,
};
use rayon::prelude::*;
use tempfile::{NamedTempFile, TempPath};

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
const DEFAULT_EDITOR: &str = "nvim -d {file1} {file2}";
const ORDER_WEIGHT: f64 = 0.70;
const EDITOR_FILE1_PLACEHOLDER: &str = "{file1}";
const EDITOR_FILE2_PLACEHOLDER: &str = "{file2}";

#[derive(Clone, Debug, Parser)]
#[command(version = VERSION, about = "Compare codegen between two binaries")]
struct Cli {
    /// First binary to compare.
    binary1: PathBuf,
    /// Second binary to compare.
    binary2: PathBuf,
    /// Path to objdump program.
    #[arg(short = 'o', long = "objdump")]
    objdump: Option<PathBuf>,
    /// Command used to launch the diff editor.
    #[arg(short = 'e', long = "editor", default_value = DEFAULT_EDITOR)]
    editor: String,
    /// Sort mode for similarity results.
    #[arg(short = 'd', long = "diff-mode", default_value_t = DiffMode::Combined)]
    diff_mode: DiffMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum DiffMode {
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
    const fn score(self, comparison: &FunctionComparison) -> f64 {
        match self {
            Self::Combined => comparison.combined_score,
            Self::Count => comparison.count_score,
            Self::Order => comparison.order_score,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Combined => "combined",
            Self::Count => "count",
            Self::Order => "order",
        }
    }
}

#[derive(Clone, Debug)]
struct BinaryAnalysis {
    functions: HashMap<String, FunctionDisassembly>,
}

#[derive(Clone, Debug)]
struct FunctionDisassembly {
    instructions: Vec<String>,
    rendered: String,
}

#[derive(Clone, Debug)]
struct FunctionComparison {
    name: String,
    function1: Option<FunctionDisassembly>,
    function2: Option<FunctionDisassembly>,
    combined_score: f64,
    count_score: f64,
    order_score: f64,
}

#[derive(Debug)]
struct PreparedComparison {
    comparison: FunctionComparison,
    diff1_path: TempPath,
    diff2_path: TempPath,
}

#[derive(Debug)]
enum ProgressEvent {
    Started { label: String, total_bytes: u64 },
    Processed { label: String, bytes: u64 },
    Finished { label: String },
}

#[derive(Debug)]
struct ProgressState {
    total_bytes: u64,
    processed_bytes: u64,
    completed: bool,
}

#[derive(Debug)]
struct App {
    items: Vec<PreparedComparison>,
    diff_mode: DiffMode,
    table_state: TableState,
    should_quit: bool,
    help_visible: bool,
}

impl App {
    fn new(mut items: Vec<PreparedComparison>, diff_mode: DiffMode) -> Self {
        sort_comparisons(&mut items, diff_mode);
        let mut table_state = TableState::default();
        if !items.is_empty() {
            table_state.select(Some(0));
        }

        Self {
            items,
            diff_mode,
            table_state,
            should_quit: false,
            help_visible: false,
        }
    }

    fn selected(&self) -> Option<&PreparedComparison> {
        self.table_state
            .selected()
            .and_then(|index| self.items.get(index))
    }

    fn next(&mut self) {
        if self.items.is_empty() {
            return;
        }

        let next_index = match self.table_state.selected() {
            Some(index) if index + 1 < self.items.len() => index + 1,
            _ => 0,
        };
        self.table_state.select(Some(next_index));
    }

    fn previous(&mut self) {
        if self.items.is_empty() {
            return;
        }

        let previous_index = match self.table_state.selected() {
            Some(0) | None => self.items.len() - 1,
            Some(index) => index - 1,
        };
        self.table_state.select(Some(previous_index));
    }

    fn resort(&mut self, diff_mode: DiffMode) {
        self.diff_mode = diff_mode;
        sort_comparisons(&mut self.items, diff_mode);
        if !self.items.is_empty() && self.table_state.selected().is_none() {
            self.table_state.select(Some(0));
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let objdump = select_objdump(cli.objdump.as_deref())?;

    let (progress_tx, progress_rx) = mpsc::channel();
    let binary1 = cli.binary1.clone();
    let binary2 = cli.binary2.clone();
    let objdump_one = objdump.clone();
    let progress_tx_one = progress_tx.clone();
    let progress_tx_two = progress_tx.clone();

    let handle_one = thread::spawn(move || {
        analyze_binary(&objdump_one, &binary1, "binary-1", &progress_tx_one)
    });
    let handle_two = thread::spawn(move || {
        analyze_binary(&objdump, &binary2, "binary-2", &progress_tx_two)
    });
    drop(progress_tx);

    let mut states = HashMap::new();
    render_progress(&progress_rx, &mut states)?;

    let analysis_one = join_analysis(handle_one, "binary-1")?;
    let analysis_two = join_analysis(handle_two, "binary-2")?;

    let comparisons = build_comparisons(&analysis_one, &analysis_two);
    let prepared = prepare_comparisons(comparisons)?;
    run_tui(prepared, cli.diff_mode, &cli.editor)?;

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

fn analyze_binary(
    objdump: &Path,
    binary_path: &Path,
    label: &str,
    progress_tx: &mpsc::Sender<ProgressEvent>,
) -> Result<BinaryAnalysis> {
    let metadata = fs::metadata(binary_path).with_context(|| {
        format!("failed to stat binary {}", binary_path.display())
    })?;
    send_progress_start(progress_tx, label, metadata.len())?;

    let mut child = build_objdump_command(objdump, binary_path)
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn {} for {}",
                objdump.display(),
                binary_path.display()
            )
        })?;
    let stdout = child.stdout.take().context("missing objdump stdout pipe")?;
    let stderr = child.stderr.take().context("missing objdump stderr pipe")?;
    let stderr_handle = spawn_stderr_reader(stderr);
    let functions = parse_objdump_output(
        stdout,
        binary_path,
        label,
        metadata.len(),
        progress_tx,
    )?;

    let output = child.wait().with_context(|| {
        format!("failed waiting on objdump for {}", binary_path.display())
    })?;
    let stderr_output = stderr_handle
        .join()
        .map_err(|_| anyhow!("stderr reader thread panicked"))?
        .context("failed reading objdump stderr")?;

    if !output.success() {
        bail!(
            "objdump failed for {}: {}",
            binary_path.display(),
            stderr_output.trim()
        );
    }

    send_progress_finished(progress_tx, label)?;

    Ok(BinaryAnalysis { functions })
}

fn build_objdump_command(objdump: &Path, binary_path: &Path) -> Command {
    let mut command = Command::new(objdump);
    command
        .arg("--disassemble")
        .arg("--demangle")
        .arg(binary_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn spawn_stderr_reader(
    stderr: std::process::ChildStderr,
) -> thread::JoinHandle<io::Result<String>> {
    thread::spawn(move || -> io::Result<String> {
        let mut buffer = String::new();
        let mut reader = BufReader::new(stderr);
        reader.read_to_string(&mut buffer)?;
        Ok(buffer)
    })
}

fn parse_objdump_output(
    stdout: std::process::ChildStdout,
    binary_path: &Path,
    label: &str,
    total_bytes: u64,
    progress_tx: &mpsc::Sender<ProgressEvent>,
) -> Result<HashMap<String, FunctionDisassembly>> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let mut functions = HashMap::new();
    let mut current_name: Option<String> = None;
    let mut current_lines = Vec::new();
    let mut current_instructions = Vec::new();
    let mut processed_bytes = 0_u64;

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).with_context(|| {
            format!(
                "failed reading objdump output for {}",
                binary_path.display()
            )
        })?;
        if bytes_read == 0 {
            break;
        }

        let bytes_read_u64 = u64::try_from(bytes_read)
            .context("objdump output line length overflowed u64")?;
        processed_bytes = processed_bytes.saturating_add(bytes_read_u64);
        send_progress_processed(
            progress_tx,
            label,
            processed_bytes.min(total_bytes),
        )?;

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(name) = parse_function_header(trimmed) {
            flush_current_function(
                &mut functions,
                &mut current_name,
                &mut current_lines,
                &mut current_instructions,
            );
            current_name = Some(name);
            current_lines.push(trimmed.to_owned());
            continue;
        }

        if current_name.is_some() {
            if let Some(instruction) = parse_instruction_mnemonic(trimmed) {
                current_instructions.push(instruction);
            }
            current_lines.push(trimmed.to_owned());
        }
    }

    flush_current_function(
        &mut functions,
        &mut current_name,
        &mut current_lines,
        &mut current_instructions,
    );

    Ok(functions)
}

fn send_progress_start(
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

fn send_progress_processed(
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

fn send_progress_finished(
    progress_tx: &mpsc::Sender<ProgressEvent>,
    label: &str,
) -> Result<()> {
    progress_tx
        .send(ProgressEvent::Finished {
            label: label.to_owned(),
        })
        .map_err(|_| anyhow!("failed to send progress update"))
}

fn flush_current_function(
    functions: &mut HashMap<String, FunctionDisassembly>,
    current_name: &mut Option<String>,
    current_lines: &mut Vec<String>,
    current_instructions: &mut Vec<String>,
) {
    if let Some(name) = current_name.take() {
        let rendered = current_lines.join("\n");
        let disassembly = FunctionDisassembly {
            instructions: std::mem::take(current_instructions),
            rendered,
        };
        functions.insert(name, disassembly);
        current_lines.clear();
    }
}

fn parse_function_header(line: &str) -> Option<String> {
    let line = line.trim();
    let suffix = ">:";
    let start = line.find('<')?;
    if !line.ends_with(suffix) || start == 0 {
        return None;
    }

    Some(line[start + 1..line.len() - suffix.len()].to_owned())
}

fn parse_instruction_mnemonic(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.ends_with(':') {
        return None;
    }

    let (_address, remainder) = trimmed.split_once(':')?;
    let mnemonic = remainder.split_whitespace().find(|token| {
        token.chars().any(char::is_alphabetic)
            && !token.chars().all(|char| char.is_ascii_hexdigit())
    })?;
    Some(mnemonic.to_owned())
}

fn render_progress(
    progress_rx: &mpsc::Receiver<ProgressEvent>,
    states: &mut HashMap<String, ProgressState>,
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

        print_progress(states)?;
    }

    if !states.is_empty() {
        print_progress(states)?;
        println!();
    }

    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn print_progress(states: &HashMap<String, ProgressState>) -> Result<()> {
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

    print!("\r{line}");
    io::stdout()
        .flush()
        .context("failed to flush progress output")
}

fn build_comparisons(
    analysis_one: &BinaryAnalysis,
    analysis_two: &BinaryAnalysis,
) -> Vec<FunctionComparison> {
    let names = analysis_one
        .functions
        .keys()
        .chain(analysis_two.functions.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    names
        .into_iter()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|name| {
            let function1 = analysis_one.functions.get(&name).cloned();
            let function2 = analysis_two.functions.get(&name).cloned();

            let instructions1 =
                function1.as_ref().map_or_else(Vec::new, |function| {
                    function.instructions.clone()
                });
            let instructions2 =
                function2.as_ref().map_or_else(Vec::new, |function| {
                    function.instructions.clone()
                });

            let count_score = weighted_jaccard(&instructions1, &instructions2);
            let order_score = order_similarity(&instructions1, &instructions2);
            let combined_score = ORDER_WEIGHT
                .mul_add(order_score, (1.0 - ORDER_WEIGHT) * count_score);

            FunctionComparison {
                name,
                function1,
                function2,
                combined_score,
                count_score,
                order_score,
            }
        })
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn weighted_jaccard(left: &[String], right: &[String]) -> f64 {
    let mut counts_left = HashMap::<&str, usize>::new();
    let mut counts_right = HashMap::<&str, usize>::new();

    for item in left {
        *counts_left.entry(item.as_str()).or_default() += 1;
    }
    for item in right {
        *counts_right.entry(item.as_str()).or_default() += 1;
    }

    let keys = counts_left
        .keys()
        .chain(counts_right.keys())
        .copied()
        .collect::<BTreeSet<_>>();

    let (intersection, union) =
        keys.into_iter()
            .fold((0_usize, 0_usize), |(inter, uni), key| {
                let left_count = counts_left.get(key).copied().unwrap_or(0);
                let right_count = counts_right.get(key).copied().unwrap_or(0);
                (
                    inter + left_count.min(right_count),
                    uni + left_count.max(right_count),
                )
            });

    if union == 0 {
        1.0
    } else {
        intersection as f64 / union as f64
    }
}

#[allow(clippy::cast_precision_loss)]
fn order_similarity(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }

    let lcs = lcs_len(left, right);
    (2.0 * lcs as f64) / (left.len() + right.len()) as f64
}

fn lcs_len(left: &[String], right: &[String]) -> usize {
    if left.len() < right.len() {
        return lcs_len(right, left);
    }

    let mut previous = vec![0_usize; right.len() + 1];

    for left_item in left {
        let mut current = Vec::with_capacity(right.len() + 1);
        current.push(0);

        for (index, right_item) in right.iter().enumerate() {
            if left_item == right_item {
                current.push(previous[index] + 1);
            } else {
                current.push(
                    previous[index + 1].max(*current.last().unwrap_or(&0)),
                );
            }
        }

        previous = current;
    }

    previous.last().copied().unwrap_or(0)
}

fn prepare_comparisons(
    comparisons: Vec<FunctionComparison>,
) -> Result<Vec<PreparedComparison>> {
    comparisons
        .into_iter()
        .map(|comparison| {
            let diff1_contents = comparison.function1.as_ref().map_or_else(
                || format!("missing function: {}\n", comparison.name),
                |function| function.rendered.clone(),
            );
            let diff1_path = write_temp_disassembly(&diff1_contents)?;
            let diff2_contents = comparison.function2.as_ref().map_or_else(
                || format!("missing function: {}\n", comparison.name),
                |function| function.rendered.clone(),
            );
            let diff2_path = write_temp_disassembly(&diff2_contents)?;

            Ok(PreparedComparison {
                comparison,
                diff1_path,
                diff2_path,
            })
        })
        .collect()
}

fn write_temp_disassembly(contents: &str) -> Result<TempPath> {
    let mut file = NamedTempFile::new()
        .context("failed to create temp disassembly file")?;
    file.write_all(contents.as_bytes())
        .context("failed to write temp disassembly file")?;
    Ok(file.into_temp_path())
}

fn sort_comparisons(items: &mut [PreparedComparison], diff_mode: DiffMode) {
    items.sort_by(|left, right| {
        let left_score = diff_mode.score(&left.comparison);
        let right_score = diff_mode.score(&right.comparison);

        left_score
            .partial_cmp(&right_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.comparison.name.cmp(&right.comparison.name))
    });
}

fn run_tui(
    items: Vec<PreparedComparison>,
    diff_mode: DiffMode,
    editor: &str,
) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = App::new(items, diff_mode);

    loop {
        terminal.draw(|frame| draw(frame, &mut app))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(250))
            .context("failed polling terminal events")?
        {
            match event::read().context("failed reading terminal event")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key
                    .code
                {
                    KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                    KeyCode::Down | KeyCode::Char('j') => app.next(),
                    KeyCode::Up | KeyCode::Char('k') => app.previous(),
                    KeyCode::Char('1') => app.resort(DiffMode::Combined),
                    KeyCode::Char('2') => app.resort(DiffMode::Count),
                    KeyCode::Char('3') => app.resort(DiffMode::Order),
                    KeyCode::Char('?') => app.help_visible = !app.help_visible,
                    KeyCode::Enter => {
                        if let Some(selection) = app.selected() {
                            restore_terminal(&mut terminal)?;
                            let launch_result =
                                launch_editor(editor, selection);
                            terminal = setup_terminal()?;
                            launch_result?;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    restore_terminal(&mut terminal)
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("failed to initialize terminal")
}

fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to restore cursor")
}

fn draw(frame: &mut ratatui::Frame<'_>, app: &mut App) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    draw_header(frame, vertical[0], app);
    draw_body(frame, vertical[1], app);
    draw_footer(frame, vertical[2], app);

    if app.help_visible {
        draw_help(frame);
    }
}

fn draw_header(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let selected = app
        .selected()
        .map_or("none", |selection| selection.comparison.name.as_str());
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                "cgdiff",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::raw(format!("sort: {}", app.diff_mode.label())),
        ]),
        Line::from(format!("selected: {selected}")),
    ])
    .block(Block::default().title("Summary").borders(Borders::ALL));

    frame.render_widget(header, area);
}

fn draw_body(frame: &mut ratatui::Frame<'_>, area: Rect, app: &mut App) {
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);

    draw_table(frame, horizontal[0], app);
    draw_details(frame, horizontal[1], app.selected());
}

fn draw_table(frame: &mut ratatui::Frame<'_>, area: Rect, app: &mut App) {
    let rows = app.items.iter().map(|item| {
        Row::new([
            Cell::from(item.comparison.name.clone()),
            Cell::from(format!("{:.3}", item.comparison.combined_score)),
            Cell::from(format!("{:.3}", item.comparison.count_score)),
            Cell::from(format!("{:.3}", item.comparison.order_score)),
            Cell::from(if item.comparison.function1.is_some() {
                "yes"
            } else {
                "no"
            }),
            Cell::from(if item.comparison.function2.is_some() {
                "yes"
            } else {
                "no"
            }),
        ])
    });

    let widths = [
        Constraint::Percentage(46),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(7),
    ];
    let table = Table::new(rows, widths)
        .header(
            Row::new([
                "Function", "Combined", "Count", "Order", "Bin1", "Bin2",
            ])
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .block(Block::default().title("Functions").borders(Borders::ALL))
        .row_highlight_style(Style::default().bg(Color::Blue).fg(Color::Black))
        .highlight_symbol(">> ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn draw_details(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    selection: Option<&PreparedComparison>,
) {
    let lines = selection.map_or_else(
        || vec![Line::from("No functions were found to compare.")],
        |selection| {
            let function1 = selection
                .comparison
                .function1
                .as_ref()
                .map_or(0, |function| function.instructions.len());
            let function2 = selection
                .comparison
                .function2
                .as_ref()
                .map_or(0, |function| function.instructions.len());

            vec![
                Line::from(format!("function: {}", selection.comparison.name)),
                Line::from(format!(
                    "combined: {:.4}",
                    selection.comparison.combined_score
                )),
                Line::from(format!(
                    "count:    {:.4}",
                    selection.comparison.count_score
                )),
                Line::from(format!(
                    "order:    {:.4}",
                    selection.comparison.order_score
                )),
                Line::from(""),
                Line::from(format!("binary1 instructions: {function1}")),
                Line::from(format!("binary2 instructions: {function2}")),
                Line::from(""),
                Line::from(format!(
                    "temp files: {} | {}",
                    selection.diff1_path.display(),
                    selection.diff2_path.display()
                )),
                Line::from(""),
                Line::from(
                    "Enter opens the configured editor on the rendered disassembly.",
                ),
            ]
        },
    );

    let details = Paragraph::new(lines)
        .block(Block::default().title("Details").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(details, area);
}

fn draw_footer(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let footer = Paragraph::new(format!(
        "j/k or arrows move  Enter diff  1/2/3 resort  ? help  q quit  items: {}",
        app.items.len()
    ))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, area);
}

fn draw_help(frame: &mut ratatui::Frame<'_>) {
    let popup = centered_rect(60, 40, frame.area());
    frame.render_widget(Clear, popup);
    let help = Paragraph::new(vec![
        Line::from("q / Esc: quit"),
        Line::from("j / Down: next function"),
        Line::from("k / Up: previous function"),
        Line::from("1: sort by combined score"),
        Line::from("2: sort by count score"),
        Line::from("3: sort by order score"),
        Line::from("Enter: open diff editor"),
        Line::from("?: toggle this help"),
    ])
    .block(Block::default().title("Help").borders(Borders::ALL))
    .wrap(Wrap { trim: true });
    frame.render_widget(help, popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn launch_editor(editor: &str, selection: &PreparedComparison) -> Result<()> {
    let file1 = selection.diff1_path.to_string_lossy();
    let file2 = selection.diff2_path.to_string_lossy();
    let rendered = editor
        .replace(EDITOR_FILE1_PLACEHOLDER, &file1)
        .replace(EDITOR_FILE2_PLACEHOLDER, &file2);
    let parts = shlex::split(&rendered)
        .ok_or_else(|| anyhow!("failed to parse editor command"))?;
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| anyhow!("editor command resolved to no executable"))?;

    let status =
        Command::new(program).args(args).status().with_context(|| {
            format!("failed to launch editor command: {rendered}")
        })?;
    if !status.success() {
        bail!("editor command exited with status {status}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        lcs_len, order_similarity, parse_function_header,
        parse_instruction_mnemonic, weighted_jaccard,
    };

    #[test]
    fn parses_function_headers() {
        let name = parse_function_header("0000000000001139 <main>:")
            .expect("expected function name");
        assert_eq!(name, "main");
    }

    #[test]
    fn parses_instruction_mnemonics() {
        let mnemonic = parse_instruction_mnemonic(
            "   113d:\t48 89 e5             \tmov    %rsp,%rbp",
        )
        .expect("expected mnemonic");
        assert_eq!(mnemonic, "mov");
    }

    #[test]
    fn computes_weighted_jaccard() {
        let left = vec!["mov".to_owned(), "call".to_owned(), "call".to_owned()];
        let right = vec!["mov".to_owned(), "call".to_owned(), "jmp".to_owned()];
        let score = weighted_jaccard(&left, &right);
        assert!((score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn computes_lcs_length() {
        let left = vec!["mov".to_owned(), "call".to_owned(), "ret".to_owned()];
        let right = vec!["mov".to_owned(), "jmp".to_owned(), "ret".to_owned()];
        assert_eq!(lcs_len(&left, &right), 2);
    }

    #[test]
    fn computes_order_similarity() {
        let left = vec!["mov".to_owned(), "call".to_owned(), "ret".to_owned()];
        let right = vec!["mov".to_owned(), "jmp".to_owned(), "ret".to_owned()];
        let score = order_similarity(&left, &right);
        assert!((score - (4.0 / 6.0)).abs() < f64::EPSILON);
    }
}
