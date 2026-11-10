mod order;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::{Rc, Weak};

use crate::{ReadSignal, Scope, WriteSignal};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionError {
    Disposed,
    CrossRuntime,
    Graph(crate::GraphError),
    DuplicateKey,
    MissingKey,
    InvalidPosition,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Delta<K, T> {
    Insert {
        key: K,
        before: Option<K>,
        value: Rc<T>,
    },
    Update {
        key: K,
        previous: Rc<T>,
        value: Rc<T>,
    },
    Remove {
        key: K,
        previous: Rc<T>,
    },
    Move {
        key: K,
        before: Option<K>,
    },
}

#[derive(Clone, Debug)]
pub enum Changes<K, T> {
    Deltas {
        revision: u64,
        deltas: Vec<Rc<Delta<K, T>>>,
    },
    Reset {
        revision: u64,
        keys: Vec<K>,
    },
}

type Listener<K, T> = dyn Fn(&Delta<K, T>);

struct RecordObserver<T> {
    write: WriteSignal<Rc<T>>,
    scope: Scope,
}

struct CommittedDelta<K, T> {
    revision: u64,
    shape: u64,
    delta: Rc<Delta<K, T>>,
}

struct State<K, T> {
    rows: BTreeMap<K, Rc<T>>,
    order: order::Order<K>,
    revision: u64,
    shape_revision: u64,
    journal: VecDeque<(u64, Rc<Delta<K, T>>)>,
    observers: BTreeMap<K, BTreeMap<u64, RecordObserver<T>>>,
    incarnations: BTreeMap<K, u64>,
    next_incarnation: u64,
    listeners: BTreeMap<u64, Weak<Listener<K, T>>>,
    borrowers: BTreeMap<u64, Scope>,
    next_observer: u64,
    pending: VecDeque<CommittedDelta<K, T>>,
    publishing: bool,
}

pub struct Collection<K, T> {
    state: Rc<RefCell<State<K, T>>>,
    scope: Scope,
    revision: ReadSignal<u64>,
    set_revision: WriteSignal<u64>,
    shape: ReadSignal<u64>,
    set_shape: WriteSignal<u64>,
}

impl<K, T> Clone for Collection<K, T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            scope: self.scope.clone(),
            revision: self.revision.clone(),
            set_revision: self.set_revision.clone(),
            shape: self.shape.clone(),
            set_shape: self.set_shape.clone(),
        }
    }
}

impl<K: Ord + Clone + 'static, T: 'static> Collection<K, T> {
    pub fn new(
        scope: &Scope,
        rows: impl IntoIterator<Item = (K, T)>,
    ) -> Result<Self, CollectionError> {
        if !scope.is_live() {
            return Err(CollectionError::Disposed);
        }
        let mut state = State {
            rows: BTreeMap::new(),
            order: order::Order::new(),
            revision: 0,
            shape_revision: 0,
            journal: VecDeque::new(),
            observers: BTreeMap::new(),
            incarnations: BTreeMap::new(),
            next_incarnation: 0,
            listeners: BTreeMap::new(),
            borrowers: BTreeMap::new(),
            next_observer: 0,
            pending: VecDeque::new(),
            publishing: false,
        };
        for (key, value) in rows {
            if state.rows.insert(key.clone(), Rc::new(value)).is_some() {
                return Err(CollectionError::DuplicateKey);
            }
            state.next_incarnation += 1;
            state
                .incarnations
                .insert(key.clone(), state.next_incarnation);
            state.order.insert(state.order.len(), key);
        }
        let (revision, set_revision) = scope.signal(0);
        let (shape, set_shape) = scope.signal(0);
        let state = Rc::new(RefCell::new(state));
        let weak = Rc::downgrade(&state);
        scope.on_cleanup(move || {
            if let Some(state) = weak.upgrade() {
                let mut state = state.borrow_mut();
                state.listeners.clear();
                let observers = std::mem::take(&mut state.observers);
                let borrowers = std::mem::take(&mut state.borrowers);
                state.incarnations.clear();
                state.rows.clear();
                state.order = order::Order::new();
                state.journal.clear();
                state.pending.clear();
                drop(state);
                for borrower in borrowers.into_values() {
                    borrower.dispose();
                }
                for observers in observers.into_values() {
                    for observer in observers.into_values() {
                        observer.scope.dispose();
                    }
                }
            }
        });
        Ok(Self {
            state,
            scope: scope.clone(),
            revision,
            set_revision,
            shape,
            set_shape,
        })
    }

    pub fn revision(&self) -> ReadSignal<u64> {
        self.revision.clone()
    }
    pub fn structure(&self) -> ReadSignal<u64> {
        self.shape.clone()
    }
    pub fn shape_revision(&self) -> u64 {
        self.state.borrow().shape_revision
    }
    pub fn len(&self) -> usize {
        self.state.borrow().order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.state.borrow().order.is_empty()
    }
    pub fn keys(&self) -> Vec<K> {
        self.state.borrow().order.keys()
    }
    pub fn key_at(&self, index: usize) -> Option<K> {
        self.state.borrow().order.get(index).cloned()
    }
    pub fn get(&self, key: &K) -> Option<Rc<T>> {
        if !self.scope.is_live() {
            return None;
        }
        self.state.borrow().rows.get(key).cloned()
    }
    pub fn incarnation(&self, key: &K) -> Option<u64> {
        self.state.borrow().incarnations.get(key).copied()
    }

    pub fn observer_count(&self) -> usize {
        self.state
            .borrow()
            .observers
            .values()
            .map(BTreeMap::len)
            .sum()
    }

    pub fn observe(&self, scope: &Scope, key: &K) -> Result<ReadSignal<Rc<T>>, CollectionError> {
        if !self.scope.is_live() || !scope.is_live() {
            return Err(CollectionError::Disposed);
        }
        if !scope.shares_runtime(&self.revision) {
            return Err(CollectionError::CrossRuntime);
        }
        let value = self.get(key).ok_or(CollectionError::MissingKey)?;
        let owner = scope.child();
        let (read, write) = owner.signal(value);
        let mut state = self.state.borrow_mut();
        let token = state.next_observer;
        state.next_observer += 1;
        state.observers.entry(key.clone()).or_default().insert(
            token,
            RecordObserver {
                write,
                scope: owner.clone(),
            },
        );
        drop(state);
        let weak = Rc::downgrade(&self.state);
        let key = key.clone();
        owner.on_cleanup(move || {
            if let Some(state) = weak.upgrade() {
                let mut state = state.borrow_mut();
                if let Some(observers) = state.observers.get_mut(&key) {
                    observers.remove(&token);
                    if observers.is_empty() {
                        state.observers.remove(&key);
                    }
                }
            }
        });
        Ok(read)
    }

    pub fn field<V: Clone + PartialEq + 'static>(
        &self,
        scope: &Scope,
        key: &K,
        project: impl Fn(&T) -> V + 'static,
    ) -> Result<ReadSignal<V>, CollectionError> {
        let record = self.observe(scope, key)?;
        Ok(scope.derive((record,), move |(record,)| project(&record)))
    }

    pub fn update_field<V: PartialEq>(
        &self,
        key: &K,
        value: V,
        read: impl FnOnce(&T) -> V,
        write: impl FnOnce(&mut T, V),
    ) -> Result<(), CollectionError>
    where
        T: Clone + PartialEq,
    {
        self.ensure_live()?;
        let previous = self.get(key).ok_or(CollectionError::MissingKey)?;
        if read(&previous) == value {
            return Ok(());
        }
        let mut next = (*previous).clone();
        write(&mut next, value);
        self.update(key, next)
    }

    pub fn changes_since(&self, revision: u64) -> Changes<K, T> {
        let state = self.state.borrow();
        let floor = state
            .journal
            .front()
            .map_or(state.revision, |(revision, _)| revision.saturating_sub(1));
        match revision >= floor && revision <= state.revision {
            true => Changes::Deltas {
                revision: state.revision,
                deltas: state
                    .journal
                    .iter()
                    .filter(|(r, _)| *r > revision)
                    .map(|(_, delta)| delta.clone())
                    .collect(),
            },
            false => Changes::Reset {
                revision: state.revision,
                keys: state.order.keys(),
            },
        }
    }

    pub fn insert(&self, key: K, before: Option<K>, value: T) -> Result<(), CollectionError> {
        self.ensure_live()?;
        let value = Rc::new(value);
        {
            let mut state = self.state.borrow_mut();
            if state.rows.contains_key(&key) {
                return Err(CollectionError::DuplicateKey);
            }
            let position = Self::position(&state, before.as_ref())?;
            state.order.insert(position, key.clone());
            state.next_incarnation += 1;
            let incarnation = state.next_incarnation;
            state.incarnations.insert(key.clone(), incarnation);
            state.rows.insert(key.clone(), value.clone());
        }
        self.publish(Delta::Insert { key, before, value });
        Ok(())
    }

    pub fn update(&self, key: &K, value: T) -> Result<(), CollectionError>
    where
        T: PartialEq,
    {
        self.ensure_live()?;
        let previous = self.get(key).ok_or(CollectionError::MissingKey)?;
        if *previous == value {
            return Ok(());
        }
        let value = Rc::new(value);
        self.state
            .borrow_mut()
            .rows
            .insert(key.clone(), value.clone());
        self.publish(Delta::Update {
            key: key.clone(),
            previous,
            value,
        });
        Ok(())
    }

    pub fn remove(&self, key: &K) -> Result<(), CollectionError> {
        self.ensure_live()?;
        let (previous, observers) = {
            let mut state = self.state.borrow_mut();
            let previous = state.rows.remove(key).ok_or(CollectionError::MissingKey)?;
            state.order.remove(key);
            state.incarnations.remove(key);
            let observers = state.observers.remove(key).unwrap_or_default();
            (previous, observers)
        };
        for observer in observers.into_values() {
            observer.scope.dispose();
        }
        self.publish(Delta::Remove {
            key: key.clone(),
            previous,
        });
        Ok(())
    }

    pub fn move_before(&self, key: &K, before: Option<K>) -> Result<(), CollectionError> {
        self.ensure_live()?;
        {
            let mut state = self.state.borrow_mut();
            if !state.rows.contains_key(key) {
                return Err(CollectionError::MissingKey);
            }
            Self::position(&state, before.as_ref())?;
            if before.as_ref() == Some(key) {
                return Ok(());
            }
            let index = state.order.rank(key).ok_or(CollectionError::MissingKey)?;
            let next = state.order.get(index + 1);
            if next == before.as_ref() {
                return Ok(());
            }
            state.order.remove(key);
            let position = Self::position(&state, before.as_ref())?;
            state.order.insert(position, key.clone());
        }
        self.publish(Delta::Move {
            key: key.clone(),
            before,
        });
        Ok(())
    }

    pub fn listen(&self, scope: &Scope, listener: impl Fn(&Delta<K, T>) + 'static) {
        assert!(
            scope.is_live() && self.scope.is_live(),
            "collection listeners require live scopes"
        );
        assert!(
            scope.shares_runtime(&self.revision),
            "collection listeners must share a runtime"
        );
        let mut state = self.state.borrow_mut();
        let token = state.next_observer;
        state.next_observer += 1;
        let listener: Rc<Listener<K, T>> = Rc::new(listener);
        state.listeners.insert(token, Rc::downgrade(&listener));
        drop(state);
        let weak = Rc::downgrade(&self.state);
        scope.on_cleanup(move || {
            if let Some(state) = weak.upgrade() {
                state.borrow_mut().listeners.remove(&token);
            }
            drop(listener);
        });
    }

    pub fn count_where(
        &self,
        scope: &Scope,
        predicate: impl Fn(&T) -> bool + 'static,
    ) -> ReadSignal<usize> {
        let owner = self.borrow_scope(scope);
        let count = self
            .state
            .borrow()
            .rows
            .values()
            .filter(|row| predicate(row))
            .count();
        let (read, write) = owner.signal(count);
        let count = Rc::new(std::cell::Cell::new(count));
        self.listen(&owner, move |delta| {
            let (removed, added) = match delta {
                Delta::Insert { value, .. } => (false, predicate(value)),
                Delta::Remove { previous, .. } => (predicate(previous), false),
                Delta::Update {
                    previous, value, ..
                } => (predicate(previous), predicate(value)),
                Delta::Move { .. } => (false, false),
            };
            if removed != added {
                let next = count.get() - usize::from(removed) + usize::from(added);
                count.set(next);
                let _ = write.try_set(next);
            }
        });
        read
    }

    pub fn query<S: Ord + Clone + 'static>(
        &self,
        scope: &Scope,
        predicate: impl Fn(&T) -> bool + 'static,
        sort_key: impl Fn(&T) -> S + 'static,
    ) -> Query<K, T, S> {
        let owner = self.borrow_scope(scope);
        let rules = Rc::new(RefCell::new(QueryRules {
            predicate: Box::new(predicate) as Box<dyn Fn(&T) -> bool>,
            sort_key: Box::new(sort_key) as Box<dyn Fn(&T) -> S>,
        }));
        let mut index = BTreeSet::new();
        {
            let rules = rules.borrow();
            let state = self.state.borrow();
            for (key, value) in &state.rows {
                if (rules.predicate)(value) {
                    index.insert(((rules.sort_key)(value), key.clone()));
                }
            }
        }
        let (revision, write) = owner.signal(0u64);
        let index = Rc::new(RefCell::new(index));
        let target = index.clone();
        let counter = Rc::new(std::cell::Cell::new(0u64));
        let update_counter = counter.clone();
        let update_write = write.clone();
        let update_rules = rules.clone();
        self.listen(&owner, move |delta| {
            let rules = update_rules.borrow();
            let predicate = &rules.predicate;
            let sort_key = &rules.sort_key;
            let mut index = target.borrow_mut();
            let changed = match delta {
                Delta::Insert { key, value, .. } => {
                    predicate(value) && index.insert((sort_key(value), key.clone()))
                }
                Delta::Remove { key, previous } => {
                    predicate(previous) && index.remove(&(sort_key(previous), key.clone()))
                }
                Delta::Update {
                    key,
                    previous,
                    value,
                } => {
                    let old = predicate(previous).then(|| (sort_key(previous), key.clone()));
                    let new = predicate(value).then(|| (sort_key(value), key.clone()));
                    if old == new {
                        false
                    } else {
                        if let Some(old) = old {
                            index.remove(&old);
                        }
                        if let Some(new) = new {
                            index.insert(new);
                        }
                        true
                    }
                }
                Delta::Move { .. } => false,
            };
            drop(index);
            drop(rules);
            if changed {
                update_counter.set(update_counter.get() + 1);
                let _ = update_write.try_set(update_counter.get());
            }
        });
        Query {
            collection: self.clone(),
            index,
            revision,
            write,
            counter,
            rules,
        }
    }

    fn borrow_scope(&self, scope: &Scope) -> Scope {
        assert!(
            self.scope.is_live() && scope.is_live(),
            "collection computations require live scopes"
        );
        assert!(
            scope.shares_runtime(&self.revision),
            "collection computations must share a runtime"
        );
        let owner = scope.child();
        let mut state = self.state.borrow_mut();
        let token = state.next_observer;
        state.next_observer += 1;
        state.borrowers.insert(token, owner.clone());
        drop(state);
        let weak = Rc::downgrade(&self.state);
        owner.on_cleanup(move || {
            if let Some(state) = weak.upgrade() {
                state.borrow_mut().borrowers.remove(&token);
            }
        });
        owner
    }

    fn ensure_live(&self) -> Result<(), CollectionError> {
        if !self.scope.is_live() {
            return Err(CollectionError::Disposed);
        }
        self.set_revision
            .check_writable()
            .map_err(CollectionError::Graph)
    }

    fn position(state: &State<K, T>, before: Option<&K>) -> Result<usize, CollectionError> {
        match before {
            Some(key) => state
                .order
                .rank(key)
                .ok_or(CollectionError::InvalidPosition),
            None => Ok(state.order.len()),
        }
    }

    fn publish(&self, delta: Delta<K, T>) {
        let delta = Rc::new(delta);
        {
            let mut state = self.state.borrow_mut();
            state.revision += 1;
            if !matches!(&*delta, Delta::Update { .. }) {
                state.shape_revision += 1;
            }
            let revision = state.revision;
            let shape = state.shape_revision;
            state.journal.push_back((revision, delta.clone()));
            while state.journal.len() > 1024 {
                state.journal.pop_front();
            }
            state.pending.push_back(CommittedDelta {
                revision,
                shape,
                delta,
            });
            if state.publishing {
                return;
            }
            state.publishing = true;
        }
        let _reset = PublishingGuard(self.state.clone());
        self.scope.batch(|| loop {
            let next = self.state.borrow_mut().pending.pop_front();
            let Some(CommittedDelta {
                revision,
                shape,
                delta,
            }) = next
            else {
                self.state.borrow_mut().publishing = false;
                break;
            };
            let (listeners, observers) = {
                let state = self.state.borrow();
                let observers = match &*delta {
                    Delta::Update { key, value, .. } => state.observers.get(key).map(|observers| {
                        (
                            value.clone(),
                            observers
                                .values()
                                .map(|observer| observer.write.clone())
                                .collect::<Vec<_>>(),
                        )
                    }),
                    _ => None,
                };
                (
                    state
                        .listeners
                        .iter()
                        .filter_map(|(token, listener)| {
                            listener.upgrade().map(|listener| (*token, listener))
                        })
                        .collect::<Vec<_>>(),
                    observers,
                )
            };
            if let Some((value, observers)) = observers {
                for observer in observers {
                    let _ = observer.try_set(value.clone());
                }
            }
            for (token, listener) in listeners {
                let live = self.state.borrow().listeners.contains_key(&token);
                if live {
                    listener(&delta);
                }
            }
            let _ = self.set_revision.try_set(revision);
            if !matches!(&*delta, Delta::Update { .. }) {
                let _ = self.set_shape.try_set(shape);
            }
        });
    }
}

struct PublishingGuard<K, T>(Rc<RefCell<State<K, T>>>);

impl<K, T> Drop for PublishingGuard<K, T> {
    fn drop(&mut self) {
        self.0.borrow_mut().publishing = false;
    }
}

struct QueryRules<T, S> {
    predicate: Box<dyn Fn(&T) -> bool>,
    sort_key: Box<dyn Fn(&T) -> S>,
}

pub struct Query<K, T, S> {
    collection: Collection<K, T>,
    index: Rc<RefCell<BTreeSet<(S, K)>>>,
    revision: ReadSignal<u64>,
    write: WriteSignal<u64>,
    counter: Rc<std::cell::Cell<u64>>,
    rules: Rc<RefCell<QueryRules<T, S>>>,
}

impl<K, T, S> Clone for Query<K, T, S> {
    fn clone(&self) -> Self {
        Self {
            collection: self.collection.clone(),
            index: self.index.clone(),
            revision: self.revision.clone(),
            write: self.write.clone(),
            counter: self.counter.clone(),
            rules: self.rules.clone(),
        }
    }
}

impl<K: Ord + Clone + 'static, T: 'static, S: Ord + Clone + 'static> Query<K, T, S> {
    pub fn keys(&self) -> Vec<K> {
        if !self.revision.is_live() {
            return Vec::new();
        }
        self.index
            .borrow()
            .iter()
            .map(|(_, key)| key.clone())
            .collect()
    }
    pub fn revision(&self) -> ReadSignal<u64> {
        self.revision.clone()
    }
    pub fn collection(&self) -> &Collection<K, T> {
        &self.collection
    }
    pub fn len(&self) -> usize {
        self.index.borrow().len()
    }
    pub fn is_empty(&self) -> bool {
        self.index.borrow().is_empty()
    }

    pub fn configure(
        &self,
        predicate: impl Fn(&T) -> bool + 'static,
        sort_key: impl Fn(&T) -> S + 'static,
    ) -> Result<usize, CollectionError> {
        if !self.write.is_live() {
            return Err(CollectionError::Disposed);
        }
        self.collection.ensure_live()?;
        self.write
            .check_writable()
            .map_err(CollectionError::Graph)?;
        let rules = QueryRules {
            predicate: Box::new(predicate),
            sort_key: Box::new(sort_key),
        };
        let mut next = BTreeSet::new();
        let state = self.collection.state.borrow();
        let inspected = state.rows.len();
        for (key, value) in &state.rows {
            if (rules.predicate)(value) {
                next.insert(((rules.sort_key)(value), key.clone()));
            }
        }
        drop(state);
        *self.rules.borrow_mut() = rules;
        let changed = *self.index.borrow() != next;
        if changed {
            *self.index.borrow_mut() = next;
            self.counter.set(self.counter.get() + 1);
            self.write.set(self.counter.get());
        }
        Ok(inspected)
    }
}
