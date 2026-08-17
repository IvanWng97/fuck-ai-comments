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
