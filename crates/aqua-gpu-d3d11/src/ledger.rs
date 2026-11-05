use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex, MutexGuard};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Swapchain,
    Texture,
    TransientTexture,
    DynamicBuffer,
    StagingTexture,
    Query,
    Shader,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub count: u32,
    pub bytes: u64,
    pub peak_count: u32,
    pub peak_bytes: u64,
    pub created: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: &'static str,
    pub kind: Kind,
    pub line: Line,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub entries: Vec<Entry>,
}

impl Snapshot {
    pub fn live_bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.line.bytes).sum()
    }

    pub fn live_bytes_of(&self, kind: Kind) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .map(|entry| entry.line.bytes)
            .sum()
    }

    pub fn peak_bytes_of(&self, label: &str) -> u64 {
        self.entries
            .iter()
            .filter(|entry| entry.label == label)
            .map(|entry| entry.line.peak_bytes)
            .sum()
    }

    pub fn find(&self, label: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.label == label)
    }

    pub fn render(&self) -> String {
        let mib = |bytes: u64| bytes as f64 / 1048576.0;
        let mut lines: Vec<&Entry> = self.entries.iter().collect();
        lines.sort_by_key(|entry| std::cmp::Reverse(entry.line.peak_bytes));
        let body = lines
            .iter()
            .map(|entry| {
                format!(
                    "{}[{:?}] live {}x{:.3} peak {}x{:.3} created {}",
                    entry.label,
                    entry.kind,
                    entry.line.count,
                    mib(entry.line.bytes),
                    entry.line.peak_count,
                    mib(entry.line.peak_bytes),
                    entry.line.created
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!("ledger live {:.3} MiB: {body}", mib(self.live_bytes()))
    }
}

#[derive(Default)]
struct State {
    next: u64,
    live: HashMap<u64, (&'static str, u64)>,
    lines: BTreeMap<&'static str, (Kind, Line)>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

fn state() -> MutexGuard<'static, State> {
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[must_use]
pub struct Token {
    id: u64,
}

pub fn record(label: &'static str, kind: Kind, bytes: u64) -> Token {
    let mut state = state();
    state.next += 1;
    let id = state.next;
    state.live.insert(id, (label, bytes));
    let (_, line) = state.lines.entry(label).or_insert((kind, Line::default()));
    line.count += 1;
    line.bytes += bytes;
    line.created += 1;
    line.peak_count = line.peak_count.max(line.count);
    line.peak_bytes = line.peak_bytes.max(line.bytes);
    Token { id }
}

impl Drop for Token {
    fn drop(&mut self) {
        let mut state = state();
        let Some((label, bytes)) = state.live.remove(&self.id) else {
            return;
        };
        if let Some((_, line)) = state.lines.get_mut(label) {
            line.count -= 1;
            line.bytes -= bytes;
        }
    }
}

pub fn snapshot() -> Snapshot {
    let state = state();
    Snapshot {
        entries: state
            .lines
            .iter()
            .map(|(label, (kind, line))| Entry {
                label,
                kind: *kind,
                line: *line,
            })
            .collect(),
    }
}
