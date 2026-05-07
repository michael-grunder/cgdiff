use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};

use crate::output::PreparedComparison;

#[derive(Clone, Debug)]
pub(crate) struct DiffView {
    title: String,
    lines: Vec<DiffDisplayLine>,
    scroll: u16,
    horizontal_scroll: u16,
}

#[derive(Clone, Debug)]
struct DiffDisplayLine {
    kind: DiffLineKind,
    text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffLineKind {
    Context,
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
    pub(crate) fn from_selection(selection: &PreparedComparison) -> Self {
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
            lines: diff_lines(&left, &right),
            scroll: 0,
            horizontal_scroll: 0,
        }
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) const fn scroll(&self) -> (u16, u16) {
        (self.scroll, self.horizontal_scroll)
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

    pub(crate) fn rendered_lines(&self) -> Vec<Line<'static>> {
        if self.lines.is_empty() {
            return vec![Line::from("No normalized disassembly differences.")];
        }

        self.lines.iter().map(DiffDisplayLine::render).collect()
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
        .map(|(index, (tag, text))| DiffDisplayLine {
            kind: diff_line_kind(&changes, index, *tag),
            text: text.clone(),
        })
        .collect()
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

impl DiffLineKind {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Context => "  ",
            Self::Added | Self::ChangedAdded => "+ ",
            Self::Removed | Self::ChangedRemoved => "- ",
        }
    }

    const fn prefix_style(self) -> Style {
        match self {
            Self::Context => Style::new().fg(Color::DarkGray),
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
            Self::Context => None,
            Self::Added => Some(Color::Rgb(8, 48, 28)),
            Self::Removed => Some(Color::Rgb(58, 24, 30)),
            Self::ChangedAdded | Self::ChangedRemoved => {
                Some(Color::Rgb(55, 45, 18))
            }
        }
    }
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
