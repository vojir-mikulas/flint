// SPDX-License-Identifier: GPL-3.0-or-later

//! A small hand-rolled SQL tokenizer for **M0 spike B**. It stands in for the
//! RED-domain tokenizer (SQL dialect logic stays in RED, per the MVP split): the
//! generic [`CodeEditor`](crate::editor::CodeEditor) takes a highlighter callback
//! and knows nothing about SQL. Coarse on purpose — the spike proves the
//! *render* path (tokens → colored runs); M4 decides whether to keep this or back
//! it with `tree-sitter-sql`.

use std::ops::Range;

use flint::TokenStyle;

const KEYWORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "drop",
    "alter",
    "add",
    "column",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "full",
    "cross",
    "on",
    "using",
    "group",
    "by",
    "order",
    "asc",
    "desc",
    "limit",
    "offset",
    "as",
    "and",
    "or",
    "not",
    "null",
    "is",
    "in",
    "like",
    "ilike",
    "between",
    "distinct",
    "case",
    "when",
    "then",
    "else",
    "end",
    "union",
    "intersect",
    "except",
    "all",
    "having",
    "primary",
    "key",
    "foreign",
    "references",
    "default",
    "unique",
    "check",
    "constraint",
    "index",
    "view",
    "with",
    "exists",
    "cast",
    "begin",
    "commit",
    "rollback",
    "transaction",
    "if",
    "returning",
    "true",
    "false",
];

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}
fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
fn is_operator(b: u8) -> bool {
    matches!(
        b,
        b'+' | b'-' | b'*' | b'/' | b'=' | b'<' | b'>' | b'!' | b'%' | b'|' | b'&' | b'^' | b'~'
    )
}

/// Tokenize `src` into `(byte range, style)` spans. Gaps between spans
/// (whitespace, punctuation) are left out and rendered in the default text color
/// by the editor.
pub fn tokenize(src: &str) -> Vec<(Range<usize>, TokenStyle)> {
    let b = src.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;

    while i < n {
        let c = b[i];

        // Line comment: -- to end of line.
        if c == b'-' && i + 1 < n && b[i + 1] == b'-' {
            let start = i;
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            out.push((start..i, TokenStyle::Comment));
            continue;
        }

        // Block comment: /* ... */
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
            out.push((start..i, TokenStyle::Comment));
            continue;
        }

        // String / quoted literal (' or ").
        if c == b'\'' || c == b'"' {
            let quote = c;
            let start = i;
            i += 1;
            while i < n && b[i] != quote {
                i += 1;
            }
            if i < n {
                i += 1; // closing quote
            }
            out.push((start..i, TokenStyle::String));
            continue;
        }

        // Number.
        if c.is_ascii_digit() {
            let start = i;
            while i < n && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            out.push((start..i, TokenStyle::Number));
            continue;
        }

        // Identifier / keyword / function.
        if is_ident_start(c) {
            let start = i;
            while i < n && is_ident_continue(b[i]) {
                i += 1;
            }
            let word = &src[start..i];
            let lower = word.to_ascii_lowercase();
            let style = if KEYWORDS.contains(&lower.as_str()) {
                TokenStyle::Keyword
            } else {
                // Function = identifier immediately followed by '(' (skip spaces).
                let mut j = i;
                while j < n && (b[j] == b' ' || b[j] == b'\t') {
                    j += 1;
                }
                if j < n && b[j] == b'(' {
                    TokenStyle::Function
                } else {
                    TokenStyle::Identifier
                }
            };
            out.push((start..i, style));
            continue;
        }

        // Operator run.
        if is_operator(c) {
            let start = i;
            while i < n && is_operator(b[i]) {
                i += 1;
            }
            out.push((start..i, TokenStyle::Operator));
            continue;
        }

        // Whitespace / punctuation: default color, no span.
        i += 1;
    }

    out
}
