use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use mini_racer_sync::lock;

use crate::control::JsText;

// The byte cap bounds the memory; the count keeps each large eval's scan of
// the list short. Caches of scripts of 128 KiB to 1 MiB measured 99 KB
// (React's bundle) to 530 KB (Autoprefixer's), so 64 of them take about 34 MB
// at most, where 8 entries let a few such scripts push a bundle's cache out.
const MAX_ENTRIES: usize = 64;
const MAX_BYTES: usize = 256 << 20;
// 32 KiB of digests, so a bundle loaded again after thousands of large scripts
// evaluated once is still seen a second time: a list of 64 forgot one after 70
// such scripts, so only its fourth load read a cache. Each large eval scans
// it, in about 2 microseconds when full.
const MAX_SIGHTINGS: usize = 4096;
const MAX_REFUSED: usize = 8;
// Units read from each end of a source, and a smaller window from each of a
// few points between, to tell one sighting from another without reading it all.
const SAMPLE: usize = 4096;
const SAMPLE_POINTS: usize = 8;

/// V8 code caches for the scripts of 128 KiB or more one process's contexts
/// have run, so the next context to evaluate one reads its compiled code back
/// instead of compiling it again.
///
/// A script is cached the second time the process sees it. The first sighting
/// costs a sampled digest and no cache, as a large script evaluated once
/// (per-request data, say) gains nothing from one; it costs the full digest
/// too only in a context that reads the script from its shared source.
#[derive(Default)]
pub struct CodeCache {
    // Most recently used last.
    entries: Mutex<Vec<Arc<Entry>>>,
    sightings: Mutex<Vec<u64>>,
    // Being built, so contexts loading a script at once build it once.
    building: Mutex<Vec<Key>>,
    // Built but over the size limit, so not built again.
    refused: Mutex<Vec<Key>>,
    // V8 checks a cache against the source's length alone, and runs whatever
    // code it holds. Digests are keyed with this process's random key, so a
    // script cannot be written to match another's.
    hasher: RandomState,
    hits: AtomicU64,
    stored: AtomicU64,
}

/// What a cached script is found by: its file name, the startup snapshot its
/// context began from (V8 refuses a cache made under another), and its
/// source's length and keyed digest.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Key {
    filename: JsText,
    snapshot: Option<u64>,
    units: usize,
    digest: u64,
    sighting: u64,
}

pub(crate) struct Entry {
    key: Key,
    data: Box<[u8]>,
}

impl Entry {
    pub(crate) fn data(&self) -> &[u8] {
        &self.data
    }
}

/// A key being built; dropped unbuilt, it lets another context build it.
pub(crate) struct Building {
    cache: &'static CodeCache,
    key: Key,
}

impl Drop for Building {
    fn drop(&mut self) {
        lock(&self.cache.building).retain(|key| *key != self.key);
    }
}

/// How the runner compiles a large script.
pub(crate) enum Plan {
    /// From source: the cache is off or a first sighting, or with its key, a
    /// first sighting whose source is shared, a key being built elsewhere or
    /// a refused cache.
    Compile(Option<Key>),
    /// From source, then a cache made once the dispatch is over.
    Make(Building),
    /// From the entry, falling back to source if V8 refuses it.
    Consume(Arc<Entry>, Key),
}

impl Plan {
    /// The script's key, unless the plan needed none.
    pub(crate) const fn key(&self) -> Option<&Key> {
        match self {
            Self::Compile(key) => key.as_ref(),
            Self::Make(building) => Some(&building.key),
            Self::Consume(_, key) => Some(key),
        }
    }
}

impl CodeCache {
    /// `shared` asks for a first sighting's key as well, which only a
    /// context reading the script from its shared source needs.
    pub(crate) fn plan(
        &'static self,
        filename: &JsText,
        source: &JsText,
        snapshot: Option<u64>,
        shared: bool,
    ) -> Plan {
        if !enabled() {
            return Plan::Compile(None);
        }
        let sighting = self.sighting(filename, source, snapshot);
        let seen = touch(&self.sightings, sighting, MAX_SIGHTINGS);
        // An entry counts as seen however many sightings have passed since.
        let cached = lock(&self.entries)
            .iter()
            .any(|entry| entry.key.sighting == sighting);
        let first = !seen && !cached;
        if first && !shared {
            return Plan::Compile(None);
        }
        let key = Key {
            filename: filename.clone(),
            snapshot,
            units: source.units(),
            digest: self.hasher.hash_one(source),
            sighting,
        };
        if first {
            return Plan::Compile(Some(key));
        }
        let mut entries = lock(&self.entries);
        if let Some(at) = entries.iter().position(|entry| entry.key == key) {
            let entry = entries.remove(at);
            entries.push(Arc::clone(&entry));
            drop(entries);
            return Plan::Consume(entry, key);
        }
        drop(entries);
        if lock(&self.refused).contains(&key) {
            return Plan::Compile(Some(key));
        }
        self.begin(key.clone())
            .map_or(Plan::Compile(Some(key)), Plan::Make)
    }

    /// Claims `key` for this context to build, unless another is building it.
    pub(crate) fn begin(&'static self, key: Key) -> Option<Building> {
        let mut building = lock(&self.building);
        if building.contains(&key) {
            return None;
        }
        building.push(key.clone());
        drop(building);
        Some(Building { cache: self, key })
    }

    /// A startup snapshot's identity for `Key`, from a sample of the blob:
    /// one snapshot taken for another costs a refused cache, never wrong code.
    pub(crate) fn snapshot_id(&self, blob: &[u8]) -> u64 {
        let mut hasher = self.hasher.build_hasher();
        sample(blob, &mut hasher);
        hasher.finish()
    }

    pub(crate) fn hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Keeps `data` for the key built, dropping the least recently used
    /// entries past the limits; a cache larger than all of them is refused.
    pub(crate) fn store(&self, building: Building, data: Box<[u8]>) {
        let key = building.key.clone();
        drop(building);
        if data.len() > MAX_BYTES {
            touch(&self.refused, key, MAX_REFUSED);
            return;
        }
        let mut entries = lock(&self.entries);
        if entries.iter().any(|entry| entry.key == key) {
            return;
        }
        entries.push(Arc::new(Entry { key, data }));
        while entries.len() > MAX_ENTRIES
            || entries.iter().map(|entry| entry.data.len()).sum::<usize>() > MAX_BYTES
        {
            entries.remove(0);
        }
        drop(entries);
        self.stored.fetch_add(1, Ordering::Relaxed);
    }

    /// Drops an entry V8 refused, so no later context tries it again.
    pub(crate) fn forget(&self, entry: &Arc<Entry>) {
        lock(&self.entries).retain(|kept| !Arc::ptr_eq(kept, entry));
    }

    /// A forked child's cache: what its parent kept, had seen and refused,
    /// unless a thread of the parent held it at the fork, under the same key.
    pub(crate) fn inherited(&self) -> Self {
        Self {
            entries: Mutex::new(copied(&self.entries)),
            sightings: Mutex::new(copied(&self.sightings)),
            building: Mutex::default(),
            refused: Mutex::new(copied(&self.refused)),
            hasher: self.hasher.clone(),
            hits: AtomicU64::new(0),
            stored: AtomicU64::new(0),
        }
    }

    /// Evaluations this process compiled from a cache V8 accepted.
    #[must_use]
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Caches this process made and kept.
    #[must_use]
    pub fn stored(&self) -> u64 {
        self.stored.load(Ordering::Relaxed)
    }

    fn sighting(&self, filename: &JsText, source: &JsText, snapshot: Option<u64>) -> u64 {
        let mut hasher = self.hasher.build_hasher();
        filename.hash(&mut hasher);
        snapshot.hash(&mut hasher);
        let encoding: u8 = match source {
            JsText::Utf8(bytes) => {
                sample(bytes, &mut hasher);
                0
            }
            JsText::Latin1(bytes) => {
                sample(bytes, &mut hasher);
                1
            }
            JsText::Utf16(units) => {
                sample(units, &mut hasher);
                2
            }
        };
        encoding.hash(&mut hasher);
        hasher.finish()
    }
}

// Moves `item` to the back of `list`, adding it if absent and dropping the
// front past `max`. Returns whether it was there.
fn touch<T: PartialEq>(list: &Mutex<Vec<T>>, item: T, max: usize) -> bool {
    let mut list = lock(list);
    let found = list.iter().position(|kept| *kept == item);
    if let Some(at) = found {
        list.remove(at);
    } else if list.len() == max {
        list.remove(0);
    }
    list.push(item);
    drop(list);
    found.is_some()
}

fn copied<T: Clone>(list: &Mutex<Vec<T>>) -> Vec<T> {
    list.try_lock().map(|list| list.clone()).unwrap_or_default()
}

fn sample<T: Hash>(text: &[T], hasher: &mut impl Hasher) {
    let len = text.len();
    len.hash(hasher);
    let span = SAMPLE.min(len);
    text.get(..span).hash(hasher);
    text.get(len - span..).hash(hasher);
    for point in 1..SAMPLE_POINTS {
        let at = len / SAMPLE_POINTS * point;
        text.get(at..(at + SAMPLE / SAMPLE_POINTS).min(len))
            .hash(hasher);
    }
}

/// Whether the cache is on: `MINI_RACER_RS_CODE_CACHE=off` turns it off.
/// First read on a Ruby thread, by `ContextHandle::spawn`, where nothing in
/// Ruby can be changing the environment at the same time.
pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("MINI_RACER_RS_CODE_CACHE").is_none_or(|value| value != "off")
    })
}

#[cfg(test)]
#[allow(clippy::panic, reason = "a test fails by panicking")]
mod tests {
    use super::{CodeCache, MAX_SIGHTINGS, Plan};
    use crate::control::JsText;

    fn cache() -> &'static CodeCache {
        Box::leak(Box::default())
    }

    fn source(seed: u8) -> JsText {
        JsText::Latin1(vec![seed; 2 << 20])
    }

    fn name() -> JsText {
        JsText::from("bundle.js")
    }

    // Sees `text` twice and stores a cache for it.
    fn cached(cache: &'static CodeCache, text: &JsText, data: Box<[u8]>) {
        let _ = cache.plan(&name(), text, None, true);
        let Plan::Make(building) = cache.plan(&name(), text, None, true) else {
            panic!("a second sighting makes a cache");
        };
        cache.store(building, data);
    }

    #[test]
    fn caches_a_script_the_second_time_it_is_seen() {
        let cache = cache();
        assert!(matches!(
            cache.plan(&name(), &source(1), None, true),
            Plan::Compile(Some(_))
        ));
        cached(cache, &source(1), Box::from(&b"compiled"[..]));
        assert!(matches!(
            cache.plan(&name(), &source(1), None, true),
            Plan::Consume(..)
        ));
        assert!(matches!(
            cache.plan(&name(), &source(1), Some(7), true),
            Plan::Compile(Some(_))
        ));
        assert!(matches!(
            cache.plan(&JsText::from("other.js"), &source(1), None, true),
            Plan::Compile(Some(_))
        ));
    }

    // The key a first sighting has for its shared source is the one the
    // cache is then made and found under.
    #[test]
    fn a_first_sighting_has_its_key_only_for_a_shared_source() {
        let cache = cache();
        assert!(matches!(
            cache.plan(&name(), &source(8), None, false),
            Plan::Compile(None)
        ));
        let first = cache.plan(&name(), &source(9), None, true);
        assert!(matches!(first, Plan::Compile(Some(_))));
        let second = cache.plan(&name(), &source(9), None, false);
        assert!(matches!(second, Plan::Make(..)));
        assert!(first.key() == second.key());
        let Plan::Make(building) = second else {
            panic!("a second sighting makes a cache");
        };
        cache.store(building, Box::from(&b"compiled"[..]));
        let Plan::Consume(_, key) = cache.plan(&name(), &source(9), None, true) else {
            panic!("a stored cache is found");
        };
        assert!(first.key() == Some(&key));
    }

    // Sources that match on every sampled unit and differ elsewhere share a
    // sighting, and the full digest still tells them apart.
    #[test]
    fn a_source_alike_in_its_samples_is_not_taken_for_another() {
        let cache = cache();
        let mut alike = vec![5_u8; 2 << 20];
        if let Some(unit) = alike.get_mut((2 << 20) / 16 + 4096) {
            *unit = 6;
        }
        let (first, second) = (JsText::Latin1(vec![5; 2 << 20]), JsText::Latin1(alike));
        assert_eq!(
            cache.sighting(&name(), &first, None),
            cache.sighting(&name(), &second, None)
        );
        cached(cache, &first, Box::from(&b"compiled"[..]));
        assert!(matches!(
            cache.plan(&name(), &second, None, true),
            Plan::Make(..)
        ));
    }

    #[test]
    fn an_entry_outlives_any_number_of_other_sightings() {
        let cache = cache();
        cached(cache, &source(2), Box::from(&b"compiled"[..]));
        // More sightings than the list keeps.
        for seed in 10_u8..80 {
            let _ = cache.plan(&name(), &source(seed), None, false);
        }
        assert!(matches!(
            cache.plan(&name(), &source(2), None, true),
            Plan::Consume(..)
        ));
    }

    // The list holds as many sightings as it keeps and no more: a script is
    // seen again after one fewer others, and forgotten after that many more.
    #[test]
    fn a_sighting_is_forgotten_once_the_list_has_had_its_fill_of_others() {
        let cache = cache();
        let script = |seed: usize| JsText::Latin1(seed.to_le_bytes().to_vec());
        for seed in 0..MAX_SIGHTINGS {
            let _ = cache.plan(&name(), &script(seed), None, false);
        }
        assert!(matches!(
            cache.plan(&name(), &script(0), None, false),
            Plan::Make(..)
        ));
        for seed in MAX_SIGHTINGS..2 * MAX_SIGHTINGS {
            let _ = cache.plan(&name(), &script(seed), None, false);
        }
        assert_eq!(mini_racer_sync::lock(&cache.sightings).len(), MAX_SIGHTINGS);
        assert!(matches!(
            cache.plan(&name(), &script(0), None, false),
            Plan::Compile(None)
        ));
    }

    #[test]
    fn a_script_being_built_is_built_once() {
        let cache = cache();
        let _ = cache.plan(&name(), &source(3), None, true);
        let first = cache.plan(&name(), &source(3), None, true);
        assert!(matches!(first, Plan::Make(..)));
        let elsewhere = cache.plan(&name(), &source(3), None, true);
        assert!(matches!(elsewhere, Plan::Compile(Some(_))));
        assert!(elsewhere.key() == first.key());
        drop(first);
        assert!(matches!(
            cache.plan(&name(), &source(3), None, true),
            Plan::Make(..)
        ));
    }

    #[test]
    fn a_child_starts_from_its_parents_entries() {
        let cache = cache();
        cached(cache, &source(4), Box::from(&b"compiled"[..]));
        let child: &'static CodeCache = Box::leak(Box::new(cache.inherited()));
        assert!(matches!(
            child.plan(&name(), &source(4), None, true),
            Plan::Consume(..)
        ));
    }

    #[test]
    fn a_child_forked_while_the_cache_was_held_starts_without_its_entries() {
        let cache = cache();
        cached(cache, &source(5), Box::from(&b"compiled"[..]));
        let held = cache.entries.lock();
        let child: &'static CodeCache = Box::leak(Box::new(cache.inherited()));
        drop(held);
        assert!(matches!(
            child.plan(&name(), &source(5), None, true),
            Plan::Make(..)
        ));
    }

    #[test]
    fn a_cache_larger_than_the_limit_is_refused_and_not_rebuilt() {
        let cache = cache();
        cached(cache, &source(6), Box::from(&b"compiled"[..]));
        cached(
            cache,
            &source(7),
            vec![0; super::MAX_BYTES + 1].into_boxed_slice(),
        );
        assert_eq!(cache.stored(), 1);
        assert!(matches!(
            cache.plan(&name(), &source(7), None, true),
            Plan::Compile(Some(_))
        ));
        assert!(matches!(
            cache.plan(&name(), &source(6), None, true),
            Plan::Consume(..)
        ));
    }

    #[test]
    fn caches_past_the_size_limit_drop_the_least_recently_used() {
        let cache = cache();
        let half = || vec![0; super::MAX_BYTES / 2 + 1].into_boxed_slice();
        cached(cache, &source(10), half());
        cached(cache, &source(11), half());
        assert_eq!(cache.stored(), 2);
        assert!(matches!(
            cache.plan(&name(), &source(11), None, true),
            Plan::Consume(..)
        ));
        assert!(matches!(
            cache.plan(&name(), &source(10), None, true),
            Plan::Make(..)
        ));
    }
}
