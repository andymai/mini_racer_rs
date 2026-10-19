use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mini_racer_sourcemap::{Placement, SourceMap};
use mini_racer_sync::lock;

use crate::error::SourceMapError;

/// Bundles with a source map, each parsed once.
pub const MAX_MAPS: usize = 8;

/// The source maps one process has registered, found by their bundle's
/// content: each kept once, parsed, and read by every context whose eval
/// holds its bundle.
#[derive(Default)]
pub struct SourceMaps {
    maps: Mutex<Vec<Arc<SourceMap>>>,
    // The shortest bundle's length, or 0 with none, so a source too short to
    // hold any is passed over without the lock.
    shortest: AtomicUsize,
}

impl SourceMaps {
    /// Parses `map` for `bundle` and keeps it, in place of the map an earlier
    /// call gave the same bundle; past `MAX_MAPS` bundles, a new one is
    /// refused.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the shortest length is stored under the lock, so it never trails the list"
    )]
    pub fn register(&self, bundle: &str, map: &[u8]) -> Result<(), SourceMapError> {
        let parsed = Arc::new(SourceMap::new(bundle, map)?);
        let mut maps = lock(&self.maps);
        if let Some(kept) = maps.iter_mut().find(|kept| kept.bundle() == bundle) {
            *kept = parsed;
        } else if maps.len() < MAX_MAPS {
            maps.push(parsed);
        } else {
            return Err(SourceMapError::Full(MAX_MAPS));
        }
        let shortest = maps
            .iter()
            .map(|kept| kept.bundle().len())
            .min()
            .unwrap_or(0);
        self.shortest.store(shortest, Ordering::Release);
        Ok(())
    }

    /// The first registered map whose bundle `source` holds, and where.
    pub(crate) fn find(&self, source: &[u8]) -> Option<(Arc<SourceMap>, Placement)> {
        let shortest = self.shortest.load(Ordering::Acquire);
        if shortest == 0 || source.len() < shortest {
            return None;
        }
        let maps = lock(&self.maps).clone();
        maps.into_iter()
            .find_map(|map| map.find(source).map(|placement| (map, placement)))
    }

    // A forked child's own list, from its parent's: try-locked, as a thread
    // gone with the fork may have held it.
    pub(crate) fn inherited(&self) -> Self {
        let maps = self
            .maps
            .try_lock()
            .map(|maps| maps.clone())
            .unwrap_or_default();
        let shortest = maps
            .iter()
            .map(|kept| kept.bundle().len())
            .min()
            .unwrap_or(0);
        Self {
            maps: Mutex::new(maps),
            shortest: AtomicUsize::new(shortest),
        }
    }
}
