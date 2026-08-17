use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[cfg(test)]
use std::cell::Cell;

use similar::{Algorithm, DiffTag, TextDiff, capture_diff_slices};

use crate::languages;
use crate::model::{AnalysisError, Finding, OwnerKind, Selection, SourceFile};
use crate::policy::{CommentSnapshot, OwnerSnapshot, ParsedFile};

const STALE_RULE: &str = "comment-policy/comment-owner-changed";
const REPARENTED_RULE: &str = "comment-policy/comment-reparented";

#[cfg(test)]
thread_local! {
    static OWNER_FRONTIER_VISITS: Cell<usize> = const { Cell::new(0) };
    static OWNER_ANCHOR_CANDIDATE_EVALUATIONS: Cell<usize> = const { Cell::new(0) };
}

pub(crate) fn analyze(
    before: SourceFile<'_>,
    after: SourceFile<'_>,
) -> Result<Vec<Finding>, AnalysisError> {
    if !languages::same_adapter(before.path, after.path)? {
        return languages::analyze_file(after.path, after.text, &Selection::all());
    }

    let before_document = languages::parse_file(before.path, before.text)?;
    let after_document = languages::parse_file(after.path, after.text)?;
    validate_document(&before_document, before.path.to_string_lossy().as_ref())?;
    validate_document(&after_document, after.path.to_string_lossy().as_ref())?;

    let anchors = LineAnchors::new(before.text, after.text, &before_document, &after_document);
    let owners = pair_owners(&before_document, &after_document, &anchors)?;
    let owner_changes = OwnerChangeIndex::new(&before_document, &after_document, &owners);
    let comments = pair_comments(&before_document, &after_document, &owners, &anchors)?;
    let selection = semantic_selection(
        &before_document,
        &after_document,
        &owners,
        &comments,
        &owner_changes,
    );

    let mut findings = languages::analyze_file(after.path, after.text, &selection)?;
    findings.extend(change_findings(
        after,
        &before_document,
        &after_document,
        &owners,
        &comments,
        &owner_changes,
    ));
    findings.sort();
    findings.dedup();
    Ok(findings)
}

fn validate_document(document: &ParsedFile, path: &str) -> Result<(), AnalysisError> {
    let Some(file) = document.owners.first() else {
        return Err(AnalysisError::Invariant(format!(
            "{path} has no implicit file owner"
        )));
    };
    if file.kind != OwnerKind::File || file.parent.is_some() {
        return Err(AnalysisError::Invariant(format!(
            "{path} has an invalid implicit file owner"
        )));
    }
    if document.owners.iter().skip(1).any(|owner| {
        owner
            .parent
            .is_none_or(|parent| parent >= document.owners.len())
    }) {
        return Err(AnalysisError::Invariant(format!(
            "{path} contains an owner with no valid parent"
        )));
    }
    if document
        .comments
        .iter()
        .any(|comment| comment.owner >= document.owners.len())
    {
        return Err(AnalysisError::Invariant(format!(
            "{path} contains a comment with no valid owner"
        )));
    }
    Ok(())
}

#[derive(Debug)]
struct Pairing {
    before_to_after: Vec<Option<usize>>,
    after_to_before: Vec<Option<usize>>,
}

impl Pairing {
    fn new(before_len: usize, after_len: usize) -> Self {
        Self {
            before_to_after: vec![None; before_len],
            after_to_before: vec![None; after_len],
        }
    }

    fn insert(&mut self, before: usize, after: usize) -> Result<(), AnalysisError> {
        if self.before_to_after[before].is_some() || self.after_to_before[after].is_some() {
            return Err(AnalysisError::AmbiguousChange(
                "one owner was selected by multiple pairings".to_owned(),
            ));
        }
        self.before_to_after[before] = Some(after);
        self.after_to_before[after] = Some(before);
        Ok(())
    }
}

struct OwnerChangeIndex {
    identity_path_changed: Vec<bool>,
}

impl OwnerChangeIndex {
    fn new(before: &ParsedFile, after: &ParsedFile, pairs: &Pairing) -> Self {
        let children = owners_by_parent(before);
        let mut identity_path_changed = vec![true; before.owners.len()];
        let mut frontier = VecDeque::from([0]);

        while let Some(before_index) = frontier.pop_front() {
            let old_owner = &before.owners[before_index];
            if let Some(after_index) = pairs.before_to_after[before_index] {
                let new_owner = &after.owners[after_index];
                let expected_after_parent = old_owner
                    .parent
                    .and_then(|parent| pairs.before_to_after[parent]);
                let parent_path_changed = old_owner.parent.is_some_and(|parent| {
                    before.owners[parent].kind == OwnerKind::Type && identity_path_changed[parent]
                });
                let type_parent_changed = old_owner.parent.is_some_and(|parent| {
                    before.owners[parent].kind == OwnerKind::Type
                        && new_owner.parent != expected_after_parent
                });
                identity_path_changed[before_index] = parent_path_changed
                    || type_parent_changed
                    || old_owner.kind != new_owner.kind
                    || old_owner.identity != new_owner.identity;
            }
            frontier.extend(children[before_index].iter().copied());
        }

        Self {
            identity_path_changed,
        }
    }

    fn changed(
        &self,
        before: &ParsedFile,
        after: &ParsedFile,
        before_index: usize,
        after_index: usize,
    ) -> bool {
        self.identity_path_changed[before_index]
            || before.owners[before_index].code != after.owners[after_index].code
    }
}

fn pair_owners(
    before: &ParsedFile,
    after: &ParsedFile,
    anchors: &LineAnchors,
) -> Result<Pairing, AnalysisError> {
    let mut pairs = Pairing::new(before.owners.len(), after.owners.len());
    pairs.insert(0, 0)?;
    let before_children = owners_by_parent(before);
    let after_children = owners_by_parent(after);
    let mut frontier = VecDeque::from([(0, 0)]);

    while let Some((before_parent, after_parent)) = frontier.pop_front() {
        loop {
            let stable = unique_stable_owner_pairs(
                before,
                after,
                &before_children[before_parent],
                &after_children[after_parent],
                &pairs,
            );
            enqueue_owner_pairs(stable, &mut pairs, &mut frontier)?;

            let anchored = anchored_owner_pairs(
                before,
                after,
                &before_children[before_parent],
                &after_children[after_parent],
                anchors,
                &pairs,
            )?;
            if anchored.is_empty() {
                break;
            }
            enqueue_owner_pairs(anchored, &mut pairs, &mut frontier)?;
        }
    }
    Ok(pairs)
}

fn owners_by_parent(document: &ParsedFile) -> Vec<Vec<usize>> {
    let mut children = vec![Vec::new(); document.owners.len()];
    for (index, owner) in document.owners.iter().enumerate().skip(1) {
        if let Some(parent) = owner.parent {
            children[parent].push(index);
        }
    }
    for siblings in &mut children {
        siblings.sort_unstable_by_key(|index| {
            let span = &document.owners[*index].span;
            (span.start_byte, span.end_byte)
        });
        debug_assert!(siblings.windows(2).all(|pair| {
            document.owners[pair[0]].span.end_byte <= document.owners[pair[1]].span.start_byte
        }));
    }
    children
}

fn enqueue_owner_pairs(
    new_pairs: impl IntoIterator<Item = (usize, usize)>,
    pairs: &mut Pairing,
    frontier: &mut VecDeque<(usize, usize)>,
) -> Result<(), AnalysisError> {
    for (before, after) in new_pairs {
        pairs.insert(before, after)?;
        frontier.push_back((before, after));
    }
    Ok(())
}

fn unique_stable_owner_pairs(
    before: &ParsedFile,
    after: &ParsedFile,
    before_children: &[usize],
    after_children: &[usize],
    pairs: &Pairing,
) -> Vec<(usize, usize)> {
    let before_by_key = stable_owners_by_key(before, before_children, &pairs.before_to_after);
    let after_by_key = stable_owners_by_key(after, after_children, &pairs.after_to_before);

    before_by_key
        .into_iter()
        .filter_map(|(key, before_index)| {
            before_index.zip(after_by_key.get(&key).copied().flatten())
        })
        .collect()
}

type StableOwnerKey<'identity> = (OwnerKind, &'identity [String]);

fn stable_owners_by_key<'document>(
    document: &'document ParsedFile,
    children: &[usize],
    paired: &[Option<usize>],
) -> BTreeMap<StableOwnerKey<'document>, Option<usize>> {
    let mut groups = BTreeMap::new();
    for &index in children {
        #[cfg(test)]
        OWNER_FRONTIER_VISITS.with(|visits| visits.set(visits.get() + 1));
        let owner = &document.owners[index];
        if paired[index].is_none() && has_stable_identity(owner) {
            groups
                .entry((owner.kind, owner.identity.as_slice()))
                .and_modify(|unique| *unique = None)
                .or_insert(Some(index));
        }
    }
    groups
}

fn has_stable_identity(owner: &OwnerSnapshot) -> bool {
    owner.identity.iter().all(|segment| {
        ["<anonymous>", "<destructured>", "<unknown>"]
            .iter()
            .all(|placeholder| !segment.contains(placeholder))
    })
}

fn anchored_owner_pairs(
    before: &ParsedFile,
    after: &ParsedFile,
    before_children: &[usize],
    after_children: &[usize],
    anchors: &LineAnchors,
    pairs: &Pairing,
) -> Result<Vec<(usize, usize)>, AnalysisError> {
    let scores = connected_owner_scores(
        before,
        after,
        before_children,
        after_children,
        anchors,
        pairs,
    )?;

    let before_choices = owner_choices(
        scores
            .iter()
            .map(|&(before, after, score)| (before, after, score)),
        "old owner",
    )?;
    let after_choices = owner_choices(
        scores
            .iter()
            .map(|&(before, after, score)| (after, before, score)),
        "new owner",
    )?;
    Ok(before_choices
        .into_iter()
        .filter(|(before_index, after_index)| after_choices.get(after_index) == Some(before_index))
        .collect())
}

fn connected_owner_scores(
    before: &ParsedFile,
    after: &ParsedFile,
    before_children: &[usize],
    after_children: &[usize],
    anchors: &LineAnchors,
    pairs: &Pairing,
) -> Result<Vec<(usize, usize, usize)>, AnalysisError> {
    let mut before_sweep = OwnerLineSweep::new(
        before,
        before_children
            .iter()
            .copied()
            .filter(|index| pairs.before_to_after[*index].is_none()),
    );
    let mut after_sweep = OwnerLineSweep::new(
        after,
        after_children
            .iter()
            .copied()
            .filter(|index| pairs.after_to_before[*index].is_none()),
    );
    let Some(before_lines) = before_sweep.line_range() else {
        return Ok(Vec::new());
    };
    if after_sweep.is_empty() {
        return Ok(Vec::new());
    }

    let mut scores = BTreeMap::new();
    for (&before_line, &after_line) in anchors.owner.range(before_lines) {
        let before_by_kind =
            unique_owner_evidence_by_kind(before, before_sweep.owners_at(before_line));
        let after_by_kind = unique_owner_evidence_by_kind(after, after_sweep.owners_at(after_line));
        for (kind, before_evidence) in before_by_kind {
            let Some(after_evidence) = after_by_kind.get(&kind).copied() else {
                continue;
            };
            let (OwnerEvidence::Unique(before_index), OwnerEvidence::Unique(after_index)) =
                (before_evidence, after_evidence)
            else {
                return Err(AnalysisError::AmbiguousChange(format!(
                    "exact line anchor {} connects multiple {kind:?} sibling owners",
                    before_line + 1
                )));
            };
            *scores.entry((before_index, after_index)).or_insert(0) += 1;
        }
    }
    Ok(scores
        .into_iter()
        .map(|((before, after), score)| (before, after, score))
        .collect())
}

#[derive(Clone, Copy)]
enum OwnerEvidence {
    Unique(usize),
    Ambiguous,
}

fn unique_owner_evidence_by_kind(
    document: &ParsedFile,
    candidates: &[usize],
) -> BTreeMap<OwnerKind, OwnerEvidence> {
    let mut by_kind = BTreeMap::new();
    for &index in candidates {
        #[cfg(test)]
        OWNER_ANCHOR_CANDIDATE_EVALUATIONS.with(|evaluations| {
            evaluations.set(evaluations.get() + 1);
        });
        by_kind
            .entry(document.owners[index].kind)
            .and_modify(|evidence| *evidence = OwnerEvidence::Ambiguous)
            .or_insert(OwnerEvidence::Unique(index));
    }
    by_kind
}

struct OwnerLineSweep<'document> {
    document: &'document ParsedFile,
    owners: Vec<usize>,
    first_active: usize,
    past_active: usize,
    previous_line: Option<usize>,
}

impl<'document> OwnerLineSweep<'document> {
    fn new(document: &'document ParsedFile, owners: impl IntoIterator<Item = usize>) -> Self {
        let owners: Vec<_> = owners.into_iter().collect();
        Self {
            document,
            owners,
            first_active: 0,
            past_active: 0,
            previous_line: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }

    fn line_range(&self) -> Option<std::ops::Range<usize>> {
        let first = &self.document.owners[*self.owners.first()?].span;
        let last = &self.document.owners[*self.owners.last()?].span;
        Some(first.start_line.saturating_sub(1)..last.end_line)
    }

    fn owners_at(&mut self, line: usize) -> &[usize] {
        debug_assert!(self.previous_line.is_none_or(|previous| previous <= line));
        while self.first_active < self.owners.len()
            && self.document.owners[self.owners[self.first_active]]
                .span
                .end_line
                <= line
        {
            self.first_active += 1;
        }
        self.past_active = self.past_active.max(self.first_active);
        while self.past_active < self.owners.len()
            && self.document.owners[self.owners[self.past_active]]
                .span
                .start_line
                .saturating_sub(1)
                <= line
        {
            self.past_active += 1;
        }
        self.previous_line = Some(line);
        &self.owners[self.first_active..self.past_active]
    }
}

fn owner_choices(
    candidates: impl Iterator<Item = (usize, usize, usize)>,
    side: &str,
) -> Result<BTreeMap<usize, usize>, AnalysisError> {
    let mut by_owner: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (owner, candidate, score) in candidates {
        by_owner.entry(owner).or_default().push((candidate, score));
    }
    by_owner
        .into_iter()
        .filter_map(|(owner, candidates)| {
            unique_best_choice(candidates.into_iter(), owner, side)
                .transpose()
                .map(|choice| choice.map(|candidate| (owner, candidate)))
        })
        .collect()
}

fn unique_best_choice(
    candidates: impl Iterator<Item = (usize, usize)>,
    owner_index: usize,
    side: &str,
) -> Result<Option<usize>, AnalysisError> {
    let mut best: Option<(usize, usize)> = None;
    let mut tied = false;
    for candidate in candidates {
        match best {
            None => best = Some(candidate),
            Some((_, score)) if candidate.1 > score => {
                best = Some(candidate);
                tied = false;
            }
            Some((_, score)) if candidate.1 == score => tied = true,
            Some(_) => {}
        }
    }
    if tied {
        return Err(AnalysisError::AmbiguousChange(format!(
            "{side} {owner_index} has exact anchors in multiple equally plausible owners"
        )));
    }
    Ok(best.map(|(index, _)| index))
}

fn pair_comments(
    before: &ParsedFile,
    after: &ParsedFile,
    owners: &Pairing,
    anchors: &LineAnchors,
) -> Result<Pairing, AnalysisError> {
    let mut pairs = Pairing::new(before.comments.len(), after.comments.len());
    let before_by_owner = group_comments(&before.comments, |_, comment| {
        owners.before_to_after[comment.owner].map(|owner| (owner, attestation_key(&comment.text)))
    });
    let after_by_owner = group_comments(&after.comments, |_, comment| {
        Some((comment.owner, attestation_key(&comment.text)))
    });
    for (key, old_indexes) in before_by_owner {
        let Some(new_indexes) = after_by_owner.get(&key) else {
            continue;
        };
        for (old_index, new_index) in align_comment_group(
            &old_indexes,
            new_indexes,
            &before.comments,
            &after.comments,
            anchors,
        )? {
            pairs.insert(old_index, new_index)?;
        }
    }

    let old_leftovers = group_comments(&before.comments, |index, comment| {
        pairs.before_to_after[index]
            .is_none()
            .then(|| attestation_key(&comment.text))
    });
    let new_leftovers = group_comments(&after.comments, |index, comment| {
        pairs.after_to_before[index]
            .is_none()
            .then(|| attestation_key(&comment.text))
    });
    for (key, old_indexes) in old_leftovers {
        let Some(new_indexes) = new_leftovers.get(&key) else {
            continue;
        };
        for (old_index, new_index) in line_anchored_comment_pairs(
            &old_indexes,
            new_indexes,
            &before.comments,
            &after.comments,
            anchors,
        ) {
            let old_owner = before.comments[old_index].owner;
            let new_owner = after.comments[new_index].owner;
            if owners.before_to_after[old_owner].is_none()
                && owners.after_to_before[new_owner].is_none()
            {
                return Err(AnalysisError::AmbiguousChange(format!(
                    "comment text {key:?} is anchored between two owners with no proven correspondence"
                )));
            }
            pairs.insert(old_index, new_index)?;
        }
        let old_unpaired: Vec<_> = old_indexes
            .into_iter()
            .filter(|index| pairs.before_to_after[*index].is_none())
            .collect();
        let new_unpaired: Vec<_> = new_indexes
            .iter()
            .copied()
            .filter(|index| pairs.after_to_before[*index].is_none())
            .collect();
        if !old_unpaired.is_empty() && !new_unpaired.is_empty() {
            return Err(AnalysisError::AmbiguousChange(format!(
                "comment text {key:?} survives across owners with no exact line anchor"
            )));
        }
    }
    Ok(pairs)
}

fn group_comments<K: Ord>(
    comments: &[CommentSnapshot],
    key: impl Fn(usize, &CommentSnapshot) -> Option<K>,
) -> BTreeMap<K, Vec<usize>> {
    let mut groups = BTreeMap::new();
    for (index, comment) in comments.iter().enumerate() {
        if let Some(key) = key(index, comment) {
            groups.entry(key).or_insert_with(Vec::new).push(index);
        }
    }
    groups
}

fn attestation_key(comment: &str) -> String {
    let body = strip_comment_delimiters(comment.trim());
    let collapsed = body
        .lines()
        .map(|line| line.trim().strip_prefix('*').unwrap_or(line.trim()).trim())
        .filter(|line| !line.is_empty())
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    collapsed
        .trim_end_matches(['.', '!', '?', '。', '！', '？'])
        .trim_end()
        .to_owned()
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

fn align_comment_group(
    old_indexes: &[usize],
    new_indexes: &[usize],
    old_comments: &[CommentSnapshot],
    new_comments: &[CommentSnapshot],
    anchors: &LineAnchors,
) -> Result<Vec<(usize, usize)>, AnalysisError> {
    let mut pairs = line_anchored_comment_pairs(
        old_indexes,
        new_indexes,
        old_comments,
        new_comments,
        anchors,
    );
    let mut paired_old: BTreeSet<_> = pairs.iter().map(|(old, _)| *old).collect();
    let mut paired_new: BTreeSet<_> = pairs.iter().map(|(_, new)| *new).collect();
    let old_remaining: Vec<_> = old_indexes
        .iter()
        .copied()
        .filter(|index| !paired_old.contains(index))
        .collect();
    let new_remaining: Vec<_> = new_indexes
        .iter()
        .copied()
        .filter(|index| !paired_new.contains(index))
        .collect();

    for (old, new) in
        exact_comment_sequence_pairs(&old_remaining, &new_remaining, old_comments, new_comments)
    {
        paired_old.insert(old);
        paired_new.insert(new);
        pairs.push((old, new));
    }
    let old_remaining: Vec<_> = old_indexes
        .iter()
        .copied()
        .filter(|index| !paired_old.contains(index))
        .collect();
    let new_remaining: Vec<_> = new_indexes
        .iter()
        .copied()
        .filter(|index| !paired_new.contains(index))
        .collect();
    if old_remaining.len() != new_remaining.len()
        && !old_remaining.is_empty()
        && !new_remaining.is_empty()
    {
        return Err(AnalysisError::AmbiguousChange(
            "same-key comments changed format while their sequence length also changed".to_owned(),
        ));
    }
    pairs.extend(old_remaining.into_iter().zip(new_remaining));
    pairs.sort_unstable();
    Ok(pairs)
}

fn line_anchored_comment_pairs(
    old_indexes: &[usize],
    new_indexes: &[usize],
    old_comments: &[CommentSnapshot],
    new_comments: &[CommentSnapshot],
    anchors: &LineAnchors,
) -> Vec<(usize, usize)> {
    let mut new_by_line: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for new_index in new_indexes {
        new_by_line
            .entry(new_comments[*new_index].span.start_line)
            .or_default()
            .push(*new_index);
    }
    let mut used_new = BTreeSet::new();
    let mut pairs = Vec::new();
    for old_index in old_indexes {
        let Some(new_line) = anchors.after_line(old_comments[*old_index].span.start_line) else {
            continue;
        };
        let Some(candidates) = new_by_line.get(&new_line) else {
            continue;
        };
        if let [new_index] = candidates.as_slice()
            && used_new.insert(*new_index)
        {
            pairs.push((*old_index, *new_index));
        }
    }
    pairs
}

fn exact_comment_sequence_pairs(
    old_indexes: &[usize],
    new_indexes: &[usize],
    old_comments: &[CommentSnapshot],
    new_comments: &[CommentSnapshot],
) -> Vec<(usize, usize)> {
    let old_text: Vec<_> = old_indexes
        .iter()
        .map(|index| old_comments[*index].text.as_str())
        .collect();
    let new_text: Vec<_> = new_indexes
        .iter()
        .map(|index| new_comments[*index].text.as_str())
        .collect();
    let mut pairs = Vec::new();
    for operation in capture_diff_slices(Algorithm::Myers, &old_text, &new_text) {
        if operation.tag() != DiffTag::Equal {
            continue;
        }
        for (old_position, new_position) in operation.old_range().zip(operation.new_range()) {
            pairs.push((old_indexes[old_position], new_indexes[new_position]));
        }
    }
    pairs
}

fn semantic_selection(
    before: &ParsedFile,
    after: &ParsedFile,
    owners: &Pairing,
    comments: &Pairing,
    owner_changes: &OwnerChangeIndex,
) -> Selection {
    let mut affected = BTreeSet::new();

    for (before_index, after_index) in owners.before_to_after.iter().enumerate() {
        if let Some(after_index) = after_index
            && owner_changes.changed(before, after, before_index, *after_index)
        {
            affected.insert(*after_index);
        }
    }
    for (before_index, after_index) in owners.before_to_after.iter().enumerate() {
        if after_index.is_none()
            && before.owners[before_index].kind == OwnerKind::Leaf
            && let Some(after_parent) = before.owners[before_index]
                .parent
                .and_then(|parent| owners.before_to_after[parent])
        {
            affected.insert(after_parent);
        }
    }
    for (after_index, before_index) in owners.after_to_before.iter().enumerate() {
        if before_index.is_none() {
            affected.insert(after_index);
        }
    }

    for (before_index, after_index) in comments.before_to_after.iter().enumerate() {
        match after_index {
            Some(after_index) => {
                let old_comment = &before.comments[before_index];
                let new_comment = &after.comments[*after_index];
                if old_comment.kind != new_comment.kind || old_comment.text != new_comment.text {
                    affected.insert(after.comments[*after_index].owner);
                }
                let old_owner = old_comment.owner;
                let new_owner = new_comment.owner;
                if owners.before_to_after[old_owner] != Some(new_owner) {
                    affected.insert(new_owner);
                }
            }
            None => {
                let old_owner = before.comments[before_index].owner;
                if let Some(new_owner) = owners.before_to_after[old_owner] {
                    affected.insert(new_owner);
                }
            }
        }
    }
    for (after_index, before_index) in comments.after_to_before.iter().enumerate() {
        if before_index.is_none() {
            let comment = &after.comments[after_index];
            affected.insert(comment.owner);
        }
    }

    let mut selection = Selection::default();
    for owner_index in affected {
        let owner = &after.owners[owner_index];
        selection.select_owner(owner.kind, owner.span.start_byte, owner.span.end_byte);
        if owner.kind == OwnerKind::Leaf {
            let mut parent = owner.parent;
            while let Some(parent_index) = parent {
                let parent_owner = &after.owners[parent_index];
                if parent_owner.kind == OwnerKind::Function {
                    selection.select_owner(
                        parent_owner.kind,
                        parent_owner.span.start_byte,
                        parent_owner.span.end_byte,
                    );
                    break;
                }
                parent = parent_owner.parent;
            }
        }
    }
    selection
}

fn change_findings(
    after_file: SourceFile<'_>,
    before: &ParsedFile,
    after: &ParsedFile,
    owners: &Pairing,
    comments: &Pairing,
    owner_changes: &OwnerChangeIndex,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (before_comment_index, after_comment_index) in comments.before_to_after.iter().enumerate() {
        let Some(after_comment_index) = after_comment_index else {
            continue;
        };
        let old_comment = &before.comments[before_comment_index];
        let new_comment = &after.comments[*after_comment_index];
        let paired_owner = owners.before_to_after[old_comment.owner];
        if paired_owner != Some(new_comment.owner) {
            findings.push(Finding {
                path: after_file.path.display().to_string(),
                line: new_comment.span.start_line,
                rule: REPARENTED_RULE,
                message: format!(
                    "unchanged comment moved from {} to {}; edit or delete it to attest the new ownership",
                    owner_label(&before.owners[old_comment.owner]),
                    owner_label(&after.owners[new_comment.owner]),
                ),
            });
            continue;
        }

        let Some(after_owner_index) = paired_owner else {
            continue;
        };
        let new_owner = &after.owners[after_owner_index];
        if owner_changes.changed(before, after, old_comment.owner, after_owner_index)
            || old_comment.kind != new_comment.kind
        {
            findings.push(Finding {
                path: after_file.path.display().to_string(),
                line: new_comment.span.start_line,
                rule: STALE_RULE,
                message: format!(
                    "{} or this comment's semantic role changed while its meaningful text did not; edit or delete it to attest that it remains true",
                    owner_label(new_owner)
                ),
            });
        }
    }
    findings
}

fn owner_label(owner: &OwnerSnapshot) -> String {
    match owner.kind {
        OwnerKind::File => "file owner".to_owned(),
        OwnerKind::Function => format!("function `{}`", owner.name),
        OwnerKind::Type => format!("type `{}`", owner.name),
        OwnerKind::Leaf => format!("leaf `{}`", owner.name),
        OwnerKind::Template => "template owner".to_owned(),
        OwnerKind::TomlKey => format!("TOML key `{}`", owner.name),
    }
}

struct LineAnchors {
    exact: BTreeMap<usize, usize>,
    owner: BTreeMap<usize, usize>,
}

impl LineAnchors {
    fn new(before: &str, after: &str, old: &ParsedFile, new: &ParsedFile) -> Self {
        let mut config = TextDiff::configure();
        config.algorithm(Algorithm::Myers);
        let diff = config.diff_lines(before, after);
        let comment_lines = |document: &ParsedFile| {
            document
                .comments
                .iter()
                .flat_map(|comment| comment.span.lines())
                .map(|line| line - 1)
                .collect::<BTreeSet<_>>()
        };
        let (old_comment_lines, new_comment_lines) = (comment_lines(old), comment_lines(new));
        let source_lines: Vec<_> = before.lines().collect();
        let mut exact = BTreeMap::new();
        let mut owner = BTreeMap::new();
        for operation in diff
            .ops()
            .iter()
            .filter(|operation| operation.tag() == DiffTag::Equal)
        {
            for (old_line, new_line) in operation.old_range().zip(operation.new_range()) {
                exact.insert(old_line, new_line);
                if !old_comment_lines.contains(&old_line)
                    && !new_comment_lines.contains(&new_line)
                    && source_lines
                        .get(old_line)
                        .is_some_and(|line| line.chars().any(char::is_alphanumeric))
                {
                    owner.insert(old_line, new_line);
                }
            }
        }
        Self { exact, owner }
    }

    fn after_line(&self, one_based_old_line: usize) -> Option<usize> {
        self.exact
            .get(&one_based_old_line.saturating_sub(1))
            .map(|line| line + 1)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::Path;

    use super::*;

    fn nested_kotlin(depth: usize, value: usize) -> String {
        let mut source = String::new();
        for level in 0..depth {
            writeln!(source, "class Level{level} {{").expect("writing to a String cannot fail");
        }
        writeln!(source, "fun run(): Int {{").expect("writing to a String cannot fail");
        writeln!(source, "// Coupled to the deepest implementation.")
            .expect("writing to a String cannot fail");
        writeln!(source, "return {value}").expect("writing to a String cannot fail");
        source.push_str("}\n");
        for _ in 0..depth {
            source.push_str("}\n");
        }
        source
    }

    fn owner_frontier_visits(depth: usize) -> usize {
        let before = nested_kotlin(depth, 1);
        let after = nested_kotlin(depth, 2);
        OWNER_FRONTIER_VISITS.with(|visits| visits.set(0));

        analyze(
            SourceFile {
                path: Path::new("Deep.kt"),
                text: &before,
            },
            SourceFile {
                path: Path::new("Deep.kt"),
                text: &after,
            },
        )
        .expect("valid deeply nested Kotlin change");

        OWNER_FRONTIER_VISITS.with(Cell::get)
    }

    fn same_line_anonymous_callbacks(count: usize, version: usize) -> String {
        let mut source = format!("const VERSION = {version};\n");
        for index in 0..count {
            write!(source, "(() => {{ stable{index}(); }})();")
                .expect("writing to a String cannot fail");
        }
        source.push('\n');
        source
    }

    fn owner_anchor_candidate_evaluations(count: usize) -> usize {
        let before = same_line_anonymous_callbacks(count, 1);
        let after = same_line_anonymous_callbacks(count, 2);
        OWNER_ANCHOR_CANDIDATE_EVALUATIONS.with(|evaluations| evaluations.set(0));

        let error = analyze(
            SourceFile {
                path: Path::new("callbacks.js"),
                text: &before,
            },
            SourceFile {
                path: Path::new("callbacks.js"),
                text: &after,
            },
        )
        .expect_err("same-line anonymous siblings are ambiguous");
        assert!(matches!(error, AnalysisError::AmbiguousChange(_)));

        OWNER_ANCHOR_CANDIDATE_EVALUATIONS.with(Cell::get)
    }

    #[test]
    fn deep_stable_owner_pairing_visits_each_owner_once_per_snapshot() {
        for depth in [25, 50, 100, 200] {
            let visits = owner_frontier_visits(depth);

            assert_eq!(
                visits,
                2 * (depth + 1),
                "each non-file owner must enter exactly one frontier per snapshot"
            );
        }
    }

    #[test]
    fn same_line_anonymous_owner_anchor_candidates_are_evaluated_linearly() {
        for count in [64, 128, 256] {
            assert_eq!(owner_anchor_candidate_evaluations(count), 2 * count);
        }
    }
}
