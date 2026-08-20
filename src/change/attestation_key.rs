use std::fmt;

use icu_properties::CodePointSetData;
use icu_properties::props::DefaultIgnorableCodePoint;

#[derive(Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct AttestationKey(String);

impl AttestationKey {
    pub(super) fn from_comment(comment: &str) -> Option<Self> {
        let body = strip_comment_delimiters(comment.trim());
        let default_ignorable = CodePointSetData::new::<DefaultIgnorableCodePoint>();
        let mut key = String::with_capacity(body.len());

        let mut pending_space = false;
        for line in body.lines() {
            let mut characters = line
                .chars()
                .filter(|character| !default_ignorable.contains(*character))
                .peekable();
            while characters
                .next_if(|character| character.is_whitespace())
                .is_some()
            {}
            characters.next_if_eq(&'*');
            while characters
                .next_if(|character| character.is_whitespace())
                .is_some()
            {}

            for character in characters {
                if character.is_whitespace() {
                    pending_space = !key.is_empty();
                } else {
                    if pending_space {
                        key.push(' ');
                        pending_space = false;
                    }
                    key.push(character);
                }
            }
            pending_space = !key.is_empty();
        }

        let meaningful_end = key
            .trim_end_matches(['.', '!', '?', '。', '！', '？'])
            .trim_end()
            .len();
        key.truncate(meaningful_end);
        (!key.is_empty()).then_some(Self(key))
    }
}

impl fmt::Debug for AttestationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

fn strip_comment_delimiters(comment: &str) -> &str {
    [
        ("<!--", "-->"),
        ("/**", "*/"),
        ("/*!", "*/"),
        ("/*", "*/"),
        ("\"\"\"", "\"\"\""),
        ("'''", "'''"),
    ]
    .into_iter()
    .find_map(|(prefix, suffix)| {
        comment
            .strip_prefix(prefix)
            .and_then(|body| body.strip_suffix(suffix))
    })
    .or_else(|| {
        ["///", "//!", "//", "#"]
            .into_iter()
            .find_map(|prefix| comment.strip_prefix(prefix))
    })
    .unwrap_or(comment)
}
