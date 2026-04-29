use std::collections::{BTreeSet, HashMap};

use rayon::prelude::*;

use crate::disassembly::{BinaryAnalysis, FunctionDisassembly};
use crate::filter::SearchFilter;

const ORDER_WEIGHT: f64 = 0.70;

#[derive(Clone, Debug)]
pub(crate) struct FunctionComparison {
    pub(crate) name: String,
    pub(crate) function1: Option<FunctionDisassembly>,
    pub(crate) function2: Option<FunctionDisassembly>,
    pub(crate) combined_score: f64,
    pub(crate) count_score: f64,
    pub(crate) order_score: f64,
}

pub(crate) fn build_comparisons(
    analysis_one: &BinaryAnalysis,
    analysis_two: &BinaryAnalysis,
    include_unique_functions: bool,
    include_identical_functions: bool,
    exclude: Option<&SearchFilter>,
    include: Option<&SearchFilter>,
) -> Vec<FunctionComparison> {
    let names = analysis_one
        .functions
        .keys()
        .chain(analysis_two.functions.keys())
        .filter(|name| {
            exclude.is_none_or(|exclude| {
                exclude.is_empty() || !exclude.matches(name)
            })
        })
        .filter(|name| include.is_none_or(|include| include.matches(name)))
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
        .filter(|comparison| {
            include_unique_functions || comparison.is_present_in_both()
        })
        .filter(|comparison| {
            include_identical_functions
                || !comparison.is_effectively_identical()
        })
        .collect()
}

impl FunctionComparison {
    pub(crate) const fn is_present_in_both(&self) -> bool {
        self.function1.is_some() && self.function2.is_some()
    }

    pub(crate) fn left_op_count(&self) -> usize {
        self.function1
            .as_ref()
            .map_or(0, |function| function.instructions.len())
    }

    pub(crate) fn right_op_count(&self) -> usize {
        self.function2
            .as_ref()
            .map_or(0, |function| function.instructions.len())
    }

    pub(crate) fn is_identical(&self) -> bool {
        self.function1
            .as_ref()
            .zip(self.function2.as_ref())
            .is_some_and(|(left, right)| {
                left.normalized_instructions == right.normalized_instructions
            })
    }

    pub(crate) fn has_perfect_similarity(&self) -> bool {
        self.is_present_in_both()
            && (self.combined_score - 1.0).abs() < f64::EPSILON
    }

    pub(crate) fn is_effectively_identical(&self) -> bool {
        self.is_identical() || self.has_perfect_similarity()
    }
}

#[allow(clippy::cast_precision_loss)]
pub(crate) fn weighted_jaccard(left: &[String], right: &[String]) -> f64 {
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
pub(crate) fn order_similarity(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() && right.is_empty() {
        return 1.0;
    }

    let lcs = lcs_len(left, right);
    (2.0 * lcs as f64) / (left.len() + right.len()) as f64
}

pub(crate) fn lcs_len(left: &[String], right: &[String]) -> usize {
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
