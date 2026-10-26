//! Bounded GPU resource pools and render diagnostics.
//!
//! The scratch texture pool (blur/group intermediates) is byte- and
//! count-bounded with LRU eviction. The pool tracks live vs cached bytes
//! honestly; frame pacing and in-flight-frame caps belong to the platform
//! scheduler, not this budget (see below for exactly what is covered).

use std::collections::{HashMap, VecDeque};

/// Budget for the renderer's SCRATCH texture pool (blur/group/snapshot
/// intermediates) and its pure-logic model. Coverage, honestly stated:
/// - INCLUDED: pooled scratch textures, live + cached.
/// - EXCLUDED: the base framebuffer, surface swapchains, the glyph atlas,
///   media caches, upload staging buffers. Those have their own owners and
///   budgets; this cap claims none of them. There is no separate
///   pending-release category: released-but-reusable entries count as
///   cached until evicted.
#[derive(Clone, Copy, Debug)]
pub struct ResourceBudget {
    /// Hard cap on scratch-pool texture bytes (live + cached).
    pub max_intermediate_bytes: u64,
    /// Maximum pooled textures kept for reuse.
    pub max_pooled_textures: usize,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            max_intermediate_bytes: 128 * 1024 * 1024,
            max_pooled_textures: 16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureDesc {
    pub width: u32,
    pub height: u32,
    /// wgpu format Discriminant as u8 tag (kept comparable without wgpu dep
    /// in pure-logic tests; renderer maps to real formats).
    pub format_tag: u8,
    pub mipmaps: bool,
}

impl TextureDesc {
    pub fn rgba8(w: u32, h: u32) -> Self {
        Self {
            width: w.max(1),
            height: h.max(1),
            format_tag: 0,
            mipmaps: false,
        }
    }

    /// Checked byte size; `None` on overflow or absurd dimensions.
    pub fn checked_bytes(&self) -> Option<u64> {
        // RGBA8 baseline; format_tag 1 = R16Float (8 B/px) for HDR scratch.
        let bpp: u64 = if self.format_tag == 1 { 8 } else { 4 };
        (self.width as u64)
            .checked_mul(self.height as u64)?
            .checked_mul(bpp)
    }

    /// Byte size, saturating (use `checked_bytes` for admission control).
    pub fn bytes(&self) -> u64 {
        self.checked_bytes().unwrap_or(u64::MAX)
    }

    /// Reject dimensions that could never fit any real budget (defense
    /// against arbitrary `u32` sizes before multiplication).
    pub fn dimensions_sane(&self) -> bool {
        self.width <= 16384 && self.height <= 16384 && self.checked_bytes().is_some()
    }
}

/// Pure-logic pool model: admission/eviction decisions without a GPU.
/// The renderer mirrors these decisions with real wgpu textures.
///
/// Accounting invariant: `allocated_bytes = live + cached`. Released
/// textures STILL occupy memory until evicted, so the budget caps ALL
/// resident resources, not just live ones. Every resident id carries its
/// byte size; duplicate/unknown releases are rejected instead of silently
/// corrupting ownership.
#[derive(Debug, Default)]
pub struct TexturePoolModel {
    budget: ResourceBudget,
    /// Live (checked-out) ids -> (desc, bytes).
    live: HashMap<u64, (TextureDesc, u64)>,
    /// Cached (pooled, reusable) ids -> (desc, bytes).
    cached: HashMap<u64, (TextureDesc, u64)>,
    /// Per-desc free lists for O(1) hit lookup.
    free_by_desc: HashMap<TextureDesc, Vec<u64>>,
    lru: VecDeque<u64>,
    next_id: u64,
    pub evictions: u64,
    pub hits: u64,
    pub misses: u64,
    /// Rejected duplicate/unknown releases (diagnostic: caller bug).
    pub bad_releases: u64,
}

impl TexturePoolModel {
    pub fn new(budget: ResourceBudget) -> Self {
        Self {
            budget,
            ..Default::default()
        }
    }

    /// All resident bytes: live + cached + (pending-release == cached here).
    pub fn allocated_bytes(&self) -> u64 {
        self.live.values().map(|(_, b)| *b).sum::<u64>()
            + self.cached.values().map(|(_, b)| *b).sum::<u64>()
    }

    /// Acquire a texture id for `desc`. Reuses a cached entry on hit.
    /// Returns `None` when admission would exceed the byte budget even
    /// after evicting all evictable (cached) entries.
    pub fn acquire(&mut self, desc: TextureDesc) -> Option<u64> {
        if !desc.dimensions_sane() {
            return None;
        }
        let bytes = desc.checked_bytes()?;
        // Hit path: reuse a cached entry of the same desc.
        if let Some(ids) = self.free_by_desc.get_mut(&desc) {
            while let Some(id) = ids.pop() {
                if let Some((_, b)) = self.cached.remove(&id) {
                    debug_assert_eq!(b, bytes);
                    self.lru.retain(|&x| x != id);
                    self.live.insert(id, (desc, bytes));
                    self.hits += 1;
                    return Some(id);
                }
            }
        }
        self.misses += 1;
        if bytes > self.budget.max_intermediate_bytes {
            return None; // single texture exceeds the whole budget
        }
        self.evict_to_fit(bytes);
        if self.allocated_bytes().checked_add(bytes)? > self.budget.max_intermediate_bytes {
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.live.insert(id, (desc, bytes));
        Some(id)
    }

    /// Release back to the pool (reuse after GPU completion in real code).
    /// Returns `false` (and counts `bad_releases`) for unknown ids or
    /// double releases instead of corrupting ownership.
    pub fn release(&mut self, desc: TextureDesc, id: u64) -> bool {
        // Validate BEFORE removing: a wrong-desc or unknown release must
        // leave the live entry (and its bytes) untouched. Removing first
        // would silently drop accounting and let residency exceed budget.
        let valid = matches!(self.live.get(&id), Some((live_desc, _)) if *live_desc == desc);
        if !valid {
            self.bad_releases += 1;
            return false;
        }
        let (_, bytes) = self.live.remove(&id).expect("validated live entry");
        self.cached.insert(id, (desc, bytes));
        self.free_by_desc.entry(desc).or_default().push(id);
        self.lru.push_back(id);
        self.evict_beyond_count_cap();
        true
    }

    /// Evict cached entries (LRU) until `need` more bytes fit——or nothing
    /// cached remains. Live entries are never evicted (still referenced).
    fn evict_to_fit(&mut self, need: u64) {
        while self
            .allocated_bytes()
            .checked_add(need)
            .is_none_or(|total| total > self.budget.max_intermediate_bytes)
        {
            match self.lru.pop_front() {
                Some(id) => {
                    if let Some((desc, _)) = self.cached.remove(&id) {
                        if let Some(ids) = self.free_by_desc.get_mut(&desc) {
                            ids.retain(|&x| x != id);
                        }
                        self.evictions += 1;
                    }
                }
                None => break,
            }
        }
    }

    fn evict_beyond_count_cap(&mut self) {
        while self.lru.len() > self.budget.max_pooled_textures {
            match self.lru.pop_front() {
                Some(id) => {
                    if let Some((desc, _)) = self.cached.remove(&id) {
                        if let Some(ids) = self.free_by_desc.get_mut(&desc) {
                            ids.retain(|&x| x != id);
                        }
                        self.evictions += 1;
                    }
                }
                None => break,
            }
        }
    }

    pub fn live_bytes(&self) -> u64 {
        self.live.values().map(|(_, b)| *b).sum()
    }

    pub fn cached_bytes(&self) -> u64 {
        self.cached.values().map(|(_, b)| *b).sum()
    }

    pub fn cached_count(&self) -> usize {
        self.cached.len()
    }
}

/// Per-frame and cumulative diagnostics. No performance claims beyond
/// measured values; the inspector reads these directly.
#[derive(Clone, Debug, Default)]
pub struct RenderDiagnostics {
    /// Frames composited into the offscreen base (includes headless
    /// readback work with no surface). Incremented by `Renderer::draw`.
    pub frames_produced: u64,
    /// Frames actually presented to a surface. Incremented ONLY by the
    /// facade via `Renderer::note_presented` on a real present.
    pub frames_presented: u64,
    pub frames_skipped_idle: u64,
    /// Last frame: scene nodes vs nodes re-encoded.
    pub nodes_total_last: usize,
    pub nodes_updated_last: usize,
    /// Bytes uploaded to GPU last frame (owned instance buffers ONLY).
    /// Glyph-atlas uploads are NOT included: glyphon exposes no counter and
    /// unknown bytes are never added to real totals (see text::TextStats).
    pub bytes_uploaded_last: u64,
    pub bytes_uploaded_total: u64,
    /// Text resolution counts last frame (exact, from text::TextStats).
    pub text_areas_last: u64,
    pub text_stale_last: u64,
    pub text_mismatch_last: u64,
    /// External layers currently retained (views/dims/bounds). Pruned to
    /// scene-referenced layers on structural walks; see prepare.
    pub external_layers_live: u64,
    /// Render/compute passes encoded last frame.
    pub passes_last: u32,
    pub intermediate_bytes_last: u64,
    pub pool_hits: u64,
    pub pool_misses: u64,
    pub pool_evictions: u64,
    pub last_frame_us: u64,
}

impl RenderDiagnostics {
    pub fn upload_ratio_last(&self, scene_bytes: u64) -> f32 {
        if scene_bytes == 0 {
            return 0.0;
        }
        self.bytes_uploaded_last as f32 / scene_bytes as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_reuses_released_texture() {
        let mut pool = TexturePoolModel::new(ResourceBudget::default());
        let desc = TextureDesc::rgba8(256, 256);
        let id = pool.acquire(desc).unwrap();
        assert!(pool.release(desc, id));
        let id2 = pool.acquire(desc).unwrap();
        assert_eq!(id, id2);
        assert_eq!(pool.hits, 1);
    }

    #[test]
    fn pool_refuses_over_budget() {
        let mut pool = TexturePoolModel::new(ResourceBudget {
            max_intermediate_bytes: 1024,
            max_pooled_textures: 4,
        });
        // 32x32 RGBA8 = 4096 B > 1024 budget.
        assert!(pool.acquire(TextureDesc::rgba8(32, 32)).is_none());
        assert!(pool.acquire(TextureDesc::rgba8(8, 8)).is_some());
    }

    #[test]
    fn released_bytes_still_count_until_evicted() {
        // Budget 1024. acquire 8x8 (256 B), release (still resident),
        // then 16x16 (1024 B) must evict the 8x8 first or be rejected —
        // never hold 1280 resident bytes.
        let mut pool = TexturePoolModel::new(ResourceBudget {
            max_intermediate_bytes: 1024,
            max_pooled_textures: 4,
        });
        let small = TextureDesc::rgba8(8, 8);
        let id = pool.acquire(small).unwrap();
        assert!(pool.release(small, id));
        assert_eq!(pool.allocated_bytes(), 256);
        let big = TextureDesc::rgba8(16, 16);
        let got = pool.acquire(big);
        assert!(got.is_some(), "eviction must make room");
        assert!(pool.evictions >= 1, "the 8x8 must have been evicted");
        assert!(
            pool.allocated_bytes() <= 1024,
            "budget holds: {}",
            pool.allocated_bytes()
        );
    }

    #[test]
    fn duplicate_and_unknown_releases_rejected() {
        let mut pool = TexturePoolModel::new(ResourceBudget::default());
        let desc = TextureDesc::rgba8(64, 64);
        assert!(!pool.release(desc, 999), "unknown id rejected");
        let id = pool.acquire(desc).unwrap();
        assert!(pool.release(desc, id));
        assert!(!pool.release(desc, id), "double release rejected");
        assert!(
            !pool.release(TextureDesc::rgba8(32, 32), id),
            "wrong desc rejected"
        );
        assert_eq!(pool.bad_releases, 3);
        // Pool state intact: the one cached entry still acquirable once.
        assert_eq!(pool.acquire(desc), Some(id));
        assert!(pool.acquire(desc).is_some_and(|new| new != id));
    }

    #[test]
    fn wrong_desc_release_preserves_live_accounting() {
        // Regression: release validated the desc AFTER removing the live
        // entry, so a wrong-desc release silently dropped live bytes and
        // later admissions could exceed the budget. Validate first.
        let mut pool = TexturePoolModel::new(ResourceBudget {
            max_intermediate_bytes: 1024,
            max_pooled_textures: 4,
        });
        let desc = TextureDesc::rgba8(16, 16); // 1024 B: fills the budget
        let id = pool.acquire(desc).unwrap();
        assert_eq!(pool.live_bytes(), 1024);
        // Wrong desc against a LIVE id: rejected AND entry preserved.
        assert!(!pool.release(TextureDesc::rgba8(8, 8), id));
        assert_eq!(pool.live_bytes(), 1024, "live bytes preserved");
        assert_eq!(pool.allocated_bytes(), 1024, "residency preserved");
        // Budget still enforced: nothing else fits while it is live.
        assert!(pool.acquire(TextureDesc::rgba8(8, 8)).is_none());
        // Valid release works afterwards and the entry is reusable.
        assert!(pool.release(desc, id));
        assert_eq!(pool.live_bytes(), 0);
        assert_eq!(pool.cached_bytes(), 1024);
        assert_eq!(pool.acquire(desc), Some(id));
    }

    #[test]
    fn insane_dimensions_rejected_without_overflow() {
        let mut pool = TexturePoolModel::new(ResourceBudget::default());
        assert!(TextureDesc::rgba8(u32::MAX, u32::MAX)
            .checked_bytes()
            .is_none());
        assert!(!TextureDesc::rgba8(u32::MAX, u32::MAX).dimensions_sane());
        assert!(pool
            .acquire(TextureDesc::rgba8(u32::MAX, u32::MAX))
            .is_none());
        assert!(pool
            .acquire(TextureDesc::rgba8(1_000_000, 1_000_000))
            .is_none());
    }

    #[test]
    fn pool_evicts_lru_beyond_count_cap() {
        let mut pool = TexturePoolModel::new(ResourceBudget {
            max_intermediate_bytes: 64 * 1024 * 1024,
            max_pooled_textures: 2,
        });
        let desc = TextureDesc::rgba8(64, 64);
        let ids: Vec<u64> = (0..4).map(|_| pool.acquire(desc).unwrap()).collect();
        for id in ids {
            pool.release(desc, id);
        }
        assert!(pool.cached_count() <= 2);
        assert!(pool.evictions >= 2);
    }

    #[test]
    fn full_hd_budget_math() {
        // A 3840x2160 RGBA8 target = ~31.6 MiB; default budget must at
        // least hold base + two blur scratch targets at 1080p.
        let base = TextureDesc::rgba8(3840, 2160).bytes();
        assert!(base > 30 * 1024 * 1024 && base < 34 * 1024 * 1024);
        let scratch = TextureDesc::rgba8(1920, 1080).bytes();
        let budget = ResourceBudget::default();
        assert!(budget.max_intermediate_bytes >= base + 2 * scratch);
    }
}
