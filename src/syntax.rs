// SPDX-License-Identifier: GPL-3.0-or-later

//! A tiny, dependency-free language highlighter — the generic companion to the
//! [`Highlighter`](crate::Highlighter) seam.
//!
//! It lexes source into the same [`TokenStyle`] classes the editors already map
//! to theme colours, so no new colour tokens are needed. The output is *sparse*:
//! only keywords, strings, numbers, comments, function calls and operators get a
//! span; identifiers, whitespace and brackets fall through as gaps that render in
//! the default text colour (the [`Highlighter`](crate::Highlighter) contract).
//!
//! It is deliberately narrow — generic over the C/JS family with a small
//! per-language table for comment markers and keywords. Anything it can't classify
//! stays a gap, so it never mangles input. For exact, grammar-aware highlighting a
//! caller can still supply its own [`Highlighter`](crate::Highlighter); this is the
//! good-enough default for prose code blocks.

use std::ops::Range;

use crate::components::code_editor::TokenStyle;

/// Per-language lexing rules. Unknown languages fall back to the C/JS family.
struct Syntax {
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    keywords: &'static [&'static str],
}

const JS_KW: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "of",
    "private",
    "protected",
    "public",
    "readonly",
    "return",
    "set",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

const RUST_KW: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

const PY_KW: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while",
    "with", "yield",
];

const GO_KW: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
    "nil",
    "true",
    "false",
];

const C_KW: &[&str] = &[
    "auto",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "delete",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "import",
    "int",
    "interface",
    "long",
    "namespace",
    "new",
    "null",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "struct",
    "switch",
    "template",
    "this",
    "throw",
    "true",
    "try",
    "typedef",
    "typename",
    "union",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
];

const SH_KW: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "in", "return", "local", "export", "source", "echo", "exit",
];

/// Pick lexing rules for a fenced block's info string (`None` → C/JS default).
fn syntax_for(lang: Option<&str>) -> Syntax {
    let lang = lang.unwrap_or("").to_ascii_lowercase();
    let c_block = Some(("/*", "*/"));
    match lang.as_str() {
        "rust" | "rs" => Syntax {
            line: &["//"],
            block: c_block,
            keywords: RUST_KW,
        },
        "python" | "py" => Syntax {
            line: &["#"],
            block: None,
            keywords: PY_KW,
        },
        "go" | "golang" => Syntax {
            line: &["//"],
            block: c_block,
            keywords: GO_KW,
        },
        "c" | "cpp" | "c++" | "h" | "hpp" | "java" | "cs" | "csharp" | "swift" | "kotlin"
        | "kt" => Syntax {
            line: &["//"],
            block: c_block,
            keywords: C_KW,
        },
        "sh" | "bash" | "zsh" | "shell" | "fish" => Syntax {
            line: &["#"],
            block: None,
            keywords: SH_KW,
        },
        "ruby" | "rb" => Syntax {
            line: &["#"],
            block: None,
            keywords: JS_KW,
        },
        "yaml" | "yml" | "toml" | "ini" | "conf" => Syntax {
            line: &["#"],
            block: None,
            keywords: &[],
        },
        "sql" => Syntax {
            line: &["--"],
            block: c_block,
            keywords: C_KW,
        },
        "lua" => Syntax {
            line: &["--"],
            block: None,
            keywords: C_KW,
        },
        // js / jsx / ts / tsx / json / css / unknown → the broad C/JS family.
        _ => Syntax {
            line: &["//"],
            block: c_block,
            keywords: JS_KW,
        },
    }
}

/// `true` for operator bytes that earn an [`Operator`](TokenStyle::Operator) span.
/// Brackets, commas and semicolons are intentionally excluded — they read better
/// in the default text colour than tinted.
fn is_op(c: u8) -> bool {
    matches!(
        c,
        b'=' | b'+' | b'-' | b'*' | b'/' | b'%' | b'<' | b'>' | b'!' | b'&' | b'|' | b'^' | b'~'
    )
}

/// Highlight `text` as `lang`, returning ordered, non-overlapping `(range, style)`
/// spans for the coloured tokens only. Gaps render in the default text colour.
///
/// Multi-line constructs (block comments, template literals) are handled, so this
/// is safe to run over a whole code block at once.
pub fn highlight(text: &str, lang: Option<&str>) -> Vec<(Range<usize>, TokenStyle)> {
    let syn = syntax_for(lang);
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;

    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    let is_ident_start = |c: u8| c.is_ascii_alphabetic() || c == b'_' || c >= 0x80;

    while i < n {
        let c = b[i];

        // Whitespace — skip (gap).
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Line comment.
        if let Some(marker) = syn.line.iter().find(|m| text[i..].starts_with(**m)) {
            let s = i;
            i += marker.len();
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            out.push((s..i, TokenStyle::Comment));
            continue;
        }

        // Block comment (may span lines).
        if let Some((open, close)) = syn.block {
            if text[i..].starts_with(open) {
                let s = i;
                i += open.len();
                while i < n && !text[i..].starts_with(close) {
                    i += 1;
                }
                i = (i + close.len()).min(n);
                out.push((s..i, TokenStyle::Comment));
                continue;
            }
        }

        // String / char literal — handles backslash escapes; stops at EOL if
        // unterminated (except backtick template literals, which may span lines).
        if c == b'"' || c == b'\'' || c == b'`' {
            let quote = c;
            let s = i;
            i += 1;
            while i < n {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == quote || (quote != b'`' && b[i] == b'\n') {
                    break;
                }
                i += 1;
            }
            if i < n && b[i] == quote {
                i += 1;
            }
            out.push((s..i.min(n), TokenStyle::String));
            continue;
        }

        // Number.
        if c.is_ascii_digit() {
            let s = i;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            out.push((s..i, TokenStyle::Number));
            continue;
        }

        // Identifier → keyword / function call / capitalised-as-function / gap.
        if is_ident_start(c) {
            let s = i;
            while i < n && is_ident(b[i]) {
                i += 1;
            }
            let word = &text[s..i];
            let mut j = i;
            while j < n && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            if syn.keywords.contains(&word) {
                out.push((s..i, TokenStyle::Keyword));
            } else if (j < n && b[j] == b'(') || c.is_ascii_uppercase() {
                out.push((s..i, TokenStyle::Function));
            }
            // else: plain identifier — leave as a gap.
            continue;
        }

        // Operator run.
        if is_op(c) {
            let s = i;
            while i < n && is_op(b[i]) {
                i += 1;
            }
            out.push((s..i, TokenStyle::Operator));
            continue;
        }

        // Brackets, separators, anything else — gap.
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(substring, style)` for every span — easier to assert against than ranges.
    fn toks(text: &str, lang: Option<&str>) -> Vec<(String, TokenStyle)> {
        highlight(text, lang)
            .into_iter()
            .map(|(r, s)| (text[r].to_string(), s))
            .collect()
    }

    #[test]
    fn spans_are_ordered_and_non_overlapping() {
        let src = "let maxZoom = sqrt(w * h); // note";
        let mut end = 0;
        for (r, _) in highlight(src, Some("js")) {
            assert!(r.start >= end, "spans must not overlap");
            assert!(r.end <= src.len());
            end = r.end;
        }
    }

    #[test]
    fn highlights_the_jsx_example() {
        let t = toks("const finalZoom = min(maxH, maxZoom);", Some("jsx"));
        assert!(t.contains(&("const".into(), TokenStyle::Keyword)));
        assert!(t.contains(&("min".into(), TokenStyle::Function)));
        assert!(t.contains(&("=".into(), TokenStyle::Operator)));
        // Plain identifiers are gaps, not spans.
        assert!(!t.iter().any(|(s, _)| s == "finalZoom"));
    }

    #[test]
    fn strings_numbers_and_comments() {
        let t = toks(r#"x = "hi" + 42 // tail"#, Some("js"));
        assert!(t.contains(&("\"hi\"".into(), TokenStyle::String)));
        assert!(t.contains(&("42".into(), TokenStyle::Number)));
        assert!(t.contains(&("// tail".into(), TokenStyle::Comment)));
    }

    #[test]
    fn language_specific_keywords_and_comments() {
        // `fn` is a keyword in Rust but not JS; `#` is a comment in Python only.
        assert!(toks("fn main() {}", Some("rust")).contains(&("fn".into(), TokenStyle::Keyword)));
        assert!(!toks("fn main() {}", Some("js")).contains(&("fn".into(), TokenStyle::Keyword)));
        assert!(toks("x = 1 # c", Some("py")).contains(&("# c".into(), TokenStyle::Comment)));
    }

    #[test]
    fn block_comment_spans_lines() {
        let t = toks("a /* one\ntwo */ b", Some("c"));
        assert!(t.contains(&("/* one\ntwo */".into(), TokenStyle::Comment)));
    }
}
