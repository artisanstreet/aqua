use std::cell::Cell;
use std::rc::Rc;

use aqua_reactive::collection::{Changes, Collection, CollectionError, Delta};
use aqua_reactive::Runtime;

#[derive(Clone, Debug, PartialEq)]
struct Row {
    title: String,
    unread: bool,
    rank: i32,
}

fn row(rank: i32, unread: bool) -> Row {
    Row {
        title: rank.to_string(),
        rank,
        unread,
    }
}

#[test]
fn record_updates_do_not_invalidate_structure_or_unrelated_observers() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let rows = Collection::new(
        &scope,
        (0..100_000u32).map(|key| (key, row(key as i32, false))),
    )
    .unwrap();
    let visible = scope.child();
    let record = rows.observe(&visible, &5).unwrap();
    let observed = Rc::new(Cell::new(0));
    let count = observed.clone();
    scope.subscribe(&record, move |_| count.set(count.get() + 1));
    let shape = rows.structure().version();
    rows.update(&99_999, row(99_999, true)).unwrap();
    assert_eq!(observed.get(), 1);
    assert_eq!(rows.structure().version(), shape);
    assert_eq!(rows.observer_count(), 1);
    rows.update(&5, row(5, true)).unwrap();
    assert_eq!(observed.get(), 2);
    visible.dispose();
    assert_eq!(rows.observer_count(), 0);
    rows.update(&5, row(5, false)).unwrap();
    assert!(!record.is_live());
}

#[test]
fn count_and_sorted_query_process_only_changed_records() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let rows = Collection::new(
        &scope,
        [(1, row(10, true)), (2, row(5, false)), (3, row(20, true))],
    )
    .unwrap();
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    let count = rows.count_where(&scope, |row| row.unread);
    let query = rows.query(
        &scope,
        move |row| {
            observed.set(observed.get() + 1);
            row.unread
        },
        |row| row.rank,
    );
    assert_eq!(query.keys(), vec![1, 3]);
    calls.set(0);
    rows.update(&2, row(0, true)).unwrap();
    assert_eq!(count.get(), 3);
    assert_eq!(query.keys(), vec![2, 1, 3]);
    assert_eq!(calls.get(), 2);
    let revision = query.revision().version();
    rows.move_before(&3, Some(1)).unwrap();
    assert_eq!(query.revision().version(), revision);
    rows.remove(&1).unwrap();
    assert_eq!(count.get(), 2);
    assert_eq!(query.keys(), vec![2, 3]);
    rows.insert(4, Some(2), row(-10, true)).unwrap();
    assert_eq!(query.keys(), vec![4, 2, 3]);
}

#[test]
fn invalid_changes_are_atomic_and_delta_cursors_recover_from_overrun() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let rows = Collection::new(&scope, [(1, 10), (2, 20)]).unwrap();
    assert_eq!(rows.insert(1, None, 30), Err(CollectionError::DuplicateKey));
    assert_eq!(
        rows.move_before(&1, Some(99)),
        Err(CollectionError::InvalidPosition)
    );
    assert_eq!(rows.keys(), vec![1, 2]);
    rows.move_before(&2, Some(1)).unwrap();
    let Changes::Deltas { revision, deltas } = rows.changes_since(0) else {
        panic!()
    };
    assert_eq!(revision, 1);
    assert!(matches!(
        &*deltas[0],
        Delta::Move {
            key: 2,
            before: Some(1)
        }
    ));
    for value in 0..1100 {
        rows.update(&1, value).unwrap();
    }
    assert!(matches!(rows.changes_since(0), Changes::Reset { keys, .. } if keys == vec![2, 1]));
    scope.dispose();
    assert_eq!(rows.update(&1, 5), Err(CollectionError::Disposed));
}

#[test]
fn ordered_edits_match_a_sequence_model_through_slot_reuse() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let rows = Collection::new(&scope, (0..100u32).map(|key| (key, key))).unwrap();
    let mut expected: Vec<_> = (0..100u32).collect();
    let mut seed = 7u64;
    for key in 100..2100u32 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let index = seed as usize % expected.len();
        let before = expected[index];
        rows.insert(key, Some(before), key).unwrap();
        expected.insert(index, key);
        let moved = expected[(index + expected.len() / 2) % expected.len()];
        rows.move_before(&moved, Some(key)).unwrap();
        let old = expected
            .iter()
            .position(|candidate| *candidate == moved)
            .unwrap();
        expected.remove(old);
        let target = expected
            .iter()
            .position(|candidate| *candidate == key)
            .unwrap();
        expected.insert(target, moved);
        let removed = expected.pop().unwrap();
        rows.remove(&removed).unwrap();
        assert_eq!(rows.keys(), expected);
        for (index, key) in expected.iter().enumerate() {
            assert_eq!(rows.key_at(index), Some(*key));
        }
    }
}

#[test]
fn query_reconfiguration_replaces_dependencies_on_filter_rules() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let rows = Collection::new(&scope, [(1, row(1, true)), (2, row(2, false))]).unwrap();
    let query = rows.query(&scope, |row| row.unread, |row| row.rank);
    assert_eq!(query.configure(|_| true, |row| -row.rank).unwrap(), 2);
    assert_eq!(query.keys(), vec![2, 1]);
    rows.insert(3, None, row(3, false)).unwrap();
    assert_eq!(query.keys(), vec![3, 2, 1]);
    rows.update(&1, row(4, false)).unwrap();
    assert_eq!(query.keys(), vec![1, 3, 2]);
}

#[test]
fn reentrant_collection_edits_deliver_deltas_in_commit_order() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let rows = Collection::new(&scope, [(1, 0)]).unwrap();
    let nested = rows.clone();
    let observed = Rc::new(std::cell::RefCell::new(Vec::new()));
    let trace = observed.clone();
    rows.listen(&scope, move |delta| {
        if let Delta::Update { value, .. } = delta {
            trace.borrow_mut().push(**value);
            if **value == 1 {
                nested.update(&1, 2).unwrap();
            }
        }
    });
    rows.update(&1, 1).unwrap();
    assert_eq!(*observed.borrow(), vec![1, 2]);
    assert_eq!(rows.revision().get(), 2);
    assert_eq!(*rows.get(&1).unwrap(), 2);
    scope.dispose();
}

#[test]
fn effect_driven_collection_updates_finish_in_the_same_flush() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let rows = Collection::new(&scope, [(1, 0)]).unwrap();
    let record = rows.observe(&scope, &1).unwrap();
    let nested = rows.clone();
    scope.subscribe(&record, move |value| {
        if *value == 1 {
            nested.update(&1, 2).unwrap();
        }
    });
    rows.update(&1, 1).unwrap();
    assert_eq!(rows.revision().get(), 2);
    assert_eq!(*record.get(), 2);
    assert_eq!(runtime.pending(), 0);
    scope.dispose();
}

#[test]
fn removing_and_reinserting_a_key_creates_a_new_record_lifetime() {
    let runtime = Runtime::new();
    let owner = runtime.create_scope();
    let borrower = runtime.create_scope();
    let rows = Collection::new(&owner, [(1, 10)]).unwrap();
    let first = rows.observe(&borrower, &1).unwrap();
    let incarnation = rows.incarnation(&1).unwrap();
    rows.remove(&1).unwrap();
    assert!(!first.is_live());
    rows.insert(1, None, 20).unwrap();
    assert_ne!(rows.incarnation(&1), Some(incarnation));
    let second = rows.observe(&borrower, &1).unwrap();
    assert_eq!(*second.get(), 20);
    owner.dispose();
    assert!(!second.is_live());
    assert!(borrower.is_live());
    assert_eq!(rows.observer_count(), 0);
    borrower.dispose();
}

#[test]
fn derived_writes_are_rejected_before_collection_state_changes() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let rows = Collection::new(&scope, [(1, 10)]).unwrap();
    let (trigger, _) = scope.signal(20);
    let target = rows.clone();
    let outcome = scope.derive((trigger,), move |(value,)| target.update(&1, value));
    assert!(matches!(
        outcome.get(),
        Err(CollectionError::Graph(
            aqua_reactive::GraphError::DerivedWrite(_)
        ))
    ));
    assert_eq!(*rows.get(&1).unwrap(), 10);
    assert_eq!(rows.revision().get(), 0);
}

#[test]
fn dropping_the_runtime_releases_listeners_that_capture_their_collection() {
    let runtime = Runtime::new();
    let scope = runtime.create_scope();
    let value = Rc::new(());
    let weak = Rc::downgrade(&value);
    let rows = Collection::new(&scope, [(1, value)]).unwrap();
    let observed = rows.clone();
    rows.listen(&scope, move |_| {
        let _ = observed.len();
    });
    drop(rows);
    drop(scope);
    drop(runtime);
    assert!(weak.upgrade().is_none());
}

#[test]
fn disposing_data_kills_aggregates_and_queries_owned_by_other_scopes() {
    let runtime = Runtime::new();
    let owner = runtime.create_scope();
    let borrower = runtime.create_scope();
    let rows = Collection::new(&owner, [(1, 10)]).unwrap();
    let count = rows.count_where(&borrower, |_| true);
    let query = rows.query(&borrower, |_| true, |value| *value);
    owner.dispose();
    assert!(!count.is_live());
    assert!(!query.revision().is_live());
    assert!(query.keys().is_empty());
    assert!(borrower.is_live());
    borrower.dispose();
}

#[test]
fn disposed_listeners_are_skipped_during_an_in_progress_commit() {
    let runtime = Runtime::new();
    let owner = runtime.create_scope();
    let first = owner.child();
    let second = owner.child();
    let rows = Collection::new(&owner, [(1, 0)]).unwrap();
    let dispose = second.clone();
    rows.listen(&first, move |_| dispose.clone().dispose());
    let calls = Rc::new(Cell::new(0));
    let observed = calls.clone();
    rows.listen(&second, move |_| observed.set(observed.get() + 1));
    rows.update(&1, 1).unwrap();
    assert_eq!(calls.get(), 0);
    owner.dispose();
}
