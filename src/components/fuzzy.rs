//! Shared fuzzy-matching primitives used by the search-driven components
//! ([`Palette`](super::palette::Palette), [`Switcher`](super::switcher::Switcher)).
//! A case-insensitive subsequence match with a small scoring heuristic, plus a
//! label renderer that emphasises the matched glyphs.

use gpui::{div, prelude::*, FontWeight, Hsla, SharedString};

/// Render a label with its fuzzy-matched characters emphasised. Consecutive
/// matched/unmatched runs become sibling spans in a flex row, so the matched
/// glyphs paint in `hit` (bold) over the `base` colour.
pub(crate) fn highlighted_label(
    label: &SharedString,
    positions: &[usize],
    base: Hsla,
    hit: Hsla,
) -> impl IntoElement {
    let mut spans: Vec<(String, bool)> = Vec::new();
    let mut current = String::new();
    let mut current_hit = false;
    for (byte, ch) in label.char_indices() {
        let is_hit = positions.contains(&byte);
        if is_hit != current_hit && !current.is_empty() {
            spans.push((std::mem::take(&mut current), current_hit));
        }
        current_hit = is_hit;
        current.push(ch);
    }
    if !current.is_empty() {
        spans.push((current, current_hit));
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .children(spans.into_iter().map(move |(segment, is_hit)| {
            div()
                .when(is_hit, |d| d.font_weight(FontWeight::SEMIBOLD))
                .text_color(if is_hit { hit } else { base })
                .child(segment)
        }))
}

/// Case-insensitive subsequence fuzzy match of `query` against `text`. Returns
/// `None` unless every (non-whitespace) query char appears in order. On a match,
/// returns a score (higher = better) and the byte offsets in `text` that matched,
/// for highlighting. An empty query matches everything with score 0 and no marks,
/// so the list shows in its natural order.
pub(crate) fn fuzzy_match(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let needles: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if needles.is_empty() {
        return Some((0, Vec::new()));
    }

    let haystack: Vec<(usize, char)> = text.char_indices().collect();
    let mut qi = 0;
    let mut positions = Vec::with_capacity(needles.len());
    let mut score: i32 = 0;
    let mut prev_matched_at: Option<usize> = None;

    for (ci, (byte, ch)) in haystack.iter().enumerate() {
        if qi >= needles.len() {
            break;
        }
        let lowered = ch.to_lowercase().next().unwrap_or(*ch);
        if lowered != needles[qi] {
            continue;
        }
        positions.push(*byte);
        score += 1;
        // Adjacent to the previous match — runs of consecutive hits read best.
        if prev_matched_at == Some(ci.wrapping_sub(1)) {
            score += 5;
        }
        // At a word boundary (string start, after a separator, or a camelCase hump).
        let at_boundary = ci == 0 || {
            let prev = haystack[ci - 1].1;
            !prev.is_alphanumeric() || (prev.is_lowercase() && ch.is_uppercase())
        };
        if at_boundary {
            score += 8;
        }
        // Mild bias toward earlier matches.
        score -= ci as i32 / 4;
        prev_matched_at = Some(ci);
        qi += 1;
    }

    (qi == needles.len()).then_some((score, positions))
}

#[cfg(test)]
mod tests {
    use super::fuzzy_match;

    fn score(query: &str, text: &str) -> Option<i32> {
        fuzzy_match(query, text).map(|(s, _)| s)
    }

    #[test]
    fn empty_query_matches_everything_with_no_marks() {
        let (score, marks) = fuzzy_match("", "query: run").unwrap();
        assert_eq!(score, 0);
        assert!(marks.is_empty());
    }

    #[test]
    fn requires_all_chars_in_order() {
        assert!(fuzzy_match("run", "query: run").is_some());
        assert!(fuzzy_match("nur", "query: run").is_none()); // out of order
        assert!(fuzzy_match("runs", "query: run").is_none()); // extra char
    }

    #[test]
    fn match_is_case_insensitive() {
        assert!(fuzzy_match("RUN", "query: run").is_some());
        assert!(fuzzy_match("QR", "Query: Run").is_some());
    }

    #[test]
    fn marks_point_at_matched_bytes() {
        let (_, marks) = fuzzy_match("qr", "query: run").unwrap();
        // 'q' at byte 0, the first 'r' at byte 3 ("que[r]y").
        assert_eq!(marks, vec![0, 3]);
    }

    #[test]
    fn whitespace_in_query_is_ignored() {
        assert_eq!(score("q run", "query: run"), score("qrun", "query: run"));
    }

    #[test]
    fn prefix_beats_scattered_match() {
        // "run" as a word start should outrank the scattered r…u…n in "regular unit".
        let prefix = score("run", "run query").unwrap();
        let scattered = score("run", "regular unit number").unwrap();
        assert!(
            prefix > scattered,
            "prefix {prefix} should beat scattered {scattered}"
        );
    }

    #[test]
    fn consecutive_run_beats_scattered_run() {
        // Same chars, same text length, no word-boundary help on either side:
        // the consecutive substring should win on the adjacency bonus alone.
        let consecutive = score("abc", "abcxx").unwrap();
        let scattered = score("abc", "axbxc").unwrap();
        assert!(
            consecutive > scattered,
            "consecutive {consecutive} should beat scattered {scattered}"
        );
    }
}
