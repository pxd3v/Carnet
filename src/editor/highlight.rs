use std::{ops::Range, path::Path, sync::OnceLock};

use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Theme, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HighlightLanguage {
    Markdown,
    Html,
    PlainText,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HighlightStyle {
    pub foreground: [u8; 4],
    pub background: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub style: HighlightStyle,
}

pub(super) struct HighlightCache {
    language: HighlightLanguage,
    cached_text: Option<String>,
    spans: Vec<HighlightSpan>,
}

impl HighlightCache {
    pub(super) fn for_path(path: &Path) -> Self {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let language = match extension.as_str() {
            "md" | "markdown" | "mdown" | "mkd" => HighlightLanguage::Markdown,
            "html" | "htm" => HighlightLanguage::Html,
            _ => HighlightLanguage::PlainText,
        };
        Self {
            language,
            cached_text: None,
            spans: Vec::new(),
        }
    }

    pub(super) fn language(&self) -> HighlightLanguage {
        self.language
    }

    pub(super) fn spans(&mut self, text: &str) -> &[HighlightSpan] {
        if self.cached_text.as_deref() != Some(text) {
            self.spans = highlight(self.language, text);
            self.cached_text = Some(text.to_owned());
        }
        &self.spans
    }
}

fn highlight(language: HighlightLanguage, text: &str) -> Vec<HighlightSpan> {
    if language == HighlightLanguage::PlainText || text.is_empty() {
        return Vec::new();
    }
    let syntaxes = syntaxes();
    let syntax = match language {
        HighlightLanguage::Markdown => syntaxes.find_syntax_by_extension("md"),
        HighlightLanguage::Html => syntaxes.find_syntax_by_extension("html"),
        HighlightLanguage::PlainText => None,
    };
    let Some(syntax) = syntax else {
        return Vec::new();
    };
    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut spans = Vec::new();
    let mut start = 0;
    for line in LinesWithEndings::from(text) {
        let Ok(regions) = highlighter.highlight_line(line, syntaxes) else {
            return Vec::new();
        };
        for (style, segment) in regions {
            let length = segment.chars().count();
            if length == 0 {
                continue;
            }
            let foreground = style.foreground;
            let background = style.background;
            spans.push(HighlightSpan {
                range: start..start + length,
                style: HighlightStyle {
                    foreground: [foreground.r, foreground.g, foreground.b, foreground.a],
                    background: [background.r, background.g, background.b, background.a],
                    bold: style.font_style.contains(FontStyle::BOLD),
                    italic: style.font_style.contains(FontStyle::ITALIC),
                    underline: style.font_style.contains(FontStyle::UNDERLINE),
                },
            });
            start += length;
        }
    }
    spans
}

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        let themes = ThemeSet::load_defaults();
        themes
            .themes
            .get("base16-ocean.dark")
            .or_else(|| themes.themes.values().next())
            .cloned()
            .unwrap_or_default()
    })
}
