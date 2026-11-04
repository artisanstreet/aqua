use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use aqua_core::{ImageId, ImageRef, RawId};
use aqua_render::media::{
    DecodedImage,
    animation::{AnimationBudget, DecodedAnimation, decode_animation},
};

static NEXT_IMAGE_INDEX: AtomicU32 = AtomicU32::new(0x8000_0000);

mod application;
pub use application::ManagedUiApp;

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
            decoded_bytes: u64::MAX,
            pending_bytes: usize::MAX,
            images: usize::MAX,
            workers: usize::MAX,
            gpu_bytes: u64::MAX,
            gpu_images: usize::MAX,
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
    Failed(ImageAssetError),
}
struct Slot {
    generation: u32,
    phase: Option<Phase>,
    preview: bool,
}
struct DecodeJob {
    id: ImageId,
    encoded: Arc<[u8]>,
    preview: bool,
    _guard: Option<Box<dyn Send>>,
}

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
                    },
                );
                index
            }
        };
        let slot = self.slots.get_mut(&index).unwrap();
        slot.phase = Some(phase);
        slot.preview = preview;
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
        .spawn(move || {
            loop {
                let (job, budget) = {
                    let Some(shared) = weak.upgrade() else {
                        return;
                    };
                    let mut state = lock(&shared);
                    let Some(job) = state.queue.pop_front() else {
                        state.stats.workers -= 1;
                        return;
                    };
                    (job, state.budget.decoded_bytes)
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
                let result = if job.preview && job.encoded.len() > 32 * 1024 * 1024 {
                    Err(ImageAssetError::Decode(
                        "image source exceeds 32 MiB".into(),
                    ))
                } else if aqua_render::media::is_svg(&job.encoded) {
                    aqua_render::media::decode_svg(&job.encoded, budget)
                        .map(|image| Data::Still(Arc::new(image)))
                        .map_err(|error| ImageAssetError::Decode(error.to_string()))
                } else if job.preview && !aqua_render::media::is_gif(&job.encoded) {
                    aqua_render::media::decode_preview_still(&job.encoded, budget)
                        .map(|image| Data::Still(Arc::new(image)))
                        .map_err(|error| ImageAssetError::Decode(error.to_string()))
                } else {
                    let result = if job.preview {
                        aqua_render::media::animation::decode_preview_gif(&job.encoded, budget)
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
                            if animation.frames().len() == 1 {
                                Data::Still(Arc::new(animation.frames()[0].image.clone()))
                            } else {
                                Data::Animation(Arc::new(animation))
                            }
                        })
                        .map_err(|error| ImageAssetError::Decode(error.to_string()))
                };
                if let Some(shared) = weak.upgrade() {
                    complete(&shared, job.id, job.encoded.len(), result);
                }
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
    pending: usize,
    result: Result<Data, ImageAssetError>,
) {
    let wake = {
        let mut state = lock(shared);
        state.stats.pending_bytes -= pending;
        let result = result.and_then(|data| {
            if state.stats.decoded_bytes.saturating_add(data.bytes()) <= state.budget.decoded_bytes
            {
                Ok(data)
            } else {
                Err(ImageAssetError::OverBudget)
            }
        });
        if state.slot(id).is_some() {
            let phase = match result {
                Ok(data) => {
                    state.stats.decoded_bytes += data.bytes();
                    Phase::Ready(data)
                }
                Err(error) => Phase::Failed(error),
            };
            state.slot_mut(id).unwrap().phase = Some(phase);
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
        match state.slot(self.id()).and_then(|slot| slot.phase.as_ref()) {
            Some(Phase::Pending) => ImageStatus::Pending,
            Some(Phase::Ready(data)) => {
                let first = data.first();
                ImageStatus::Ready {
                    width: first.width(),
                    height: first.height(),
                }
            }
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
