//! A small glob matcher for the patterns `deadwood.toml` accepts.
//!
//! Two kinds of pattern need matching — file paths (`src/generated/**`) and
//! item paths (`mylib::prelude::*`) — and they differ only in the separator
//! between segments, so one matcher serves both.
//!
//! The syntax is the familiar subset and nothing more:
//!
//! - `*` matches any run of characters *within* one segment, never across a
//!   separator;
//! - `**` as a whole segment matches any number of segments, including none;
//! - `?` matches exactly one character;
//! - everything else is literal.
//!
//! There is deliberately no character class, no brace expansion, and no
//! negation. A pattern language is a thing users have to learn, and every
//! feature here has to earn that cost; these four are what the ignore and
//! public-API settings actually need. A crate like `globset` would bring the
//! rest along with it, and Deadwood adds dependencies only for confirmed
//! problems.

/// A compiled glob pattern, bound to the separator it splits segments on.
#[derive(Debug, Clone)]
pub struct Glob {
    segments: Vec<Segment>,
    separator: &'static str,
}

#[derive(Debug, Clone)]
enum Segment {
    /// A `**` segment: matches any number of segments, including none.
    AnyDepth,
    /// A literal-and-wildcard segment, as characters so that `?` counts code
    /// points rather than bytes.
    Pattern(Vec<char>),
}

impl Glob {
    /// A pattern over `/`-separated paths.
    pub fn path(pattern: &str) -> Self {
        Self::new(pattern, "/")
    }

    /// A pattern over `::`-separated item paths.
    pub fn item(pattern: &str) -> Self {
        Self::new(pattern, "::")
    }

    fn new(pattern: &str, separator: &'static str) -> Self {
        Glob {
            segments: pattern
                .split(separator)
                .map(|segment| match segment {
                    "**" => Segment::AnyDepth,
                    other => Segment::Pattern(other.chars().collect()),
                })
                .collect(),
            separator,
        }
    }

    /// Whether `text`, split on this glob's separator, matches the pattern.
    pub fn matches(&self, text: &str) -> bool {
        let segments: Vec<&str> = text.split(self.separator).collect();
        matches_segments(&self.segments, &segments)
    }
}

fn matches_segments(pattern: &[Segment], text: &[&str]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        // `**` may swallow any prefix of the remaining segments, so every
        // split point has to be tried before declaring a mismatch.
        Some((Segment::AnyDepth, rest)) => {
            (0..=text.len()).any(|taken| matches_segments(rest, &text[taken..]))
        }
        Some((Segment::Pattern(chars), rest)) => match text.split_first() {
            Some((first, remaining)) => {
                let first: Vec<char> = first.chars().collect();
                matches_chars(chars, &first) && matches_segments(rest, remaining)
            }
            None => false,
        },
    }
}

/// Match one segment, where `*` covers any run of characters and `?` exactly
/// one.
///
/// Backtracking is exponential in the number of `*`s in the worst case, which
/// is irrelevant for the segment-sized patterns people write in a config file.
fn matches_chars(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|taken| matches_chars(rest, &text[taken..])),
        Some(('?', rest)) => !text.is_empty() && matches_chars(rest, &text[1..]),
        Some((expected, rest)) => match text.split_first() {
            Some((actual, remaining)) => actual == expected && matches_chars(rest, remaining),
            None => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_pattern_matches_only_itself() {
        let glob = Glob::path("src/lib.rs");
        assert!(glob.matches("src/lib.rs"));
        assert!(!glob.matches("src/lib.rss"));
        assert!(!glob.matches("other/src/lib.rs"));
    }

    /// The distinction the whole matcher exists for: `*` is a segment-local
    /// wildcard, `**` is the one that crosses separators.
    #[test]
    fn a_star_does_not_cross_a_separator_but_a_double_star_does() {
        let shallow = Glob::path("src/*.rs");
        assert!(shallow.matches("src/lib.rs"));
        assert!(!shallow.matches("src/inner/lib.rs"));

        let deep = Glob::path("src/**/*.rs");
        assert!(deep.matches("src/inner/lib.rs"));
        assert!(deep.matches("src/a/b/c/lib.rs"));
    }

    /// `**` matching *zero* segments is what makes `generated/**` cover the
    /// directory itself and not just its contents.
    #[test]
    fn a_double_star_matches_no_segments_at_all() {
        let glob = Glob::path("generated/**");
        assert!(glob.matches("generated"));
        assert!(glob.matches("generated/a.rs"));
        assert!(glob.matches("generated/deep/a.rs"));
        assert!(!glob.matches("other/a.rs"));
    }

    #[test]
    fn a_question_mark_matches_exactly_one_character() {
        let glob = Glob::path("src/v?.rs");
        assert!(glob.matches("src/v1.rs"));
        assert!(!glob.matches("src/v.rs"));
        assert!(!glob.matches("src/v12.rs"));
    }

    /// Item paths are separated by `::`, and a single `:` inside a segment
    /// must not be mistaken for one.
    #[test]
    fn item_paths_split_on_the_double_colon() {
        let glob = Glob::item("mylib::prelude::*");
        assert!(glob.matches("mylib::prelude::Thing"));
        assert!(!glob.matches("mylib::prelude::inner::Thing"));
        assert!(Glob::item("mylib::**").matches("mylib::a::b::Thing"));
    }

    /// A `*` is not a free pass to a different segment count: `mylib::*`
    /// covers the crate's own items, not everything under it.
    #[test]
    fn a_wildcard_segment_still_matches_exactly_one_segment() {
        let glob = Glob::item("mylib::*");
        assert!(glob.matches("mylib::Thing"));
        assert!(!glob.matches("mylib::inner::Thing"));
    }

    #[test]
    fn several_wildcards_in_one_segment_backtrack_correctly() {
        let glob = Glob::path("*_test_*.rs");
        assert!(glob.matches("a_test_b.rs"));
        assert!(glob.matches("_test_.rs"));
        assert!(!glob.matches("a_tests_b.rs"));
    }
}
