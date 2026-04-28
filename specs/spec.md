## cgdiff

The purpose of this application is to compare the codegen from two different
binaries (e.g. shared objects or executables).

### Usage
```bash
cgdiff [OPTIONS] <binary1> <binary2>

Options:
    -h, --help             Print help information
    -V, --version          Print version information
    -o, --objdump <PATH>   Path to objdump program (default:
                           llvm-objdump if available, otherwise objdump)
    -e, --editor <CMD>     Command to open editor (default: "nvim -d {file1} {file2}")
    -d, --diff-mode <MODE> combined (default), count, order
```

### Process

This application should use the configured `objdump` program to disassemble the
code. Initially we can just support either `objdump` or `llvm-objdump`. Prefer
`llvm-objdump` if available unless the user explicitly specifies `objdump`.

Then for each function in each binary we should compute a disassembly similarity
score using roughly this algorithm:

```python
from collections import Counter

def lcs_len(a, b):
    if len(a) < len(b):
        a, b = b, a

    prev = [0] * (len(b) + 1)

    for x in a:
        cur = [0]
        for j, y in enumerate(b, 1):
            if x == y:
                cur.append(prev[j - 1] + 1)
            else:
                cur.append(max(prev[j], cur[-1]))
        prev = cur

    return prev[-1]


def weighted_jaccard(a, b):
    ca = Counter(a)
    cb = Counter(b)
    keys = ca.keys() | cb.keys()

    inter = sum(min(ca[k], cb[k]) for k in keys)
    union = sum(max(ca[k], cb[k]) for k in keys)

    return inter / union if union else 1.0


def asm_similarity(a, b, order_weight=0.70):
    if not a and not b:
        return 1.0

    lcs = lcs_len(a, b)
    order_score = 2 * lcs / (len(a) + len(b))
    count_score = weighted_jaccard(a, b)

    return order_weight * order_score + (1.0 - order_weight) * count_score
```

So we're generating both an instruction count similarity score and an instruction order similarity score. We should then deriver an aggregate score, but keep the other two specific scores in case the user wants to order "similarity" by that.

So for example they may want to see the most different function by operation count instead of just "similarity".

Given that this is Rust, we can run the objdump and aggregation operations in parallel. We should provide rich output while we're processing the files, e.g. outputting
on a line by bytes processed as we go, etc.

Once we've extracted all the functions we should output a table ordered by similarity in reverse (most different first). By default this should be TUI-like where the user can select a function to see the details. Use ratatui for the tui stuff.

When the user selects a function it should open the configured editor which we can default to `nvim -d ${file1} ${file2}`.
