use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

use aqua_core::DisplayItem;
use aqua_core::{ImageId, InputEvent, NodeId, Rect};
use aqua_protocol::{A11yActionRequest, AquaWindowId, ObservedWindowState, WindowEvent};
use aqua_protocol::{FrameReason, Instant};
use aqua_runtime::Runtime;

use crate::{
    mailbox::Mailbox,
    runtime::{AppCtx, Application, ExternalFrame},
    ui_app::UiApp,
};

use aqua_core::image::ImageFit;
use aqua_media::decode::Detail;

use super::{Data, Extent, ImageAssetError, ImageAssets};

pub struct ImageFrame {
    pub layer: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

pub struct ManagedUiApp {
    ui: UiApp,
    images: ImageAssets,
    mailbox: Mailbox<()>,
    _subscription: super::ImageSubscription,
    states: HashMap<Option<AquaWindowId>, ImageState>,
}

struct PreviewInstance {
    source: ImageId,
    layer: ImageId,
    frame: usize,
    last_frame_time: Option<Instant>,
    observed: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Shown {
    width: f32,
    height: f32,
    fit: ImageFit,
    crop: (f32, f32),
}

impl Shown {
    fn long_side(self, source: (u32, u32)) -> Option<f32> {
        let (width, height) = (source.0.max(1) as f32, source.1.max(1) as f32);
        let across = self.width / (width * self.crop.0);
        let down = self.height / (height * self.crop.1);
        let scale = match self.fit {
            ImageFit::Contain => across.min(down),
            ImageFit::Fill | ImageFit::Cover => across.max(down),
            ImageFit::None | ImageFit::ScaleDown => return None,
        };
        Some(width.max(height) * scale)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Resident {
    frame: usize,
    width: u32,
    height: u32,
}

impl Resident {
    fn bytes(self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 4
    }
}

fn wanted_detail(shown: &[Shown], extent: Extent) -> Option<Detail> {
    shown
        .iter()
        .map(|shown| shown.long_side(extent.source))
        .try_fold(0.0f32, |longest, side| side.map(|side| longest.max(side)))
        .map(|longest| Detail::covering(longest).max(Detail::STANDARD))
}

fn affordable_detail(wanted: Detail, source: (u32, u32), available: u64) -> Option<Detail> {
    std::iter::successors(Some(wanted), |detail| detail.lower()).find(|detail| {
        let (width, height) = detail.fit(source);
        u64::from(width) * u64::from(height) * 4 <= available
    })
}

#[derive(Default)]
struct ImageState {
    resident: HashMap<ImageId, Resident>,
    visible: HashMap<ImageId, Vec<Shown>>,
    ceiling: HashMap<ImageId, Detail>,
    scene_revision: Option<(u64, aqua_core::Size, f32)>,
    started: HashMap<ImageId, Instant>,
    preview_instances: HashMap<NodeId, PreviewInstance>,
    preview_layers: HashMap<ImageId, NodeId>,
    active: bool,
    prepared: Option<(Instant, Option<Instant>)>,
    released: Vec<u64>,
}

impl ImageState {
    fn source_id(&self, id: ImageId) -> ImageId {
        self.preview_layers
            .get(&id)
            .and_then(|node| self.preview_instances.get(node))
            .map_or(id, |instance| instance.source)
    }

    fn gpu_image_bytes(&self) -> u64 {
        self.resident
            .values()
            .map(|resident| resident.bytes())
            .sum()
    }

    fn prepare_preview_instances(&mut self, rt: &mut Runtime, images: &ImageAssets) {
        let found: Vec<_> = rt
            .scene()
            .ordered()
            .filter_map(|(node, item)| {
                let DisplayItem::Image { layer, .. } = item else {
                    return None;
                };
                let (aqua_core::scene::LayerKind::Media, raw) =
                    aqua_core::scene::decode_layer(*layer)?
                else {
                    return None;
                };
                let id = ImageId::from_raw(raw);
                let source = self.source_id(id);
                if !images.is_preview(source)
                    || !matches!(images.ready(source), Some(Data::Animation(_)))
                {
                    return None;
                }
                Some((node, source, item.clone()))
            })
            .collect();
        let live: HashSet<_> = found.iter().map(|(node, _, _)| *node).collect();
        let stale: Vec<_> = self
            .preview_instances
            .keys()
            .filter(|node| !live.contains(node))
            .copied()
            .collect();
        for node in stale {
            if let Some(instance) = self.preview_instances.remove(&node) {
                self.preview_layers.remove(&instance.layer);
                self.release_gpu(instance.layer);
            }
        }
        for (node, source, mut item) in found {
            let replaced = self
                .preview_instances
                .get(&node)
                .is_some_and(|instance| instance.source != source);
            if replaced {
                if let Some(instance) = self.preview_instances.remove(&node) {
                    self.preview_layers.remove(&instance.layer);
                    self.release_gpu(instance.layer);
                }
            }
            if !self.preview_instances.contains_key(&node) {
                let Ok(index) = super::NEXT_IMAGE_INDEX.fetch_update(
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                    |index| index.checked_add(1).filter(|next| *next != u32::MAX),
                ) else {
                    continue;
                };
                let layer = ImageId::from_raw(aqua_core::RawId {
                    index,
                    generation: 0,
                });
                self.preview_layers.insert(layer, node);
                self.preview_instances.insert(
                    node,
                    PreviewInstance {
                        source,
                        layer,
                        frame: 0,
                        last_frame_time: None,
                        observed: None,
                    },
                );
            }
            if let (Some(instance), DisplayItem::Image { layer, .. }) =
                (self.preview_instances.get(&node), &mut item)
            {
                *layer = aqua_core::scene::media_layer(instance.layer);
                rt.scene_mut().update(node, item);
            }
        }
    }

    fn collect_visible(&mut self, rt: &Runtime) {
        let key = (rt.scene().revision(), rt.viewport(), rt.scale());
        if self.scene_revision == Some(key) {
            return;
        }
        self.visible.clear();
        let viewport = rt.viewport();
        let density = rt.scale().max(f32::EPSILON);
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            w: viewport.w,
            h: viewport.h,
        };
        let mut transforms = vec![aqua_core::Transform2D::IDENTITY];
        let mut clips = vec![viewport];
        let mut opacities = vec![1.0f32];
        for (_, item) in rt.scene().ordered() {
            let transform = *transforms.last().unwrap();
            let world = |bounds: Rect| Rect {
                x: bounds.x * transform.a + transform.tx,
                y: bounds.y * transform.d + transform.ty,
                w: bounds.w * transform.a,
                h: bounds.h * transform.d,
            };
            match item {
                DisplayItem::PushOpacity(opacity) => {
                    opacities.push(opacities.last().unwrap() * opacity.max(0.0))
                }
                DisplayItem::PopOpacity => {
                    if opacities.len() > 1 {
                        opacities.pop();
                    }
                }
                DisplayItem::PushTransform(local) => {
                    transforms.push(local.then(transform));
                }
                DisplayItem::PopTransform => {
                    if transforms.len() > 1 {
                        transforms.pop();
                    }
                }
                DisplayItem::PushClip { bounds, .. }
                | DisplayItem::PushRoundedClip { bounds, .. } => {
                    clips.push(clips.last().unwrap().intersect(world(*bounds)))
                }
                DisplayItem::PopClip => {
                    if clips.len() > 1 {
                        clips.pop();
                    }
                }
                DisplayItem::Image {
                    bounds,
                    layer,
                    fit,
                    source,
                    opacity,
                    ..
                } if *opacities.last().unwrap() > 0.0
                    && *opacity > 0.0
                    && !world(*bounds).intersect(*clips.last().unwrap()).is_empty() =>
                {
                    if let Some((aqua_core::scene::LayerKind::Media, raw)) =
                        aqua_core::scene::decode_layer(*layer)
                    {
                        let placed = world(*bounds);
                        self.visible
                            .entry(ImageId::from_raw(raw))
                            .or_default()
                            .push(Shown {
                                width: placed.w.abs() * density,
                                height: placed.h.abs() * density,
                                fit: *fit,
                                crop: (source.w.max(f32::EPSILON), source.h.max(f32::EPSILON)),
                            });
                    }
                }
                DisplayItem::External {
                    bounds,
                    layer,
                    opacity,
                    ..
                } if *opacities.last().unwrap() > 0.0
                    && *opacity > 0.0
                    && !world(*bounds).intersect(*clips.last().unwrap()).is_empty() =>
                {
                    if let Some((aqua_core::scene::LayerKind::Media, raw)) =
                        aqua_core::scene::decode_layer(*layer)
                    {
                        self.visible.entry(ImageId::from_raw(raw)).or_default();
                    }
                }
                _ => {}
            }
        }
        self.scene_revision = Some(key);
    }

    fn release_gpu(&mut self, id: ImageId) {
        if self.resident.remove(&id).is_some() {
            self.released.push(aqua_core::scene::media_layer(id));
        }
    }

    fn selected_frame(
        &mut self,
        id: ImageId,
        data: &Data,
        now: Instant,
    ) -> (usize, Option<Instant>) {
        match data {
            Data::Still(_) => (0, None),
            Data::Animation(animation) => {
                if let Some(node) = self.preview_layers.get(&id) {
                    let instance = self.preview_instances.get_mut(node).unwrap();
                    instance.frame = instance.frame.min(animation.frames().len() - 1);
                    if instance.observed != Some(now) {
                        instance.observed = Some(now);
                        if self.active && animation.frames().len() > 1 {
                            if let Some(last) = instance.last_frame_time {
                                let elapsed = now.saturating_duration_since(last);
                                let delay = animation.frames()[instance.frame].delay;
                                if elapsed >= delay {
                                    instance.frame =
                                        (instance.frame + 1) % animation.frames().len();
                                    instance.last_frame_time = Some(now - (elapsed - delay));
                                }
                            } else {
                                instance.last_frame_time = Some(now);
                            }
                        } else {
                            instance.last_frame_time = None;
                        }
                    }
                    let deadline = instance
                        .last_frame_time
                        .map(|last| last + animation.frames()[instance.frame].delay);
                    return (instance.frame, deadline);
                }
                let start = *self.started.entry(id).or_insert(now);
                let sample = animation.sample(now.saturating_duration_since(start));
                (
                    sample.frame,
                    sample.next_frame_at.map(|elapsed| start + elapsed),
                )
            }
        }
    }

    fn synchronize(
        &mut self,
        rt: &Runtime,
        images: &ImageAssets,
        other_bytes: u64,
        now: Instant,
        keep: &mut HashSet<ImageId>,
    ) -> Result<Vec<ImageFrame>, ImageAssetError> {
        self.collect_visible(rt);
        let budget = images.budget();
        let released: Vec<_> = self
            .resident
            .keys()
            .filter(|id| !self.visible.contains_key(id) || !images.has_data(self.source_id(**id)))
            .copied()
            .collect();
        for id in released {
            self.release_gpu(id);
        }
        let sources: HashSet<_> = self.visible.keys().map(|id| self.source_id(*id)).collect();
        self.ceiling.retain(|source, _| sources.contains(source));
        let mut frames = Vec::new();
        let mut failure = None;
        let visible: Vec<_> = self.visible.keys().copied().collect();
        for id in visible {
            let source = self.source_id(id);
            let extent = images.extent(source);
            let wanted = extent
                .and_then(|extent| wanted_detail(&self.visible[&id], extent))
                .map(|wanted| match self.ceiling.get(&source) {
                    Some(ceiling) => wanted.min(*ceiling),
                    None => wanted,
                });
            let Some(data) = images.ready(source) else {
                if !self.resident.contains_key(&id) {
                    images.revive(source, wanted.unwrap_or(Detail::STANDARD));
                } else if let Some(wanted) = wanted {
                    images.redetail(source, wanted);
                }
                continue;
            };
            let (index, _) = self.selected_frame(id, &data, now);
            let image = match &data {
                Data::Still(image) => image.as_ref(),
                Data::Animation(animation) => &animation.frames()[index].image,
            };
            let shown = Resident {
                frame: index,
                width: image.width(),
                height: image.height(),
            };
            if self.resident.get(&id) == Some(&shown) {
                if let Some(wanted) = wanted {
                    images.redetail(source, wanted);
                }
                continue;
            }
            let previous_bytes = self
                .resident
                .get(&id)
                .map_or(0, |resident| resident.bytes());
            let new_count = self.resident.len() + usize::from(!self.resident.contains_key(&id));
            let available = budget
                .gpu_bytes
                .saturating_sub(self.gpu_image_bytes().saturating_sub(previous_bytes))
                .saturating_sub(other_bytes);
            if shown.bytes() > available || new_count > budget.gpu_images {
                let smaller =
                    extent
                        .filter(|_| new_count <= budget.gpu_images)
                        .and_then(|extent| {
                            affordable_detail(
                                wanted.unwrap_or(Detail::STANDARD),
                                extent.source,
                                available,
                            )
                            .filter(|detail| detail.fit(extent.source) != extent.decoded)
                        });
                match smaller {
                    Some(detail) => {
                        self.ceiling.insert(source, detail);
                        images.redetail(source, detail);
                    }
                    None => {
                        images.fail_upload(source, "visible image GPU budget exceeded".into());
                        self.release_gpu(id);
                        failure = Some(ImageAssetError::OverBudget);
                    }
                }
                continue;
            }
            frames.push(ImageFrame {
                layer: aqua_core::scene::media_layer(id),
                width: image.width(),
                height: image.height(),
                rgba: Arc::from(image.pixels()),
            });
            self.resident.insert(id, shown);
            match wanted.filter(|wanted| {
                extent.is_some_and(|extent| wanted.fit(extent.source) != extent.decoded)
            }) {
                Some(wanted) => images.redetail(source, wanted),
                None => images.retire(source),
            }
        }
        self.started.retain(|id, _| images.has_data(*id));
        keep.extend(self.visible.keys().map(|id| self.source_id(*id)));
        keep.extend(self.resident.keys().map(|id| self.source_id(*id)));
        match failure {
            Some(error) if frames.is_empty() => Err(error),
            _ => Ok(frames),
        }
    }
}

type WindowKey = Option<AquaWindowId>;

fn runtime_of(ui: &UiApp, key: WindowKey) -> Option<&Runtime> {
    match key {
        None => Some(&**ui),
        Some(id) => ui.runtime_for(id),
    }
}

fn runtime_of_mut(ui: &mut UiApp, key: WindowKey) -> Option<&mut Runtime> {
    match key {
        None => Some(&mut **ui),
        Some(id) => ui.runtime_for_mut(id),
    }
}

impl ManagedUiApp {
    pub fn new(ui: UiApp, images: ImageAssets) -> Self {
        let mailbox = Mailbox::new(NonZeroUsize::MIN);
        let sender = mailbox.sender();
        let subscription = images.subscribe(Arc::new(move || {
            let _ = sender.send(());
        }));
        Self {
            ui,
            images,
            mailbox,
            _subscription: subscription,
            states: HashMap::new(),
        }
    }
    pub fn ui(&self) -> &UiApp {
        &self.ui
    }
    pub fn ui_mut(&mut self) -> &mut UiApp {
        &mut self.ui
    }
    pub fn images(&self) -> &ImageAssets {
        &self.images
    }
    pub fn gpu_image_bytes(&self) -> u64 {
        self.states.values().map(ImageState::gpu_image_bytes).sum()
    }

    fn key_of(&self, id: AquaWindowId) -> WindowKey {
        match self.ui.is_root_window(id) {
            true => None,
            false => Some(id),
        }
    }

    fn other_bytes(&self, key: WindowKey) -> u64 {
        self.states
            .iter()
            .filter(|(other, _)| **other != key)
            .map(|(_, state)| state.gpu_image_bytes())
            .sum()
    }

    pub fn take_released_images(&mut self) -> Vec<u64> {
        self.take_released_images_for(None)
    }

    fn take_released_images_for(&mut self, key: WindowKey) -> Vec<u64> {
        self.states
            .get_mut(&key)
            .map(|state| std::mem::take(&mut state.released))
            .unwrap_or_default()
    }

    pub fn synchronize_images(&mut self, now: Instant) -> Result<Vec<ImageFrame>, ImageAssetError> {
        self.synchronize_key(None, now)
    }

    pub fn synchronize_images_for(
        &mut self,
        id: AquaWindowId,
        now: Instant,
    ) -> Result<Vec<ImageFrame>, ImageAssetError> {
        let key = self.key_of(id);
        self.synchronize_key(key, now)
    }

    fn synchronize_key(
        &mut self,
        key: WindowKey,
        now: Instant,
    ) -> Result<Vec<ImageFrame>, ImageAssetError> {
        let other = self.other_bytes(key);
        let Self {
            ui, images, states, ..
        } = self;
        let Some(rt) = runtime_of(ui, key) else {
            return Ok(Vec::new());
        };
        let mut keep = HashSet::new();
        let frames = states
            .entry(key)
            .or_default()
            .synchronize(rt, images, other, now, &mut keep);
        states
            .iter()
            .filter(|(other_key, _)| **other_key != key)
            .for_each(|(_, state)| {
                keep.extend(state.visible.keys().map(|id| state.source_id(*id)));
                keep.extend(state.resident.keys().map(|id| state.source_id(*id)));
            });
        images.retire_idle(&keep, super::RETIRE_IDLE);
        frames
    }
}

impl Application for ManagedUiApp {
    fn input_invalidation_epoch(&self, id: AquaWindowId) -> u64 {
        self.ui.input_invalidation_epoch(id)
    }

    fn color_profile(&self) -> aqua_core::ColorProfile {
        self.ui.color_profile()
    }

    fn color_profile_for(&self, id: AquaWindowId) -> aqua_core::ColorProfile {
        self.ui.color_profile_for(id)
    }

    fn on_click_settings(&mut self, settings: aqua_core::ClickSettings) {
        self.ui.on_click_settings(settings);
    }
    fn on_start(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
        ctx.bind_mailbox(id, &self.mailbox);
        self.ui.on_start(ctx, id);
    }
    fn can_close(&mut self, ctx: &mut AppCtx, id: AquaWindowId) -> bool {
        self.ui.can_close(ctx, id)
    }
    fn on_wake(&mut self, ctx: &mut AppCtx, id: AquaWindowId) {
        self.images.flush_notifications();
        if !self.mailbox.drain().is_empty() {
            ctx.request_frame(id, FrameReason::MEDIA);
        }
        self.ui.on_wake(ctx, id);
    }
    fn on_frame_completed(&mut self, ctx: &mut AppCtx, id: AquaWindowId, sequence: u64) {
        self.ui.on_frame_completed(ctx, id, sequence);
    }
    fn cursor(&self) -> Option<aqua_core::CursorIcon> {
        None
    }
    fn cursor_for(&self, id: AquaWindowId) -> Option<aqua_core::CursorIcon> {
        self.ui.cursor_for(id)
    }
    fn a11y_nodes_for(
        &self,
        id: AquaWindowId,
    ) -> Option<(
        aqua_core::NodeId,
        aqua_core::NodeId,
        Vec<aqua_protocol::A11yNode>,
    )> {
        self.ui.a11y_nodes_for(id)
    }
    fn step_input(&mut self, id: AquaWindowId, input: aqua_protocol::RuntimeInput) {
        self.ui.step_input(id, input);
    }
    fn take_output(&mut self, id: AquaWindowId) -> aqua_protocol::RuntimeOutput {
        self.ui.take_output(id)
    }
    fn on_dialog(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        request: aqua_protocol::RequestId,
        outcome: aqua_protocol::DialogOutcome,
    ) {
        self.ui.on_dialog(ctx, id, request, outcome);
    }
    fn on_window_event(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
        let key = self.key_of(id);
        match event {
            WindowEvent::Focused(_, false) => {
                if let Some(state) = self.states.get_mut(&key) {
                    state.active = false;
                    for instance in state.preview_instances.values_mut() {
                        instance.last_frame_time = None;
                        instance.observed = None;
                    }
                }
            }
            WindowEvent::Destroyed(_) => {
                self.states.remove(&key);
            }
            _ => {}
        }
        self.ui.on_window_event(ctx, id, event);
    }
    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &InputEvent) {
        self.ui.on_input(ctx, id, event);
    }
    fn on_files_dropped(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
    ) {
        self.ui.on_files_dropped(ctx, id, paths);
    }
    fn on_files_dropped_at(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
        at: Option<aqua_core::Point>,
    ) {
        self.ui.on_files_dropped_at(ctx, id, paths, at);
    }
    fn on_external_file_drag(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        event: &crate::runtime::ExternalFileDrag,
    ) {
        self.ui.on_external_file_drag(ctx, id, event);
    }
    fn on_a11y_action(&mut self, ctx: &mut AppCtx, id: AquaWindowId, request: &A11yActionRequest) {
        self.ui.on_a11y_action(ctx, id, request);
    }
    fn prepare_accessibility(
        &mut self,
        window: &ObservedWindowState,
        now: Instant,
    ) -> Option<Instant> {
        self.ui.prepare_accessibility(window, now)
    }
    fn externals(&mut self, frame: &mut ExternalFrame<'_>) {
        let key = self.key_of(frame.window.id);
        let deadline = self.build_scene(frame.window, frame.now);
        self.states.entry(key).or_default().prepared = Some((frame.now, deadline));
        let images = self.synchronize_key(key, frame.now);
        for layer in self.take_released_images_for(key) {
            frame.unregister_external(layer);
        }
        match images {
            Ok(images) => {
                for image in images {
                    frame.register_image(image.layer, image.width, image.height, image.rgba);
                }
            }
            Err(error) => eprintln!("aqua: {error}"),
        }
    }
    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
        let key = self.key_of(window.id);
        if let Some((prepared, deadline)) = self
            .states
            .get_mut(&key)
            .and_then(|state| state.prepared.take())
        {
            if prepared == now {
                return deadline;
            }
        }
        self.images.flush_notifications();
        let mut next = self.ui.build_scene(window, now);
        let Self {
            ui, images, states, ..
        } = self;
        let Some(reduced) = runtime_of(ui, key).map(|rt| {
            matches!(
                rt.host().motion().reduced_motion(),
                aqua_ui::motion::ReducedMotion::Reduce
            )
        }) else {
            return next;
        };
        let state = states.entry(key).or_default();
        state.active = window.focused && !reduced;
        if let Some(rt) = runtime_of_mut(ui, key) {
            state.prepare_preview_instances(rt, images);
        }
        ui.flush_scene_for(window.id);
        let Some(rt) = runtime_of(ui, key) else {
            return next;
        };
        state.collect_visible(rt);
        let visible: Vec<_> = state.visible.keys().copied().collect();
        for id in visible {
            if let Some(data) = images.ready(state.source_id(id)) {
                if let (_, Some(deadline)) = state.selected_frame(id, &data, now) {
                    next = Some(next.map_or(deadline, |old| old.min(deadline)));
                }
            }
        }
        next
    }
    fn reactive_runtime(&self) -> Option<&aqua_reactive::Runtime> {
        Some(self.ui.runtime())
    }
    fn reactive_runtime_for(&self, id: AquaWindowId) -> Option<&aqua_reactive::Runtime> {
        self.ui.reactive_runtime_for(id)
    }
}
