use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

use aqua_core::DisplayItem;
use aqua_core::{ImageId, InputEvent, NodeId, Rect};
use aqua_platform::{a11y::A11yActionRequest, AquaWindowId, ObservedWindowState, WindowEvent};
use aqua_protocol::{FrameReason, Instant};

use crate::{
    mailbox::Mailbox,
    runtime::{AppCtx, Application, ExternalFrame},
    ui_app::UiApp,
};

use super::{Data, ImageAssetError, ImageAssets};

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
    resident: HashMap<ImageId, (usize, u64)>,
    visible: HashSet<ImageId>,
    scene_revision: Option<(u64, aqua_core::Size)>,
    started: HashMap<ImageId, Instant>,
    preview_instances: HashMap<NodeId, PreviewInstance>,
    preview_layers: HashMap<ImageId, NodeId>,
    active: bool,
    now: Instant,
    prepared: Option<(Instant, Option<Instant>)>,
    released: Vec<u64>,
}

struct PreviewInstance {
    source: ImageId,
    layer: ImageId,
    frame: usize,
    last_frame_time: Option<Instant>,
    observed: Option<Instant>,
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
            resident: HashMap::new(),
            visible: HashSet::new(),
            scene_revision: None,
            started: HashMap::new(),
            preview_instances: HashMap::new(),
            preview_layers: HashMap::new(),
            active: false,
            now: Instant::ZERO,
            prepared: None,
            released: Vec::new(),
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
        self.resident.values().map(|(_, bytes)| *bytes).sum()
    }

    fn source_id(&self, id: ImageId) -> ImageId {
        self.preview_layers
            .get(&id)
            .and_then(|node| self.preview_instances.get(node))
            .map_or(id, |instance| instance.source)
    }

    fn prepare_preview_instances(&mut self) {
        let images: Vec<_> = self
            .ui
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
                if !self.images.is_preview(source)
                    || !matches!(self.images.ready(source), Some(Data::Animation(_)))
                {
                    return None;
                }
                Some((node, source, item.clone()))
            })
            .collect();
        let live: HashSet<_> = images.iter().map(|(node, _, _)| *node).collect();
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
        for (node, source, mut item) in images {
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
                self.ui.scene_mut().update(node, item);
            }
        }
    }

    fn collect_visible(&mut self) {
        let key = (self.ui.scene().revision(), self.ui.viewport());
        if self.scene_revision == Some(key) {
            return;
        }
        self.visible.clear();
        let viewport = self.ui.viewport();
        let viewport = Rect {
            x: 0.0,
            y: 0.0,
            w: viewport.w,
            h: viewport.h,
        };
        let mut transforms = vec![aqua_core::Transform2D::IDENTITY];
        let mut clips = vec![viewport];
        let mut opacities = vec![1.0f32];
        for (_, item) in self.ui.scene().ordered() {
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
                    opacity,
                    ..
                }
                | DisplayItem::External {
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
                        self.visible.insert(ImageId::from_raw(raw));
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

    pub fn take_released_images(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.released)
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

    pub fn synchronize_images(&mut self, now: Instant) -> Result<Vec<ImageFrame>, ImageAssetError> {
        self.collect_visible();
        let budget = self.images.budget();
        let released: Vec<_> = self
            .resident
            .keys()
            .filter(|id| {
                !self.visible.contains(id) || self.images.ready(self.source_id(**id)).is_none()
            })
            .copied()
            .collect();
        for id in released {
            self.release_gpu(id);
        }
        let mut frames = Vec::new();
        let mut failure = None;
        let visible: Vec<_> = self.visible.iter().copied().collect();
        for id in visible {
            let source = self.source_id(id);
            let Some(data) = self.images.ready(source) else {
                continue;
            };
            let (index, _) = self.selected_frame(id, &data, now);
            if self.resident.get(&id).map(|(frame, _)| *frame) == Some(index) {
                continue;
            }
            let image = match &data {
                Data::Still(image) => image.as_ref(),
                Data::Animation(animation) => &animation.frames()[index].image,
            };
            let bytes = u64::from(image.width()) * u64::from(image.height()) * 4;
            let previous_bytes = self.resident.get(&id).map_or(0, |(_, bytes)| *bytes);
            let new_count = self.resident.len() + usize::from(!self.resident.contains_key(&id));
            if self
                .gpu_image_bytes()
                .saturating_sub(previous_bytes)
                .saturating_add(bytes)
                > budget.gpu_bytes
                || new_count > budget.gpu_images
            {
                self.images
                    .fail_upload(source, "visible image GPU budget exceeded".into());
                self.release_gpu(id);
                failure = Some(ImageAssetError::OverBudget);
                continue;
            }
            frames.push(ImageFrame {
                layer: aqua_core::scene::media_layer(id),
                width: image.width(),
                height: image.height(),
                rgba: Arc::from(image.pixels()),
            });
            self.resident.insert(id, (index, bytes));
        }
        self.started
            .retain(|id, _| self.images.ready(*id).is_some());
        match failure {
            Some(error) if frames.is_empty() => Err(error),
            _ => Ok(frames),
        }
    }
}

impl Application for ManagedUiApp {
    fn input_invalidation_epoch(&self, id: AquaWindowId) -> u64 {
        self.ui.input_invalidation_epoch(id)
    }

    fn color_profile(&self) -> aqua_core::ColorProfile {
        self.ui.color_profile()
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
        if matches!(event, WindowEvent::Focused(_, false)) {
            self.active = false;
            for instance in self.preview_instances.values_mut() {
                instance.last_frame_time = None;
                instance.observed = None;
            }
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
        let deadline = self.build_scene(frame.window, frame.now);
        self.prepared = Some((frame.now, deadline));
        let images = self.synchronize_images(frame.now);
        for layer in self.take_released_images() {
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
        if let Some((prepared, deadline)) = self.prepared.take() {
            if prepared == now {
                return deadline;
            }
        }
        self.now = now;
        self.active = window.focused
            && !matches!(
                self.ui.host().motion().reduced_motion(),
                aqua_ui::motion::ReducedMotion::Reduce
            );
        self.images.flush_notifications();
        let mut next = self.ui.build_scene(window, now);
        self.prepare_preview_instances();
        self.ui.flush_scene();
        self.collect_visible();
        let visible: Vec<_> = self.visible.iter().copied().collect();
        for id in visible {
            if let Some(data) = self.images.ready(self.source_id(id)) {
                if let (_, Some(deadline)) = self.selected_frame(id, &data, now) {
                    next = Some(next.map_or(deadline, |old| old.min(deadline)));
                }
            }
        }
        next
    }
    fn reactive_runtime(&self) -> Option<&aqua_reactive::Runtime> {
        Some(self.ui.runtime())
    }
}
