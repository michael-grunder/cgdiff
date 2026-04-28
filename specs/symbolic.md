Implement v1 address/symbol normalization for cgdiff.

Context:
- The current tool parses objdump output, groups instructions by function,
  stores:
  - instructions: Vec<String> containing only mnemonics
  - normalized_instructions: Vec<String> containing instruction text
  - rendered: String containing the original objdump function text
- Similarity currently uses only mnemonic sequences, while identical-function
  detection uses normalized_instructions.
- parse_instruction_text currently extracts the post-address instruction text
  from objdump lines. Existing code and tests are in the attached file.

### Goal

Normalize unstable absolute addresses in disassembly so diffs are not polluted
when a function or binary layout moves.

The editor diff should show normalized disassembly. The similarity/identity
logic should also use normalized instruction text where appropriate.

### Scope for v1

1. Strip per-instruction addresses from rendered function output.
2. Replace intra-function branch targets with stable local labels.
3. Replace call/jmp targets to named symbols with symbolic targets.
4. Replace RIP-relative comments that name data symbols with symbolic operands.
5. Keep implementation architecture-light and objdump-text based.
6. Preserve current CLI behavior; no new flags required for v1.

### Important examples

#### Input

```asm
00000000000b7ab0 <relayExec>:
   b7add:       jne     0xb7b78 <relayExec+0xc8>
   b7af6:       call    0x4dac0 <redisAppendFormattedCommand$plt>
   b7c46:       call    0x70bf0 <relayUsTime>
   b7aef:       lea     rsi, [rip - 0x6c46c] # 0x4b68a <.LC765>
```

#### Output should be conceptually like

```asm
<relayExec>:
    jne .L0008
    call sym:redisAppendFormattedCommand$plt
    call sym:relayUsTime
    lea rsi, [rip + data:.LC765]
```

The exact local label format may differ, but it must be deterministic within
a parsed function.

### Implementation design:

Add an intermediate representation for parsed instructions.

```rust

struct ParsedInstruction {
    original_line: String,
    address: Option<u64>,
    text: String,
}

struct FunctionBuilder {
    name: String,
    header_line: String,
    lines: Vec<String>,
    instructions: Vec<ParsedInstruction>,
}
```

Keep FunctionDisassembly as the final stored result unless a small extension is
useful.

Parsing phase:
- While parsing a function, collect ParsedInstruction entries instead of
  immediately finalizing normalized strings.
- parse_instruction_text may remain, but add a parser that also returns the
  numeric instruction address.
- For lines that are inside a function but not instructions, preserve them only
  if useful. v1 may omit blank/non-instruction lines from normalized rendered
  output except the function header.

### Address parsing


Parse instruction lines of the form:

    optional whitespace
    hex address
    :
    rest of line

The address parser should accept addresses with or without 0x.
Examples:
    "b7add:"
    "00000000000b7ab0:"

Because objdump is currently invoked with --no-show-raw-insn, the remainder
should usually already be the instruction text. Keep the existing robust
rsplit('\t') behavior so current tests keep passing.

### Local target map

When finalizing a function, build:

    HashMap<u64, String> local_labels

For every parsed instruction address, map:

    address -> format!(".L{:04}", instruction_index)

Then normalize branch/call/jmp operands.

An address is considered intra-function if:
- it appears as an instruction address in the current function's address map

For v1, this is better than range checks because it avoids accidentally
labeling data addresses or cold-section addresses as local code.

### Instruction normalization rules

Normalize only instruction text, not raw source lines.

Given instruction text:
    "<mnemonic> <operands/comments>"

1. Remove comments after '#', but first use comments that contain symbol info
   to normalize RIP-relative data references.

2. Normalize direct control-flow targets.

   If operand contains:
       0xADDR <SYMBOL>
       ADDR <SYMBOL>

   Then:
   - if ADDR exists in local_labels:
       replace the whole target with local_labels[ADDR]
   - else if SYMBOL exists:
       replace with "sym:<SYMBOL>"
   - else:
       replace with "addr:<hex-normalized>" or "addr:unknown"

3. For direct branch targets without symbol comments:
       jne 0xb7b78
   If address exists in local_labels:
       jne .L0008
   Otherwise:
       jne addr:external

4. For named symbol calls:
       call 0x4dac0 <redisAppendFormattedCommand$plt>
   becomes:
       call sym:redisAppendFormattedCommand$plt

5. For RIP-relative data comments:
       lea rsi, [rip - 0x6c46c] # 0x4b68a <.LC765>
   becomes:
       lea rsi, [rip + data:.LC765]

   Simpler acceptable v1:
       lea rsi, [rip + sym:.LC765]

   Do not keep the absolute 0x4b68a.

### Mnemonic sequence

For now, keep FunctionDisassembly.instructions as mnemonics only, preserving
the existing scoring behavior.

But normalized_instructions should become the fully normalized instruction text,
not the current raw instruction text.

FunctionComparison::is_identical() should therefore stop reporting differences
caused only by address movement.

### Renderd diff output

prepare_comparisons should write normalized rendered disassembly to temp files,
not raw objdump text.

FunctionDisassembly.rendered should become normalized rendered text.

Normalized rendered format:

    <function_name>:
        <normalized instruction text>
        <normalized instruction text>

Do not include per-instruction addresses in the normalized rendered output.

Optionally include local labels before target instructions:

    .L0008:
        test al, 0x1

This is useful when reading editor diffs. For v1, include labels only for
instructions that are actually branch targets. Do not label every instruction
unless that is simpler.

### Suggest5ed label behavior

Collect all intra-function target addresses used by any instruction.

When rendering:
- before an instruction whose address is in target_addresses, emit:
      .Lxxxx:
- then emit:
      "    {normalized_instruction}"

This keeps diffs readable without adding hundreds of labels.

### Regex helpers

Use small compiled regexes or simple parsing helpers.

Useful patterns:

Symbol target:
    (?P<addr>0x[0-9a-fA-F]+|[0-9a-fA-F]+)\s+<(?P<sym>[^>]+)>

Hex address:
    0x[0-9a-fA-F]+

RIP data comment:
    #\s*(?P<addr>0x[0-9a-fA-F]+|[0-9a-fA-F]+)\s+<(?P<sym>[^>]+)>

### Be careful

    Do not replace structure offsets or immediates like:
    [r15 + 0x350]
    cmp eax, 0x6
    add rsp, 0x68

Only normalize immediates that are clearly control-flow targets or are part of
an objdump symbol annotation/comment.

### Control-flow nmemonics

Treat these as direct target mnemonics:
- call
- jmp
- ja, jae, jb, jbe, jc, je, jg, jge, jl, jle, jna, jnae, jnb, jnbe
- jne, jng, jnge, jnl, jnle, jno, jnp, jns, jnz, jo, jp, jpe, jpo
- js, jz
- loop, loope, loopne, loopnz, loopz
- xbegin

Case-insensitive matching is fine, but objdump probably emits lowercase.

### External/cold symbols:

For v1, any named target not in the current function address map becomes:

    sym:<symbol>

Examples:
    relayExec.cold+0x26      -> sym:relayExec.cold+0x26
    redisGetReply$plt        -> sym:redisGetReply$plt
    relay_exception_ce       -> sym:relay_exception_ce

Do not try to distinguish PLT/GOT/external/local object symbols yet unless
that falls out naturally.

### Testing requirements:

Add unit tests for:

1. Instruction address parsing:
   input:
       "   b7add:\tjne\t0xb7b78 <relayExec+0xc8>"
   expects:
       address = 0xb7add
       text = "jne\t0xb7b78 <relayExec+0xc8>" or normalized whitespace
       mnemonic = "jne"

2. Intra-function jump normalization:
   function has instructions at 0x1000 and 0x1010
   instruction:
       "jne 0x1010 <foo+0x10>"
   normalized:
       "jne .L0001"

3. Symbol call normalization:
       "call 0x4dac0 <redisAppendFormattedCommand$plt>"
   normalized:
       "call sym:redisAppendFormattedCommand$plt"

4. RIP data normalization:
       "lea rsi, [rip - 0x6c46c] # 0x4b68a <.LC765>"
   normalized must not contain:
       "0x4b68a"
       "- 0x6c46c"
   normalized must contain:
       ".LC765"

5. Non-target immediates are preserved:
       "cmp eax, 0x6"
       "mov rax, qword ptr [rsi + 0x350]"
   should keep those numeric constants.

6. Rendered output strips instruction addresses:
   normalized rendered output must not contain:
       "1000:"
       "1010:"
   and should contain:
       "<foo>:"
       "jne .L0001"

7. Identical detection ignores moved function addresses:
   left:
       1000: jne 0x1010 <foo+0x10>
       1010: ret
   right:
       2000: jne 0x2010 <foo+0x10>
       2010: ret
   normalized_instructions equal.

### Non-goals for v1:

- No CFG matching.
- No basic-block isomorphism.
- No operand-class similarity scoring.
- No relocation-table parsing.
- No DWARF parsing.
- No cross-function identity matching for renamed/static symbols.
- No attempt to normalize all numeric constants.
- No architecture-specific decoder dependency.

### Acceptance criteria:

- Current tests continue to pass.
- New tests pass.
- Comparing the same function moved to a different address should not produce
  address-only editor diffs.
- Branches within the same function normalize to stable local labels.
- Calls/jumps to named symbols normalize to symbol names.
- RIP-relative data references with objdump comments normalize to symbol names.
- Numeric constants used as ordinary immediates or struct offsets remain intact.

### Notes

The current code already has the right insertion points: parse_objdump_output
builds per-function instruction vectors, FunctionDisassembly.rendered feeds
the editor temp files, and normalized_instructions drives exact identity
checks. So this can be implemented mostly by changing finalization from “store
raw instruction text” to “store normalized instruction text/rendering.”
