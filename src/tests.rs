#![cfg(test)]

use crate::cli::DiffMode;
use crate::compare::{
    FunctionComparison, build_comparisons, lcs_len, order_similarity,
    weighted_jaccard,
};
use crate::disassembly::{
    BinaryAnalysis, FunctionBuilder, FunctionDisassembly, ParsedInstruction,
    TargetArchitecture, build_objdump_command_for_arch,
    detect_target_architecture, finalize_function, normalize_instruction_text,
    parse_function_header, parse_instruction_line, parse_instruction_mnemonic,
    parse_instruction_text,
};
use crate::filter::SearchFilter;
use crate::output::{
    PreparedComparison, dump_comparisons, temp_file_labels,
    write_temp_disassembly,
};
use crate::tui::App;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

#[test]
fn parses_function_headers() {
    let name = parse_function_header("0000000000001139 <main>:")
        .expect("expected function name");
    assert_eq!(name, "main");
}

#[test]
fn parses_instruction_mnemonics() {
    let instruction = parse_instruction_text(
        "   113d:\t48 89 e5             \tmov    %rsp,%rbp",
    )
    .expect("expected instruction");
    let mnemonic =
        parse_instruction_mnemonic(&instruction).expect("expected mnemonic");
    assert_eq!(mnemonic, "mov");
}

#[test]
fn parses_instruction_text_without_raw_bytes() {
    let instruction = parse_instruction_text("113d:\tmov    rsp, rbp")
        .expect("expected instruction");
    assert_eq!(instruction, "mov    rsp, rbp");
}

#[test]
fn parses_instruction_address_and_text() {
    let instruction =
        parse_instruction_line("   b7add:\tjne\t0xb7b78 <relayExec+0xc8>")
            .expect("expected instruction");
    let mnemonic = parse_instruction_mnemonic(&instruction.text)
        .expect("expected mnemonic");

    assert_eq!(instruction.address, Some(0xb7add));
    assert_eq!(instruction.text, "jne\t0xb7b78 <relayExec+0xc8>");
    assert_eq!(mnemonic, "jne");
}

#[test]
fn parses_aarch64_instruction_address_and_text() {
    let instruction =
        parse_instruction_line("   1010:\tb.eq\t0x1020 <worker+0x20>")
            .expect("expected instruction");
    let mnemonic = parse_instruction_mnemonic(&instruction.text)
        .expect("expected mnemonic");

    assert_eq!(instruction.address, Some(0x1010));
    assert_eq!(instruction.text, "b.eq\t0x1020 <worker+0x20>");
    assert_eq!(mnemonic, "b.eq");
}

#[test]
fn normalizes_intra_function_jump_targets() {
    let labels = local_labels_for(&[(0x1000, ".L0000"), (0x1010, ".L0001")]);
    let instruction =
        normalize_instruction_text("jne 0x1010 <foo+0x10>", &labels);

    assert_eq!(instruction.text, "jne .L0001");
    assert_eq!(instruction.local_target, Some(0x1010));
}

#[test]
fn normalizes_aarch64_branch_targets() {
    let labels = local_labels_for(&[(0x1000, ".L0000"), (0x1010, ".L0001")]);
    let conditional =
        normalize_instruction_text("b.eq 0x1010 <foo+0x10>", &labels);
    let compare =
        normalize_instruction_text("cbz x0, 0x1010 <foo+0x10>", &labels);

    assert_eq!(conditional.text, "b.eq .L0001");
    assert_eq!(conditional.local_target, Some(0x1010));
    assert_eq!(compare.text, "cbz x0, .L0001");
    assert_eq!(compare.local_target, Some(0x1010));
}

#[test]
fn normalizes_aarch64_symbol_call_targets() {
    let instruction = normalize_instruction_text(
        "bl 0x4dac0 <redisAppendFormattedCommand$plt>",
        &HashMap::new(),
    );

    assert_eq!(instruction.text, "bl sym:redisAppendFormattedCommand$plt");
    assert_eq!(instruction.local_target, None);
}

#[test]
fn normalizes_symbol_call_targets() {
    let instruction = normalize_instruction_text(
        "call 0x4dac0 <redisAppendFormattedCommand$plt>",
        &HashMap::new(),
    );

    assert_eq!(instruction.text, "call sym:redisAppendFormattedCommand$plt");
    assert_eq!(instruction.local_target, None);
}

#[test]
fn normalizes_rip_relative_data_comments() {
    let instruction = normalize_instruction_text(
        "lea rsi, [rip - 0x6c46c] # 0x4b68a <.LC765>",
        &HashMap::new(),
    );

    assert!(!instruction.text.contains("0x4b68a"));
    assert!(!instruction.text.contains("- 0x6c46c"));
    assert!(instruction.text.contains(".LC765"));
}

#[test]
fn preserves_non_target_immediates() {
    let labels = HashMap::new();
    let comparison = normalize_instruction_text("cmp eax, 0x6", &labels);
    let offset =
        normalize_instruction_text("mov rax, qword ptr [rsi + 0x350]", &labels);

    assert_eq!(comparison.text, "cmp eax, 0x6");
    assert_eq!(offset.text, "mov rax, qword ptr [rsi + 0x350]");
}

#[test]
fn rendered_output_strips_instruction_addresses() {
    let function = finalize_function(&function_builder(
        "foo",
        &[
            parsed_instruction(0x1000, "jne 0x1010 <foo+0x10>"),
            parsed_instruction(0x1010, "ret"),
        ],
    ));

    assert!(!function.rendered.contains("1000:"));
    assert!(!function.rendered.contains("1010:"));
    assert!(function.rendered.contains("<foo>:"));
    assert!(function.rendered.contains("jne .L0001"));
}

#[test]
fn identical_detection_ignores_moved_function_addresses() {
    let left = finalize_function(&function_builder(
        "foo",
        &[
            parsed_instruction(0x1000, "jne 0x1010 <foo+0x10>"),
            parsed_instruction(0x1010, "ret"),
        ],
    ));
    let right = finalize_function(&function_builder(
        "foo",
        &[
            parsed_instruction(0x2000, "jne 0x2010 <foo+0x10>"),
            parsed_instruction(0x2010, "ret"),
        ],
    ));

    assert_eq!(left.normalized_instructions, right.normalized_instructions);
}

#[test]
fn builds_gnu_objdump_command_with_intel_syntax() {
    let command = build_objdump_command_for_arch(
        Path::new("objdump"),
        Path::new("binary"),
        TargetArchitecture::X86,
        "x86_64",
    );
    let args: Vec<OsString> = command.get_args().map(OsString::from).collect();

    assert_eq!(
        args,
        vec![
            OsString::from("--disassemble"),
            OsString::from("--demangle"),
            OsString::from("--no-show-raw-insn"),
            OsString::from("-Mintel"),
            OsString::from("binary"),
        ]
    );
}

#[test]
fn builds_llvm_objdump_command_with_intel_syntax() {
    let command = build_objdump_command_for_arch(
        Path::new("llvm-objdump"),
        Path::new("binary"),
        TargetArchitecture::X86,
        "x86_64",
    );
    let args: Vec<OsString> = command.get_args().map(OsString::from).collect();

    assert_eq!(
        args,
        vec![
            OsString::from("--disassemble"),
            OsString::from("--demangle"),
            OsString::from("--no-show-raw-insn"),
            OsString::from("--x86-asm-syntax=intel"),
            OsString::from("binary"),
        ]
    );
}

#[test]
fn omits_intel_syntax_for_aarch64_targets() {
    let command = build_objdump_command_for_arch(
        Path::new("objdump"),
        Path::new("binary"),
        TargetArchitecture::Aarch64,
        "x86_64",
    );
    let args: Vec<OsString> = command.get_args().map(OsString::from).collect();

    assert_eq!(
        args,
        vec![
            OsString::from("--disassemble"),
            OsString::from("--demangle"),
            OsString::from("--no-show-raw-insn"),
            OsString::from("binary"),
        ]
    );
}

#[test]
fn omits_intel_syntax_on_aarch64_hosts() {
    let command = build_objdump_command_for_arch(
        Path::new("objdump"),
        Path::new("binary"),
        TargetArchitecture::X86,
        "aarch64",
    );
    let args: Vec<OsString> = command.get_args().map(OsString::from).collect();

    assert_eq!(
        args,
        vec![
            OsString::from("--disassemble"),
            OsString::from("--demangle"),
            OsString::from("--no-show-raw-insn"),
            OsString::from("binary"),
        ]
    );
}

#[test]
fn detects_elf_target_architecture() {
    let x86 = temp_elf_binary(62);
    let aarch64 = temp_elf_binary(183);

    assert_eq!(
        detect_target_architecture(x86.path()),
        TargetArchitecture::X86
    );
    assert_eq!(
        detect_target_architecture(aarch64.path()),
        TargetArchitecture::Aarch64
    );
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

#[test]
fn identifies_functions_present_in_both_binaries() {
    let shared = FunctionComparison {
        name: "shared".to_owned(),
        function1: Some(FunctionDisassembly {
            instructions: Vec::new(),
            normalized_instructions: Vec::new(),
            rendered: String::new(),
        }),
        function2: Some(FunctionDisassembly {
            instructions: Vec::new(),
            normalized_instructions: Vec::new(),
            rendered: String::new(),
        }),
        combined_score: 1.0,
        count_score: 1.0,
        order_score: 1.0,
    };
    let unique = FunctionComparison {
        name: "unique".to_owned(),
        function1: Some(FunctionDisassembly {
            instructions: Vec::new(),
            normalized_instructions: Vec::new(),
            rendered: String::new(),
        }),
        function2: None,
        combined_score: 0.0,
        count_score: 0.0,
        order_score: 0.0,
    };

    assert!(shared.is_present_in_both());
    assert!(!unique.is_present_in_both());
}

#[test]
fn detects_identical_functions_from_normalized_instructions() {
    let left = FunctionComparison {
        name: "shared".to_owned(),
        function1: Some(FunctionDisassembly {
            instructions: vec!["mov".to_owned(), "ret".to_owned()],
            normalized_instructions: vec![
                "mov %rsp,%rbp".to_owned(),
                "ret".to_owned(),
            ],
            rendered: String::new(),
        }),
        function2: Some(FunctionDisassembly {
            instructions: vec!["mov".to_owned(), "ret".to_owned()],
            normalized_instructions: vec![
                "mov %rsp,%rbp".to_owned(),
                "ret".to_owned(),
            ],
            rendered: String::new(),
        }),
        combined_score: 1.0,
        count_score: 1.0,
        order_score: 1.0,
    };
    let right = FunctionComparison {
        name: "different".to_owned(),
        function1: left.function1.clone(),
        function2: Some(FunctionDisassembly {
            instructions: vec!["mov".to_owned(), "ret".to_owned()],
            normalized_instructions: vec![
                "mov %rsp,%rbp".to_owned(),
                "ret $0x8".to_owned(),
            ],
            rendered: String::new(),
        }),
        combined_score: 0.5,
        count_score: 1.0,
        order_score: 1.0,
    };

    assert!(left.is_identical());
    assert!(!right.is_identical());
    assert!(left.is_effectively_identical());
    assert!(!right.is_effectively_identical());
}

#[test]
fn treats_perfect_similarity_as_effectively_identical() {
    let comparison = FunctionComparison {
        name: "shared".to_owned(),
        function1: Some(FunctionDisassembly {
            instructions: vec!["mov".to_owned(), "ret".to_owned()],
            normalized_instructions: vec![
                "mov %rsp,%rbp".to_owned(),
                "ret".to_owned(),
            ],
            rendered: String::new(),
        }),
        function2: Some(FunctionDisassembly {
            instructions: vec!["mov".to_owned(), "ret".to_owned()],
            normalized_instructions: vec![
                "mov %rax,%rbx".to_owned(),
                "ret".to_owned(),
            ],
            rendered: String::new(),
        }),
        combined_score: 1.0,
        count_score: 1.0,
        order_score: 1.0,
    };

    assert!(!comparison.is_identical());
    assert!(comparison.has_perfect_similarity());
    assert!(comparison.is_effectively_identical());
}

#[test]
fn reports_left_and_right_op_counts() {
    let comparison = FunctionComparison {
        name: "shared".to_owned(),
        function1: Some(FunctionDisassembly {
            instructions: vec![
                "mov".to_owned(),
                "call".to_owned(),
                "ret".to_owned(),
            ],
            normalized_instructions: Vec::new(),
            rendered: String::new(),
        }),
        function2: Some(FunctionDisassembly {
            instructions: vec!["mov".to_owned(), "ret".to_owned()],
            normalized_instructions: Vec::new(),
            rendered: String::new(),
        }),
        combined_score: 0.0,
        count_score: 0.0,
        order_score: 0.0,
    };

    assert_eq!(comparison.left_op_count(), 3);
    assert_eq!(comparison.right_op_count(), 2);
}

#[test]
fn filters_visible_items_case_insensitively() {
    let mut app = App::new(
        vec![
            prepared_comparison("AlphaRelay", 0.1),
            prepared_comparison("beta", 0.2),
            prepared_comparison("relay_worker", 0.3),
        ],
        DiffMode::Combined,
        false,
        false,
        String::new(),
    );

    app.start_search();
    for character in "ReLaY".chars() {
        app.append_search_char(character);
    }
    app.confirm_search();

    assert_eq!(app.visible_count(), 2);
    assert_eq!(
        visible_names(&app),
        vec!["AlphaRelay".to_owned(), "relay_worker".to_owned()]
    );
    assert_eq!(
        app.selected().map(|item| item.comparison.name.as_str()),
        Some("AlphaRelay")
    );
}

#[test]
fn filters_visible_items_with_regex() {
    let mut app = App::new(
        vec![
            prepared_comparison("AlphaRelay", 0.1),
            prepared_comparison("relay_worker", 0.2),
            prepared_comparison("other", 0.3),
        ],
        DiffMode::Combined,
        false,
        false,
        String::new(),
    );

    app.start_search();
    for character in "/^relay|alpha/".chars() {
        app.append_search_char(character);
    }
    app.confirm_search();

    assert_eq!(
        visible_names(&app),
        vec!["AlphaRelay".to_owned(), "relay_worker".to_owned()]
    );
    assert!(app.search_error().is_none());
}

#[test]
fn invalid_regex_yields_no_matches_and_error() {
    let mut app = App::new(
        vec![
            prepared_comparison("AlphaRelay", 0.1),
            prepared_comparison("relay_worker", 0.2),
        ],
        DiffMode::Combined,
        false,
        false,
        String::new(),
    );

    app.start_search();
    for character in "/(/".chars() {
        app.append_search_char(character);
    }
    app.confirm_search();

    assert_eq!(app.visible_count(), 0);
    assert!(app.search_error().is_some());
    assert!(app.selected().is_none());
}

#[test]
fn cancel_search_restores_previous_filter() {
    let mut app = App::new(
        vec![
            prepared_comparison("relay_a", 0.1),
            prepared_comparison("relay_b", 0.2),
            prepared_comparison("other", 0.3),
        ],
        DiffMode::Combined,
        false,
        false,
        String::new(),
    );

    app.start_search();
    for character in "relay".chars() {
        app.append_search_char(character);
    }
    app.confirm_search();

    app.start_search();
    app.append_search_char('z');
    assert_eq!(app.visible_count(), 0);

    app.cancel_search();

    assert_eq!(app.search_query, "relay");
    assert_eq!(app.visible_count(), 2);
    assert_eq!(
        visible_names(&app),
        vec!["relay_a".to_owned(), "relay_b".to_owned()]
    );
}

#[test]
fn dumps_sorted_stdio_table() {
    let comparisons = vec![
        comparison_for_stdio("beta", 0.4, 0.6, 0.2, true, true),
        comparison_for_stdio("alpha", 0.1, 0.3, 0.0, true, false),
    ];
    let mut output = Vec::new();

    dump_comparisons(&mut output, &comparisons, DiffMode::Combined)
        .expect("failed to dump table");

    let rendered = String::from_utf8(output).expect("expected utf-8");
    let mut lines = rendered.lines();
    let header = lines.next().expect("missing header");
    let first = lines.next().expect("missing first row");
    let second = lines.next().expect("missing second row");

    assert!(header.contains("Function"));
    assert!(header.contains("combined"));
    assert!(header.contains("count"));
    assert!(header.contains("ops"));
    assert!(header.contains("Bin1"));
    assert!(header.contains("Bin2"));
    assert!(first.starts_with("alpha"));
    assert!(first.ends_with(" yes    no"));
    assert!(second.starts_with("beta"));
    assert!(second.ends_with(" yes   yes"));
}

#[test]
fn app_applies_initial_filter() {
    let app = App::new(
        vec![
            prepared_comparison("AlphaRelay", 0.1),
            prepared_comparison("relay_worker", 0.2),
            prepared_comparison("other", 0.3),
        ],
        DiffMode::Combined,
        false,
        false,
        "relay".to_owned(),
    );

    assert_eq!(app.search_query, "relay");
    assert_eq!(
        visible_names(&app),
        vec!["AlphaRelay".to_owned(), "relay_worker".to_owned()]
    );
    assert_eq!(
        app.selected().map(|item| item.comparison.name.as_str()),
        Some("AlphaRelay")
    );
}

#[test]
fn build_comparisons_pre_filters_names() {
    let analysis_one = BinaryAnalysis {
        functions: HashMap::from([
            ("AlphaRelay".to_owned(), synthetic_function()),
            ("other".to_owned(), synthetic_function()),
        ]),
    };
    let analysis_two = BinaryAnalysis {
        functions: HashMap::from([
            ("relay_worker".to_owned(), synthetic_function()),
            ("other".to_owned(), synthetic_function()),
        ]),
    };
    let filter = SearchFilter::compile("relay");

    let comparisons = build_comparisons(
        &analysis_one,
        &analysis_two,
        true,
        true,
        Some(&filter),
    );

    assert_eq!(
        comparisons
            .iter()
            .map(|comparison| comparison.name.as_str())
            .collect::<Vec<_>>(),
        vec!["AlphaRelay", "relay_worker"]
    );
}

#[test]
fn build_comparisons_hides_perfect_similarity_by_default() {
    let analysis_one = BinaryAnalysis {
        functions: HashMap::from([(
            "shared".to_owned(),
            FunctionDisassembly {
                instructions: vec!["mov".to_owned(), "ret".to_owned()],
                normalized_instructions: vec![
                    "mov %rsp,%rbp".to_owned(),
                    "ret".to_owned(),
                ],
                rendered: String::new(),
            },
        )]),
    };
    let analysis_two = BinaryAnalysis {
        functions: HashMap::from([(
            "shared".to_owned(),
            FunctionDisassembly {
                instructions: vec!["mov".to_owned(), "ret".to_owned()],
                normalized_instructions: vec![
                    "mov %rax,%rbx".to_owned(),
                    "ret".to_owned(),
                ],
                rendered: String::new(),
            },
        )]),
    };

    let hidden =
        build_comparisons(&analysis_one, &analysis_two, false, false, None);
    let shown =
        build_comparisons(&analysis_one, &analysis_two, false, true, None);

    assert!(hidden.is_empty());
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].name, "shared");
    assert!(shown[0].has_perfect_similarity());
}

fn visible_names(app: &App) -> Vec<String> {
    app.filtered_indices
        .iter()
        .map(|index| app.items[*index].comparison.name.clone())
        .collect()
}

fn comparison_for_stdio(
    name: &str,
    combined_score: f64,
    count_score: f64,
    order_score: f64,
    present_in_binary1: bool,
    present_in_binary2: bool,
) -> FunctionComparison {
    FunctionComparison {
        name: name.to_owned(),
        function1: present_in_binary1.then(synthetic_function),
        function2: present_in_binary2.then(synthetic_function),
        combined_score,
        count_score,
        order_score,
    }
}

fn synthetic_function() -> FunctionDisassembly {
    FunctionDisassembly {
        instructions: vec!["mov".to_owned()],
        normalized_instructions: vec!["mov".to_owned()],
        rendered: "mov\n".to_owned(),
    }
}

fn local_labels_for(entries: &[(u64, &str)]) -> HashMap<u64, String> {
    entries
        .iter()
        .map(|(address, label)| (*address, (*label).to_owned()))
        .collect()
}

fn parsed_instruction(address: u64, text: &str) -> ParsedInstruction {
    ParsedInstruction {
        original_line: format!("{address:x}:\t{text}"),
        address: Some(address),
        text: text.to_owned(),
    }
}

fn function_builder(
    name: &str,
    instructions: &[ParsedInstruction],
) -> FunctionBuilder {
    let header_line = format!("0000000000001000 <{name}>:");
    let mut lines = Vec::with_capacity(instructions.len() + 1);
    lines.push(header_line.clone());
    lines.extend(
        instructions
            .iter()
            .map(|instruction| instruction.original_line.clone()),
    );

    FunctionBuilder {
        name: name.to_owned(),
        header_line,
        lines,
        instructions: instructions.to_vec(),
    }
}

fn prepared_comparison(name: &str, combined_score: f64) -> PreparedComparison {
    PreparedComparison {
        comparison: FunctionComparison {
            name: name.to_owned(),
            function1: Some(FunctionDisassembly {
                instructions: vec!["mov".to_owned()],
                normalized_instructions: vec!["mov".to_owned()],
                rendered: format!("{name}\n"),
            }),
            function2: Some(FunctionDisassembly {
                instructions: vec!["mov".to_owned()],
                normalized_instructions: vec!["mov".to_owned()],
                rendered: format!("{name}\n"),
            }),
            combined_score,
            count_score: combined_score,
            order_score: combined_score,
        },
        diff1_path: write_temp_disassembly(&format!("{name}-left\n"), "left")
            .expect("failed to create left temp file"),
        diff2_path: write_temp_disassembly(&format!("{name}-right\n"), "right")
            .expect("failed to create right temp file"),
    }
}

fn temp_elf_binary(machine: u16) -> NamedTempFile {
    let mut file = NamedTempFile::new().expect("failed to create temp file");
    let mut header = [0_u8; 64];
    header[0..4].copy_from_slice(b"\x7fELF");
    header[4] = 2;
    header[5] = 1;
    header[18..20].copy_from_slice(&machine.to_le_bytes());
    file.write_all(&header).expect("failed to write ELF header");
    file
}

#[test]
fn temp_labels_use_distinct_basenames_when_available() {
    let labels =
        temp_file_labels(Path::new("/tmp/old.so"), Path::new("/tmp/new.so"));

    assert_eq!(labels, ["old.so".to_owned(), "new.so".to_owned()]);
}

#[test]
fn temp_labels_add_side_markers_when_basenames_match() {
    let labels = temp_file_labels(
        Path::new("/tmp/old/foo.so"),
        Path::new("/tmp/new/foo.so"),
    );

    assert_eq!(
        labels,
        ["LEFT-foo.so".to_owned(), "RIGHT-foo.so".to_owned()]
    );
}

#[test]
fn temp_disassembly_path_includes_label_prefix() {
    let temp_path =
        write_temp_disassembly("mov\n", "LEFT-foo.so").expect("temp file");
    let path = temp_path.display().to_string();

    assert!(path.contains("cgdiff-LEFT-foo.so-"));
    assert_eq!(temp_path.extension(), Some(std::ffi::OsStr::new("s")));
}
