use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;

use toml_edit::{Document, Item, Table, TomlError};
use toml_parser::Source;
use toml_parser::lexer::{Token, TokenKind};

use crate::config::PolicyConfig;
use crate::identity::IdentityArena;
use crate::model::{AnalysisError, Finding, OwnerKind, Selection};
use crate::policy::{
    CodeToken, Comment, CommentClassification, CommentSnapshot, LEAF_COMMENT_MAX_LINES,
    OwnerSnapshot, ParsedFile, Span, configured_category_cap_findings, file_findings,
    owner_comment_cap_finding_with_policy,
};
use crate::rules;

const TABLE_PATH_TOKEN_KIND: &str = "toml-table-path";

#[cfg(test)]
thread_local! {
    static COMMENT_OWNER_INDEX_CAPACITY: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static COMMENT_OWNER_INDEX_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TOML_SEMANTIC_STORAGE_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TOML_RETAINED_TOKEN_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_comment_owner_index(capacity: usize, entries: usize) {
    COMMENT_OWNER_INDEX_CAPACITY.with(|total| total.set(capacity));
    COMMENT_OWNER_INDEX_ENTRIES.with(|total| total.set(entries));
}

#[cfg(not(test))]
fn record_comment_owner_index(_capacity: usize, _entries: usize) {}

#[cfg(test)]
fn reset_comment_owner_index_counts() {
    COMMENT_OWNER_INDEX_CAPACITY.with(|total| total.set(0));
    COMMENT_OWNER_INDEX_ENTRIES.with(|total| total.set(0));
}

#[cfg(test)]
fn comment_owner_index_counts() -> (usize, usize) {
    (
        COMMENT_OWNER_INDEX_CAPACITY.with(std::cell::Cell::get),
        COMMENT_OWNER_INDEX_ENTRIES.with(std::cell::Cell::get),
    )
}

#[cfg(test)]
fn record_toml_semantic_storage(bytes: usize, retained_tokens: usize) {
    TOML_SEMANTIC_STORAGE_BYTES.with(|total| total.set(bytes));
    TOML_RETAINED_TOKEN_COUNT.with(|total| total.set(retained_tokens));
}

#[cfg(not(test))]
fn record_toml_semantic_storage(_bytes: usize, _retained_tokens: usize) {}

#[cfg(test)]
fn toml_semantic_storage() -> (usize, usize) {
    (
        TOML_SEMANTIC_STORAGE_BYTES.with(std::cell::Cell::get),
        TOML_RETAINED_TOKEN_COUNT.with(std::cell::Cell::get),
    )
}

struct TomlSemanticSource<'source> {
    original: &'source str,
    compact: String,
    segments: Vec<CompactedSegment>,
    comments: Vec<LexedComment>,
    code: Vec<Token>,
}

struct CompactedSegment {
    compact_start: usize,
    compact_end: usize,
    original_start: usize,
}

struct LexedComment {
    token: Token,
    start_line: usize,
    starts_line: bool,
}

impl<'source> TomlSemanticSource<'source> {
    fn new(path: &Path, original: &'source str) -> Result<Self, AnalysisError> {
        let mut semantic = Self {
            original,
            compact: String::new(),
            segments: Vec::new(),
            comments: Vec::new(),
            code: Vec::new(),
        };
        let mut line_start = 0;
        let mut current_line = 1;
        let mut line_has_semantic_content = false;

        let parser_source = Source::new(original);
        for token in parser_source.lex() {
            let span = token.span();
            let raw = parser_source.get(token).ok_or_else(|| {
                toml_error(path, "TOML parser returned an invalid UTF-8 source span")
            })?;
            let token_text = raw.as_str();
            match token.kind() {
                TokenKind::Comment => {
                    semantic.comments.push(LexedComment {
                        token,
                        start_line: current_line,
                        starts_line: !line_has_semantic_content,
                    });
                    let mut validation_error = None;
                    raw.decode_comment(&mut validation_error);
                    if validation_error.is_some() {
                        line_has_semantic_content = true;
                    }
                }
                TokenKind::Whitespace => {}
                TokenKind::Newline => {
                    let mut validation_error = None;
                    raw.decode_newline(&mut validation_error);
                    if validation_error.is_some() {
                        line_has_semantic_content = true;
                    }
                    if line_has_semantic_content {
                        semantic.retain(path, line_start, span.end())?;
                    }
                    line_start = span.end();
                    line_has_semantic_content = false;
                }
                TokenKind::Eof => {
                    if line_has_semantic_content && line_start < original.len() {
                        semantic.retain(path, line_start, original.len())?;
                    }
                }
                _ => {
                    semantic.code.push(token);
                    line_has_semantic_content = true;
                }
            }
            current_line += token_text.bytes().filter(|byte| *byte == b'\n').count();
        }

        record_toml_semantic_storage(semantic.storage_bytes(), semantic.retained_token_count());
        Ok(semantic)
    }

    fn retain(&mut self, path: &Path, start: usize, end: usize) -> Result<(), AnalysisError> {
        let text = source_slice(path, self.original, start, end)?;
        let compact_start = self.compact.len();
        self.compact.push_str(text);
        let compact_end = self.compact.len();
        if let Some(previous) = self.segments.last_mut()
            && previous.compact_end == compact_start
            && previous.original_start + (previous.compact_end - previous.compact_start) == start
        {
            previous.compact_end = compact_end;
        } else {
            self.segments.push(CompactedSegment {
                compact_start,
                compact_end,
                original_start: start,
            });
        }
        Ok(())
    }

    fn original_range(
        &self,
        path: &Path,
        compact_start: usize,
        compact_end: usize,
    ) -> Result<(usize, usize), AnalysisError> {
        if compact_start >= compact_end {
            return Err(AnalysisError::Invariant(format!(
                "{} has an empty TOML owner span in the compact semantic source",
                path.display()
            )));
        }
        let original_start = self.original_offset(path, compact_start)?;
        let original_end = self
            .original_offset(path, compact_end - 1)?
            .checked_add(1)
            .ok_or_else(|| {
                AnalysisError::Invariant("TOML owner span endpoint overflowed".to_owned())
            })?;
        Ok((original_start, original_end))
    }

    fn original_error_range(
        &self,
        path: &Path,
        compact: Range<usize>,
    ) -> Result<Range<usize>, AnalysisError> {
        if compact.start > compact.end || compact.end > self.compact.len() {
            return Err(AnalysisError::Invariant(format!(
                "{} has a TOML parse error outside the compact semantic source",
                path.display()
            )));
        }
        if compact.is_empty() {
            let position = if compact.start == self.compact.len() {
                self.original.len()
            } else {
                self.original_offset(path, compact.start)?
            };
            return Ok(position..position);
        }
        let (start, end) = self.original_range(path, compact.start, compact.end)?;
        Ok(start..end)
    }

    fn error_detail(&self, path: &Path, error: &TomlError) -> Result<String, AnalysisError> {
        let Some(compact_span) = error.span() else {
            return Ok(error.to_string());
        };
        let original_span = self.original_error_range(path, compact_span)?;
        render_toml_error(path, self.original, error.message(), original_span)
    }

    fn original_offset(&self, path: &Path, compact: usize) -> Result<usize, AnalysisError> {
        let segment_index = self
            .segments
            .partition_point(|segment| segment.compact_end <= compact);
        let segment = self
            .segments
            .get(segment_index)
            .filter(|segment| segment.compact_start <= compact && compact < segment.compact_end);
        let Some(segment) = segment else {
            return Err(AnalysisError::Invariant(format!(
                "{} has a TOML owner outside the compact semantic source",
                path.display()
            )));
        };
        Ok(segment.original_start + (compact - segment.compact_start))
    }

    fn storage_bytes(&self) -> usize {
        self.compact.capacity()
            + self.segments.capacity() * std::mem::size_of::<CompactedSegment>()
            + self.comments.capacity() * std::mem::size_of::<LexedComment>()
            + self.code.capacity() * std::mem::size_of::<Token>()
    }

    fn retained_token_count(&self) -> usize {
        self.comments.len() + self.code.len()
    }
}

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
    policy: &PolicyConfig,
) -> Result<Vec<Finding>, AnalysisError> {
    let document = parse_file(path, source)?;
    let comments: Vec<_> = document
        .comments
        .iter()
        .map(|comment| Comment {
            span: comment.span.clone(),
            classification: comment.classification,
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
        let owned_comments: Vec<_> = owned
            .iter()
            .map(|comment_index| comments[*comment_index].clone())
            .collect();
        let owner_label = format!("TOML key `{}`", owner.name);
        findings.extend(owner_comment_cap_finding_with_policy(
            path,
            OwnerKind::TomlKey,
            &owner_label,
            &owned_comments,
            policy,
        ));
        findings.extend(configured_category_cap_findings(
            path,
            &owner_label,
            &owned_comments,
            policy,
        ));
        let lines: BTreeSet<_> = owned
            .iter()
            .map(|comment_index| &document.comments[*comment_index])
            .filter(|comment| comment.classification.uses_relative_budget(policy))
            .flat_map(|comment| comment.span.lines())
            .collect();
        if lines.len() > LEAF_COMMENT_MAX_LINES {
            findings.push(Finding {
                path: path.display().to_string(),
                line: lines.first().copied().unwrap_or(owner.span.start_line),
                rule: rules::LEAF_COMMENT_BUDGET,
                message: format!(
                    "TOML key `{}` owns {} comment lines; allowance is {LEAF_COMMENT_MAX_LINES}",
                    owner.name,
                    lines.len()
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
        policy,
    ));
    Ok(findings)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    let semantic = TomlSemanticSource::new(path, source)?;
    let document = match Document::parse(semantic.compact.as_str()) {
        Ok(document) => document,
        Err(error) => return Err(toml_error(path, semantic.error_detail(path, &error)?)),
    };
    let drafts = owner_drafts(path, &document)?;
    let mut line_cursor = OriginalLineCursor::new(source);
    let mut identities = IdentityArena::default();
    let mut owners = Vec::with_capacity(drafts.len() + 1);
    let file_identity = identities.push_path(["file"])?;
    owners.push(OwnerSnapshot {
        kind: OwnerKind::File,
        name: "<file>".to_owned(),
        identity: file_identity,
        span: Span {
            start_byte: 0,
            end_byte: source.len(),
            start_line: 1,
            end_line: source.lines().count().max(1),
        },
        parent: None,
        code: Vec::new(),
    });
    for draft in drafts {
        let (start, end) = semantic.original_range(path, draft.start, draft.end)?;
        let identity = identities.push_path([draft.name.as_str()])?;
        owners.push(OwnerSnapshot {
            kind: OwnerKind::TomlKey,
            name: draft.name,
            identity,
            span: line_cursor.span(path, start, end)?,
            parent: Some(0),
            code: draft
                .table
                .into_iter()
                .map(|text| CodeToken::atom(TABLE_PATH_TOKEN_KIND, &text))
                .collect(),
        });
    }
    validate_flat_owner_spans(path, &owners)?;

    let comment_owners = assign_comment_owners(source, &owners, &semantic.comments);
    let comments = semantic
        .comments
        .iter()
        .zip(comment_owners)
        .map(|(comment, owner)| {
            let parser_span = comment.token.span();
            let text = source_slice(path, source, parser_span.start(), parser_span.end())?;
            Ok(CommentSnapshot {
                classification: CommentClassification::narrative(),
                text: text.to_owned(),
                span: Span {
                    start_byte: parser_span.start(),
                    end_byte: parser_span.end(),
                    start_line: comment.start_line,
                    end_line: comment.start_line,
                },
                owner,
            })
        })
        .collect::<Result<Vec<_>, AnalysisError>>()?;

    assign_code_tokens(path, source, &semantic.code, &mut owners)?;
    Ok(ParsedFile {
        identities,
        owners,
        comments,
    })
}

fn toml_error(path: &Path, detail: impl Into<String>) -> AnalysisError {
    AnalysisError::Toml {
        path: path.display().to_string(),
        detail: detail.into(),
    }
}

fn render_toml_error(
    path: &Path,
    source: &str,
    message: &str,
    span: Range<usize>,
) -> Result<String, AnalysisError> {
    if span.start > span.end
        || span.end > source.len()
        || !source.is_char_boundary(span.start)
        || !source.is_char_boundary(span.end)
    {
        return Err(AnalysisError::Invariant(format!(
            "{} has an invalid mapped TOML error span",
            path.display()
        )));
    }

    let line_start = source[..span.start]
        .rfind('\n')
        .map_or(0, |newline| newline + 1);
    let line_end = source[span.start..]
        .find('\n')
        .map_or(source.len(), |newline| span.start + newline);
    let line_number = source.as_bytes()[..line_start]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1;
    let column = source[line_start..span.start].chars().count();
    let content = source_slice(path, source, line_start, line_end)?;
    let highlight_end = span.end.min(line_end);
    let highlight = source_slice(path, source, span.start, highlight_end)?
        .chars()
        .count()
        .max(1);
    let gutter_padding = " ".repeat(line_number.to_string().len() + 1);

    Ok(format!(
        "TOML parse error at line {line_number}, column {}\n{gutter_padding}|\n{line_number} | {content}\n{gutter_padding}|{}{}\n{message}\n",
        column + 1,
        " ".repeat(column + 1),
        "^".repeat(highlight),
    ))
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
    owners: &[OwnerSnapshot],
    comments: &[LexedComment],
) -> Vec<usize> {
    let mut assigned = vec![None; comments.len()];
    let mut owner_cursor = 1;
    for (comment_index, comment) in comments.iter().enumerate() {
        let comment_span = comment.token.span();
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

    let mut standalone_by_line = Vec::with_capacity(comments.len());
    for (index, comment) in comments.iter().enumerate() {
        if comment.starts_line {
            insert_sparse_line(&mut standalone_by_line, comment.start_line, index);
        }
    }
    for (owner_index, owner) in owners.iter().enumerate().skip(1) {
        let mut cursor = owner.span.start_line.saturating_sub(1);
        while cursor > 0 {
            let Some(comment_index) = sparse_line_value(&standalone_by_line, cursor) else {
                break;
            };
            if assigned[comment_index].is_some() {
                break;
            }
            assigned[comment_index] = Some(owner_index);
            cursor = cursor.saturating_sub(1);
        }
    }

    let mut trailing_by_line = Vec::with_capacity(owners.len().saturating_sub(1));
    for (owner_index, owner) in owners.iter().enumerate().skip(1) {
        insert_sparse_line(&mut trailing_by_line, owner.span.end_line, owner_index);
    }
    for (comment_index, comment) in comments.iter().enumerate() {
        if assigned[comment_index].is_some() {
            continue;
        }
        let comment_span = comment.token.span();
        let comment_line = comment.start_line;
        assigned[comment_index] =
            sparse_line_value(&trailing_by_line, comment_line).filter(|owner_index| {
                let owner = &owners[*owner_index];
                owner.span.end_byte <= comment_span.start()
                    && source.as_bytes()[owner.span.end_byte..comment_span.start()]
                        .iter()
                        .all(u8::is_ascii_whitespace)
            });
    }

    record_comment_owner_index(
        standalone_by_line.capacity() + trailing_by_line.capacity(),
        standalone_by_line.len() + trailing_by_line.len(),
    );

    assigned
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect()
}

struct SparseLineValue {
    line: usize,
    value: usize,
}

fn insert_sparse_line(index: &mut Vec<SparseLineValue>, line: usize, value: usize) {
    if let Some(previous) = index.last_mut()
        && previous.line == line
    {
        previous.value = value;
        return;
    }
    debug_assert!(index.last().is_none_or(|previous| previous.line < line));
    index.push(SparseLineValue { line, value });
}

fn sparse_line_value(index: &[SparseLineValue], line: usize) -> Option<usize> {
    index
        .binary_search_by_key(&line, |entry| entry.line)
        .ok()
        .map(|position| index[position].value)
}

fn assign_code_tokens(
    path: &Path,
    source: &str,
    tokens: &[Token],
    owners: &mut [OwnerSnapshot],
) -> Result<(), AnalysisError> {
    let mut owner_cursor = 1;
    for token in tokens {
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

struct OriginalLineCursor<'source> {
    source: &'source [u8],
    offset: usize,
    line: usize,
}

impl<'source> OriginalLineCursor<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source: source.as_bytes(),
            offset: 0,
            line: 1,
        }
    }

    fn span(&mut self, path: &Path, start: usize, end: usize) -> Result<Span, AnalysisError> {
        if start >= end {
            return Err(AnalysisError::Invariant(format!(
                "{} has an empty TOML owner span in the original source",
                path.display()
            )));
        }
        let start_line = self.line_at(path, start)?;
        let end_line = self.line_at(path, end - 1)?;
        Ok(Span {
            start_byte: start,
            end_byte: end,
            start_line,
            end_line,
        })
    }

    fn line_at(&mut self, path: &Path, byte: usize) -> Result<usize, AnalysisError> {
        let bytes = self.source.get(self.offset..byte).ok_or_else(|| {
            AnalysisError::Invariant(format!(
                "{} has non-monotonic TOML owner spans",
                path.display()
            ))
        })?;
        self.line += bytes
            .iter()
            .filter(|candidate| **candidate == b'\n')
            .count();
        self.offset = byte;
        Ok(self.line)
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
    use crate::policy::{line_start_storage, reset_line_start_storage};
    use crate::{SourceFile, analyze_all};

    #[test]
    fn sixteen_mib_newline_dense_toml_retains_only_semantic_storage() {
        const SOURCE_BYTES: usize = 16 * 1_024 * 1_024;
        const KEY: &str = "timeout = 200\n";
        let mut source = "\n".repeat(SOURCE_BYTES - KEY.len());
        source.push_str(KEY);
        assert_eq!(
            source.len(),
            SOURCE_BYTES,
            "fixture must stay exactly 16 MiB"
        );
        reset_comment_owner_index_counts();
        reset_line_start_storage();

        let findings = analyze_all(SourceFile {
            path: Path::new("config.toml"),
            text: &source,
        })
        .expect("newline-dense TOML source must parse");

        assert!(findings.is_empty(), "comment-free source stays clean");
        assert!(
            line_start_storage() <= 2,
            "comment-free TOML analysis must not build a dense physical-line index"
        );
        let (capacity, entries) = comment_owner_index_counts();
        assert_eq!(entries, 1, "only the TOML key's sparse trailing row exists");
        assert!(
            capacity <= 2,
            "TOML ownership indexes must be bounded by comments and owners; capacity={capacity}"
        );
        let (semantic_bytes, retained_tokens) = toml_semantic_storage();
        assert!(
            semantic_bytes <= 4 * 1_024,
            "TOML semantic parsing must not retain trivia-sized storage; bytes={semantic_bytes}"
        );
        assert!(
            retained_tokens <= 16,
            "TOML semantic parsing must retain only meaningful tokens; tokens={retained_tokens}"
        );
    }

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
