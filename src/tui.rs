use std::io;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
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

use crate::cli::DiffMode;
use crate::filter::SearchFilter;
use crate::output::{PreparedComparison, sort_comparisons};

const EDITOR_FILE1_PLACEHOLDER: &str = "{file1}";
const EDITOR_FILE2_PLACEHOLDER: &str = "{file2}";

enum Overlay {
    Help,
    Info,
}

#[derive(Debug)]
struct SearchState {
    buffer: String,
    previous_query: String,
}
pub(crate) struct App {
    pub(crate) items: Vec<PreparedComparison>,
    pub(crate) filtered_indices: Vec<usize>,
    diff_mode: DiffMode,
    table_state: TableState,
    should_quit: bool,
    overlay: Option<Overlay>,
    include_unique_functions: bool,
    include_identical_functions: bool,
    pub(crate) search_query: String,
    search_filter: SearchFilter,
    search_state: Option<SearchState>,
}

impl App {
    pub(crate) fn new(
        mut items: Vec<PreparedComparison>,
        diff_mode: DiffMode,
        include_unique_functions: bool,
        include_identical_functions: bool,
        initial_search_query: String,
    ) -> Self {
        sort_comparisons(&mut items, diff_mode);
        let mut app = Self {
            items,
            filtered_indices: Vec::new(),
            diff_mode,
            table_state: TableState::default(),
            should_quit: false,
            overlay: None,
            include_unique_functions,
            include_identical_functions,
            search_query: initial_search_query,
            search_filter: SearchFilter::Empty,
            search_state: None,
        };
        app.rebuild_filter(None);
        if !app.filtered_indices.is_empty() {
            app.table_state.select(Some(0));
        }

        app
    }

    pub(crate) fn selected(&self) -> Option<&PreparedComparison> {
        self.table_state
            .selected()
            .and_then(|index| self.filtered_indices.get(index))
            .and_then(|index| self.items.get(*index))
    }

    pub(crate) fn next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }

        let next_index = match self.table_state.selected() {
            Some(index) if index + 1 < self.filtered_indices.len() => index + 1,
            _ => 0,
        };
        self.table_state.select(Some(next_index));
    }

    pub(crate) fn previous(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }

        let previous_index = match self.table_state.selected() {
            Some(0) | None => self.filtered_indices.len() - 1,
            Some(index) => index - 1,
        };
        self.table_state.select(Some(previous_index));
    }

    pub(crate) fn resort(&mut self, diff_mode: DiffMode) {
        let selected_name =
            self.selected().map(|item| item.comparison.name.clone());
        self.diff_mode = diff_mode;
        sort_comparisons(&mut self.items, diff_mode);
        self.rebuild_filter(selected_name.as_deref());
    }

    pub(crate) fn toggle_details(&mut self) {
        if self.selected().is_some() {
            self.overlay = match self.overlay {
                Some(Overlay::Info) => None,
                _ => Some(Overlay::Info),
            };
        }
    }

    pub(crate) const fn toggle_help(&mut self) {
        self.overlay = match self.overlay {
            Some(Overlay::Help) => None,
            _ => Some(Overlay::Help),
        };
    }

    pub(crate) const fn close_overlay(&mut self) -> bool {
        if self.overlay.is_some() {
            self.overlay = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn start_search(&mut self) {
        self.search_state = Some(SearchState {
            buffer: self.search_query.clone(),
            previous_query: self.search_query.clone(),
        });
    }

    fn search_buffer_mut(&mut self) -> Option<&mut String> {
        self.search_state.as_mut().map(|state| &mut state.buffer)
    }

    pub(crate) fn append_search_char(&mut self, character: char) {
        if let Some(buffer) = self.search_buffer_mut() {
            buffer.push(character);
            self.apply_search_buffer();
        }
    }

    pub(crate) fn pop_search_char(&mut self) {
        if let Some(buffer) = self.search_buffer_mut() {
            buffer.pop();
            self.apply_search_buffer();
        }
    }

    pub(crate) fn apply_search_buffer(&mut self) {
        let selected_name =
            self.selected().map(|item| item.comparison.name.clone());
        if let Some(state) = &self.search_state {
            self.search_query = state.buffer.clone();
        }
        self.rebuild_filter(selected_name.as_deref());
    }

    pub(crate) fn confirm_search(&mut self) {
        self.search_state = None;
    }

    pub(crate) fn cancel_search(&mut self) {
        if let Some(state) = self.search_state.take() {
            self.search_query = state.previous_query;
            self.rebuild_filter(None);
        }
    }

    pub(crate) const fn is_searching(&self) -> bool {
        self.search_state.is_some()
    }

    pub(crate) fn search_prompt(&self) -> String {
        self.search_state.as_ref().map_or_else(
            || self.search_query.clone(),
            |state| state.buffer.clone(),
        )
    }

    pub(crate) const fn search_error(&self) -> Option<&str> {
        self.search_filter.error_message()
    }

    pub(crate) const fn visible_count(&self) -> usize {
        self.filtered_indices.len()
    }

    pub(crate) fn rebuild_filter(&mut self, selected_name: Option<&str>) {
        self.search_filter = SearchFilter::compile(&self.search_query);
        self.filtered_indices = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                self.search_filter.matches(&item.comparison.name)
            })
            .map(|(index, _)| index)
            .collect();

        let selected_index = selected_name.and_then(|name| {
            self.filtered_indices
                .iter()
                .position(|index| self.items[*index].comparison.name == name)
        });

        let fallback_index = (!self.filtered_indices.is_empty()).then_some(0);
        self.table_state.select(selected_index.or(fallback_index));
    }
}
pub(crate) fn run_tui(
    items: Vec<PreparedComparison>,
    diff_mode: DiffMode,
    include_unique_functions: bool,
    include_identical_functions: bool,
    initial_search_query: &str,
    editor: &str,
) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let mut app = App::new(
        items,
        diff_mode,
        include_unique_functions,
        include_identical_functions,
        initial_search_query.to_owned(),
    );

    loop {
        terminal.draw(|frame| draw(frame, &mut app))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(250))
            .context("failed polling terminal events")?
        {
            match event::read().context("failed reading terminal event")? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.is_searching() {
                        match key.code {
                            KeyCode::Esc => app.cancel_search(),
                            KeyCode::Enter => app.confirm_search(),
                            KeyCode::Backspace => app.pop_search_char(),
                            KeyCode::Char(character) => {
                                app.append_search_char(character);
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Esc if !app.close_overlay() => {
                                app.should_quit = true;
                            }
                            KeyCode::Char('q') => app.should_quit = true,
                            KeyCode::Down | KeyCode::Char('j') => app.next(),
                            KeyCode::Up | KeyCode::Char('k') => app.previous(),
                            KeyCode::Char('1') => {
                                app.resort(DiffMode::Combined);
                            }
                            KeyCode::Char('2') => app.resort(DiffMode::Count),
                            KeyCode::Char('3') => app.resort(DiffMode::Order),
                            KeyCode::Char('i' | 'I') => {
                                app.toggle_details();
                            }
                            KeyCode::Char('/') => app.start_search(),
                            KeyCode::Char('?') => app.toggle_help(),
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
                        }
                    }
                }
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

    match app.overlay {
        Some(Overlay::Help) => draw_help(frame),
        Some(Overlay::Info) => draw_details_popup(frame, app.selected()),
        None => {}
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
            Span::raw("  "),
            Span::raw(format!(
                "unique: {}",
                if app.include_unique_functions {
                    "shown"
                } else {
                    "hidden"
                }
            )),
            Span::raw("  "),
            Span::raw(format!(
                "identical: {}",
                if app.include_identical_functions {
                    "shown"
                } else {
                    "hidden"
                }
            )),
            Span::raw("  "),
            Span::raw(format!(
                "filter: {}",
                if app.search_query.is_empty() {
                    "(none)"
                } else {
                    app.search_query.as_str()
                }
            )),
        ]),
        Line::from(format!(
            "selected: {selected}  visible: {}/{}",
            app.visible_count(),
            app.items.len()
        )),
    ])
    .block(Block::default().title("Summary").borders(Borders::ALL));

    frame.render_widget(header, area);
}

fn draw_body(frame: &mut ratatui::Frame<'_>, area: Rect, app: &mut App) {
    draw_table(frame, area, app);
}

fn draw_table(frame: &mut ratatui::Frame<'_>, area: Rect, app: &mut App) {
    let show_presence_columns = app.include_unique_functions;
    let table = if show_presence_columns {
        let rows = app.filtered_indices.iter().map(|index| {
            let item = &app.items[*index];
            Row::new([
                Cell::from(item.comparison.name.clone()),
                Cell::from(item.comparison.left_op_count().to_string()),
                Cell::from(item.comparison.right_op_count().to_string()),
                Cell::from(format!(
                    "{:.3}",
                    app.diff_mode.score(&item.comparison)
                )),
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
            Constraint::Percentage(50),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(10),
            Constraint::Length(7),
            Constraint::Length(7),
        ];

        Table::new(rows, widths).header(
            Row::new([
                "Function",
                "Left ops",
                "Right ops",
                app.diff_mode.label(),
                "Bin1",
                "Bin2",
            ])
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        )
    } else {
        let rows = app.filtered_indices.iter().map(|index| {
            let item = &app.items[*index];
            Row::new([
                Cell::from(item.comparison.name.clone()),
                Cell::from(item.comparison.left_op_count().to_string()),
                Cell::from(item.comparison.right_op_count().to_string()),
                Cell::from(format!(
                    "{:.3}",
                    app.diff_mode.score(&item.comparison)
                )),
            ])
        });

        let widths = [
            Constraint::Percentage(60),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(10),
        ];

        Table::new(rows, widths).header(
            Row::new([
                "Function",
                "Left ops",
                "Right ops",
                app.diff_mode.label(),
            ])
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        )
    }
    .block(Block::default().title("Functions").borders(Borders::ALL))
    .row_highlight_style(Style::default().bg(Color::Blue).fg(Color::Black))
    .highlight_symbol(">> ");

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn detail_lines(selection: Option<&PreparedComparison>) -> Vec<Line<'static>> {
    selection.map_or_else(
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
    )
}

fn draw_details_popup(
    frame: &mut ratatui::Frame<'_>,
    selection: Option<&PreparedComparison>,
) {
    let popup = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, popup);
    let details = Paragraph::new(detail_lines(selection))
        .block(Block::default().title("Info").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(details, popup);
}

fn draw_footer(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let footer_text = if app.is_searching() {
        let mut footer = format!(
            "/{}  Enter apply  Esc cancel  Backspace delete  matches: {}",
            app.search_prompt(),
            app.visible_count()
        );
        if let Some(error) = app.search_error() {
            footer.push_str("  regex error: ");
            footer.push_str(error);
        }
        footer
    } else {
        format!(
            "j/k or arrows move  / filter  Enter diff  i info  1/2/3 resort  ? help  q quit  items: {}",
            app.items.len()
        )
    };
    let footer = Paragraph::new(footer_text)
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
        Line::from("/: filter by substring or /regex/"),
        Line::from("1: sort by combined score"),
        Line::from("2: sort by count score"),
        Line::from("3: sort by ops score"),
        Line::from("i: toggle selection info popup"),
        Line::from("Enter: open diff editor"),
        Line::from(
            "Default view hides unique functions and shared functions that are identical or score 1.000.",
        ),
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

pub(crate) fn launch_editor(
    editor: &str,
    selection: &PreparedComparison,
) -> Result<()> {
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
