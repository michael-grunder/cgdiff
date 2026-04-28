# cgdiff

`cgdiff` compares code generation between two compiled binaries. It disassembles
both inputs, matches functions by name, scores how similar their instruction
streams are, and presents the most different functions first.

The default interface is an interactive terminal UI. A `--stdio` mode is also
available for scripts or quick checks.

## Features

- Compares executables or shared objects with `llvm-objdump` or GNU `objdump`.
- Scores each shared function with combined, instruction-count, and
  instruction-order similarity values.
- Sorts functions from least similar to most similar.
- Hides unique and effectively identical functions by default so the TUI starts
  on likely codegen differences.
- Opens the selected function pair in a configured diff editor.
- Normalizes unstable disassembly details, including instruction addresses,
  local branch targets, symbol call targets, and RIP-relative data references.
- Supports case-insensitive substring filtering and `/regex/` filtering.
- Provides `--stdio` output for non-interactive use.
- Embeds build metadata in `--version`, including build date and git SHA.

## Requirements

- Rust 2024 toolchain.
- `llvm-objdump` or `objdump` available in `PATH`, unless provided with
  `--objdump`.
- A diff-capable editor for the interactive flow. The default command is
  `nvim -d {file1} {file2}`.

## Installation

Build from the repository:

```bash
cargo build --release
```

The binary will be available at:

```bash
target/release/cgdiff
```

## Usage

```bash
cgdiff [OPTIONS] <BINARY1> <BINARY2>
```

Example:

```bash
cgdiff ./old/libexample.so ./new/libexample.so
```

Use a specific disassembler:

```bash
cgdiff --objdump /usr/bin/llvm-objdump ./old/app ./new/app
```

Use a different editor command:

```bash
cgdiff --editor "vimdiff {file1} {file2}" ./old/app ./new/app
```

Print a sorted table instead of opening the TUI:

```bash
cgdiff --stdio ./old/app ./new/app
```

Filter functions before comparison:

```bash
cgdiff --filter relay ./old/app ./new/app
cgdiff --filter '/^relay|worker/' ./old/app ./new/app
```

Show functions that are hidden by default:

```bash
cgdiff --include-unique-functions --include-identical-functions ./old/app ./new/app
```

## Options

```text
Usage: cgdiff [OPTIONS] <BINARY1> <BINARY2>

Arguments:
  <BINARY1>  First binary to compare
  <BINARY2>  Second binary to compare

Options:
  -o, --objdump <OBJDUMP>            Path to objdump program
  -e, --editor <EDITOR>              Command used to launch the diff editor
  -d, --diff-mode <DIFF_MODE>        Sort mode: combined, count, or order
      --include-unique-functions     Include functions only present in one binary
      --include-identical-functions  Include identical or perfect-score functions
      --filter <FILTER>              Pre-filter by substring or `/regex/`
      --stdio                        Dump a sorted table to stdout
  -h, --help                         Print help
  -V, --version                      Print version
```

## TUI Controls

- `j` / `Down`: select next function.
- `k` / `Up`: select previous function.
- `/`: filter visible functions by substring or `/regex/`.
- `1`: sort by combined score.
- `2`: sort by instruction-count score.
- `3`: sort by instruction-order score.
- `i`: show or hide details for the selected function.
- `Enter`: open the selected pair in the configured diff editor.
- `?`: show help.
- `q` / `Esc`: quit.

## Similarity Scores

`cgdiff` calculates three scores for each function:

- `count`: weighted Jaccard similarity over mnemonic counts.
- `order`: longest common subsequence similarity over mnemonic order.
- `combined`: weighted aggregate of order and count scores.

Lower scores mean larger differences. Results are sorted with the lowest score
first.

## Disassembly Normalization

Disassembly often changes when addresses move even if the generated code is
effectively the same. `cgdiff` normalizes rendered function text before editor
diffs and identity checks:

- Per-instruction addresses are removed.
- Intra-function control-flow targets become stable local labels.
- Calls and jumps to named external symbols become symbolic targets.
- RIP-relative data comments become symbolic data references.

This keeps the diff focused on meaningful instruction changes instead of layout
noise.

## Development

Run the standard checks before submitting changes:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

The repository also includes design notes in `specs/`.

## License

This project is licensed under the MIT License. See `LICENSE` for details.
