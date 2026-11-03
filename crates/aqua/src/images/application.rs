use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

use aqua_core::{ImageId, InputEvent, NodeId, Rect};
use aqua_platform::{
    a11y::A11yActionRequest, chrome::DragRegions, AquaWindowId, ObservedWindowState, WindowEvent,
};
use aqua_protocol::{A11yNode, FrameReason, Instant};
use aqua_render::{
    media::{GpuImageCache, GpuMediaBudget},
    DisplayItem, Scene,
};

use crate::{
    mailbox::Mailbox,
    runtime::{AppCtx, Application, ExternalFrame},
    ui_app::UiApp,
};

use super::{Data, ImageAssetError, ImageAssets};

pub struct ImageFrame {
    pub layer: u64,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

pub struct ManagedUiApp {
    ui: UiApp,
    images: ImageAssets,
    mailbox: Mailbox<()>,
    _subscription: super::ImageSubscription,
    gpu: GpuImageCache,
    device: Option<wgpu::Device>,
    resident: HashMap<ImageId, usize>,
    visible: HashSet<ImageId>,
    scene_revision: Option<(u64, aqua_core::Size)>,
    started: HashMap<ImageId, Instant>,
    now: Instant,
    prepared: Option<(Instant, Option<Instant>)>,
    released: Vec<u64>,
}

impl ManagedUiApp {
    pub fn new(ui: UiApp, images: ImageAssets) -> Self {
        let mailbox = Mailbox::new(NonZeroUsize::MIN);
        let sender = mailbox.sender();
        let subscription = images.subscribe(Arc::new(move || {
            let _ = sender.send(());
        }));
        let budget = images.budget();
        Self {
            ui,
            images,
            mailbox,
            _subscription: subscription,
            gpu: GpuImageCache::new(GpuMediaBudget {
                max_gpu_bytes: budget.gpu_bytes,
                max_textures: budget.gpu_images,
            }),
            device: None,
            resident: HashMap::new(),
            visible: HashSet::new(),
            scene_revision: None,
            started: HashMap::new(),
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
        self.gpu.resident_bytes()
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
                DisplayItem::PushClip { bounds, .. } => {
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
        self.gpu.release(id);
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
                let start = *self.started.entry(id).or_insert(now);
                let sample = animation.sample(now.saturating_duration_since(start));
                (
                    sample.frame,
                    sample.next_frame_at.map(|elapsed| start + elapsed),
                )
            }
        }
    }

    pub fn synchronize_images(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        now: Instant,
    ) -> Result<Vec<ImageFrame>, ImageAssetError> {
        self.collect_visible();
        let budget = self.images.budget();
        if self.device.as_ref() != Some(device) {
            self.gpu = GpuImageCache::new(GpuMediaBudget {
                max_gpu_bytes: budget.gpu_bytes,
                max_textures: budget.gpu_images,
            });
            self.resident.clear();
            self.device = Some(device.clone());
        }
        let released: Vec<_> = self
            .resident
            .keys()
            .filter(|id| !self.visible.contains(id) || self.images.ready(**id).is_none())
            .copied()
            .collect();
        for id in released {
            self.release_gpu(id);
        }
        let mut frames = Vec::new();
        let mut failure = None;
        let visible: Vec<_> = self.visible.iter().copied().collect();
        for id in visible {
            let Some(data) = self.images.ready(id) else {
                continue;
            };
            let (index, _) = self.selected_frame(id, &data, now);
            if self.resident.get(&id) == Some(&index) {
                continue;
            }
            let image = match &data {
                Data::Still(image) => image.as_ref(),
                Data::Animation(animation) => &animation.frames()[index].image,
            };
            let bytes =
                u64::from((image.width() * 4).div_ceil(256) * 256) * u64::from(image.height());
            let previous_bytes = self.gpu.view(id).map_or(0, |image| image.bytes);
            let new_count = self.resident.len() + usize::from(!self.resident.contains_key(&id));
            if self
                .gpu
                .resident_bytes()
                .saturating_sub(previous_bytes)
                .saturating_add(bytes)
                > budget.gpu_bytes
                || new_count > budget.gpu_images
            {
                self.images
                    .fail_upload(id, "visible image GPU budget exceeded".into());
                self.release_gpu(id);
                failure = Some(ImageAssetError::OverBudget);
                continue;
            }
            match self.gpu.upload(device, queue, id, image) {
                Ok(image) => {
                    frames.push(ImageFrame {
                        layer: aqua_core::scene::media_layer(id),
                        view: image.view.clone(),
                        width: image.width,
                        height: image.height,
                    });
                    self.resident.insert(id, index);
                }
                Err(error) => {
                    self.images.fail_upload(id, error.to_string());
                    self.release_gpu(id);
                    failure = Some(ImageAssetError::Upload(error.to_string()));
                }
            }
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
    fn cursor(&self) -> aqua_core::CursorIcon {
        self.ui.cursor()
    }
    fn on_window_event(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
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
    fn a11y_nodes(&self) -> Option<(NodeId, NodeId, Vec<A11yNode>)> {
        self.ui.a11y_nodes()
    }
    fn ime_cursor_area(&self) -> Option<Rect> {
        self.ui.ime_anchor()
    }
    fn chrome_regions(&self, window: &ObservedWindowState) -> Option<DragRegions> {
        Application::chrome_regions(&self.ui, window)
    }
    fn externals(&mut self, frame: &mut ExternalFrame<'_>) {
        let deadline = self.build_scene(frame.window, frame.now);
        self.prepared = Some((frame.now, deadline));
        let images = self.synchronize_images(frame.device, frame.queue, frame.now);
        for layer in self.take_released_images() {
            frame.unregister_external(layer);
        }
        match images {
            Ok(images) => {
                for image in images {
                    frame.register_external(image.layer, image.view, image.width, image.height);
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
        self.images.flush_notifications();
        let mut next = self.ui.build_scene(window, now);
        self.collect_visible();
        let visible: Vec<_> = self.visible.iter().copied().collect();
        for id in visible {
            if let Some(data) = self.images.ready(id) {
                if let (_, Some(deadline)) = self.selected_frame(id, &data, now) {
                    next = Some(next.map_or(deadline, |old| old.min(deadline)));
                }
            }
        }
        next
    }
    fn scene_mut(&mut self, _id: AquaWindowId) -> Option<&mut Scene> {
        Some(self.ui.scene_mut())
    }
    fn reactive_runtime(&self) -> Option<&aqua_reactive::Runtime> {
        Some(self.ui.runtime())
    }
}
