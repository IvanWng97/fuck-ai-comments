use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Range;

#[cfg(test)]
use std::cell::Cell;

use imara_diff::{Algorithm, Diff, InternedInput, Interner, Token, sources};

use crate::AnalysisContext;
use crate::identity::{
    CanonicalIdentity, CanonicalIdentityId, CanonicalIdentityInterner, CanonicalIdentityMap,
};
use crate::languages;
use crate::model::{AnalysisError, AnalysisProfile, Finding, OwnerKind, Selection, SourceFile};
use crate::policy::{CommentSnapshot, OwnerSnapshot, ParsedFile, Span};

mod attestation_key;

use attestation_key::AttestationKey;

const STALE_RULE: &str = "comment-policy/comment-owner-changed";
const REPARENTED_RULE: &str = "comment-policy/comment-reparented";
const CHANGE_DIFF_ALGORITHM: Algorithm = Algorithm::Myers;
const MAX_DIFF_TOKENS: usize = i32::MAX as usize - 1;

struct ChangeDiff<'source> {
    input: InternedInput<&'source str>,
    diff: Diff,
}

fn compute_change_diff<'source>(
    before: impl Iterator<Item = &'source str>,
    after: impl Iterator<Item = &'source str>,
) -> Result<ChangeDiff<'source>, AnalysisError> {
    #[cfg(test)]
    CHANGE_DIFF_COMPUTATIONS.with(|computations| computations.set(computations.get() + 1));
    let mut input = InternedInput::default();
    input.update_before(before);
    check_diff_capacity("before", input.before.len())?;
    input.update_after(after);
    check_diff_capacity("after", input.after.len())?;
    let diff = Diff::compute(CHANGE_DIFF_ALGORITHM, &input);
    Ok(ChangeDiff { input, diff })
}

fn check_diff_capacity(snapshot: &'static str, tokens: usize) -> Result<(), AnalysisError> {
    if tokens > MAX_DIFF_TOKENS {
        return Err(AnalysisError::DiffCapacity {
            snapshot,
            tokens,
            maximum: MAX_DIFF_TOKENS,
        });
    }
    Ok(())
}

fn visit_equal_ranges(
    diff: &Diff,
    before_len: usize,
    after_len: usize,
    mut visit: impl FnMut(Range<usize>, Range<usize>) -> Result<(), AnalysisError>,
) -> Result<(), AnalysisError> {
    let mut before_position = 0;
    let mut after_position = 0;
    for hunk in diff.hunks() {
        let before_start = hunk.before.start as usize;
        let before_end = hunk.before.end as usize;
        let after_start = hunk.after.start as usize;
        let after_end = hunk.after.end as usize;
        if before_start < before_position
            || after_start < after_position
            || before_end > before_len
            || after_end > after_len
        {
            return Err(AnalysisError::Invariant(
                "diff engine returned an invalid hunk range".to_owned(),
            ));
        }
        visit_equal_range(
            before_position..before_start,
            after_position..after_start,
            &mut visit,
        )?;
        before_position = before_end;
        after_position = after_end;
    }
    visit_equal_range(
        before_position..before_len,
        after_position..after_len,
        &mut visit,
    )
}

fn visit_equal_range(
    before: Range<usize>,
    after: Range<usize>,
    visit: &mut impl FnMut(Range<usize>, Range<usize>) -> Result<(), AnalysisError>,
) -> Result<(), AnalysisError> {
    if before.len() != after.len() {
        return Err(AnalysisError::Invariant(
            "diff engine returned unequal unchanged ranges".to_owned(),
        ));
    }
    if !before.is_empty() {
        visit(before, after)?;
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static CHANGE_DIFF_COMPUTATIONS: Cell<usize> = const { Cell::new(0) };
    static OWNER_FRONTIER_VISITS: Cell<usize> = const { Cell::new(0) };
    static OWNER_ANCHOR_CANDIDATE_EVALUATIONS: Cell<usize> = const { Cell::new(0) };
    static OWNER_EXACT_SPAN_COMPARISONS: Cell<usize> = const { Cell::new(0) };
    static OWNER_EXACT_COMPARISON_WORK: Cell<usize> = const { Cell::new(0) };
    static OWNER_PHYSICAL_LINE_WORK: Cell<usize> = const { Cell::new(0) };
    static OWNER_COMMENT_SPAN_VISITS: Cell<usize> = const { Cell::new(0) };
}

pub(crate) fn analyze(
    before: SourceFile<'_>,
    after: SourceFile<'_>,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze_with_profile(before, after, AnalysisProfile::Full)
}

pub(crate) fn analyze_with_profile(
    before: SourceFile<'_>,
    after: SourceFile<'_>,
    profile: AnalysisProfile,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze_with_context(&AnalysisContext::default(), before, after, profile)
}

pub(crate) fn analyze_with_context(
    context: &AnalysisContext,
    before: SourceFile<'_>,
    after: SourceFile<'_>,
    profile: AnalysisProfile,
) -> Result<Vec<Finding>, AnalysisError> {
    if !languages::same_adapter(before.path, after.path)? {
        return Err(AnalysisError::AmbiguousChange(format!(
            "cannot attest a change across language adapters: {} -> {}",
            before.path.display(),
            after.path.display()
        )));
    }

    let before_document =
        languages::parse_validated_file_with_context(context, before.path, before.text)?;
    let after_document =
        languages::parse_validated_file_with_context(context, after.path, after.text)?;
    let identities = CanonicalOwnerIdentities::new(&before_document, &after_document)?;

    let anchors = LineAnchors::new(before.text, after.text, &before_document, &after_document)?;
    let owners = pair_owners(&before_document, &after_document, &anchors, &identities)?;
    let owner_changes =
        OwnerChangeIndex::new(&before_document, &after_document, &owners, &identities);
    let comments = pair_comments(&before_document, &after_document, &owners, &anchors)?;
    let mut findings = if profile.runs_static_policy() {
        let selection = semantic_selection(
            &before_document,
            &after_document,
            &owners,
            &comments,
            &owner_changes,
        );
        languages::analyze_file_with_context(context, after.path, after.text, &selection)?
    } else {
        Vec::new()
    };
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

struct CanonicalOwnerIdentities {
    before: Vec<CanonicalIdentity>,
    after: Vec<CanonicalIdentity>,
}

impl CanonicalOwnerIdentities {
    fn new(before: &ParsedFile, after: &ParsedFile) -> Result<Self, AnalysisError> {
        let mut interner = CanonicalIdentityInterner::with_capacity(
            before.identities.len() + after.identities.len(),
        );
        let before_map = interner.canonicalize(&before.identities)?;
        let after_map = interner.canonicalize(&after.identities)?;
        Ok(Self {
            before: resolve_owner_identities(before, &before_map)?,
            after: resolve_owner_identities(after, &after_map)?,
        })
    }
}

fn resolve_owner_identities(
    document: &ParsedFile,
    canonical: &CanonicalIdentityMap,
) -> Result<Vec<CanonicalIdentity>, AnalysisError> {
    document
        .owners
        .iter()
        .map(|owner| canonical.resolve(owner.identity))
        .collect()
}

struct OwnerChangeIndex {
    identity_path_changed: Vec<bool>,
}

impl OwnerChangeIndex {
    fn new(
        before: &ParsedFile,
        after: &ParsedFile,
        pairs: &Pairing,
        identities: &CanonicalOwnerIdentities,
    ) -> Self {
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
                    matches!(
                        before.owners[parent].kind,
                        OwnerKind::Type | OwnerKind::Member
                    ) && identity_path_changed[parent]
                });
                let structural_parent_changed =
                    old_owner.parent.is_some() && new_owner.parent != expected_after_parent;
                identity_path_changed[before_index] = parent_path_changed
                    || structural_parent_changed
                    || old_owner.kind != new_owner.kind
                    || identities.before[before_index].id != identities.after[after_index].id;
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
    identities: &CanonicalOwnerIdentities,
) -> Result<Pairing, AnalysisError> {
    let mut pairs = Pairing::new(before.owners.len(), after.owners.len());
    pairs.insert(0, 0)?;
    let before_children = owners_by_parent(before);
    let after_children = owners_by_parent(after);
    let mut frontier = VecDeque::from([(0, 0)]);
    let mut cross_parent_phase_complete = false;

    loop {
        while let Some((before_parent, after_parent)) = frontier.pop_front() {
            let before_siblings = &before_children[before_parent];
            let after_siblings = &after_children[after_parent];
            let stable = unique_stable_owner_pairs(
                before,
                after,
                before_siblings,
                after_siblings,
                &pairs,
                identities,
            );
            enqueue_owner_pairs(stable, &mut pairs, &mut frontier)?;

            let evidence = connected_owner_scores(
                before,
                after,
                before_siblings,
                after_siblings,
                anchors,
                &pairs,
            )?;
            if !evidence.exact_pairs.is_empty() {
                enqueue_owner_pairs(evidence.exact_pairs, &mut pairs, &mut frontier)?;
                let stable = unique_stable_owner_pairs(
                    before,
                    after,
                    before_siblings,
                    after_siblings,
                    &pairs,
                    identities,
                );
                enqueue_owner_pairs(stable, &mut pairs, &mut frontier)?;
            }

            let anchored = anchored_owner_pairs(&evidence.scores, &pairs);
            if anchored.is_empty() {
                continue;
            }
            enqueue_owner_pairs(anchored, &mut pairs, &mut frontier)?;

            let stable = unique_stable_owner_pairs(
                before,
                after,
                before_siblings,
                after_siblings,
                &pairs,
                identities,
            );
            enqueue_owner_pairs(stable, &mut pairs, &mut frontier)?;

            // A second anchor wave would make correspondence depend on the first wave's guesses.
            if !anchored_owner_pairs(&evidence.scores, &pairs).is_empty() {
                return Err(AnalysisError::AmbiguousChange(
                    "owner correspondence requires iterative anchor preference peeling".to_owned(),
                ));
            }
        }
        if cross_parent_phase_complete {
            break;
        }
        cross_parent_phase_complete = true;
        let moved = unique_exact_cross_parent_owner_pairs(before, after, &pairs, identities);
        enqueue_owner_pairs(moved, &mut pairs, &mut frontier)?;
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
    identities: &CanonicalOwnerIdentities,
) -> Vec<(usize, usize)> {
    let before_by_key = stable_owners_by_key(
        before,
        before_children.iter().copied(),
        &pairs.before_to_after,
        &identities.before,
    );
    let after_by_key = stable_owners_by_key(
        after,
        after_children.iter().copied(),
        &pairs.after_to_before,
        &identities.after,
    );

    before_by_key
        .into_iter()
        .filter_map(|(key, before_index)| {
            before_index.zip(after_by_key.get(&key).copied().flatten())
        })
        .collect()
}

fn unique_exact_cross_parent_owner_pairs(
    before: &ParsedFile,
    after: &ParsedFile,
    pairs: &Pairing,
    identities: &CanonicalOwnerIdentities,
) -> Vec<(usize, usize)> {
    let before_by_key = stable_owners_by_key(
        before,
        (1..before.owners.len()).filter(|index| pairs.before_to_after[*index].is_none()),
        &pairs.before_to_after,
        &identities.before,
    );
    let after_by_key = stable_owners_by_key(
        after,
        (1..after.owners.len()).filter(|index| pairs.after_to_before[*index].is_none()),
        &pairs.after_to_before,
        &identities.after,
    );

    before_by_key
        .into_iter()
        .filter_map(|(key, before_index)| {
            before_index
                .zip(after_by_key.get(&key).copied().flatten())
                .filter(|(before_index, after_index)| {
                    direct_owner_code_is_exact(
                        &before.owners[*before_index],
                        &after.owners[*after_index],
                    )
                })
        })
        .collect()
}

type StableOwnerKey = (OwnerKind, CanonicalIdentityId);

fn stable_owners_by_key(
    document: &ParsedFile,
    indexes: impl IntoIterator<Item = usize>,
    paired: &[Option<usize>],
    identities: &[CanonicalIdentity],
) -> BTreeMap<StableOwnerKey, Option<usize>> {
    let mut groups = BTreeMap::new();
    for index in indexes {
        #[cfg(test)]
        OWNER_FRONTIER_VISITS.with(|visits| visits.set(visits.get() + 1));
        let owner = &document.owners[index];
        let identity = identities[index];
        if paired[index].is_none() && has_stable_identity(identity) {
            groups
                .entry((owner.kind, identity.id))
                .and_modify(|unique| *unique = None)
                .or_insert(Some(index));
        }
    }
    groups
}

fn has_stable_identity(identity: CanonicalIdentity) -> bool {
    !identity.contains_placeholder
}

fn anchored_owner_pairs(scores: &[(usize, usize, usize)], pairs: &Pairing) -> Vec<(usize, usize)> {
    let remaining = || {
        scores.iter().copied().filter(|&(before, after, _)| {
            pairs.before_to_after[before].is_none() && pairs.after_to_before[after].is_none()
        })
    };
    let before_choices = owner_choices(remaining());
    let after_choices =
        owner_choices(remaining().map(|(before, after, score)| (after, before, score)));
    before_choices
        .into_iter()
        .filter(|(before_index, after_index)| after_choices.get(after_index) == Some(before_index))
        .collect()
}

fn connected_owner_scores(
    before: &ParsedFile,
    after: &ParsedFile,
    before_children: &[usize],
    after_children: &[usize],
    anchors: &LineAnchors,
    pairs: &Pairing,
) -> Result<OwnerAnchorEvidence, AnalysisError> {
    let mut before_sweep = OwnerRankSweep::new(
        before,
        before_children
            .iter()
            .copied()
            .filter(|index| pairs.before_to_after[*index].is_none()),
        |owner| anchors.before_owner_ranks(owner),
    );
    let mut after_sweep = OwnerRankSweep::new(
        after,
        after_children
            .iter()
            .copied()
            .filter(|index| pairs.after_to_before[*index].is_none()),
        |owner| anchors.after_owner_ranks(owner),
    );
    if before_sweep.is_empty() || after_sweep.is_empty() {
        return Ok(OwnerAnchorEvidence::default());
    }

    let mut boundaries: Vec<_> = before_sweep
        .boundaries()
        .chain(after_sweep.boundaries())
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();

    let mut scores = BTreeMap::new();
    let mut exact_pairs = ProvenOwnerPairs::default();
    for segment in boundaries.windows(2) {
        let rank = segment[0];
        let anchor_count = segment[1] - rank;
        let before_by_kind = owner_evidence_by_kind(before, before_sweep.owners_at(rank));
        let after_by_kind = owner_evidence_by_kind(after, after_sweep.owners_at(rank));
        for (kind, before_evidence) in before_by_kind {
            let Some(after_evidence) = after_by_kind.get(&kind) else {
                continue;
            };
            if let (OwnerEvidence::Unique(before_index), OwnerEvidence::Unique(after_index)) =
                (&before_evidence, after_evidence)
            {
                *scores.entry((*before_index, *after_index)).or_insert(0) += anchor_count;
                continue;
            }
            if anchor_count == 1
                && let Some(proven) = exact_same_line_owner_pairs(
                    before,
                    after,
                    anchors.owner[rank],
                    &before_evidence,
                    after_evidence,
                )
            {
                exact_pairs.extend(proven)?;
            } else {
                return Err(AnalysisError::AmbiguousChange(format!(
                    "exact line anchor {} connects multiple {kind:?} sibling owners",
                    anchors.owner[rank].before.index + 1
                )));
            }
        }
    }
    Ok(OwnerAnchorEvidence {
        scores: scores
            .into_iter()
            .map(|((before, after), score)| (before, after, score))
            .collect(),
        exact_pairs: exact_pairs.into_pairs(),
    })
}

#[derive(Default)]
struct ProvenOwnerPairs {
    pairs: Vec<(usize, usize)>,
    before_to_after: HashMap<usize, usize>,
    after_to_before: HashMap<usize, usize>,
}

impl ProvenOwnerPairs {
    fn extend(
        &mut self,
        pairs: impl IntoIterator<Item = (usize, usize)>,
    ) -> Result<(), AnalysisError> {
        for (before, after) in pairs {
            match (
                self.before_to_after.get(&before),
                self.after_to_before.get(&after),
            ) {
                (None, None) => {
                    self.before_to_after.insert(before, after);
                    self.after_to_before.insert(after, before);
                    self.pairs.push((before, after));
                }
                (Some(&paired_after), Some(&paired_before))
                    if paired_after == after && paired_before == before => {}
                _ => {
                    return Err(AnalysisError::AmbiguousChange(
                        "exact line proofs disagree on owner correspondence".to_owned(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn into_pairs(self) -> Vec<(usize, usize)> {
        self.pairs
    }
}

#[derive(Default)]
struct OwnerAnchorEvidence {
    scores: Vec<(usize, usize, usize)>,
    exact_pairs: Vec<(usize, usize)>,
}

enum OwnerEvidence {
    Unique(usize),
    Multiple(Vec<usize>),
}

impl OwnerEvidence {
    fn as_slice(&self) -> &[usize] {
        match self {
            Self::Unique(index) => std::slice::from_ref(index),
            Self::Multiple(indexes) => indexes,
        }
    }

    fn insert(&mut self, index: usize) {
        match self {
            Self::Unique(first) => *self = Self::Multiple(vec![*first, index]),
            Self::Multiple(indexes) => indexes.push(index),
        }
    }
}

fn owner_evidence_by_kind(
    document: &ParsedFile,
    candidates: impl IntoIterator<Item = usize>,
) -> BTreeMap<OwnerKind, OwnerEvidence> {
    let mut by_kind: BTreeMap<OwnerKind, OwnerEvidence> = BTreeMap::new();
    for index in candidates {
        #[cfg(test)]
        OWNER_ANCHOR_CANDIDATE_EVALUATIONS.with(|evaluations| {
            evaluations.set(evaluations.get() + 1);
        });
        by_kind
            .entry(document.owners[index].kind)
            .and_modify(|evidence| evidence.insert(index))
            .or_insert(OwnerEvidence::Unique(index));
    }
    by_kind
}

fn exact_same_line_owner_pairs(
    before: &ParsedFile,
    after: &ParsedFile,
    anchor: OwnerLineAnchor,
    before_evidence: &OwnerEvidence,
    after_evidence: &OwnerEvidence,
) -> Option<Vec<(usize, usize)>> {
    let before_indexes = before_evidence.as_slice();
    let after_indexes = after_evidence.as_slice();
    if before_indexes.len() != after_indexes.len() {
        return None;
    }
    let before_line = physical_line(anchor.before, anchor.content_len)?;
    let after_line = physical_line(anchor.after, anchor.content_len)?;
    if !owner_spans_are_disjoint(before, before_indexes)
        || !owner_spans_are_disjoint(after, after_indexes)
    {
        return None;
    }

    let mut proven = Vec::with_capacity(before_indexes.len());
    let mut exact_pair_count = 0;
    for (before_index, after_index) in before_indexes
        .iter()
        .copied()
        .zip(after_indexes.iter().copied())
    {
        #[cfg(test)]
        OWNER_EXACT_SPAN_COMPARISONS.with(|comparisons| {
            comparisons.set(comparisons.get() + 1);
        });
        let before_span = &before.owners[before_index].span;
        let after_span = &after.owners[after_index].span;
        let before_extent = owner_line_extent(before_span, before_line)?;
        let after_extent = owner_line_extent(after_span, after_line)?;
        if before_extent != after_extent {
            return None;
        }
        exact_pair_count += usize::from(direct_owner_code_is_exact(
            &before.owners[before_index],
            &after.owners[after_index],
        ));
        proven.push((before_index, after_index));
    }
    let non_exact_pair_count = proven.len() - exact_pair_count;
    // One changed pair is determined by eliminating its already-proven exact siblings.
    (exact_pair_count > 0 && non_exact_pair_count <= 1).then_some(proven)
}

fn direct_owner_code_is_exact(before: &OwnerSnapshot, after: &OwnerSnapshot) -> bool {
    // Nested owners and comments have separate correspondence passes, so only direct code belongs here.
    if before.code.len() != after.code.len() {
        return false;
    }
    before.code.iter().zip(&after.code).all(|(old, new)| {
        #[cfg(test)]
        OWNER_EXACT_COMPARISON_WORK.with(|work| work.set(work.get() + 1));
        old == new
    })
}

fn owner_spans_are_disjoint(document: &ParsedFile, indexes: &[usize]) -> bool {
    indexes.windows(2).all(|pair| {
        document.owners[pair[0]].span.end_byte <= document.owners[pair[1]].span.start_byte
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OwnerLineExtent {
    line_start: usize,
    line_end: usize,
    owner_start: usize,
    owner_end: usize,
}

fn owner_line_extent(span: &Span, line: PhysicalLine) -> Option<OwnerLineExtent> {
    let intersection_start = span.start_byte.max(line.coordinate.start_byte);
    let intersection_end = span.end_byte.min(line.end_byte);
    (intersection_start < intersection_end).then_some(OwnerLineExtent {
        line_start: intersection_start.checked_sub(line.coordinate.start_byte)?,
        line_end: intersection_end.checked_sub(line.coordinate.start_byte)?,
        owner_start: intersection_start.checked_sub(span.start_byte)?,
        owner_end: intersection_end.checked_sub(span.start_byte)?,
    })
}

struct RankedOwner {
    index: usize,
    start_rank: usize,
    end_rank: usize,
}

struct OwnerRankSweep {
    owners: Vec<RankedOwner>,
    first_active: usize,
    past_active: usize,
    previous_rank: Option<usize>,
}

impl OwnerRankSweep {
    fn new(
        document: &ParsedFile,
        owners: impl IntoIterator<Item = usize>,
        rank_range: impl Fn(&OwnerSnapshot) -> std::ops::Range<usize>,
    ) -> Self {
        let owners = owners
            .into_iter()
            .filter_map(|index| {
                let ranks = rank_range(&document.owners[index]);
                (ranks.start < ranks.end).then_some(RankedOwner {
                    index,
                    start_rank: ranks.start,
                    end_rank: ranks.end,
                })
            })
            .collect();
        Self {
            owners,
            first_active: 0,
            past_active: 0,
            previous_rank: None,
        }
    }

    fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }

    fn boundaries(&self) -> impl Iterator<Item = usize> + '_ {
        self.owners
            .iter()
            .flat_map(|owner| [owner.start_rank, owner.end_rank])
    }

    fn owners_at(&mut self, rank: usize) -> impl Iterator<Item = usize> + '_ {
        debug_assert!(self.previous_rank.is_none_or(|previous| previous <= rank));
        while self.first_active < self.owners.len()
            && self.owners[self.first_active].end_rank <= rank
        {
            self.first_active += 1;
        }
        self.past_active = self.past_active.max(self.first_active);
        while self.past_active < self.owners.len()
            && self.owners[self.past_active].start_rank <= rank
        {
            self.past_active += 1;
        }
        self.previous_rank = Some(rank);
        self.owners[self.first_active..self.past_active]
            .iter()
            .map(|owner| owner.index)
    }
}

fn owner_choices(
    candidates: impl Iterator<Item = (usize, usize, usize)>,
) -> BTreeMap<usize, usize> {
    let mut by_owner: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for (owner, candidate, score) in candidates {
        by_owner.entry(owner).or_default().push((candidate, score));
    }
    by_owner
        .into_iter()
        .filter_map(|(owner, candidates)| {
            unique_best_choice(candidates.into_iter()).map(|candidate| (owner, candidate))
        })
        .collect()
}

fn unique_best_choice(candidates: impl Iterator<Item = (usize, usize)>) -> Option<usize> {
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
        return None;
    }
    best.map(|(index, _)| index)
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
        let local_pairs = if key.1.is_some() {
            align_comment_group(
                &old_indexes,
                new_indexes,
                &before.comments,
                &after.comments,
                anchors,
            )?
        } else {
            old_indexes
                .iter()
                .copied()
                .zip(new_indexes.iter().copied())
                .collect()
        };
        for (old_index, new_index) in local_pairs {
            pairs.insert(old_index, new_index)?;
        }
    }

    let old_leftovers = group_comments(&before.comments, |index, comment| {
        if pairs.before_to_after[index].is_none() {
            attestation_key(&comment.text)
        } else {
            None
        }
    });
    let new_leftovers = group_comments(&after.comments, |index, comment| {
        if pairs.after_to_before[index].is_none() {
            attestation_key(&comment.text)
        } else {
            None
        }
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

fn attestation_key(comment: &str) -> Option<AttestationKey> {
    AttestationKey::from_comment(comment)
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
        exact_comment_sequence_pairs(&old_remaining, &new_remaining, old_comments, new_comments)?
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
) -> Result<Vec<(usize, usize)>, AnalysisError> {
    let computed = compute_change_diff(
        old_indexes
            .iter()
            .map(|index| old_comments[*index].text.as_str()),
        new_indexes
            .iter()
            .map(|index| new_comments[*index].text.as_str()),
    )?;
    let mut pairs = Vec::new();
    visit_equal_ranges(
        &computed.diff,
        computed.input.before.len(),
        computed.input.after.len(),
        |old_range, new_range| {
            for (old_position, new_position) in old_range.zip(new_range) {
                pairs.push((old_indexes[old_position], new_indexes[new_position]));
            }
            Ok(())
        },
    )?;
    Ok(pairs)
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
            && sizes_parent_budget(before.owners[before_index].kind)
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
                if old_comment.classification != new_comment.classification
                    || old_comment.text != new_comment.text
                {
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
        if sizes_parent_budget(owner.kind) {
            let mut parent = owner.parent;
            while let Some(parent_index) = parent {
                let parent_owner = &after.owners[parent_index];
                if is_scoped_budget_owner(parent_owner.kind) {
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

fn is_scoped_budget_owner(kind: OwnerKind) -> bool {
    matches!(kind, OwnerKind::Function | OwnerKind::Type)
}

fn sizes_parent_budget(kind: OwnerKind) -> bool {
    matches!(kind, OwnerKind::Leaf | OwnerKind::Member)
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
        if attestation_key(&old_comment.text).is_none() {
            continue;
        }
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
            || old_comment.classification != new_comment.classification
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
        OwnerKind::Member => format!("member `{}`", owner.name),
        OwnerKind::Template => "template owner".to_owned(),
        OwnerKind::TomlKey => format!("TOML key `{}`", owner.name),
    }
}

struct LineAnchors {
    exact: Vec<ExactLineAnchor>,
    owner: Vec<OwnerLineAnchor>,
}

#[derive(Clone, Copy)]
struct ExactLineAnchor {
    before: usize,
    after: usize,
}

#[derive(Clone, Copy)]
struct OwnerLineAnchor {
    before: LineCoordinate,
    after: LineCoordinate,
    content_len: usize,
}

#[derive(Clone, Copy)]
struct LineCoordinate {
    index: usize,
    start_byte: usize,
}

#[derive(Clone, Copy)]
struct PhysicalLine {
    coordinate: LineCoordinate,
    end_byte: usize,
}

impl LineAnchors {
    fn new(
        before: &str,
        after: &str,
        old: &ParsedFile,
        new: &ParsedFile,
    ) -> Result<Self, AnalysisError> {
        let ChangeDiff { input, diff } =
            compute_change_diff(sources::lines(before), sources::lines(after))?;
        let mut old_comments = CommentSpanSweep::new(&old.comments);
        let mut new_comments = CommentSpanSweep::new(&new.comments);
        let mut exact_targets: Vec<_> = old
            .comments
            .iter()
            .map(|comment| comment.span.start_line.saturating_sub(1))
            .collect();
        exact_targets.sort_unstable();
        exact_targets.dedup();
        let mut next_exact_target = 0;
        let mut exact = Vec::with_capacity(exact_targets.len());
        let mut owner = Vec::new();
        let mut before_lines = DiffLineCursor::new(&input.before, &input.interner);
        let mut after_lines = DiffLineCursor::new(&input.after, &input.interner);
        visit_equal_ranges(
            &diff,
            input.before.len(),
            input.after.len(),
            |before_range, after_range| {
                before_lines.advance_to(before_range.start)?;
                after_lines.advance_to(after_range.start)?;
                for _ in 0..before_range.len() {
                    let (before_line, before_text) = before_lines.next_line()?;
                    let (after_line, after_text) = after_lines.next_line()?;
                    if before_text != after_text {
                        return Err(AnalysisError::Invariant(
                            "diff engine marked unequal lines as unchanged".to_owned(),
                        ));
                    }

                    while exact_targets
                        .get(next_exact_target)
                        .is_some_and(|target| *target < before_line.index)
                    {
                        next_exact_target += 1;
                    }
                    if exact_targets.get(next_exact_target) == Some(&before_line.index) {
                        exact.push(ExactLineAnchor {
                            before: before_line.index,
                            after: after_line.index,
                        });
                        next_exact_target += 1;
                    }

                    let content = line_content(before_text);
                    if old_comments.has_non_comment_alphanumeric(before_line.start_byte, content)
                        && new_comments.has_non_comment_alphanumeric(after_line.start_byte, content)
                    {
                        owner.push(OwnerLineAnchor {
                            before: before_line,
                            after: after_line,
                            content_len: content.len(),
                        });
                    }
                }
                Ok(())
            },
        )?;
        debug_assert!(owner.windows(2).all(|pair| {
            pair[0].before.index < pair[1].before.index && pair[0].after.index < pair[1].after.index
        }));
        Ok(Self { exact, owner })
    }

    fn before_owner_ranks(&self, owner: &OwnerSnapshot) -> std::ops::Range<usize> {
        let start_line = owner.span.start_line.saturating_sub(1);
        let end_line = owner.span.end_line;
        self.owner
            .partition_point(|anchor| anchor.before.index < start_line)
            ..self
                .owner
                .partition_point(|anchor| anchor.before.index < end_line)
    }

    fn after_owner_ranks(&self, owner: &OwnerSnapshot) -> std::ops::Range<usize> {
        let start_line = owner.span.start_line.saturating_sub(1);
        let end_line = owner.span.end_line;
        self.owner
            .partition_point(|anchor| anchor.after.index < start_line)
            ..self
                .owner
                .partition_point(|anchor| anchor.after.index < end_line)
    }

    fn after_line(&self, one_based_old_line: usize) -> Option<usize> {
        self.exact
            .binary_search_by_key(&one_based_old_line.saturating_sub(1), |anchor| {
                anchor.before
            })
            .ok()
            .map(|index| self.exact[index].after + 1)
    }
}

struct DiffLineCursor<'input, 'source> {
    tokens: &'input [Token],
    interner: &'input Interner<&'source str>,
    position: usize,
    start_byte: usize,
}

impl<'input, 'source> DiffLineCursor<'input, 'source> {
    fn new(tokens: &'input [Token], interner: &'input Interner<&'source str>) -> Self {
        Self {
            tokens,
            interner,
            position: 0,
            start_byte: 0,
        }
    }

    fn advance_to(&mut self, target: usize) -> Result<(), AnalysisError> {
        if target < self.position || target > self.tokens.len() {
            return Err(AnalysisError::Invariant(
                "diff engine line ranges were not monotonic".to_owned(),
            ));
        }
        while self.position < target {
            self.next_line()?;
        }
        Ok(())
    }

    fn next_line(&mut self) -> Result<(LineCoordinate, &'source str), AnalysisError> {
        let token = self.tokens.get(self.position).copied().ok_or_else(|| {
            AnalysisError::Invariant("diff engine line range exceeded its input".to_owned())
        })?;
        let line = self.interner[token];
        let coordinate = LineCoordinate {
            index: self.position,
            start_byte: self.start_byte,
        };
        self.start_byte = self.start_byte.checked_add(line.len()).ok_or_else(|| {
            AnalysisError::Invariant("diff line byte offset exceeded the platform limit".to_owned())
        })?;
        self.position += 1;
        Ok((coordinate, line))
    }
}

struct CommentSpanSweep {
    coverage: Vec<std::ops::Range<usize>>,
    first_relevant: usize,
    previous_line_start: Option<usize>,
}

impl CommentSpanSweep {
    fn new(comments: &[CommentSnapshot]) -> Self {
        let mut spans: Vec<_> = comments
            .iter()
            .map(|comment| comment.span.start_byte..comment.span.end_byte)
            .collect();
        spans.sort_unstable_by_key(|span| (span.start, span.end));

        let mut coverage: Vec<std::ops::Range<usize>> = Vec::with_capacity(spans.len());
        for span in spans {
            debug_assert!(span.start <= span.end);
            if let Some(previous) = coverage.last_mut()
                && span.start <= previous.end
            {
                previous.end = previous.end.max(span.end);
            } else {
                coverage.push(span);
            }
        }
        Self {
            coverage,
            first_relevant: 0,
            previous_line_start: None,
        }
    }

    fn has_non_comment_alphanumeric(&mut self, line_start: usize, content: &str) -> bool {
        debug_assert!(
            self.previous_line_start
                .is_none_or(|start| start <= line_start)
        );
        self.previous_line_start = Some(line_start);
        let line_end = line_start + content.len();
        while self
            .coverage
            .get(self.first_relevant)
            .is_some_and(|span| span.end <= line_start)
        {
            self.first_relevant += 1;
        }

        let mut uncovered_start = 0;
        for span in &self.coverage[self.first_relevant..] {
            if span.start >= line_end {
                break;
            }
            #[cfg(test)]
            OWNER_COMMENT_SPAN_VISITS.with(|visits| visits.set(visits.get() + 1));
            let covered_start = span.start.max(line_start) - line_start;
            if uncovered_start < covered_start
                && content[uncovered_start..covered_start]
                    .chars()
                    .any(char::is_alphanumeric)
            {
                return true;
            }
            uncovered_start = span.end.min(line_end) - line_start;
        }
        content[uncovered_start..]
            .chars()
            .any(char::is_alphanumeric)
    }
}

fn line_content(line: &str) -> &str {
    line.strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\r'))
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line)
}

fn physical_line(coordinate: LineCoordinate, content_len: usize) -> Option<PhysicalLine> {
    #[cfg(test)]
    OWNER_PHYSICAL_LINE_WORK.with(|work| work.set(work.get() + 1));
    Some(PhysicalLine {
        coordinate,
        end_byte: coordinate.start_byte.checked_add(content_len)?,
    })
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::path::Path;

    use super::*;
    use crate::policy::CommentClassification;

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

    fn same_line_owner_pairing_work(count: usize) -> (usize, usize) {
        let before = same_line_anonymous_callbacks(count, 1);
        let after = same_line_anonymous_callbacks(count, 2);
        OWNER_ANCHOR_CANDIDATE_EVALUATIONS.with(|evaluations| evaluations.set(0));
        OWNER_EXACT_SPAN_COMPARISONS.with(|comparisons| comparisons.set(0));

        analyze(
            SourceFile {
                path: Path::new("callbacks.js"),
                text: &before,
            },
            SourceFile {
                path: Path::new("callbacks.js"),
                text: &after,
            },
        )
        .expect("identical ordered sub-line spans prove anonymous sibling correspondence");

        (
            OWNER_ANCHOR_CANDIDATE_EVALUATIONS.with(Cell::get),
            OWNER_EXACT_SPAN_COMPARISONS.with(Cell::get),
        )
    }

    fn deeply_nested_anonymous_callbacks(depth: usize, version: usize) -> String {
        let mut source = format!("const VERSION = {version};\n");
        for level in 0..depth {
            writeln!(source, "(() => {{ stable_{level:08}();")
                .expect("writing to a String cannot fail");
        }
        source.push_str("0\n");
        for _ in 0..depth {
            source.push_str("})();\n");
        }
        source
    }

    fn nested_same_line_owners(depth: usize, version: usize) -> String {
        let mut nested = "value()".to_owned();
        for index in 0..depth {
            nested = format!("use_two(|| {{ {nested}; }}, || {{ stable_{index}(); }})");
        }
        format!("const VERSION: usize = {version};\nfn work() {{ {nested}; }}\n")
    }

    fn nested_same_line_owner_proof_work(depth: usize) -> (usize, usize) {
        let before = nested_same_line_owners(depth, 1);
        let after = nested_same_line_owners(depth, 2);
        OWNER_EXACT_COMPARISON_WORK.with(|work| work.set(0));
        OWNER_PHYSICAL_LINE_WORK.with(|work| work.set(0));

        analyze(
            SourceFile {
                path: Path::new("src/lib.rs"),
                text: &before,
            },
            SourceFile {
                path: Path::new("src/lib.rs"),
                text: &after,
            },
        )
        .expect("the exact nested owner line proves every sibling correspondence");

        (
            OWNER_EXACT_COMPARISON_WORK.with(Cell::get),
            OWNER_PHYSICAL_LINE_WORK.with(Cell::get),
        )
    }

    fn nested_anonymous_owner_anchor_evaluations(depth: usize) -> usize {
        let before = deeply_nested_anonymous_callbacks(depth, 1);
        let after = deeply_nested_anonymous_callbacks(depth, 2);
        OWNER_ANCHOR_CANDIDATE_EVALUATIONS.with(|evaluations| evaluations.set(0));

        analyze(
            SourceFile {
                path: Path::new("nested.js"),
                text: &before,
            },
            SourceFile {
                path: Path::new("nested.js"),
                text: &after,
            },
        )
        .expect("nested anonymous owners pair by exact anchors");

        OWNER_ANCHOR_CANDIDATE_EVALUATIONS.with(Cell::get)
    }

    fn mixed_comment_line_anchor_work(count: usize) -> usize {
        let mut source = String::from("fn work() {\n");
        for index in 0..count {
            writeln!(source, "    consume({index}); // rationale {index}")
                .expect("writing to a String cannot fail");
        }
        source.push_str("}\n");
        let before = languages::parse_file(Path::new("src/lib.rs"), &source)
            .expect("the sweep fixture is valid Rust");
        let after = before.clone();
        OWNER_COMMENT_SPAN_VISITS.with(|visits| visits.set(0));

        let _anchors = LineAnchors::new(&source, &source, &before, &after)
            .expect("the identical comment-line fixture must diff");

        OWNER_COMMENT_SPAN_VISITS.with(Cell::get)
    }

    fn regrouped_anonymous_callbacks(count: usize) -> (String, String) {
        fn emit_callbacks(
            source: &mut String,
            statements: &mut impl Iterator<Item = usize>,
            count: usize,
        ) {
            source.push_str("(() => {\n");
            for statement in statements.take(count) {
                writeln!(source, "  statement_{statement:08}();")
                    .expect("writing to a String cannot fail");
            }
            source.push_str("})();\n");
        }

        let statement_count = 2 * count * count - count;
        let mut before = String::new();
        let mut before_statements = 0..statement_count;
        for index in 0..count - 1 {
            emit_callbacks(
                &mut before,
                &mut before_statements,
                (2 * index + 1) + (2 * index + 2),
            );
        }
        emit_callbacks(&mut before, &mut before_statements, 2 * (count - 1) + 1);

        let mut after = String::new();
        let mut after_statements = 0..statement_count;
        emit_callbacks(&mut after, &mut after_statements, 1);
        for index in 1..count {
            emit_callbacks(
                &mut after,
                &mut after_statements,
                2 * index + (2 * index + 1),
            );
        }
        assert_eq!(before_statements.next(), None);
        assert_eq!(after_statements.next(), None);
        (before, after)
    }

    fn regrouped_owner_pairing_work(count: usize) -> (usize, usize) {
        let (before, after) = regrouped_anonymous_callbacks(count);
        OWNER_FRONTIER_VISITS.with(|visits| visits.set(0));
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
        .expect_err("iterative preference peeling is not proof of owner correspondence");
        assert!(matches!(error, AnalysisError::AmbiguousChange(_)));

        (
            OWNER_FRONTIER_VISITS.with(Cell::get),
            OWNER_ANCHOR_CANDIDATE_EVALUATIONS.with(Cell::get),
        )
    }

    #[test]
    fn owner_line_anchors_store_two_coordinates_and_one_content_length() {
        assert_eq!(
            std::mem::size_of::<OwnerLineAnchor>(),
            5 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn exact_line_anchors_retain_only_before_comment_lines() {
        let mut source = String::from("fn work() {\n");
        for statement in 0..256 {
            if statement == 128 {
                source.push_str("    // Coupled to the following operation.\n");
            }
            writeln!(source, "    operation_{statement:03}();")
                .expect("writing to a String cannot fail");
        }
        source.push_str("}\n");
        let before = languages::parse_file(Path::new("src/lib.rs"), &source)
            .expect("the sparse-anchor fixture is valid Rust");
        let after = before.clone();

        let anchors = LineAnchors::new(&source, &source, &before, &after)
            .expect("identical source must diff");

        assert_eq!(anchors.exact.len(), before.comments.len());
    }

    #[test]
    fn diff_capacity_returns_a_typed_error_before_compute() {
        let error = check_diff_capacity("after", i32::MAX as usize)
            .expect_err("the upstream token boundary must be rejected without panicking");

        assert!(matches!(
            error,
            AnalysisError::DiffCapacity {
                snapshot: "after",
                tokens,
                maximum: MAX_DIFF_TOKENS,
            } if tokens == i32::MAX as usize
        ));
    }

    #[test]
    fn change_diff_algorithm_is_myers() {
        assert_eq!(CHANGE_DIFF_ALGORITHM, Algorithm::Myers);
    }

    #[test]
    fn line_and_comment_alignment_share_one_diff_engine() {
        const SOURCE: &str = concat!(
            "fn work() {\n",
            "    // Coupled to the operation.\n",
            "    operation();\n",
            "}\n",
        );
        let before = languages::parse_file(Path::new("src/lib.rs"), SOURCE)
            .expect("the shared-engine fixture is valid Rust");
        let after = before.clone();
        CHANGE_DIFF_COMPUTATIONS.with(|computations| computations.set(0));

        LineAnchors::new(SOURCE, SOURCE, &before, &after)
            .expect("line alignment must use the shared engine");
        exact_comment_sequence_pairs(&[0], &[0], &before.comments, &after.comments)
            .expect("comment alignment must use the shared engine");

        assert_eq!(CHANGE_DIFF_COMPUTATIONS.with(Cell::get), 2);
    }

    #[test]
    fn exact_same_line_owner_proof_rejects_incomplete_evidence() {
        const SOURCE: &str = "(() => { stable0(); })();(() => { stable1(); })();\n";
        let before = languages::parse_file(Path::new("callbacks.js"), SOURCE)
            .expect("the proof fixture is valid JavaScript");
        let after = before.clone();
        let evidence = OwnerEvidence::Multiple(vec![1, 2]);
        let one_owner = OwnerEvidence::Unique(1);
        let line = LineCoordinate {
            index: 0,
            start_byte: 0,
        };
        let anchor = OwnerLineAnchor {
            before: line,
            after: line,
            content_len: line_content(SOURCE).len(),
        };

        let mut shifted_after = after.clone();
        shifted_after.owners[2].span.start_byte += 1;
        let mut overlapping_before = before.clone();
        let mut overlapping_after = after.clone();
        let overlap_end = overlapping_before.owners[2].span.start_byte + 1;
        overlapping_before.owners[1].span.end_byte = overlap_end;
        overlapping_after.owners[1].span.end_byte = overlap_end;
        let mut two_changed_after = after.clone();
        two_changed_after.owners[1].code.clear();
        two_changed_after.owners[2].code.clear();

        let rejected = [
            exact_same_line_owner_pairs(&before, &after, anchor, &evidence, &one_owner),
            exact_same_line_owner_pairs(&before, &shifted_after, anchor, &evidence, &evidence),
            exact_same_line_owner_pairs(&before, &two_changed_after, anchor, &evidence, &evidence),
            exact_same_line_owner_pairs(
                &overlapping_before,
                &overlapping_after,
                anchor,
                &evidence,
                &evidence,
            ),
        ];

        assert!(
            rejected.iter().all(Option::is_none),
            "cardinality, range, direct-code, and overlap mismatches are not correspondence proof"
        );
    }

    #[test]
    fn exact_owner_pair_deduplication_rejects_conflicting_proofs() {
        let mut repeated = ProvenOwnerPairs::default();
        repeated
            .extend([(1, 2), (1, 2)])
            .expect("the same proof may arise at both boundaries of one owner");
        assert_eq!(repeated.into_pairs(), [(1, 2)]);

        let mut conflicting_after = ProvenOwnerPairs::default();
        conflicting_after
            .extend([(1, 2)])
            .expect("the initial proof is valid");
        let after_error = conflicting_after
            .extend([(1, 3)])
            .expect_err("one old owner cannot map to two new owners");
        assert!(matches!(after_error, AnalysisError::AmbiguousChange(_)));

        let mut conflicting_before = ProvenOwnerPairs::default();
        conflicting_before
            .extend([(1, 2)])
            .expect("the initial proof is valid");
        let before_error = conflicting_before
            .extend([(3, 2)])
            .expect_err("two old owners cannot map to one new owner");
        assert!(matches!(before_error, AnalysisError::AmbiguousChange(_)));
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
    fn same_line_anonymous_owner_pairing_is_linear() {
        for count in [64, 128, 256] {
            assert_eq!(same_line_owner_pairing_work(count), (2 * count, count));
        }
    }

    #[test]
    fn nested_same_line_owner_proof_work_is_linear() {
        let work = [32, 64, 128, 256].map(nested_same_line_owner_proof_work);

        assert!(work[0].0 > 0, "the test must observe exact-content work");
        assert!(work[0].1 > 0, "the test must observe physical-line work");
        for pair in work.windows(2) {
            assert!(
                pair[1].0 <= 2 * pair[0].0 + 64,
                "exact-content work grew superlinearly: {work:?}"
            );
            assert!(
                pair[1].1 <= 2 * pair[0].1 + 64,
                "physical-line work grew superlinearly: {work:?}"
            );
        }
    }

    #[test]
    fn deep_anonymous_owner_anchor_candidates_are_evaluated_linearly() {
        let evaluations = [10, 20, 40, 80].map(nested_anonymous_owner_anchor_evaluations);

        assert_eq!(evaluations, [20, 40, 80, 160]);
    }

    #[test]
    fn mixed_line_comment_spans_are_swept_once_per_snapshot() {
        for count in [64, 128, 256, 512] {
            assert_eq!(mixed_comment_line_anchor_work(count), 2 * count);
        }
    }

    #[test]
    fn comment_span_sweep_unions_out_of_order_container_coverage() {
        let comments =
            [(4, 8), (0, 6), (13, 17), (9, 15)].map(|(start_byte, end_byte)| CommentSnapshot {
                classification: CommentClassification::narrative(),
                text: "container comment".to_owned(),
                span: Span {
                    start_byte,
                    end_byte,
                    start_line: 1,
                    end_line: 1,
                },
                owner: 0,
            });
        let mut sweep = CommentSpanSweep::new(&comments);
        OWNER_COMMENT_SPAN_VISITS.with(|visits| visits.set(0));

        assert!(!sweep.has_non_comment_alphanumeric(0, "abcdefgh-ijklmnop"));
        assert_eq!(OWNER_COMMENT_SPAN_VISITS.with(Cell::get), 2);
    }

    #[test]
    fn regrouped_anonymous_owner_graph_is_built_once() {
        for count in [10, 20, 40] {
            let (frontier_visits, anchor_evaluations) = regrouped_owner_pairing_work(count);
            let statement_count = 2 * count * count - count;

            assert_eq!(frontier_visits, 4 * count);
            assert!(
                anchor_evaluations <= 2 * statement_count,
                "each exact statement line can inspect at most one owner per snapshot"
            );
        }
    }
}
