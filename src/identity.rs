use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::model::AnalysisError;

const PLACEHOLDERS: [&str; 3] = ["<anonymous>", "<destructured>", "<unknown>"];

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct IdentityId(usize);

#[derive(Clone, Debug, Default)]
pub(crate) struct IdentityArena {
    entries: Vec<IdentityEntry>,
}

#[derive(Clone, Debug)]
struct IdentityEntry {
    parent: Option<IdentityId>,
    segment: String,
    contains_placeholder: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct CanonicalIdentityId(usize);

#[derive(Clone, Copy, Debug)]
pub(crate) struct CanonicalIdentity {
    pub(crate) id: CanonicalIdentityId,
    pub(crate) contains_placeholder: bool,
}

#[derive(Debug)]
pub(crate) struct CanonicalIdentityMap {
    entries: Vec<CanonicalIdentity>,
}

#[derive(Default)]
pub(crate) struct CanonicalIdentityInterner {
    entries: HashMap<CanonicalIdentityKey, CanonicalIdentityId>,
}

#[derive(Eq, PartialEq)]
struct CanonicalIdentityKey {
    parent: Option<CanonicalIdentityId>,
    segment: String,
}

impl Hash for CanonicalIdentityKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        record_canonical_identity_hash_probe();
        self.parent.hash(state);
        self.segment.hash(state);
    }
}

impl IdentityArena {
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn push(
        &mut self,
        parent: Option<IdentityId>,
        segment: String,
    ) -> Result<IdentityId, AnalysisError> {
        let parent_contains_placeholder = parent
            .map(|parent| {
                self.entries
                    .get(parent.0)
                    .map(|entry| entry.contains_placeholder)
                    .ok_or_else(|| missing_identity(parent))
            })
            .transpose()?
            .unwrap_or(false);
        let contains_placeholder = parent_contains_placeholder || segment_has_placeholder(&segment);
        let id = IdentityId(self.entries.len());
        self.entries.push(IdentityEntry {
            parent,
            segment,
            contains_placeholder,
        });
        record_identity_arena_node();
        Ok(id)
    }

    pub(crate) fn push_path<'segment>(
        &mut self,
        segments: impl IntoIterator<Item = &'segment str>,
    ) -> Result<IdentityId, AnalysisError> {
        let mut identity = None;
        for segment in segments {
            identity = Some(self.push(identity, segment.to_owned())?);
        }
        identity.ok_or_else(|| {
            AnalysisError::Invariant("owner identity path has no segments".to_owned())
        })
    }

    pub(crate) fn import(&mut self, source: Self) -> Result<Vec<IdentityId>, AnalysisError> {
        let source_len = source.entries.len();
        let mut imported = Vec::with_capacity(source_len);
        for (index, entry) in source.entries.into_iter().enumerate() {
            let parent = remap_parent(entry.parent, index, source_len, &imported)?;
            let expected_placeholder = parent
                .map(|parent| self.entries[parent.0].contains_placeholder)
                .unwrap_or(false)
                || segment_has_placeholder(&entry.segment);
            if expected_placeholder != entry.contains_placeholder {
                return Err(AnalysisError::Invariant(
                    "identity arena has an inconsistent placeholder cache".to_owned(),
                ));
            }
            imported.push(self.push(parent, entry.segment)?);
        }
        Ok(imported)
    }

    pub(crate) fn remap(
        mapping: &[IdentityId],
        identity: IdentityId,
    ) -> Result<IdentityId, AnalysisError> {
        mapping
            .get(identity.0)
            .copied()
            .ok_or_else(|| missing_identity(identity))
    }
}

impl CanonicalIdentityInterner {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
        }
    }

    pub(crate) fn canonicalize(
        &mut self,
        arena: &IdentityArena,
    ) -> Result<CanonicalIdentityMap, AnalysisError> {
        let mut canonical = Vec::with_capacity(arena.entries.len());
        for (index, entry) in arena.entries.iter().enumerate() {
            let parent = canonical_parent(entry.parent, index, arena.entries.len(), &canonical)?;
            let expected_placeholder = parent
                .map(|parent| parent.contains_placeholder)
                .unwrap_or(false)
                || segment_has_placeholder(&entry.segment);
            if expected_placeholder != entry.contains_placeholder {
                return Err(AnalysisError::Invariant(
                    "identity arena has an inconsistent placeholder cache".to_owned(),
                ));
            }
            let key = CanonicalIdentityKey {
                parent: parent.map(|parent| parent.id),
                segment: entry.segment.clone(),
            };
            let next = CanonicalIdentityId(self.entries.len());
            let canonical_id = *self.entries.entry(key).or_insert_with(|| {
                record_canonical_identity_node();
                next
            });
            canonical.push(CanonicalIdentity {
                id: canonical_id,
                contains_placeholder: entry.contains_placeholder,
            });
            record_canonical_identity_visit();
        }
        Ok(CanonicalIdentityMap { entries: canonical })
    }
}

impl CanonicalIdentityMap {
    pub(crate) fn resolve(&self, identity: IdentityId) -> Result<CanonicalIdentity, AnalysisError> {
        self.entries
            .get(identity.0)
            .copied()
            .ok_or_else(|| missing_identity(identity))
    }
}

fn remap_parent(
    parent: Option<IdentityId>,
    index: usize,
    arena_len: usize,
    imported: &[IdentityId],
) -> Result<Option<IdentityId>, AnalysisError> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    validate_parent(parent, index, arena_len)?;
    imported
        .get(parent.0)
        .copied()
        .map(Some)
        .ok_or_else(|| missing_identity(parent))
}

fn canonical_parent(
    parent: Option<IdentityId>,
    index: usize,
    arena_len: usize,
    canonical: &[CanonicalIdentity],
) -> Result<Option<CanonicalIdentity>, AnalysisError> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    validate_parent(parent, index, arena_len)?;
    canonical
        .get(parent.0)
        .copied()
        .map(Some)
        .ok_or_else(|| missing_identity(parent))
}

fn validate_parent(
    parent: IdentityId,
    child_index: usize,
    arena_len: usize,
) -> Result<(), AnalysisError> {
    if parent.0 >= arena_len {
        return Err(missing_identity(parent));
    }
    if parent.0 >= child_index {
        return Err(AnalysisError::Invariant(
            "identity arena contains a parent cycle or non-parent-first edge".to_owned(),
        ));
    }
    Ok(())
}

fn missing_identity(identity: IdentityId) -> AnalysisError {
    AnalysisError::Invariant(format!("identity id {} is outside its arena", identity.0))
}

fn segment_has_placeholder(segment: &str) -> bool {
    PLACEHOLDERS
        .iter()
        .any(|placeholder| segment.contains(placeholder))
}

#[cfg(test)]
thread_local! {
    static IDENTITY_ARENA_NODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CANONICAL_IDENTITY_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CANONICAL_IDENTITY_NODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CANONICAL_IDENTITY_HASH_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_identity_arena_node() {
    IDENTITY_ARENA_NODES.with(|nodes| nodes.set(nodes.get() + 1));
}

#[cfg(not(test))]
fn record_identity_arena_node() {}

#[cfg(test)]
fn record_canonical_identity_visit() {
    CANONICAL_IDENTITY_VISITS.with(|visits| visits.set(visits.get() + 1));
}

#[cfg(not(test))]
fn record_canonical_identity_visit() {}

#[cfg(test)]
fn record_canonical_identity_node() {
    CANONICAL_IDENTITY_NODES.with(|nodes| nodes.set(nodes.get() + 1));
}

#[cfg(not(test))]
fn record_canonical_identity_node() {}

#[cfg(test)]
fn record_canonical_identity_hash_probe() {
    CANONICAL_IDENTITY_HASH_PROBES.with(|probes| probes.set(probes.get() + 1));
}

#[cfg(not(test))]
fn record_canonical_identity_hash_probe() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_preserves_placeholder_from_an_anonymous_parent() {
        let mut embedded = IdentityArena::default();
        let callback = embedded
            .push_path(["script", "<anonymous>", "callback"])
            .expect("embedded callable identity must be valid");
        let mut destination = IdentityArena::default();

        let imported = destination
            .import(embedded)
            .expect("embedded identity must import");
        let callback = IdentityArena::remap(&imported, callback)
            .expect("imported callback identity must remain addressable");
        let canonical = CanonicalIdentityInterner::default()
            .canonicalize(&destination)
            .expect("imported identity must canonicalize")
            .resolve(callback)
            .expect("imported callback must resolve");

        assert!(canonical.contains_placeholder);
    }

    #[test]
    fn import_preserves_multisegment_parent_prefixes() {
        let mut destination = IdentityArena::default();
        let top_level_overload = destination
            .push_path(["overload"])
            .expect("top-level identity must be valid");
        let mut embedded = IdentityArena::default();
        let nested_overload = embedded
            .push_path(["namespace", "overload"])
            .expect("embedded identity must be valid");

        let imported = destination
            .import(embedded)
            .expect("embedded identity must import");
        let nested_overload = IdentityArena::remap(&imported, nested_overload)
            .expect("imported overload identity must remain addressable");
        let canonical = CanonicalIdentityInterner::default()
            .canonicalize(&destination)
            .expect("imported identities must canonicalize");

        assert_ne!(
            canonical
                .resolve(top_level_overload)
                .expect("top-level overload must resolve")
                .id,
            canonical
                .resolve(nested_overload)
                .expect("nested overload must resolve")
                .id,
        );
    }

    #[test]
    fn canonicalization_rejects_missing_identity_parents() {
        let arena = IdentityArena {
            entries: vec![IdentityEntry {
                parent: Some(IdentityId(2)),
                segment: "operation".to_owned(),
                contains_placeholder: false,
            }],
        };

        let error = CanonicalIdentityInterner::default()
            .canonicalize(&arena)
            .expect_err("missing identity parent must fail closed");

        assert!(matches!(error, AnalysisError::Invariant(_)));
    }

    #[test]
    fn canonicalization_rejects_identity_parent_cycles() {
        let arena = IdentityArena {
            entries: vec![IdentityEntry {
                parent: Some(IdentityId(0)),
                segment: "operation".to_owned(),
                contains_placeholder: false,
            }],
        };

        let error = CanonicalIdentityInterner::default()
            .canonicalize(&arena)
            .expect_err("identity parent cycle must fail closed");

        assert!(matches!(error, AnalysisError::Invariant(_)));
    }
}
