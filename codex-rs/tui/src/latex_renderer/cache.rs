//! Generation-local LaTeX cache and bounded negative memory.

use std::collections::HashMap;
use std::sync::Arc;

use super::RenderedImage;
use super::RendererLimits;
use crate::latex_image::LatexPng;
use crate::latex_image::LatexRenderStyle;

pub(super) const NEGATIVE_KEY_CAPACITY: usize = 64;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct RenderKey {
    pub(super) generation: u64,
    pub(super) formula: crate::latex_image::ValidatedLatexFormula,
    pub(super) foreground: [u8; 3],
    pub(super) style: LatexRenderStyle,
    pub(super) cell_height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RenderAdmission {
    Historical,
    Live(u64),
}

impl RenderAdmission {
    fn effective(self, latest_live_admission_id: u64) -> Self {
        match self {
            Self::Live(admission_id) if admission_id == latest_live_admission_id => self,
            Self::Historical | Self::Live(_) => Self::Historical,
        }
    }
}

enum RenderState {
    Pending,
    Ready(Arc<RenderedImage>),
}

struct CacheEntry {
    admission: RenderAdmission,
    // A bounded scan over ranks avoids storing every potentially large key twice.
    priority: i64,
    state: RenderState,
}

pub(super) enum CacheLookup {
    Ready(Arc<RenderedImage>),
    Pending,
    Missing,
}

#[derive(Default)]
struct BoundedKeyMemory {
    keys: Vec<RenderKey>,
    exhausted: bool,
}

impl BoundedKeyMemory {
    fn blocks(&self, key: &RenderKey) -> bool {
        self.exhausted || self.contains(key)
    }

    fn contains(&self, key: &RenderKey) -> bool {
        self.keys.contains(key)
    }

    fn record(&mut self, key: &RenderKey) {
        if self.contains(key) {
            return;
        }
        if self.keys.len() == NEGATIVE_KEY_CAPACITY {
            self.exhausted = true;
        } else {
            self.keys.push(key.clone());
        }
    }

    fn clear(&mut self) {
        self.keys.clear();
        self.exhausted = false;
    }
}

#[derive(Default)]
pub(super) struct RenderCache {
    latest_live_admission_id: u64,
    entry_capacity: usize,
    byte_capacity: usize,
    terminal_byte_capacity: usize,
    selected_cell_height: Option<u32>,
    entries: HashMap<RenderKey, CacheEntry>,
    bytes: usize,
    terminal_bytes: usize,
    failed: BoundedKeyMemory,
    historical_capacity: BoundedKeyMemory,
    // Failures and capacity rejections share one strict per-live-message work budget.
    live_rejections: BoundedKeyMemory,
}

pub(super) enum CacheCompletion {
    Missing,
    Updated,
    ImageIdExhausted,
}

impl RenderCache {
    pub(super) fn new(limits: RendererLimits) -> Self {
        Self {
            entry_capacity: limits.cache_entry_capacity,
            byte_capacity: limits.cache_byte_capacity,
            terminal_byte_capacity: limits.cache_terminal_byte_capacity,
            ..Self::default()
        }
    }

    pub(super) fn begin_live_admission(&mut self) -> RenderAdmission {
        self.latest_live_admission_id = self.latest_live_admission_id.wrapping_add(1);
        self.live_rejections.clear();
        RenderAdmission::Live(self.latest_live_admission_id)
    }

    pub(super) fn select_cell_height(&mut self, cell_height: u32) {
        if self.selected_cell_height != Some(cell_height) {
            self.clear();
            self.selected_cell_height = Some(cell_height);
        }
    }

    pub(super) fn lookup(&mut self, key: &RenderKey, admission: RenderAdmission) -> CacheLookup {
        let admission = admission.effective(self.latest_live_admission_id);
        let priority =
            matches!(admission, RenderAdmission::Live(_)).then(|| self.next_priority(admission));
        let Some(entry) = self.entries.get_mut(key) else {
            return CacheLookup::Missing;
        };
        let result = match &entry.state {
            RenderState::Pending => CacheLookup::Pending,
            RenderState::Ready(image) => CacheLookup::Ready(Arc::clone(image)),
        };
        if let Some(priority) = priority {
            entry.admission = admission;
            entry.priority = priority;
        }
        result
    }

    pub(super) fn can_admit(&mut self, key: &RenderKey, admission: RenderAdmission) -> bool {
        let admission = admission.effective(self.latest_live_admission_id);
        let blocked = match admission {
            RenderAdmission::Historical => {
                self.failed.blocks(key) || self.historical_capacity.blocks(key)
            }
            RenderAdmission::Live(_) => {
                self.failed.contains(key) || self.live_rejections.blocks(key)
            }
        };
        if blocked {
            return false;
        }
        if self.entries.len() < self.entry_capacity {
            return true;
        }
        match admission {
            RenderAdmission::Historical => false,
            RenderAdmission::Live(_) => {
                if self.least_priority_evictable_key().is_some() {
                    true
                } else {
                    self.live_rejections.exhausted = true;
                    false
                }
            }
        }
    }

    pub(super) fn insert_pending(&mut self, key: RenderKey, admission: RenderAdmission) {
        if self.entries.len() == self.entry_capacity {
            let Some(eviction_key) = self.least_priority_evictable_key() else {
                return;
            };
            self.remove_entry(&eviction_key);
        }
        let admission = admission.effective(self.latest_live_admission_id);
        let priority = self.next_priority(admission);
        let previous = self.entries.insert(
            key,
            CacheEntry {
                admission,
                priority,
                state: RenderState::Pending,
            },
        );
        debug_assert!(previous.is_none());
    }

    pub(super) fn contains_pending(&self, key: &RenderKey) -> bool {
        matches!(
            self.entries.get(key),
            Some(CacheEntry {
                state: RenderState::Pending,
                ..
            })
        )
    }

    pub(super) fn complete(
        &mut self,
        key: &RenderKey,
        png: LatexPng,
        terminal_bytes: usize,
    ) -> CacheCompletion {
        let Some(CacheEntry {
            admission,
            state: RenderState::Pending,
            ..
        }) = self.entries.get(key)
        else {
            return CacheCompletion::Missing;
        };
        let admission = admission.effective(self.latest_live_admission_id);
        let png_bytes = png.bytes.len();

        while matches!(admission, RenderAdmission::Live(_))
            && !self.fits(png_bytes, terminal_bytes)
            && let Some(eviction_key) = self.least_priority_evictable_key()
        {
            self.remove_entry(&eviction_key);
        }

        if self.fits(png_bytes, terminal_bytes) {
            let Some(image_id) = super::kitty_placeholder::next_image_id() else {
                self.remove_entry(key);
                return CacheCompletion::ImageIdExhausted;
            };
            let Some(entry) = self.entries.get_mut(key) else {
                return CacheCompletion::Missing;
            };
            self.bytes += png_bytes;
            self.terminal_bytes += terminal_bytes;
            entry.admission = admission;
            entry.state = RenderState::Ready(Arc::new(RenderedImage::new_with_terminal_bytes(
                png,
                terminal_bytes,
                image_id,
            )));
        } else {
            self.remove_entry(key);
            match admission {
                RenderAdmission::Historical => self.historical_capacity.record(key),
                RenderAdmission::Live(_) => self.live_rejections.record(key),
            }
        }
        CacheCompletion::Updated
    }

    pub(super) fn fail(&mut self, key: &RenderKey) -> bool {
        let Some(CacheEntry {
            admission,
            state: RenderState::Pending,
            ..
        }) = self.entries.get(key)
        else {
            return false;
        };
        let admission = admission.effective(self.latest_live_admission_id);
        self.remove_entry(key);
        self.failed.record(key);
        if matches!(admission, RenderAdmission::Live(_)) {
            self.live_rejections.record(key);
        }
        true
    }

    pub(super) fn discard_pending(&mut self, key: &RenderKey) {
        if self.contains_pending(key) {
            self.remove_entry(key);
        }
    }

    pub(super) fn clear(&mut self) {
        self.selected_cell_height = None;
        self.entries.clear();
        self.bytes = 0;
        self.terminal_bytes = 0;
        self.failed.clear();
        self.historical_capacity.clear();
        self.live_rejections.clear();
    }

    fn fits(&self, png_bytes: usize, terminal_bytes: usize) -> bool {
        self.bytes.saturating_add(png_bytes) <= self.byte_capacity
            && self.terminal_bytes.saturating_add(terminal_bytes) <= self.terminal_byte_capacity
    }

    fn least_priority_evictable_key(&self) -> Option<RenderKey> {
        self.entries
            .iter()
            .filter(|(_, entry)| {
                matches!(
                    entry.admission.effective(self.latest_live_admission_id),
                    RenderAdmission::Historical
                )
            })
            .min_by_key(|(_, entry)| entry.priority)
            .map(|(key, _)| key.clone())
    }

    fn next_priority(&self, admission: RenderAdmission) -> i64 {
        let priorities = self.entries.values().map(|entry| entry.priority);
        match admission {
            RenderAdmission::Historical => priorities.min().unwrap_or(0).saturating_sub(1),
            RenderAdmission::Live(_) => priorities.max().unwrap_or(0).saturating_add(1),
        }
    }

    fn remove_entry(&mut self, key: &RenderKey) -> Option<CacheEntry> {
        let entry = self.entries.remove(key)?;
        if let RenderState::Ready(image) = &entry.state {
            debug_assert!(self.bytes >= image.png.bytes.len());
            debug_assert!(self.terminal_bytes >= image.terminal_bytes);
            self.bytes = self.bytes.saturating_sub(image.png.bytes.len());
            self.terminal_bytes = self.terminal_bytes.saturating_sub(image.terminal_bytes);
        }
        Some(entry)
    }
}
