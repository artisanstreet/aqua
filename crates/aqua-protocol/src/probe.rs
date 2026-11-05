use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Phase {
    Input,
    Reactive,
    Build,
    Layout,
    Text,
    Lower,
    Upload,
    Plan,
    Record,
    Submit,
    Present,
    Wait,
}

impl Phase {
    pub const ALL: [Phase; 12] = [
        Phase::Input,
        Phase::Reactive,
        Phase::Build,
        Phase::Layout,
        Phase::Text,
        Phase::Lower,
        Phase::Upload,
        Phase::Plan,
        Phase::Record,
        Phase::Submit,
        Phase::Present,
        Phase::Wait,
    ];

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Phase::Input => "input",
            Phase::Reactive => "reactive",
            Phase::Build => "build",
            Phase::Layout => "layout",
            Phase::Text => "text",
            Phase::Lower => "lower",
            Phase::Upload => "upload",
            Phase::Plan => "plan",
            Phase::Record => "record",
            Phase::Submit => "submit",
            Phase::Present => "present",
            Phase::Wait => "wait",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Metric {
    Layouts,
    NodesMeasured,
    NodesCommitted,
    LayoutNodesVisited,
    NodesVisited,
    TextNodesChecked,
    TextRunsShaped,
    TextSourceBytes,
    SceneOpsEmitted,
    SceneReused,
    ScenePatched,
    Commands,
    Glyphs,
    UploadTextures,
    UploadBytes,
    Passes,
    Instances,
    Barriers,
    TransientBytes,
    PsoCompiled,
    PsoLoaded,
    AtlasGrowthBytes,
}

impl Metric {
    pub const ALL: [Metric; 22] = [
        Metric::Layouts,
        Metric::NodesMeasured,
        Metric::NodesCommitted,
        Metric::LayoutNodesVisited,
        Metric::NodesVisited,
        Metric::TextNodesChecked,
        Metric::TextRunsShaped,
        Metric::TextSourceBytes,
        Metric::SceneOpsEmitted,
        Metric::SceneReused,
        Metric::ScenePatched,
        Metric::Commands,
        Metric::Glyphs,
        Metric::UploadTextures,
        Metric::UploadBytes,
        Metric::Passes,
        Metric::Instances,
        Metric::Barriers,
        Metric::TransientBytes,
        Metric::PsoCompiled,
        Metric::PsoLoaded,
        Metric::AtlasGrowthBytes,
    ];

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Metric::Layouts => "layouts",
            Metric::NodesMeasured => "nodes_measured",
            Metric::NodesCommitted => "nodes_committed",
            Metric::LayoutNodesVisited => "layout_visited",
            Metric::NodesVisited => "nodes_visited",
            Metric::TextNodesChecked => "text_nodes_checked",
            Metric::TextRunsShaped => "text_runs_shaped",
            Metric::TextSourceBytes => "text_source_bytes",
            Metric::SceneOpsEmitted => "scene_ops_emitted",
            Metric::SceneReused => "scene_reused",
            Metric::ScenePatched => "scene_items_patched",
            Metric::Commands => "commands",
            Metric::Glyphs => "glyphs",
            Metric::UploadTextures => "upload_textures",
            Metric::UploadBytes => "upload_bytes",
            Metric::Passes => "passes",
            Metric::Instances => "instances",
            Metric::Barriers => "barriers",
            Metric::TransientBytes => "transient_bytes",
            Metric::PsoCompiled => "pso_compiled",
            Metric::PsoLoaded => "pso_loaded",
            Metric::AtlasGrowthBytes => "atlas_growth_bytes",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Enter(Phase),
    Exit(Phase),
    Count(Metric, u64),
}

pub type Sink = fn(Event);

static SINK: OnceLock<Sink> = OnceLock::new();

pub fn install(sink: Sink) -> bool {
    SINK.set(sink).is_ok()
}

#[must_use]
pub fn active() -> bool {
    SINK.get().is_some()
}

pub fn count(metric: Metric, value: u64) {
    if let Some(sink) = SINK.get() {
        sink(Event::Count(metric, value));
    }
}

#[must_use]
pub struct Span {
    phase: Phase,
    sink: Option<Sink>,
}

pub fn span(phase: Phase) -> Span {
    let sink = SINK.get().copied();
    if let Some(sink) = sink {
        sink(Event::Enter(phase));
    }
    Span { phase, sink }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(sink) = self.sink {
            sink(Event::Exit(self.phase));
        }
    }
}
