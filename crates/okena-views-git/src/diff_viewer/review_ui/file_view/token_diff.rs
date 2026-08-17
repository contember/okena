//! Token diff of the two normalized signatures — spec §9. Pure; the details
//! block only paints the result.
//!
//! The shared prefix and suffix stay `Same`, so each line is rebuilt by taking
//! the segments that belong to its side, in order.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SegmentKind {
    Same,
    Removed,
    Added,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Segment {
    pub text: String,
    pub kind: SegmentKind,
}

impl Segment {
    /// Whether this segment belongs on the line whose change kind is `changed`.
    pub(super) fn on_side(&self, changed: SegmentKind) -> bool {
        self.kind == SegmentKind::Same || self.kind == changed
    }
}

/// Longest common prefix and suffix; whatever is left in the middle changed.
pub(super) fn token_diff(old: &str, new: &str) -> Vec<Segment> {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    let shortest = old_tokens.len().min(new_tokens.len());
    let prefix = (0..shortest)
        .take_while(|index| old_tokens[*index] == new_tokens[*index])
        .count();
    let suffix = (0..shortest - prefix)
        .take_while(|index| {
            old_tokens[old_tokens.len() - 1 - index] == new_tokens[new_tokens.len() - 1 - index]
        })
        .count();
    let old_end = old_tokens.len() - suffix;
    let new_end = new_tokens.len() - suffix;

    let mut segments = Vec::new();
    push(&mut segments, SegmentKind::Same, &old_tokens[..prefix]);
    push(
        &mut segments,
        SegmentKind::Removed,
        &old_tokens[prefix..old_end],
    );
    push(
        &mut segments,
        SegmentKind::Added,
        &new_tokens[prefix..new_end],
    );
    push(&mut segments, SegmentKind::Same, &old_tokens[old_end..]);
    segments
}

fn push(segments: &mut Vec<Segment>, kind: SegmentKind, tokens: &[&str]) {
    if tokens.is_empty() {
        return;
    }
    segments.push(Segment {
        text: tokens.concat(),
        kind,
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Punctuation,
}

fn class_of(character: char) -> Class {
    if character.is_alphanumeric() || character == '_' {
        Class::Word
    } else if character.is_whitespace() {
        Class::Space
    } else {
        Class::Punctuation
    }
}

/// Identifier and whitespace runs stay together; punctuation is one token each,
/// so `(`, `,` and `:` can anchor the diff.
fn tokenize(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut previous: Option<Class> = None;
    for (index, character) in text.char_indices() {
        let class = class_of(character);
        let continues = matches!(
            (previous, class),
            (Some(Class::Word), Class::Word) | (Some(Class::Space), Class::Space)
        );
        if !continues {
            if index > start {
                tokens.push(&text[start..index]);
            }
            start = index;
        }
        previous = Some(class);
    }
    if start < text.len() {
        tokens.push(&text[start..]);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::{Segment, SegmentKind, token_diff, tokenize};

    fn side(segments: &[Segment], changed: SegmentKind) -> String {
        segments
            .iter()
            .filter(|segment| segment.on_side(changed))
            .map(|segment| segment.text.as_str())
            .collect()
    }

    fn only(segments: &[Segment], kind: SegmentKind) -> Vec<&str> {
        segments
            .iter()
            .filter(|segment| segment.kind == kind)
            .map(|segment| segment.text.as_str())
            .collect()
    }

    #[test]
    fn tokens_split_identifiers_whitespace_and_single_punctuation() {
        assert_eq!(
            tokenize("fn run(&self) -> u32"),
            [
                "fn", " ", "run", "(", "&", "self", ")", " ", "-", ">", " ", "u32"
            ]
        );
        assert_eq!(tokenize(""), Vec::<&str>::new());
        assert_eq!(tokenize("   "), ["   "]);
    }

    #[test]
    fn an_added_parameter_is_the_only_highlighted_span() {
        let old = "pub fn run(&self, input: &str) -> Result<()>";
        let new = "pub fn run(&self, input: &str, retries: u32) -> Result<()>";
        let segments = token_diff(old, new);
        assert_eq!(only(&segments, SegmentKind::Added), [", retries: u32"]);
        assert!(only(&segments, SegmentKind::Removed).is_empty());
        assert_eq!(side(&segments, SegmentKind::Removed), old);
        assert_eq!(side(&segments, SegmentKind::Added), new);
    }

    #[test]
    fn the_new_parameter_of_a_typescript_signature_is_highlighted() {
        let old = "export async function compileProject(project: Project): Promise<Bundle>";
        let new =
            "export async function compileProject(project: Project, host: Host): Promise<Bundle>";
        let segments = token_diff(old, new);
        let added = only(&segments, SegmentKind::Added);
        assert_eq!(added.len(), 1);
        assert!(added[0].contains("host: Host"), "{added:?}");
        assert!(!added[0].contains("project"), "{added:?}");
        assert_eq!(side(&segments, SegmentKind::Added), new);
    }

    #[test]
    fn a_replaced_span_is_removed_on_one_line_and_added_on_the_other() {
        let old = "fn f(a: A)";
        let new = "fn f(b: B)";
        let segments = token_diff(old, new);
        assert_eq!(only(&segments, SegmentKind::Removed), ["a: A"]);
        assert_eq!(only(&segments, SegmentKind::Added), ["b: B"]);
        assert_eq!(side(&segments, SegmentKind::Removed), old);
        assert_eq!(side(&segments, SegmentKind::Added), new);
    }

    #[test]
    fn a_dropped_parameter_is_highlighted_on_the_old_line_only() {
        let old = "fn f(a, b)";
        let new = "fn f(a)";
        let segments = token_diff(old, new);
        assert_eq!(only(&segments, SegmentKind::Removed), [", b"]);
        assert!(only(&segments, SegmentKind::Added).is_empty());
        assert_eq!(side(&segments, SegmentKind::Removed), old);
        assert_eq!(side(&segments, SegmentKind::Added), new);
    }

    #[test]
    fn equal_signatures_produce_one_shared_segment() {
        let segments = token_diff("fn f()", "fn f()");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].kind, SegmentKind::Same);
        assert_eq!(segments[0].text, "fn f()");
    }

    #[test]
    fn every_pair_rebuilds_both_lines_exactly() {
        let pairs = [
            ("", ""),
            ("", "fn f()"),
            ("fn f()", ""),
            ("a", "b"),
            ("fn f(a: A) -> B", "fn g(a: A) -> B"),
            ("pub fn run()", "fn run()"),
        ];
        for (old, new) in pairs {
            let segments = token_diff(old, new);
            assert_eq!(side(&segments, SegmentKind::Removed), old, "{old} / {new}");
            assert_eq!(side(&segments, SegmentKind::Added), new, "{old} / {new}");
        }
    }
}
