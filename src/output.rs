use std::cmp::Ordering;
use std::fmt::Write as FmtWrite;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use ratatui::style::Color;
use tempfile::{Builder, TempPath};

use crate::cli::DiffMode;
use crate::compare::FunctionComparison;
use crate::diff_view::{
    DIFF_PREFIX_WIDTH, DiffDisplayLine, DiffLineKind,
    SIDE_BY_SIDE_GUTTER_WIDTH, TokenClass, side_by_side_lines, tokenize_asm,
};
use crate::theme::SyntaxTheme;

const MAX_TEMP_FUNCTION_COMPONENT_LEN: usize = 160;

/// ANSI background tints for added and removed unified-diff lines. They mirror
/// the built-in TUI diff viewer so paged and interactive output stay aligned.
const REMOVED_BACKGROUND: Color = Color::Rgb(58, 24, 30);
const ADDED_BACKGROUND: Color = Color::Rgb(8, 48, 28);

/// Styling applied to non-interactive stdio output.
///
/// `color` is enabled only when stdout is a terminal; piped output stays plain
/// so it can be parsed or redirected without escape sequences.
#[derive(Clone, Copy)]
pub(crate) struct RenderStyle<'a> {
    pub(crate) color: bool,
    pub(crate) theme: &'a SyntaxTheme,
}

fn ansi_wrap(text: &str, codes: &str) -> String {
    format!("\u{1b}[{codes}m{text}\u{1b}[0m")
}

/// Picks a row color for a similarity score: red flags large differences,
/// yellow moderate ones, and green near-identical functions.
const fn score_color_code(score: f64) -> &'static str {
    if score < 0.5 {
        "31"
    } else if score < 0.9 {
        "33"
    } else {
        "32"
    }
}

pub(crate) struct PreparedComparison {
    pub(crate) comparison: FunctionComparison,
    pub(crate) diff1_path: TempPath,
    pub(crate) diff2_path: TempPath,
}

pub(crate) struct ComparisonTableRow {
    pub(crate) cells: Vec<String>,
}

pub(crate) fn prepare_comparisons(
    comparisons: Vec<FunctionComparison>,
) -> Result<Vec<PreparedComparison>> {
    comparisons
        .into_iter()
        .map(|comparison| {
            let diff1_contents = comparison.function1.as_ref().map_or_else(
                || format!("missing function: {}\n", comparison.name),
                |function| function.rendered.clone(),
            );
            let diff1_path =
                write_temp_disassembly(&diff1_contents, &comparison.name, "a")?;
            let diff2_contents = comparison.function2.as_ref().map_or_else(
                || format!("missing function: {}\n", comparison.name),
                |function| function.rendered.clone(),
            );
            let diff2_path =
                write_temp_disassembly(&diff2_contents, &comparison.name, "b")?;

            Ok(PreparedComparison {
                comparison,
                diff1_path,
                diff2_path,
            })
        })
        .collect()
}

pub(crate) fn write_temp_disassembly(
    contents: &str,
    function_name: &str,
    side: &str,
) -> Result<TempPath> {
    let function_name = temp_function_component(function_name);
    let prefix = format!("cgdiff-{function_name}.{side}.");
    let mut file = Builder::new()
        .prefix(&prefix)
        .suffix(".s")
        .tempfile()
        .context("failed to create temp disassembly file")?;
    file.write_all(contents.as_bytes())
        .context("failed to write temp disassembly file")?;
    Ok(file.into_temp_path())
}

pub(crate) fn temp_function_component(function_name: &str) -> String {
    let mut component = String::new();
    let mut last_was_replacement = false;

    for character in function_name.chars() {
        if component.len() >= MAX_TEMP_FUNCTION_COMPONENT_LEN {
            break;
        }

        if character.is_ascii_alphanumeric()
            || matches!(character, '.' | '-' | '_')
        {
            component.push(character);
            last_was_replacement = false;
        } else if !last_was_replacement && !component.is_empty() {
            component.push('_');
            last_was_replacement = true;
        }
    }

    let component = component.trim_matches(['.', '_', '-']).to_owned();
    if component.is_empty() {
        "function".to_owned()
    } else {
        component
    }
}

pub(crate) fn sort_comparisons(
    items: &mut [PreparedComparison],
    diff_mode: DiffMode,
) {
    items.sort_by(|left, right| {
        let left_score = diff_mode.score(&left.comparison);
        let right_score = diff_mode.score(&right.comparison);

        left_score
            .partial_cmp(&right_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.comparison.name.cmp(&right.comparison.name))
    });
}

pub(crate) fn sort_function_comparisons(
    items: &mut [FunctionComparison],
    diff_mode: DiffMode,
) {
    items.sort_by(|left, right| {
        let left_score = diff_mode.score(left);
        let right_score = diff_mode.score(right);

        left_score
            .partial_cmp(&right_score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.name.cmp(&right.name))
    });
}

pub(crate) fn comparison_table_headers(
    diff_mode: DiffMode,
    show_presence_columns: bool,
) -> Vec<String> {
    let mut headers = vec![
        "Function".to_owned(),
        "Left ops".to_owned(),
        "Right ops".to_owned(),
        diff_mode.label().to_owned(),
    ];
    if show_presence_columns {
        headers.extend(["Bin1".to_owned(), "Bin2".to_owned()]);
    }
    headers
}

pub(crate) fn comparison_table_row(
    comparison: &FunctionComparison,
    diff_mode: DiffMode,
    show_presence_columns: bool,
) -> ComparisonTableRow {
    let mut cells = vec![
        comparison.name.clone(),
        comparison.left_op_count().to_string(),
        comparison.right_op_count().to_string(),
        format!("{:.3}", diff_mode.score(comparison)),
    ];
    if show_presence_columns {
        cells.extend([
            yes_or_no(comparison.function1.is_some()).to_owned(),
            yes_or_no(comparison.function2.is_some()).to_owned(),
        ]);
    }
    ComparisonTableRow { cells }
}

pub(crate) fn comparison_table_shows_presence_columns<'a>(
    comparisons: impl IntoIterator<Item = &'a FunctionComparison>,
) -> bool {
    comparisons
        .into_iter()
        .any(|comparison| !comparison.is_present_in_both())
}

pub(crate) fn dump_comparisons(
    mut writer: impl Write,
    comparisons: &[FunctionComparison],
    diff_mode: DiffMode,
    style: RenderStyle<'_>,
) -> Result<()> {
    let mut sorted = comparisons.to_vec();
    sort_function_comparisons(&mut sorted, diff_mode);

    let show_presence_columns =
        comparison_table_shows_presence_columns(&sorted);
    let function_width = sorted
        .iter()
        .map(|comparison| comparison.name.len())
        .max()
        .unwrap_or("Function".len())
        .max("Function".len());
    let headers = comparison_table_headers(diff_mode, show_presence_columns);

    let header_line =
        format_table_row(&headers, function_width, show_presence_columns);
    if style.color {
        writeln!(writer, "{}", ansi_wrap(&header_line, "1"))?;
    } else {
        writeln!(writer, "{header_line}")?;
    }

    for comparison in sorted {
        let row =
            comparison_table_row(&comparison, diff_mode, show_presence_columns);
        let line =
            format_table_row(&row.cells, function_width, show_presence_columns);
        if style.color {
            let codes = score_color_code(diff_mode.score(&comparison));
            writeln!(writer, "{}", ansi_wrap(&line, codes))?;
        } else {
            writeln!(writer, "{line}")?;
        }
    }

    Ok(())
}

fn format_table_row(
    cells: &[String],
    function_width: usize,
    show_presence_columns: bool,
) -> String {
    if show_presence_columns {
        format!(
            "{:<function_width$}  {:>8}  {:>9}  {:>8}  {:>4}  {:>4}",
            cells[0], cells[1], cells[2], cells[3], cells[4], cells[5],
        )
    } else {
        format!(
            "{:<function_width$}  {:>8}  {:>9}  {:>8}",
            cells[0], cells[1], cells[2], cells[3],
        )
    }
}

pub(crate) fn dump_comparison_diff(
    mut writer: impl Write,
    comparisons: &[FunctionComparison],
    diff_mode: DiffMode,
    binary1: &Path,
    binary2: &Path,
    style: RenderStyle<'_>,
) -> Result<()> {
    let mut sorted = comparisons.to_vec();
    sort_function_comparisons(&mut sorted, diff_mode);

    let left = aggregate_rendered_functions(
        &sorted,
        ComparisonSide::Left,
        SideLabel::Left,
    );
    let right = aggregate_rendered_functions(
        &sorted,
        ComparisonSide::Right,
        SideLabel::Right,
    );
    write_unified_diff(&mut writer, binary1, binary2, &left, &right, style)
}

pub(crate) fn dump_comparison_side_by_side_diff(
    mut writer: impl Write,
    comparisons: &[FunctionComparison],
    diff_mode: DiffMode,
    diff_context: usize,
    width: usize,
    style: RenderStyle<'_>,
) -> Result<()> {
    let mut sorted = comparisons.to_vec();
    sort_function_comparisons(&mut sorted, diff_mode);

    let Some(text_width) = side_by_side_text_width(width) else {
        writeln!(writer, "Terminal is too narrow for side-by-side diff.")?;
        return Ok(());
    };

    for (index, comparison) in sorted.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
        }

        write_side_by_side_header(&mut writer, comparison, diff_mode, style)?;

        let left = comparison.function1.as_ref().map_or_else(
            || format!("missing left function: {}\n", comparison.name),
            |function| function.rendered.clone(),
        );
        let right = comparison.function2.as_ref().map_or_else(
            || format!("missing right function: {}\n", comparison.name),
            |function| function.rendered.clone(),
        );

        for line in side_by_side_lines(&left, &right, diff_context) {
            write_side_by_side_column(
                &mut writer,
                line.left.as_ref(),
                text_width,
                style,
            )?;
            write_side_by_side_gutter(&mut writer, style)?;
            write_side_by_side_column(
                &mut writer,
                line.right.as_ref(),
                text_width,
                style,
            )?;
            writeln!(writer)?;
        }
    }

    Ok(())
}

fn side_by_side_text_width(width: usize) -> Option<usize> {
    let fixed_width = (DIFF_PREFIX_WIDTH * 2) + SIDE_BY_SIDE_GUTTER_WIDTH;
    width
        .checked_sub(fixed_width)
        .map(|remaining| remaining / 2)
        .filter(|text_width| *text_width > 0)
}

fn write_side_by_side_header(
    writer: &mut impl Write,
    comparison: &FunctionComparison,
    diff_mode: DiffMode,
    style: RenderStyle<'_>,
) -> Result<()> {
    let header = format!(
        "@@ {} ({}, {:.3}) @@",
        comparison.name,
        diff_mode.label(),
        diff_mode.score(comparison)
    );
    write_meta_line(writer, style, &header, "36")
}

fn write_side_by_side_gutter(
    writer: &mut impl Write,
    style: RenderStyle<'_>,
) -> Result<()> {
    if style.color {
        write!(writer, "{}", ansi_wrap(" | ", "90"))?;
    } else {
        write!(writer, " | ")?;
    }
    Ok(())
}

fn write_side_by_side_column(
    writer: &mut impl Write,
    line: Option<&DiffDisplayLine>,
    text_width: usize,
    style: RenderStyle<'_>,
) -> Result<()> {
    let Some(line) = line else {
        write_missing_side_by_side_column(writer, text_width, style)?;
        return Ok(());
    };

    let visible_text = visible_text(&line.text, text_width);
    let visible_width = visible_text.chars().count();
    write_diff_prefix(writer, line.kind, style)?;
    write_highlighted_diff_text(
        writer,
        &visible_text,
        line.kind.background(),
        style,
    )?;
    write_padding(
        writer,
        text_width.saturating_sub(visible_width),
        line.kind.background(),
        style,
    )
}

fn write_missing_side_by_side_column(
    writer: &mut impl Write,
    text_width: usize,
    style: RenderStyle<'_>,
) -> Result<()> {
    if style.color {
        write!(writer, "{}", ansi_wrap("  ", "90"))?;
    } else {
        write!(writer, "  ")?;
    }
    write_padding(writer, text_width, None, style)
}

fn write_diff_prefix(
    writer: &mut impl Write,
    kind: DiffLineKind,
    style: RenderStyle<'_>,
) -> Result<()> {
    if style.color {
        write!(
            writer,
            "{}",
            ansi_wrap(kind.prefix(), diff_prefix_codes(kind))
        )?;
    } else {
        write!(writer, "{}", kind.prefix())?;
    }
    Ok(())
}

const fn diff_prefix_codes(kind: DiffLineKind) -> &'static str {
    match kind {
        DiffLineKind::Context | DiffLineKind::Fold => "90",
        DiffLineKind::Added => "1;32",
        DiffLineKind::Removed => "1;31",
        DiffLineKind::ChangedAdded | DiffLineKind::ChangedRemoved => "1;33",
    }
}

fn write_highlighted_diff_text(
    writer: &mut impl Write,
    text: &str,
    background: Option<Color>,
    style: RenderStyle<'_>,
) -> Result<()> {
    if !style.color {
        write!(writer, "{text}")?;
        return Ok(());
    }

    for token in tokenize_asm(text) {
        write!(
            writer,
            "{}",
            style.theme.ansi_paint(token.class, &token.text, background)
        )?;
    }
    Ok(())
}

fn write_padding(
    writer: &mut impl Write,
    width: usize,
    background: Option<Color>,
    style: RenderStyle<'_>,
) -> Result<()> {
    if width == 0 {
        return Ok(());
    }

    let padding = " ".repeat(width);
    if style.color {
        write!(
            writer,
            "{}",
            style
                .theme
                .ansi_paint(TokenClass::Plain, &padding, background)
        )?;
    } else {
        write!(writer, "{padding}")?;
    }
    Ok(())
}

fn visible_text(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

const fn yes_or_no(present: bool) -> &'static str {
    if present { "yes" } else { "no" }
}

#[derive(Clone, Copy)]
enum ComparisonSide {
    Left,
    Right,
}

#[derive(Clone, Copy)]
enum SideLabel {
    Left,
    Right,
}

fn aggregate_rendered_functions(
    comparisons: &[FunctionComparison],
    side: ComparisonSide,
    missing_side: SideLabel,
) -> String {
    let mut output = String::new();
    for comparison in comparisons {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }

        let rendered = match side {
            ComparisonSide::Left => comparison
                .function1
                .as_ref()
                .map(|function| function.rendered.as_str()),
            ComparisonSide::Right => comparison
                .function2
                .as_ref()
                .map(|function| function.rendered.as_str()),
        };

        if let Some(rendered) = rendered {
            output.push_str(rendered);
        } else {
            writeln!(
                output,
                "missing {} function: {}",
                missing_side.label(),
                comparison.name
            )
            .expect("writing to string should not fail");
        }
    }

    output
}

impl SideLabel {
    const fn label(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

fn write_unified_diff(
    writer: &mut impl Write,
    binary1: &Path,
    binary2: &Path,
    left: &str,
    right: &str,
    style: RenderStyle<'_>,
) -> Result<()> {
    if left == right {
        return Ok(());
    }

    let left_path = diff_path("a", binary1);
    let right_path = diff_path("b", binary2);
    let left_lines = split_diff_lines(left);
    let right_lines = split_diff_lines(right);

    write_meta_line(
        writer,
        style,
        &format!("diff --git {left_path} {right_path}"),
        "1",
    )?;
    write_meta_line(writer, style, &format!("--- {left_path}"), "1")?;
    write_meta_line(writer, style, &format!("+++ {right_path}"), "1")?;
    write_meta_line(
        writer,
        style,
        &format!(
            "@@ -{} +{} @@",
            unified_range(left_lines.len()),
            unified_range(right_lines.len())
        ),
        "36",
    )?;

    for line in diff_lines(&left_lines, &right_lines) {
        match line {
            DiffLine::Context(text) => {
                write_diff_content_line(writer, " ", "", text, None, style)?;
            }
            DiffLine::Delete(text) => write_diff_content_line(
                writer,
                "-",
                "1;31",
                text,
                Some(REMOVED_BACKGROUND),
                style,
            )?,
            DiffLine::Insert(text) => write_diff_content_line(
                writer,
                "+",
                "1;32",
                text,
                Some(ADDED_BACKGROUND),
                style,
            )?,
        }
    }

    Ok(())
}

fn write_meta_line(
    writer: &mut impl Write,
    style: RenderStyle<'_>,
    text: &str,
    codes: &str,
) -> Result<()> {
    if style.color {
        writeln!(writer, "{}", ansi_wrap(text, codes))?;
    } else {
        writeln!(writer, "{text}")?;
    }
    Ok(())
}

fn write_diff_content_line(
    writer: &mut impl Write,
    marker: &str,
    marker_codes: &str,
    text: &str,
    background: Option<Color>,
    style: RenderStyle<'_>,
) -> Result<()> {
    if !style.color {
        writeln!(writer, "{marker}{text}")?;
        return Ok(());
    }

    if marker_codes.is_empty() {
        write!(writer, "{marker}")?;
    } else {
        write!(writer, "{}", ansi_wrap(marker, marker_codes))?;
    }
    for token in tokenize_asm(text) {
        write!(
            writer,
            "{}",
            style.theme.ansi_paint(token.class, &token.text, background)
        )?;
    }
    writeln!(writer)?;
    Ok(())
}

fn diff_path(prefix: &str, path: &Path) -> String {
    let path = path.display().to_string();
    format!("{prefix}/{}", path.trim_start_matches('/'))
}

fn unified_range(line_count: usize) -> String {
    if line_count == 0 {
        "0,0".to_owned()
    } else if line_count == 1 {
        "1".to_owned()
    } else {
        format!("1,{line_count}")
    }
}

fn split_diff_lines(contents: &str) -> Vec<&str> {
    contents.lines().collect()
}

#[derive(Debug, Eq, PartialEq)]
enum DiffLine<'a> {
    Context(&'a str),
    Delete(&'a str),
    Insert(&'a str),
}

fn diff_lines<'a>(left: &[&'a str], right: &[&'a str]) -> Vec<DiffLine<'a>> {
    let rows = left.len() + 1;
    let columns = right.len() + 1;
    let mut lcs_lengths = vec![0; rows * columns];

    for left_index in (0..left.len()).rev() {
        for right_index in (0..right.len()).rev() {
            let cell = left_index * columns + right_index;
            lcs_lengths[cell] = if left[left_index] == right[right_index] {
                lcs_lengths[(left_index + 1) * columns + right_index + 1] + 1
            } else {
                lcs_lengths[(left_index + 1) * columns + right_index]
                    .max(lcs_lengths[left_index * columns + right_index + 1])
            };
        }
    }

    let mut output = Vec::new();
    let mut left_index = 0;
    let mut right_index = 0;
    while left_index < left.len() && right_index < right.len() {
        if left[left_index] == right[right_index] {
            output.push(DiffLine::Context(left[left_index]));
            left_index += 1;
            right_index += 1;
        } else if lcs_lengths[(left_index + 1) * columns + right_index]
            >= lcs_lengths[left_index * columns + right_index + 1]
        {
            output.push(DiffLine::Delete(left[left_index]));
            left_index += 1;
        } else {
            output.push(DiffLine::Insert(right[right_index]));
            right_index += 1;
        }
    }
    output.extend(left[left_index..].iter().map(|line| DiffLine::Delete(line)));
    output.extend(
        right[right_index..]
            .iter()
            .map(|line| DiffLine::Insert(line)),
    );

    output
}
