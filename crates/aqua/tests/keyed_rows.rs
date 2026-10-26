//! Verification: retained identity for keyed rows.
//!
//! The design requires keyed collections to preserve component identity
//! through insertion, removal, filtering, and reordering. This exercises the
//! mechanism black-box: one child `Scope` per stable key, per-row signals
//! for item fields, and disposal on removal. Reordering must not touch row
//! state; churn must return live counters to baseline (no tombstones).

use std::collections::HashMap;

use aqua_reactive::{ReadSignal, Runtime, Scope, WriteSignal};

struct Row {
    scope: Scope,
    text: WriteSignal<String>,
    text_read: ReadSignal<String>,
    done: WriteSignal<bool>,
    done_read: ReadSignal<bool>,
}

struct KeyedList {
    rt: Runtime,
    rows: HashMap<u32, Row>,
    order: Vec<u32>,
}

impl KeyedList {
    fn new() -> Self {
        Self {
            rt: Runtime::new(),
            rows: HashMap::new(),
            order: Vec::new(),
        }
    }

    fn insert(&mut self, key: u32, text: &str) {
        let scope = self.rt.create_scope();
        let (text_read, text) = scope.signal(text.to_string());
        let (done_read, done) = scope.signal(false);
        self.rows.insert(
            key,
            Row {
                scope,
                text,
                text_read,
                done,
                done_read,
            },
        );
        self.order.push(key);
    }

    fn remove(&mut self, key: u32) {
        if let Some(row) = self.rows.remove(&key) {
            row.scope.dispose();
        }
        self.order.retain(|k| *k != key);
    }

    fn texts_in_order(&self) -> Vec<String> {
        self.order
            .iter()
            .map(|k| self.rows[k].text_read.get())
            .collect()
    }
}

#[test]
fn reorder_preserves_per_row_state() {
    let mut list = KeyedList::new();
    list.insert(1, "buy milk");
    list.insert(2, "write report");
    list.insert(3, "call mom");
    // Check off row 2 and edit row 1.
    list.rows[&2].done.set(true);
    list.rows[&1].text.set("buy oat milk".to_string());

    // Reorder: move row 1 to the end (remove + re-push order only, no
    // scope churn — this is what a keyed diff does).
    list.order.remove(0);
    list.order.push(1);

    assert_eq!(
        list.texts_in_order(),
        vec!["write report", "call mom", "buy oat milk"]
    );
    assert!(list.rows[&2].done_read.get(), "selection survived reorder");
    assert_eq!(list.rows[&1].text_read.get(), "buy oat milk");
}

#[test]
fn removal_disposes_row_and_stale_handles_fail() {
    let mut list = KeyedList::new();
    list.insert(1, "a");
    list.insert(2, "b");
    let stale_text = list.rows[&1].text_read.clone();
    list.remove(1);
    assert_eq!(list.texts_in_order(), vec!["b".to_string()]);
    assert_eq!(stale_text.try_get(), None, "removed row fails closed");
    assert_eq!(list.rt.stats().sources, 2, "only row 2's signals remain");
}

#[test]
fn filter_and_restore_keeps_survivor_state() {
    let mut list = KeyedList::new();
    for (k, t) in [(1, "alpha"), (2, "beta"), (3, "gamma")] {
        list.insert(k, t);
    }
    list.rows[&2].done.set(true);
    // Filter to rows containing 'a'... keep it simple: remove non-matching.
    list.remove(2);
    list.remove(3);
    assert_eq!(list.texts_in_order(), vec!["alpha".to_string()]);
    // Restore: re-inserted rows are FRESH state, survivor keeps its state.
    list.insert(2, "beta");
    assert!(
        !list.rows[&2].done_read.get(),
        "re-inserted row starts fresh"
    );
    assert_eq!(list.rows[&1].text_read.get(), "alpha");
}

#[test]
fn churn_returns_counters_to_baseline() {
    // 500 insert/remove cycles: live counters must return to zero, proving
    // disposal reclaims map entries instead of tombstoning.
    let mut list = KeyedList::new();
    for i in 0..500u32 {
        list.insert(i, "temp");
        list.remove(i);
    }
    let s = list.rt.stats();
    assert_eq!((s.sources, s.derived, s.effects), (0, 0, 0));
    assert_eq!(s.live_scopes, 0);
    assert!(list.rows.is_empty() && list.order.is_empty());
}

#[test]
fn per_field_signals_give_field_granularity() {
    // Collection membership (row scope) and item fields (per-field signals)
    // are separate: editing one field of one row notifies only its
    // dependents, checked here via versions.
    let mut list = KeyedList::new();
    list.insert(1, "task");
    let v_text = list.rows[&1].text_read.version();
    let v_done = list.rows[&1].done_read.version();
    list.rows[&1].done.set(true);
    assert!(list.rows[&1].done_read.version() != v_done);
    assert_eq!(
        list.rows[&1].text_read.version(),
        v_text,
        "untouched field keeps its version"
    );
}

#[test]
fn derived_total_over_keyed_rows() {
    let mut list = KeyedList::new();
    for (k, t) in [(1, "a"), (2, "bb"), (3, "ccc")] {
        list.insert(k, t);
    }
    // Derived total length over explicitly declared per-row handles.
    let scope = list.rt.create_scope();
    let handles: Vec<ReadSignal<String>> = list
        .order
        .iter()
        .map(|k| list.rows[k].text_read.clone())
        .collect();
    // Tuples only go to 8; use three rows with a 3-tuple.
    let total = scope.derive(
        (handles[0].clone(), handles[1].clone(), handles[2].clone()),
        |(a, b, c)| a.len() + b.len() + c.len(),
    );
    assert_eq!(total.get(), 6);
    list.rows[&2].text.set("bbbb".to_string());
    assert_eq!(total.get(), 8, "field edit propagates to derived total");
    scope.dispose();
}
