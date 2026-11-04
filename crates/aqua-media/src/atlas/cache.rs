use std::collections::HashMap;
use std::hash::Hash;

use super::shelf::ShelfPacker;
use super::types::*;

const MASK_SHARED_EDGE_CAP: u32 = 1024;
const COLOR_SHARED_EDGE_CAP: u32 = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasConfig {
    pub format: PageFormat,
    pub base_edge: u32,
    pub max_edge: u32,
    pub max_shared_pages: usize,
    pub budget_bytes: u64,
}

impl AtlasConfig {
    pub fn glyphs() -> Self {
        Self {
            format: PageFormat::Mask8,
            base_edge: 256,
            max_edge: 4096,
            max_shared_pages: 8,
            budget_bytes: 8 * 1024 * 1024,
        }
    }

    pub fn images() -> Self {
        Self {
            format: PageFormat::Color8,
            base_edge: 256,
            max_edge: 4096,
            max_shared_pages: 8,
            budget_bytes: 24 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSummary {
    pub width: u32,
    pub height: u32,
    pub shared: bool,
    pub live: u32,
    pub idle_frames: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AtlasStats {
    pub pages: u32,
    pub resident_bytes: u64,
    pub entries: u32,
    pub hits: u64,
    pub inserts: u64,
    pub page_evictions: u64,
    pub entries_evicted: u64,
    pub over_budget_pages: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageKind {
    Shared,
    Dedicated,
}

struct Page<K> {
    kind: PageKind,
    width: u32,
    height: u32,
    packer: ShelfPacker,
    keys: Vec<K>,
    live: u32,
    last_used: u64,
}

impl<K> Page<K> {
    fn bytes(&self, format: PageFormat) -> u64 {
        self.width as u64 * self.height as u64 * format.bytes_per_texel() as u64
    }
}

struct Entry {
    slot: Slot,
    last_used: u64,
}

pub struct Atlas<K> {
    config: AtlasConfig,
    frame: u64,
    pages: Vec<Option<Page<K>>>,
    free_indices: Vec<u32>,
    entries: HashMap<K, Entry>,
    pending: AtlasUpdates,
    evicted: Vec<K>,
    stats: AtlasStats,
}

impl<K: Clone + Eq + Hash> Atlas<K> {
    pub fn new(config: AtlasConfig) -> Self {
        Self {
            config,
            frame: 1,
            pages: Vec::new(),
            free_indices: Vec::new(),
            entries: HashMap::new(),
            pending: AtlasUpdates::default(),
            evicted: Vec::new(),
            stats: AtlasStats::default(),
        }
    }

    pub fn config(&self) -> &AtlasConfig {
        &self.config
    }

    pub fn format(&self) -> PageFormat {
        self.config.format
    }

    pub fn stats(&self) -> AtlasStats {
        AtlasStats {
            pages: self.live_pages() as u32,
            entries: self.entries.len() as u32,
            ..self.stats
        }
    }

    pub fn resident_bytes(&self) -> u64 {
        self.stats.resident_bytes
    }

    pub fn page_layout(&self) -> Vec<PageSummary> {
        self.pages
            .iter()
            .flatten()
            .map(|page| PageSummary {
                width: page.width,
                height: page.height,
                shared: page.kind == PageKind::Shared,
                live: page.live,
                idle_frames: self.frame.saturating_sub(page.last_used),
            })
            .collect()
    }

    pub fn page_keys(&self) -> Vec<Vec<K>> {
        self.pages
            .iter()
            .flatten()
            .map(|page| page.keys.clone())
            .collect()
    }

    pub fn frame(&self) -> u64 {
        self.frame
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    pub fn take_updates(&mut self) -> AtlasUpdates {
        std::mem::take(&mut self.pending)
    }

    pub fn take_evicted(&mut self) -> Vec<K> {
        std::mem::take(&mut self.evicted)
    }

    pub fn resolve(
        &mut self,
        key: K,
        produce: impl FnOnce(&K) -> Option<Raster>,
    ) -> Result<Slot, AtlasError> {
        match self.get(&key) {
            Some(slot) => Ok(slot),
            None => match produce(&key) {
                None => Ok(self.insert_blank(key, [0, 0])),
                Some(raster) => self.insert(
                    key,
                    raster.width,
                    raster.height,
                    raster.bearing,
                    raster.data,
                ),
            },
        }
    }

    pub fn get(&mut self, key: &K) -> Option<Slot> {
        let frame = self.frame;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = frame;
        let slot = entry.slot;
        if let Slot::Resident(placement) = slot {
            if let Some(Some(page)) = self.pages.get_mut(placement.page.index as usize) {
                page.last_used = frame;
            }
        }
        self.stats.hits += 1;
        Some(slot)
    }

    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    pub fn insert_blank(&mut self, key: K, bearing: [i32; 2]) -> Slot {
        let slot = Slot::Blank { bearing };
        self.discard(&key);
        self.entries.insert(
            key,
            Entry {
                slot,
                last_used: self.frame,
            },
        );
        slot
    }

    pub fn insert(
        &mut self,
        key: K,
        width: u32,
        height: u32,
        bearing: [i32; 2],
        data: Vec<u8>,
    ) -> Result<Slot, AtlasError> {
        if width == 0 || height == 0 {
            return Ok(self.insert_blank(key, bearing));
        }
        let expected =
            width as usize * height as usize * self.config.format.bytes_per_texel() as usize;
        if data.len() != expected {
            return Err(AtlasError::DataSize {
                expected,
                actual: data.len(),
            });
        }
        if width > self.config.max_edge || height > self.config.max_edge {
            return Err(AtlasError::TooLarge {
                width,
                height,
                max_edge: self.config.max_edge,
            });
        }
        self.discard(&key);
        let (page, x, y) = self.allocate(width, height);
        let (page_width, page_height) = self.page_extent(page);
        let placement = Placement {
            page,
            x,
            y,
            width,
            height,
            page_width,
            page_height,
            bearing,
        };
        let slot = Slot::Resident(placement);
        if let Some(Some(entry_page)) = self.pages.get_mut(page.index as usize) {
            entry_page.keys.push(key.clone());
            entry_page.live += 1;
            entry_page.last_used = self.frame;
        }
        self.pending.uploads.push(TexelUpload {
            page,
            x,
            y,
            width,
            height,
            data,
        });
        self.entries.insert(
            key,
            Entry {
                slot,
                last_used: self.frame,
            },
        );
        self.stats.inserts += 1;
        Ok(slot)
    }

    pub fn remove(&mut self, key: &K) -> bool {
        self.discard(key)
    }

    pub fn remove_where(&mut self, mut predicate: impl FnMut(&K) -> bool) -> usize {
        let doomed: Vec<K> = self
            .entries
            .keys()
            .filter(|key| predicate(key))
            .cloned()
            .collect();
        doomed.iter().filter(|key| self.discard(key)).count()
    }

    pub fn trim_idle(&mut self, idle_frames: u64) -> usize {
        let threshold = self.frame.saturating_sub(idle_frames);
        let doomed: Vec<u32> = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(index, page)| {
                page.as_ref()
                    .filter(|page| page.live == 0 || page.last_used < threshold)
                    .filter(|page| page.last_used < self.frame)
                    .map(|_| index as u32)
            })
            .collect();
        doomed.iter().for_each(|index| self.release_page(*index));
        doomed.len()
    }

    pub fn enforce_budget(&mut self) -> usize {
        let mut released = 0;
        while self.stats.resident_bytes > self.config.budget_bytes {
            match self.eviction_candidate() {
                Some(index) => {
                    self.release_page(index);
                    released += 1;
                }
                None => break,
            }
        }
        released
    }

    pub fn set_budget(&mut self, bytes: u64) {
        self.config.budget_bytes = bytes;
    }

    pub fn clear(&mut self) {
        self.pages.clear();
        self.free_indices.clear();
        self.entries.clear();
        self.pending = AtlasUpdates::default();
        self.evicted.clear();
        self.stats.resident_bytes = 0;
    }

    fn live_pages(&self) -> usize {
        self.pages.iter().flatten().count()
    }

    fn shared_pages(&self) -> usize {
        self.pages
            .iter()
            .flatten()
            .filter(|page| page.kind == PageKind::Shared)
            .count()
    }

    fn page_extent(&self, id: PageId) -> (u32, u32) {
        self.pages
            .get(id.index as usize)
            .and_then(|page| page.as_ref())
            .map(|page| (page.width, page.height))
            .unwrap_or((0, 0))
    }

    fn discard(&mut self, key: &K) -> bool {
        let Some(entry) = self.entries.remove(key) else {
            return false;
        };
        if let Slot::Resident(placement) = entry.slot {
            let index = placement.page.index;
            let release = match self.pages.get_mut(index as usize) {
                Some(Some(page)) => {
                    page.live = page.live.saturating_sub(1);
                    match (page.kind, page.live) {
                        (PageKind::Dedicated, 0) => true,
                        (PageKind::Shared, 0) => {
                            page.packer.reset();
                            page.keys.clear();
                            false
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            if release {
                self.release_page(index);
            }
        }
        true
    }

    fn allocate(&mut self, width: u32, height: u32) -> (PageId, u32, u32) {
        let limit = self.shared_edge_limit().min(self.config.base_edge * 4);
        let dedicated = width.saturating_mul(2) > limit || height.saturating_mul(2) > limit;
        match dedicated {
            true => self.allocate_dedicated(width, height),
            false => self.allocate_shared(width, height),
        }
    }

    fn id(&self, index: u32) -> PageId {
        PageId {
            format: self.config.format,
            index,
        }
    }

    fn allocate_shared(&mut self, width: u32, height: u32) -> (PageId, u32, u32) {
        if let Some(found) = self.fit_existing(width, height) {
            return found;
        }
        let page_edge = self
            .next_shared_edge()
            .max(width)
            .max(height)
            .next_power_of_two()
            .min(self.config.max_edge);
        let page_bytes =
            page_edge as u64 * page_edge as u64 * self.config.format.bytes_per_texel() as u64;
        loop {
            let below_cap = self.shared_pages() < self.config.max_shared_pages;
            let within_budget = self.stats.resident_bytes + page_bytes <= self.config.budget_bytes;
            if below_cap && within_budget {
                return self.create_shared_and_place(page_edge, width, height);
            }
            match self.eviction_candidate() {
                Some(index) => {
                    self.release_or_reset(index);
                    if let Some(found) = self.fit_existing(width, height) {
                        return found;
                    }
                }
                None => break,
            }
        }
        self.stats.over_budget_pages += 1;
        self.create_shared_and_place(page_edge, width, height)
    }

    fn next_shared_edge(&self) -> u32 {
        let growth = (self.shared_pages() as u32).min(2);
        (self.config.base_edge << growth).min(self.shared_edge_limit())
    }

    fn shared_edge_limit(&self) -> u32 {
        self.config.max_edge.min(match self.config.format {
            PageFormat::Mask8 => MASK_SHARED_EDGE_CAP,
            PageFormat::Color8 => COLOR_SHARED_EDGE_CAP,
        })
    }

    fn fit_existing(&mut self, width: u32, height: u32) -> Option<(PageId, u32, u32)> {
        let format = self.config.format;
        self.pages
            .iter_mut()
            .enumerate()
            .rev()
            .find_map(|(index, page)| {
                let page = page.as_mut().filter(|page| page.kind == PageKind::Shared)?;
                let (x, y) = page.packer.allocate(width, height)?;
                Some((
                    PageId {
                        format,
                        index: index as u32,
                    },
                    x,
                    y,
                ))
            })
    }

    fn create_shared_and_place(
        &mut self,
        edge: u32,
        width: u32,
        height: u32,
    ) -> (PageId, u32, u32) {
        let id = self.create_page(PageKind::Shared, edge, edge);
        let position = self.pages[id.index as usize]
            .as_mut()
            .and_then(|page| page.packer.allocate(width, height))
            .unwrap_or((0, 0));
        (id, position.0, position.1)
    }

    fn allocate_dedicated(&mut self, width: u32, height: u32) -> (PageId, u32, u32) {
        let bytes = width as u64 * height as u64 * self.config.format.bytes_per_texel() as u64;
        while self.stats.resident_bytes + bytes > self.config.budget_bytes {
            match self.eviction_candidate() {
                Some(index) => self.release_page(index),
                None => {
                    self.stats.over_budget_pages += 1;
                    break;
                }
            }
        }
        let id = self.create_page(PageKind::Dedicated, width, height);
        (id, 0, 0)
    }

    fn eviction_candidate(&self) -> Option<u32> {
        self.pages
            .iter()
            .enumerate()
            .filter_map(|(index, page)| page.as_ref().map(|page| (index as u32, page)))
            .filter(|(_, page)| page.last_used < self.frame)
            .min_by_key(|(_, page)| page.last_used)
            .map(|(index, _)| index)
    }

    fn release_or_reset(&mut self, index: u32) {
        let shared = matches!(
            self.pages[index as usize].as_ref().map(|page| page.kind),
            Some(PageKind::Shared)
        );
        match shared {
            true => self.reset_shared(index),
            false => self.release_page(index),
        }
    }

    fn reset_shared(&mut self, index: u32) {
        let keys = match self.pages[index as usize].as_mut() {
            Some(page) => {
                page.packer.reset();
                page.live = 0;
                page.last_used = self.frame;
                std::mem::take(&mut page.keys)
            }
            None => return,
        };
        let id = self.id(index);
        let mut evicted = 0;
        for key in keys {
            let on_page = matches!(
                self.entries.get(&key).map(|entry| entry.slot),
                Some(Slot::Resident(placement)) if placement.page == id
            );
            if on_page {
                self.entries.remove(&key);
                self.evicted.push(key);
                evicted += 1;
            }
        }
        self.stats.entries_evicted += evicted;
        self.stats.page_evictions += 1;
    }

    fn create_page(&mut self, kind: PageKind, width: u32, height: u32) -> PageId {
        let index = self.free_indices.pop().unwrap_or(self.pages.len() as u32);
        let page = Page {
            kind,
            width,
            height,
            packer: ShelfPacker::new(width, height),
            keys: Vec::new(),
            live: 0,
            last_used: self.frame,
        };
        self.stats.resident_bytes += page.bytes(self.config.format);
        match index as usize == self.pages.len() {
            true => self.pages.push(Some(page)),
            false => self.pages[index as usize] = Some(page),
        }
        let id = self.id(index);
        self.pending.created.push(PageCreate { id, width, height });
        id
    }

    fn release_page(&mut self, index: u32) {
        let Some(page) = self
            .pages
            .get_mut(index as usize)
            .and_then(|page| page.take())
        else {
            return;
        };
        let id = self.id(index);
        self.stats.resident_bytes -= page.bytes(self.config.format);
        self.free_indices.push(index);
        let mut evicted = 0;
        for key in page.keys {
            let on_page = matches!(
                self.entries.get(&key).map(|entry| entry.slot),
                Some(Slot::Resident(placement)) if placement.page == id
            );
            if on_page {
                self.entries.remove(&key);
                self.evicted.push(key);
                evicted += 1;
            }
        }
        self.stats.entries_evicted += evicted;
        self.stats.page_evictions += 1;
        let created_here = self
            .pending
            .created
            .iter()
            .position(|created| created.id == id);
        match created_here {
            Some(position) => {
                self.pending.created.remove(position);
                self.pending.uploads.retain(|upload| upload.page != id);
            }
            None => {
                self.pending.uploads.retain(|upload| upload.page != id);
                self.pending.released.push(id);
            }
        }
    }
}
