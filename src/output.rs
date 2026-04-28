use std::cmp::Ordering;
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

pub(crate) fn dump_comparisons(
    mut writer: impl Write,
    comparisons: &[FunctionComparison],
    diff_mode: DiffMode,
) -> Result<()> {
    let mut sorted = comparisons.to_vec();
    sort_function_comparisons(&mut sorted, diff_mode);

    let show_presence_columns = sorted
        .iter()
        .any(|comparison| !comparison.is_present_in_both());
    let function_width = sorted
        .iter()
        .map(|comparison| comparison.name.len())
        .max()
        .unwrap_or("Function".len())
        .max("Function".len());

    if show_presence_columns {
        writeln!(
            writer,
            "{:<function_width$}  {:>8}  {:>8}  {:>8}  {:>8}  {:>4}  {:>4}",
            "Function",
            diff_mode.label(),
            "combined",
            "count",
            "ops",
            "Bin1",
            "Bin2",
        )?;
    } else {
        writeln!(
            writer,
            "{:<function_width$}  {:>8}  {:>8}  {:>8}  {:>8}",
            "Function",
            diff_mode.label(),
            "combined",
            "count",
            "ops",
        )?;
    }

    for comparison in sorted {
        if show_presence_columns {
            writeln!(
                writer,
                "{:<function_width$}  {:>8.3}  {:>8.3}  {:>8.3}  {:>8.3}  {:>4}  {:>4}",
                comparison.name,
                diff_mode.score(&comparison),
                comparison.combined_score,
                comparison.count_score,
                comparison.order_score,
                yes_or_no(comparison.function1.is_some()),
                yes_or_no(comparison.function2.is_some()),
            )?;
        } else {
            writeln!(
                writer,
                "{:<function_width$}  {:>8.3}  {:>8.3}  {:>8.3}  {:>8.3}",
                comparison.name,
                diff_mode.score(&comparison),
                comparison.combined_score,
                comparison.count_score,
                comparison.order_score,
            )?;
        }
    }

    Ok(())
}

const fn yes_or_no(present: bool) -> &'static str {
    if present { "yes" } else { "no" }
}
