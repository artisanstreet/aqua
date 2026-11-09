use std::sync::atomic::Ordering;

use aqua_core::{DisplayItem, Oklch, Rect, Size, TextRun};
use aqua_protocol::probe::{Metric, Phase};
use aqua_text::{FontFamily, RunId, SharedFonts, SharedRuns, TextLayout, TextStyle};

use super::history::{History, Sample, Summary, Thread, CAPACITY};
use crate::render::host::RenderStats;

const PAD: f32 = 12.0;
const TEXT: f32 = 11.0;
const LINE: f32 = 16.0;
const CHAR: f32 = TEXT * 0.6;
const GRAPH: f32 = 64.0;
const GAP: f32 = 8.0;
const MILLI: f64 = 1_000_000.0;

pub const SURFACE: Oklch = Oklch::new(0.16, 0.0, 0.0);
const TRACK: Oklch = Oklch::new(0.21, 0.0, 0.0);
const INK: Oklch = Oklch::new(0.94, 0.0, 0.0);
const DIM: Oklch = Oklch::new(0.68, 0.0, 0.0);
const BUDGET: Oklch = Oklch::new(0.86, 0.15, 95.0).with_alpha(0.8);
const GOOD: Oklch = Oklch::new(0.74, 0.16, 150.0);
const BAD: Oklch = Oklch::new(0.66, 0.21, 25.0);

#[must_use]
pub fn phase_color(phase: Phase) -> Oklch {
    let hue = match phase {
        Phase::Input => 250.0,
        Phase::Reactive => 290.0,
        Phase::Build => 40.0,
        Phase::Layout => 70.0,
        Phase::Text => 110.0,
        Phase::Lower => 150.0,
        Phase::Upload => 180.0,
        Phase::Plan => 205.0,
        Phase::Record => 225.0,
        Phase::Submit => 330.0,
        Phase::Present => 10.0,
        Phase::Wait => return DIM,
    };
    Oklch::new(0.72, 0.14, hue)
}

struct Label {
    at: [f32; 2],
    text: String,
    color: Oklch,
}

struct Line {
    id: RunId,
    revision: u64,
    text: String,
}

struct Sheet {
    x: f32,
    y: f32,
    width: f32,
    shapes: Vec<DisplayItem>,
    labels: Vec<Label>,
}

fn rect(x: f32, y: f32, w: f32, h: f32, fill: Oklch) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect { x, y, w, h },
        radius: 0.0,
        fill,
        border_width: 0.0,
        border_color: fill,
        shadow: None,
    }
}

fn ms(nanos: u64) -> String {
    format!("{:.2}", nanos as f64 / MILLI)
}

fn bytes(count: u64) -> String {
    match count {
        0..1_024 => format!("{count} B"),
        1_024..1_048_576 => format!("{:.1} KB", count as f64 / 1_024.0),
        _ => format!("{:.1} MB", count as f64 / 1_048_576.0),
    }
}

impl Sheet {
    fn inner(&self) -> f32 {
        self.width - 2.0 * PAD
    }

    fn fits(&self) -> usize {
        (self.inner() / CHAR).max(8.0) as usize
    }

    fn text(&mut self, text: String, color: Oklch) {
        self.labels.push(Label {
            at: [self.x + PAD, self.y],
            text,
            color,
        });
        self.y += LINE;
    }

    fn heading(&mut self, text: &str) {
        self.y += GAP / 2.0;
        self.text(text.to_owned(), INK);
    }

    fn graph(&mut self, values: &[Vec<(Oklch, u64)>], scale: u64, budget: Option<u64>) {
        let (left, top, width) = (self.x + PAD, self.y, self.inner());
        self.shapes.push(rect(left, top, width, GRAPH, TRACK));
        let bar = width / CAPACITY as f32;
        let start = CAPACITY.saturating_sub(values.len());
        let scale = scale.max(1) as f32;
        for (slot, stack) in values.iter().enumerate() {
            let x = left + (start + slot) as f32 * bar;
            let mut floor = top + GRAPH;
            for (color, nanos) in stack {
                let height = (*nanos as f32 / scale * GRAPH).min(floor - top);
                if height <= 0.0 {
                    continue;
                }
                floor -= height;
                self.shapes
                    .push(rect(x, floor, bar.max(1.0), height, *color));
            }
        }
        if let Some(budget) = budget {
            let y = top + GRAPH - (budget as f32 / scale * GRAPH).min(GRAPH);
            self.shapes.push(rect(left, y, width, 1.0, BUDGET));
        }
        let scale = format!("{} ms", ms(scale as u64));
        self.labels.push(Label {
            at: [
                left + width - scale.chars().count() as f32 * CHAR,
                top - LINE,
            ],
            text: scale,
            color: DIM,
        });
        self.y += GRAPH + GAP / 2.0;
    }

    fn legend(&mut self, phases: &[Phase]) {
        let mut x = self.x + PAD;
        for phase in phases {
            let width = 14.0 + phase.name().len() as f32 * CHAR + 10.0;
            if x + width > self.x + self.width - PAD {
                x = self.x + PAD;
                self.y += LINE;
            }
            self.shapes
                .push(rect(x, self.y + 4.0, 8.0, 8.0, phase_color(*phase)));
            self.labels.push(Label {
                at: [x + 12.0, self.y],
                text: phase.name().to_owned(),
                color: DIM,
            });
            x += width;
        }
        self.y += LINE;
    }
}

fn stacks(samples: &[&Sample]) -> (Vec<Vec<(Oklch, u64)>>, Vec<Phase>) {
    let shown: Vec<Phase> = Phase::ALL
        .into_iter()
        .filter(|phase| *phase != Phase::Wait)
        .filter(|phase| {
            samples
                .iter()
                .any(|sample| sample.phases[phase.index()] > 0)
        })
        .collect();
    let bars = samples
        .iter()
        .map(|sample| {
            shown
                .iter()
                .map(|phase| (phase_color(*phase), sample.phases[phase.index()]))
                .collect()
        })
        .collect();
    (bars, shown)
}

fn counters(sample: &Sample, metrics: &[Metric]) -> Vec<String> {
    metrics
        .chunks(2)
        .map(|pair| {
            pair.iter()
                .map(|metric| {
                    format!(
                        "{:<26}",
                        format!("{} {}", metric.name(), sample.metrics[metric.index()])
                    )
                })
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn note<'a>(sample: Option<&'a Sample>, prefix: &str) -> Option<&'a str> {
    sample?
        .notes
        .iter()
        .find_map(|note| note.strip_prefix(prefix))
}

fn clip(text: &str, chars: usize) -> String {
    match text.chars().count() > chars {
        true => format!("{}…", text.chars().take(chars - 1).collect::<String>()),
        false => text.to_owned(),
    }
}

fn frame_section(
    sheet: &mut Sheet,
    history: &History,
    thread: Thread,
    title: &str,
    budget: Option<u64>,
) -> Vec<Phase> {
    let samples: Vec<&Sample> = history.samples(thread).iter().collect();
    let (bars, phases) = stacks(&samples);
    let spread = super::history::Spread::of(samples.iter().map(|sample| sample.work()).collect());
    sheet.heading(&format!(
        "{title}  p50 {}  p95 {}  max {} ms",
        ms(spread.p50),
        ms(spread.p95),
        ms(spread.max)
    ));
    let scale = budget
        .map_or(4_000_000, |budget| budget * 2)
        .max(spread.p95 + spread.p95 / 4);
    sheet.graph(&bars, scale, budget);
    phases
}

fn render_lines(sheet: &mut Sheet, stats: &RenderStats) {
    let load = |counter: &std::sync::atomic::AtomicU64| counter.load(Ordering::Relaxed);
    sheet.text(
        format!(
            "frames full {}  partial {}  culled {}  damaged {} px",
            load(&stats.full_frames),
            load(&stats.partial_frames),
            load(&stats.culled_commands),
            load(&stats.damaged_pixels)
        ),
        DIM,
    );
    sheet.text(
        format!(
            "atlas mask {}  color {}  pages {}  uploads {}",
            bytes(load(&stats.atlas_mask_bytes)),
            bytes(load(&stats.atlas_color_bytes)),
            load(&stats.atlas_pages),
            bytes(load(&stats.atlas_upload_bytes))
        ),
        DIM,
    );
    sheet.text(
        format!(
            "glyphs rasterized {}  slugs {} ({})  device losses {}",
            load(&stats.glyph_rasterized),
            load(&stats.slug_created),
            bytes(load(&stats.slug_bytes)),
            load(&stats.recoveries)
        ),
        DIM,
    );
}

fn sheet(
    history: &History,
    summary: &Summary,
    stats: &RenderStats,
    viewport: Size,
    observed: Size,
) -> Sheet {
    let mut sheet = Sheet {
        x: 0.0,
        y: PAD,
        width: viewport.w,
        shapes: Vec::new(),
        labels: Vec::new(),
    };
    let refresh = summary.refresh;
    sheet.text(
        format!(
            "app {} frames/s  GPU {} submits/s",
            summary.app_updates_per_second, summary.presents_per_second
        ),
        INK,
    );
    sheet.text(
        format!(
            "refresh {}  missed {}",
            refresh.map_or_else(
                || "?".to_owned(),
                |refresh| format!("{} ms ({:.0} Hz)", ms(refresh), 1e9 / refresh as f64)
            ),
            summary.missed
        ),
        INK,
    );
    sheet.text(
        format!(
            "viewport {:.0}x{:.0}  app frames {}  render frames {}",
            observed.w,
            observed.h,
            history.samples(Thread::App).len(),
            history.samples(Thread::Render).len()
        ),
        DIM,
    );
    let mut legend = frame_section(&mut sheet, history, Thread::App, "app thread", refresh);
    for phase in frame_section(
        &mut sheet,
        history,
        Thread::Render,
        "render thread",
        refresh,
    ) {
        if !legend.contains(&phase) {
            legend.push(phase);
        }
    }
    legend.sort_by_key(|phase| phase.index());
    sheet.legend(&legend);
    let intervals = history.intervals();
    let budget = refresh.unwrap_or(summary.interval.p95.max(1));
    sheet.heading(&format!(
        "present interval  p50 {}  p95 {}  max {} ms",
        ms(summary.interval.p50),
        ms(summary.interval.p95),
        ms(summary.interval.max)
    ));
    let bars: Vec<Vec<(Oklch, u64)>> = intervals
        .iter()
        .map(|interval| {
            let color = match *interval > budget + budget / 2 {
                true => BAD,
                false => GOOD,
            };
            vec![(color, *interval)]
        })
        .collect();
    sheet.graph(&bars, budget.saturating_mul(3), refresh);
    let app = history.samples(Thread::App).back();
    let render = history.samples(Thread::Render).back();
    sheet.heading("last app frame");
    if let Some(app) = app {
        for line in counters(
            app,
            &[
                Metric::Layouts,
                Metric::NodesMeasured,
                Metric::NodesCommitted,
                Metric::NodesVisited,
            ],
        ) {
            sheet.text(line, DIM);
        }
        for line in counters(
            app,
            &[
                Metric::TextNodesChecked,
                Metric::TextRunsShaped,
                Metric::SceneOpsEmitted,
                Metric::ScenePatched,
            ],
        ) {
            sheet.text(line, DIM);
        }
        sheet.text(
            format!(
                "wall {} ms  heap {} allocs  {}",
                ms(app.wall),
                app.allocs,
                bytes(app.bytes)
            ),
            DIM,
        );
    }
    if let Some(reason) = note(app, "rebuild_reason=") {
        let fits = sheet.fits();
        sheet.text(clip(&format!("reason {reason}"), fits), DIM);
    }
    sheet.heading("last render frame");
    if let Some(render) = render {
        for line in counters(
            render,
            &[
                Metric::Commands,
                Metric::Glyphs,
                Metric::Passes,
                Metric::Instances,
            ],
        ) {
            sheet.text(line, DIM);
        }
        sheet.text(
            format!(
                "uploads {}  {}  barriers {}  pso {}/{}  atlas +{}",
                render.metrics[Metric::UploadTextures.index()],
                bytes(render.metrics[Metric::UploadBytes.index()]),
                render.metrics[Metric::Barriers.index()],
                render.metrics[Metric::PsoCompiled.index()],
                render.metrics[Metric::PsoLoaded.index()],
                bytes(render.metrics[Metric::AtlasGrowthBytes.index()])
            ),
            DIM,
        );
        sheet.text(
            format!(
                "wait {} ms  work {} ms",
                ms(render.phases[Phase::Wait.index()]),
                ms(render.work())
            ),
            DIM,
        );
    }
    if let Some(cause) = note(render, "render_cause=") {
        let fits = sheet.fits();
        sheet.text(clip(&format!("cause {cause}"), fits), DIM);
    }
    sheet.heading("renderer totals");
    render_lines(&mut sheet, stats);
    sheet
}

pub struct Panel {
    fonts: SharedFonts,
    runs: SharedRuns,
    lines: Vec<Line>,
}

impl Panel {
    #[must_use]
    pub fn new(fonts: SharedFonts, runs: SharedRuns) -> Self {
        Self {
            fonts,
            runs,
            lines: Vec::new(),
        }
    }

    pub fn items(
        &mut self,
        viewport: Size,
        stats: &RenderStats,
        refresh: Option<u64>,
        observed: Size,
    ) -> Vec<DisplayItem> {
        let Some(sheet) = super::with_history(|history| {
            let summary = history.summary(super::now(), refresh);
            sheet(history, &summary, stats, viewport, observed)
        }) else {
            return Vec::new();
        };
        let mut items = vec![rect(0.0, 0.0, viewport.w, viewport.h, SURFACE)];
        items.extend(sheet.shapes);
        let runs = self.sync(&sheet.labels);
        items.extend(
            sheet
                .labels
                .iter()
                .zip(runs)
                .map(|(label, run)| DisplayItem::Text {
                    at: label.at,
                    run,
                    color: label.color,
                    opacity: 1.0,
                }),
        );
        items
    }

    fn sync(&mut self, labels: &[Label]) -> Vec<TextRun> {
        let style = TextStyle {
            families: vec![
                FontFamily::Named("Cascadia Mono".to_owned()),
                FontFamily::Named("Consolas".to_owned()),
                FontFamily::Monospace,
            ],
            size_px: TEXT,
            line_height_px: LINE,
            ..TextStyle::default()
        };
        let shaped: Vec<Option<TextLayout>> = {
            let mut fonts = self.fonts.lock();
            labels
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    let current = self
                        .lines
                        .get(index)
                        .is_some_and(|line| line.text == label.text);
                    (!current).then(|| {
                        let mut layout = TextLayout::new();
                        layout.set_text(&label.text, &style);
                        layout.layout(&mut fonts);
                        layout
                    })
                })
                .collect()
        };
        let mut runs = self.runs.lock();
        for line in self.lines.drain(labels.len().min(self.lines.len())..) {
            runs.remove(line.id);
        }
        labels
            .iter()
            .zip(shaped)
            .enumerate()
            .map(|(index, (label, layout))| {
                match (self.lines.get_mut(index), layout) {
                    (Some(line), Some(layout)) => {
                        line.text.clone_from(&label.text);
                        line.revision = runs.update(line.id, layout).unwrap_or(line.revision);
                    }
                    (None, Some(layout)) => {
                        let (id, revision) = runs.insert(layout);
                        self.lines.push(Line {
                            id,
                            revision,
                            text: label.text.clone(),
                        });
                    }
                    (_, None) => {}
                }
                let line = &self.lines[index];
                TextRun {
                    id: line.id.0,
                    revision: line.revision,
                }
            })
            .collect()
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        let mut runs = self.runs.lock();
        for line in self.lines.drain(..) {
            runs.remove(line.id);
        }
    }
}
