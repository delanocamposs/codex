//! Generation-local LaTeX cache and bounded negative memory.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;

use super::RenderedImage;
use super::RendererLimits;
use crate::latex_image::LatexPng;
use crate::latex_image::LatexRenderStyle;

pub(super) const NEGATIVE_KEY_CAPACITY: usize = 64;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct RenderKey {
    pub(super) generation: u64,
    pub(super) formula: String,
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
    pub(super) fn effective(self, latest_live_admission_id: u64) -> Self {
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
    selected_cell_height: Option<u32>,
    entries: HashMap<RenderKey, CacheEntry>,
    /// Highest-priority entries first.
    entry_priority: VecDeque<RenderKey>,
    bytes: usize,
    terminal_bytes: usize,
    failed: BoundedKeyMemory,
    historical_capacity: BoundedKeyMemory,
    // Failures and capacity rejections share one strict per-live-message work budget.
    live_rejections: BoundedKeyMemory,
}

impl RenderCache {
    pub(super) fn begin_live_admission(&mut self) {
        self.live_rejections.clear();
    }

    pub(super) fn select_cell_height(&mut self, cell_height: u32) {
        if self.selected_cell_height != Some(cell_height) {
            *self = Self {
                selected_cell_height: Some(cell_height),
                ..Self::default()
            };
        }
    }

    pub(super) fn lookup(
        &mut self,
        key: &RenderKey,
        admission: RenderAdmission,
        latest_live_admission_id: u64,
    ) -> CacheLookup {
        let Some(entry) = self.entries.get_mut(key) else {
            return CacheLookup::Missing;
        };
        let result = match &entry.state {
            RenderState::Pending => CacheLookup::Pending,
            RenderState::Ready(image) => CacheLookup::Ready(Arc::clone(image)),
        };
        if matches!(
            admission.effective(latest_live_admission_id),
            RenderAdmission::Live(_)
        ) {
            entry.admission = admission;
            let position = self
                .entry_priority
                .iter()
                .position(|candidate| candidate == key)
                .expect("LaTeX cache entry should have an eviction-order key");
            self.entry_priority.remove(position);
            self.entry_priority.push_front(key.clone());
        }
        result
    }

    pub(super) fn can_admit(
        &mut self,
        key: &RenderKey,
        admission: RenderAdmission,
        latest_live_admission_id: u64,
        entry_capacity: usize,
    ) -> bool {
        let admission = admission.effective(latest_live_admission_id);
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
        if self.entries.len() < entry_capacity {
            return true;
        }
        match admission {
            RenderAdmission::Historical => false,
            RenderAdmission::Live(_) => {
                if self
                    .least_priority_evictable_key(latest_live_admission_id)
                    .is_some()
                {
                    true
                } else {
                    self.live_rejections.exhausted = true;
                    false
                }
            }
        }
    }

    pub(super) fn insert_pending(
        &mut self,
        key: RenderKey,
        admission: RenderAdmission,
        latest_live_admission_id: u64,
        entry_capacity: usize,
    ) {
        if self.entries.len() == entry_capacity {
            let eviction_key = self
                .least_priority_evictable_key(latest_live_admission_id)
                .expect("admission should reserve an evictable LaTeX cache entry");
            self.remove_entry(&eviction_key);
        }
        let previous = self.entries.insert(
            key.clone(),
            CacheEntry {
                admission,
                state: RenderState::Pending,
            },
        );
        debug_assert!(previous.is_none());
        match admission.effective(latest_live_admission_id) {
            RenderAdmission::Historical => self.entry_priority.push_back(key),
            RenderAdmission::Live(_) => self.entry_priority.push_front(key),
        }
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
        latest_live_admission_id: u64,
        limits: RendererLimits,
    ) -> bool {
        let Some(CacheEntry {
            admission,
            state: RenderState::Pending,
        }) = self.entries.get(key)
        else {
            return false;
        };
        let admission = admission.effective(latest_live_admission_id);
        let png_bytes = png.bytes.len();

        while matches!(admission, RenderAdmission::Live(_))
            && !self.fits(png_bytes, terminal_bytes, limits)
            && let Some(eviction_key) = self.least_priority_evictable_key(latest_live_admission_id)
        {
            self.remove_entry(&eviction_key);
        }

        if self.fits(png_bytes, terminal_bytes, limits)
            && let Some(image) = RenderedImage::new_with_terminal_bytes(png, terminal_bytes)
        {
            self.bytes += png_bytes;
            self.terminal_bytes += terminal_bytes;
            let entry = self
                .entries
                .get_mut(key)
                .expect("pending LaTeX cache entry should still exist");
            entry.admission = admission;
            entry.state = RenderState::Ready(Arc::new(image));
        } else {
            self.remove_entry(key);
            match admission {
                RenderAdmission::Historical => self.historical_capacity.record(key),
                RenderAdmission::Live(_) => self.live_rejections.record(key),
            }
        }
        true
    }

    pub(super) fn fail(&mut self, key: &RenderKey, latest_live_admission_id: u64) -> bool {
        let Some(CacheEntry {
            admission,
            state: RenderState::Pending,
        }) = self.entries.get(key)
        else {
            return false;
        };
        let admission = admission.effective(latest_live_admission_id);
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
        *self = Self::default();
    }

    fn fits(&self, png_bytes: usize, terminal_bytes: usize, limits: RendererLimits) -> bool {
        self.bytes.saturating_add(png_bytes) <= limits.cache_byte_capacity
            && self.terminal_bytes.saturating_add(terminal_bytes)
                <= limits.cache_terminal_byte_capacity
    }

    fn least_priority_evictable_key(&self, latest_live_admission_id: u64) -> Option<RenderKey> {
        self.entry_priority
            .iter()
            .rev()
            .find(|key| {
                self.entries.get(*key).is_some_and(|entry| {
                    matches!(
                        entry.admission.effective(latest_live_admission_id),
                        RenderAdmission::Historical
                    )
                })
            })
            .cloned()
    }

    fn remove_entry(&mut self, key: &RenderKey) -> Option<CacheEntry> {
        let entry = self.entries.remove(key)?;
        let position = self
            .entry_priority
            .iter()
            .position(|candidate| candidate == key)
            .expect("LaTeX cache entry should have an eviction-order key");
        self.entry_priority.remove(position);
        if let RenderState::Ready(image) = &entry.state {
            self.bytes = self
                .bytes
                .checked_sub(image.png.bytes.len())
                .expect("LaTeX cache byte accounting should not underflow");
            self.terminal_bytes = self
                .terminal_bytes
                .checked_sub(image.terminal_bytes)
                .expect("LaTeX terminal byte accounting should not underflow");
        }
        Some(entry)
    }
}
