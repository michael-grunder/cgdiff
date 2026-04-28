use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{LazyLock, mpsc};
use std::thread;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;

use crate::progress::{
    ProgressEvent, send_progress_finished, send_progress_processed,
    send_progress_start,
};

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

#[derive(Clone, Debug)]
pub(crate) struct BinaryAnalysis {
    pub(crate) functions: HashMap<String, FunctionDisassembly>,
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionDisassembly {
    pub(crate) instructions: Vec<String>,
    pub(crate) normalized_instructions: Vec<String>,
    pub(crate) rendered: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedInstruction {
    pub(crate) original_line: String,
    pub(crate) address: Option<u64>,
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
    let mut command = Command::new(objdump);
    command
        .arg("--disassemble")
        .arg("--demangle")
        .arg("--no-show-raw-insn")
        .args(x86_intel_syntax_args(objdump))
        .arg(binary_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn x86_intel_syntax_args(objdump: &Path) -> &'static [&'static str] {
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
    let text = parse_instruction_remainder(remainder)?;

    Some(ParsedInstruction {
        original_line: line.to_owned(),
        address: Some(address),
        text,
    })
}

fn parse_instruction_remainder(remainder: &str) -> Option<String> {
    let trimmed = remainder.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((first_column, rest)) = trimmed.split_once('\t')
        && is_raw_byte_column(first_column)
    {
        let instruction = rest.trim();
        return (!instruction.is_empty()).then(|| instruction.to_owned());
    }

    Some(trimmed.to_owned())
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
                text: format!("{mnemonic} {label}"),
                local_target: Some(address),
            };
        }

        return NormalizedInstruction {
            text: format!("{mnemonic} sym:{symbol}"),
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

fn is_direct_control_flow_mnemonic(mnemonic: &str) -> bool {
    matches!(
        mnemonic.to_ascii_lowercase().as_str(),
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
    )
}
