use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use aqua_protocol::AquaWindowId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AquaUserEvent {
    ReactiveWake(AquaWindowId),
    Messages(AquaWindowId),
    WindowRequests,
    A11yActions(AquaWindowId),
    RenderEvents(AquaWindowId),
    Tsf(AquaWindowId),
}

pub type Waker = Arc<dyn Fn() + Send + Sync>;

impl AquaUserEvent {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ReactiveWake(_) => "reactive",
            Self::Messages(_) => "messages",
            Self::WindowRequests => "window-requests",
            Self::A11yActions(_) => "a11y",
            Self::RenderEvents(_) => "render-events",
            Self::Tsf(_) => "tsf",
        }
    }
}

pub type PostCounts = BTreeMap<(&'static str, String), u32>;

#[derive(Default)]
struct Shared {
    events: Mutex<Vec<AquaUserEvent>>,
    waker: Mutex<Option<Waker>>,
    accounting: AtomicBool,
    posts: Mutex<PostCounts>,
}

#[derive(Clone, Default)]
pub struct Inbox {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Inbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inbox").finish_non_exhaustive()
    }
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Inbox {
    pub fn account_posts(&self, enabled: bool) {
        self.shared.accounting.store(enabled, Ordering::Relaxed);
    }

    pub fn take_post_counts(&self) -> PostCounts {
        std::mem::take(&mut *locked(&self.shared.posts))
    }

    pub fn post(&self, event: AquaUserEvent) {
        self.post_labeled(event, event.label(), "");
    }

    pub fn post_labeled(&self, event: AquaUserEvent, label: &'static str, origin: &str) {
        if self.shared.accounting.load(Ordering::Relaxed) {
            let thread = std::thread::current();
            let name = format!("{}{origin}", thread.name().unwrap_or("unnamed"));
            *locked(&self.shared.posts).entry((label, name)).or_default() += 1;
        }
        {
            let mut events = locked(&self.shared.events);
            if !events.contains(&event) {
                events.push(event);
            }
        }
        let waker = locked(&self.shared.waker).clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    pub fn set_waker(&self, waker: Option<Waker>) {
        *locked(&self.shared.waker) = waker;
    }

    pub fn drain(&self) -> Vec<AquaUserEvent> {
        std::mem::take(&mut *locked(&self.shared.events))
    }

    pub fn drain_into(&self, scratch: &mut Vec<AquaUserEvent>) {
        scratch.clear();
        std::mem::swap(&mut *locked(&self.shared.events), scratch);
    }

    pub fn is_empty(&self) -> bool {
        locked(&self.shared.events).is_empty()
    }
}
