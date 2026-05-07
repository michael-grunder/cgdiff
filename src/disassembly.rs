use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, mpsc};
use std::thread;

use anyhow::{Context, Result, anyhow, bail};
use regex::{NoExpand, Regex};

use crate::progress::{
    ProgressEvent, send_progress_finished, send_progress_processed,
    send_progress_start,
};

const EM_386: u16 = 3;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;
const MACHO_CPU_TYPE_X86: u32 = 7;
const MACHO_CPU_TYPE_X86_64: u32 = 0x0100_0007;
const MACHO_CPU_TYPE_ARM64: u32 = 0x0100_000c;

static SYMBOL_TARGET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<addr>0x[0-9a-fA-F]+|[0-9a-fA-F]+)\s+<(?P<sym>[^>]+)>")
        .expect("symbol target regex must compile")
});
static RIP_DATA_COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"#\s*(?P<addr>0x[0-9a-fA-F]+|[0-9a-fA-F]+)\s+<(?P<sym>[^>]+)>")
        .expect("RIP data comment regex must compile")
});
static RIP_RELATIVE_OPERAND_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\s*rip\s*[+-]\s*(?:0x[0-9a-fA-F]+|[0-9a-fA-F]+)\s*\]")
        .expect("RIP-relative operand regex must compile")
});
static MEMORY_OPERAND_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[[^\]]+\]|\([^)]*\)")
        .expect("memory operand regex must compile")
});

#[derive(Clone, Debug)]
pub(crate) struct BinaryAnalysis {
    pub(crate) functions: HashMap<String, FunctionDisassembly>,
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionDisassembly {
    pub(crate) instructions: Vec<String>,
    pub(crate) normalized_instructions: Vec<String>,
    pub(crate) aggregates: FunctionAggregates,
    pub(crate) rendered: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FunctionAggregates {
    pub(crate) instructions_total: usize,
    pub(crate) bytes_total: usize,
    pub(crate) unique_mnemonics: usize,
    pub(crate) calls: usize,
    pub(crate) direct_calls: usize,
    pub(crate) indirect_calls: usize,
    pub(crate) branches_total: usize,
    pub(crate) conditional_branches: usize,
    pub(crate) unconditional_branches: usize,
    pub(crate) indirect_branches: usize,
    pub(crate) returns: usize,
    pub(crate) memory_operands_total: usize,
    pub(crate) memory_loads: usize,
    pub(crate) memory_stores: usize,
    pub(crate) stack_loads: usize,
    pub(crate) stack_stores: usize,
    pub(crate) rip_relative_loads: usize,
    pub(crate) rip_relative_stores: usize,
    pub(crate) atomics: usize,
    pub(crate) locked_ops: usize,
    pub(crate) memory_barriers: usize,
    pub(crate) integer_mul: usize,
    pub(crate) integer_div: usize,
    pub(crate) floating_point_ops: usize,
    pub(crate) simd_vector_ops: usize,
    pub(crate) pushes: usize,
    pub(crate) pops: usize,
    pub(crate) frame_setup_ops: usize,
    pub(crate) register_moves: usize,
    pub(crate) lea_ops: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MemoryAccess {
    None,
    Load,
    Store,
    LoadStore,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedInstruction {
    pub(crate) original_line: String,
    pub(crate) address: Option<u64>,
    pub(crate) byte_count: Option<usize>,
    pub(crate) text: String,
}

#[derive(Debug)]
pub(crate) struct FunctionBuilder {
    pub(crate) name: String,
    pub(crate) header_line: String,
    pub(crate) lines: Vec<String>,
    pub(crate) instructions: Vec<ParsedInstruction>,
}

#[derive(Debug)]
pub(crate) struct NormalizedInstruction {
    pub(crate) text: String,
    pub(crate) local_target: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetArchitecture {
    X86,
    Aarch64,
    Other,
}

pub(crate) fn analyze_binary(
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

pub(crate) fn build_objdump_command(
    objdump: &Path,
    binary_path: &Path,
) -> Command {
    build_objdump_command_for_arch(
        objdump,
        binary_path,
        detect_target_architecture(binary_path),
        std::env::consts::ARCH,
    )
}

pub(crate) fn build_objdump_command_for_arch(
    objdump: &Path,
    binary_path: &Path,
    target_architecture: TargetArchitecture,
    host_architecture: &str,
) -> Command {
    let mut command = Command::new(objdump);
    command
        .arg("--disassemble")
        .arg("--demangle")
        .args(x86_intel_syntax_args(
            objdump,
            target_architecture,
            host_architecture,
        ))
        .arg(binary_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

pub(crate) fn detect_target_architecture(
    binary_path: &Path,
) -> TargetArchitecture {
    read_binary_header(binary_path)
        .ok()
        .and_then(|contents| detect_target_architecture_from_bytes(&contents))
        .unwrap_or(TargetArchitecture::Other)
}

fn read_binary_header(binary_path: &Path) -> io::Result<Vec<u8>> {
    let mut file = fs::File::open(binary_path)?;
    let mut contents = vec![0; 4096];
    let bytes_read = file.read(&mut contents)?;
    contents.truncate(bytes_read);
    Ok(contents)
}

fn detect_target_architecture_from_bytes(
    contents: &[u8],
) -> Option<TargetArchitecture> {
    detect_elf_architecture(contents)
        .or_else(|| detect_macho_architecture(contents))
}

fn detect_elf_architecture(contents: &[u8]) -> Option<TargetArchitecture> {
    if contents.len() < 20 || !contents.starts_with(b"\x7fELF") {
        return None;
    }

    let machine = match contents.get(5).copied()? {
        1 => u16::from_le_bytes([contents[18], contents[19]]),
        2 => u16::from_be_bytes([contents[18], contents[19]]),
        _ => return Some(TargetArchitecture::Other),
    };

    Some(match machine {
        EM_386 | EM_X86_64 => TargetArchitecture::X86,
        EM_AARCH64 => TargetArchitecture::Aarch64,
        _ => TargetArchitecture::Other,
    })
}

fn detect_macho_architecture(contents: &[u8]) -> Option<TargetArchitecture> {
    if contents.len() < 8 {
        return None;
    }

    match &contents[..4] {
        [0xcf | 0xce, 0xfa, 0xed, 0xfe] => {
            Some(macho_architecture_from_cpu_type(u32::from_le_bytes([
                contents[4],
                contents[5],
                contents[6],
                contents[7],
            ])))
        }
        [0xfe, 0xed, 0xfa, 0xcf | 0xce] => {
            Some(macho_architecture_from_cpu_type(u32::from_be_bytes([
                contents[4],
                contents[5],
                contents[6],
                contents[7],
            ])))
        }
        [0xca, 0xfe, 0xba, 0xbe] => {
            detect_fat_macho_architecture(contents, u32::from_be_bytes)
        }
        [0xbe, 0xba, 0xfe, 0xca] => {
            detect_fat_macho_architecture(contents, u32::from_le_bytes)
        }
        _ => None,
    }
}

fn detect_fat_macho_architecture(
    contents: &[u8],
    read_u32: fn([u8; 4]) -> u32,
) -> Option<TargetArchitecture> {
    if contents.len() < 8 {
        return None;
    }

    let architecture_count =
        usize::try_from(read_u32(contents[4..8].try_into().ok()?)).ok()?;
    let mut detected = TargetArchitecture::Other;
    for index in 0..architecture_count {
        let offset = 8 + index.saturating_mul(20);
        let cpu_type_bytes = contents.get(offset..offset + 4)?;
        let cpu_type = read_u32(cpu_type_bytes.try_into().ok()?);
        match macho_architecture_from_cpu_type(cpu_type) {
            TargetArchitecture::Aarch64 => {
                return Some(TargetArchitecture::Aarch64);
            }
            TargetArchitecture::X86 => detected = TargetArchitecture::X86,
            TargetArchitecture::Other => {}
        }
    }

    Some(detected)
}

const fn macho_architecture_from_cpu_type(cpu_type: u32) -> TargetArchitecture {
    match cpu_type {
        MACHO_CPU_TYPE_X86 | MACHO_CPU_TYPE_X86_64 => TargetArchitecture::X86,
        MACHO_CPU_TYPE_ARM64 => TargetArchitecture::Aarch64,
        _ => TargetArchitecture::Other,
    }
}

fn x86_intel_syntax_args(
    objdump: &Path,
    target_architecture: TargetArchitecture,
    host_architecture: &str,
) -> &'static [&'static str] {
    if target_architecture != TargetArchitecture::X86
        || !matches!(host_architecture, "x86" | "x86_64")
    {
        return &[];
    }

    match objdump.file_name().and_then(|name| name.to_str()) {
        Some(name) if name.starts_with("llvm-objdump") => {
            &["--x86-asm-syntax=intel"]
        }
        _ => &["-Mintel"],
    }
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
    let mut current_function: Option<FunctionBuilder> = None;
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
            flush_current_function(&mut functions, &mut current_function);
            current_function = Some(FunctionBuilder {
                name,
                header_line: trimmed.to_owned(),
                lines: vec![trimmed.to_owned()],
                instructions: Vec::new(),
            });
            continue;
        }

        if let Some(function) = current_function.as_mut() {
            function.lines.push(trimmed.to_owned());
            if let Some(instruction) = parse_instruction_line(trimmed) {
                function.instructions.push(instruction);
            }
        }
    }

    flush_current_function(&mut functions, &mut current_function);

    Ok(functions)
}
fn flush_current_function(
    functions: &mut HashMap<String, FunctionDisassembly>,
    current_function: &mut Option<FunctionBuilder>,
) {
    if let Some(builder) = current_function.take() {
        functions.insert(builder.name.clone(), finalize_function(&builder));
    }
}

pub(crate) fn finalize_function(
    builder: &FunctionBuilder,
) -> FunctionDisassembly {
    debug_assert_eq!(
        parse_function_header(&builder.header_line).as_deref(),
        Some(builder.name.as_str())
    );

    let local_labels = builder
        .instructions
        .iter()
        .enumerate()
        .filter_map(|(index, instruction)| {
            instruction
                .address
                .map(|address| (address, format!(".L{index:04}")))
        })
        .collect::<HashMap<_, _>>();

    let normalized = builder
        .instructions
        .iter()
        .map(|instruction| {
            normalize_instruction_text(&instruction.text, &local_labels)
        })
        .collect::<Vec<_>>();

    let target_addresses = normalized
        .iter()
        .filter_map(|instruction| instruction.local_target)
        .collect::<HashSet<_>>();

    let instructions = builder
        .instructions
        .iter()
        .filter_map(|instruction| parse_instruction_mnemonic(&instruction.text))
        .collect::<Vec<_>>();
    let normalized_instructions = normalized
        .iter()
        .map(|instruction| instruction.text.clone())
        .collect::<Vec<_>>();
    let aggregates = aggregate_function(&builder.instructions);
    let original_bytes = builder
        .instructions
        .iter()
        .map(|instruction| instruction.original_line.len())
        .sum::<usize>();
    let mut rendered_lines = Vec::with_capacity(
        builder.lines.len().max(builder.instructions.len() + 1),
    );
    rendered_lines.push(format!("<{}>:", builder.name));

    for (instruction, normalized_instruction) in
        builder.instructions.iter().zip(&normalized)
    {
        if let Some(address) = instruction.address
            && target_addresses.contains(&address)
            && let Some(label) = local_labels.get(&address)
        {
            rendered_lines.push(format!("{label}:"));
        }
        rendered_lines.push(format!("    {}", normalized_instruction.text));
    }

    let mut rendered = String::with_capacity(
        original_bytes.max(
            rendered_lines
                .iter()
                .map(String::len)
                .sum::<usize>()
                .saturating_add(rendered_lines.len()),
        ),
    );
    rendered.push_str(&rendered_lines.join("\n"));
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }

    FunctionDisassembly {
        instructions,
        normalized_instructions,
        aggregates,
        rendered,
    }
}

pub(crate) fn parse_function_header(line: &str) -> Option<String> {
    let line = line.trim();
    let suffix = ">:";
    let start = line.find('<')?;
    if !line.ends_with(suffix) || start == 0 {
        return None;
    }

    Some(line[start + 1..line.len() - suffix.len()].to_owned())
}

#[cfg(test)]
pub(crate) fn parse_instruction_text(line: &str) -> Option<String> {
    parse_instruction_line(line).map(|instruction| instruction.text)
}

pub(crate) fn parse_instruction_line(line: &str) -> Option<ParsedInstruction> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.ends_with(':') {
        return None;
    }

    let (address_text, remainder) = trimmed.split_once(':')?;
    let address = parse_hex_address(address_text)?;
    let (text, byte_count) = parse_instruction_remainder(remainder)?;

    Some(ParsedInstruction {
        original_line: line.to_owned(),
        address: Some(address),
        byte_count,
        text,
    })
}

fn parse_instruction_remainder(
    remainder: &str,
) -> Option<(String, Option<usize>)> {
    let trimmed = remainder.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((first_column, rest)) = trimmed.split_once('\t')
        && is_raw_byte_column(first_column)
    {
        let instruction = rest.trim();
        return (!instruction.is_empty()).then(|| {
            (
                instruction.to_owned(),
                Some(first_column.split_whitespace().count()),
            )
        });
    }

    Some((trimmed.to_owned(), None))
}

fn is_raw_byte_column(column: &str) -> bool {
    let mut byte_count = 0;
    for token in column.split_whitespace() {
        if token.len() != 2
            || !token.chars().all(|character| character.is_ascii_hexdigit())
        {
            return false;
        }
        byte_count += 1;
    }

    byte_count > 0
}

fn parse_hex_address(text: &str) -> Option<u64> {
    let trimmed = text.trim();
    let hex = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);

    (!hex.is_empty()
        && hex.chars().all(|character| character.is_ascii_hexdigit()))
    .then(|| u64::from_str_radix(hex, 16).ok())
    .flatten()
}

pub(crate) fn parse_instruction_mnemonic(
    instruction_text: &str,
) -> Option<String> {
    instruction_text
        .split_whitespace()
        .next()
        .map(str::to_owned)
}

pub(crate) fn normalize_instruction_text(
    instruction_text: &str,
    local_labels: &HashMap<u64, String>,
) -> NormalizedInstruction {
    let code = normalize_rip_data_reference(instruction_text);
    let code = strip_comment(&code);
    let Some((mnemonic, operands)) = split_mnemonic_operands(&code) else {
        return NormalizedInstruction {
            text: collapse_whitespace(&code),
            local_target: None,
        };
    };

    if !is_direct_control_flow_mnemonic(mnemonic) {
        return NormalizedInstruction {
            text: collapse_whitespace(&code),
            local_target: None,
        };
    }

    if let Some((address, symbol)) = parse_symbol_target(operands) {
        if let Some(label) = local_labels.get(&address) {
            return NormalizedInstruction {
                text: format!(
                    "{mnemonic} {}",
                    normalize_symbol_target_operand(operands, label)
                ),
                local_target: Some(address),
            };
        }

        return NormalizedInstruction {
            text: format!(
                "{mnemonic} {}",
                normalize_symbol_target_operand(
                    operands,
                    &format!("sym:{symbol}"),
                )
            ),
            local_target: None,
        };
    }

    if let Some(address) = parse_direct_address_operand(operands) {
        if let Some(label) = local_labels.get(&address) {
            return NormalizedInstruction {
                text: format!("{mnemonic} {label}"),
                local_target: Some(address),
            };
        }

        return NormalizedInstruction {
            text: format!("{mnemonic} addr:external"),
            local_target: None,
        };
    }

    NormalizedInstruction {
        text: collapse_whitespace(&code),
        local_target: None,
    }
}

fn normalize_rip_data_reference(instruction_text: &str) -> String {
    let Some(captures) = RIP_DATA_COMMENT_RE.captures(instruction_text) else {
        return instruction_text.to_owned();
    };
    let Some(symbol_match) = captures.name("sym") else {
        return instruction_text.to_owned();
    };

    let symbol = symbol_match.as_str();
    let code = instruction_text
        .split_once('#')
        .map_or(instruction_text, |(code, _comment)| code);
    let replacement = format!("[rip + data:{symbol}]");
    RIP_RELATIVE_OPERAND_RE
        .replace(code, replacement.as_str())
        .into_owned()
}

fn strip_comment(instruction_text: &str) -> String {
    instruction_text
        .split_once('#')
        .map_or(instruction_text, |(code, _comment)| code)
        .trim()
        .to_owned()
}

fn split_mnemonic_operands(instruction_text: &str) -> Option<(&str, &str)> {
    let trimmed = instruction_text.trim();
    let mnemonic = trimmed.split_whitespace().next()?;
    let operands = trimmed[mnemonic.len()..].trim();
    Some((mnemonic, operands))
}

fn parse_symbol_target(operands: &str) -> Option<(u64, String)> {
    let captures = SYMBOL_TARGET_RE.captures(operands)?;
    let address = parse_hex_address(captures.name("addr")?.as_str())?;
    let symbol = captures.name("sym")?.as_str().to_owned();
    Some((address, symbol))
}

fn normalize_symbol_target_operand(
    operands: &str,
    replacement: &str,
) -> String {
    collapse_whitespace(
        &SYMBOL_TARGET_RE.replace(operands, NoExpand(replacement)),
    )
}

fn parse_direct_address_operand(operands: &str) -> Option<u64> {
    let operand = operands.trim();
    if operand.contains('[') || operand.contains(',') {
        return None;
    }

    let token = operand.split_whitespace().next()?;
    parse_hex_address(token)
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn aggregate_function(
    instructions: &[ParsedInstruction],
) -> FunctionAggregates {
    let mut aggregates = FunctionAggregates {
        instructions_total: instructions.len(),
        bytes_total: instructions
            .iter()
            .filter_map(|instruction| instruction.byte_count)
            .sum(),
        ..FunctionAggregates::default()
    };
    let mut unique_mnemonics = HashSet::new();

    for instruction in instructions {
        let Some(mnemonic) = parse_instruction_mnemonic(&instruction.text)
        else {
            continue;
        };
        let mnemonic = mnemonic.to_ascii_lowercase();
        unique_mnemonics.insert(mnemonic.clone());
        let operands = instruction.text[mnemonic.len()..].trim();

        aggregate_control_flow(&mut aggregates, &mnemonic, operands);
        aggregate_memory(&mut aggregates, &mnemonic, operands);
        aggregate_operation_classes(&mut aggregates, &mnemonic, operands);
        aggregate_stack_and_moves(&mut aggregates, &mnemonic, operands);
    }

    aggregates.unique_mnemonics = unique_mnemonics.len();
    aggregates
}

fn aggregate_control_flow(
    aggregates: &mut FunctionAggregates,
    mnemonic: &str,
    operands: &str,
) {
    if is_call_mnemonic(mnemonic) {
        aggregates.calls += 1;
        if is_indirect_control_flow(mnemonic, operands) {
            aggregates.indirect_calls += 1;
        } else {
            aggregates.direct_calls += 1;
        }
        return;
    }

    if is_return_mnemonic(mnemonic) {
        aggregates.returns += 1;
        aggregates.branches_total += 1;
        return;
    }

    if is_conditional_branch_mnemonic(mnemonic) {
        aggregates.conditional_branches += 1;
        aggregates.branches_total += 1;
    } else if is_unconditional_branch_mnemonic(mnemonic) {
        aggregates.unconditional_branches += 1;
        aggregates.branches_total += 1;
        if is_indirect_control_flow(mnemonic, operands) {
            aggregates.indirect_branches += 1;
        }
    }
}

fn aggregate_memory(
    aggregates: &mut FunctionAggregates,
    mnemonic: &str,
    operands: &str,
) {
    let access = memory_access(mnemonic, operands);
    if access == MemoryAccess::None {
        return;
    }

    aggregates.memory_operands_total +=
        MEMORY_OPERAND_RE.find_iter(operands).count();
    if matches!(access, MemoryAccess::Load | MemoryAccess::LoadStore) {
        aggregates.memory_loads += 1;
    }
    if matches!(access, MemoryAccess::Store | MemoryAccess::LoadStore) {
        aggregates.memory_stores += 1;
    }

    let operands = operands.to_ascii_lowercase();
    let stack_relative = operands.contains("[rsp")
        || operands.contains("[rbp")
        || operands.contains("(%rsp")
        || operands.contains("(%rbp")
        || operands.contains("[sp")
        || operands.contains(", sp")
        || operands.contains("[x29")
        || operands.contains("[fp");
    let rip_relative = operands.contains("[rip") || operands.contains("(%rip");

    if stack_relative
        && matches!(access, MemoryAccess::Load | MemoryAccess::LoadStore)
    {
        aggregates.stack_loads += 1;
    }
    if stack_relative
        && matches!(access, MemoryAccess::Store | MemoryAccess::LoadStore)
    {
        aggregates.stack_stores += 1;
    }
    if rip_relative
        && matches!(access, MemoryAccess::Load | MemoryAccess::LoadStore)
    {
        aggregates.rip_relative_loads += 1;
    }
    if rip_relative
        && matches!(access, MemoryAccess::Store | MemoryAccess::LoadStore)
    {
        aggregates.rip_relative_stores += 1;
    }
}

fn aggregate_operation_classes(
    aggregates: &mut FunctionAggregates,
    mnemonic: &str,
    operands: &str,
) {
    if mnemonic == "lock" || mnemonic.starts_with("lock") {
        aggregates.locked_ops += 1;
        aggregates.atomics += 1;
    }
    if matches!(
        mnemonic,
        "xadd"
            | "xchg"
            | "cmpxchg"
            | "cmpxchg8b"
            | "cmpxchg16b"
            | "ldxr"
            | "ldaxr"
            | "stxr"
            | "stlxr"
    ) {
        aggregates.atomics += 1;
    }
    if matches!(
        mnemonic,
        "mfence" | "lfence" | "sfence" | "dmb" | "dsb" | "isb"
    ) {
        aggregates.memory_barriers += 1;
    }
    if mnemonic.contains("mul") && !mnemonic.contains("fmul") {
        aggregates.integer_mul += 1;
    }
    if mnemonic.contains("div") && !mnemonic.contains("fdiv") {
        aggregates.integer_div += 1;
    }
    if is_floating_point_mnemonic(mnemonic) {
        aggregates.floating_point_ops += 1;
    }
    if is_simd_vector_op(mnemonic, operands) {
        aggregates.simd_vector_ops += 1;
    }
}

fn aggregate_stack_and_moves(
    aggregates: &mut FunctionAggregates,
    mnemonic: &str,
    operands: &str,
) {
    if mnemonic.starts_with("push")
        || mnemonic == "stp" && operands.contains("sp")
    {
        aggregates.pushes += 1;
    }
    if mnemonic.starts_with("pop")
        || mnemonic == "ldp" && operands.contains("sp")
    {
        aggregates.pops += 1;
    }
    if is_frame_setup_op(mnemonic, operands) {
        aggregates.frame_setup_ops += 1;
    }
    if mnemonic == "lea" || mnemonic == "adrp" || mnemonic == "adr" {
        aggregates.lea_ops += 1;
    }
    if is_register_move(mnemonic, operands) {
        aggregates.register_moves += 1;
    }
}

fn memory_access(mnemonic: &str, operands: &str) -> MemoryAccess {
    if !MEMORY_OPERAND_RE.is_match(operands) {
        return MemoryAccess::None;
    }
    if mnemonic.starts_with("push") {
        return MemoryAccess::Store;
    }
    if mnemonic.starts_with("pop") {
        return MemoryAccess::Load;
    }
    if mnemonic.starts_with("cmp")
        || mnemonic.starts_with("test")
        || mnemonic.starts_with("prefetch")
    {
        return MemoryAccess::Load;
    }
    if mnemonic.starts_with("str") || mnemonic == "stp" || mnemonic == "stur" {
        return MemoryAccess::Store;
    }
    if mnemonic.starts_with("ldr") || mnemonic == "ldp" || mnemonic == "ldur" {
        return MemoryAccess::Load;
    }
    if matches!(mnemonic, "xadd" | "xchg" | "cmpxchg")
        || mnemonic.starts_with("lock")
    {
        return MemoryAccess::LoadStore;
    }

    let first_operand_is_memory = operands.split_once(',').map_or_else(
        || MEMORY_OPERAND_RE.is_match(operands),
        |(first, _)| MEMORY_OPERAND_RE.is_match(first),
    );
    if first_operand_is_memory && writes_first_operand(mnemonic) {
        MemoryAccess::Store
    } else {
        MemoryAccess::Load
    }
}

fn writes_first_operand(mnemonic: &str) -> bool {
    mnemonic.starts_with("mov")
        || mnemonic.starts_with("add")
        || mnemonic.starts_with("sub")
        || mnemonic.starts_with("and")
        || mnemonic.starts_with("or")
        || mnemonic.starts_with("xor")
        || mnemonic.starts_with("inc")
        || mnemonic.starts_with("dec")
        || mnemonic.starts_with("sh")
        || mnemonic.starts_with("sal")
        || mnemonic.starts_with("sar")
        || mnemonic.starts_with("rol")
        || mnemonic.starts_with("ror")
}

fn is_call_mnemonic(mnemonic: &str) -> bool {
    matches!(mnemonic, "call" | "callq" | "bl" | "blr")
}

fn is_return_mnemonic(mnemonic: &str) -> bool {
    mnemonic.starts_with("ret")
}

fn is_conditional_branch_mnemonic(mnemonic: &str) -> bool {
    (mnemonic.starts_with('j') && !matches!(mnemonic, "jmp" | "jmpq"))
        || mnemonic.starts_with("b.")
        || matches!(mnemonic, "cbz" | "cbnz" | "tbz" | "tbnz")
}

fn is_unconditional_branch_mnemonic(mnemonic: &str) -> bool {
    matches!(mnemonic, "jmp" | "jmpq" | "b" | "br")
}

fn is_indirect_control_flow(mnemonic: &str, operands: &str) -> bool {
    let operands = operands.trim().to_ascii_lowercase();
    mnemonic == "blr"
        || mnemonic == "br"
        || operands.starts_with('*')
        || MEMORY_OPERAND_RE.is_match(&operands)
        || is_register_name(
            operands.split_whitespace().next().unwrap_or_default(),
        )
}

fn is_register_name(operand: &str) -> bool {
    let operand = operand.trim_matches(|character: char| {
        character == ',' || character == '[' || character == ']'
    });
    matches!(
        operand,
        "rax"
            | "rbx"
            | "rcx"
            | "rdx"
            | "rsi"
            | "rdi"
            | "rsp"
            | "rbp"
            | "eax"
            | "ebx"
            | "ecx"
            | "edx"
            | "esi"
            | "edi"
            | "esp"
            | "ebp"
            | "x0"
            | "x1"
            | "x2"
            | "x3"
            | "x4"
            | "x5"
            | "x6"
            | "x7"
            | "x8"
            | "x9"
            | "x10"
            | "x11"
            | "x12"
            | "x13"
            | "x14"
            | "x15"
            | "x16"
            | "x17"
            | "x18"
            | "x19"
            | "x20"
            | "x21"
            | "x22"
            | "x23"
            | "x24"
            | "x25"
            | "x26"
            | "x27"
            | "x28"
            | "x29"
            | "x30"
    ) || operand.starts_with('r')
        && operand[1..]
            .chars()
            .all(|character| character.is_ascii_digit())
}

fn is_floating_point_mnemonic(mnemonic: &str) -> bool {
    mnemonic.starts_with('f')
        || mnemonic.starts_with("vadd")
        || mnemonic.starts_with("vsub")
        || mnemonic.starts_with("vmul")
        || mnemonic.starts_with("vdiv")
        || mnemonic.starts_with("addsd")
        || mnemonic.starts_with("addss")
        || mnemonic.starts_with("subsd")
        || mnemonic.starts_with("subss")
        || mnemonic.starts_with("mulsd")
        || mnemonic.starts_with("mulss")
        || mnemonic.starts_with("divsd")
        || mnemonic.starts_with("divss")
}

fn is_simd_vector_op(mnemonic: &str, operands: &str) -> bool {
    mnemonic.starts_with('v')
        || operands.contains("xmm")
        || operands.contains("ymm")
        || operands.contains("zmm")
        || operands.contains(".2")
        || operands.contains(".4")
        || operands.contains(".8")
        || operands.contains(".16")
}

fn is_frame_setup_op(mnemonic: &str, operands: &str) -> bool {
    let operands = collapse_whitespace(&operands.to_ascii_lowercase());
    mnemonic.starts_with("push") && operands == "rbp"
        || mnemonic == "mov" && operands == "rbp, rsp"
        || mnemonic == "sub" && operands.starts_with("rsp,")
        || mnemonic == "stp"
            && operands.contains("x29")
            && operands.contains("sp")
        || mnemonic == "mov" && operands == "x29, sp"
}

fn is_register_move(mnemonic: &str, operands: &str) -> bool {
    if !matches!(mnemonic, "mov" | "movq" | "movl" | "movw" | "movb" | "orr") {
        return false;
    }
    let mut operands = operands.split(',').map(str::trim);
    let Some(first) = operands.next() else {
        return false;
    };
    let Some(second) = operands.next() else {
        return false;
    };
    is_register_name(first) && is_register_name(second)
}

fn is_direct_control_flow_mnemonic(mnemonic: &str) -> bool {
    let mnemonic = mnemonic.to_ascii_lowercase();
    matches!(
        mnemonic.as_str(),
        "call"
            | "jmp"
            | "ja"
            | "jae"
            | "jb"
            | "jbe"
            | "jc"
            | "je"
            | "jg"
            | "jge"
            | "jl"
            | "jle"
            | "jna"
            | "jnae"
            | "jnb"
            | "jnbe"
            | "jne"
            | "jng"
            | "jnge"
            | "jnl"
            | "jnle"
            | "jno"
            | "jnp"
            | "jns"
            | "jnz"
            | "jo"
            | "jp"
            | "jpe"
            | "jpo"
            | "js"
            | "jz"
            | "loop"
            | "loope"
            | "loopne"
            | "loopnz"
            | "loopz"
            | "xbegin"
            | "b"
            | "bl"
            | "cbnz"
            | "cbz"
            | "tbnz"
            | "tbz"
    ) || mnemonic.starts_with("b.")
}
