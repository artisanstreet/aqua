use std::collections::BTreeMap;

struct Node<K> {
    key: K,
    left: Option<usize>,
    right: Option<usize>,
    parent: Option<usize>,
    size: usize,
    priority: u64,
}

pub(super) struct Order<K> {
    nodes: Vec<Option<Node<K>>>,
    free: Vec<usize>,
    index: BTreeMap<K, usize>,
    root: Option<usize>,
    sequence: u64,
}

impl<K: Ord + Clone> Order<K> {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            free: Vec::new(),
            index: BTreeMap::new(),
            root: None,
            sequence: 0,
        }
    }
    pub fn len(&self) -> usize {
        self.size(self.root)
    }
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn get(&self, mut index: usize) -> Option<&K> {
        let mut cursor = self.root;
        while let Some(slot) = cursor {
            let node = self.node(slot);
            let left = self.size(node.left);
            match index.cmp(&left) {
                std::cmp::Ordering::Less => cursor = node.left,
                std::cmp::Ordering::Equal => return Some(&node.key),
                std::cmp::Ordering::Greater => {
                    index -= left + 1;
                    cursor = node.right;
                }
            }
        }
        None
    }

    pub fn rank(&self, key: &K) -> Option<usize> {
        let mut slot = *self.index.get(key)?;
        let mut rank = self.size(self.node(slot).left);
        while let Some(parent) = self.node(slot).parent {
            if self.node(parent).right == Some(slot) {
                rank += self.size(self.node(parent).left) + 1;
            }
            slot = parent;
        }
        Some(rank)
    }

    pub fn keys(&self) -> Vec<K> {
        let mut out = Vec::with_capacity(self.len());
        let mut stack = Vec::new();
        let mut cursor = self.root;
        loop {
            match cursor {
                Some(slot) => {
                    stack.push(slot);
                    cursor = self.node(slot).left;
                }
                None => match stack.pop() {
                    Some(slot) => {
                        out.push(self.node(slot).key.clone());
                        cursor = self.node(slot).right;
                    }
                    None => return out,
                },
            }
        }
    }

    pub fn insert(&mut self, at: usize, key: K) {
        self.sequence = self.sequence.wrapping_add(1);
        let mut priority = self.sequence.wrapping_add(0x9e3779b97f4a7c15);
        priority = (priority ^ (priority >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        priority = (priority ^ (priority >> 27)).wrapping_mul(0x94d049bb133111eb);
        priority ^= priority >> 31;
        let node = Node {
            key: key.clone(),
            left: None,
            right: None,
            parent: None,
            size: 1,
            priority,
        };
        let slot = match self.free.pop() {
            Some(slot) => {
                self.nodes[slot] = Some(node);
                slot
            }
            None => {
                let slot = self.nodes.len();
                self.nodes.push(Some(node));
                slot
            }
        };
        self.index.insert(key, slot);
        let (left, right) = self.split(self.root, at);
        let left = self.join(left, Some(slot));
        self.root = self.join(left, right);
    }

    pub fn remove(&mut self, key: &K) -> Option<K> {
        let rank = self.rank(key)?;
        let (left, tail) = self.split(self.root, rank);
        let (removed, right) = self.split(tail, 1);
        self.root = self.join(left, right);
        let slot = removed?;
        let node = self.nodes[slot].take()?;
        self.index.remove(&node.key);
        self.free.push(slot);
        Some(node.key)
    }

    fn node(&self, slot: usize) -> &Node<K> {
        self.nodes[slot].as_ref().expect("order node must be live")
    }
    fn node_mut(&mut self, slot: usize) -> &mut Node<K> {
        self.nodes[slot].as_mut().expect("order node must be live")
    }
    fn size(&self, root: Option<usize>) -> usize {
        root.map_or(0, |slot| self.node(slot).size)
    }

    fn refresh(&mut self, slot: usize) {
        let left = self.node(slot).left;
        let right = self.node(slot).right;
        self.node_mut(slot).size = 1 + self.size(left) + self.size(right);
        if let Some(child) = left {
            self.node_mut(child).parent = Some(slot);
        }
        if let Some(child) = right {
            self.node_mut(child).parent = Some(slot);
        }
        self.node_mut(slot).parent = None;
    }

    fn split(&mut self, root: Option<usize>, at: usize) -> (Option<usize>, Option<usize>) {
        let Some(slot) = root else {
            return (None, None);
        };
        let left_size = self.size(self.node(slot).left);
        match at <= left_size {
            true => {
                let (left, middle) = self.split(self.node(slot).left, at);
                self.node_mut(slot).left = middle;
                self.refresh(slot);
                (left, Some(slot))
            }
            false => {
                let (middle, right) = self.split(self.node(slot).right, at - left_size - 1);
                self.node_mut(slot).right = middle;
                self.refresh(slot);
                (Some(slot), right)
            }
        }
    }

    fn join(&mut self, left: Option<usize>, right: Option<usize>) -> Option<usize> {
        match (left, right) {
            (None, root) | (root, None) => {
                if let Some(slot) = root {
                    self.node_mut(slot).parent = None;
                }
                root
            }
            (Some(left), Some(right)) => {
                match self.node(left).priority <= self.node(right).priority {
                    true => {
                        let joined = self.join(self.node(left).right, Some(right));
                        self.node_mut(left).right = joined;
                        self.refresh(left);
                        Some(left)
                    }
                    false => {
                        let joined = self.join(Some(left), self.node(right).left);
                        self.node_mut(right).left = joined;
                        self.refresh(right);
                        Some(right)
                    }
                }
            }
        }
    }
}
