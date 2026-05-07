use std::fmt::Write as _;
use std::io::Write;
use std::str::FromStr;

use anyhow::{Result, bail};
use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;

pub(crate) const DEFAULT_THEME_NAME: &str = "default";

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

impl TokenClass {
    const ALL: [Self; 10] = [
        Self::Address,
        Self::Bytes,
        Self::Label,
        Self::Mnemonic,
        Self::Register,
        Self::Immediate,
        Self::Memory,
        Self::Symbol,
        Self::Comment,
        Self::Plain,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TokenStyleDirective {
    foreground: Option<Color>,
    bold: bool,
}

impl TokenStyleDirective {
    const fn new(foreground: Option<Color>, bold: bool) -> Self {
        Self { foreground, bold }
    }

    const fn with_foreground(self, foreground: Color) -> Self {
        Self {
            foreground: Some(foreground),
            ..self
        }
    }

    pub(crate) fn style(self) -> Style {
        let style = self
            .foreground
            .map_or_else(Style::new, |color| Style::new().fg(color));
        if self.bold {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TokenStyleDirectives {
    address: TokenStyleDirective,
    bytes: TokenStyleDirective,
    label: TokenStyleDirective,
    mnemonic: TokenStyleDirective,
    register: TokenStyleDirective,
    immediate: TokenStyleDirective,
    memory: TokenStyleDirective,
    symbol: TokenStyleDirective,
    comment: TokenStyleDirective,
    plain: TokenStyleDirective,
}

impl TokenStyleDirectives {
    const fn style(self, class: TokenClass) -> TokenStyleDirective {
        match class {
            TokenClass::Address => self.address,
            TokenClass::Bytes => self.bytes,
            TokenClass::Label => self.label,
            TokenClass::Mnemonic => self.mnemonic,
            TokenClass::Register => self.register,
            TokenClass::Immediate => self.immediate,
            TokenClass::Memory => self.memory,
            TokenClass::Symbol => self.symbol,
            TokenClass::Comment => self.comment,
            TokenClass::Plain => self.plain,
        }
    }

    const fn set_foreground(&mut self, class: TokenClass, foreground: Color) {
        let directive = match class {
            TokenClass::Address => &mut self.address,
            TokenClass::Bytes => &mut self.bytes,
            TokenClass::Label => &mut self.label,
            TokenClass::Mnemonic => &mut self.mnemonic,
            TokenClass::Register => &mut self.register,
            TokenClass::Immediate => &mut self.immediate,
            TokenClass::Memory => &mut self.memory,
            TokenClass::Symbol => &mut self.symbol,
            TokenClass::Comment => &mut self.comment,
            TokenClass::Plain => &mut self.plain,
        };
        *directive = directive.with_foreground(foreground);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SyntaxTheme {
    name: &'static str,
    styles: TokenStyleDirectives,
}

impl SyntaxTheme {
    pub(crate) fn named(name: &str) -> Result<Self> {
        let normalized = normalize_theme_name(name);
        THEMES
            .iter()
            .find(|theme| normalize_theme_name(theme.name) == normalized)
            .copied()
            .map(Self::from_definition)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "unsupported syntax theme `{name}`; available themes: {}",
                    theme_names()
                )
            })
    }

    #[cfg(test)]
    pub(crate) const fn default_theme() -> Self {
        Self::from_definition(ThemeDefinition {
            name: DEFAULT_THEME_NAME,
            styles: DEFAULT_STYLES,
        })
    }

    const fn from_definition(definition: ThemeDefinition) -> Self {
        Self {
            name: definition.name,
            styles: definition.styles,
        }
    }

    pub(crate) const fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) fn style(&self, class: TokenClass) -> Style {
        self.styles.style(class).style()
    }

    pub(crate) fn apply_color_overrides(
        &mut self,
        overrides: &SyntaxColorOverrides,
    ) {
        for (class, color) in overrides.colors() {
            self.styles.set_foreground(class, color);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ThemeDefinition {
    name: &'static str,
    styles: TokenStyleDirectives,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TokenColor(Color);

impl TokenColor {
    pub(crate) const fn color(self) -> Color {
        self.0
    }
}

impl<'de> Deserialize<'de> for TokenColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(serde::de::Error::custom)
    }
}

impl FromStr for TokenColor {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Ok(Self(parse_color(value, "syntax color")?))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SyntaxColorOverrides {
    address: Option<TokenColor>,
    bytes: Option<TokenColor>,
    label: Option<TokenColor>,
    mnemonic: Option<TokenColor>,
    register: Option<TokenColor>,
    immediate: Option<TokenColor>,
    memory: Option<TokenColor>,
    symbol: Option<TokenColor>,
    comment: Option<TokenColor>,
    plain: Option<TokenColor>,
}

impl SyntaxColorOverrides {
    pub(crate) fn color(&self, class: TokenClass) -> Option<Color> {
        self.token_color(class).map(TokenColor::color)
    }

    fn colors(&self) -> impl Iterator<Item = (TokenClass, Color)> + '_ {
        TokenClass::ALL
            .into_iter()
            .filter_map(|class| self.color(class).map(|color| (class, color)))
    }

    const fn token_color(&self, class: TokenClass) -> Option<TokenColor> {
        match class {
            TokenClass::Address => self.address,
            TokenClass::Bytes => self.bytes,
            TokenClass::Label => self.label,
            TokenClass::Mnemonic => self.mnemonic,
            TokenClass::Register => self.register,
            TokenClass::Immediate => self.immediate,
            TokenClass::Memory => self.memory,
            TokenClass::Symbol => self.symbol,
            TokenClass::Comment => self.comment,
            TokenClass::Plain => self.plain,
        }
    }
}

pub(crate) fn parse_color(value: &str, field_name: &str) -> Result<Color> {
    let normalized = value.trim().to_ascii_lowercase();
    let color = match normalized.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "dark-gray" | "dark-grey" | "darkgray" | "darkgrey" => Color::DarkGray,
        "light-red" | "lightred" => Color::LightRed,
        "light-green" | "lightgreen" => Color::LightGreen,
        "light-yellow" | "lightyellow" => Color::LightYellow,
        "light-blue" | "lightblue" => Color::LightBlue,
        "light-magenta" | "lightmagenta" => Color::LightMagenta,
        "light-cyan" | "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return parse_hex_color(&normalized, field_name),
    };

    Ok(color)
}

pub(crate) fn write_theme_samples(mut writer: impl Write) -> Result<()> {
    for theme in THEMES {
        let theme = SyntaxTheme::from_definition(*theme);
        writeln!(writer, "Theme: {}", theme.name())?;
        writeln!(writer, "{}", themed_sample(&theme))?;
    }
    Ok(())
}

fn themed_sample(theme: &SyntaxTheme) -> String {
    let mut sample = String::new();
    push_styled(&mut sample, theme, TokenClass::Label, "<demo>:");
    sample.push('\n');
    push_styled(&mut sample, theme, TokenClass::Plain, "  ");
    push_styled(&mut sample, theme, TokenClass::Address, "1130:");
    sample.push(' ');
    push_styled(&mut sample, theme, TokenClass::Bytes, "48 8b 45 f8");
    sample.push_str("    ");
    push_styled(&mut sample, theme, TokenClass::Mnemonic, "mov");
    sample.push(' ');
    push_styled(&mut sample, theme, TokenClass::Register, "rax");
    sample.push_str(", ");
    push_styled(
        &mut sample,
        theme,
        TokenClass::Memory,
        "qword ptr [rbp - 0x8]",
    );
    sample.push(' ');
    push_styled(&mut sample, theme, TokenClass::Comment, "# load local");
    sample.push('\n');
    push_styled(&mut sample, theme, TokenClass::Plain, "  ");
    push_styled(&mut sample, theme, TokenClass::Address, "1138:");
    sample.push(' ');
    push_styled(&mut sample, theme, TokenClass::Bytes, "e8 13 00 00 00");
    sample.push(' ');
    push_styled(&mut sample, theme, TokenClass::Mnemonic, "call");
    sample.push(' ');
    push_styled(&mut sample, theme, TokenClass::Symbol, "sym:worker");
    sample.push(' ');
    push_styled(&mut sample, theme, TokenClass::Immediate, "0x10");
    sample
}

fn push_styled(
    sample: &mut String,
    theme: &SyntaxTheme,
    class: TokenClass,
    text: &str,
) {
    let directive = theme.styles.style(class);
    let mut codes = Vec::new();
    if directive.bold {
        codes.push("1".to_owned());
    }
    if let Some(color) = directive.foreground {
        codes.push(ansi_foreground_code(color));
    }
    if codes.is_empty() {
        sample.push_str(text);
        return;
    }

    let _ = write!(sample, "\u{1b}[{}m{text}\u{1b}[0m", codes.join(";"));
}

fn ansi_foreground_code(color: Color) -> String {
    match color {
        Color::Black => "30".to_owned(),
        Color::Red => "31".to_owned(),
        Color::Green => "32".to_owned(),
        Color::Yellow => "33".to_owned(),
        Color::Blue => "34".to_owned(),
        Color::Magenta => "35".to_owned(),
        Color::Cyan => "36".to_owned(),
        Color::Gray => "37".to_owned(),
        Color::DarkGray => "90".to_owned(),
        Color::LightRed => "91".to_owned(),
        Color::LightGreen => "92".to_owned(),
        Color::LightYellow => "93".to_owned(),
        Color::LightBlue => "94".to_owned(),
        Color::LightMagenta => "95".to_owned(),
        Color::LightCyan => "96".to_owned(),
        Color::White => "97".to_owned(),
        Color::Indexed(index) => format!("38;5;{index}"),
        Color::Rgb(red, green, blue) => format!("38;2;{red};{green};{blue}"),
        Color::Reset => "39".to_owned(),
    }
}

fn parse_hex_color(value: &str, field_name: &str) -> Result<Color> {
    let Some(hex) = value.strip_prefix('#') else {
        bail!("unsupported {field_name} `{value}`");
    };
    if hex.len() != 6 {
        bail!("unsupported {field_name} `{value}`; expected #rrggbb");
    }

    let red = parse_hex_component(&hex[0..2], field_name, value)?;
    let green = parse_hex_component(&hex[2..4], field_name, value)?;
    let blue = parse_hex_component(&hex[4..6], field_name, value)?;
    Ok(Color::Rgb(red, green, blue))
}

fn parse_hex_component(
    component: &str,
    field_name: &str,
    value: &str,
) -> Result<u8> {
    u8::from_str_radix(component, 16)
        .map_err(|_| anyhow::anyhow!("unsupported {field_name} `{value}`"))
}

fn normalize_theme_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn theme_names() -> String {
    THEMES
        .iter()
        .map(|theme| theme.name)
        .collect::<Vec<_>>()
        .join(", ")
}

const fn directive(foreground: Color, bold: bool) -> TokenStyleDirective {
    TokenStyleDirective::new(Some(foreground), bold)
}

const fn plain_directive() -> TokenStyleDirective {
    TokenStyleDirective::new(None, false)
}

const DEFAULT_STYLES: TokenStyleDirectives = TokenStyleDirectives {
    address: directive(Color::DarkGray, false),
    bytes: directive(Color::Gray, false),
    label: directive(Color::Cyan, true),
    mnemonic: directive(Color::Yellow, true),
    register: directive(Color::LightBlue, false),
    immediate: directive(Color::LightMagenta, false),
    memory: directive(Color::LightCyan, false),
    symbol: directive(Color::LightGreen, false),
    comment: directive(Color::DarkGray, false),
    plain: plain_directive(),
};

const ANSI_STYLES: TokenStyleDirectives = TokenStyleDirectives {
    address: directive(Color::DarkGray, false),
    bytes: directive(Color::Gray, false),
    label: directive(Color::Cyan, true),
    mnemonic: directive(Color::Yellow, true),
    register: directive(Color::Blue, false),
    immediate: directive(Color::Magenta, false),
    memory: directive(Color::Cyan, false),
    symbol: directive(Color::Green, false),
    comment: directive(Color::DarkGray, false),
    plain: plain_directive(),
};

const MONOKAI_STYLES: TokenStyleDirectives = TokenStyleDirectives {
    address: directive(Color::Rgb(117, 113, 94), false),
    bytes: directive(Color::Rgb(174, 129, 255), false),
    label: directive(Color::Rgb(102, 217, 239), true),
    mnemonic: directive(Color::Rgb(249, 38, 114), true),
    register: directive(Color::Rgb(166, 226, 46), false),
    immediate: directive(Color::Rgb(174, 129, 255), false),
    memory: directive(Color::Rgb(230, 219, 116), false),
    symbol: directive(Color::Rgb(166, 226, 46), false),
    comment: directive(Color::Rgb(117, 113, 94), false),
    plain: plain_directive(),
};

const SOLARIZED_DARK_STYLES: TokenStyleDirectives = TokenStyleDirectives {
    address: directive(Color::Rgb(88, 110, 117), false),
    bytes: directive(Color::Rgb(147, 161, 161), false),
    label: directive(Color::Rgb(42, 161, 152), true),
    mnemonic: directive(Color::Rgb(181, 137, 0), true),
    register: directive(Color::Rgb(38, 139, 210), false),
    immediate: directive(Color::Rgb(211, 54, 130), false),
    memory: directive(Color::Rgb(108, 113, 196), false),
    symbol: directive(Color::Rgb(133, 153, 0), false),
    comment: directive(Color::Rgb(88, 110, 117), false),
    plain: plain_directive(),
};

const GRUVBOX_DARK_STYLES: TokenStyleDirectives = TokenStyleDirectives {
    address: directive(Color::Rgb(146, 131, 116), false),
    bytes: directive(Color::Rgb(211, 134, 155), false),
    label: directive(Color::Rgb(142, 192, 124), true),
    mnemonic: directive(Color::Rgb(250, 189, 47), true),
    register: directive(Color::Rgb(131, 165, 152), false),
    immediate: directive(Color::Rgb(211, 134, 155), false),
    memory: directive(Color::Rgb(254, 128, 25), false),
    symbol: directive(Color::Rgb(184, 187, 38), false),
    comment: directive(Color::Rgb(146, 131, 116), false),
    plain: plain_directive(),
};

const NORD_STYLES: TokenStyleDirectives = TokenStyleDirectives {
    address: directive(Color::Rgb(97, 110, 136), false),
    bytes: directive(Color::Rgb(180, 142, 173), false),
    label: directive(Color::Rgb(136, 192, 208), true),
    mnemonic: directive(Color::Rgb(235, 203, 139), true),
    register: directive(Color::Rgb(129, 161, 193), false),
    immediate: directive(Color::Rgb(180, 142, 173), false),
    memory: directive(Color::Rgb(143, 188, 187), false),
    symbol: directive(Color::Rgb(163, 190, 140), false),
    comment: directive(Color::Rgb(97, 110, 136), false),
    plain: plain_directive(),
};

const THEMES: &[ThemeDefinition] = &[
    ThemeDefinition {
        name: DEFAULT_THEME_NAME,
        styles: DEFAULT_STYLES,
    },
    ThemeDefinition {
        name: "ansi",
        styles: ANSI_STYLES,
    },
    ThemeDefinition {
        name: "monokai",
        styles: MONOKAI_STYLES,
    },
    ThemeDefinition {
        name: "solarized-dark",
        styles: SOLARIZED_DARK_STYLES,
    },
    ThemeDefinition {
        name: "gruvbox-dark",
        styles: GRUVBOX_DARK_STYLES,
    },
    ThemeDefinition {
        name: "nord",
        styles: NORD_STYLES,
    },
];
