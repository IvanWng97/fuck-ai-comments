use std::path::Path;

use crate::model::{AnalysisError, Finding, Selection};

const LEAF_COMMENT_MAX_LINES: usize = 3;

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    toml::from_str::<toml::Table>(source).map_err(|error| AnalysisError::Toml {
        path: path.display().to_string(),
        detail: error.to_string(),
    })?;

    let mut findings = Vec::new();
    let mut pending = Vec::new();
    let mut mode = None;
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let (code, has_comment) =
            line_parts(raw, &mut mode).map_err(|detail| AnalysisError::Toml {
                path: path.display().to_string(),
                detail: detail.to_owned(),
            })?;
        if has_comment && code.trim().is_empty() {
            pending.push(line_number);
            continue;
        }
        if code.trim().is_empty() {
            continue;
        }

        let owner = key(code);
        let mut owned = pending.clone();
        if owner.is_some() && has_comment {
            owned.push(line_number);
        }
        let owner_touched = selection.owners.contains(&line_number);
        let comment_touched = owned.iter().any(|line| selection.changed.contains(line));
        if let Some(owner) = owner {
            if !owned.is_empty() && owner_touched && !comment_touched {
                findings.push(Finding {
                    path: path.display().to_string(),
                    line: owned[0],
                    rule: "comment-policy/comment-owner-changed",
                    message: format!(
                        "TOML key `{owner}` changed while its comment did not; edit or delete the comment to attest that it remains true"
                    ),
                });
            }
            if owned.len() > LEAF_COMMENT_MAX_LINES && (owner_touched || comment_touched) {
                findings.push(Finding {
                    path: path.display().to_string(),
                    line: owned[0],
                    rule: "comment-policy/leaf-comment-budget",
                    message: format!(
                        "{} comment lines own TOML key `{owner}`; allowance is {LEAF_COMMENT_MAX_LINES}",
                        owned.len()
                    ),
                });
            }
        }
        pending.clear();
    }
    if mode.is_some() {
        return Err(AnalysisError::Toml {
            path: path.display().to_string(),
            detail: "unterminated multiline string".to_owned(),
        });
    }
    Ok(findings)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum StringMode {
    Basic,
    Literal,
    MultilineBasic,
    MultilineLiteral,
}

fn line_parts<'line>(
    line: &'line str,
    mode: &mut Option<StringMode>,
) -> Result<(&'line str, bool), &'static str> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match *mode {
            Some(StringMode::MultilineBasic) => {
                if bytes[index..].starts_with(b"\"\"\"") {
                    *mode = None;
                    index += 3;
                } else if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else {
                    index += 1;
                }
            }
            Some(StringMode::MultilineLiteral) => {
                if bytes[index..].starts_with(b"'''") {
                    *mode = None;
                    index += 3;
                } else {
                    index += 1;
                }
            }
            Some(StringMode::Basic) => {
                if bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else {
                    if bytes[index] == b'\"' {
                        *mode = None;
                    }
                    index += 1;
                }
            }
            Some(StringMode::Literal) => {
                if bytes[index] == b'\'' {
                    *mode = None;
                }
                index += 1;
            }
            None => {
                if bytes[index] == b'#' {
                    return Ok((&line[..index], true));
                }
                if bytes[index..].starts_with(b"\"\"\"") {
                    *mode = Some(StringMode::MultilineBasic);
                    index += 3;
                } else if bytes[index..].starts_with(b"'''") {
                    *mode = Some(StringMode::MultilineLiteral);
                    index += 3;
                } else if bytes[index] == b'\"' {
                    *mode = Some(StringMode::Basic);
                    index += 1;
                } else if bytes[index] == b'\'' {
                    *mode = Some(StringMode::Literal);
                    index += 1;
                } else {
                    index += 1;
                }
            }
        }
    }
    if matches!(mode, Some(StringMode::Basic | StringMode::Literal)) {
        return Err("unterminated single-line string");
    }
    Ok((line, false))
}

fn key(code: &str) -> Option<&str> {
    let stripped = code.trim();
    if stripped.is_empty() || stripped.starts_with('[') {
        return None;
    }

    let bytes = stripped.as_bytes();
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        match quote {
            Some(b'\"') => {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'\"' {
                    quote = None;
                }
            }
            Some(b'\'') => {
                if byte == b'\'' {
                    quote = None;
                }
            }
            Some(_) => return None,
            None if matches!(byte, b'\"' | b'\'') => quote = Some(byte),
            None if byte == b'=' => return Some(stripped[..index].trim()),
            None => {}
        }
    }
    None
}
