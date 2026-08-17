use std::collections::BTreeSet;
use std::path::Path;

use toml_edit::{Document, Item, Table};
use toml_parser::Source;
use toml_parser::lexer::{Token, TokenKind};

use crate::model::{AnalysisError, Finding, OwnerKind, Selection};
use crate::policy::{
    CodeToken, Comment, CommentKind, CommentSnapshot, LEAF_COMMENT_MAX_LINES, OwnerSnapshot,
    ParsedFile, Span, file_findings,
};

const TABLE_PATH_TOKEN_KIND: &str = "toml-table-path";

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    let document = parse_file(path, source)?;
    let comments: Vec<_> = document
        .comments
        .iter()
        .map(|comment| Comment {
            span: comment.span.clone(),
            kind: comment.kind,
            text: comment.text.clone(),
        })
        .collect();
    let mut comments_by_owner = vec![Vec::new(); document.owners.len()];
    for (comment_index, comment) in document.comments.iter().enumerate() {
        comments_by_owner[comment.owner].push(comment_index);
    }
    let mut findings = Vec::new();
    for (owner_index, owner) in document.owners.iter().enumerate().skip(1) {
        if owner.kind != OwnerKind::TomlKey {
            continue;
        }
        let owned = &comments_by_owner[owner_index];
        let selected = selection.selects_owner(
            OwnerKind::TomlKey,
            owner.span.start_byte,
            owner.span.end_byte,
        );
        if !selected {
            continue;
        }
        let lines: BTreeSet<_> = owned
            .iter()
            .map(|comment_index| &document.comments[*comment_index])
            .filter(|comment| comment.kind == CommentKind::Narrative)
            .flat_map(|comment| comment.span.lines())
            .collect();
        if lines.len() > LEAF_COMMENT_MAX_LINES {
            findings.push(Finding {
                path: path.display().to_string(),
                line: lines.first().copied().unwrap_or(owner.span.start_line),
                rule: "comment-policy/leaf-comment-budget",
                message: format!(
                    "{} comment lines own TOML key `{}`; allowance is {LEAF_COMMENT_MAX_LINES}",
                    lines.len(),
                    owner.name
                ),
            });
        }
    }
    let file_comments: Vec<_> = comments_by_owner[0]
        .iter()
        .map(|index| comments[*index].clone())
        .collect();
    findings.extend(file_findings(
        path,
        source,
        selection,
        &file_comments,
        &comments,
    ));
    Ok(findings)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    let document = Document::parse(source).map_err(|error| toml_error(path, error.to_string()))?;
    let parser_source = Source::new(source);
    let tokens: Vec<_> = parser_source.lex().collect();

    let line_starts = line_starts(source);
    let drafts = owner_drafts(path, &document)?;
    let mut owners = Vec::with_capacity(drafts.len() + 1);
    owners.push(OwnerSnapshot {
        kind: OwnerKind::File,
        name: "<file>".to_owned(),
        identity: vec!["file".to_owned()],
        span: Span {
            start_byte: 0,
            end_byte: source.len(),
            start_line: 1,
            end_line: source.lines().count().max(1),
        },
        parent: None,
        code: Vec::new(),
    });
    owners.extend(drafts.into_iter().map(|draft| {
        let identity = vec![draft.name.clone()];
        OwnerSnapshot {
            kind: OwnerKind::TomlKey,
            name: draft.name,
            identity,
            span: span_from_bytes(&line_starts, draft.start, draft.end),
            parent: Some(0),
            code: draft
                .table
                .into_iter()
                .map(|text| CodeToken::atom(TABLE_PATH_TOKEN_KIND, &text))
                .collect(),
        }
    }));
    validate_flat_owner_spans(path, &owners)?;

    let comment_tokens: Vec<_> = tokens
        .iter()
        .filter(|token| token.kind() == TokenKind::Comment)
        .copied()
        .collect();
    let comment_owners = assign_comment_owners(source, &line_starts, &owners, &comment_tokens);
    let comments = comment_tokens
        .iter()
        .zip(comment_owners)
        .map(|(token, owner)| {
            let parser_span = token.span();
            let text = source_slice(path, source, parser_span.start(), parser_span.end())?;
            Ok(CommentSnapshot {
                kind: CommentKind::Narrative,
                text: text.to_owned(),
                span: span_from_bytes(&line_starts, parser_span.start(), parser_span.end()),
                owner,
            })
        })
        .collect::<Result<Vec<_>, AnalysisError>>()?;

    assign_code_tokens(path, source, &tokens, &mut owners)?;
    Ok(ParsedFile { owners, comments })
}

fn toml_error(path: &Path, detail: impl Into<String>) -> AnalysisError {
    AnalysisError::Toml {
        path: path.display().to_string(),
        detail: detail.into(),
    }
}

#[derive(Debug)]
struct OwnerDraft {
    name: String,
    table: Option<String>,
    start: usize,
    end: usize,
}

fn owner_drafts(path: &Path, document: &Document<&str>) -> Result<Vec<OwnerDraft>, AnalysisError> {
    let mut owners = Vec::new();
    collect_table_owners(
        path,
        document.as_table(),
        &mut Vec::new(),
        &mut owners,
        true,
    )?;
    owners.sort_by_key(|owner| owner.start);
    Ok(owners)
}

fn collect_table_owners(
    path: &Path,
    table: &Table,
    table_path: &mut Vec<String>,
    owners: &mut Vec<OwnerDraft>,
    include_values: bool,
) -> Result<(), AnalysisError> {
    let table_name =
        (!table_path.is_empty()).then(|| qualified_name(table_path.iter().map(String::as_str)));
    if include_values {
        for (keys, value) in table.get_values() {
            let first = keys
                .first()
                .and_then(|key| key.span())
                .ok_or_else(|| toml_error(path, "TOML key has no source span"))?;
            let end = value
                .span()
                .map(|span| span.end)
                .ok_or_else(|| toml_error(path, "TOML value has no source span"))?;
            let key_parts = keys.iter().map(|key| key.get());
            owners.push(OwnerDraft {
                name: qualified_name(table_path.iter().map(String::as_str).chain(key_parts)),
                table: table_name.clone(),
                start: first.start,
                end,
            });
        }
    }

    for (key, item) in table.iter() {
        match item {
            Item::Table(child) => {
                table_path.push(key.to_owned());
                collect_table_owners(path, child, table_path, owners, !child.is_dotted())?;
                table_path.pop();
            }
            Item::ArrayOfTables(array) => {
                table_path.push(key.to_owned());
                for child in array.iter() {
                    collect_table_owners(path, child, table_path, owners, true)?;
                }
                table_path.pop();
            }
            Item::None | Item::Value(_) => {}
        }
    }
    Ok(())
}

fn qualified_name<'part>(parts: impl IntoIterator<Item = &'part str>) -> String {
    parts
        .into_iter()
        .map(|part| part.replace('\\', "\\\\").replace('.', "\\."))
        .collect::<Vec<_>>()
        .join(".")
}

fn assign_comment_owners(
    source: &str,
    line_starts: &[usize],
    owners: &[OwnerSnapshot],
    comments: &[Token],
) -> Vec<usize> {
    let mut assigned = vec![None; comments.len()];
    let mut owner_cursor = 1;
    for (comment_index, comment) in comments.iter().enumerate() {
        let comment_span = comment.span();
        while owner_cursor < owners.len()
            && owners[owner_cursor].span.end_byte <= comment_span.start()
        {
            owner_cursor += 1;
        }
        assigned[comment_index] = owners.get(owner_cursor).and_then(|owner| {
            (owner.span.start_byte <= comment_span.start()
                && comment_span.end() <= owner.span.end_byte)
                .then_some(owner_cursor)
        });
    }

    let mut standalone_by_line = vec![None; line_starts.len() + 1];
    for (index, comment) in comments.iter().enumerate() {
        let start = comment.span().start();
        let line = line_number(line_starts, start);
        let physical_start = line_starts[line - 1];
        if source.as_bytes()[physical_start..start]
            .iter()
            .all(u8::is_ascii_whitespace)
        {
            standalone_by_line[line] = Some(index);
        }
    }
    for (owner_index, owner) in owners.iter().enumerate().skip(1) {
        let mut cursor = owner.span.start_line.saturating_sub(1);
        while cursor > 0 {
            let Some(comment_index) = standalone_by_line.get(cursor).copied().flatten() else {
                break;
            };
            if assigned[comment_index].is_some() {
                break;
            }
            assigned[comment_index] = Some(owner_index);
            cursor = cursor.saturating_sub(1);
        }
    }

    let mut trailing_by_line = vec![None; line_starts.len() + 1];
    for (owner_index, owner) in owners.iter().enumerate().skip(1) {
        trailing_by_line[owner.span.end_line] = Some(owner_index);
    }
    for (comment_index, comment) in comments.iter().enumerate() {
        if assigned[comment_index].is_some() {
            continue;
        }
        let comment_span = comment.span();
        let comment_line = line_number(line_starts, comment_span.start());
        assigned[comment_index] = trailing_by_line[comment_line].filter(|owner_index| {
            let owner = &owners[*owner_index];
            owner.span.end_byte <= comment_span.start()
                && source.as_bytes()[owner.span.end_byte..comment_span.start()]
                    .iter()
                    .all(u8::is_ascii_whitespace)
        });
    }

    assigned
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect()
}

fn assign_code_tokens(
    path: &Path,
    source: &str,
    tokens: &[Token],
    owners: &mut [OwnerSnapshot],
) -> Result<(), AnalysisError> {
    let mut owner_cursor = 1;
    for token in tokens.iter().filter(|token| {
        !matches!(
            token.kind(),
            TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment | TokenKind::Eof
        )
    }) {
        let parser_span = token.span();
        while owner_cursor < owners.len()
            && owners[owner_cursor].span.end_byte <= parser_span.start()
        {
            owner_cursor += 1;
        }
        let owner = owners.get(owner_cursor).map_or(0, |owner| {
            if owner.span.start_byte <= parser_span.start()
                && parser_span.end() <= owner.span.end_byte
            {
                owner_cursor
            } else {
                0
            }
        });
        let text = source_slice(path, source, parser_span.start(), parser_span.end())?;
        owners[owner]
            .code
            .push(CodeToken::atom(token.kind().description(), text));
    }
    Ok(())
}

fn validate_flat_owner_spans(path: &Path, owners: &[OwnerSnapshot]) -> Result<(), AnalysisError> {
    for pair in owners[1..].windows(2) {
        if pair[0].span.end_byte > pair[1].span.start_byte {
            return Err(AnalysisError::Invariant(format!(
                "{} has overlapping TOML key owners at bytes {}..{} and {}..{}",
                path.display(),
                pair[0].span.start_byte,
                pair[0].span.end_byte,
                pair[1].span.start_byte,
                pair[1].span.end_byte,
            )));
        }
    }
    Ok(())
}

fn line_starts(source: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            source
                .bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index + 1),
        )
        .collect()
}

fn line_number(line_starts: &[usize], byte: usize) -> usize {
    line_starts.partition_point(|start| *start <= byte).max(1)
}

fn span_from_bytes(line_starts: &[usize], start: usize, end: usize) -> Span {
    Span {
        start_byte: start,
        end_byte: end,
        start_line: line_number(line_starts, start),
        end_line: line_number(line_starts, end.saturating_sub(1).max(start)),
    }
}

fn source_slice<'source>(
    path: &Path,
    source: &'source str,
    start: usize,
    end: usize,
) -> Result<&'source str, AnalysisError> {
    source
        .get(start..end)
        .ok_or_else(|| toml_error(path, "TOML parser returned an invalid UTF-8 source span"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OwnerKind;

    #[test]
    fn parse_file_assigns_multiline_array_to_its_key_owner() {
        let source = concat!(
            "# Required by the deployment contract.\n",
            "features = [\n",
            "  \"alpha\",\n",
            "  # The fallback must remain last.\n",
            "  \"fallback\",\n",
            "]\n",
            "unrelated = true\n",
        );

        let document = parse_file(Path::new("config.toml"), source).expect("valid TOML");
        let features_index = document
            .owners
            .iter()
            .position(|owner| owner.kind == OwnerKind::TomlKey && owner.name == "features")
            .expect("features owner");
        let features = &document.owners[features_index];
        let comments: Vec<_> = document
            .comments
            .iter()
            .map(|comment| (comment.text.as_str(), comment.owner))
            .collect();

        assert_eq!(
            (
                features.span.start_line,
                features.span.end_line,
                features
                    .code
                    .iter()
                    .map(|token| token.text.as_str())
                    .collect::<Vec<_>>(),
                comments,
            ),
            (
                2,
                6,
                vec![
                    "features",
                    "=",
                    "[",
                    "\"alpha\"",
                    ",",
                    "\"fallback\"",
                    ",",
                    "]"
                ],
                vec![
                    ("# Required by the deployment contract.", features_index),
                    ("# The fallback must remain last.", features_index),
                ],
            )
        );
    }

    #[test]
    fn parse_file_keeps_hashes_inside_multiline_strings_as_code() {
        let source = concat!(
            "message = \"\"\"\n",
            "hashes # remain data\n",
            "across lines\n",
            "\"\"\" # Explains the external wire text.\n",
        );

        let document = parse_file(Path::new("config.toml"), source).expect("valid TOML");
        let owner = &document.owners[1];

        assert_eq!(
            (
                owner.span.end_line,
                owner
                    .code
                    .iter()
                    .map(|token| token.text.as_str())
                    .collect::<Vec<_>>(),
                document
                    .comments
                    .iter()
                    .map(|comment| (comment.text.as_str(), comment.owner))
                    .collect::<Vec<_>>(),
            ),
            (
                4,
                vec![
                    "message",
                    "=",
                    "\"\"\"\nhashes # remain data\nacross lines\n\"\"\"",
                ],
                vec![("# Explains the external wire text.", 1)],
            )
        );
    }

    #[test]
    fn parse_file_assigns_multiline_inline_table_to_outer_key() {
        let source = concat!(
            "settings = {\n",
            "  enabled = true,\n",
            "  # Coupled to the service retry policy.\n",
            "  retries = 3,\n",
            "}\n",
        );

        let document = parse_file(Path::new("config.toml"), source).expect("valid TOML");
        let owner = &document.owners[1];

        assert_eq!(
            (
                owner.name.as_str(),
                owner.span.end_line,
                document.comments[0].owner,
                owner
                    .code
                    .iter()
                    .map(|token| token.text.as_str())
                    .collect::<Vec<_>>(),
            ),
            (
                "settings",
                5,
                1,
                vec![
                    "settings", "=", "{", "enabled", "=", "true", ",", "retries", "=", "3", ",",
                    "}",
                ],
            )
        );
    }

    #[test]
    fn parse_file_leaves_detached_comments_with_file_owner() {
        let source = concat!(
            "# Applies to the whole file.\n",
            "\n",
            "timeout = 200 # Kept below the proxy deadline.\n",
            "# Required by the retry protocol.\n",
            "retries = 3\n",
        );

        let document = parse_file(Path::new("config.toml"), source).expect("valid TOML");

        assert_eq!(
            document
                .comments
                .iter()
                .map(|comment| (comment.text.as_str(), comment.owner))
                .collect::<Vec<_>>(),
            vec![
                ("# Applies to the whole file.", 0),
                ("# Kept below the proxy deadline.", 1),
                ("# Required by the retry protocol.", 2),
            ]
        );
    }

    #[test]
    fn parse_file_qualifies_keys_with_their_table_path() {
        let source = concat!(
            "[package.metadata.\"release.config\"]\n",
            "# Must match the publisher retry policy.\n",
            "retries = 3\n",
        );

        let document = parse_file(Path::new("Cargo.toml"), source).expect("valid TOML");

        assert_eq!(
            (document.owners[1].name.as_str(), document.comments[0].owner,),
            ("package.metadata.release\\.config.retries", 1)
        );
    }

    #[test]
    fn parse_file_code_changes_when_an_inner_array_value_changes() {
        let before = "# Ordered by preference.\nformats = [\"json\", \"text\"]\n";
        let after = "# Ordered by preference.\nformats = [\"json\", \"yaml\"]\n";

        let before_document =
            parse_file(Path::new("config.toml"), before).expect("valid before TOML");
        let after_document = parse_file(Path::new("config.toml"), after).expect("valid after TOML");

        assert_ne!(
            before_document.owners[1].code,
            after_document.owners[1].code
        );
    }

    #[test]
    fn parse_file_code_changes_when_the_owning_table_changes() {
        let before = "[production]\n# Must match this environment.\ntimeout = 200\n";
        let after = "[staging]\n# Must match this environment.\ntimeout = 200\n";

        let before_document =
            parse_file(Path::new("config.toml"), before).expect("valid before TOML");
        let after_document = parse_file(Path::new("config.toml"), after).expect("valid after TOML");

        assert_ne!(
            before_document.owners[1].code,
            after_document.owners[1].code
        );
    }
}
