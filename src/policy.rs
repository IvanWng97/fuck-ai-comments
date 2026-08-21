use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use tree_sitter::Node;

use crate::config::{PolicyConfig, StaticPolicy};
use crate::identity::{IdentityArena, IdentityId};
use crate::model::{AnalysisError, Finding, OwnerKind, Selection};

const FUNCTION_COMMENT_ABSOLUTE_MAX: usize = 8;
const FUNCTION_CODE_LINES_PER_COMMENT: usize = 4;
const COMMENT_BLOCK_MIN_LINES: usize = 3;
const FILE_COMMENT_ABSOLUTE_MAX: usize = 8;
const FILE_CODE_LINES_PER_COMMENT: usize = 16;
pub(crate) const LEAF_COMMENT_MAX_LINES: usize = 3;
const TEMPLATE_COMMENT_MAX_LINES: usize = 3;
const OWNER_COMMENT_CAP_RULE: &str = "comment-policy/owner-comment-cap";
const COMMENT_CATEGORY_CAP_RULE: &str = "comment-policy/comment-category-cap";

#[cfg(test)]
thread_local! {
    static LINE_START_STORAGE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PHYSICAL_LINE_SCAN_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_line_start_storage() {
    LINE_START_STORAGE.with(|entries| entries.set(0));
}

#[cfg(test)]
fn reset_physical_line_scan_work() {
    PHYSICAL_LINE_SCAN_WORK.with(|work| work.set(0));
}

#[cfg(test)]
pub(crate) fn line_start_storage() -> usize {
    LINE_START_STORAGE.with(std::cell::Cell::get)
}

#[cfg(test)]
fn physical_line_scan_work() -> usize {
    PHYSICAL_LINE_SCAN_WORK.with(std::cell::Cell::get)
}

#[cfg(test)]
fn record_physical_line_scan_work(bytes: usize) {
    PHYSICAL_LINE_SCAN_WORK.with(|work| work.set(work.get() + bytes));
}

#[cfg(not(test))]
fn record_physical_line_scan_work(_bytes: usize) {}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct Span {
    pub(crate) start_byte: usize,
    pub(crate) end_byte: usize,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
}

impl Span {
    pub(crate) fn from_node(node: Node<'_>) -> Self {
        Self {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
        }
    }

    pub(crate) fn from_comment_node(node: Node<'_>, source: &str) -> Self {
        let mut span = Self::from_node(node);
        while span.end_byte > span.start_byte
            && source.as_bytes()[span.end_byte - 1].is_ascii_whitespace()
            && matches!(source.as_bytes()[span.end_byte - 1], b'\n' | b'\r')
        {
            span.end_byte -= 1;
        }
        if span.end_byte < node.end_byte() && span.end_line > span.start_line {
            span.end_line -= 1;
        }
        span
    }

    pub(crate) fn contains(&self, other: &Self) -> bool {
        self.start_byte <= other.start_byte && other.end_byte <= self.end_byte
    }

    pub(crate) fn lines(&self) -> impl Iterator<Item = usize> {
        self.start_line..=self.end_line
    }
}

#[derive(Debug)]
pub(crate) struct Function {
    pub(crate) span: Span,
    pub(crate) name: String,
    pub(crate) identity: IdentitySource,
    pub(crate) budget_code_lines: usize,
}

#[derive(Debug)]
pub(crate) struct TypeOwner {
    pub(crate) span: Span,
    pub(crate) name: String,
    pub(crate) identity: IdentitySource,
    pub(crate) budget_code_lines: usize,
}

#[derive(Debug)]
pub(crate) enum IdentitySource {
    Segments(Vec<String>),
    Child {
        parent: Option<IdentityId>,
        segment: String,
    },
}

impl IdentitySource {
    pub(crate) fn segments(segments: Vec<String>) -> Self {
        Self::Segments(segments)
    }

    pub(crate) fn child(parent: Option<IdentityId>, segment: String) -> Self {
        Self::Child { parent, segment }
    }

    fn insert(&self, arena: &mut IdentityArena) -> Result<IdentityId, AnalysisError> {
        match self {
            Self::Segments(segments) => arena.push_path(segments.iter().map(String::as_str)),
            Self::Child { parent, segment } => arena.push(*parent, segment.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Comment {
    pub(crate) span: Span,
    pub(crate) classification: CommentClassification,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct CommentClassification {
    role: CommentRole,
    attachment: CommentAttachmentScope,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum CommentRole {
    Narrative,
    Documentation(DocumentationAudience),
    SafetyProof,
    ToolDirective,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum DocumentationAudience {
    General,
    Public,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CommentAttachmentScope {
    File,
    Type,
    Inferred,
}

impl CommentClassification {
    pub(crate) const fn narrative() -> Self {
        Self {
            role: CommentRole::Narrative,
            attachment: CommentAttachmentScope::Inferred,
        }
    }

    pub(crate) const fn documentation(attachment: CommentAttachmentScope) -> Self {
        Self {
            role: CommentRole::Documentation(DocumentationAudience::General),
            attachment,
        }
    }

    pub(crate) const fn public_documentation(attachment: CommentAttachmentScope) -> Self {
        Self {
            role: CommentRole::Documentation(DocumentationAudience::Public),
            attachment,
        }
    }

    pub(crate) const fn safety_proof() -> Self {
        Self {
            role: CommentRole::SafetyProof,
            attachment: CommentAttachmentScope::Inferred,
        }
    }

    pub(crate) const fn tool_directive() -> Self {
        Self {
            role: CommentRole::ToolDirective,
            attachment: CommentAttachmentScope::Inferred,
        }
    }

    pub(crate) const fn attachment(self) -> CommentAttachmentScope {
        self.attachment
    }

    fn static_policy(self, policy: &PolicyConfig) -> StaticPolicy {
        match self.role {
            CommentRole::Narrative => policy.narrative(),
            CommentRole::Documentation(DocumentationAudience::General) => policy.documentation(),
            CommentRole::Documentation(DocumentationAudience::Public) => {
                policy.public_documentation()
            }
            CommentRole::SafetyProof => policy.safety_proof(),
            CommentRole::ToolDirective => policy.tool_directive(),
        }
    }

    const fn role(self) -> CommentRole {
        self.role
    }

    pub(crate) fn uses_relative_budget(self, policy: &PolicyConfig) -> bool {
        self.static_policy(policy) == StaticPolicy::Relative
    }

    fn uses_absolute_owner_cap(self, policy: &PolicyConfig) -> bool {
        matches!(
            self.static_policy(policy),
            StaticPolicy::Relative | StaticPolicy::OwnerCapped
        )
    }
}

#[derive(Debug)]
pub(crate) struct Leaf {
    pub(crate) span: Span,
    pub(crate) name: String,
}

pub(crate) struct TreeOwnership {
    pub(crate) function_budget: Vec<Vec<Comment>>,
    pub(crate) function_parents: Vec<Option<TreeOwner>>,
    pub(crate) type_budget: Vec<Vec<Comment>>,
    pub(crate) type_parents: Vec<Option<TreeOwner>>,
    pub(crate) leaves: Vec<Vec<Comment>>,
    pub(crate) leaf_parents: Vec<Option<TreeOwner>>,
    pub(crate) file: Vec<Comment>,
    pub(crate) comment_owners: Vec<Option<TreeOwner>>,
}

#[derive(Clone, Copy)]
pub(crate) struct TreeInput<'input> {
    pub(crate) functions: &'input [Function],
    pub(crate) types: &'input [TypeOwner],
    pub(crate) leaves: &'input [Leaf],
    pub(crate) comments: &'input [Comment],
    pub(crate) ownership: &'input TreeOwnership,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct CodeToken {
    event: CodeEvent,
    pub(crate) kind: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum CodeEvent {
    Enter,
    Atom,
    Leave,
}

impl CodeToken {
    pub(crate) fn enter(kind: &str) -> Self {
        Self {
            event: CodeEvent::Enter,
            kind: kind.to_owned(),
            text: String::new(),
        }
    }

    pub(crate) fn atom(kind: &str, text: &str) -> Self {
        Self {
            event: CodeEvent::Atom,
            kind: kind.to_owned(),
            text: text.to_owned(),
        }
    }

    pub(crate) fn is_atom(&self) -> bool {
        self.event == CodeEvent::Atom
    }

    pub(crate) fn leave(kind: &str) -> Self {
        Self {
            event: CodeEvent::Leave,
            kind: kind.to_owned(),
            text: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct OwnerSnapshot {
    pub(crate) kind: OwnerKind,
    pub(crate) name: String,
    pub(crate) identity: IdentityId,
    pub(crate) span: Span,
    pub(crate) parent: Option<usize>,
    pub(crate) code: Vec<CodeToken>,
}

#[derive(Debug, Clone)]
pub(crate) struct CommentSnapshot {
    pub(crate) classification: CommentClassification,
    pub(crate) text: String,
    pub(crate) span: Span,
    pub(crate) owner: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedFile {
    pub(crate) identities: IdentityArena,
    pub(crate) owners: Vec<OwnerSnapshot>,
    pub(crate) comments: Vec<CommentSnapshot>,
}

pub(crate) fn tree_document(
    source: &str,
    input: TreeInput<'_>,
    code: Vec<Vec<CodeToken>>,
    mut identities: IdentityArena,
) -> Result<ParsedFile, AnalysisError> {
    let file_span = Span {
        start_byte: 0,
        end_byte: source.len(),
        start_line: 1,
        end_line: source.lines().count().max(1),
    };
    let mut owners =
        Vec::with_capacity(1 + input.functions.len() + input.types.len() + input.leaves.len());
    let file_identity = identities.push_path(["file"])?;
    owners.push(OwnerSnapshot {
        kind: OwnerKind::File,
        name: "<file>".to_owned(),
        identity: file_identity,
        span: file_span,
        parent: None,
        code: code.first().cloned().unwrap_or_default(),
    });
    for (index, function) in input.functions.iter().enumerate() {
        owners.push(OwnerSnapshot {
            kind: OwnerKind::Function,
            name: function.name.clone(),
            identity: function.identity.insert(&mut identities)?,
            span: function.span.clone(),
            parent: input.ownership.function_parents[index]
                .map(|parent| {
                    owner_snapshot_index(parent, input.functions.len(), input.types.len())
                })
                .or(Some(0)),
            code: code.get(index + 1).cloned().unwrap_or_default(),
        });
    }
    let type_offset = 1 + input.functions.len();
    for (index, type_owner) in input.types.iter().enumerate() {
        owners.push(OwnerSnapshot {
            kind: OwnerKind::Type,
            name: type_owner.name.clone(),
            identity: type_owner.identity.insert(&mut identities)?,
            span: type_owner.span.clone(),
            parent: input.ownership.type_parents[index]
                .map(|parent| {
                    owner_snapshot_index(parent, input.functions.len(), input.types.len())
                })
                .or(Some(0)),
            code: code.get(type_offset + index).cloned().unwrap_or_default(),
        });
    }
    let leaf_offset = type_offset + input.types.len();
    for (index, leaf) in input.leaves.iter().enumerate() {
        owners.push(OwnerSnapshot {
            kind: OwnerKind::Leaf,
            name: leaf.name.clone(),
            identity: identities.push_path([leaf.name.as_str()])?,
            span: leaf.span.clone(),
            parent: input.ownership.leaf_parents[index]
                .map(|parent| {
                    owner_snapshot_index(parent, input.functions.len(), input.types.len())
                })
                .or(Some(0)),
            code: code.get(leaf_offset + index).cloned().unwrap_or_default(),
        });
    }
    let comments = input
        .comments
        .iter()
        .zip(&input.ownership.comment_owners)
        .map(|(comment, owner)| CommentSnapshot {
            classification: comment.classification,
            text: comment.text.clone(),
            span: comment.span.clone(),
            owner: owner.map_or(0, |owner| {
                owner_snapshot_index(owner, input.functions.len(), input.types.len())
            }),
        })
        .collect();
    Ok(ParsedFile {
        identities,
        owners,
        comments,
    })
}

fn owner_snapshot_index(owner: TreeOwner, function_count: usize, type_count: usize) -> usize {
    match owner {
        TreeOwner::Function(index) => index + 1,
        TreeOwner::Type(index) => function_count + index + 1,
        TreeOwner::Leaf(index) => function_count + type_count + index + 1,
    }
}

pub(crate) fn tree_findings(
    path: &Path,
    source: &str,
    selection: &Selection,
    input: TreeInput<'_>,
    language: &str,
    policy: &PolicyConfig,
) -> Vec<Finding> {
    let positions = PhysicalCommentPositions::new(source, input.comments);
    let mut findings = function_findings(
        path,
        &positions,
        selection,
        input.functions,
        &input.ownership.function_budget,
        policy,
    );
    findings.extend(type_findings(
        path,
        &positions,
        selection,
        input.types,
        &input.ownership.type_budget,
        policy,
    ));
    findings.extend(leaf_findings(
        path,
        selection,
        input.leaves,
        &input.ownership.leaves,
        language,
        policy,
    ));
    findings.extend(file_findings_with_lines(
        path,
        source,
        &positions,
        selection,
        &input.ownership.file,
        input.comments,
        policy,
    ));
    findings
}

pub(crate) fn template_findings(
    path: &Path,
    selection: &Selection,
    comments: &[Comment],
    owner: &Span,
    policy: &PolicyConfig,
) -> Vec<Finding> {
    let template_selected =
        selection.selects_owner(OwnerKind::Template, owner.start_byte, owner.end_byte);
    if comments.is_empty() || !template_selected {
        return Vec::new();
    }
    let mut findings = owner_comment_cap_finding_with_policy(
        path,
        OwnerKind::Template,
        "template",
        comments,
        policy,
    )
    .into_iter()
    .collect::<Vec<_>>();
    findings.extend(configured_category_cap_findings(
        path, "template", comments, policy,
    ));
    let narrative: Vec<Comment> = comments
        .iter()
        .filter(|comment| comment.classification.uses_relative_budget(policy))
        .cloned()
        .collect();
    let lines = comment_lines(&narrative);
    if lines.len() <= TEMPLATE_COMMENT_MAX_LINES {
        return findings;
    }
    findings.push(Finding {
        path: path.display().to_string(),
        line: first_line(&lines),
        rule: "comment-policy/template-comment-budget",
        message: format!(
            "template owns {} comment lines; allowance is {TEMPLATE_COMMENT_MAX_LINES}",
            lines.len()
        ),
    });
    findings
}

fn function_findings(
    path: &Path,
    positions: &PhysicalCommentPositions,
    selection: &Selection,
    functions: &[Function],
    budget: &[Vec<Comment>],
    policy: &PolicyConfig,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (function, budget_comments) in functions.iter().zip(budget) {
        findings.extend(scoped_owner_findings(
            path,
            positions,
            selection,
            ScopedOwner {
                kind: OwnerKind::Function,
                kind_label: "function",
                budget_rule: "comment-policy/function-comment-budget",
                name: &function.name,
                span: &function.span,
                budget_code_lines: function.budget_code_lines,
            },
            budget_comments,
            policy,
        ));
    }
    findings
}

fn type_findings(
    path: &Path,
    positions: &PhysicalCommentPositions,
    selection: &Selection,
    types: &[TypeOwner],
    budget: &[Vec<Comment>],
    policy: &PolicyConfig,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (type_owner, budget_comments) in types.iter().zip(budget) {
        findings.extend(scoped_owner_findings(
            path,
            positions,
            selection,
            ScopedOwner {
                kind: OwnerKind::Type,
                kind_label: "type",
                budget_rule: "comment-policy/type-comment-budget",
                name: &type_owner.name,
                span: &type_owner.span,
                budget_code_lines: type_owner.budget_code_lines,
            },
            budget_comments,
            policy,
        ));
    }
    findings
}

struct ScopedOwner<'owner> {
    kind: OwnerKind,
    kind_label: &'static str,
    budget_rule: &'static str,
    name: &'owner str,
    span: &'owner Span,
    budget_code_lines: usize,
}

fn scoped_owner_findings(
    path: &Path,
    positions: &PhysicalCommentPositions,
    selection: &Selection,
    owner: ScopedOwner<'_>,
    budget_comments: &[Comment],
    policy: &PolicyConfig,
) -> Vec<Finding> {
    if budget_comments.is_empty()
        || !selection.selects_owner(owner.kind, owner.span.start_byte, owner.span.end_byte)
    {
        return Vec::new();
    }

    let mut budget_comments = budget_comments.to_vec();
    budget_comments.sort_by_key(|comment| comment.span.start_byte);
    let owner_label = format!("{} `{}`", owner.kind_label, owner.name);
    let mut findings = owner_comment_cap_finding_with_policy(
        path,
        owner.kind,
        &owner_label,
        &budget_comments,
        policy,
    )
    .into_iter()
    .collect::<Vec<_>>();
    findings.extend(configured_category_cap_findings(
        path,
        &owner_label,
        &budget_comments,
        policy,
    ));
    let narrative: Vec<Comment> = budget_comments
        .iter()
        .filter(|comment| comment.classification.uses_relative_budget(policy))
        .cloned()
        .collect();
    let narrative_lines = comment_lines(&narrative);
    if narrative_lines.is_empty() {
        return findings;
    }
    for block in comment_blocks(positions, &narrative) {
        let lines = comment_lines(&block);
        if lines.len() >= COMMENT_BLOCK_MIN_LINES {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&lines),
                rule: "comment-policy/comment-block-budget",
                message: format!(
                    "{COMMENT_BLOCK_MIN_LINES}+-comment run inside {owner_label}; split the code or keep the local rationale below {COMMENT_BLOCK_MIN_LINES} lines"
                ),
            });
        }
    }

    let code_lines = owner.budget_code_lines;
    let allowance = FUNCTION_COMMENT_ABSOLUTE_MAX
        .min(1_usize.max(code_lines / FUNCTION_CODE_LINES_PER_COMMENT));
    if narrative_lines.len() > allowance {
        findings.push(Finding {
            path: path.display().to_string(),
            line: first_line(&narrative_lines),
            rule: owner.budget_rule,
            message: format!(
                "{owner_label} owns {} comment lines for {code_lines} code lines; allowance is {allowance}",
                narrative_lines.len()
            ),
        });
    }
    findings
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TreeOwner {
    Function(usize),
    Type(usize),
    Leaf(usize),
}

fn leaf_findings(
    path: &Path,
    selection: &Selection,
    leaves: &[Leaf],
    owned: &[Vec<Comment>],
    language: &str,
    policy: &PolicyConfig,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (leaf, owned_comments) in leaves.iter().zip(owned) {
        let narrative: Vec<Comment> = owned_comments
            .iter()
            .filter(|comment| comment.classification.uses_relative_budget(policy))
            .cloned()
            .collect();
        let narrative_lines = comment_lines(&narrative);
        let owner_touched =
            selection.selects_owner(OwnerKind::Leaf, leaf.span.start_byte, leaf.span.end_byte);
        if !owner_touched {
            continue;
        }
        findings.extend(owner_comment_cap_finding_with_policy(
            path,
            OwnerKind::Leaf,
            &format!("{language} leaf `{}`", leaf.name),
            owned_comments,
            policy,
        ));
        findings.extend(configured_category_cap_findings(
            path,
            &format!("{language} leaf `{}`", leaf.name),
            owned_comments,
            policy,
        ));
        if narrative_lines.len() > LEAF_COMMENT_MAX_LINES {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&narrative_lines),
                rule: "comment-policy/leaf-comment-budget",
                message: format!(
                    "{} comment lines own {language} leaf `{}`; allowance is {LEAF_COMMENT_MAX_LINES}",
                    narrative_lines.len(),
                    leaf.name
                ),
            });
        }
    }
    findings
}

pub(crate) fn file_findings(
    path: &Path,
    source: &str,
    selection: &Selection,
    comments: &[Comment],
    all_comments: &[Comment],
    policy: &PolicyConfig,
) -> Vec<Finding> {
    if comments.is_empty() {
        return Vec::new();
    }
    let positions = PhysicalCommentPositions::new(source, all_comments);
    file_findings_with_lines(
        path,
        source,
        &positions,
        selection,
        comments,
        all_comments,
        policy,
    )
}

fn file_findings_with_lines(
    path: &Path,
    source: &str,
    positions: &PhysicalCommentPositions,
    selection: &Selection,
    comments: &[Comment],
    all_comments: &[Comment],
    policy: &PolicyConfig,
) -> Vec<Finding> {
    let file_span = Span {
        start_byte: 0,
        end_byte: source.len(),
        start_line: 1,
        end_line: source.lines().count().max(1),
    };
    if comments.is_empty()
        || !selection.selects_owner(OwnerKind::File, file_span.start_byte, file_span.end_byte)
    {
        return Vec::new();
    }
    let mut findings = owner_comment_cap_finding_with_policy(
        path,
        OwnerKind::File,
        "file scope",
        comments,
        policy,
    )
    .into_iter()
    .collect::<Vec<_>>();
    findings.extend(configured_category_cap_findings(
        path,
        "file scope",
        comments,
        policy,
    ));
    let narrative: Vec<Comment> = comments
        .iter()
        .filter(|comment| comment.classification.uses_relative_budget(policy))
        .cloned()
        .collect();
    let narrative_lines = comment_lines(&narrative);
    if narrative_lines.is_empty() {
        return findings;
    }

    for block in comment_blocks(positions, &narrative) {
        let lines = comment_lines(&block);
        if lines.len() >= COMMENT_BLOCK_MIN_LINES {
            findings.push(Finding {
                path: path.display().to_string(),
                line: first_line(&lines),
                rule: "comment-policy/comment-block-budget",
                message: format!(
                    "{COMMENT_BLOCK_MIN_LINES}+-comment run at file scope; keep rationale beside its owner or below {COMMENT_BLOCK_MIN_LINES} lines"
                ),
            });
        }
    }

    let code_lines = code_line_count(source, &file_span, all_comments);
    let allowance =
        FILE_COMMENT_ABSOLUTE_MAX.min(2_usize.max(code_lines / FILE_CODE_LINES_PER_COMMENT));
    if narrative_lines.len() > allowance {
        findings.push(Finding {
            path: path.display().to_string(),
            line: first_line(&narrative_lines),
            rule: "comment-policy/file-comment-budget",
            message: format!(
                "file scope owns {} comment lines for {code_lines} code lines; allowance is {allowance}",
                narrative_lines.len()
            ),
        });
    }
    findings
}

pub(crate) fn owner_comment_cap_finding_with_policy(
    path: &Path,
    owner_kind: OwnerKind,
    owner: &str,
    comments: &[Comment],
    policy: &PolicyConfig,
) -> Option<Finding> {
    let lines: BTreeSet<usize> = comments
        .iter()
        .filter(|comment| comment.classification.uses_absolute_owner_cap(policy))
        .flat_map(|comment| comment.span.lines())
        .collect();
    let relative_lines: BTreeSet<usize> = comments
        .iter()
        .filter(|comment| comment.classification.uses_relative_budget(policy))
        .flat_map(|comment| comment.span.lines())
        .collect();
    let allowance = match owner_kind {
        OwnerKind::Function | OwnerKind::Type => FUNCTION_COMMENT_ABSOLUTE_MAX,
        OwnerKind::File => FILE_COMMENT_ABSOLUTE_MAX,
        OwnerKind::Leaf | OwnerKind::TomlKey => LEAF_COMMENT_MAX_LINES,
        OwnerKind::Template => TEMPLATE_COMMENT_MAX_LINES,
    };
    (lines.len() > allowance && relative_lines.len() <= allowance).then(|| Finding {
        path: path.display().to_string(),
        line: first_line(&lines),
        rule: OWNER_COMMENT_CAP_RULE,
        message: format!(
            "{owner} owns {} statically budgeted comment lines; absolute allowance is {allowance}",
            lines.len()
        ),
    })
}

pub(crate) fn configured_category_cap_findings(
    path: &Path,
    owner: &str,
    comments: &[Comment],
    policy: &PolicyConfig,
) -> Vec<Finding> {
    let mut capped_lines: BTreeMap<(CommentRole, usize), BTreeSet<usize>> = BTreeMap::new();
    for comment in comments {
        let StaticPolicy::Capped(allowance) = comment.classification.static_policy(policy) else {
            continue;
        };
        capped_lines
            .entry((comment.classification.role(), allowance))
            .or_default()
            .extend(comment.span.lines());
    }
    capped_lines
        .into_iter()
        .filter_map(|((category, allowance), lines)| {
            (lines.len() > allowance).then(|| Finding {
                path: path.display().to_string(),
                line: first_line(&lines),
                rule: COMMENT_CATEGORY_CAP_RULE,
                message: format!(
                    "{owner} owns {} {} comment lines; configured allowance is {allowance}",
                    lines.len(),
                    category.label()
                ),
            })
        })
        .collect()
}

impl CommentRole {
    const fn label(self) -> &'static str {
        match self {
            Self::Narrative => "narrative",
            Self::Documentation(DocumentationAudience::General) => "documentation",
            Self::Documentation(DocumentationAudience::Public) => "public-documentation",
            Self::SafetyProof => "safety-proof",
            Self::ToolDirective => "tool-directive",
        }
    }
}

struct PhysicalCommentPositions(HashMap<usize, PhysicalLinePosition>);

impl PhysicalCommentPositions {
    fn new(source: &str, comments: &[Comment]) -> Self {
        let mut cursor = PhysicalLineCursor::new(source);
        let positions = comments
            .iter()
            .map(|comment| {
                (
                    comment.span.start_byte,
                    cursor.advance_to(comment.span.start_byte),
                )
            })
            .collect::<HashMap<_, _>>();
        debug_assert_eq!(positions.len(), comments.len());
        Self(positions)
    }

    fn get(&self, comment: &Comment) -> PhysicalLinePosition {
        self.0[&comment.span.start_byte]
    }
}

struct PhysicalLineCursor<'source> {
    source: &'source [u8],
    offset: usize,
    line_start: usize,
    line_number: usize,
    last_non_whitespace: Option<usize>,
}

impl<'source> PhysicalLineCursor<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source: source.as_bytes(),
            offset: 0,
            line_start: 0,
            line_number: 1,
            last_non_whitespace: None,
        }
    }

    fn advance_to(&mut self, offset: usize) -> PhysicalLinePosition {
        debug_assert!(self.offset <= offset);
        record_physical_line_scan_work(offset - self.offset);
        for (relative, byte) in self.source[self.offset..offset].iter().enumerate() {
            let absolute = self.offset + relative;
            if *byte == b'\n' {
                self.line_start = absolute + 1;
                self.line_number += 1;
            } else if !byte.is_ascii_whitespace() {
                self.last_non_whitespace = Some(absolute);
            }
        }
        self.offset = offset;
        PhysicalLinePosition {
            number: self.line_number,
            starts_line: self
                .last_non_whitespace
                .is_none_or(|byte| byte < self.line_start),
        }
    }
}

#[derive(Clone, Copy)]
struct PhysicalLinePosition {
    number: usize,
    starts_line: bool,
}

fn comment_blocks(positions: &PhysicalCommentPositions, comments: &[Comment]) -> Vec<Vec<Comment>> {
    let mut blocks: Vec<Vec<Comment>> = Vec::new();
    let mut previous_position: Option<PhysicalLinePosition> = None;
    for comment in comments {
        let position = positions.get(comment);
        if let Some(block) = blocks.last_mut()
            && block.last().is_some_and(|previous| {
                position.number == previous.span.end_line + 1
                    && previous_position.is_some_and(|previous| previous.starts_line)
                    && position.starts_line
            })
        {
            block.push(comment.clone());
        } else {
            blocks.push(vec![comment.clone()]);
        }
        previous_position = Some(position);
    }
    blocks
}

fn comment_lines(comments: &[Comment]) -> BTreeSet<usize> {
    comments
        .iter()
        .flat_map(|comment| comment.span.lines())
        .collect()
}

fn code_line_count(source: &str, owner: &Span, comments: &[Comment]) -> usize {
    let mut comments = comments.iter().peekable();
    while comments
        .peek()
        .is_some_and(|comment| comment.span.end_byte <= owner.start_byte)
    {
        comments.next();
    }
    let mut current = comments.next();
    let mut line_has_code = false;
    let mut code_lines = 0;
    for (offset, byte) in source.as_bytes()[owner.start_byte..owner.end_byte]
        .iter()
        .copied()
        .enumerate()
    {
        let absolute = owner.start_byte + offset;
        while current.is_some_and(|comment| comment.span.end_byte <= absolute) {
            current = comments.next();
        }
        if byte == b'\n' {
            code_lines += usize::from(line_has_code);
            line_has_code = false;
        } else if current.is_none_or(|comment| {
            absolute < comment.span.start_byte || comment.span.end_byte <= absolute
        }) && !byte.is_ascii_whitespace()
        {
            line_has_code = true;
        }
    }
    code_lines + usize::from(line_has_code)
}

fn first_line(lines: &BTreeSet<usize>) -> usize {
    lines.first().copied().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::Path;

    use crate::{Finding, SourceFile, analyze_all};

    use super::{
        Comment, CommentClassification, PhysicalCommentPositions, Span, comment_blocks,
        line_start_storage, physical_line_scan_work, reset_line_start_storage,
        reset_physical_line_scan_work,
    };

    #[test]
    fn sixteen_mib_newline_dense_rust_keeps_exact_findings_without_line_storage() {
        const SOURCE_BYTES: usize = 16 * 1_024 * 1_024;
        const TAIL: &str = concat!(
            "fn operation() {\n",
            "    // The first invariant governs startup.\n",
            "    // The second invariant governs recovery.\n",
            "    // The third invariant governs shutdown.\n",
            "    run();\n",
            "}\n",
        );
        let prefix_lines = SOURCE_BYTES - TAIL.len();
        let mut source = "\n".repeat(prefix_lines);
        source.push_str(TAIL);
        assert_eq!(
            source.len(),
            SOURCE_BYTES,
            "fixture must stay exactly 16 MiB"
        );
        reset_line_start_storage();

        let findings = analyze_all(SourceFile {
            path: Path::new("src/lib.rs"),
            text: &source,
        })
        .expect("newline-dense Rust source must parse");

        assert_eq!(
            findings,
            [
                Finding {
                    path: "src/lib.rs".to_owned(),
                    line: prefix_lines + 2,
                    rule: "comment-policy/comment-block-budget",
                    message: "3+-comment run inside function `operation`; split the code or keep the local rationale below 3 lines".to_owned(),
                },
                Finding {
                    path: "src/lib.rs".to_owned(),
                    line: prefix_lines + 2,
                    rule: "comment-policy/function-comment-budget",
                    message: "function `operation` owns 3 comment lines for 3 code lines; allowance is 1".to_owned(),
                },
            ]
        );
        assert_eq!(
            line_start_storage(),
            0,
            "physical comment grouping must retain no per-line index"
        );
    }

    #[test]
    fn physical_comment_grouping_scans_source_once() {
        const COMMENT_COUNT: usize = 256;
        let mut source = String::from("fn operation() {\n");
        for comment in 0..COMMENT_COUNT {
            writeln!(source, "    // Invariant {comment}.")
                .expect("writing to a String cannot fail");
        }
        source.push_str("    run();\n}\n");
        reset_physical_line_scan_work();

        let findings = analyze_all(SourceFile {
            path: Path::new("src/lib.rs"),
            text: &source,
        })
        .expect("comment-dense Rust source must parse");

        assert!(
            !findings.is_empty(),
            "the fixture must exercise comment policy"
        );
        assert!(
            physical_line_scan_work() <= source.len(),
            "physical-line work must be one monotonic source pass"
        );
    }

    #[test]
    fn physical_comment_grouping_shares_one_scan_across_owners() {
        const OWNER_COUNT: usize = 256;
        let mut source = String::new();
        for owner in 0..OWNER_COUNT {
            writeln!(
                source,
                "fn operation_{owner}() {{\n    // Coupled to operation {owner}.\n    run();\n}}"
            )
            .expect("writing to a String cannot fail");
        }
        reset_physical_line_scan_work();

        let findings = analyze_all(SourceFile {
            path: Path::new("src/lib.rs"),
            text: &source,
        })
        .expect("many-owner Rust source must parse");

        assert!(findings.is_empty(), "one comment stays within each budget");
        assert!(
            physical_line_scan_work() <= source.len(),
            "all owners must share one monotonic source pass"
        );
    }

    #[test]
    fn physical_comment_grouping_preserves_line_ending_semantics() {
        let block_counts = [("\n", [1, 2, 3]), ("\r\n", [1, 2, 3]), ("\r", [1, 1, 1])].map(
            |(separator, lines)| {
                let mut source = String::new();
                let mut comments = Vec::new();
                for (index, line) in lines.into_iter().enumerate() {
                    let text = format!("// Invariant {index}.");
                    let start_byte = source.len();
                    source.push_str(&text);
                    comments.push(Comment {
                        span: Span {
                            start_byte,
                            end_byte: source.len(),
                            start_line: line,
                            end_line: line,
                        },
                        classification: CommentClassification::narrative(),
                        text,
                    });
                    if index + 1 < lines.len() {
                        source.push_str(separator);
                    }
                }
                let positions = PhysicalCommentPositions::new(&source, &comments);
                comment_blocks(&positions, &comments).len()
            },
        );

        assert_eq!(block_counts, [1, 1, 3]);
    }
}
