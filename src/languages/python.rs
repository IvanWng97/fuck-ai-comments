use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::Node;

use super::tree::{
    ANONYMOUS_FUNCTION_NAME, CallableSubtrees, LanguageSpec, OwnerCandidate, OwnerLocation,
    analyze, document, node_text, starts_physical_line,
};
use super::walk::{WalkEvent, events};
use crate::model::{AnalysisError, Finding, Selection};
use crate::policy::{CommentKind, ParsedFile, Span};

const BANDIT_NUMERIC_TEST_ID_DIGITS: usize = 3;

#[derive(Clone, Copy)]
struct Python;

#[derive(Default)]
struct PythonContext {
    tool_directives: HashSet<usize>,
    docstring_scopes: HashMap<usize, DocstringScope>,
}

pub(crate) fn analyze_file(
    path: &Path,
    source: &str,
    selection: &Selection,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze(path, source, selection, Python)
}

pub(crate) fn parse_file(path: &Path, source: &str) -> Result<ParsedFile, AnalysisError> {
    document(path, source, Python)
}

impl LanguageSpec for Python {
    type Context = PythonContext;

    fn label(self) -> &'static str {
        "Python"
    }

    fn grammar(self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn build_context(self, root: Node<'_>, source: &str) -> Result<Self::Context, AnalysisError> {
        if !has_context_candidate(root, source) {
            return Ok(PythonContext::default());
        }
        PythonContextBuilder::new(source).build(root)
    }

    fn owner(
        self,
        node: Node<'_>,
        location: OwnerLocation<'_>,
        source: &str,
        _context: &Self::Context,
        function_depth: usize,
        _callable_subtrees: &CallableSubtrees,
    ) -> Result<Option<OwnerCandidate>, AnalysisError> {
        if let Some(class) = class_owner_node(node, location) {
            let name = class
                .child_by_field_name("name")
                .map(|name| node_text(name, source).to_owned());
            let Some(name) = name else {
                return Ok(None);
            };
            return Ok(Some(OwnerCandidate::type_owner(
                Span::from_node(node),
                name.clone(),
                vec![format!("class:{name}")],
            )));
        }
        if let Some(function) = function_owner_node(node, location) {
            return Ok(Some(OwnerCandidate::function(
                Span::from_node(node),
                function
                    .child_by_field_name("name")
                    .map_or(ANONYMOUS_FUNCTION_NAME, |name| node_text(name, source))
                    .to_owned(),
                Vec::new(),
            )));
        }
        if node.kind() != "assignment" || function_depth != 0 {
            return Ok(None);
        }
        let name = node
            .child_by_field_name("left")
            .filter(|left| left.kind() == "identifier")
            .map(|left| node_text(left, source));
        let Some(name) = name else {
            return Ok(None);
        };
        let uppercase = name.bytes().any(|byte| byte.is_ascii_uppercase())
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_');
        Ok(uppercase.then(|| OwnerCandidate::leaf(Span::from_node(node), name.to_owned())))
    }

    fn classify_comment(
        self,
        node: Node<'_>,
        _source: &str,
        context: &Self::Context,
    ) -> Option<CommentKind> {
        if let Some(scope) = context.docstring_scopes.get(&node.id()) {
            return Some(match scope {
                DocstringScope::Function => CommentKind::Narrative,
                DocstringScope::Type => CommentKind::TypeNarrative,
                DocstringScope::File => CommentKind::FileNarrative,
            });
        }
        (node.kind() == "comment").then(|| {
            if context.tool_directives.contains(&node.id()) {
                CommentKind::ToolDirective
            } else {
                CommentKind::Narrative
            }
        })
    }
}

fn class_owner_node<'tree>(
    node: Node<'tree>,
    location: OwnerLocation<'tree>,
) -> Option<Node<'tree>> {
    match node.kind() {
        "decorated_definition" => decorated_body(node, "class_definition"),
        "class_definition"
            if location
                .parent()
                .is_none_or(|parent| parent.kind() != "decorated_definition") =>
        {
            Some(node)
        }
        _ => None,
    }
}

fn function_owner_node<'tree>(
    node: Node<'tree>,
    location: OwnerLocation<'tree>,
) -> Option<Node<'tree>> {
    match node.kind() {
        "decorated_definition" => decorated_body(node, "function_definition"),
        "function_definition"
            if location
                .parent()
                .is_none_or(|parent| parent.kind() != "decorated_definition") =>
        {
            Some(node)
        }
        _ => None,
    }
}

fn decorated_body<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn directive_placement(body: &str) -> Option<DirectivePlacement> {
    match body {
        "fmt: off" | "fmt: on" => Some(DirectivePlacement::Standalone),
        "fmt: skip" => Some(DirectivePlacement::Trailing),
        _ => is_line_suppression(body).then_some(DirectivePlacement::Trailing),
    }
}

fn has_context_candidate(root: Node<'_>, source: &str) -> bool {
    events(root).any(|event| {
        let WalkEvent::Enter(node) = event else {
            return false;
        };
        record_placement_operations(1);
        match node.kind() {
            "string" => true,
            "comment" => directive_placement(comment_body(node_text(node, source))).is_some(),
            _ => false,
        }
    })
}

#[derive(Clone, Copy)]
enum DirectivePlacement {
    Standalone,
    Trailing,
}

fn comment_body(comment: &str) -> &str {
    comment
        .trim_start()
        .strip_prefix('#')
        .unwrap_or(comment)
        .trim()
}

fn is_line_suppression(body: &str) -> bool {
    let directive = body.split_once(" #").map_or(body, |(head, _)| head).trim();
    is_noqa(directive)
        || is_type_ignore(directive)
        || directive.eq_ignore_ascii_case("pragma: no cover")
        || is_nosec(directive)
}

fn is_noqa(directive: &str) -> bool {
    if directive.eq_ignore_ascii_case("noqa") {
        return true;
    }
    strip_ascii_case_prefix(directive, "noqa:").is_some_and(|codes| valid_list(codes, is_lint_code))
}

fn is_type_ignore(directive: &str) -> bool {
    if directive == "type: ignore" {
        return true;
    }
    directive
        .strip_prefix("type: ignore[")
        .and_then(|codes| codes.strip_suffix(']'))
        .is_some_and(|codes| valid_list(codes, is_mypy_code))
}

fn is_nosec(directive: &str) -> bool {
    if directive == "nosec" {
        return true;
    }
    directive
        .strip_prefix("nosec ")
        .is_some_and(|tests| valid_list(tests, is_bandit_test))
}

fn valid_list(input: &str, valid_item: fn(&str) -> bool) -> bool {
    let mut items = input
        .split(|character: char| character == ',' || character.is_ascii_whitespace())
        .filter(|item| !item.is_empty());
    let Some(first) = items.next() else {
        return false;
    };
    valid_item(first) && items.all(valid_item)
}

fn is_lint_code(code: &str) -> bool {
    let letters = code.bytes().take_while(u8::is_ascii_uppercase).count();
    letters > 0 && letters < code.len() && code.as_bytes()[letters..].iter().all(u8::is_ascii_digit)
}

fn is_mypy_code(code: &str) -> bool {
    code.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && code
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_bandit_test(test: &str) -> bool {
    let is_id = test.strip_prefix('B').is_some_and(|digits| {
        digits.len() == BANDIT_NUMERIC_TEST_ID_DIGITS
            && digits.bytes().all(|byte| byte.is_ascii_digit())
    });
    let is_name = test.contains('_')
        && test
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    is_id || is_name
}

fn strip_ascii_case_prefix<'text>(text: &'text str, prefix: &str) -> Option<&'text str> {
    text.get(..prefix.len())
        .filter(|candidate| candidate.eq_ignore_ascii_case(prefix))?;
    text.get(prefix.len()..)
}

fn controls_line_directive(kind: &str) -> bool {
    kind.ends_with("_statement")
        || kind.ends_with("_definition")
        || matches!(
            kind,
            "decorator"
                | "elif_clause"
                | "else_clause"
                | "except_clause"
                | "finally_clause"
                | "case_clause"
        )
}

#[derive(Clone, Copy)]
enum DocstringScope {
    Function,
    Type,
    File,
}

struct PythonContextBuilder<'source> {
    source: &'source str,
    context: PythonContext,
    frames: Vec<PythonFrame>,
    invariant_error: Option<&'static str>,
}

impl<'source> PythonContextBuilder<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            context: PythonContext::default(),
            frames: Vec::new(),
            invariant_error: None,
        }
    }

    fn build(mut self, root: Node<'_>) -> Result<PythonContext, AnalysisError> {
        for event in events(root) {
            match event {
                WalkEvent::Enter(node) => self.enter(node),
                WalkEvent::Leave(node) => self.leave(node),
            }
        }
        self.finish()
    }

    fn finish(mut self) -> Result<PythonContext, AnalysisError> {
        if !self.frames.is_empty() {
            self.record_invariant("Python placement scopes did not close");
        }
        match self.invariant_error {
            Some(detail) => Err(AnalysisError::Invariant(detail.to_owned())),
            None => Ok(self.context),
        }
    }

    fn enter(&mut self, node: Node<'_>) {
        record_materialized_context_frame();
        record_placement_operations(1);
        let kind = node.kind();
        let frame_kind = PythonFrameKind::from_kind(kind);
        let is_barrier = matches!(frame_kind, PythonFrameKind::Module | PythonFrameKind::Block);
        let controls_line = controls_line_directive(kind);
        let inherited_control_row = self
            .frames
            .last()
            .and_then(|parent| parent.trailing_control_row);
        let sibling_control_row = self.frames.last().and_then(|parent| {
            parent
                .last_named_child
                .filter(|child| child.controls_line_directive)
                .map(|child| child.end_row)
        });
        let own_control_row = controls_line.then(|| node.start_position().row);
        let trailing_control_row = (!is_barrier)
            .then(|| {
                inherited_control_row
                    .max(sibling_control_row)
                    .max(own_control_row)
            })
            .flatten();

        let statement_scope = self.statement_scope(node);
        let docstring_candidate_scope = node
            .is_named()
            .then(|| self.frames.last().and_then(|parent| parent.statement_scope))
            .flatten();

        if kind == "comment"
            && let Some(placement) = directive_placement(comment_body(node_text(node, self.source)))
        {
            record_placement_operations(node.start_position().column + 1);
            let starts_line = starts_physical_line(node, self.source);
            let row = node.start_position().row;
            match placement {
                DirectivePlacement::Trailing
                    if !starts_line && trailing_control_row == Some(row) =>
                {
                    self.context.tool_directives.insert(node.id());
                }
                DirectivePlacement::Standalone if starts_line => {
                    let direct_parent_is_barrier = self.frames.last().is_some_and(|parent| {
                        matches!(
                            parent.kind,
                            PythonFrameKind::Module | PythonFrameKind::Block
                        )
                    });
                    if direct_parent_is_barrier {
                        self.context.tool_directives.insert(node.id());
                    } else if let Some(parent) = self.frames.last_mut()
                        && parent.controls_line_directive
                    {
                        parent.standalone_candidates.push(node.id());
                    }
                }
                DirectivePlacement::Standalone | DirectivePlacement::Trailing => {}
            }
        }

        if let Some(parent) = self.frames.last_mut() {
            parent.has_direct_block |= kind == "block";
            if node.is_named() {
                parent.saw_non_extra_named_child |= !node.is_extra();
                parent.last_named_child = Some(PythonNamedChild {
                    end_row: node.end_position().row,
                    controls_line_directive: controls_line,
                });
            }
        }
        self.frames.push(PythonFrame {
            node_id: node.id(),
            kind: frame_kind,
            controls_line_directive: controls_line,
            trailing_control_row,
            last_named_child: None,
            has_direct_block: false,
            standalone_candidates: Vec::new(),
            statement_scope,
            docstring_candidate_scope,
            saw_non_extra_named_child: false,
            contains_interpolation: kind == "interpolation",
            named_child_count: 0,
            first_named_child_is_static_expression: false,
            all_named_children_are_static_text: true,
        });
    }

    fn leave(&mut self, node: Node<'_>) {
        let Some(frame) = self.frames.pop() else {
            self.record_invariant("Python traversal frame stack underflowed");
            return;
        };
        if frame.node_id != node.id() {
            self.record_invariant("Python traversal frames closed out of order");
        }
        let static_facts = frame.static_facts(node, self.source);
        if let Some(scope) = frame.docstring_candidate_scope
            && static_facts.is_static_expression
        {
            self.context.docstring_scopes.insert(node.id(), scope);
        }
        if frame.has_direct_block {
            self.context
                .tool_directives
                .extend(frame.standalone_candidates);
        }
        if let Some(parent) = self.frames.last_mut() {
            parent.contains_interpolation |= frame.contains_interpolation;
            if node.is_named() && !node.is_extra() {
                parent.named_child_count += 1;
                if parent.named_child_count == 1 {
                    parent.first_named_child_is_static_expression =
                        static_facts.is_static_expression;
                }
                parent.all_named_children_are_static_text &= static_facts.is_static_text;
            }
        }
    }

    fn statement_scope(&self, node: Node<'_>) -> Option<DocstringScope> {
        if node.kind() != "expression_statement"
            || !node.is_named()
            || self
                .frames
                .last()
                .is_none_or(|parent| parent.saw_non_extra_named_child)
        {
            return None;
        }
        match self.frames.last()?.kind {
            PythonFrameKind::Module => Some(DocstringScope::File),
            PythonFrameKind::Block => match self.frames.iter().rev().nth(1)?.kind {
                PythonFrameKind::Function => Some(DocstringScope::Function),
                PythonFrameKind::Class => Some(DocstringScope::Type),
                PythonFrameKind::Module | PythonFrameKind::Block | PythonFrameKind::Other => None,
            },
            PythonFrameKind::Function | PythonFrameKind::Class | PythonFrameKind::Other => None,
        }
    }

    fn record_invariant(&mut self, detail: &'static str) {
        self.invariant_error.get_or_insert(detail);
    }
}

#[derive(Clone, Copy)]
struct PythonNamedChild {
    end_row: usize,
    controls_line_directive: bool,
}

struct PythonFrame {
    node_id: usize,
    kind: PythonFrameKind,
    controls_line_directive: bool,
    trailing_control_row: Option<usize>,
    last_named_child: Option<PythonNamedChild>,
    has_direct_block: bool,
    standalone_candidates: Vec<usize>,
    statement_scope: Option<DocstringScope>,
    docstring_candidate_scope: Option<DocstringScope>,
    saw_non_extra_named_child: bool,
    contains_interpolation: bool,
    named_child_count: usize,
    first_named_child_is_static_expression: bool,
    all_named_children_are_static_text: bool,
}

impl PythonFrame {
    fn static_facts(&self, node: Node<'_>, source: &str) -> StaticExpressionFacts {
        let is_static_text = node.kind() == "string"
            && !self.contains_interpolation
            && has_static_text_prefix(node_text(node, source));
        let is_static_expression = is_static_text
            || (node.kind() == "concatenated_string"
                && self.named_child_count > 0
                && self.all_named_children_are_static_text)
            || (node.kind() == "parenthesized_expression"
                && self.named_child_count == 1
                && self.first_named_child_is_static_expression);
        StaticExpressionFacts {
            is_static_expression,
            is_static_text,
        }
    }
}

#[derive(Clone, Copy)]
struct StaticExpressionFacts {
    is_static_expression: bool,
    is_static_text: bool,
}

#[derive(Clone, Copy)]
enum PythonFrameKind {
    Module,
    Block,
    Function,
    Class,
    Other,
}

impl PythonFrameKind {
    fn from_kind(kind: &str) -> Self {
        match kind {
            "module" => Self::Module,
            "block" => Self::Block,
            "function_definition" => Self::Function,
            "class_definition" => Self::Class,
            _ => Self::Other,
        }
    }
}

fn has_static_text_prefix(text: &str) -> bool {
    let prefix = text
        .find(['\'', '"'])
        .and_then(|quote| text.get(..quote))
        .unwrap_or(text);
    !prefix
        .bytes()
        .any(|byte| matches!(byte.to_ascii_lowercase(), b'b' | b'f'))
}

#[cfg(test)]
thread_local! {
    static PLACEMENT_OPERATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MATERIALIZED_CONTEXT_FRAMES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_placement_operations(count: usize) {
    PLACEMENT_OPERATIONS.with(|operations| operations.set(operations.get() + count));
}

#[cfg(test)]
fn record_materialized_context_frame() {
    MATERIALIZED_CONTEXT_FRAMES.with(|frames| frames.set(frames.get() + 1));
}

#[cfg(not(test))]
fn record_placement_operations(_count: usize) {}

#[cfg(not(test))]
fn record_materialized_context_frame() {}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn nested_directive_source(depth: usize) -> String {
        let mut source = String::from("def work():\n    value = (\n");
        for index in 0..depth {
            writeln!(source, "        call_{index}(  # noqa: F821")
                .expect("writing to a String cannot fail");
        }
        source.push_str("            1\n");
        source.push_str(&"        )\n".repeat(depth));
        source.push_str("    )\n    return value\n");
        source
    }

    fn nested_directive_placement_operations(depth: usize) -> usize {
        let source = nested_directive_source(depth);
        PLACEMENT_OPERATIONS.with(|operations| operations.set(0));
        parse_file(Path::new("deep.py"), &source).expect("valid nested Python");
        PLACEMENT_OPERATIONS.with(std::cell::Cell::get)
    }

    fn token_dense_placement_operations(token_count: usize) -> usize {
        let mut source = String::from("value = base");
        for index in 0..token_count {
            write!(source, " + item_{index}").expect("writing to a String cannot fail");
        }
        source.push('\n');
        PLACEMENT_OPERATIONS.with(|operations| operations.set(0));
        parse_file(Path::new("dense.py"), &source).expect("valid token-dense Python");
        PLACEMENT_OPERATIONS.with(std::cell::Cell::get)
    }

    fn python_tree(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("Python grammar should load");
        parser
            .parse(source, None)
            .expect("Python parser should run")
    }

    #[test]
    fn nested_directive_placement_operations_are_linear() {
        let shallow = nested_directive_placement_operations(16);
        let medium = nested_directive_placement_operations(32);
        let deep = nested_directive_placement_operations(64);

        assert_eq!(deep - medium, 2 * (medium - shallow));
    }

    #[test]
    fn token_dense_placement_operations_are_linear_without_comments() {
        let shallow = token_dense_placement_operations(64);
        let medium = token_dense_placement_operations(128);
        let deep = token_dense_placement_operations(256);

        assert!(shallow > 0);
        assert_eq!(deep - medium, 2 * (medium - shallow));
    }

    #[test]
    fn finish_rejects_an_unclosed_python_frame() {
        let tree = python_tree("");
        let mut builder = PythonContextBuilder::new("");
        builder.enter(tree.root_node());

        let error = builder.finish().err().expect("unclosed scopes must fail");

        assert!(matches!(error, AnalysisError::Invariant(_)));
    }

    #[test]
    fn finish_rejects_python_frames_closed_out_of_order() {
        let source = "value = 1\n";
        let tree = python_tree(source);
        let root = tree.root_node();
        let child = root.named_child(0).expect("module statement");
        let mut builder = PythonContextBuilder::new(source);
        builder.enter(root);
        builder.leave(child);

        let error = builder
            .finish()
            .err()
            .expect("mismatched traversal frames must fail");

        assert!(matches!(error, AnalysisError::Invariant(_)));
    }

    #[test]
    fn standalone_format_marker_before_a_direct_block_is_a_tool_directive() {
        let source = "if ready:\n    # fmt: off\n    value = 1\n";

        let document = parse_file(Path::new("marker.py"), source).expect("valid Python");
        let marker = document
            .comments
            .iter()
            .find(|comment| comment.text.contains("fmt: off"))
            .expect("format marker comment");

        assert_eq!(marker.kind, CommentKind::ToolDirective);
    }

    #[test]
    fn ordinary_comments_do_not_materialize_directive_placement_facts() {
        let source = "# file rationale\nvalue = 1  # trailing rationale\nif ready:\n    # block rationale\n    value = 2\n";
        let tree = python_tree(source);
        MATERIALIZED_CONTEXT_FRAMES.with(|frames| frames.set(0));

        let context = Python
            .build_context(tree.root_node(), source)
            .expect("valid Python placement context");

        assert!(context.tool_directives.is_empty());
        assert_eq!(MATERIALIZED_CONTEXT_FRAMES.with(std::cell::Cell::get), 0);
    }

    #[test]
    fn directive_and_docstring_candidates_materialize_context_frames() {
        for source in ["value = 1  # noqa\n", "\"\"\"module docs\"\"\"\n"] {
            let tree = python_tree(source);
            MATERIALIZED_CONTEXT_FRAMES.with(|frames| frames.set(0));

            Python
                .build_context(tree.root_node(), source)
                .expect("valid Python placement context");

            assert!(MATERIALIZED_CONTEXT_FRAMES.with(std::cell::Cell::get) > 0);
        }
    }
}
