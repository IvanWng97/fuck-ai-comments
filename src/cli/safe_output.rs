use std::borrow::Cow;

pub(crate) fn text(value: &str) -> Cow<'_, str> {
    if !value.chars().any(needs_escape) {
        return Cow::Borrowed(value);
    }

    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{2028}' => escaped.push_str("\\u{2028}"),
            '\u{2029}' => escaped.push_str("\\u{2029}"),
            character if is_bidi_control(character) => {
                escaped.extend(character.escape_unicode());
            }
            character if character.is_control() => escaped.extend(character.escape_unicode()),
            character => escaped.push(character),
        }
    }
    Cow::Owned(escaped)
}

pub(crate) fn finding_path(value: &str) -> Cow<'_, str> {
    let escaped = text(value);
    if escaped.starts_with("::") {
        Cow::Owned(format!("./{escaped}"))
    } else {
        escaped
    }
}

fn needs_escape(character: char) -> bool {
    character == '\\'
        || character.is_control()
        || is_bidi_control(character)
        || matches!(character, '\u{2028}' | '\u{2029}')
}

fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{61c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::{finding_path, text};

    #[test]
    fn text_escapes_log_control_characters() {
        assert_eq!(text("a\\b\n\u{1b}c"), "a\\\\b\\n\\u{1b}c");
    }

    #[test]
    fn text_escapes_every_unicode_bidi_control() {
        let controls = "\u{61c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}";

        assert_eq!(
            text(controls),
            "\\u{61c}\\u{200e}\\u{200f}\\u{202a}\\u{202b}\\u{202c}\\u{202d}\\u{202e}\\u{2066}\\u{2067}\\u{2068}\\u{2069}"
        );
    }

    #[test]
    fn finding_path_cannot_start_a_github_workflow_command() {
        assert_eq!(finding_path("::warning::pwn.rs"), "./::warning::pwn.rs");
    }
}
