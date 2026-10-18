use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mini_racer_sync::lock;

use crate::control::JsText;
use crate::process::code_cache::Key;

const MAX_SOURCES: usize = 8;
const MAX_BYTES: usize = 256 << 20;

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
            JsText::Utf8(bytes) => {
                let text = std::str::from_utf8(bytes).ok()?;
                text.chars()
                    .map(u8::try_from)
                    .collect::<Result<_, _>>()
                    .map_or_else(
                        |_| Self::TwoByte(text.encode_utf16().collect()),
                        Self::OneByte,
                    )
            }
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
/// A script is kept once the process has seen it before, and every context
/// compiling it then reads the kept copy in place of one on its own heap.
/// Each string V8 makes from one holds a reference to it, which V8 hands back
/// when the string dies (`sys::external_string`). A source is dropped only
/// when no string holds one, so V8 never releases the last reference itself.
#[derive(Default)]
pub struct Sources {
    // Most recently used last.
    kept: Mutex<Vec<Kept>>,
    made: AtomicU64,
    reads: AtomicU64,
}

struct Kept {
    key: Key,
    units: Units,
    // The references from this process's list and every ancestor's, which a
    // forked child never touches again; the strings reading it hold the rest.
    held: usize,
}

impl Kept {
    fn unread(&self) -> bool {
        self.units.references() == self.held
    }
}

impl Sources {
    /// The shared source of the script `text` under `key`, kept from the first
    /// context that asks for it. `None` leaves the context its own copy: the
    /// source is invalid UTF-8, or too large for what is kept and still read.
    pub(crate) fn units(&self, key: &Key, text: &JsText) -> Option<Units> {
        if let Some(units) = self.find(key) {
            return Some(units);
        }
        let units = Units::of(text)?;
        let mut kept = lock(&self.kept);
        if let Some(at) = kept.iter().position(|kept| kept.key == *key) {
            // Made meanwhile by a context loading the same script.
            let found = kept.remove(at);
            let shared = found.units.clone();
            kept.push(found);
            return Some(shared);
        }
        let (read, read_bytes) = kept
            .iter()
            .filter(|kept| !kept.unread())
            .fold((0, 0), |(count, bytes), kept| {
                (count + 1, bytes + kept.units.bytes())
            });
        if read >= MAX_SOURCES || read_bytes + units.bytes() > MAX_BYTES {
            return None;
        }
        let mut dropped = Vec::new();
        while kept.len() >= MAX_SOURCES
            || kept.iter().map(|kept| kept.units.bytes()).sum::<usize>() + units.bytes() > MAX_BYTES
        {
            let Some(at) = kept.iter().position(Kept::unread) else {
                break;
            };
            dropped.push(kept.remove(at));
        }
        kept.push(Kept {
            key: key.clone(),
            units: units.clone(),
            held: 1,
        });
        drop(kept);
        // Freed once the lock is released, as a large source takes a while.
        drop(dropped);
        self.made.fetch_add(1, Ordering::Relaxed);
        Some(units)
    }

    fn find(&self, key: &Key) -> Option<Units> {
        let mut kept = lock(&self.kept);
        let at = kept.iter().position(|kept| kept.key == *key)?;
        let found = kept.remove(at);
        let units = found.units.clone();
        kept.push(found);
        drop(kept);
        Some(units)
    }

    pub(crate) fn read(&self) {
        self.reads.fetch_add(1, Ordering::Relaxed);
    }

    /// A forked child's sources: what its parent kept, unless a thread of the
    /// parent held them at the fork. The parent's list keeps its references
    /// in the child, so the child frees none of these.
    pub(crate) fn inherited(&self) -> Self {
        let kept = self
            .kept
            .try_lock()
            .map(|kept| {
                kept.iter()
                    .map(|kept| Kept {
                        key: kept.key.clone(),
                        units: kept.units.clone(),
                        held: kept.held + 1,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            kept: Mutex::new(kept),
            made: AtomicU64::new(0),
            reads: AtomicU64::new(0),
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

    /// Strings alive now that read a kept source.
    #[must_use]
    pub fn reading(&self) -> usize {
        lock(&self.kept)
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

    // A script's key once the code cache has seen it twice.
    fn key(text: &JsText) -> Key {
        let cache: &'static CodeCache = Box::leak(Box::default());
        let name = JsText::from("bundle.js");
        let _ = cache.plan(&name, text, None);
        cache
            .plan(&name, text, None)
            .key()
            .cloned()
            .unwrap_or_else(|| panic!("a second sighting has a key"))
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
        let first = one_byte(sources.units(&key, &text));
        let second = one_byte(sources.units(&key, &text));
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

    #[test]
    fn a_source_still_read_is_kept_and_an_unread_one_makes_room() {
        let sources = Sources::default();
        let count = u8::try_from(MAX_SOURCES).unwrap_or(u8::MAX);
        let texts: Vec<JsText> = (10..10 + count).map(script).collect();
        let keys: Vec<Key> = texts.iter().map(key).collect();
        let mut read: Vec<Option<Units>> = texts
            .iter()
            .zip(&keys)
            .map(|(text, key)| sources.units(key, text))
            .collect();
        let extra = script(100);
        let extra_key = key(&extra);
        assert!(sources.units(&extra_key, &extra).is_none());
        drop(read.remove(3));
        drop(read.remove(0));
        drop(sources.units(&keys[3], &texts[3]));
        // Of the two unread, the least recently used makes room.
        assert!(sources.units(&extra_key, &extra).is_some());
        let made = sources.made();
        assert_eq!(made, u64::from(count) + 1);
        drop(sources.units(&keys[3], &texts[3]));
        assert_eq!(sources.made(), made);
        drop(sources.units(&keys[0], &texts[0]));
        assert_eq!(sources.made(), made + 1);
        for (at, held) in [1, 2, 4, 5, 6, 7].into_iter().zip(&read) {
            assert!(same(
                sources.units(&keys[at], &texts[at]).as_ref(),
                held.as_ref()
            ));
        }
        assert_eq!(sources.made(), made + 1);
    }

    #[test]
    fn a_child_reads_its_parents_copy_and_never_frees_it() {
        let parent: &'static Sources = Box::leak(Box::default());
        let text = script(2);
        let key = key(&text);
        let shared = parent.units(&key, &text);
        let child = parent.inherited();
        let in_child = child.units(&key, &text);
        assert!(same(shared.as_ref(), in_child.as_ref()));
        assert_eq!(child.reading(), 2);
        let copy = Arc::downgrade(&one_byte(shared));
        drop(in_child);
        let count = u8::try_from(MAX_SOURCES).unwrap_or(u8::MAX);
        for seed in 30..30 + count {
            let other = script(seed);
            drop(child.units(&self::key(&other), &other));
        }
        assert_eq!(copy.strong_count(), 1);
        let again = one_byte(child.units(&key, &text));
        assert!(
            !copy
                .upgrade()
                .is_some_and(|copy| Arc::ptr_eq(&copy, &again))
        );
        let from_parent = one_byte(parent.units(&key, &text));
        assert!(
            copy.upgrade()
                .is_some_and(|copy| Arc::ptr_eq(&copy, &from_parent))
        );
    }

    #[test]
    fn a_child_forked_while_the_sources_were_held_starts_without_them() {
        let parent = Sources::default();
        let text = script(3);
        let key = key(&text);
        let shared = one_byte(parent.units(&key, &text));
        let held = parent.kept.lock();
        let child = parent.inherited();
        drop(held);
        assert!(!Arc::ptr_eq(&shared, &one_byte(child.units(&key, &text))));
        assert_eq!(child.made(), 1);
    }
}
