use std::cmp::Ordering;
use std::fmt::Write as FmtWrite;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use tempfile::{Builder, TempPath};

use crate::cli::DiffMode;
use crate::compare::FunctionComparison;

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
    binary1: &Path,
    binary2: &Path,
) -> Result<Vec<PreparedComparison>> {
    let [label1, label2] = temp_file_labels(binary1, binary2);

    comparisons
        .into_iter()
        .map(|comparison| {
            let diff1_contents = comparison.function1.as_ref().map_or_else(
                || format!("missing function: {}\n", comparison.name),
                |function| function.rendered.clone(),
            );
            let diff1_path = write_temp_disassembly(&diff1_contents, &label1)?;
            let diff2_contents = comparison.function2.as_ref().map_or_else(
                || format!("missing function: {}\n", comparison.name),
                |function| function.rendered.clone(),
            );
            let diff2_path = write_temp_disassembly(&diff2_contents, &label2)?;

            Ok(PreparedComparison {
                comparison,
                diff1_path,
                diff2_path,
            })
        })
        .collect()
}

pub(crate) fn temp_file_labels(binary1: &Path, binary2: &Path) -> [String; 2] {
    let basename1 = binary1.file_name().map_or_else(
        || "binary1".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let basename2 = binary2.file_name().map_or_else(
        || "binary2".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );

    if basename1 == basename2 {
        [format!("LEFT-{basename1}"), format!("RIGHT-{basename2}")]
    } else {
        [basename1, basename2]
    }
}

pub(crate) fn write_temp_disassembly(
    contents: &str,
    label: &str,
) -> Result<TempPath> {
    let prefix = format!("cgdiff-{label}-");
    let mut file = Builder::new()
        .prefix(&prefix)
        .suffix(".s")
        .tempfile()
        .context("failed to create temp disassembly file")?;
    file.write_all(contents.as_bytes())
        .context("failed to write temp disassembly file")?;
    Ok(file.into_temp_path())
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

    if show_presence_columns {
        writeln!(
            writer,
            "{:<function_width$}  {:>8}  {:>9}  {:>8}  {:>4}  {:>4}",
            headers[0],
            headers[1],
            headers[2],
            headers[3],
            headers[4],
            headers[5],
        )?;
    } else {
        writeln!(
            writer,
            "{:<function_width$}  {:>8}  {:>9}  {:>8}",
            headers[0], headers[1], headers[2], headers[3],
        )?;
    }

    for comparison in sorted {
        let row =
            comparison_table_row(&comparison, diff_mode, show_presence_columns);
        if show_presence_columns {
            writeln!(
                writer,
                "{:<function_width$}  {:>8}  {:>9}  {:>8}  {:>4}  {:>4}",
                row.cells[0],
                row.cells[1],
                row.cells[2],
                row.cells[3],
                row.cells[4],
                row.cells[5],
            )?;
        } else {
            writeln!(
                writer,
                "{:<function_width$}  {:>8}  {:>9}  {:>8}",
                row.cells[0], row.cells[1], row.cells[2], row.cells[3],
            )?;
        }
    }

    Ok(())
}

pub(crate) fn dump_comparison_diff(
    mut writer: impl Write,
    comparisons: &[FunctionComparison],
    diff_mode: DiffMode,
    binary1: &Path,
    binary2: &Path,
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
    write_unified_diff(&mut writer, binary1, binary2, &left, &right)
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
) -> Result<()> {
    if left == right {
        return Ok(());
    }

    let left_path = diff_path("a", binary1);
    let right_path = diff_path("b", binary2);
    let left_lines = split_diff_lines(left);
    let right_lines = split_diff_lines(right);

    writeln!(writer, "diff --git {left_path} {right_path}")?;
    writeln!(writer, "--- {left_path}")?;
    writeln!(writer, "+++ {right_path}")?;
    writeln!(
        writer,
        "@@ -{} +{} @@",
        unified_range(left_lines.len()),
        unified_range(right_lines.len())
    )?;

    for line in diff_lines(&left_lines, &right_lines) {
        match line {
            DiffLine::Context(text) => writeln!(writer, " {text}")?,
            DiffLine::Delete(text) => writeln!(writer, "-{text}")?,
            DiffLine::Insert(text) => writeln!(writer, "+{text}")?,
        }
    }

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
