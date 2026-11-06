use std::time::{Duration, Instant};

pub const POOL_TTL: Duration = Duration::from_secs(10);
pub const POOL_LIMIT: usize = 6;

pub struct Pool<K, T> {
    items: Vec<(K, T, Instant)>,
}

impl<K: PartialEq, T> Default for Pool<K, T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<K: PartialEq, T> Pool<K, T> {
    pub fn put(&mut self, key: K, item: T, now: Instant) {
        self.items.push((key, item, now));
        if self.items.len() > POOL_LIMIT {
            self.items.remove(0);
        }
    }

    pub fn take(&mut self, key: &K) -> Option<T> {
        let index = self.items.iter().rposition(|(held, _, _)| held == key)?;
        Some(self.items.remove(index).1)
    }

    pub fn expire(&mut self, now: Instant, ttl: Duration) -> usize {
        let before = self.items.len();
        self.items
            .retain(|(_, _, since)| now.saturating_duration_since(*since) < ttl);
        before - self.items.len()
    }

    pub fn clear(&mut self) -> usize {
        let count = self.items.len();
        self.items.clear();
        count
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &T)> {
        self.items.iter().map(|(key, item, _)| (key, item))
    }
}
