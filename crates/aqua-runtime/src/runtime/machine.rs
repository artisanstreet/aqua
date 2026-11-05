use std::collections::HashSet;

use aqua_core::{CursorIcon, InputEvent as CoreInputEvent, NodeId, Point, Rect, Size};
use aqua_protocol::{
    AccessAction, AccessTreeUpdate, DialogOutcome, DialogSpec, DragRegions, FileDropEvent, FrameId,
    FrameReason, FrameReport, GpuEvent, ImeCommand, ImeDocument, ImeEvent, Instant, Machine,
    OsPreferences, RenderCommand, RenderEvent, RequestId, RuntimeInput, RuntimeOutput,
    ShellCommand, ShellEvent, Utf16Range, WindowId, WindowSpec,
};
use aqua_ui::input::{ClipboardPaste, ExternalFileDragEvent};
use aqua_ui::tree::{NodeKind, TextBacking};

use super::{Runtime, emit_scene_diff};

#[derive(Clone, Copy, Debug, PartialEq)]
struct ImeProbe {
    node: NodeId,
    revision: u64,
    anchor: usize,
    caret: usize,
    composing: bool,
    area: Option<Rect>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum ImeState {
    #[default]
    Off,
    On(ImeProbe),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AccessKey {
    epoch: u64,
    focus: NodeId,
    scale_bits: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AccessState {
    #[default]
    Detached,
    Attached(Option<AccessKey>),
}

#[derive(Clone, Debug, Default)]
pub struct GpuState {
    pub lost: bool,
    pub presented: Option<FrameId>,
    pub report: Option<FrameReport>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DragState {
    #[default]
    Idle,
    Hovering,
}

pub(super) struct Bridge {
    window: Option<WindowId>,
    next_window: u64,
    next_request: u64,
    visible: bool,
    pointer: Option<Point>,
    drag: DragState,
    cursor: Option<CursorIcon>,
    regions: Option<DragRegions>,
    viewport: Option<(Size, f32)>,
    ime: ImeState,
    ime_area: Option<Rect>,
    access: AccessState,
    pastes: HashSet<RequestId>,
    pub(super) input_consumed: bool,
    dialogs: Vec<(RequestId, DialogOutcome)>,
    queued: Vec<ShellCommand>,
    wake: Option<Instant>,
    preferences: OsPreferences,
    gpu: GpuState,
    render_events: Vec<RenderEvent>,
}

impl Bridge {
    pub(super) fn new() -> Self {
        Self {
            window: None,
            next_window: 1,
            next_request: 1,
            visible: true,
            pointer: None,
            drag: DragState::Idle,
            cursor: None,
            regions: None,
            viewport: None,
            ime: ImeState::Off,
            ime_area: None,
            access: AccessState::Detached,
            pastes: HashSet::new(),
            input_consumed: false,
            dialogs: Vec::new(),
            queued: Vec::new(),
            wake: None,
            preferences: OsPreferences::default(),
            gpu: GpuState::default(),
            render_events: Vec::new(),
        }
    }

    fn request(&mut self) -> RequestId {
        let id = RequestId(self.next_request);
        self.next_request += 1;
        id
    }
}

fn pointer_position(event: &CoreInputEvent) -> Option<Point> {
    match event {
        CoreInputEvent::PointerMove(pointer)
        | CoreInputEvent::PointerDown(pointer, _)
        | CoreInputEvent::PointerUp(pointer, _) => Some(pointer.position),
        CoreInputEvent::Scroll { at, .. } => Some(*at),
        _ => None,
    }
}

enum ClipboardIntent {
    Copy,
    Cut,
    Paste,
}

fn clipboard_intent(event: &CoreInputEvent) -> Option<ClipboardIntent> {
    use aqua_core::{Key, Modifiers, NamedKey};
    let CoreInputEvent::KeyDown(key) = event else {
        return None;
    };
    let control = key.modifiers == Modifiers::CONTROL
        || key.modifiers == Modifiers::CONTROL.union(Modifiers::SHIFT);
    let shift = key.modifiers == Modifiers::SHIFT;
    let copy = control && matches!(key.key, Key::Char('c' | 'C') | Key::Named(NamedKey::Insert));
    let cut = (control && matches!(key.key, Key::Char('x' | 'X')))
        || (shift && key.key == Key::Named(NamedKey::Delete));
    let paste = (control && matches!(key.key, Key::Char('v' | 'V')))
        || (shift && key.key == Key::Named(NamedKey::Insert));
    match (copy, cut, paste) {
        (true, _, _) => Some(ClipboardIntent::Copy),
        (_, true, _) => Some(ClipboardIntent::Cut),
        (_, _, true) => Some(ClipboardIntent::Paste),
        _ => None,
    }
}

impl Runtime {
    pub fn open_window(&mut self, spec: WindowSpec) -> WindowId {
        let raw = self.bridge.next_window;
        self.bridge.next_window += 1;
        let window = WindowId::new(raw).expect("window ids start at one");
        self.bridge
            .queued
            .push(ShellCommand::CreateWindow { window, spec });
        window
    }

    pub fn submit(&mut self, command: ShellCommand) {
        self.bridge.queued.push(command);
    }

    pub fn request_dialog(&mut self, spec: DialogSpec) -> RequestId {
        let request = self.bridge.request();
        self.bridge
            .queued
            .push(ShellCommand::Dialog { request, spec });
        request
    }

    pub fn take_dialog_outcome(&mut self, request: RequestId) -> Option<DialogOutcome> {
        let index = self
            .bridge
            .dialogs
            .iter()
            .position(|(candidate, _)| *candidate == request)?;
        Some(self.bridge.dialogs.remove(index).1)
    }

    pub fn take_render_events(&mut self) -> Vec<RenderEvent> {
        std::mem::take(&mut self.bridge.render_events)
    }

    #[must_use]
    pub fn os_preferences(&self) -> OsPreferences {
        self.bridge.preferences
    }

    #[must_use]
    pub fn gpu_state(&self) -> &GpuState {
        &self.bridge.gpu
    }

    #[must_use]
    pub fn window(&self) -> Option<WindowId> {
        self.bridge.window
    }

    #[must_use]
    pub fn access_attached(&self) -> bool {
        matches!(self.bridge.access, AccessState::Attached(_))
    }

    fn request_frame(&self, out: &mut RuntimeOutput, reason: FrameReason) {
        if reason.is_empty() {
            return;
        }
        let Some(window) = self.bridge.window else {
            return;
        };
        let merged = out.shell.iter_mut().find_map(|command| match command {
            ShellCommand::RequestFrame {
                window: target,
                reason: existing,
            } if *target == window => Some(existing),
            _ => None,
        });
        match merged {
            Some(existing) => *existing |= reason,
            None => out
                .shell
                .push(ShellCommand::RequestFrame { window, reason }),
        }
    }

    fn bind(&mut self, window: WindowId) {
        self.bridge.window.get_or_insert(window);
    }

    fn shell_input(&mut self, event: &CoreInputEvent, out: &mut RuntimeOutput) {
        if let Some(position) = pointer_position(event) {
            self.bridge.pointer = Some(position);
        }
        let intent = clipboard_intent(event);
        let consumed = self.handle_input_with_default(event, |runtime| match intent {
            Some(ClipboardIntent::Copy | ClipboardIntent::Cut) => {
                let cut = matches!(intent, Some(ClipboardIntent::Cut));
                let mut redraw = false;
                if let Some(text) = runtime.selected_text() {
                    runtime
                        .bridge
                        .queued
                        .push(ShellCommand::ClipboardWrite { text });
                    redraw = cut
                        && runtime
                            .cut_external_selection()
                            .unwrap_or_else(|| runtime.delete_selected_text());
                }
                Some(redraw)
            }
            Some(ClipboardIntent::Paste) => {
                let request = runtime.bridge.request();
                runtime.bridge.pastes.insert(request);
                runtime
                    .bridge
                    .queued
                    .push(ShellCommand::ClipboardRead { request });
                Some(false)
            }
            None => None,
        });
        self.bridge.input_consumed = consumed;
        let reason = self.input_redraw(consumed);
        self.request_frame(out, reason);
    }

    fn shell_event(&mut self, event: ShellEvent, out: &mut RuntimeOutput) {
        match event {
            ShellEvent::Input { window, event } => {
                self.bind(window);
                self.shell_input(&event, out);
            }
            ShellEvent::Resized {
                window,
                size,
                scale,
            } => {
                self.bind(window);
                let scale = scale.max(f32::EPSILON);
                self.set_viewport(size.width as f32 / scale, size.height as f32 / scale, scale);
                self.request_frame(out, FrameReason::RESIZE);
            }
            ShellEvent::Frame {
                window,
                presentation,
            } => {
                self.bind(window);
                self.frame(presentation, out);
            }
            ShellEvent::Visibility { window, visible } => {
                self.bind(window);
                self.bridge.visible = visible;
                if visible {
                    self.request_frame(out, FrameReason::EXPOSE);
                }
            }
            ShellEvent::Maximized { window, .. } => self.bind(window),
            ShellEvent::CaptionHover { window, .. } => self.bind(window),
            ShellEvent::CloseRequested(window) => {
                out.shell.push(ShellCommand::CloseWindow(window));
            }
            ShellEvent::Destroyed(window) => {
                if self.bridge.window == Some(window) {
                    self.bridge.window = None;
                    out.shell.push(ShellCommand::Quit);
                }
            }
            ShellEvent::Preferences(preferences) => {
                if preferences.reduced_motion && !self.bridge.preferences.reduced_motion {
                    self.apply_startup_motion_policy(true);
                }
                self.bridge.preferences = preferences;
                self.request_frame(out, FrameReason::MATERIAL);
            }
            ShellEvent::Clipboard {
                request,
                text,
                image,
                paths,
            } => {
                if self.bridge.pastes.remove(&request) {
                    let redraw = self.paste_clipboard(ClipboardPaste {
                        text: text.map(std::sync::Arc::from),
                        image: image.and_then(|image| {
                            aqua_ui::input::PasteImage::new(image.width, image.height, image.rgba)
                        }),
                        paths: paths
                            .into_iter()
                            .map(std::path::PathBuf::from)
                            .collect::<Vec<_>>()
                            .into(),
                    });
                    if redraw {
                        self.request_frame(out, FrameReason::INPUT);
                    }
                }
            }
            ShellEvent::FileDrop { window, event } => {
                self.bind(window);
                self.file_drop(event, out);
            }
            ShellEvent::Dialog { request, outcome } => {
                self.bridge.dialogs.push((request, outcome));
                self.request_frame(out, FrameReason::INPUT);
            }
        }
    }

    fn file_drop(&mut self, event: FileDropEvent, out: &mut RuntimeOutput) {
        let position = self.bridge.pointer.unwrap_or(Point::new(0.0, 0.0));
        let paths = |paths: Vec<String>| -> Vec<std::path::PathBuf> {
            paths.into_iter().map(std::path::PathBuf::from).collect()
        };
        let handled = match (event, self.bridge.drag) {
            (FileDropEvent::Hover { paths: list }, DragState::Idle) => {
                self.bridge.drag = DragState::Hovering;
                self.external_file_drag(ExternalFileDragEvent::Enter {
                    paths: paths(list),
                    position,
                })
            }
            (FileDropEvent::Hover { .. }, DragState::Hovering) => {
                self.external_file_drag(ExternalFileDragEvent::Move { position })
            }
            (FileDropEvent::Drop { paths: list }, DragState::Hovering) => {
                self.bridge.drag = DragState::Idle;
                let submitted = self.external_file_drag(ExternalFileDragEvent::Submit { position });
                let dropped = self.drop_files_at(paths(list), Some(position));
                submitted || dropped
            }
            (FileDropEvent::Drop { paths: list }, DragState::Idle) => {
                self.drop_files_at(paths(list), Some(position))
            }
            (FileDropEvent::Cancel, DragState::Hovering) => {
                self.bridge.drag = DragState::Idle;
                self.external_file_drag(ExternalFileDragEvent::Exit)
            }
            (FileDropEvent::Cancel, DragState::Idle) => false,
        };
        if handled {
            self.request_frame(out, FrameReason::INPUT);
        }
    }

    fn ime_event(&mut self, event: ImeEvent, out: &mut RuntimeOutput) {
        let routed = match event {
            ImeEvent::Composition(composition) => CoreInputEvent::Ime(composition),
            ImeEvent::Replace { range, text } => CoreInputEvent::ImeReplace {
                start: range.start,
                end: range.end,
                text,
            },
            ImeEvent::Select { range, reversed } => CoreInputEvent::ImeSelect {
                start: range.start,
                end: range.end,
                reversed,
            },
        };
        let consumed = self.handle_input(&routed);
        let reason = self.input_redraw(consumed);
        self.request_frame(out, reason);
    }

    fn access_action(&mut self, action: AccessAction, out: &mut RuntimeOutput) {
        self.apply_access(&action);
        self.request_frame(out, FrameReason::INPUT);
    }

    fn gpu_event(&mut self, event: GpuEvent, out: &mut RuntimeOutput) {
        match event {
            GpuEvent::FrameExecuted { report, .. } => self.bridge.gpu.report = Some(report),
            GpuEvent::Presented(frame) => self.bridge.gpu.presented = Some(frame),
            GpuEvent::DeviceLost => self.bridge.gpu.lost = true,
            GpuEvent::DeviceRecovered => {
                self.bridge.gpu.lost = false;
                self.request_frame(out, FrameReason::EXPOSE);
            }
        }
    }

    fn render_event(&mut self, event: RenderEvent, out: &mut RuntimeOutput) {
        if matches!(event, RenderEvent::DeviceRecovered) {
            self.request_frame(out, FrameReason::EXPOSE);
        }
        if self.deliver_motion_event(&event) {
            self.request_frame(out, FrameReason::DAMAGE);
        }
        self.bridge.render_events.push(event);
    }

    fn tick(&mut self, now: Instant, out: &mut RuntimeOutput) {
        self.advance_clock(now.as_secs_f64());
        if self.bridge.wake.is_some_and(|wake| wake <= now) {
            self.bridge.wake = None;
            self.request_frame(out, FrameReason::ANIMATION);
        }
    }

    fn frame(&mut self, presentation: Instant, out: &mut RuntimeOutput) {
        self.advance_clock(presentation.as_secs_f64());
        if !self.bridge.visible {
            return;
        }
        let deadline = match self.build_frame() {
            Ok(deadline) => deadline,
            Err(error) => {
                self.diagnostics
                    .push(format!("aqua: build failed: {error}"));
                None
            }
        };
        let wake = deadline.map(Instant::from_secs_f64);
        self.bridge.wake = wake;
        out.wake_at = wake;
        self.emit_render(out);
        self.emit_access(out);
    }

    fn emit_render(&mut self, out: &mut RuntimeOutput) {
        let viewport = (Size::new(self.viewport.w, self.viewport.h), self.scale);
        if self.bridge.viewport != Some(viewport) {
            self.bridge.viewport = Some(viewport);
            out.render.push(RenderCommand::Viewport {
                size: viewport.0,
                scale: viewport.1,
            });
        }
        let diff = emit_scene_diff(&mut self.scene);
        if !diff.is_empty() {
            out.render.push(RenderCommand::Scene(diff));
        }
        out.render.extend(self.motion_commands());
    }

    fn emit_access(&mut self, out: &mut RuntimeOutput) {
        let AccessState::Attached(sent) = self.bridge.access else {
            return;
        };
        let Some(root) = self.tree.root() else {
            return;
        };
        let key = AccessKey {
            epoch: self.tree.invalidation_epoch(),
            focus: self.tree.focused().unwrap_or(root),
            scale_bits: self.scale.to_bits(),
        };
        if sent == Some(key) {
            return;
        }
        let Some((root, focus, nodes)) = self.a11y_nodes() else {
            return;
        };
        self.bridge.access = AccessState::Attached(Some(key));
        out.access = Some(AccessTreeUpdate { root, focus, nodes });
    }

    fn ime_probe(&self) -> Option<ImeProbe> {
        let node = self.tree.focused()?;
        if !self.text_input_allowed() {
            return None;
        }
        let NodeKind::Text(text) = &self.tree.get(node)?.kind else {
            return None;
        };
        let TextBacking::Editable(editor) = &text.backing else {
            return None;
        };
        let selection = editor.selection();
        Some(ImeProbe {
            node,
            revision: editor.revision(),
            anchor: selection.anchor,
            caret: selection.caret,
            composing: editor.is_composing(),
            area: self.ime_anchor(),
        })
    }

    fn ime_document(&self, area: Option<Rect>) -> Option<ImeDocument> {
        let surroundings = aqua_ui::events::ime_surroundings(&self.tree)?;
        Some(ImeDocument {
            window_start: surroundings.window_start,
            total_len: surroundings.total_len,
            text: surroundings.text,
            selection: Utf16Range::new(surroundings.selection.0, surroundings.selection.1),
            selection_reversed: surroundings.reversed,
            area: area.unwrap_or(Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            }),
        })
    }

    fn sync_ime(&mut self, out: &mut RuntimeOutput) {
        let probe = self.ime_probe();
        let state = self.bridge.ime;
        let command = match (state, probe) {
            (ImeState::Off, None) => None,
            (ImeState::On(_), None) => {
                self.bridge.ime = ImeState::Off;
                Some(ImeCommand::Disable)
            }
            (ImeState::Off, Some(probe)) => self.ime_document(probe.area).map(|document| {
                self.bridge.ime = ImeState::On(probe);
                ImeCommand::Enable(document)
            }),
            (ImeState::On(previous), Some(probe)) if previous == probe => None,
            (ImeState::On(previous), Some(probe)) => {
                self.ime_document(probe.area).map(|document| {
                    self.bridge.ime = ImeState::On(probe);
                    match previous.node == probe.node {
                        true => ImeCommand::Update(document),
                        false => ImeCommand::Enable(document),
                    }
                })
            }
        };
        out.ime.extend(command);
        let area = probe.and_then(|probe| probe.area);
        if area.is_some() && area != self.bridge.ime_area {
            out.ime_area = area;
        }
        self.bridge.ime_area = area;
    }

    fn sync_shell(&mut self, out: &mut RuntimeOutput) {
        let Some(window) = self.bridge.window else {
            return;
        };
        let cursor = self.cursor();
        if self.bridge.cursor != Some(cursor) {
            self.bridge.cursor = Some(cursor);
            out.shell.push(ShellCommand::SetCursor {
                window,
                icon: cursor,
            });
        }
        if self.bridge.regions != self.chrome_regions {
            self.bridge.regions = self.chrome_regions.clone();
            if let Some(regions) = self.bridge.regions.clone() {
                out.shell
                    .push(ShellCommand::SetDragRegions { window, regions });
            }
        }
    }
}

impl Machine for Runtime {
    type Input = RuntimeInput;
    type Output = RuntimeOutput;

    fn step(&mut self, input: RuntimeInput) -> RuntimeOutput {
        let mut out = RuntimeOutput::default();
        match input {
            RuntimeInput::Shell(event) => self.shell_event(event, &mut out),
            RuntimeInput::Ime(event) => self.ime_event(event, &mut out),
            RuntimeInput::Access(action) => self.access_action(action, &mut out),
            RuntimeInput::AccessClient { connected } => {
                self.bridge.access = match connected {
                    true => AccessState::Attached(None),
                    false => AccessState::Detached,
                };
                self.request_frame(&mut out, FrameReason::EXPLICIT);
            }
            RuntimeInput::Gpu(event) => self.gpu_event(event, &mut out),
            RuntimeInput::Render(event) => self.render_event(event, &mut out),
            RuntimeInput::Tick(now) => self.tick(now, &mut out),
        }
        self.sync_ime(&mut out);
        self.sync_shell(&mut out);
        out.shell
            .splice(0..0, std::mem::take(&mut self.bridge.queued));
        out
    }
}
