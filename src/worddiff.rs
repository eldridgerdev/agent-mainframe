//! Token-level diffing *within* a pair of changed lines.
//!
//! The patch renderer already tells the reviewer that a line changed; this
//! narrows it to *which tokens* changed, so a one-character edit inside a long
//! line doesn't read the same as a full rewrite. Pure and self-contained: it
//! takes the two line texts and returns byte ranges to emphasise on each side.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

/// Lines longer than this (in tokens) skip word diffing. The matrix below is
/// O(n·m), and a minified or generated line is exactly where the result would
/// be noise anyway.
const MAX_TOKENS: usize = 400;

/// Cached pairs kept per thread before the whole cache is dropped, so the memo
/// can't creep as the reviewer walks a large changeset. Only the selected file's
/// hunks are rendered, so this is sized to hold every changed pair of a very
/// large single file — a cap that a frame could exceed would clear and refill on
/// every frame, i.e. never hit.
const MAX_CACHED_PAIRS: usize = 16_384;

/// How much of a pair must be shared before intra-line highlighting is worth
/// showing. Below this the two lines are effectively unrelated, and marking
/// nearly every token would be louder than marking nothing.
const MIN_SIMILARITY: f32 = 0.35;

/// Byte ranges that differ between a paired removed/added line, relative to
/// each line's content (the `+`/`-` prefix already stripped).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WordDiff {
    pub old: Vec<Range<usize>>,
    pub new: Vec<Range<usize>>,
}

#[derive(Debug, Clone)]
struct Token<'a> {
    text: &'a str,
    range: Range<usize>,
}

/// Split a line into comparable tokens: identifier-ish runs, whitespace runs,
/// and single punctuation characters. Splitting punctuation individually keeps
/// edits like `foo(a, b)` -> `foo(a, c)` tight instead of swallowing the whole
/// argument list into one token.
fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let class = TokenClass::of(c);
        let mut end = start + c.len_utf8();
        if !matches!(class, TokenClass::Other) {
            while let Some(&(next_idx, next_c)) = chars.peek() {
                if TokenClass::of(next_c) != class {
                    break;
                }
                end = next_idx + next_c.len_utf8();
                chars.next();
            }
        }
        tokens.push(Token {
            text: &text[start..end],
            range: start..end,
        });
    }
    tokens
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenClass {
    Word,
    Space,
    Other,
}

impl TokenClass {
    fn of(c: char) -> Self {
        if c.is_alphanumeric() || c == '_' {
            TokenClass::Word
        } else if c.is_whitespace() {
            TokenClass::Space
        } else {
            TokenClass::Other
        }
    }
}

/// One memoised pair. The line texts are kept so a hash collision is caught
/// rather than silently emphasising the wrong tokens.
struct CachedPair {
    old: String,
    new: String,
    diff: Option<Rc<WordDiff>>,
}

thread_local! {
    static PAIR_CACHE: RefCell<HashMap<u64, CachedPair>> = RefCell::new(HashMap::new());
}

/// `word_diff`, memoised on the pair of line texts.
///
/// The renderers rebuild every visible row from scratch on each frame (twice,
/// when the cursor sync re-measures the patch), and the LCS matrix is O(n·m) in
/// tokens — up to `MAX_TOKENS`² cells per pair. Hashing the two lines is linear
/// in their length, so the memo turns a per-frame quadratic cost into a lookup.
///
/// Keying on the text rather than on a line index means it stays correct across
/// anything that rewrites the hunks — a base-ref change, `-w`, context
/// expansion: a rewritten line simply misses the cache.
pub fn word_diff_cached(old: &str, new: &str) -> Option<Rc<WordDiff>> {
    let key = pair_key(old, new);
    PAIR_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(entry) = cache.get(&key)
            && entry.old == old
            && entry.new == new
        {
            return entry.diff.clone();
        }
        let diff = word_diff(old, new).map(Rc::new);
        if cache.len() >= MAX_CACHED_PAIRS {
            cache.clear();
        }
        cache.insert(
            key,
            CachedPair {
                old: old.to_string(),
                new: new.to_string(),
                diff: diff.clone(),
            },
        );
        diff
    })
}

fn pair_key(old: &str, new: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    old.hash(&mut hasher);
    new.hash(&mut hasher);
    hasher.finish()
}

/// The byte ranges that changed between a removed line and its paired added
/// line.
///
/// Returns `None` when intra-line highlighting shouldn't be shown at all:
/// either side empty, either side too long to diff cheaply, the lines
/// identical, or the two too dissimilar to be a meaningful pair (a wholesale
/// rewrite, where the row's add/remove colour already says everything).
pub fn word_diff(old: &str, new: &str) -> Option<WordDiff> {
    if old.is_empty() || new.is_empty() || old == new {
        return None;
    }
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    if old_tokens.len() > MAX_TOKENS || new_tokens.len() > MAX_TOKENS {
        return None;
    }

    let matched = longest_common_subsequence(&old_tokens, &new_tokens);

    // Similarity is measured in bytes rather than tokens so a single shared
    // keyword can't make two long, otherwise-unrelated lines look like a pair.
    let shared: usize = matched.iter().map(|&(o, _)| old_tokens[o].text.len()).sum();
    let similarity = (2.0 * shared as f32) / (old.len() + new.len()) as f32;
    if similarity < MIN_SIMILARITY {
        return None;
    }

    let old_matched: Vec<usize> = matched.iter().map(|&(o, _)| o).collect();
    let new_matched: Vec<usize> = matched.iter().map(|&(_, n)| n).collect();
    let diff = WordDiff {
        old: unmatched_ranges(&old_tokens, &old_matched),
        new: unmatched_ranges(&new_tokens, &new_matched),
    };
    // Both sides matching everything means the lines differ only in ways the
    // tokenizer erased; nothing useful to point at.
    if diff.old.is_empty() && diff.new.is_empty() {
        return None;
    }
    Some(diff)
}

/// Indices of tokens that are *not* part of the common subsequence, collapsed
/// into contiguous byte ranges so the renderer emphasises runs rather than
/// individual tokens.
fn unmatched_ranges(tokens: &[Token<'_>], matched: &[usize]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut matched_iter = matched.iter().peekable();
    for (idx, token) in tokens.iter().enumerate() {
        if matched_iter.peek() == Some(&&idx) {
            matched_iter.next();
            continue;
        }
        match out.last_mut() {
            // Adjacent unmatched tokens merge into one highlighted run.
            Some(last) if last.end == token.range.start => last.end = token.range.end,
            _ => out.push(token.range.clone()),
        }
    }
    // Trailing/leading whitespace-only runs are visual noise — a changed
    // indent is better shown by the row colour than by a highlighted blank.
    out.retain(|range| {
        tokens
            .iter()
            .any(|t| t.range.start >= range.start && t.range.end <= range.end && !is_blank(t.text))
    });
    out
}

fn is_blank(text: &str) -> bool {
    text.chars().all(char::is_whitespace)
}

/// Standard LCS over token text, returning the matched `(old_idx, new_idx)`
/// pairs in order. Bounded by `MAX_TOKENS` on both sides by the caller.
fn longest_common_subsequence(old: &[Token<'_>], new: &[Token<'_>]) -> Vec<(usize, usize)> {
    let (n, m) = (old.len(), new.len());
    // Row-major (n+1) x (m+1) table of common-subsequence lengths.
    let mut table = vec![0u16; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if old[i].text == new[j].text {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }

    let mut pairs = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if old[i].text == new[j].text {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_diff_marks_only_the_changed_token() {
        let old = "let value = compute(a, b);";
        let new = "let value = compute(a, c);";
        let diff = word_diff(old, new).unwrap();

        assert_eq!(diff.old.len(), 1);
        assert_eq!(&old[diff.old[0].clone()], "b");
        assert_eq!(diff.new.len(), 1);
        assert_eq!(&new[diff.new[0].clone()], "c");
    }

    #[test]
    fn word_diff_handles_an_insertion_with_nothing_removed() {
        let old = "fn run(a) {}";
        let new = "fn run(a, b) {}";
        let diff = word_diff(old, new).unwrap();

        assert!(diff.old.is_empty(), "nothing was removed");
        // The inserted `, b` collapses into one contiguous run.
        assert_eq!(diff.new.len(), 1);
        assert_eq!(&new[diff.new[0].clone()], ", b");
    }

    #[test]
    fn word_diff_leaves_an_unchanged_separator_out_of_the_highlight() {
        // The `.` really didn't change, so it stays unmarked and the two
        // renamed identifiers highlight separately. Precision beats a
        // smoother-looking single run here.
        let old = "let x = foo.bar();";
        let new = "let x = baz.qux();";
        let diff = word_diff(old, new).unwrap();

        assert_eq!(diff.old.len(), 2);
        assert_eq!(&old[diff.old[0].clone()], "foo");
        assert_eq!(&old[diff.old[1].clone()], "bar");
        assert_eq!(&new[diff.new[0].clone()], "baz");
        assert_eq!(&new[diff.new[1].clone()], "qux");
    }

    #[test]
    fn word_diff_finds_several_separate_edits() {
        let old = "call(first, middle, last)";
        let new = "call(FIRST, middle, LAST)";
        let diff = word_diff(old, new).unwrap();

        assert_eq!(diff.old.len(), 2);
        assert_eq!(&old[diff.old[0].clone()], "first");
        assert_eq!(&old[diff.old[1].clone()], "last");
    }

    #[test]
    fn word_diff_declines_unrelated_lines() {
        // Marking nearly every token would be louder than marking nothing; the
        // row's add/remove colour already carries the message.
        assert_eq!(
            word_diff("use std::path::PathBuf;", "let mut total = 0usize;"),
            None
        );
    }

    #[test]
    fn word_diff_declines_empty_identical_and_oversized_lines() {
        assert_eq!(word_diff("", "abc"), None);
        assert_eq!(word_diff("abc", ""), None);
        assert_eq!(word_diff("same", "same"), None);

        let huge = "a ".repeat(MAX_TOKENS + 10);
        assert_eq!(word_diff(&huge, &format!("{huge}b")), None);
    }

    #[test]
    fn word_diff_ignores_a_whitespace_only_run() {
        // A changed indent alone shouldn't light up a blank region.
        let diff = word_diff("    value = 1;", "        value = 1;");
        assert!(
            diff.as_ref().is_none_or(|d| d
                .new
                .iter()
                .all(|r| !"        value = 1;"[r.clone()].trim().is_empty())),
            "whitespace-only run was highlighted: {diff:?}"
        );
    }

    #[test]
    fn word_diff_cached_repeats_the_uncached_answer() {
        let old = "let value = compute(a, b);";
        let new = "let value = compute(a, c);";

        let first = word_diff_cached(old, new).unwrap();
        let second = word_diff_cached(old, new).unwrap();

        assert_eq!(*first, word_diff(old, new).unwrap());
        // Second call was a hit, not a recomputation.
        assert!(Rc::ptr_eq(&first, &second));
    }

    #[test]
    fn word_diff_cached_keys_on_both_lines() {
        let old = "call(first, middle, last)";
        let first = word_diff_cached(old, "call(FIRST, middle, last)").unwrap();
        let second = word_diff_cached(old, "call(first, middle, LAST)").unwrap();

        // Same left side, different right side: distinct entries, and the ranges
        // point at the token that actually changed in each pair.
        assert_ne!(first.old, second.old);
        assert_eq!(&old[first.old[0].clone()], "first");
        assert_eq!(&old[second.old[0].clone()], "last");
    }

    #[test]
    fn word_diff_cached_remembers_a_declined_pair() {
        // A `None` is cached too, so an unrelated pair isn't re-diffed each frame.
        assert!(word_diff_cached("same", "same").is_none());
        assert!(word_diff_cached("same", "same").is_none());
    }

    #[test]
    fn word_diff_ranges_are_valid_utf8_boundaries() {
        let old = "let s = \"héllo wörld\";";
        let new = "let s = \"héllo there\";";
        let diff = word_diff(old, new).unwrap();

        // Slicing must not panic — ranges always land on char boundaries.
        for range in &diff.old {
            assert!(old.get(range.clone()).is_some());
        }
        for range in &diff.new {
            assert!(new.get(range.clone()).is_some());
        }
    }

    #[test]
    fn word_diff_ranges_are_ascending_and_non_overlapping() {
        let old = "a bb ccc dddd eeeee";
        let new = "a XX ccc YYYY eeeee";
        let diff = word_diff(old, new).unwrap();

        for side in [&diff.old, &diff.new] {
            for pair in side.windows(2) {
                assert!(pair[0].end <= pair[1].start, "ranges overlap: {side:?}");
            }
        }
    }
}
