//! Stable rule identifiers and the metadata published with machine-readable reports.

/// Metadata describing one comment-policy rule.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct Rule {
    /// Stable identifier reported in [`crate::Finding::rule`].
    pub id: &'static str,
    /// PascalCase display name.
    pub name: &'static str,
    /// One-sentence summary of the rule.
    pub short_description: &'static str,
    /// What the rule enforces and why.
    pub full_description: &'static str,
    /// Markdown remediation guidance.
    pub help: &'static str,
}

/// Narrative comment lines in a function exceed its relative budget.
pub const FUNCTION_COMMENT_BUDGET: &str = "comment-policy/function-comment-budget";
/// Narrative comment lines in a type exceed its relative budget.
pub const TYPE_COMMENT_BUDGET: &str = "comment-policy/type-comment-budget";
/// Three or more consecutive narrative comment lines form a block.
pub const COMMENT_BLOCK_BUDGET: &str = "comment-policy/comment-block-budget";
/// Narrative comment lines on a leaf owner exceed the leaf allowance.
pub const LEAF_COMMENT_BUDGET: &str = "comment-policy/leaf-comment-budget";
/// Narrative comment lines on a member owner exceed the member allowance.
pub const MEMBER_COMMENT_BUDGET: &str = "comment-policy/member-comment-budget";
/// File-scope narrative comment lines exceed the file budget.
pub const FILE_COMMENT_BUDGET: &str = "comment-policy/file-comment-budget";
/// Narrative comment lines in a template owner exceed the template allowance.
pub const TEMPLATE_COMMENT_BUDGET: &str = "comment-policy/template-comment-budget";
/// Statically budgeted comment lines exceed the owner's absolute cap.
pub const OWNER_COMMENT_CAP: &str = "comment-policy/owner-comment-cap";
/// A configured semantic category exceeds its `max-lines` allowance.
pub const COMMENT_CATEGORY_CAP: &str = "comment-policy/comment-category-cap";
/// An unchanged comment's owning code or semantic role changed.
pub const COMMENT_OWNER_CHANGED: &str = "comment-policy/comment-owner-changed";
/// An unchanged comment moved to a different owner.
pub const COMMENT_REPARENTED: &str = "comment-policy/comment-reparented";

/// Every rule, in stable report order.
pub const ALL: &[Rule] = &[
    Rule {
        id: FUNCTION_COMMENT_BUDGET,
        name: "FunctionCommentBudget",
        short_description: "A function owns more narrative comment lines than its code allows.",
        full_description: "Functions may own at most min(8, max(1, code_lines / 4)) narrative comment lines, where code_lines counts the physical rows assigned to that function's budget. Nested functions and types use their own budgets.",
        help: "Remove narration that restates the code, move rationale into names or tests, or split the function so each part carries only the comments it needs.",
    },
    Rule {
        id: TYPE_COMMENT_BUDGET,
        name: "TypeCommentBudget",
        short_description: "A type owns more narrative comment lines than its code allows.",
        full_description: "Recognized type owners use the same relative narrative budget as functions: at most min(8, max(1, code_lines / 4)) lines over the rows assigned to the type. Attached documentation is budgeted under its own semantic category.",
        help: "Trim narration inside the type body, or keep explanations attached to the specific member they describe.",
    },
    Rule {
        id: COMMENT_BLOCK_BUDGET,
        name: "CommentBlockBudget",
        short_description: "Three or more consecutive narrative comment lines form a block.",
        full_description: "Runs of three or more consecutive narrative-only comment lines inside a function, type, or file scope fail regardless of the owner's total budget.",
        help: "Keep local rationale below three consecutive lines, or split the code so each piece carries a short comment.",
    },
    Rule {
        id: LEAF_COMMENT_BUDGET,
        name: "LeafCommentBudget",
        short_description: "A constant, static, or equivalent leaf owns more than 3 narrative lines.",
        full_description: "Leaf owners such as constants, statics, and TOML keys get at most 3 narrative comment lines.",
        help: "Shorten the leaf's rationale to three lines or fewer, or document the concept once at a higher-level owner.",
    },
    Rule {
        id: MEMBER_COMMENT_BUDGET,
        name: "MemberCommentBudget",
        short_description: "A field, variant, or equivalent member owns more than 3 narrative lines.",
        full_description: "Member owners such as struct, union, and tuple fields and enum variants budget their own comments and get at most 3 narrative lines; their code rows still size the declaring type's relative budget.",
        help: "Shorten the member's rationale to three lines or fewer, or configure a capped allowance for its semantic category in fuck-ai-comments.toml.",
    },
    Rule {
        id: FILE_COMMENT_BUDGET,
        name: "FileCommentBudget",
        short_description: "File-scope narrative comment lines exceed the file budget.",
        full_description: "File scope may own at most min(8, max(2, code_lines / 16)) narrative comment lines, where code_lines counts the whole file.",
        help: "Move narration beside the function or type it describes, or delete header commentary that the code already expresses.",
    },
    Rule {
        id: TEMPLATE_COMMENT_BUDGET,
        name: "TemplateCommentBudget",
        short_description: "A template owner owns more than 3 narrative lines.",
        full_description: "HTML, CSS, and Astro template owners get at most 3 narrative comment lines.",
        help: "Shorten the template comment or remove markup narration that the structure already conveys.",
    },
    Rule {
        id: OWNER_COMMENT_CAP,
        name: "OwnerCommentCap",
        short_description: "Statically budgeted comment lines exceed the owner's absolute cap.",
        full_description: "Comments using built-in relative or owner-capped policies cannot exceed 8 lines on function, type, or file owners, or 3 lines on leaf, template, or TOML owners, regardless of code size.",
        help: "Reduce the comment to the owner's absolute allowance, or configure a capped allowance for that semantic category in fuck-ai-comments.toml.",
    },
    Rule {
        id: COMMENT_CATEGORY_CAP,
        name: "CommentCategoryCap",
        short_description: "A configured semantic category exceeds its max-lines allowance.",
        full_description: "A comment category configured with mode = \"capped\" may own at most max-lines lines per owner; max-lines = 0 bans the category.",
        help: "Shorten the comment to the configured allowance, or raise max-lines for that category in fuck-ai-comments.toml.",
    },
    Rule {
        id: COMMENT_OWNER_CHANGED,
        name: "CommentOwnerChanged",
        short_description: "Owning code changed while the comment's meaningful text did not.",
        full_description: "When a comment's owning code or semantic role changes between revisions and its normalized meaningful text stays the same, the comment must be edited or deleted to attest that it remains true.",
        help: "Re-read the comment against the new code, then edit it (even minimally) or delete it to attest the change.",
    },
    Rule {
        id: COMMENT_REPARENTED,
        name: "CommentReparented",
        short_description: "An unchanged comment moved to a different owner.",
        full_description: "When a comment's normalized meaningful text is unchanged but it now belongs to a different owner, the move must be attested.",
        help: "Edit or delete the comment so the new ownership is deliberate.",
    },
];

/// Finds a rule by its stable identifier.
#[must_use]
pub fn lookup(id: &str) -> Option<&'static Rule> {
    ALL.iter().find(|rule| rule.id == id)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{ALL, lookup};

    #[test]
    fn rule_identifiers_are_unique_and_namespaced() {
        let ids: BTreeSet<_> = ALL.iter().map(|rule| rule.id).collect();
        assert_eq!(ids.len(), ALL.len(), "rule ids must be unique");
        for rule in ALL {
            assert!(
                rule.id.starts_with("comment-policy/"),
                "{} is outside the comment-policy namespace",
                rule.id
            );
            assert_eq!(lookup(rule.id), Some(rule));
        }
    }

    #[test]
    fn rule_metadata_fits_code_scanning_limits() {
        for rule in ALL {
            assert!(
                !rule.name.is_empty() && rule.name.len() <= 255,
                "{}",
                rule.id
            );
            for text in [rule.short_description, rule.full_description, rule.help] {
                assert!(!text.is_empty() && text.len() <= 1024, "{}", rule.id);
            }
        }
    }

    #[test]
    fn unknown_identifiers_have_no_rule() {
        assert_eq!(lookup("comment-policy/unknown"), None);
    }
}
