use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mini_racer_sync::lock;

use crate::control::JsText;
use crate::process::code_cache::Key;
use crate::text;

const MAX_SOURCES: usize = 8;
const MAX_BYTES: usize = 256 << 20;
const MAX_REFUSED: usize = 8;
const MAX_SWEPT: usize = 8;

/// A large script's source as V8 stores a string of it: a byte a unit when
/// every unit fits Latin-1, two otherwise. Never changed once made.
#[derive(Clone)]
pub(crate) enum Units {
    OneByte(Arc<[u8]>),
    TwoByte(Arc<[u16]>),
}

impl Units {
    // V8 decodes valid UTF-8 to the same units; invalid UTF-8 is left to its
    // own decoder, whose replacements this does not repeat.
    fn of(text: &JsText) -> Option<Self> {
        Some(match text {
            JsText::Latin1(bytes) => Self::OneByte(Arc::from(bytes.as_slice())),
            JsText::Utf8(bytes) if bytes.is_ascii() => Self::OneByte(Arc::from(bytes.as_slice())),
            JsText::Utf8(bytes) => text::units(bytes)?,
            JsText::Utf16(units) => units
                .iter()
                .map(|&unit| u8::try_from(unit))
                .collect::<Result<_, _>>()
                .map_or_else(
                    |_| Self::TwoByte(Arc::from(units.as_slice())),
                    Self::OneByte,
                ),
        })
    }

    fn bytes(&self) -> usize {
        match self {
            Self::OneByte(bytes) => bytes.len(),
            Self::TwoByte(units) => units.len() * 2,
        }
    }

    fn references(&self) -> usize {
        match self {
            Self::OneByte(bytes) => Arc::strong_count(bytes),
            Self::TwoByte(units) => Arc::strong_count(units),
        }
    }
}

/// One copy of each large script's source, outside every V8 heap.
///
/// A script is kept by the first context to compile it, and every context
/// compiling it, that one included, reads the kept copy in place of one on
/// its own heap.
/// Each string V8 makes from one holds a reference to it, which V8 hands back
/// when the string dies (`sys::external_string`). A source is dropped only
/// when no string holds one, so V8 never releases the last reference itself.
pub struct Sources {
    list: Mutex<List>,
    // How many sources, and how many bytes of them, may be kept and read.
    bounds: (usize, usize),
    // The number the next context's reads are told apart by.
    next_reader: AtomicU64,
    made: AtomicU64,
    reads: AtomicU64,
    conversions: AtomicU64,
}

impl Default for Sources {
    fn default() -> Self {
        Self::bounded(MAX_SOURCES, MAX_BYTES)
    }
}

#[derive(Default)]
struct List {
    // Most recently used last.
    kept: Vec<Kept>,
    // What each source converted and left unkept takes, too much for the room
    // left or, as invalid UTF-8, unbounded, so that while it is among the last
    // `MAX_REFUSED` refused it is not converted again without room for it.
    refused: Vec<(Key, usize)>,
    // The last `MAX_SWEPT` sources swept, so one made again, which a second
    // context has now read after all, is kept as such.
    swept: Vec<Key>,
}

struct Kept {
    key: Key,
    units: Units,
    // The references from this process's list and every ancestor's, which a
    // forked child never touches again; the strings reading it hold the rest.
    held: usize,
    readers: Readers,
}

// The contexts that have read a kept source. One alone, as a script evaluated
// once has, gains nothing from a copy no string reads any more; a source more
// have read is likely to be read again, and an inherited one is only copied
// again if a child drops it, so both stay until a new source needs the room.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Readers {
    One(u64),
    Many,
}

impl Kept {
    fn unread(&self) -> bool {
        self.units.references() == self.held
    }

    fn spent(&self) -> bool {
        matches!(self.readers, Readers::One(_)) && self.unread()
    }
}

impl List {
    fn find(&mut self, key: &Key, reader: u64) -> Option<Units> {
        let at = self.kept.iter().position(|kept| kept.key == *key)?;
        let mut found = self.kept.remove(at);
        if found.readers != Readers::One(reader) {
            found.readers = Readers::Many;
        }
        let units = found.units.clone();
        self.kept.push(found);
        Some(units)
    }

    // V8's release of a string touches only the count, so a copy whose last
    // string has gone waits for the next `Sources::units` call.
    fn sweep(&mut self) -> Vec<Kept> {
        let spent: Vec<Kept> = self.kept.extract_if(.., |kept| kept.spent()).collect();
        for kept in &spent {
            if self.swept.len() >= MAX_SWEPT {
                self.swept.remove(0);
            }
            self.swept.push(kept.key.clone());
        }
        spent
    }

    fn unswept(&mut self, key: &Key) -> bool {
        let before = self.swept.len();
        self.swept.retain(|swept| swept != key);
        self.swept.len() < before
    }

    // Whether `bytes` more fit beside what strings still read, which nothing
    // can drop.
    fn room(&self, bytes: usize, (sources, most): (usize, usize)) -> bool {
        let (read, read_bytes) = self
            .kept
            .iter()
            .filter(|kept| !kept.unread())
            .fold((0, 0), |(count, total), kept| {
                (count + 1, total + kept.units.bytes())
            });
        read < sources && read_bytes.saturating_add(bytes) <= most
    }

    fn refuse(&mut self, key: &Key, bytes: usize) {
        self.refused.retain(|(refused, _)| refused != key);
        if self.refused.len() >= MAX_REFUSED {
            self.refused.remove(0);
        }
        self.refused.push((key.clone(), bytes));
    }

    fn least_bytes(&self, key: &Key, text: &JsText) -> usize {
        self.refused
            .iter()
            .find(|(refused, _)| refused == key)
            .map_or_else(|| least_bytes(text), |&(_, bytes)| bytes)
    }
}

// The fewest bytes `text` can take as units, so a source that cannot fit is
// never converted: UTF-8 takes at least a byte for every two, as Latin-1
// written in UTF-8 does.
const fn least_bytes(text: &JsText) -> usize {
    match text {
        JsText::Latin1(bytes) => bytes.len(),
        JsText::Utf8(bytes) => bytes.len().div_ceil(2),
        JsText::Utf16(units) => units.len(),
    }
}

impl Sources {
    fn bounded(sources: usize, bytes: usize) -> Self {
        Self {
            list: Mutex::default(),
            bounds: (sources, bytes),
            next_reader: AtomicU64::new(0),
            made: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            conversions: AtomicU64::new(0),
        }
    }

    /// A number for a new context to read shared sources as, told apart from
    /// every other context of this process.
    pub(crate) fn reader(&self) -> u64 {
        self.next_reader.fetch_add(1, Ordering::Relaxed)
    }

    /// The shared source of the script `text` under `key`, kept from the first
    /// context that asks for it, for the context `reader`. `None` leaves the
    /// context its own copy: the source is invalid UTF-8, or too large for
    /// what is kept and still read. Only a source that may fit is converted,
    /// outside the lock. Every call looks up the requested source, then drops
    /// each other copy that no string reads and that one context alone ever
    /// read.
    pub(crate) fn units(&self, key: &Key, text: &JsText, reader: u64) -> Option<Units> {
        let mut list = lock(&self.list);
        // Found before the sweep, so a copy the context before this one left
        // unread is read again, and kept, rather than copied again.
        let found = list.find(key, reader);
        let spent = list.sweep();
        let room = found.is_none() && list.room(list.least_bytes(key, text), self.bounds);
        drop(list);
        drop(spent);
        if !room {
            return found;
        }
        self.conversions.fetch_add(1, Ordering::Relaxed);
        let units = Units::of(text);
        let mut list = lock(&self.list);
        // Made meanwhile by a context loading the same script.
        if let Some(found) = list.find(key, reader) {
            return Some(found);
        }
        let bytes = units.as_ref().map_or(usize::MAX, Units::bytes);
        if !list.room(bytes, self.bounds) {
            list.refuse(key, bytes);
            drop(list);
            return None;
        }
        let units = units?;
        list.refused.retain(|(refused, _)| refused != key);
        let (sources, most) = self.bounds;
        let mut dropped = Vec::new();
        while list.kept.len() >= sources
            || list
                .kept
                .iter()
                .map(|kept| kept.units.bytes())
                .sum::<usize>()
                + bytes
                > most
        {
            let Some(at) = list.kept.iter().position(Kept::unread) else {
                break;
            };
            dropped.push(list.kept.remove(at));
        }
        let readers = if list.unswept(key) {
            Readers::Many
        } else {
            Readers::One(reader)
        };
        list.kept.push(Kept {
            key: key.clone(),
            units: units.clone(),
            held: 1,
            readers,
        });
        drop(list);
        // Freed once the lock is released, as a large source takes a while,
        // like the spent copies above.
        drop(dropped);
        self.made.fetch_add(1, Ordering::Relaxed);
        Some(units)
    }

    pub(crate) fn read(&self) {
        self.reads.fetch_add(1, Ordering::Relaxed);
    }

    /// A forked child's sources: what its parent kept, unless a thread of the
    /// parent held them at the fork. The parent's list keeps its references
    /// in the child, so the child frees none of these.
    pub(crate) fn inherited(&self) -> Self {
        let list = self
            .list
            .try_lock()
            .map(|list| List {
                kept: list
                    .kept
                    .iter()
                    .map(|kept| Kept {
                        key: kept.key.clone(),
                        units: kept.units.clone(),
                        held: kept.held + 1,
                        readers: Readers::Many,
                    })
                    .collect(),
                refused: list.refused.clone(),
                swept: list.swept.clone(),
            })
            .unwrap_or_default();
        Self {
            list: Mutex::new(list),
            ..Self::bounded(self.bounds.0, self.bounds.1)
        }
    }

    /// Sources this process made and kept.
    #[must_use]
    pub fn made(&self) -> u64 {
        self.made.load(Ordering::Relaxed)
    }

    /// Strings this process's contexts made from a kept source.
    #[must_use]
    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::Relaxed)
    }

    /// Sources this process converted to the units V8 reads, kept or not.
    #[must_use]
    pub fn conversions(&self) -> u64 {
        self.conversions.load(Ordering::Relaxed)
    }

    /// Strings alive now that read a kept source.
    #[must_use]
    pub fn reading(&self) -> usize {
        lock(&self.list)
            .kept
            .iter()
            .map(|kept| kept.units.references().saturating_sub(kept.held))
            .sum()
    }
}

#[cfg(test)]
#[allow(clippy::panic, reason = "a test fails by panicking")]
mod tests {
    use std::sync::Arc;

    use super::{MAX_SOURCES, Sources, Units};
    use crate::control::JsText;
    use crate::process::code_cache::{CodeCache, Key};

    // A script's key, as its first sighting in a context sharing it has it.
    fn key(text: &JsText) -> Key {
        let cache: &'static CodeCache = Box::leak(Box::default());
        cache
            .plan(&JsText::from("bundle.js"), text, None, true)
            .key()
            .cloned()
            .unwrap_or_else(|| panic!("a shared first sighting has a key"))
    }

    fn script(seed: u8) -> JsText {
        JsText::Latin1(vec![seed; 2 << 20])
    }

    fn one_byte(units: Option<Units>) -> Arc<[u8]> {
        match units {
            Some(Units::OneByte(bytes)) => bytes,
            _ => panic!("one byte a unit"),
        }
    }

    fn two_byte(units: Option<Units>) -> Arc<[u16]> {
        match units {
            Some(Units::TwoByte(units)) => units,
            _ => panic!("two bytes a unit"),
        }
    }

    #[test]
    fn keeps_a_source_in_the_form_v8_stores_a_string_of_it() {
        assert_eq!(
            *one_byte(Units::of(&JsText::Latin1(vec![b'a', 0xE9]))),
            [b'a', 0xE9]
        );
        assert_eq!(*one_byte(Units::of(&JsText::from("a\0b"))), *b"a\0b");
        assert_eq!(
            *two_byte(Units::of(&JsText::from("\u{FEFF}a"))),
            [0xFEFF, 0x61]
        );
        assert_eq!(
            *one_byte(Units::of(&JsText::from("caf\u{E9}\u{FF}"))),
            *b"caf\xE9\xFF"
        );
        assert_eq!(
            *two_byte(Units::of(&JsText::from("a\u{100}\u{1F600}"))),
            [0x61, 0x100, 0xD83D, 0xDE00]
        );
        assert_eq!(
            *one_byte(Units::of(&JsText::Utf16(vec![0x61, 0xE9]))),
            [0x61, 0xE9]
        );
        assert_eq!(
            *two_byte(Units::of(&JsText::Utf16(vec![0xD800, 0x61, 0xDC00]))),
            [0xD800, 0x61, 0xDC00]
        );
        assert!(Units::of(&JsText::Utf8(vec![b'a', 0xFF])).is_none());
    }

    #[test]
    fn every_context_compiling_a_script_reads_one_copy() {
        let sources = Sources::default();
        let text = script(1);
        let key = key(&text);
        let first = one_byte(sources.units(&key, &text, sources.reader()));
        let second = one_byte(sources.units(&key, &text, sources.reader()));
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(sources.made(), 1);
        assert_eq!(sources.reading(), 2);
        drop((first, second));
        assert_eq!(sources.reading(), 0);
    }

    fn same(first: Option<&Units>, second: Option<&Units>) -> bool {
        match (first, second) {
            (Some(Units::OneByte(first)), Some(Units::OneByte(second))) => {
                Arc::ptr_eq(first, second)
            }
            (Some(Units::TwoByte(first)), Some(Units::TwoByte(second))) => {
                Arc::ptr_eq(first, second)
            }
            _ => false,
        }
    }

    // A copy two contexts have read, the second still reading it.
    fn shared(sources: &Sources, key: &Key, text: &JsText) -> Option<Units> {
        drop(sources.units(key, text, sources.reader()));
        sources.units(key, text, sources.reader())
    }

    #[test]
    fn a_source_still_read_is_kept_and_an_unread_one_makes_room() {
        let sources = Sources::default();
        let count = u8::try_from(MAX_SOURCES).unwrap_or(u8::MAX);
        let texts: Vec<JsText> = (10..10 + count).map(script).collect();
        let keys: Vec<Key> = texts.iter().map(key).collect();
        let mut read: Vec<Option<Units>> = texts
            .iter()
            .zip(&keys)
            .map(|(text, key)| shared(&sources, key, text))
            .collect();
        let extra = script(100);
        let extra_key = key(&extra);
        assert!(shared(&sources, &extra_key, &extra).is_none());
        drop(read.remove(3));
        drop(read.remove(0));
        drop(sources.units(&keys[3], &texts[3], sources.reader()));
        // Of the two unread, the least recently used makes room.
        assert!(shared(&sources, &extra_key, &extra).is_some());
        let made = sources.made();
        assert_eq!(made, u64::from(count) + 1);
        drop(sources.units(&keys[3], &texts[3], sources.reader()));
        assert_eq!(sources.made(), made);
        drop(sources.units(&keys[0], &texts[0], sources.reader()));
        assert_eq!(sources.made(), made + 1);
        for (at, held) in [1, 2, 4, 5, 6, 7].into_iter().zip(&read) {
            assert!(same(
                sources
                    .units(&keys[at], &texts[at], sources.reader())
                    .as_ref(),
                held.as_ref()
            ));
        }
        assert_eq!(sources.made(), made + 1);
    }

    // One context's copy, however often it read it, waits for no new source
    // once no string reads it: the next call drops and frees it.
    #[test]
    fn a_copy_one_context_read_goes_at_the_next_call_once_unread() {
        let sources = Sources::default();
        let (text, other) = (script(70), script(71));
        let (key, other_key) = (key(&text), key(&other));
        let reader = sources.reader();
        let first = one_byte(sources.units(&key, &text, reader));
        let again = one_byte(sources.units(&key, &text, reader));
        assert!(Arc::ptr_eq(&first, &again));
        let copy = Arc::downgrade(&first);
        drop((first, again));
        assert!(copy.upgrade().is_some());
        drop(sources.units(&other_key, &other, sources.reader()));
        assert!(copy.upgrade().is_none());
        drop(sources.units(&key, &text, sources.reader()));
        assert_eq!(sources.made(), 3);
    }

    #[test]
    fn a_copy_two_contexts_read_in_turn_stays_once_unread() {
        let sources = Sources::default();
        let (text, other) = (script(72), script(73));
        let (key, other_key) = (key(&text), key(&other));
        let copy = Arc::downgrade(&one_byte(shared(&sources, &key, &text)));
        drop(sources.units(&other_key, &other, sources.reader()));
        let again = one_byte(sources.units(&key, &text, sources.reader()));
        assert!(
            copy.upgrade()
                .is_some_and(|copy| Arc::ptr_eq(&copy, &again))
        );
        assert_eq!(sources.made(), 2);
    }

    #[test]
    fn a_copy_a_second_context_reads_while_the_first_does_stays_once_unread() {
        let sources = Sources::default();
        let (text, other) = (script(74), script(75));
        let (key, other_key) = (key(&text), key(&other));
        let first = one_byte(sources.units(&key, &text, sources.reader()));
        let second = one_byte(sources.units(&key, &text, sources.reader()));
        let copy = Arc::downgrade(&first);
        drop((first, second));
        drop(sources.units(&other_key, &other, sources.reader()));
        let again = one_byte(sources.units(&key, &text, sources.reader()));
        assert!(
            copy.upgrade()
                .is_some_and(|copy| Arc::ptr_eq(&copy, &again))
        );
        assert_eq!(sources.made(), 2);
    }

    // Contexts loading two scripts in turn, each gone before the next, sweep
    // each other's copy; a copy made again after its sweep is kept as one more
    // than one context read, so each script is converted twice in all.
    #[test]
    fn scripts_loaded_in_turn_by_short_lived_contexts_stay_after_their_second_copy() {
        let sources = Sources::default();
        let texts = [script(78), script(79)];
        let keys = texts.each_ref().map(key);
        for _ in 0..2 {
            for (key, text) in keys.iter().zip(&texts) {
                drop(sources.units(key, text, sources.reader()));
            }
        }
        assert_eq!(sources.made(), 4);
        for _ in 0..3 {
            for (key, text) in keys.iter().zip(&texts) {
                drop(sources.units(key, text, sources.reader()));
            }
        }
        assert_eq!(sources.made(), 4);
    }

    #[test]
    fn a_full_list_converts_nothing() {
        let sources = Sources::default();
        let count = u8::try_from(MAX_SOURCES).unwrap_or(u8::MAX);
        let read: Vec<Option<Units>> = (40..40 + count)
            .map(|seed| {
                let text = script(seed);
                sources.units(&key(&text), &text, sources.reader())
            })
            .collect();
        let converted = sources.conversions();
        let extra = script(100);
        let extra_key = key(&extra);
        for _ in 0..3 {
            assert!(
                sources
                    .units(&extra_key, &extra, sources.reader())
                    .is_none()
            );
        }
        assert_eq!(sources.conversions(), converted);
        drop(read);
        assert!(
            sources
                .units(&extra_key, &extra, sources.reader())
                .is_some()
        );
        assert_eq!(sources.conversions(), converted + 1);
    }

    // Under a 5 MiB bound with 2 MiB still read, 4 MiB of Latin-1 cannot fit
    // and is never converted; 2 MiB of UTF-8 may fit, until converting it
    // shows one astral character makes it 4.
    #[test]
    fn a_source_is_converted_only_while_it_may_fit() {
        let sources = Sources::bounded(MAX_SOURCES, 5 << 20);
        let read = script(50);
        let still_read = sources.units(&key(&read), &read, sources.reader());
        assert!(still_read.is_some());
        let too_large = JsText::Latin1(vec![51; 4 << 20]);
        assert!(
            sources
                .units(&key(&too_large), &too_large, sources.reader())
                .is_none()
        );
        assert_eq!(sources.conversions(), 1);
        let mut astral = vec![b'a'; 2 << 20];
        astral.extend_from_slice("\u{1F600}".as_bytes());
        let astral = JsText::Utf8(astral);
        let astral_key = key(&astral);
        for _ in 0..3 {
            assert!(
                sources
                    .units(&astral_key, &astral, sources.reader())
                    .is_none()
            );
        }
        assert_eq!(sources.conversions(), 2);
        drop(still_read);
        assert!(two_byte(sources.units(&astral_key, &astral, sources.reader())).len() > 2 << 20);
        assert_eq!((sources.conversions(), sources.made()), (3, 2));
    }

    #[test]
    fn invalid_utf8_is_tried_once() {
        let sources = Sources::default();
        let mut invalid = vec![b'a'; 2 << 20];
        invalid.push(0xFF);
        let invalid = JsText::Utf8(invalid);
        let invalid_key = key(&invalid);
        for _ in 0..3 {
            assert!(
                sources
                    .units(&invalid_key, &invalid, sources.reader())
                    .is_none()
            );
        }
        assert_eq!((sources.conversions(), sources.made()), (1, 0));
    }

    #[test]
    fn a_child_reads_its_parents_copy_and_never_frees_it() {
        let parent: &'static Sources = Box::leak(Box::default());
        let text = script(2);
        let key = key(&text);
        let shared = parent.units(&key, &text, parent.reader());
        let child = parent.inherited();
        let in_child = child.units(&key, &text, child.reader());
        assert!(same(shared.as_ref(), in_child.as_ref()));
        assert_eq!(child.reading(), 2);
        let copy = Arc::downgrade(&one_byte(shared));
        drop(in_child);
        // Sources the child still reads fill its list, so the unread
        // inherited copy makes room.
        let count = u8::try_from(MAX_SOURCES).unwrap_or(u8::MAX);
        let others: Vec<Option<Units>> = (30..30 + count)
            .map(|seed| {
                let other = script(seed);
                child.units(&self::key(&other), &other, child.reader())
            })
            .collect();
        assert_eq!(copy.strong_count(), 1);
        drop(others);
        let again = one_byte(child.units(&key, &text, child.reader()));
        assert!(
            !copy
                .upgrade()
                .is_some_and(|copy| Arc::ptr_eq(&copy, &again))
        );
        let from_parent = one_byte(parent.units(&key, &text, parent.reader()));
        assert!(
            copy.upgrade()
                .is_some_and(|copy| Arc::ptr_eq(&copy, &from_parent))
        );
    }

    // Dropping an inherited copy frees nothing in a child, and its next
    // context loading the script would copy it again, so the child keeps
    // one only its parent's context read.
    #[test]
    fn a_child_keeps_an_inherited_copy_one_context_read() {
        let parent: &'static Sources = Box::leak(Box::default());
        let (text, other) = (script(76), script(77));
        let (key, other_key) = (key(&text), key(&other));
        let copy = Arc::downgrade(&one_byte(parent.units(&key, &text, parent.reader())));
        let child = parent.inherited();
        drop(child.units(&other_key, &other, child.reader()));
        let in_child = one_byte(child.units(&key, &text, child.reader()));
        assert!(
            copy.upgrade()
                .is_some_and(|copy| Arc::ptr_eq(&copy, &in_child))
        );
        assert_eq!(child.made(), 1);
    }

    // A child's inherited copies stay read while its parent's strings hold
    // them, so a child forked with a full list must not convert either.
    #[test]
    fn a_child_inheriting_a_full_list_converts_nothing() {
        let parent = Sources::default();
        let count = u8::try_from(MAX_SOURCES).unwrap_or(u8::MAX);
        let read: Vec<Option<Units>> = (60..60 + count)
            .map(|seed| {
                let text = script(seed);
                parent.units(&key(&text), &text, parent.reader())
            })
            .collect();
        let child = parent.inherited();
        let extra = script(110);
        let extra_key = key(&extra);
        for _ in 0..3 {
            assert!(child.units(&extra_key, &extra, child.reader()).is_none());
        }
        assert_eq!(child.conversions(), 0);
        drop(read);
    }

    #[test]
    fn a_child_forked_while_the_sources_were_held_starts_without_them() {
        let parent = Sources::default();
        let text = script(3);
        let key = key(&text);
        let shared = one_byte(parent.units(&key, &text, parent.reader()));
        let held = parent.list.lock();
        let child = parent.inherited();
        drop(held);
        assert!(!Arc::ptr_eq(
            &shared,
            &one_byte(child.units(&key, &text, child.reader()))
        ));
        assert_eq!(child.made(), 1);
    }
}
