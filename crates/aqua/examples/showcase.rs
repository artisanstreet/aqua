//! Aqua showcase: reactive todo list driving a real window.
//!
//! Exercises the full facade path on peer contracts:
//! - `aqua-reactive`: todo state as explicit signals, derived visible list
//!   and counts, keyed per-row child scopes (disposed on remove), `batch`
//!   around multi-write commands; host-driven flush via `bind_reactive`.
//! - `aqua-text` v1: `TextEditor`-bound input field, retained `TextLayout`s
//!   minted through the shared `SharedRuns` into real `DisplayItem::Text`
//!   runs with caret + selection geometry and IME candidate positioning.
//! - `aqua-ui` overlay service: filter popover with placement, light
//!   dismissal, and focus save/restore.
//! - Platform scheduler: input demand, `MotionController` deadlines for the
//!   toggle pulse, animation deadlines from the 3D viewport, idle sleep.
//! - GPU extensions: procedural media (PNG codec path → cache → upload →
//!   `External` every frame) and a depth-tested cube `ViewportPass`
//!   (`App::externals` + shared device/queue).
//!
//! The `UiApp` high-level host (`aqua::UiApp`, `Aqua::run_ui`) covers
//! tree mounting/binding/layout for component apps; this example drives the
//! low-level `Application` escape directly to exercise the full runtime surface.
//!
//! Run: `cargo run -p aqua --example showcase`
//! Smoke (WSLg/CI, no user needed):
//! `AQUA_SMOKE_MS=2000 cargo run -p aqua --example showcase`

use std::collections::HashMap;
use std::time::{Duration, Instant};

use aqua::prelude::*;
use aqua::Application;
use aqua_reactive::{Runtime, Scope};
use aqua_render::{
    media::{external_item, media_layer, GpuImageCache, GpuMediaBudget, MediaBudget, MediaCache},
    viewport::{Camera, MeshData, MeshRenderer, ViewportDesc, ViewportPass},
};
use aqua_text::{
    LineMode, RunId, SharedFonts, SharedRuns, TextAlign, TextEditor, TextLayout, TextStyle,
    WrapMode,
};
use aqua_ui::motion::{
    Easing, MotionController, MotionHandle, MotionKind, MotionValue, ReducedMotion,
};
use aqua_ui::overlay::{
    CloseReason, DismissPolicy, FocusControl, InputOutcome, Modality, OverlayId, OverlayKind,
    OverlayRequest, OverlayService, Placement,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    All,
    Active,
    Done,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Active => "Active",
            Filter::Done => "Done",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Todo {
    id: u64,
    title: String,
    done: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum FocusZone {
    Input,
    List,
}

// Layout constants (logical px).
const HEADER_H: f32 = 56.0;
const INPUT_Y: f32 = 68.0;
const INPUT_H: f32 = 36.0;
const ROW_Y0: f32 = 116.0;
const ROW_H: f32 = 40.0;
const ROW_GAP: f32 = 8.0;
const MENU_W: f32 = 180.0;
const MENU_H: f32 = 132.0;
const PULSE_MS: u64 = 300;

fn text_style(size_px: f32) -> TextStyle {
    TextStyle {
        size_px,
        line_height_px: size_px + 6.0,
        ..Default::default()
    }
}

struct Showcase {
    rt: Runtime,
    scope: Scope,
    todos_w: aqua_reactive::WriteSignal<Vec<Todo>>,
    todos_r: aqua_reactive::ReadSignal<Vec<Todo>>,
    filter_w: aqua_reactive::WriteSignal<Filter>,
    filter_r: aqua_reactive::ReadSignal<Filter>,
    visible: aqua_reactive::ReadSignal<Vec<Todo>>,
    counts: aqua_reactive::ReadSignal<(usize, usize, usize)>,
    /// Keyed row scopes: one child scope per todo id. Removed rows dispose
    /// their scope (state/editing context freed with the row).
    row_scopes: HashMap<u64, Scope>,
    next_id: u64,
    focus_zone: FocusZone,
    focus: FocusState,
    overlays: OverlayService,
    menu: Option<OverlayId>,
    /// Canonical motion: toggle pulse lives in the shared `MotionController`
    /// (no ad-hoc clocks); the row id selects which row renders the sample.
    motion: MotionController,
    pulse_row: Option<u64>,
    pulse_handle: MotionHandle,
    motion_start: Instant,
    scene_ids: Vec<NodeId>,
    cursor: (f32, f32),
    width: f32,
    // Text state (aqua-text v1).
    fonts: SharedFonts,
    runs: SharedRuns,
    header_run: Option<(RunId, u64)>,
    input_run: Option<(RunId, u64)>,
    input_layout: TextLayout,
    input_revision_seen: u64,
    editor: TextEditor,
    row_runs: HashMap<u64, (RunId, u64, String)>,
    menu_runs: Vec<(RunId, u64)>,
    reactive_bound: bool,
    // GPU extension state (owned here, rendered through the facade's
    // `externals` hook; device/queue are borrowed per frame).
    media: Option<MediaState>,
    viewport: Option<ViewportPass>,
    ext_items: Vec<DisplayItem>,
    ext_errors: u64,
}

struct MediaState {
    cache: MediaCache,
    gpu: GpuImageCache,
    id: aqua_core::ImageId,
}

impl Showcase {
    fn new(fonts: SharedFonts, runs: SharedRuns, reduced_motion: bool) -> Self {
        let rt = Runtime::new();
        let scope = rt.create_scope();
        let (todos_r, todos_w) = scope.signal(Vec::<Todo>::new());
        let (filter_r, filter_w) = scope.signal(Filter::All);
        let visible =
            scope.derive(
                (todos_r.clone(), filter_r.clone()),
                |(todos, filter)| match filter {
                    Filter::All => todos.clone(),
                    Filter::Active => todos.iter().filter(|t| !t.done).cloned().collect(),
                    Filter::Done => todos.iter().filter(|t| t.done).cloned().collect(),
                },
            );
        let counts = scope.derive((todos_r.clone(),), |(todos,)| {
            let done = todos.iter().filter(|t| t.done).count();
            (todos.len(), done, todos.len() - done)
        });
        let mut input_layout = TextLayout::new();
        input_layout.set_line_mode(LineMode::Single);
        input_layout.set_wrap(WrapMode::WordOrGlyph);
        input_layout.set_align(TextAlign::Start);
        let reduced = if reduced_motion {
            ReducedMotion::Reduce
        } else {
            ReducedMotion::Full
        };
        let mut motion = MotionController::new(reduced);
        let motion_start = Instant::now();
        let pulse_handle = motion.spawn(
            None,
            MotionValue::Scalar(0.0),
            MotionKind::Tween {
                duration_secs: PULSE_MS as f64 / 1000.0,
                easing: Easing::EaseOut,
            },
            0.0,
        );
        let mut app = Self {
            rt,
            scope,
            todos_w,
            todos_r,
            filter_w,
            filter_r,
            visible,
            counts,
            row_scopes: HashMap::new(),
            next_id: 1,
            focus_zone: FocusZone::Input,
            focus: FocusState::default(),
            overlays: OverlayService::new(Rect::from_edges(0.0, 0.0, 960.0, 640.0)),
            menu: None,
            motion,
            pulse_row: None,
            pulse_handle,
            motion_start,
            scene_ids: Vec::new(),
            cursor: (0.0, 0.0),
            width: 960.0,
            fonts,
            runs,
            header_run: None,
            input_run: None,
            input_layout,
            input_revision_seen: u64::MAX,
            editor: TextEditor::with_text("", LineMode::Single),
            row_runs: HashMap::new(),
            menu_runs: Vec::new(),
            reactive_bound: false,
            media: None,
            viewport: None,
            ext_items: Vec::new(),
            ext_errors: 0,
        };
        for title in ["Buy milk", "Write report", "Call mom"] {
            app.insert_todo(title.to_string());
        }
        app
    }

    fn owner_node() -> NodeId {
        NodeId::from_raw(aqua_core::id::RawId {
            index: 1,
            generation: 0,
        })
    }

    /// Shape `text` and mint-or-update a run; returns the current revision.
    fn ensure_run(
        fonts: &SharedFonts,
        runs: &SharedRuns,
        slot: &mut Option<(RunId, u64)>,
        text: &str,
        style: &TextStyle,
        max_width: Option<f32>,
        scale: f32,
    ) -> (RunId, u64) {
        // Built twice only on the rare re-mint path (TextLayout is not
        // Clone); the retained result is reused across frames.
        let build = || {
            let mut layout = TextLayout::new();
            layout.set_text(text, style);
            layout.set_line_mode(LineMode::Single);
            layout.set_wrap(WrapMode::WordOrGlyph);
            layout.set_max_width(max_width);
            layout.set_scale(scale);
            layout.layout(&mut fonts.lock());
            layout
        };
        match slot {
            Some((id, rev)) => match runs.lock().update(*id, build()) {
                Some(new_rev) => {
                    *rev = new_rev;
                    (*id, new_rev)
                }
                None => {
                    // Registry lost the run (should not happen while the UI
                    // owns removals); re-mint so text stays visible.
                    let (new_id, new_rev) = runs.lock().insert(build());
                    *slot = Some((new_id, new_rev));
                    (new_id, new_rev)
                }
            },
            None => {
                let (id, rev) = runs.lock().insert(build());
                *slot = Some((id, rev));
                (id, rev)
            }
        }
    }

    fn insert_todo(&mut self, title: String) {
        if title.trim().is_empty() {
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.rt.batch(|| {
            self.todos_w.update(|todos| {
                todos.push(Todo {
                    id,
                    title,
                    done: false,
                });
            });
            self.row_scopes.insert(id, self.scope.child());
        });
        self.gc_scopes();
    }

    fn remove_todo(&mut self, id: u64) {
        self.rt.batch(|| {
            self.todos_w.update(|todos| {
                todos.retain(|t| t.id != id);
            });
        });
        if let Some(scope) = self.row_scopes.remove(&id) {
            scope.dispose();
        }
        if let Some((run, _, _)) = self.row_runs.remove(&id) {
            self.runs.lock().remove(run);
        }
    }

    fn motion_now(&self) -> Duration {
        self.motion_start.elapsed()
    }

    fn toggle_todo(&mut self, id: u64) {
        self.rt.batch(|| {
            self.todos_w.update(|todos| {
                if let Some(t) = todos.iter_mut().find(|t| t.id == id) {
                    t.done = !t.done;
                }
            });
        });
        // Canonical motion: a fresh 0→1 tween per toggle (old handle
        // removed first, so a superseded completion can never leak onto a
        // newer pulse). Reduced motion snaps via the controller policy.
        let now = self.motion_now();
        self.motion.remove(self.pulse_handle);
        self.pulse_handle = self.motion.spawn(
            None,
            MotionValue::Scalar(0.0),
            MotionKind::Tween {
                duration_secs: PULSE_MS as f64 / 1000.0,
                easing: Easing::EaseOut,
            },
            now.as_secs_f64(),
        );
        let _ = self
            .motion
            .set_target_duration(self.pulse_handle, MotionValue::Scalar(1.0), now);
        self.pulse_row = Some(id);
    }

    fn gc_scopes(&mut self) {
        let live: Vec<u64> = self.todos_r.get().iter().map(|t| t.id).collect();
        let stale: Vec<u64> = self
            .row_scopes
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in stale {
            if let Some(scope) = self.row_scopes.remove(&id) {
                scope.dispose();
            }
        }
        for id in live {
            self.row_scopes
                .entry(id)
                .or_insert_with(|| self.scope.child());
        }
    }

    fn row_at(&self, y: f32) -> Option<u64> {
        if y < ROW_Y0 {
            return None;
        }
        let idx = ((y - ROW_Y0) / (ROW_H + ROW_GAP)) as usize;
        let within = (y - ROW_Y0) % (ROW_H + ROW_GAP) <= ROW_H;
        if !within {
            return None;
        }
        self.visible.get().get(idx).map(|t| t.id)
    }

    fn menu_button_rect(width: f32) -> Rect {
        Rect::from_edges(width - 196.0, 10.0, width - 16.0, 46.0)
    }

    fn open_menu(&mut self, width: f32) {
        if self.menu.is_some() {
            return;
        }
        let anchor = Self::menu_button_rect(width);
        let req = OverlayRequest {
            owner: Self::owner_node(),
            parent: None,
            kind: OverlayKind::Popover,
            modality: Modality::NonModal,
            dismiss: DismissPolicy::LIGHT,
            anchor,
            size: aqua_core::geometry::Size::new(MENU_W, MENU_H),
            placement: Placement::below(),
            autofocus: None,
        };
        if let Ok(id) = self.overlays.open(&mut self.focus, req) {
            self.menu = Some(id);
            self.menu_runs.clear();
        }
    }

    fn option_at(&self, menu_bounds: Rect, y: f32) -> Option<Filter> {
        let rel = y - menu_bounds.y;
        if rel < 0.0 || rel >= MENU_H {
            return None;
        }
        match (rel / (MENU_H / 3.0)) as usize {
            0 => Some(Filter::All),
            1 => Some(Filter::Active),
            _ => Some(Filter::Done),
        }
    }

    fn bg(fill_hex: &str) -> (Oklch, Oklch) {
        (
            Oklch::from_hex(fill_hex).unwrap(),
            Oklch::from_hex("#000000").unwrap(),
        )
    }

    fn text_item(&self, at: [f32; 2], run: (RunId, u64)) -> DisplayItem {
        DisplayItem::Text {
            at,
            run: TextRun {
                id: run.0 .0,
                revision: run.1,
            },
            color: Oklch::from_hex("#e8ecf4").unwrap(),
            opacity: 1.0,
        }
    }

    fn sync_items(&mut self, scene: &mut Scene, items: Vec<DisplayItem>) {
        if self.scene_ids.len() != items.len() {
            for id in self.scene_ids.drain(..) {
                scene.remove(id);
            }
            self.scene_ids = items.into_iter().map(|item| scene.insert(item)).collect();
        } else {
            for (id, item) in self.scene_ids.iter().copied().zip(items) {
                scene.update(id, item);
            }
        }
    }

    /// Re-shape retained text runs whose sources changed. Row runs follow
    /// todo titles by keyed id; unknown registry handles re-mint.
    fn sync_text(&mut self, scale: f32, max_w: f32) {
        let header_style = text_style(22.0);
        let row_style = text_style(15.0);
        let input_style = text_style(15.0);
        let (total, done, _) = self.counts.get();
        let header = format!("Todo App — {done}/{total} done");
        let (id, rev) = Self::ensure_run(
            &self.fonts,
            &self.runs,
            &mut self.header_run,
            &header,
            &header_style,
            Some(max_w),
            scale,
        );
        let _ = (id, rev);
        // Input field text follows the bound editor.
        if self.editor.revision() != self.input_revision_seen {
            self.input_revision_seen = self.editor.revision();
            let display = self.editor.display_text();
            self.input_layout.set_text(&display, &input_style);
            self.input_layout.set_max_width(Some(max_w));
            self.input_layout.set_scale(scale);
            self.input_layout.layout(&mut self.fonts.lock());
            let layout = std::mem::replace(&mut self.input_layout, TextLayout::new());
            // Restore input layout settings for next edit.
            self.input_layout.set_line_mode(LineMode::Single);
            self.input_layout.set_wrap(WrapMode::WordOrGlyph);
            self.input_layout.set_align(TextAlign::Start);
            match &mut self.input_run {
                Some((run_id, rev)) => {
                    if let Some(new_rev) = self.runs.lock().update(*run_id, layout) {
                        *rev = new_rev;
                    }
                }
                None => {
                    self.input_run = Some(self.runs.lock().insert(layout));
                }
            }
        }
        // Keyed row runs.
        let mut live: Vec<(u64, String)> = self
            .todos_r
            .get()
            .iter()
            .map(|t| (t.id, t.title.clone()))
            .collect();
        live.sort_by_key(|(id, _)| *id);
        for (id, title) in live {
            let entry = self.row_runs.entry(id).or_insert_with(|| {
                let (run, rev) = self.runs.lock().insert(TextLayout::new());
                (run, rev, String::new())
            });
            if entry.2 != title {
                let mut layout = TextLayout::new();
                layout.set_text(&title, &row_style);
                layout.set_line_mode(LineMode::Single);
                layout.set_wrap(WrapMode::WordOrGlyph);
                layout.set_max_width(Some(max_w));
                layout.set_scale(scale);
                layout.layout(&mut self.fonts.lock());
                match self.runs.lock().update(entry.0, layout) {
                    Some(new_rev) => {
                        entry.1 = new_rev;
                        entry.2 = title;
                    }
                    None => {
                        let (run, rev) = self.runs.lock().insert({
                            let mut l = TextLayout::new();
                            l.set_text(&title, &row_style);
                            l.set_line_mode(LineMode::Single);
                            l.set_wrap(WrapMode::WordOrGlyph);
                            l.set_max_width(Some(max_w));
                            l.set_scale(scale);
                            l.layout(&mut self.fonts.lock());
                            l
                        });
                        *entry = (run, rev, title);
                    }
                }
            }
        }
    }

    /// Example-local semantic ids (stable slots, not tree nodes): 1 root,
    /// 2 input, 3 filter button, 10+i visible rows.
    fn sem_node(index: u32) -> NodeId {
        NodeId::from_raw(aqua_core::id::RawId {
            index,
            generation: 0,
        })
    }

    fn ensure_media(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.media.is_some() {
            return;
        }
        let (w, h) = (96u32, 64u32);
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let fx = x as f32 / w as f32;
                let fy = y as f32 / h as f32;
                px.extend_from_slice(&[(fx * 255.0) as u8, (fy * 255.0) as u8, 128, 255]);
            }
        }
        let mut png = Vec::new();
        {
            use image::ImageEncoder;
            let enc = image::codecs::png::PngEncoder::new(&mut png);
            if enc
                .write_image(&px, w, h, image::ExtendedColorType::Rgba8)
                .is_err()
            {
                self.note_ext_error("media PNG encode failed");
                return;
            }
        }
        let mut cache = MediaCache::new(MediaBudget::default());
        let id = match cache.decode_and_insert(&png) {
            Ok(id) => id,
            Err(e) => {
                self.note_ext_error(&format!("media decode failed: {e:?}"));
                return;
            }
        };
        let mut gpu = GpuImageCache::new(GpuMediaBudget::default());
        match cache.get(id) {
            Some(img) => {
                if gpu.upload(device, queue, id, img).is_err() {
                    self.note_ext_error("media GPU upload failed");
                    return;
                }
            }
            None => {
                self.note_ext_error("media image vanished after insert");
                return;
            }
        }
        self.media = Some(MediaState { cache, gpu, id });
    }

    fn ensure_viewport(&mut self, frame: &ExternalFrame<'_>) {
        if self.viewport.is_some() {
            return;
        }
        let cam = Camera::perspective(
            [2.4, 1.5, 2.4],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60f32.to_radians(),
            480.0 / 300.0,
            0.1,
            100.0,
        );
        let mr = match MeshRenderer::new(
            frame.device,
            frame.color_format,
            wgpu::TextureFormat::Depth32Float,
            &MeshData::cube(),
            cam,
            [0.04, 0.05, 0.09, 1.0],
        ) {
            Ok(mr) => mr,
            Err(e) => {
                self.note_ext_error(&format!("viewport renderer failed: {e:?}"));
                return;
            }
        };
        let desc = ViewportDesc {
            width: 480,
            height: 300,
            scale: frame.scale,
            color_format: frame.color_format,
            depth_format: wgpu::TextureFormat::Depth32Float,
        };
        match ViewportPass::new(
            frame.device.clone(),
            frame.queue.clone(),
            &desc,
            Box::new(mr),
        ) {
            Ok(mut pass) => {
                pass.set_layout(Rect::from_edges(16.0, 300.0, 316.0, 500.0), 1.0, 12.0);
                self.viewport = Some(pass);
            }
            Err(e) => self.note_ext_error(&format!("viewport pass failed: {e:?}")),
        }
    }

    fn note_ext_error(&mut self, msg: &str) {
        // Bounded logging: the externals hook runs every frame.
        if self.ext_errors < 3 {
            eprintln!("aqua showcase media/viewport: {msg}");
        }
        self.ext_errors += 1;
    }
}

#[derive(Default)]
struct FocusState {
    focused: Option<NodeId>,
}

impl FocusControl for FocusState {
    fn focused(&self) -> Option<NodeId> {
        self.focused
    }
    fn set_focused(&mut self, node: Option<NodeId>) {
        self.focused = node;
    }
}

impl Application for Showcase {
    fn on_window_event(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &WindowEvent) {
        // Host-driven reactive boundary: one bind per window. Derives stay
        // eager; effects (when added) flush at the draw boundary or via the
        // proxy wake from worker threads.
        if !self.reactive_bound {
            self.reactive_bound = true;
            ctx.bind_reactive(id, &self.rt);
        }
        match event {
            WindowEvent::CloseRequested(_) => ctx.quit(),
            WindowEvent::Resized(_, size) => {
                self.width = size.width as f32;
                self.overlays.set_viewport(Rect::from_edges(
                    0.0,
                    0.0,
                    size.width as f32,
                    size.height as f32,
                ));
            }
            _ => {}
        }
    }

    fn on_input(&mut self, ctx: &mut AppCtx, id: AquaWindowId, event: &InputEvent) {
        // Overlay policy first: dismissal, modality, capture.
        match self.overlays.handle_input(&mut self.focus, event) {
            InputOutcome::Dismissed { closed } => {
                if closed.iter().any(|c| Some(c.id) == self.menu) {
                    self.menu = None;
                    self.menu_runs.clear();
                }
                return;
            }
            InputOutcome::Captured { overlay } => {
                if Some(overlay) == self.menu {
                    if let InputEvent::PointerDown(p, _) = event {
                        if let Some(bounds) = self.overlays.bounds(overlay) {
                            if let Some(choice) = self.option_at(bounds, p.position.y) {
                                self.filter_w.set(choice);
                                self.menu_runs.clear();
                                let menu = self.menu.take();
                                if let Some(m) = menu {
                                    self.overlays
                                        .close(&mut self.focus, m, CloseReason::Requested);
                                }
                            }
                        }
                    }
                }
                return;
            }
            InputOutcome::BlockedByModal { .. } | InputOutcome::Ignored => {}
        }

        match event {
            InputEvent::PointerMove(p) => {
                self.cursor = (p.position.x, p.position.y);
            }
            InputEvent::PointerDown(p, _) => {
                self.cursor = (p.position.x, p.position.y);
                let (x, y) = (p.position.x, p.position.y);
                let at = Point::new(x, y);
                if Self::menu_button_rect(self.width).contains(at) {
                    self.open_menu(self.width);
                    return;
                }
                if (16.0..=self.width - 16.0).contains(&x)
                    && (INPUT_Y..=INPUT_Y + INPUT_H).contains(&y)
                {
                    self.focus_zone = FocusZone::Input;
                    // Click-to-caret via shaping hit test.
                    if let Some(byte) = self.input_layout.hit(x - 24.0, y - INPUT_Y - 8.0) {
                        self.editor.set_caret(byte);
                    }
                    return;
                }
                self.focus_zone = FocusZone::List;
                if let Some(todo_id) = self.row_at(y) {
                    if x > self.width - 80.0 {
                        self.remove_todo(todo_id);
                    } else {
                        self.toggle_todo(todo_id);
                        ctx.request_animation_deadline(
                            id,
                            Instant::now() + Duration::from_millis(16),
                        );
                    }
                }
            }
            InputEvent::PointerUp(_, _) | InputEvent::PointerCancel { .. } => {
                // No press-activation on release or cancel: toggles commit
                // on press; cancel terminates without any state change.
            }
            InputEvent::Scroll { .. } => {
                // No scrollable region in this demo; the overlay router
                // already had first refusal above.
            }
            InputEvent::KeyDown(k) => {
                if k.key == Key::Named(NamedKey::Escape) {
                    // Overlay dismissal ran first; clear the draft here.
                    self.editor.set_text("");
                    self.input_revision_seen = u64::MAX.wrapping_sub(1);
                    return;
                }
                if self.focus_zone != FocusZone::Input {
                    return;
                }
                // Text arrives via TextInput (layout-resolved, multi-scalar
                // safe, IME-suppressed); KeyDown carries shortcuts only.
                match &k.key {
                    Key::Named(NamedKey::Enter) => {
                        let draft = self.editor.text().trim().to_string();
                        if !draft.is_empty() {
                            self.insert_todo(draft);
                            self.editor.set_text("");
                        }
                    }
                    Key::Named(NamedKey::Backspace) => {
                        self.editor.delete_backward();
                    }
                    Key::Named(NamedKey::Delete) => {
                        self.editor.delete_forward();
                    }
                    Key::Named(NamedKey::Left) => self.editor.move_left(false),
                    Key::Named(NamedKey::Right) => self.editor.move_right(false),
                    Key::Named(NamedKey::Home) => self.editor.move_to_start(false),
                    Key::Named(NamedKey::End) => self.editor.move_to_end(false),
                    _ => {}
                }
            }
            InputEvent::TextInput { text } => {
                if self.focus_zone == FocusZone::Input {
                    self.editor.insert(text);
                }
            }
            InputEvent::KeyUp(_) => {}
            InputEvent::Ime(ime) => {
                if self.focus_zone != FocusZone::Input {
                    return;
                }
                match ime {
                    ImeEvent::Update { marked, cursor } => {
                        let range = cursor.unwrap_or((marked.len(), marked.len()));
                        self.editor.set_composition(marked, range.0..range.1);
                    }
                    ImeEvent::Commit { text } => {
                        self.editor.set_composition(text, 0..0);
                        self.editor.commit_composition();
                    }
                    ImeEvent::Cancel => {
                        self.editor.cancel_composition();
                    }
                    ImeEvent::Start => {}
                }
            }
            InputEvent::Focused(_) => {}
        }
    }

    fn on_files_dropped(
        &mut self,
        _ctx: &mut AppCtx,
        _id: AquaWindowId,
        paths: &[std::path::PathBuf],
    ) {
        // Demo hook: dropped files are acknowledged, not imported (no media
        // pipeline yet). The hook itself is real and tested headless.
        eprintln!("aqua showcase: {} file(s) dropped", paths.len());
    }

    fn on_a11y_action(
        &mut self,
        ctx: &mut AppCtx,
        id: AquaWindowId,
        request: &aqua_platform::a11y::A11yActionRequest,
    ) {
        use aqua_platform::a11y::A11yAction as A;
        let index = request.node.raw().index;
        match (&request.action, index) {
            (A::Press, 3) => self.open_menu(self.width),
            (A::Press, n) if n >= 10 => {
                let i = (n - 10) as usize;
                if let Some(todo) = self.visible.get().get(i).cloned() {
                    self.toggle_todo(todo.id);
                    ctx.request_animation_deadline(id, Instant::now());
                }
            }
            (A::Focus, 2) => {
                self.focus_zone = FocusZone::Input;
            }
            (A::Focus, _) => {
                self.focus_zone = FocusZone::List;
            }
            _ => {}
        }
    }

    /// GPU extension hook: offscreen media decode/upload and the 3D
    /// viewport render each frame; `build_scene` commits the resulting
    /// `External` items.
    fn externals(&mut self, frame: &mut ExternalFrame<'_>) {
        self.ext_items.clear();
        self.ensure_media(frame.device, frame.queue);
        self.ensure_viewport(frame);
        if let Some(media) = &self.media {
            // Skip cleanly if the entry was evicted (LRU); the upload path
            // re-inserts on demand.
            if media.cache.get(media.id).is_some() {
                if let Some(up) = media.gpu.view(media.id) {
                    let layer = media_layer(media.id);
                    frame.register_external(layer, up.view.clone(), up.width, up.height);
                    self.ext_items.push(external_item(
                        layer,
                        Rect::from_edges(332.0, 300.0, 552.0, 440.0),
                        1.0,
                        8.0,
                    ));
                }
            }
        }
        if let Some(vp) = self.viewport.as_mut() {
            match vp.render_offscreen() {
                Ok(()) => {
                    let (layer, view, w, h) = (vp.layer(), vp.color_view(), 480u32, 300u32);
                    frame.register_external(layer, view, w, h);
                    self.ext_items.push(vp.item());
                }
                Err(e) => self.note_ext_error(&format!("viewport frame failed: {e:?}")),
            }
        }
    }

    /// IME candidate window follows the shaped input caret; `None` when the
    /// input is not focused so the OS disables the candidate window.
    fn ime_cursor_area(&self) -> Option<Rect> {
        if self.focus_zone != FocusZone::Input {
            return None;
        }
        let byte = self
            .editor
            .ime_caret()
            .min(self.editor.display_text().len());
        self.input_layout.caret(byte).map(|c| {
            Rect::from_edges(
                24.0 + c.x,
                INPUT_Y + 8.0 + c.y_top,
                26.0 + c.x,
                INPUT_Y + 8.0 + c.y_top + c.height,
            )
        })
    }

    fn a11y_nodes(&self) -> Option<(NodeId, NodeId, Vec<aqua_platform::a11y::A11yNode>)> {
        use aqua_core::{Role, SemanticNode};
        use aqua_platform::a11y::A11yNode;
        let mut items: Vec<A11yNode> = Vec::new();
        let mut child_ids = vec![Self::sem_node(2), Self::sem_node(3)];
        let visible = self.visible.get();
        for (i, todo) in visible.iter().enumerate() {
            let node = Self::sem_node(10 + i as u32);
            child_ids.push(node);
            let mut sem = SemanticNode::new(node, Role::ListItem);
            sem.label = format!(
                "{} ({})",
                todo.title,
                if todo.done { "done" } else { "open" }
            );
            sem.selected = todo.done;
            items.push(A11yNode {
                semantic: sem,
                bounds: Some(Rect::from_edges(
                    16.0,
                    ROW_Y0 + i as f32 * (ROW_H + ROW_GAP),
                    self.width - 16.0,
                    ROW_Y0 + i as f32 * (ROW_H + ROW_GAP) + ROW_H,
                )),
                children: vec![],
            });
        }
        let focus = match self.focus_zone {
            FocusZone::Input => Self::sem_node(2),
            FocusZone::List => Self::sem_node(1),
        };
        let mut input = SemanticNode::new(Self::sem_node(2), Role::TextField);
        input.label = "New todo".to_string();
        input.focused = self.focus_zone == FocusZone::Input;
        input.value_text = Some(self.editor.text().to_string());
        items.push(A11yNode {
            semantic: input,
            bounds: Some(Rect::from_edges(
                16.0,
                INPUT_Y,
                self.width - 16.0,
                INPUT_Y + INPUT_H,
            )),
            children: vec![],
        });
        let mut button = SemanticNode::new(Self::sem_node(3), Role::Button);
        button.label = format!("Filter: {}", self.filter_r.get().label());
        items.push(A11yNode {
            semantic: button,
            bounds: Some(Self::menu_button_rect(self.width)),
            children: vec![],
        });
        let mut root = SemanticNode::new(Self::sem_node(1), Role::Group);
        root.label = "Todo list".to_string();
        root.children = child_ids.clone();
        items.push(A11yNode {
            semantic: root,
            bounds: Some(Rect::from_edges(0.0, 0.0, self.width, 640.0)),
            children: child_ids,
        });
        Some((Self::sem_node(1), focus, items))
    }

    /// Procedural gradient through the real codec path: encode PNG
    /// in-memory (`image`), decode via `decode_bytes`, retain in the CPU
    /// cache, upload to the GPU cache. No fixture files, no fake pixels.
    fn build_scene(&mut self, scene: &mut Scene, window: &ObservedWindowState) -> Option<Instant> {
        let w = window.inner_size.width as f32;
        let h = window.inner_size.height as f32;
        self.width = w;
        let scale = window.scale_factor as f32;
        let (panel, border) = Self::bg("#16213a");
        let (row_bg, _) = Self::bg("#1f2d4d");
        let (done_bg, _) = Self::bg("#1a3a2a");
        let (accent, _) = Self::bg("#3b82f6");

        self.sync_text(scale, w - 32.0);

        let mut items = vec![
            DisplayItem::Rect {
                bounds: Rect::from_edges(0.0, 0.0, w, h),
                radius: 0.0,
                fill: Oklch::from_hex("#0b1020").unwrap(),
                border_width: 0.0,
                border_color: border,
                shadow: None,
            },
            DisplayItem::Rect {
                bounds: Rect::from_edges(0.0, 0.0, w, HEADER_H),
                radius: 0.0,
                fill: panel,
                border_width: 0.0,
                border_color: border,
                shadow: None,
            },
            DisplayItem::Rect {
                bounds: Rect::from_edges(16.0, INPUT_Y, w - 16.0, INPUT_Y + INPUT_H),
                radius: 8.0,
                fill: row_bg,
                border_width: if self.focus_zone == FocusZone::Input {
                    2.0
                } else {
                    1.0
                },
                border_color: accent,
                shadow: None,
            },
            DisplayItem::Rect {
                bounds: Self::menu_button_rect(w),
                radius: 8.0,
                fill: accent,
                border_width: 0.0,
                border_color: border,
                shadow: None,
            },
        ];

        // Real shaped text: header, input contents, filter label.
        if let Some(run) = self.header_run {
            items.push(self.text_item([16.0, 16.0], run));
        }
        if let Some(run) = self.input_run {
            items.push(self.text_item([24.0, INPUT_Y + 8.0], run));
            // Caret + selection from shaping geometry.
            let caret_byte = self
                .editor
                .ime_caret()
                .min(self.editor.display_text().len());
            if let Some(caret) = self.input_layout.caret(caret_byte) {
                if self.focus_zone == FocusZone::Input {
                    items.push(DisplayItem::Rect {
                        bounds: Rect::from_edges(
                            24.0 + caret.x,
                            INPUT_Y + 8.0 + caret.y_top,
                            26.0 + caret.x,
                            INPUT_Y + 8.0 + caret.y_top + caret.height,
                        ),
                        radius: 1.0,
                        fill: Oklch::from_hex("#ffffff").unwrap(),
                        border_width: 0.0,
                        border_color: border,
                        shadow: None,
                    });
                }
            }
            let sel = self.editor.selection();
            let (a, b) = (sel.anchor.min(sel.caret), sel.anchor.max(sel.caret));
            if b > a {
                for r in self.input_layout.selection_rects(a..b) {
                    items.push(DisplayItem::Rect {
                        bounds: Rect::from_edges(
                            24.0 + r.x,
                            INPUT_Y + 8.0 + r.y,
                            24.0 + r.x + r.w,
                            INPUT_Y + 8.0 + r.y + r.h,
                        ),
                        radius: 1.0,
                        fill: Oklch::from_hex("#3b82f6").unwrap(),
                        border_width: 0.0,
                        border_color: border,
                        shadow: None,
                    });
                }
            }
        }

        // Rows (keyed order from the derived visible list).
        let motion_now = self.motion_now();
        let pulse_p = match self.motion.sample_duration(self.pulse_handle, motion_now) {
            Some(MotionValue::Scalar(p)) => p.clamp(0.0, 1.0),
            _ => 1.0,
        };
        let pulse_settled = self.motion.is_settled(self.pulse_handle);
        for (i, todo) in self.visible.get().iter().enumerate() {
            let y = ROW_Y0 + i as f32 * (ROW_H + ROW_GAP);
            let mut fill = if todo.done { done_bg } else { row_bg };
            if self.pulse_row == Some(todo.id) && !pulse_settled {
                fill.alpha = 1.0 - 0.6 * pulse_p;
            }
            items.push(DisplayItem::Rect {
                bounds: Rect::from_edges(16.0, y, w - 16.0, y + ROW_H),
                radius: 8.0,
                fill,
                border_width: 0.0,
                border_color: border,
                shadow: None,
            });
            items.push(DisplayItem::Rect {
                bounds: Rect::from_edges(28.0, y + 10.0, 48.0, y + 30.0),
                radius: 4.0,
                fill: if todo.done { accent } else { panel },
                border_width: 1.0,
                border_color: border,
                shadow: None,
            });
            if let Some((run, _, _)) = self.row_runs.get(&todo.id) {
                items.push(self.text_item([60.0, y + 10.0], (*run, self.row_runs[&todo.id].1)));
            }
        }

        // Overlay host: draw open overlays above content.
        for entry in self.overlays.render_plan() {
            items.push(DisplayItem::Rect {
                bounds: entry.bounds,
                radius: 10.0,
                fill: panel,
                border_width: 1.0,
                border_color: accent,
                shadow: Some((0.0, 8.0, 16.0, Oklch::from_hex("#000000").unwrap())),
            });
            let labels = ["All", "Active", "Done"];
            for (i, label) in labels.iter().enumerate() {
                let oy = entry.bounds.y + i as f32 * (MENU_H / 3.0);
                let active = self.filter_r.get().label() == *label;
                items.push(DisplayItem::Rect {
                    bounds: Rect::from_edges(
                        entry.bounds.x + 6.0,
                        oy + 4.0,
                        entry.bounds.x + MENU_W - 6.0,
                        oy + MENU_H / 3.0 - 4.0,
                    ),
                    radius: 6.0,
                    fill: if active { accent } else { row_bg },
                    border_width: 0.0,
                    border_color: border,
                    shadow: None,
                });
                let style = text_style(14.0);
                let key = format!("menu:{label}");
                let _ = key;
                let _ = style;
                let _ = oy;
            }
        }
        // Menu option labels: static runs minted on first open.
        if self.menu.is_some() && self.menu_runs.is_empty() {
            let style = text_style(14.0);
            for label in ["All", "Active", "Done"] {
                let mut slot = None;
                let run = Self::ensure_run(
                    &self.fonts,
                    &self.runs,
                    &mut slot,
                    label,
                    &style,
                    Some(MENU_W),
                    scale,
                );
                let _ = slot;
                self.menu_runs.push(run);
            }
        }
        if !self.menu_runs.is_empty() {
            if let Some(menu_id) = self.menu {
                if let Some(bounds) = self.overlays.bounds(menu_id) {
                    for (i, run) in self.menu_runs.iter().enumerate() {
                        items.push(self.text_item(
                            [bounds.x + 16.0, bounds.y + 8.0 + i as f32 * (MENU_H / 3.0)],
                            *run,
                        ));
                    }
                }
            }
        }

        // GPU extension layers (media + 3D viewport) committed last so UI
        // chrome draws above them in painter order.
        items.extend(self.ext_items.iter().cloned());
        self.sync_items(scene, items);

        // Canonical motion drives pacing: the controller's next deadline
        // feeds the platform scheduler; settled motion schedules nothing.
        // Elapsed `Duration` clock keeps the controller headless-testable.
        let elapsed = self.motion_start.elapsed();
        if !self.motion.is_settled(self.pulse_handle) {
            if let Some(deadline) = self.motion.next_deadline_duration(elapsed) {
                return Some(Instant::now() + deadline.saturating_sub(elapsed));
            }
            return Some(Instant::now() + Duration::from_millis(16));
        }
        self.pulse_row = None;
        None
    }
}

fn main() -> Result<(), String> {
    let mut config = AquaConfig::default();
    config.window.title = "Aqua Showcase — Todo".to_string();
    config.set_fps(60).map_err(|e| e.to_string())?;
    // OS preferences drive honest behavior: reduced motion snaps the motion
    // controller; high contrast/transparency-off disables materials.
    let prefs = aqua_platform::prefs::query_preferences();
    if aqua_platform::prefs::should_reduce_motion(&prefs) {
        config.reduced_motion = true;
    }
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let app = Showcase::new(fonts.clone(), runs.clone(), config.reduced_motion);
    let (total, done, remaining) = app.counts.get();
    eprintln!("aqua showcase: {total} todos ({done} done, {remaining} remaining)");
    let mut aqua = Aqua::new(config)?;
    aqua.set_fonts(fonts);
    aqua.set_runs(runs);
    aqua.run(app)
}
