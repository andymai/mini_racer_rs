use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

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
// A first sighting this large is cached at once when its top-level run took
// at least as long as its compile, as a bundle that renders as it loads does:
// a 44 MB server rendering bundle compiled in 186 ms and ran for 690, and its
// cache of 8 MB took 35 ms. A data literal runs in a fraction of its compile,
// and its cache is larger than its source: 16 MiB of JSON compiled in 312 ms,
// ran for 55 and made a cache of 24 MB in 164 ms.
const FIRST_SIGHTING: usize = 16 << 20;

/// V8 code caches for the scripts of 128 KiB or more one process's contexts
/// have run, so the next context to evaluate one reads its compiled code back
/// instead of compiling it again.
///
/// A script is cached the second time the process sees it. The first sighting
/// costs a sampled digest and no cache, as a large script evaluated once
/// (per-request data, say) gains nothing from one; it costs the full digest
/// too only in a context that reads the script from its shared source. Such a
/// context caches a first sighting of 16 MiB or more whose run took at least
/// as long as its compile (`worth_first`), unless that would drop an entry a
/// context has read. The context that cached a script caches it again after
/// its next call (`replace`).
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
    rebuilt: AtomicU64,
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
    // A context compiled from it.
    read: AtomicBool,
}

impl Entry {
    pub(crate) fn data(&self) -> &[u8] {
        &self.data
    }
}

/// A kept entry, as the context that built it builds it again once a call
/// has compiled more of the script.
pub(crate) struct Stored {
    key: Key,
    first: bool,
}

/// A key being built; dropped unbuilt, it lets another context build it.
pub(crate) struct Building {
    cache: &'static CodeCache,
    key: Key,
    // At the script's first sighting, so it makes room only from entries no
    // context has read.
    first: bool,
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
    /// From source, a first sighting of 16 MiB or more whose source is
    /// shared, cached once it has run if `worth_first` says so (`CodeCache::first`).
    First(Key),
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
            Self::First(key) | Self::Consume(_, key) => Some(key),
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
        if first && source.byte_len() >= FIRST_SIGHTING {
            return Plan::First(key);
        }
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
        self.claim(key, false)
    }

    /// Claims a first sighting's `key` for this context to build once its
    /// script has run, unless it was cached, refused or claimed meanwhile.
    pub(crate) fn first(&'static self, key: Key) -> Option<Building> {
        if lock(&self.entries).iter().any(|entry| entry.key == key)
            || lock(&self.refused).contains(&key)
        {
            return None;
        }
        self.claim(key, true)
    }

    fn claim(&'static self, key: Key, first: bool) -> Option<Building> {
        let mut building = lock(&self.building);
        if building.contains(&key) {
            return None;
        }
        building.push(key.clone());
        drop(building);
        Some(Building {
            cache: self,
            key,
            first,
        })
    }

    /// A startup snapshot's identity for `Key`, from a sample of the blob:
    /// one snapshot taken for another costs a refused cache, never wrong code.
    pub(crate) fn snapshot_id(&self, blob: &[u8]) -> u64 {
        let mut hasher = self.hasher.build_hasher();
        sample(blob, &mut hasher);
        hasher.finish()
    }

    pub(crate) fn hit(&self, entry: &Entry) {
        entry.read.store(true, Ordering::Relaxed);
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Keeps `data` for the key built, dropping the least recently used
    /// entries past the limits; a cache larger than all of them is refused. A
    /// first sighting's cache that would drop an entry a context has read is
    /// not kept, and the script is cached at its next sighting instead.
    /// A kept entry comes back as what its builder hands `replace` once a
    /// call has run.
    pub(crate) fn store(&self, building: Building, data: Box<[u8]>) -> Option<Stored> {
        let stored = Stored {
            key: building.key.clone(),
            first: building.first,
        };
        drop(building);
        if data.len() > MAX_BYTES {
            touch(&self.refused, stored.key, MAX_REFUSED);
            return None;
        }
        let mut entries = lock(&self.entries);
        if entries.iter().any(|entry| entry.key == stored.key) {
            return None;
        }
        let dropped = room(&entries, data.len(), stored.first)?;
        entries.drain(..dropped);
        entries.push(Arc::new(Entry {
            key: stored.key.clone(),
            data,
            read: AtomicBool::new(false),
        }));
        drop(entries);
        self.stored.fetch_add(1, Ordering::Relaxed);
        Some(stored)
    }

    /// Puts `data`, built again by the context that stored the entry once a
    /// call ran more of its script, in place of the entry, as the most
    /// recently used. An entry gone since stays gone, and one the limits
    /// leave no room for keeps what it held.
    pub(crate) fn replace(&self, stored: &Stored, data: Box<[u8]>) {
        if data.len() > MAX_BYTES {
            return;
        }
        let mut entries = lock(&self.entries);
        let Some(at) = entries.iter().position(|entry| entry.key == stored.key) else {
            return;
        };
        let held = entries.remove(at);
        let read = held.read.load(Ordering::Relaxed);
        let Some(dropped) = room(&entries, data.len(), stored.first && !read) else {
            entries.insert(at, held);
            return;
        };
        entries.drain(..dropped);
        entries.push(Arc::new(Entry {
            key: stored.key.clone(),
            data,
            read: AtomicBool::new(read),
        }));
        drop(entries);
        drop(held);
        self.rebuilt.fetch_add(1, Ordering::Relaxed);
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
            rebuilt: AtomicU64::new(0),
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

    /// Kept caches this process built again after a call.
    #[must_use]
    pub fn rebuilt(&self) -> u64 {
        self.rebuilt.load(Ordering::Relaxed)
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

// How many of the least recently used entries go to make room for one of
// `bytes` more; `None` when a first sighting's would drop one a context has
// read.
fn room(entries: &[Arc<Entry>], bytes: usize, first: bool) -> Option<usize> {
    let mut count = entries.len() + 1;
    let mut total = entries.iter().map(|entry| entry.data.len()).sum::<usize>() + bytes;
    let mut dropped = 0;
    while count > MAX_ENTRIES || total > MAX_BYTES {
        let Some(oldest) = entries.get(dropped) else {
            break;
        };
        if first && oldest.read.load(Ordering::Relaxed) {
            return None;
        }
        count -= 1;
        total -= oldest.data.len();
        dropped += 1;
    }
    Some(dropped)
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

/// Whether a first sighting's script is worth a cache now, from how long it
/// took to compile and to run. `MINI_RACER_RS_CODE_CACHE_FIRST` set to
/// `always` or `never` (unstable, for tests) decides instead, read once per
/// process, first by `ContextHandle::spawn` as `enabled` is.
pub(crate) fn worth_first(compiled: Duration, ran: Duration) -> bool {
    first_override().unwrap_or(ran >= compiled)
}

pub(crate) fn first_override() -> Option<bool> {
    static DECIDED: OnceLock<Option<bool>> = OnceLock::new();
    *DECIDED.get_or_init(
        || match std::env::var_os("MINI_RACER_RS_CODE_CACHE_FIRST")?.to_str()? {
            "always" => Some(true),
            "never" => Some(false),
            _ => None,
        },
    )
}

#[cfg(test)]
#[allow(clippy::panic, reason = "a test fails by panicking")]
mod tests {
    use std::time::Duration;

    use super::{CodeCache, FIRST_SIGHTING, MAX_ENTRIES, MAX_SIGHTINGS, Plan, Stored};
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

    fn bundle(seed: u8, bytes: usize) -> JsText {
        JsText::Latin1(vec![seed; bytes])
    }

    // Small enough to fill the list quickly; `plan` reads no size but the
    // first sighting's.
    fn tiny(seed: usize) -> JsText {
        JsText::Latin1(seed.to_le_bytes().to_vec())
    }

    // Caches `text` at its first sighting.
    fn first(cache: &'static CodeCache, text: &JsText, data: Box<[u8]>) {
        let Plan::First(key) = cache.plan(&name(), text, None, true) else {
            panic!("a first sighting worth a cache at once");
        };
        let Some(building) = cache.first(key) else {
            panic!("an unclaimed first sighting");
        };
        cache.store(building, data);
    }

    fn consumed(cache: &'static CodeCache, text: &JsText) {
        let Plan::Consume(entry, _) = cache.plan(&name(), text, None, true) else {
            panic!("a stored cache is found");
        };
        cache.hit(&entry);
    }

    #[test]
    fn only_a_shared_first_sighting_of_16_mib_or_more_may_be_cached_at_once() {
        let cache = cache();
        assert!(matches!(
            cache.plan(&name(), &bundle(1, FIRST_SIGHTING), None, true),
            Plan::First(_)
        ));
        assert!(matches!(
            cache.plan(&name(), &bundle(2, FIRST_SIGHTING), None, false),
            Plan::Compile(None)
        ));
        assert!(matches!(
            cache.plan(&name(), &bundle(3, FIRST_SIGHTING - 1), None, true),
            Plan::Compile(Some(_))
        ));
        let units = vec![0x61; FIRST_SIGHTING / 2];
        assert!(matches!(
            cache.plan(&name(), &JsText::Utf16(units), None, true),
            Plan::First(_)
        ));
        // Seen again, it is cached as any other script.
        assert!(matches!(
            cache.plan(&name(), &bundle(1, FIRST_SIGHTING), None, true),
            Plan::Make(_)
        ));
    }

    #[test]
    fn a_first_sighting_is_cached_once_it_has_run_for_as_long_as_it_compiled() {
        let millis = Duration::from_millis;
        assert!(super::worth_first(millis(186), millis(690)));
        assert!(super::worth_first(millis(100), millis(100)));
        assert!(!super::worth_first(millis(312), millis(55)));
        let cache = cache();
        let text = bundle(4, FIRST_SIGHTING);
        first(cache, &text, Box::from(&b"compiled"[..]));
        assert_eq!(cache.stored(), 1);
        consumed(cache, &text);
        assert_eq!(cache.hits(), 1);
    }

    #[test]
    fn a_first_sighting_cached_or_claimed_meanwhile_is_not_built_again() {
        let cache = cache();
        let text = bundle(5, FIRST_SIGHTING);
        let Plan::First(key) = cache.plan(&name(), &text, None, true) else {
            panic!("a first sighting worth a cache at once");
        };
        let Plan::Make(building) = cache.plan(&name(), &text, None, true) else {
            panic!("a second sighting makes a cache");
        };
        assert!(cache.first(key.clone()).is_none());
        cache.store(building, Box::from(&b"compiled"[..]));
        assert!(cache.first(key).is_none());
        assert_eq!(cache.stored(), 1);
    }

    // A full list with a read entry the least recently used: a first
    // sighting's cache would drop it, so it is not kept, and its script is
    // cached at its second sighting as any other.
    #[test]
    fn a_first_sighting_never_drops_an_entry_a_context_has_read() {
        let cache = cache();
        for seed in 0..MAX_ENTRIES {
            cached(cache, &tiny(seed), Box::from(&b"compiled"[..]));
        }
        consumed(cache, &tiny(0));
        // Now the least recently used, and unread: the first sighting takes
        // its place.
        let roomy = bundle(6, FIRST_SIGHTING);
        first(cache, &roomy, Box::from(&b"compiled"[..]));
        assert_eq!(cache.stored(), 65);
        assert!(matches!(
            cache.plan(&name(), &tiny(1), None, true),
            Plan::Make(_)
        ));
        // Read in turn, every entry is read, the oldest last.
        for seed in 2..MAX_ENTRIES {
            consumed(cache, &tiny(seed));
        }
        consumed(cache, &tiny(0));
        consumed(cache, &roomy);
        let crowded = bundle(7, FIRST_SIGHTING);
        first(cache, &crowded, Box::from(&b"compiled"[..]));
        assert_eq!(cache.stored(), 65);
        consumed(cache, &tiny(2));
        let Plan::Make(building) = cache.plan(&name(), &crowded, None, true) else {
            panic!("a second sighting makes a cache");
        };
        cache.store(building, Box::from(&b"compiled"[..]));
        assert_eq!(cache.stored(), 66);
        assert!(matches!(
            cache.plan(&name(), &tiny(3), None, true),
            Plan::Make(_)
        ));
    }

    #[test]
    fn a_first_sighting_past_the_size_limit_drops_only_unread_entries() {
        let cache = cache();
        let half = || vec![0; super::MAX_BYTES / 2 + 1].into_boxed_slice();
        cached(cache, &tiny(1), half());
        consumed(cache, &tiny(1));
        first(cache, &bundle(8, FIRST_SIGHTING), half());
        assert_eq!(cache.stored(), 1);
        cached(cache, &tiny(2), half());
        assert_eq!(cache.stored(), 2);
        first(cache, &bundle(9, FIRST_SIGHTING), half());
        assert_eq!(cache.stored(), 3);
        assert!(matches!(
            cache.plan(&name(), &tiny(2), None, true),
            Plan::Make(_)
        ));
    }

    fn kept(cache: &'static CodeCache, text: &JsText, data: Box<[u8]>) -> Stored {
        let _ = cache.plan(&name(), text, None, true);
        let Plan::Make(building) = cache.plan(&name(), text, None, true) else {
            panic!("a second sighting makes a cache");
        };
        cache
            .store(building, data)
            .unwrap_or_else(|| panic!("a cache with room is kept"))
    }

    fn data_of(cache: &'static CodeCache, text: &JsText) -> Vec<u8> {
        let Plan::Consume(entry, _) = cache.plan(&name(), text, None, true) else {
            panic!("a stored cache is found");
        };
        entry.data().to_vec()
    }

    // Built again, an entry holds the new cache and becomes the most recently
    // used, read or not as it was.
    #[test]
    fn a_cache_built_again_takes_the_entrys_place() {
        let cache = cache();
        let stored = kept(cache, &tiny(0), Box::from(&b"loaded"[..]));
        for seed in 1..MAX_ENTRIES {
            cached(cache, &tiny(seed), Box::from(&b"compiled"[..]));
        }
        cache.replace(&stored, Box::from(&b"called"[..]));
        assert_eq!(cache.rebuilt(), 1);
        cached(cache, &tiny(MAX_ENTRIES), Box::from(&b"compiled"[..]));
        assert_eq!(data_of(cache, &tiny(0)), b"called");
        assert!(matches!(
            cache.plan(&name(), &tiny(1), None, true),
            Plan::Make(_)
        ));
    }

    #[test]
    fn an_entry_gone_or_too_large_built_again_is_left_as_it_was() {
        let cache = cache();
        let gone = kept(cache, &tiny(0), Box::from(&b"loaded"[..]));
        let Plan::Consume(entry, _) = cache.plan(&name(), &tiny(0), None, true) else {
            panic!("a stored cache is found");
        };
        cache.forget(&entry);
        cache.replace(&gone, Box::from(&b"called"[..]));
        assert!(matches!(
            cache.plan(&name(), &tiny(0), None, true),
            Plan::Make(_)
        ));
        let large = kept(cache, &tiny(1), Box::from(&b"loaded"[..]));
        cache.replace(&large, vec![0; super::MAX_BYTES + 1].into_boxed_slice());
        assert_eq!(data_of(cache, &tiny(1)), b"loaded");
        assert_eq!(cache.rebuilt(), 0);
    }

    // A first sighting's entry no context has read yet still makes room only
    // from unread entries when built again.
    #[test]
    fn a_first_sighting_built_again_never_drops_an_entry_a_context_has_read() {
        let cache = cache();
        let half = || vec![0; super::MAX_BYTES / 2 + 1].into_boxed_slice();
        cached(cache, &tiny(1), half());
        consumed(cache, &tiny(1));
        let text = bundle(1, FIRST_SIGHTING);
        let Plan::First(key) = cache.plan(&name(), &text, None, true) else {
            panic!("a first sighting worth a cache at once");
        };
        let building = cache.first(key).unwrap_or_else(|| panic!("unclaimed"));
        let stored = cache
            .store(building, Box::from(&b"loaded"[..]))
            .unwrap_or_else(|| panic!("a cache with room is kept"));
        cache.replace(&stored, half());
        assert_eq!(cache.rebuilt(), 0);
        assert_eq!(data_of(cache, &text), b"loaded");
        // Read now, it is built again as any other entry.
        consumed(cache, &text);
        cache.replace(&stored, half());
        assert_eq!(cache.rebuilt(), 1);
        assert!(matches!(
            cache.plan(&name(), &tiny(1), None, true),
            Plan::Make(_)
        ));
    }
}
