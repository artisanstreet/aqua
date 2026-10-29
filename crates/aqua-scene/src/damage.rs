use crate::scene::{DisplayItem, Rect};

/// Above this many precise regions, damage collapses to whole-frame.
/// Keeps every accumulation path O(n) and every take O(1)-ish: a 10k-node
/// mount must not minute-spin in a greedy coalescer. Conservative and
/// correct: fewer, larger regions never under-cover.
pub const MAX_PRECISE_REGIONS: usize = 64;

/// Why a region needs work. Paint invalidation implies damage; damage alone
/// (e.g. a scroll offset or overlay move) does not imply re-rasterization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidationReason {
    ContentChanged,
    GeometryMoved,
    EffectParamChanged,
    BackdropDependencyChanged,
    ScaleChanged,
    StructuralChanged,
}

#[derive(Clone, Debug)]
pub struct DamageSet {
    /// Regions needing re-composition this frame (physical pixels).
    pub damage: Vec<Rect>,
    /// Regions needing re-rasterization. Every entry must also be covered
    /// by `damage` (checked in tests).
    pub raster_invalid: Vec<Rect>,
    pub reasons: Vec<InvalidationReason>,
    /// Conservative whole-frame invalidation. When true, `damage` and
    /// `raster_invalid` are exactly `[frame]` and partial regions are
    /// irrelevant. Structural and scale changes set this.
    pub full: bool,
    /// Frame size in physical pixels (meaningful when `full`).
    pub frame: Rect,
}

impl Default for DamageSet {
    fn default() -> Self {
        Self {
            damage: Vec::new(),
            raster_invalid: Vec::new(),
            reasons: Vec::new(),
            full: false,
            frame: Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        }
    }
}

impl DamageSet {
    pub fn is_clean(&self) -> bool {
        !self.full && self.damage.is_empty()
    }

    /// Coalesce overlapping regions to bound the region count. Full-frame
    /// sets are already minimal and untouched.
    pub fn coalesce(&mut self, max_regions: usize) {
        if self.full {
            return;
        }
        coalesce_into(&mut self.damage, max_regions);
        coalesce_into(&mut self.raster_invalid, max_regions);
    }

    pub fn bounding_box(&self) -> Option<Rect> {
        if self.full {
            return Some(self.frame);
        }
        self.damage.iter().fold(None, |acc: Option<Rect>, &r| {
            Some(match acc {
                None => r,
                Some(a) => a.union(r),
            })
        })
    }
}

fn coalesce_into(regions: &mut Vec<Rect>, max_regions: usize) {
    if regions.len() <= max_regions {
        return;
    }
    // Bounded fallback: one bounding union, O(n). Greedy all-pairs below
    // is O(n^3)-ish and must never see large inputs (mount storms).
    if regions.len() > MAX_PRECISE_REGIONS {
        let mut iter = regions.drain(..);
        let first = iter.next().unwrap();
        let u = iter.fold(first, |a, r| a.union(r));
        regions.push(u);
        return;
    }
    loop {
        if regions.len() <= max_regions {
            break;
        }
        let mut best: Option<(usize, usize)> = None;
        let mut best_cost = f32::INFINITY;
        for i in 0..regions.len() {
            for j in (i + 1)..regions.len() {
                let u = regions[i].union(regions[j]);
                let cost = u.w * u.h - regions[i].w * regions[i].h - regions[j].w * regions[j].h;
                if cost < best_cost {
                    best_cost = cost;
                    best = Some((i, j));
                }
            }
        }
        match best {
            Some((i, j)) => {
                let u = regions[i].union(regions[j]);
                regions[i] = u;
                regions.swap_remove(j);
            }
            None => break,
        }
    }
}

/// Accumulates damage for one frame from scene changes.
#[derive(Debug)]
pub struct DamageTracker {
    set: DamageSet,
    /// Scale factor logical -> physical used for the accumulated set.
    scale: f32,
    frame_phys: Rect,
}

impl DamageTracker {
    pub fn new(scale: f32) -> Self {
        Self {
            set: DamageSet::default(),
            scale,
            frame_phys: Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
        }
    }

    pub fn set_frame_size(&mut self, width_phys: f32, height_phys: f32) {
        self.frame_phys = Rect {
            x: 0.0,
            y: 0.0,
            w: width_phys,
            h: height_phys,
        };
        self.set.frame = self.frame_phys;
    }

    /// Explicit whole-frame invalidation for structural/scale changes where
    /// precise regions are not (yet) computable. Correct, never under-covers.
    pub fn invalidate_full(&mut self, reason: InvalidationReason) {
        self.set.full = true;
        self.set.reasons.push(reason);
    }

    /// Bound accumulation: overflow collapses to whole-frame (conservative,
    /// correct) instead of growing damage unboundedly.
    fn bound(&mut self, reason: InvalidationReason) {
        if !self.set.full
            && (self.set.damage.len() > MAX_PRECISE_REGIONS
                || self.set.raster_invalid.len() > MAX_PRECISE_REGIONS)
        {
            self.invalidate_full(reason);
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() > f32::EPSILON {
            // Whole-frame invalidation on scale change: raster caches keyed
            // on physical pixels are all stale (damage AND raster).
            self.invalidate_full(InvalidationReason::ScaleChanged);
            self.scale = scale;
        }
    }

    /// Record a changed item.
    ///
    /// INVARIANT: both the item's own footprint and `old_effect_bounds` must
    /// already include effect expansion (shadow offset+blur, blur padding).
    /// The renderer guarantees this via effect-inclusive bounds helpers; a
    /// bare content rect here would under-cover and is a caller bug.
    pub fn record_change(&mut self, new_item: &DisplayItem, old_effect_bounds: Option<Rect>) {
        if self.set.full {
            self.set.reasons.push(reason_of(new_item));
            return;
        }
        let s = self.scale;
        let to_phys = |r: Rect| Rect {
            x: r.x * s,
            y: r.y * s,
            w: r.w * s,
            h: r.h * s,
        };
        match new_item {
            DisplayItem::Rect { .. } | DisplayItem::External { .. } => {
                // Footprints are effect-inclusive by caller invariant; the
                // item itself carries no separate bounds here, so the caller
                // must use record_rect_change for precise rects. Fall back to
                // full to stay correct.
                let _ = (to_phys, old_effect_bounds);
                self.invalidate_full(reason_of(new_item));
            }
            DisplayItem::BackdropBlur { .. } => {
                self.invalidate_full(reason_of(new_item));
            }
            DisplayItem::Text { .. } => {
                // Unmeasured text through this path is never precise: the
                // renderer must use record_text_change with measured ink.
                self.invalidate_full(InvalidationReason::ContentChanged);
            }
            DisplayItem::PushClip { .. }
            | DisplayItem::PushOpacity(_)
            | DisplayItem::PopClip
            | DisplayItem::PopOpacity => {
                self.invalidate_full(InvalidationReason::StructuralChanged);
            }
        }
    }

    /// Precise rect change with effect-inclusive footprints (logical px).
    pub fn record_rect_change(&mut self, new_footprint: Rect, old_footprint: Option<Rect>) {
        if self.set.full {
            self.set.reasons.push(InvalidationReason::ContentChanged);
            return;
        }
        let s = self.scale;
        let to_phys = |r: Rect| Rect {
            x: r.x * s,
            y: r.y * s,
            w: r.w * s,
            h: r.h * s,
        };
        self.set.reasons.push(InvalidationReason::ContentChanged);
        let np = to_phys(new_footprint);
        self.set.damage.push(np);
        self.set.raster_invalid.push(np);
        if let Some(old) = old_footprint {
            let op = to_phys(old);
            self.set.damage.push(op);
            self.set.raster_invalid.push(op);
        }
        self.bound(InvalidationReason::ContentChanged);
    }

    /// Measured text change: old and new ink rects (logical px) from the
    /// text provider's layout measurement. Both footprints repaint, so
    /// cleared old glyphs can never ghost.
    pub fn record_text_change(&mut self, old_ink: Option<Rect>, new_ink: Rect) {
        if self.set.full {
            self.set.reasons.push(InvalidationReason::ContentChanged);
            return;
        }
        let s = self.scale;
        let to_phys = |r: Rect| Rect {
            x: r.x * s,
            y: r.y * s,
            w: r.w * s,
            h: r.h * s,
        };
        self.set.reasons.push(InvalidationReason::ContentChanged);
        let np = to_phys(new_ink);
        self.set.damage.push(np);
        self.set.raster_invalid.push(np);
        if let Some(old) = old_ink {
            let op = to_phys(old);
            self.set.damage.push(op);
            self.set.raster_invalid.push(op);
        }
        self.bound(InvalidationReason::ContentChanged);
    }

    /// A stationary backdrop blur depends on content behind it: when the
    /// backdrop region overlaps new damage, the effect output is stale.
    pub fn record_backdrop_dependency(&mut self, blur_bounds_logical: Rect) {
        if self.set.full {
            return;
        }
        let s = self.scale;
        self.set
            .reasons
            .push(InvalidationReason::BackdropDependencyChanged);
        self.set.damage.push(Rect {
            x: blur_bounds_logical.x * s,
            y: blur_bounds_logical.y * s,
            w: blur_bounds_logical.w * s,
            h: blur_bounds_logical.h * s,
        });
        self.bound(InvalidationReason::BackdropDependencyChanged);
    }

    /// True when accumulated damage overlaps `phys_bounds`: used to decide
    /// whether an unchanged blur region must recompute against new backdrop.
    /// A `full` set overlaps everything.
    pub fn damage_overlaps(&self, phys_bounds: Rect) -> bool {
        if self.set.full {
            return true;
        }
        self.set
            .damage
            .iter()
            .any(|d| rects_overlap(*d, phys_bounds))
    }

    pub fn take(&mut self) -> DamageSet {
        let mut out = DamageSet {
            damage: Vec::new(),
            raster_invalid: Vec::new(),
            reasons: Vec::new(),
            full: self.set.full,
            frame: self.frame_phys,
        };
        std::mem::swap(&mut out.damage, &mut self.set.damage);
        std::mem::swap(&mut out.raster_invalid, &mut self.set.raster_invalid);
        std::mem::swap(&mut out.reasons, &mut self.set.reasons);
        self.set.full = false;
        if out.full {
            out.damage = vec![self.frame_phys];
            out.raster_invalid = vec![self.frame_phys];
        }
        out.coalesce(16);
        out
    }
}

fn reason_of(item: &DisplayItem) -> InvalidationReason {
    match item {
        DisplayItem::BackdropBlur { .. } => InvalidationReason::EffectParamChanged,
        DisplayItem::PushClip { .. }
        | DisplayItem::PushOpacity(_)
        | DisplayItem::PopClip
        | DisplayItem::PopOpacity => InvalidationReason::StructuralChanged,
        _ => InvalidationReason::ContentChanged,
    }
}

fn rects_overlap(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::DisplayItem;
    use aqua_core::Oklch;

    fn red() -> Oklch {
        Oklch::from_hex("#ff0000").unwrap()
    }

    #[test]
    fn clean_tracker_reports_no_work() {
        let mut t = DamageTracker::new(1.0);
        assert!(t.take().is_clean());
    }

    #[test]
    fn rect_change_covers_old_and_new_effect_bounds() {
        let mut t = DamageTracker::new(2.0);
        t.set_frame_size(800.0, 600.0);
        // Effect-inclusive footprints: content rect + shadow (offset 4, blur 6).
        t.record_rect_change(
            Rect {
                x: 10.0,
                y: 10.0,
                w: 30.0,
                h: 30.0,
            },
            Some(Rect {
                x: 50.0,
                y: 50.0,
                w: 30.0,
                h: 30.0,
            }),
        );
        let set = t.take();
        assert!(!set.is_clean());
        assert!(!set.full);
        assert_eq!(set.damage.len(), 2);
        assert_eq!(set.damage[0].x, 20.0); // scale 2
        for r in &set.raster_invalid {
            assert!(set.damage.iter().any(|d| {
                d.x <= r.x && d.y <= r.y && d.x + d.w >= r.x + r.w && d.y + d.h >= r.y + r.h
            }));
        }
    }

    #[test]
    fn text_change_repaints_old_and_new_ink() {
        let mut t = DamageTracker::new(1.0);
        t.set_frame_size(800.0, 600.0);
        t.record_text_change(
            Some(Rect {
                x: 0.0,
                y: 0.0,
                w: 60.0,
                h: 20.0,
            }),
            Rect {
                x: 100.0,
                y: 100.0,
                w: 80.0,
                h: 20.0,
            },
        );
        let set = t.take();
        assert_eq!(set.damage.len(), 2);
        // Old region covered: no ghosting.
        assert!(set.damage.iter().any(|d| d.x == 0.0 && d.w == 60.0));
        assert!(set.damage.iter().any(|d| d.x == 100.0 && d.w == 80.0));
    }

    #[test]
    fn structural_change_is_full_not_empty() {
        let mut t = DamageTracker::new(1.0);
        t.set_frame_size(400.0, 300.0);
        t.record_change(&DisplayItem::PushOpacity(0.5), None);
        let set = t.take();
        assert!(set.full);
        assert!(!set.is_clean());
        assert_eq!(
            set.bounding_box(),
            Some(Rect {
                x: 0.0,
                y: 0.0,
                w: 400.0,
                h: 300.0
            })
        );
        assert!(!set.raster_invalid.is_empty());
    }

    #[test]
    fn scale_change_invalidates_raster_too() {
        let mut t = DamageTracker::new(1.0);
        t.set_frame_size(400.0, 300.0);
        t.set_scale(2.0);
        let set = t.take();
        assert!(set.full);
        assert!(set.reasons.contains(&InvalidationReason::ScaleChanged));
        assert!(!set.raster_invalid.is_empty());
    }

    #[test]
    fn unmeasured_text_path_is_full() {
        let mut t = DamageTracker::new(1.0);
        t.set_frame_size(400.0, 300.0);
        t.record_change(
            &DisplayItem::Text {
                at: [0.0, 0.0],
                run: crate::scene::TextRun { id: 1, revision: 1 },
                color: red(),
                opacity: 1.0,
            },
            None,
        );
        assert!(t.take().full);
    }

    #[test]
    fn coalesce_bounds_region_count() {
        let mut set = DamageSet::default();
        for i in 0..40 {
            set.damage.push(Rect {
                x: i as f32 * 5.0,
                y: 0.0,
                w: 4.0,
                h: 4.0,
            });
        }
        set.coalesce(16);
        assert!(set.damage.len() <= 16);
    }

    #[test]
    fn mass_invalidation_collapses_to_full_frame() {
        // 10k regions must NOT spin in the greedy coalescer: accumulation
        // overflows to whole-frame (bounded), take() stays O(1)-ish.
        // No timing assert (fragile); the full flag IS the bound proof.
        let mut t = DamageTracker::new(1.0);
        t.set_frame_size(1024.0, 1024.0);
        for i in 0..10_000 {
            t.record_rect_change(
                Rect {
                    x: i as f32,
                    y: 0.0,
                    w: 4.0,
                    h: 4.0,
                },
                None,
            );
        }
        let set = t.take();
        assert!(set.full, "mass damage overflows to full-frame");
        assert_eq!(set.damage.len(), 1);
        assert_eq!(set.raster_invalid.len(), 1);
        // And the tracker recovered: subsequent precise damage works again.
        t.record_rect_change(
            Rect {
                x: 1.0,
                y: 1.0,
                w: 2.0,
                h: 2.0,
            },
            None,
        );
        let set2 = t.take();
        assert!(!set2.full);
        assert_eq!(set2.damage.len(), 1);
    }

    #[test]
    fn coalesce_large_set_is_single_union() {
        let mut set = DamageSet::default();
        for i in 0..500 {
            set.damage.push(Rect {
                x: i as f32 * 10.0,
                y: 0.0,
                w: 4.0,
                h: 4.0,
            });
        }
        set.coalesce(16);
        assert_eq!(set.damage.len(), 1, "bounded O(n) union fallback");
        let bb = set.bounding_box().unwrap();
        assert_eq!(bb.w, 499.0 * 10.0 + 4.0);
    }
}
