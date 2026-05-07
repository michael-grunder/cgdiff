use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, DiffTag, TextDiff};

use crate::output::PreparedComparison;

const DIFF_PREFIX_WIDTH: usize = 2;
const SIDE_BY_SIDE_GUTTER_WIDTH: usize = 3;
pub(crate) const DEFAULT_DIFF_CONTEXT: usize = 6;

#[derive(Clone, Debug)]
pub(crate) struct DiffView {
    title: String,
    stacked_lines: Vec<DiffDisplayLine>,
    side_by_side_lines: Vec<SideBySideLine>,
    mode: DiffViewMode,
    scroll: u16,
    horizontal_scroll: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiffViewMode {
    Stacked,
    SideBySide,
}

#[derive(Clone, Debug)]
struct DiffDisplayLine {
    kind: DiffLineKind,
    text: String,
}

#[derive(Clone, Debug)]
struct SideBySideLine {
    left: Option<DiffDisplayLine>,
    right: Option<DiffDisplayLine>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffLineKind {
    Context,
    Fold,
    Added,
    Removed,
    ChangedAdded,
    ChangedRemoved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TokenClass {
    Address,
    Bytes,
    Label,
    Mnemonic,
    Register,
    Immediate,
    Memory,
    Symbol,
    Comment,
    Plain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Token {
    pub(crate) class: TokenClass,
    pub(crate) text: String,
}

impl DiffView {
    pub(crate) fn from_selection_with_context(
        selection: &PreparedComparison,
        diff_context: usize,
    ) -> Self {
        let left = selection.comparison.function1.as_ref().map_or_else(
            || format!("missing function: {}\n", selection.comparison.name),
            |function| function.rendered.clone(),
        );
        let right = selection.comparison.function2.as_ref().map_or_else(
            || format!("missing function: {}\n", selection.comparison.name),
            |function| function.rendered.clone(),
        );

        Self {
            title: format!("Diff {}", selection.comparison.name),
            stacked_lines: diff_lines(&left, &right),
            side_by_side_lines: side_by_side_lines(&left, &right, diff_context),
            mode: DiffViewMode::Stacked,
            scroll: 0,
            horizontal_scroll: 0,
        }
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) const fn scroll(&self) -> (u16, u16) {
        match self.mode {
            DiffViewMode::Stacked => (self.scroll, self.horizontal_scroll),
            DiffViewMode::SideBySide => (self.scroll, 0),
        }
    }

    pub(crate) const fn mode_label(&self) -> &'static str {
        self.mode.label()
    }

    pub(crate) const fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            DiffViewMode::Stacked => DiffViewMode::SideBySide,
            DiffViewMode::SideBySide => DiffViewMode::Stacked,
        };
    }

    pub(crate) const fn scroll_down(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_add(amount);
    }

    pub(crate) const fn scroll_up(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    pub(crate) const fn scroll_right(&mut self, amount: u16) {
        self.horizontal_scroll = self.horizontal_scroll.saturating_add(amount);
    }

    pub(crate) const fn scroll_left(&mut self, amount: u16) {
        self.horizontal_scroll = self.horizontal_scroll.saturating_sub(amount);
    }

    pub(crate) const fn reset_horizontal_scroll(&mut self) {
        self.horizontal_scroll = 0;
    }

    pub(crate) fn rendered_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self.mode {
            DiffViewMode::Stacked => self.rendered_stacked_lines(),
            DiffViewMode::SideBySide => self.rendered_side_by_side_lines(width),
        }
    }

    fn rendered_stacked_lines(&self) -> Vec<Line<'static>> {
        if self.stacked_lines.is_empty() {
            return vec![Line::from("No normalized disassembly differences.")];
        }

        self.stacked_lines
            .iter()
            .map(DiffDisplayLine::render)
            .collect()
    }

    fn rendered_side_by_side_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.side_by_side_lines.is_empty() {
            return vec![Line::from("No normalized disassembly differences.")];
        }

        let width = usize::from(width);
        let fixed_width = (DIFF_PREFIX_WIDTH * 2) + SIDE_BY_SIDE_GUTTER_WIDTH;
        let Some(text_width) =
            width.checked_sub(fixed_width).map(|width| width / 2)
        else {
            return vec![Line::from(
                "Terminal is too narrow for side-by-side diff.",
            )];
        };

        if text_width == 0 {
            return vec![Line::from(
                "Terminal is too narrow for side-by-side diff.",
            )];
        }

        let horizontal_scroll = usize::from(self.horizontal_scroll);
        self.side_by_side_lines
            .iter()
            .map(|line| line.render(text_width, horizontal_scroll))
            .collect()
    }
}

fn diff_lines(left: &str, right: &str) -> Vec<DiffDisplayLine> {
    let changes = TextDiff::from_lines(left, right)
        .iter_all_changes()
        .map(|change| {
            (
                change.tag(),
                change.value().trim_end_matches(['\r', '\n']).to_owned(),
            )
        })
        .collect::<Vec<_>>();

    changes
        .iter()
        .enumerate()
        .map(|(index, (tag, text))| {
            display_line(diff_line_kind(&changes, index, *tag), text)
        })
        .collect()
}

fn side_by_side_lines(
    left: &str,
    right: &str,
    diff_context: usize,
) -> Vec<SideBySideLine> {
    let diff = TextDiff::from_lines(left, right);
    let old_lines = diff.old_slices();
    let new_lines = diff.new_slices();
    let mut lines = Vec::new();
    let ops = diff.ops();

    for (op_index, op) in ops.iter().enumerate() {
        let (tag, old_range, new_range) = op.as_tag_tuple();
        match tag {
            DiffTag::Equal => {
                lines.extend(folded_equal_lines(
                    old_lines,
                    new_lines,
                    old_range,
                    new_range,
                    EqualBlockPosition {
                        is_first: op_index == 0,
                        is_last: op_index + 1 == ops.len(),
                        is_only: ops.len() == 1,
                    },
                    diff_context,
                ));
            }
            DiffTag::Delete => {
                lines.extend(old_range.map(|old| SideBySideLine {
                    left: Some(display_line(
                        DiffLineKind::Removed,
                        old_lines[old],
                    )),
                    right: None,
                }));
            }
            DiffTag::Insert => {
                lines.extend(new_range.map(|new| SideBySideLine {
                    left: None,
                    right: Some(display_line(
                        DiffLineKind::Added,
                        new_lines[new],
                    )),
                }));
            }
            DiffTag::Replace => {
                let old_lines = &old_lines[old_range];
                let new_lines = &new_lines[new_range];
                let line_count = old_lines.len().max(new_lines.len());
                lines.extend((0..line_count).map(|index| SideBySideLine {
                    left: old_lines.get(index).map(|line| {
                        display_line(DiffLineKind::ChangedRemoved, line)
                    }),
                    right: new_lines.get(index).map(|line| {
                        display_line(DiffLineKind::ChangedAdded, line)
                    }),
                }));
            }
        }
    }

    lines
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EqualBlockPosition {
    is_first: bool,
    is_last: bool,
    is_only: bool,
}

fn folded_equal_lines(
    old_lines: &[&str],
    new_lines: &[&str],
    old_range: std::ops::Range<usize>,
    new_range: std::ops::Range<usize>,
    position: EqualBlockPosition,
    diff_context: usize,
) -> Vec<SideBySideLine> {
    let len = old_range.len();
    if position.is_only || len <= diff_context {
        return old_range
            .zip(new_range)
            .map(|(old, new)| {
                equal_side_by_side_line(old_lines[old], new_lines[new])
            })
            .collect();
    }

    let keep_head = if position.is_first {
        0
    } else {
        diff_context.min(len)
    };
    let keep_tail = if position.is_last {
        0
    } else {
        diff_context.min(len.saturating_sub(keep_head))
    };
    let kept = keep_head.saturating_add(keep_tail);
    if kept >= len {
        return old_range
            .zip(new_range)
            .map(|(old, new)| {
                equal_side_by_side_line(old_lines[old], new_lines[new])
            })
            .collect();
    }

    let fold_start_offset = keep_head;
    let fold_end_offset = len - keep_tail;
    let mut lines = Vec::with_capacity(kept.saturating_add(1));

    lines.extend(
        old_range
            .clone()
            .take(keep_head)
            .zip(new_range.clone().take(keep_head))
            .map(|(old, new)| {
                equal_side_by_side_line(old_lines[old], new_lines[new])
            }),
    );
    lines.push(fold_side_by_side_line(
        fold_end_offset - fold_start_offset,
        old_lines[old_range.start + fold_start_offset],
    ));
    lines.extend(
        old_range
            .skip(fold_end_offset)
            .zip(new_range.skip(fold_end_offset))
            .map(|(old, new)| {
                equal_side_by_side_line(old_lines[old], new_lines[new])
            }),
    );

    lines
}

fn equal_side_by_side_line(old_line: &str, new_line: &str) -> SideBySideLine {
    SideBySideLine {
        left: Some(display_line(DiffLineKind::Context, old_line)),
        right: Some(display_line(DiffLineKind::Context, new_line)),
    }
}

fn fold_side_by_side_line(
    folded_count: usize,
    first_folded_line: &str,
) -> SideBySideLine {
    let text = fold_line_text(folded_count, first_folded_line);
    SideBySideLine {
        left: Some(display_line(DiffLineKind::Fold, &text)),
        right: Some(display_line(DiffLineKind::Fold, &text)),
    }
}

fn fold_line_text(folded_count: usize, first_folded_line: &str) -> String {
    let noun = if folded_count == 1 { "line" } else { "lines" };
    let preview = trim_diff_line(first_folded_line).trim();
    if preview.is_empty() {
        format!("+-- {folded_count:>2} {noun}")
    } else {
        format!("+-- {folded_count:>2} {noun}: {preview}")
    }
}

fn display_line(kind: DiffLineKind, text: &str) -> DiffDisplayLine {
    DiffDisplayLine {
        kind,
        text: trim_diff_line(text).to_owned(),
    }
}

fn trim_diff_line(text: &str) -> &str {
    text.trim_end_matches(['\r', '\n'])
}

fn diff_line_kind(
    changes: &[(ChangeTag, String)],
    index: usize,
    tag: ChangeTag,
) -> DiffLineKind {
    if tag == ChangeTag::Equal {
        return DiffLineKind::Context;
    }

    let mut start = index;
    while start > 0 && changes[start - 1].0 != ChangeTag::Equal {
        start -= 1;
    }
    let mut end = index + 1;
    while end < changes.len() && changes[end].0 != ChangeTag::Equal {
        end += 1;
    }

    let replacement_block = changes[start..end]
        .iter()
        .any(|(tag, _)| *tag == ChangeTag::Delete)
        && changes[start..end]
            .iter()
            .any(|(tag, _)| *tag == ChangeTag::Insert);

    match tag {
        ChangeTag::Equal => DiffLineKind::Context,
        ChangeTag::Delete if replacement_block => DiffLineKind::ChangedRemoved,
        ChangeTag::Delete => DiffLineKind::Removed,
        ChangeTag::Insert if replacement_block => DiffLineKind::ChangedAdded,
        ChangeTag::Insert => DiffLineKind::Added,
    }
}

impl DiffDisplayLine {
    fn render(&self) -> Line<'static> {
        let mut spans = Vec::new();
        spans.push(Span::styled(self.kind.prefix(), self.kind.prefix_style()));
        spans.extend(highlight_asm_with_background(
            &self.text,
            self.kind.background(),
        ));
        Line::from(spans)
    }
}

impl SideBySideLine {
    fn render(
        &self,
        text_width: usize,
        horizontal_scroll: usize,
    ) -> Line<'static> {
        let mut spans = Vec::new();
        spans.extend(render_side_by_side_column(
            self.left.as_ref(),
            text_width,
            horizontal_scroll,
        ));
        spans.push(Span::styled(" | ", Style::new().fg(Color::DarkGray)));
        spans.extend(render_side_by_side_column(
            self.right.as_ref(),
            text_width,
            horizontal_scroll,
        ));
        Line::from(spans)
    }
}

impl DiffViewMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Stacked => "stacked",
            Self::SideBySide => "side-by-side",
        }
    }
}

impl DiffLineKind {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Context | Self::Fold => "  ",
            Self::Added | Self::ChangedAdded => "+ ",
            Self::Removed | Self::ChangedRemoved => "- ",
        }
    }

    const fn prefix_style(self) -> Style {
        match self {
            Self::Context | Self::Fold => Style::new().fg(Color::DarkGray),
            Self::Added => Style::new()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
            Self::Removed => Style::new()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
            Self::ChangedAdded | Self::ChangedRemoved => Style::new()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        }
    }

    const fn background(self) -> Option<Color> {
        match self {
            Self::Context | Self::Fold => None,
            Self::Added => Some(Color::Rgb(8, 48, 28)),
            Self::Removed => Some(Color::Rgb(58, 24, 30)),
            Self::ChangedAdded | Self::ChangedRemoved => {
                Some(Color::Rgb(55, 45, 18))
            }
        }
    }
}

fn render_side_by_side_column(
    line: Option<&DiffDisplayLine>,
    text_width: usize,
    horizontal_scroll: usize,
) -> Vec<Span<'static>> {
    let Some(line) = line else {
        return vec![
            Span::styled("  ", Style::new().fg(Color::DarkGray)),
            Span::raw(" ".repeat(text_width)),
        ];
    };

    let visible_text = visible_text(&line.text, horizontal_scroll, text_width);
    let visible_width = visible_text.chars().count();
    let padding_width = text_width.saturating_sub(visible_width);
    let mut spans = Vec::new();
    spans.push(Span::styled(line.kind.prefix(), line.kind.prefix_style()));
    spans.extend(highlight_asm_with_background(
        &visible_text,
        line.kind.background(),
    ));
    if padding_width > 0 {
        spans.push(Span::styled(
            " ".repeat(padding_width),
            apply_background(Style::new(), line.kind.background()),
        ));
    }
    spans
}

fn visible_text(text: &str, start: usize, width: usize) -> String {
    text.chars().skip(start).take(width).collect()
}

fn highlight_asm_with_background(
    text: &str,
    background: Option<Color>,
) -> Vec<Span<'static>> {
    tokenize_asm(text)
        .into_iter()
        .map(|token| {
            Span::styled(
                token.text,
                apply_background(token_style(token.class), background),
            )
        })
        .collect()
}

pub(crate) fn tokenize_asm(text: &str) -> Vec<Token> {
    let (leading, trimmed_start) = split_leading_whitespace(text);
    let mut tokens = Vec::new();
    push_token(&mut tokens, TokenClass::Plain, leading);

    if is_label_line(trimmed_start) {
        push_token(&mut tokens, TokenClass::Label, trimmed_start);
        return tokens;
    }

    let (code, comment) = split_comment(trimmed_start);
    tokenize_code(code, &mut tokens);
    if let Some(comment) = comment {
        push_token(&mut tokens, TokenClass::Comment, comment);
    }

    tokens
}

fn tokenize_code(mut code: &str, tokens: &mut Vec<Token>) {
    let mut saw_mnemonic = false;

    while !code.is_empty() {
        if let Some((address, remainder)) = take_address_prefix(code) {
            push_token(tokens, TokenClass::Address, address);
            code = remainder;
            continue;
        }

        if let Some((memory, remainder)) = take_memory_operand(code) {
            push_token(tokens, TokenClass::Memory, memory);
            code = remainder;
            continue;
        }

        let Some((part, remainder)) = take_next_part(code) else {
            break;
        };
        code = remainder;

        if part.chars().all(char::is_whitespace) {
            push_token(tokens, TokenClass::Plain, part);
            continue;
        }

        if is_separator(part) {
            push_token(tokens, TokenClass::Plain, part);
            continue;
        }

        let class = classify_word(part, saw_mnemonic);
        if class == TokenClass::Mnemonic {
            saw_mnemonic = true;
        }
        push_token(tokens, class, part);
    }
}

fn split_leading_whitespace(text: &str) -> (&str, &str) {
    let index = text
        .char_indices()
        .find_map(|(index, character)| {
            (!character.is_whitespace()).then_some(index)
        })
        .unwrap_or(text.len());
    text.split_at(index)
}

fn is_label_line(text: &str) -> bool {
    let trimmed = text.trim_end();
    trimmed.ends_with(':') && !trimmed.contains(char::is_whitespace)
}

fn split_comment(text: &str) -> (&str, Option<&str>) {
    let mut previous_was_whitespace = true;
    for (index, character) in text.char_indices() {
        if matches!(character, '#' | ';') && previous_was_whitespace {
            let (code, comment) = text.split_at(index);
            return (code, Some(comment));
        }
        previous_was_whitespace = character.is_whitespace();
    }

    (text, None)
}

fn take_address_prefix(text: &str) -> Option<(&str, &str)> {
    let (candidate, _remainder) = text.split_once(':')?;
    let trimmed = candidate.trim_start();
    if trimmed.is_empty()
        || trimmed.contains(char::is_whitespace)
        || !is_hex_number(trimmed)
    {
        return None;
    }

    Some(text.split_at(candidate.len() + 1))
}

fn take_memory_operand(text: &str) -> Option<(&str, &str)> {
    let open = text.chars().next()?;
    let close = match open {
        '[' => ']',
        '(' => ')',
        _ => return None,
    };
    let end = text.find(close)?;
    Some(text.split_at(end + close.len_utf8()))
}

fn take_next_part(text: &str) -> Option<(&str, &str)> {
    let first = text.chars().next()?;
    if first.is_whitespace() {
        let end = text
            .char_indices()
            .find_map(|(index, character)| {
                (!character.is_whitespace()).then_some(index)
            })
            .unwrap_or(text.len());
        return Some(text.split_at(end));
    }

    if is_separator_char(first) {
        let end = first.len_utf8();
        return Some(text.split_at(end));
    }

    let end = text
        .char_indices()
        .skip(1)
        .find_map(|(index, character)| {
            (character.is_whitespace()
                || is_separator_char(character)
                || matches!(character, '[' | '('))
            .then_some(index)
        })
        .unwrap_or(text.len());
    Some(text.split_at(end))
}

fn classify_word(part: &str, saw_mnemonic: bool) -> TokenClass {
    if !saw_mnemonic {
        return if is_byte_token(part) {
            TokenClass::Bytes
        } else {
            TokenClass::Mnemonic
        };
    }

    let normalized = part
        .trim_start_matches(['%', '$', '#'])
        .trim_end_matches(':')
        .to_ascii_lowercase();

    if is_register(&normalized) {
        TokenClass::Register
    } else if is_immediate(part) {
        TokenClass::Immediate
    } else if is_memory_keyword(&normalized) {
        TokenClass::Memory
    } else if is_symbol(part) {
        TokenClass::Symbol
    } else {
        TokenClass::Plain
    }
}

fn is_separator(part: &str) -> bool {
    part.chars().all(is_separator_char)
}

const fn is_separator_char(character: char) -> bool {
    matches!(character, ',' | '+' | '-' | '*' | '/')
}

fn is_byte_token(part: &str) -> bool {
    part.len() == 2
        && part.chars().all(|character| character.is_ascii_hexdigit())
}

fn is_hex_number(part: &str) -> bool {
    let hex = part
        .strip_prefix("0x")
        .or_else(|| part.strip_prefix("0X"))
        .unwrap_or(part);
    !hex.is_empty()
        && hex.chars().all(|character| character.is_ascii_hexdigit())
}

fn is_immediate(part: &str) -> bool {
    let value = part.trim_start_matches(['$', '#']);
    let value = value
        .strip_prefix('-')
        .or_else(|| value.strip_prefix('+'))
        .unwrap_or(value);
    is_hex_number(value)
        || value.chars().all(|character| character.is_ascii_digit())
}

fn is_symbol(part: &str) -> bool {
    part.starts_with("sym:")
        || part.starts_with("data:")
        || (part.starts_with('.') && part.len() > 1)
        || part.contains('@')
        || (part.contains('<') && part.contains('>'))
}

fn is_memory_keyword(part: &str) -> bool {
    matches!(
        part,
        "byte"
            | "word"
            | "dword"
            | "qword"
            | "tword"
            | "oword"
            | "xword"
            | "yword"
            | "zword"
            | "ptr"
            | "offset"
    )
}

fn is_register(part: &str) -> bool {
    REGISTERS.iter().any(|register| register == part)
        || is_aarch64_register(part)
}

fn is_aarch64_register(part: &str) -> bool {
    matches!(part, "sp" | "lr" | "fp" | "xzr" | "wzr")
        || is_prefixed_register(part, 'x', 30)
        || is_prefixed_register(part, 'w', 30)
        || is_prefixed_register(part, 'v', 31)
        || is_prefixed_register(part, 'q', 31)
        || is_prefixed_register(part, 'd', 31)
        || is_prefixed_register(part, 's', 31)
}

fn is_prefixed_register(part: &str, prefix: char, max: u8) -> bool {
    let Some(suffix) = part.strip_prefix(prefix) else {
        return false;
    };
    suffix.parse::<u8>().is_ok_and(|register| register <= max)
}

fn push_token(tokens: &mut Vec<Token>, class: TokenClass, text: &str) {
    if text.is_empty() {
        return;
    }
    tokens.push(Token {
        class,
        text: text.to_owned(),
    });
}

const fn token_style(class: TokenClass) -> Style {
    match class {
        TokenClass::Address | TokenClass::Comment => {
            Style::new().fg(Color::DarkGray)
        }
        TokenClass::Bytes => Style::new().fg(Color::Gray),
        TokenClass::Label => {
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        }
        TokenClass::Mnemonic => {
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        }
        TokenClass::Register => Style::new().fg(Color::LightBlue),
        TokenClass::Immediate => Style::new().fg(Color::LightMagenta),
        TokenClass::Memory => Style::new().fg(Color::LightCyan),
        TokenClass::Symbol => Style::new().fg(Color::LightGreen),
        TokenClass::Plain => Style::new(),
    }
}

fn apply_background(style: Style, background: Option<Color>) -> Style {
    background.map_or(style, |color| style.bg(color))
}

static REGISTERS: LazyLock<Vec<String>> = LazyLock::new(|| {
    let mut registers = [
        "al", "ah", "ax", "eax", "rax", "bl", "bh", "bx", "ebx", "rbx", "cl",
        "ch", "cx", "ecx", "rcx", "dl", "dh", "dx", "edx", "rdx", "si", "esi",
        "rsi", "sil", "di", "edi", "rdi", "dil", "bp", "ebp", "rbp", "bpl",
        "sp", "esp", "rsp", "spl", "ip", "eip", "rip", "flags", "eflags",
        "rflags", "cs", "ds", "es", "fs", "gs", "ss", "st", "st0", "st1",
        "st2", "st3", "st4", "st5", "st6", "st7", "mm0", "mm1", "mm2", "mm3",
        "mm4", "mm5", "mm6", "mm7",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();

    for prefix in ["r", "xmm", "ymm", "zmm", "k"] {
        for index in 0..=31 {
            registers.push(format!("{prefix}{index}"));
        }
    }

    registers
});
