use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use aqua_core::{ImageId, ImageRef, RawId};
use aqua_media::decode::animation::{decode_animation, AnimationBudget, DecodedAnimation};

static NEXT_IMAGE_INDEX: AtomicU32 = AtomicU32::new(0x8000_0000);

mod application;
pub use application::ManagedUiApp;
use aqua_media::decode::Detail;
pub use aqua_media::decode::{DecodedImage, MediaError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageAssetError {
    OverBudget,
    Decode(String),
    Upload(String),
    Worker(String),
}

impl std::fmt::Display for ImageAssetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "image asset: {self:?}")
    }
}
impl std::error::Error for ImageAssetError {}

#[derive(Clone, Copy, Debug)]
pub struct ImageAssetBudget {
    pub decoded_bytes: u64,
    pub pending_bytes: usize,
    pub images: usize,
    pub workers: usize,
    pub gpu_bytes: u64,
    pub gpu_images: usize,
}
impl Default for ImageAssetBudget {
    fn default() -> Self {
        Self {
            decoded_bytes: 128 * 1024 * 1024,
            pending_bytes: 64 * 1024 * 1024,
            images: 512,
            workers: 4,
            gpu_bytes: 128 * 1024 * 1024,
            gpu_images: 64,
        }
    }
}
impl ImageAssetBudget {
    pub fn editor() -> Self {
        Self {
            decoded_bytes: 256 * 1024 * 1024,
            pending_bytes: 256 * 1024 * 1024,
            images: 256,
            workers: 4,
            gpu_bytes: 256 * 1024 * 1024,
            gpu_images: 64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageStatus {
    Pending,
    Ready { width: u32, height: u32 },
    Failed(ImageAssetError),
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageAssetStats {
    pub images: usize,
    pub decoded_bytes: u64,
    pub pending_bytes: usize,
    pub workers: usize,
}

#[derive(Clone)]
pub struct ImageAssets {
    state: Arc<Mutex<State>>,
    observers: Rc<RefCell<Observers>>,
    subscribers: Arc<Mutex<Subscribers>>,
}

#[derive(Clone)]
pub struct ImageHandle {
    lease: Arc<Lease>,
}

#[derive(Clone)]
pub struct WeakImageHandle {
    lease: Weak<Lease>,
}

#[derive(Default)]
struct Subscribers {
    next: u64,
    callbacks: HashMap<u64, crate::mailbox::Wake>,
}

pub struct ImageSubscription {
    id: u64,
    subscribers: Weak<Mutex<Subscribers>>,
}

impl Drop for ImageSubscription {
    fn drop(&mut self) {
        if let Some(subscribers) = self.subscribers.upgrade() {
            subscribers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .callbacks
                .remove(&self.id);
        }
    }
}

struct Observer {
    alive: Rc<Cell<bool>>,
    handle: Weak<Lease>,
    last: ImageStatus,
    update: Box<dyn FnMut(ImageStatus) -> bool>,
}
#[derive(Default)]
struct Observers {
    entries: Vec<Observer>,
}

struct Lease {
    id: ImageId,
    state: Weak<Mutex<State>>,
}

#[derive(Clone)]
enum Data {
    Still(Arc<DecodedImage>),
    Animation(Arc<DecodedAnimation>),
}

impl Data {
    fn bytes(&self) -> u64 {
        match self {
            Self::Still(image) => image.pixels().len() as u64,
            Self::Animation(animation) => animation
                .frames()
                .iter()
                .map(|frame| frame.image.pixels().len() as u64)
                .sum(),
        }
    }
    fn first(&self) -> &DecodedImage {
        match self {
            Self::Still(image) => image,
            Self::Animation(animation) => &animation.frames()[0].image,
        }
    }
}

enum Phase {
    Pending,
    Ready(Data),
    Retired,
    Failed(ImageAssetError),
}
struct Retention {
    encoded: Arc<[u8]>,
    width: u32,
    height: u32,
    source: Option<(u32, u32)>,
}
struct Slot {
    generation: u32,
    phase: Option<Phase>,
    preview: bool,
    detail: Detail,
    retention: Option<Retention>,
    ready_at: Option<std::time::Instant>,
}
struct Decoded {
    data: Data,
    source: Option<(u32, u32)>,
}
const RETIRE_MIN_BYTES: u64 = 256 * 1024;
pub(crate) const RETIRE_IDLE: std::time::Duration = std::time::Duration::from_secs(2);
struct DecodeJob {
    id: ImageId,
    encoded: Arc<[u8]>,
    preview: bool,
    detail: Detail,
    _guard: Option<Box<dyn Send>>,
}

#[cfg(test)]
static TEST_SERIAL: Mutex<()> = Mutex::new(());
#[cfg(test)]
type DecodeStarted = Arc<dyn Fn(ImageId) + Send + Sync>;
#[cfg(test)]
static DECODE_STARTED: std::sync::LazyLock<Mutex<Option<DecodeStarted>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

struct State {
    queue: VecDeque<DecodeJob>,
    slots: HashMap<u32, Slot>,
    free: Vec<u32>,
    budget: ImageAssetBudget,
    stats: ImageAssetStats,
    revision: u64,
    wake: Option<crate::mailbox::Wake>,
    fonts: Option<aqua_text::SharedFonts>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Extent {
    source: (u32, u32),
    decoded: (u32, u32),
    settled: bool,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|error| error.into_inner())
}

impl State {
    fn slot(&self, id: ImageId) -> Option<&Slot> {
        self.slots
            .get(&id.raw().index)
            .filter(|slot| slot.generation == id.raw().generation && slot.phase.is_some())
    }
    fn slot_mut(&mut self, id: ImageId) -> Option<&mut Slot> {
        self.slots
            .get_mut(&id.raw().index)
            .filter(|slot| slot.generation == id.raw().generation && slot.phase.is_some())
    }
    fn allocate(&mut self, phase: Phase, preview: bool) -> Result<ImageId, ImageAssetError> {
        if self.stats.images >= self.budget.images {
            return Err(ImageAssetError::OverBudget);
        }
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                let index = NEXT_IMAGE_INDEX
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |index| {
                        index.checked_add(1).filter(|next| *next != u32::MAX)
                    })
                    .map_err(|_| ImageAssetError::OverBudget)?;
                self.slots.insert(
                    index,
                    Slot {
                        generation: 0,
                        phase: None,
                        preview: false,
                        detail: Detail::STANDARD,
                        retention: None,
                        ready_at: None,
                    },
                );
                index
            }
        };
        let slot = self.slots.get_mut(&index).unwrap();
        slot.phase = Some(phase);
        slot.preview = preview;
        slot.detail = Detail::STANDARD;
        slot.retention = None;
        self.stats.images += 1;
        self.revision = self.revision.wrapping_add(1);
        Ok(ImageId::from_raw(RawId {
            index,
            generation: slot.generation,
        }))
    }
    fn release(&mut self, id: ImageId) -> Option<crate::mailbox::Wake> {
        let phase = self.slot_mut(id)?.phase.take()?;
        if let Some(position) = self.queue.iter().position(|job| job.id == id) {
            let job = self.queue.remove(position).unwrap();
            self.stats.pending_bytes -= job.encoded.len();
        }

        if let Phase::Ready(data) = phase {
            self.stats.decoded_bytes -= data.bytes();
        }
        self.stats.images -= 1;
        let slot = self.slots.get_mut(&id.raw().index).unwrap();
        if slot.generation < aqua_core::scene::LAYER_MAX_GENERATION {
            slot.generation += 1;
            self.free.push(id.raw().index);
        }
        self.revision = self.revision.wrapping_add(1);
        self.wake.clone()
    }
}

impl Default for ImageAssets {
    fn default() -> Self {
        Self::new(ImageAssetBudget::default())
    }
}

impl ImageAssets {
    pub fn set_fonts(&self, fonts: aqua_text::SharedFonts) {
        lock(&self.state).fonts = Some(fonts);
    }
    pub fn new(budget: ImageAssetBudget) -> Self {
        let subscribers = Arc::new(Mutex::new(Subscribers::default()));
        let weak = Arc::downgrade(&subscribers);
        let wake: crate::mailbox::Wake = Arc::new(move || {
            if let Some(subscribers) = weak.upgrade() {
                let callbacks: Vec<_> = subscribers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .callbacks
                    .values()
                    .cloned()
                    .collect();
                for callback in callbacks {
                    callback();
                }
            }
        });
        Self {
            subscribers,
            observers: Rc::new(RefCell::new(Observers::default())),
            state: Arc::new(Mutex::new(State {
                queue: VecDeque::new(),
                slots: HashMap::new(),
                free: Vec::new(),
                budget,
                stats: ImageAssetStats::default(),
                revision: 0,
                wake: Some(wake),
                fonts: None,
            })),
        }
    }
    pub fn status(
        &self,
        scope: &aqua_reactive::Scope,
        image: &ImageHandle,
    ) -> aqua_reactive::ReadSignal<ImageStatus> {
        let initial = image.status();
        let (read, write) = scope.signal(initial.clone());
        let alive = Rc::new(Cell::new(true));
        let cleanup = alive.clone();
        scope.on_cleanup(move || cleanup.set(false));
        self.observers.borrow_mut().entries.push(Observer {
            alive,
            handle: Arc::downgrade(&image.lease),
            last: initial,
            update: Box::new(move |status| write.try_set(status).is_ok()),
        });
        read
    }

    pub fn flush_notifications(&self) {
        let entries = std::mem::take(&mut self.observers.borrow_mut().entries);
        let mut retained = Vec::new();
        for mut observer in entries {
            if !observer.alive.get() {
                continue;
            }
            let Some(lease) = observer.handle.upgrade() else {
                (observer.update)(ImageStatus::Cancelled);
                continue;
            };
            let status = ImageHandle { lease }.status();
            if status != observer.last {
                observer.last = status.clone();
                if !(observer.update)(status) {
                    continue;
                }
            }
            if observer.alive.get() {
                retained.push(observer);
            }
        }
        self.observers.borrow_mut().entries.extend(retained);
    }

    pub fn stats(&self) -> ImageAssetStats {
        lock(&self.state).stats
    }
    pub fn insert(&self, image: DecodedImage) -> Result<ImageHandle, ImageAssetError> {
        let data = Data::Still(Arc::new(image));
        let bytes = data.bytes();
        let (id, wake) = {
            let mut state = lock(&self.state);
            if state.stats.decoded_bytes.saturating_add(bytes) > state.budget.decoded_bytes {
                return Err(ImageAssetError::OverBudget);
            }
            let id = state.allocate(Phase::Ready(data), false)?;
            state.stats.decoded_bytes += bytes;
            (id, state.wake.clone())
        };
        if let Some(wake) = wake {
            wake();
        }
        Ok(ImageHandle {
            lease: Arc::new(Lease {
                id,
                state: Arc::downgrade(&self.state),
            }),
        })
    }
    pub fn decode(&self, encoded: impl Into<Arc<[u8]>>) -> Result<ImageHandle, ImageAssetError> {
        self.enqueue(encoded.into(), false, None)
    }
    pub fn decode_preview(
        &self,
        encoded: impl Into<Arc<[u8]>>,
    ) -> Result<ImageHandle, ImageAssetError> {
        self.enqueue(encoded.into(), true, None)
    }
    pub fn decode_preview_with_guard(
        &self,
        encoded: impl Into<Arc<[u8]>>,
        guard: impl Send + 'static,
    ) -> Result<ImageHandle, ImageAssetError> {
        self.enqueue(encoded.into(), true, Some(Box::new(guard)))
    }
    fn enqueue(
        &self,
        encoded: Arc<[u8]>,
        preview: bool,
        guard: Option<Box<dyn Send>>,
    ) -> Result<ImageHandle, ImageAssetError> {
        let (id, start_worker) = {
            let mut state = lock(&self.state);
            if state.budget.workers == 0
                || state.stats.pending_bytes.saturating_add(encoded.len())
                    > state.budget.pending_bytes
            {
                return Err(ImageAssetError::OverBudget);
            }
            let id = state.allocate(Phase::Pending, preview)?;
            state.stats.pending_bytes += encoded.len();
            state.queue.push_back(DecodeJob {
                id,
                encoded,
                preview,
                detail: Detail::STANDARD,
                _guard: guard,
            });
            let start = state.stats.workers < state.budget.workers;
            if start {
                state.stats.workers += 1;
            }
            (id, start)
        };
        if start_worker {
            spawn_worker(&self.state);
        }
        Ok(ImageHandle {
            lease: Arc::new(Lease {
                id,
                state: Arc::downgrade(&self.state),
            }),
        })
    }
    fn ready(&self, id: ImageId) -> Option<Data> {
        match &lock(&self.state).slot(id)?.phase {
            Some(Phase::Ready(data)) => Some(data.clone()),
            _ => None,
        }
    }
    fn has_data(&self, id: ImageId) -> bool {
        lock(&self.state).slot(id).is_some_and(|slot| {
            matches!(slot.phase, Some(Phase::Ready(_) | Phase::Retired))
                || (matches!(slot.phase, Some(Phase::Pending)) && slot.retention.is_some())
        })
    }
    fn retire(&self, id: ImageId) {
        let mut state = lock(&self.state);
        let Some(slot) = state.slot_mut(id) else {
            return;
        };
        let retired = match (&slot.phase, &slot.retention) {
            (Some(Phase::Ready(data @ Data::Still(_))), Some(_))
                if data.bytes() >= RETIRE_MIN_BYTES =>
            {
                Some(data.bytes())
            }
            _ => None,
        };
        if let Some(bytes) = retired {
            slot.phase = Some(Phase::Retired);
            state.stats.decoded_bytes -= bytes;
        }
    }
    fn retire_idle(&self, keep: &std::collections::HashSet<ImageId>, idle: std::time::Duration) {
        let candidates: Vec<ImageId> = {
            let state = lock(&self.state);
            state
                .slots
                .iter()
                .filter(|(_, slot)| {
                    matches!(slot.phase, Some(Phase::Ready(Data::Still(_))))
                        && slot.retention.is_some()
                        && slot.ready_at.is_some_and(|at| at.elapsed() >= idle)
                })
                .map(|(index, slot)| {
                    ImageId::from_raw(RawId {
                        index: *index,
                        generation: slot.generation,
                    })
                })
                .filter(|id| !keep.contains(id))
                .collect()
        };
        candidates.into_iter().for_each(|id| self.retire(id));
    }
    fn revive(&self, id: ImageId, detail: Detail) {
        self.redecode(id, detail, false);
    }
    fn redetail(&self, id: ImageId, detail: Detail) {
        self.redecode(id, detail, true);
    }
    fn redecode(&self, id: ImageId, detail: Detail, replace_ready: bool) {
        let start = {
            let mut state = lock(&self.state);
            let Some(slot) = state.slot_mut(id) else {
                return;
            };
            let Some(retention) = &slot.retention else {
                return;
            };
            let unchanged = retention
                .source
                .is_none_or(|source| detail.fit(source) == (retention.width, retention.height));
            let released = match &slot.phase {
                Some(Phase::Retired) if !(replace_ready && unchanged) => 0,
                Some(Phase::Ready(data)) if replace_ready && slot.preview && !unchanged => {
                    data.bytes()
                }
                _ => return,
            };
            let encoded = retention.encoded.clone();
            let preview = slot.preview;
            slot.phase = Some(Phase::Pending);
            slot.detail = detail;
            state.stats.decoded_bytes -= released;
            state.stats.pending_bytes += encoded.len();
            state.queue.push_back(DecodeJob {
                id,
                encoded,
                preview,
                detail,
                _guard: None,
            });
            let start = state.stats.workers < state.budget.workers;
            if start {
                state.stats.workers += 1;
            }
            start
        };
        if start {
            spawn_worker(&self.state);
        }
    }
    fn extent(&self, id: ImageId) -> Option<Extent> {
        let state = lock(&self.state);
        let slot = state.slot(id)?;
        let retention = slot.retention.as_ref().filter(|_| slot.preview)?;
        Some(Extent {
            source: retention.source?,
            decoded: (retention.width, retention.height),
            settled: !matches!(slot.phase, Some(Phase::Pending)),
        })
    }
    fn is_preview(&self, id: ImageId) -> bool {
        lock(&self.state).slot(id).is_some_and(|slot| slot.preview)
    }
    fn budget(&self) -> ImageAssetBudget {
        lock(&self.state).budget
    }
    pub fn subscribe(&self, wake: crate::mailbox::Wake) -> ImageSubscription {
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let id = subscribers.next;
        subscribers.next = subscribers
            .next
            .checked_add(1)
            .expect("image subscription identity exhausted");
        subscribers.callbacks.insert(id, wake);
        ImageSubscription {
            id,
            subscribers: Arc::downgrade(&self.subscribers),
        }
    }
    fn fail_upload(&self, id: ImageId, error: String) {
        let wake = {
            let mut state = lock(&self.state);
            if let Some(slot) = state.slot_mut(id) {
                if let Some(Phase::Ready(data)) = slot
                    .phase
                    .replace(Phase::Failed(ImageAssetError::Upload(error)))
                {
                    state.stats.decoded_bytes -= data.bytes();
                    state.revision = state.revision.wrapping_add(1);
                }
            }
            state.wake.clone()
        };
        if let Some(wake) = wake {
            wake();
        }
    }
}

fn spawn_worker(shared: &Arc<Mutex<State>>) {
    let weak = Arc::downgrade(shared);
    let worker = std::thread::Builder::new()
        .name("aqua-image-decode".into())
        .spawn(move || loop {
            let (job, budget, fonts) = {
                let Some(shared) = weak.upgrade() else {
                    return;
                };
                let mut state = lock(&shared);
                let Some(job) = state.queue.pop_front() else {
                    state.stats.workers -= 1;
                    return;
                };
                (job, state.budget.decoded_bytes, state.fonts.clone())
            };
            #[cfg(test)]
            let callback = DECODE_STARTED
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            #[cfg(test)]
            if let Some(callback) = callback {
                callback(job.id);
            }
            let whole = |data| Decoded { data, source: None };
            let result = if aqua_media::decode::is_svg(&job.encoded) {
                let source = match &fonts {
                    Some(fonts) => aqua_text::svg::outline_text(&job.encoded, &mut fonts.lock()),
                    None => std::borrow::Cow::Borrowed(&job.encoded[..]),
                };
                aqua_media::decode::decode_svg(&source, budget)
                    .map(|image| whole(Data::Still(Arc::new(image))))
                    .map_err(|error| ImageAssetError::Decode(error.to_string()))
            } else if job.preview && !aqua_media::decode::is_gif(&job.encoded) {
                aqua_media::decode::decode_still(&job.encoded, job.detail, budget)
                    .map(|still| Decoded {
                        data: Data::Still(Arc::new(still.image)),
                        source: Some(still.source),
                    })
                    .map_err(|error| ImageAssetError::Decode(error.to_string()))
            } else {
                let result = if job.preview {
                    aqua_media::decode::animation::decode_preview_gif(&job.encoded, budget)
                } else {
                    decode_animation(
                        &job.encoded,
                        AnimationBudget {
                            max_bytes: budget.min(64 * 1024 * 1024),
                            ..Default::default()
                        },
                    )
                };
                result
                    .map(|animation| {
                        whole(if animation.frames().len() == 1 {
                            Data::Still(Arc::new(animation.frames()[0].image.clone()))
                        } else {
                            Data::Animation(Arc::new(animation))
                        })
                    })
                    .map_err(|error| ImageAssetError::Decode(error.to_string()))
            };
            if let Some(shared) = weak.upgrade() {
                complete(&shared, job.id, job.encoded, result);
            }
        });
    if let Err(error) = worker {
        let wake = {
            let mut state = lock(shared);
            state.stats.workers -= 1;
            while let Some(job) = state.queue.pop_front() {
                state.stats.pending_bytes -= job.encoded.len();
                if let Some(slot) = state.slot_mut(job.id) {
                    slot.phase = Some(Phase::Failed(ImageAssetError::Worker(error.to_string())));
                }
            }
            state.wake.clone()
        };
        if let Some(wake) = wake {
            wake();
        }
    }
}

fn complete(
    shared: &Mutex<State>,
    id: ImageId,
    encoded: Arc<[u8]>,
    result: Result<Decoded, ImageAssetError>,
) {
    let wake = {
        let mut state = lock(shared);
        state.stats.pending_bytes -= encoded.len();
        let result = result.and_then(|decoded| {
            if state
                .stats
                .decoded_bytes
                .saturating_add(decoded.data.bytes())
                <= state.budget.decoded_bytes
            {
                Ok(decoded)
            } else {
                Err(ImageAssetError::OverBudget)
            }
        });
        if state.slot(id).is_some() {
            let retention = match &result {
                Ok(Decoded {
                    data: Data::Still(image),
                    source,
                }) => Some(Retention {
                    encoded,
                    width: image.width(),
                    height: image.height(),
                    source: *source,
                }),
                _ => None,
            };
            let phase = match result {
                Ok(decoded) => {
                    state.stats.decoded_bytes += decoded.data.bytes();
                    Phase::Ready(decoded.data)
                }
                Err(error) => Phase::Failed(error),
            };
            let slot = state.slot_mut(id).unwrap();
            slot.phase = Some(phase);
            slot.retention = retention;
            slot.ready_at = Some(std::time::Instant::now());
            state.revision = state.revision.wrapping_add(1);
            state.wake.clone()
        } else {
            None
        }
    };
    if let Some(wake) = wake {
        wake();
    }
}

impl ImageHandle {
    pub fn id(&self) -> ImageId {
        self.lease.id
    }

    pub fn downgrade(&self) -> WeakImageHandle {
        WeakImageHandle {
            lease: Arc::downgrade(&self.lease),
        }
    }

    pub fn decoded_bytes(&self) -> Option<u64> {
        let shared = self.lease.state.upgrade()?;
        let state = lock(&shared);
        match state.slot(self.id())?.phase.as_ref()? {
            Phase::Ready(data) => Some(data.bytes()),
            Phase::Retired => state
                .slot(self.id())?
                .retention
                .as_ref()
                .map(|retention| u64::from(retention.width) * u64::from(retention.height) * 4),
            Phase::Pending | Phase::Failed(_) => None,
        }
    }
    pub fn cancel(&self) {
        if let Some(state) = self.lease.state.upgrade() {
            let wake = lock(&state).release(self.id());
            if let Some(wake) = wake {
                wake();
            }
        }
    }

    pub fn status(&self) -> ImageStatus {
        let Some(shared) = self.lease.state.upgrade() else {
            return ImageStatus::Cancelled;
        };
        let state = lock(&shared);
        let retention = state
            .slot(self.id())
            .and_then(|slot| slot.retention.as_ref());
        let extent = |retention: Option<&Retention>| match retention {
            Some(retention) => {
                let (width, height) = retention
                    .source
                    .unwrap_or((retention.width, retention.height));
                ImageStatus::Ready { width, height }
            }
            None => ImageStatus::Pending,
        };
        match state.slot(self.id()).and_then(|slot| slot.phase.as_ref()) {
            Some(Phase::Pending | Phase::Retired) => extent(retention),
            Some(Phase::Ready(data)) => match retention {
                Some(retention) => extent(Some(retention)),
                None => {
                    let first = data.first();
                    ImageStatus::Ready {
                        width: first.width(),
                        height: first.height(),
                    }
                }
            },
            Some(Phase::Failed(error)) => ImageStatus::Failed(error.clone()),
            None => ImageStatus::Cancelled,
        }
    }
}

impl WeakImageHandle {
    pub fn upgrade(&self) -> Option<ImageHandle> {
        let lease = self.lease.upgrade()?;
        let state = lease.state.upgrade()?;
        lock(&state).slot(lease.id)?;
        Some(ImageHandle { lease })
    }
}
impl From<ImageHandle> for ImageRef {
    fn from(handle: ImageHandle) -> Self {
        ImageRef::with_lease(handle.id(), handle.lease)
    }
}
impl std::fmt::Debug for ImageHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ImageHandle")
            .field(&self.id())
            .field(&self.status())
            .finish()
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            let wake = lock(&state).release(self.id);
            if let Some(wake) = wake {
                wake();
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/images/guard.rs"]
mod guard_tests;

#[cfg(test)]
#[path = "../tests/images/retire.rs"]
mod retire_tests;
