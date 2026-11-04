use aqua_ui::element::Key;
use aqua_ui::tree::hash_key;

impl super::Runtime {
    pub fn focus_key(&mut self, key: impl Into<Key>) -> bool {
        let key = hash_key(key.into());
        let mut matches = self.tree.traverse().into_iter().filter(|node| {
            self.tree
                .get(*node)
                .is_some_and(|record| record.key == Some(key))
        });
        let Some(node) = matches.next() else {
            return false;
        };
        if matches.next().is_some() {
            return false;
        }
        self.tree.set_focused(Some(node))
    }
}

impl super::Runtime {
    pub fn find_debug_selector(&self, selector: &str) -> Option<aqua_core::NodeId> {
        let mut matches = self.tree.traverse().into_iter().filter(|node| {
            self.tree
                .get(*node)
                .is_some_and(|record| record.debug_selector.as_deref() == Some(selector))
        });
        let node = matches.next()?;
        matches.next().is_none().then_some(node)
    }
}
