use tree_sitter::{Node, TreeCursor};

#[derive(Clone, Copy)]
pub(crate) enum WalkEvent<'tree> {
    Enter(Node<'tree>),
    Leave(Node<'tree>),
}

pub(crate) fn events(root: Node<'_>) -> WalkEvents<'_> {
    WalkEvents {
        cursor: root.walk(),
        phase: Some(Phase::Enter),
    }
}

pub(crate) fn outermost_matching_nodes(
    root: Node<'_>,
    mut matches: impl FnMut(&str) -> bool,
) -> Vec<Node<'_>> {
    let mut cursor = root.walk();
    let mut found = Vec::new();
    loop {
        let node = cursor.node();
        record_outermost_visit();
        if matches(node.kind()) {
            found.push(node);
        } else if cursor.goto_first_child() {
            continue;
        }

        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return found;
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    static OUTERMOST_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_outermost_visit() {
    OUTERMOST_VISITS.with(|visits| visits.set(visits.get() + 1));
}

#[cfg(not(test))]
fn record_outermost_visit() {}

#[cfg(test)]
pub(crate) fn reset_outermost_visits() {
    OUTERMOST_VISITS.with(|visits| visits.set(0));
}

#[cfg(test)]
pub(crate) fn outermost_visits() -> usize {
    OUTERMOST_VISITS.with(std::cell::Cell::get)
}

pub(crate) struct WalkEvents<'tree> {
    cursor: TreeCursor<'tree>,
    phase: Option<Phase>,
}

#[derive(Clone, Copy)]
enum Phase {
    Enter,
    Leave,
}

impl<'tree> Iterator for WalkEvents<'tree> {
    type Item = WalkEvent<'tree>;

    fn next(&mut self) -> Option<Self::Item> {
        let phase = self.phase?;
        let node = self.cursor.node();
        match phase {
            Phase::Enter => {
                if self.cursor.goto_first_child() {
                    self.phase = Some(Phase::Enter);
                } else {
                    self.phase = Some(Phase::Leave);
                }
                Some(WalkEvent::Enter(node))
            }
            Phase::Leave => {
                if self.cursor.goto_next_sibling() {
                    self.phase = Some(Phase::Enter);
                } else if self.cursor.goto_parent() {
                    self.phase = Some(Phase::Leave);
                } else {
                    self.phase = None;
                }
                Some(WalkEvent::Leave(node))
            }
        }
    }
}
