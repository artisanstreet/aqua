use std::collections::HashMap;
use std::sync::Arc;

use aqua_core::shape::{ShapeProgram, ShapeTransition};
use aqua_core::{DisplayItem, Rect};

const MAX_SHAPES: usize = 64;

fn curve(first: f32, second: f32, t: f32) -> f32 {
    let inverse = 1.0 - t;
    3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
}

fn ease(progress: f32, controls: [f32; 4]) -> f32 {
    let [x1, y1, x2, y2] = controls;
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..24 {
        let middle = (low + high) * 0.5;
        match curve(x1, x2, middle) < progress {
            true => low = middle,
            false => high = middle,
        }
    }
    match progress {
        p if p <= 0.0 => 0.0,
        p if p >= 1.0 => 1.0,
        _ => curve(y1, y2, (low + high) * 0.5),
    }
}

struct Shape {
    program: Arc<ShapeProgram>,
    bounds: Rect,
    from: Rect,
    target: Rect,
    started: f64,
    transition: ShapeTransition,
    painted: Option<Rect>,
    commands: Vec<DisplayItem>,
    seen: bool,
}

impl Shape {
    fn sample(&self, now: f64) -> (Rect, bool) {
        let duration = self.transition.duration.as_secs_f64();
        let fraction = match duration > 0.0 {
            true => ((now - self.started).max(0.0) / duration).min(1.0) as f32,
            false => 1.0,
        };
        let amount = ease(fraction, self.transition.easing);
        let mix = |from: f32, to: f32| from + (to - from) * amount;
        (
            Rect {
                x: mix(self.from.x, self.target.x),
                y: mix(self.from.y, self.target.y),
                w: mix(self.from.w, self.target.w),
                h: mix(self.from.h, self.target.h),
            },
            fraction < 1.0 && self.from != self.target,
        )
    }
}

#[derive(Default)]
pub struct Shapes {
    entries: HashMap<(u32, u32), Shape>,
}

impl Shapes {
    pub fn begin_frame(&mut self) {
        for entry in self.entries.values_mut() {
            entry.seen = false;
        }
    }

    pub fn end_frame(&mut self) {
        self.entries.retain(|_, entry| entry.seen);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn paint(
        &mut self,
        id: (u32, u32),
        bounds: Rect,
        program: &Arc<ShapeProgram>,
        target: Rect,
        transition: ShapeTransition,
        now: f64,
    ) -> (Vec<DisplayItem>, bool) {
        if self.entries.len() >= MAX_SHAPES && !self.entries.contains_key(&id) {
            return ((program.paint)(bounds, target, &program.parameters), false);
        }
        let entry = self.entries.entry(id).or_insert_with(|| Shape {
            program: Arc::clone(program),
            bounds,
            from: target,
            target,
            started: now,
            transition,
            painted: None,
            commands: Vec::new(),
            seen: true,
        });
        if entry.bounds != bounds || entry.program != *program {
            entry.bounds = bounds;
            entry.program = Arc::clone(program);
            entry.from = target;
            entry.target = target;
            entry.painted = None;
            entry.commands.clear();
        } else if entry.target != target {
            let (current, active) = entry.sample(now);
            entry.from = match transition.vertical_reveal
                && !active
                && entry.target.h <= 0.001
                && target.h > 0.001
            {
                true => {
                    Rect::from_edges(target.x, target.bottom(), target.right(), target.bottom())
                }
                false => current,
            };
            entry.target = target;
            entry.started = now;
        }
        entry.transition = transition;
        if transition.duration.is_zero() {
            entry.from = target;
        }
        entry.seen = true;
        let (sampled, active) = entry.sample(now);
        if entry.painted != Some(sampled) {
            entry.commands = (program.paint)(bounds, sampled, &program.parameters);
            entry.painted = Some(sampled);
        }
        (entry.commands.clone(), active)
    }
}
