use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::{FlagError, HeapLimitError};

const MAX_FLAG_BYTES: usize = 256;

/// The least heap limit `set_heap_limit` takes. V8 15.2 gives the young
/// generation at least 6 MiB of a limit, and under 32 MiB a `Map` growing
/// past the limit, or a `JSON.parse` hit twice, aborts the process.
pub const MIN_HEAP_LIMIT: usize = 32 << 20;
/// The most: the largest heap V8 sizes by default on a 64-bit host.
pub const MAX_HEAP_LIMIT: usize = 4 << 30;

// The flags whose sizes V8 15.2's `Heap::ConfigureHeap` takes in place of the
// limits an isolate's `CreateParams` set, when not 0.
const SIZING: [&str; 3] = ["max_old_space_size", "max_heap_size", "max_semi_space_size"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    MultiThreaded,
    SingleThreaded,
}

struct Flags {
    list: Vec<String>,
    frozen: bool,
    mode: Mode,
    // What V8 will hold for each of `SIZING` once it has read `list`.
    sizes: [u64; SIZING.len()],
}

static FLAGS: Mutex<Flags> = Mutex::new(Flags {
    list: Vec::new(),
    frozen: false,
    mode: Mode::MultiThreaded,
    sizes: [0; SIZING.len()],
});

// The heap limit for contexts made from now on, 0 for none. Written under the
// flags lock, so a flag and a limit never both pass their checks, and read
// without it, so a context made in a forked child never waits for it.
static HEAP_LIMIT: AtomicUsize = AtomicUsize::new(0);

fn flags() -> MutexGuard<'static, Flags> {
    FLAGS.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Normalized {
    flag: String,
    mode: Option<Mode>,
}

fn normalize(name: &[u8], value: Option<&[u8]>) -> Result<Normalized, FlagError> {
    if name.contains(&0) || value.is_some_and(|v| v.contains(&0)) {
        return Err(FlagError::Nul);
    }
    let name = name.strip_prefix(b"--").unwrap_or(name);
    let mut flag = Vec::with_capacity(name.len() + value.map_or(0, <[u8]>::len) + 3);
    flag.extend_from_slice(b"--");
    flag.extend_from_slice(name);
    if let Some(value) = value {
        flag.push(b'=');
        flag.extend_from_slice(value);
    }
    if flag.len() >= MAX_FLAG_BYTES {
        return Err(FlagError::TooLong);
    }
    let squashed: Vec<u8> = flag
        .iter()
        .copied()
        .filter(|b| !matches!(b, b'-' | b'_'))
        .collect();
    let mode = match squashed.as_slice() {
        b"singlethreaded" => Some(Mode::SingleThreaded),
        b"nosinglethreaded" => Some(Mode::MultiThreaded),
        _ => None,
    };
    Ok(Normalized {
        flag: String::from_utf8_lossy(&flag).into_owned(),
        mode,
    })
}

pub fn push_flag(name: &[u8], value: Option<&[u8]>) -> Result<(), FlagError> {
    let normalized = normalize(name, value)?;
    let mut flags = flags();
    if flags.frozen {
        return Err(FlagError::AlreadyInitialized);
    }
    let mut sizes = flags.sizes;
    read_sizes(&normalized.flag, &mut sizes);
    if HEAP_LIMIT.load(Ordering::Relaxed) != 0
        && let Some(flag) = sizing(&sizes)
    {
        return Err(FlagError::OverridesHeapLimit(flag));
    }
    flags.sizes = sizes;
    if let Some(mode) = normalized.mode {
        flags.mode = mode;
    }
    flags.list.push(normalized.flag);
    drop(flags);
    Ok(())
}

/// Sets the heap limit, in bytes, of the contexts made from now on, or with
/// `None` clears it.
pub fn set_heap_limit(limit: Option<usize>) -> Result<(), HeapLimitError> {
    if limit.is_some_and(|limit| !(MIN_HEAP_LIMIT..=MAX_HEAP_LIMIT).contains(&limit)) {
        return Err(HeapLimitError::Range);
    }
    let flags = flags();
    if limit.is_some()
        && let Some(flag) = sizing(&flags.sizes)
    {
        return Err(HeapLimitError::Overridden(flag));
    }
    HEAP_LIMIT.store(limit.unwrap_or(0), Ordering::Relaxed);
    drop(flags);
    Ok(())
}

#[must_use]
pub fn heap_limit() -> Option<usize> {
    NonZeroUsize::new(HEAP_LIMIT.load(Ordering::Relaxed)).map(NonZeroUsize::get)
}

fn sizing(sizes: &[u64; SIZING.len()]) -> Option<&'static str> {
    SIZING
        .iter()
        .zip(sizes)
        .find_map(|(flag, size)| (*size != 0).then_some(*flag))
}

// What V8's `SetFlagsFromString` does to `SIZING` given one flag: it splits
// the string at white space and reads each argument that starts with a dash
// as a flag, a `no` before its name included, taking a value after `=` or
// from the next argument, and stops at the first error. A size is set before
// V8 finds a negation or characters after its number wrong. A flag of any
// other name is taken to need no value, so a size V8 would read as its value
// counts as set, and V8's stop at an unknown flag is not followed.
fn read_sizes(flag: &str, sizes: &mut [u64; SIZING.len()]) {
    // C's isspace, which V8 splits at.
    let space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r');
    let mut args = flag.split(space).filter(|arg| !arg.is_empty());
    while let Some(arg) = args.next() {
        if arg == "--" {
            return;
        }
        let Some(arg) = arg.strip_prefix('-') else {
            continue;
        };
        let arg = arg.strip_prefix('-').unwrap_or(arg);
        let (negated, arg) = arg.strip_prefix("no").map_or((false, arg), |arg| {
            (true, arg.strip_prefix(['-', '_']).unwrap_or(arg))
        });
        let (name, value) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (arg, None),
        };
        let Some(index) = SIZING.iter().position(|size| same_flag(size, name)) else {
            continue;
        };
        let Some((size, whole)) = value.or_else(|| args.next()).and_then(megabytes) else {
            return;
        };
        if let Some(slot) = sizes.get_mut(index) {
            *slot = size;
        }
        if negated || !whole {
            return;
        }
    }
}

// V8 takes `-` and `_` in a flag's name as one character.
fn same_flag(a: &str, b: &str) -> bool {
    let unify = |c: u8| if c == b'_' { b'-' } else { c };
    a.len() == b.len() && a.bytes().zip(b.bytes()).all(|(a, b)| unify(a) == unify(b))
}

// `strtoll` in base 10, as V8 reads a size: `None` where V8 leaves the flag
// alone, a negative number or one past `i64::MAX`, and where no digit comes
// first, which only some C libraries refuse; with the number, whether it is
// all of `text`.
fn megabytes(text: &str) -> Option<(u64, bool)> {
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, text.get(1..)?),
        Some(b'+') => (false, text.get(1..)?),
        _ => (false, text),
    };
    let count = digits.bytes().take_while(u8::is_ascii_digit).count();
    let number = digits.get(..count).filter(|number| !number.is_empty())?;
    let size = u64::try_from(number.parse::<i64>().ok()?).ok()?;
    if negative && size != 0 {
        return None;
    }
    Some((size, count == digits.len()))
}

// The mode once frozen, readable without the flags lock, which runner threads
// must not take: a fork that does not wait for them could leave it held.
static FROZEN_MODE: AtomicU8 = AtomicU8::new(0);

#[must_use]
pub fn mode() -> Mode {
    match FROZEN_MODE.load(Ordering::Acquire) {
        1 => Mode::MultiThreaded,
        2 => Mode::SingleThreaded,
        _ => flags().mode,
    }
}

#[must_use]
pub fn is_initialized() -> bool {
    flags().frozen
}

// The last --stack_size (or --stack-size) value, in KiB, as V8 parses it.
#[must_use]
pub fn stack_size_kb() -> Option<usize> {
    flags().list.iter().rev().find_map(|flag| {
        let value = flag
            .strip_prefix("--stack_size=")
            .or_else(|| flag.strip_prefix("--stack-size="))?;
        value.parse().ok()
    })
}

pub(crate) fn freeze() -> Vec<String> {
    let mut flags = flags();
    flags.frozen = true;
    let frozen = match flags.mode {
        Mode::MultiThreaded => 1,
        Mode::SingleThreaded => 2,
    };
    FROZEN_MODE.store(frozen, Ordering::Release);
    flags.list.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(name: &[u8], value: Option<&[u8]>) -> Result<String, FlagError> {
        normalize(name, value).map(|n| n.flag)
    }

    #[test]
    fn normalizes_like_upstream() {
        assert_eq!(flag(b"use_strict", None).as_deref(), Ok("--use_strict"));
        assert_eq!(flag(b"--use_strict", None).as_deref(), Ok("--use_strict"));
        assert_eq!(
            flag(b"stack_size", Some(b"512")).as_deref(),
            Ok("--stack_size=512")
        );
        assert_eq!(flag(b"bad\0", None), Err(FlagError::Nul));
        assert_eq!(flag(b"name", Some(b"v\0")), Err(FlagError::Nul));
        assert!(flag(&[b'a'; 253], None).is_ok());
        assert_eq!(flag(&[b'a'; 254], None), Err(FlagError::TooLong));
        assert!(flag(&[b'a'; 250], Some(b"xx")).is_ok());
        assert_eq!(flag(&[b'a'; 251], Some(b"xx")), Err(FlagError::TooLong));
    }

    #[test]
    fn detects_single_threaded_spellings() {
        for name in [
            &b"single_threaded"[..],
            b"--single-threaded",
            b"singlethreaded",
        ] {
            assert_eq!(
                normalize(name, None).map(|n| n.mode),
                Ok(Some(Mode::SingleThreaded))
            );
        }
        for name in [
            &b"no_single_threaded"[..],
            b"--no-single-threaded",
            b"nosingle_threaded",
        ] {
            assert_eq!(
                normalize(name, None).map(|n| n.mode),
                Ok(Some(Mode::MultiThreaded))
            );
        }
        assert_eq!(normalize(b"use_strict", None).map(|n| n.mode), Ok(None));
    }

    fn sizes_after(flags: &[(&[u8], Option<&[u8]>)]) -> [u64; 3] {
        let mut sizes = [0; 3];
        for (name, value) in flags {
            read_sizes(&flag(name, *value).expect("flag"), &mut sizes);
        }
        sizes
    }

    #[test]
    fn reads_heap_sizes_in_every_spelling_v8_takes() {
        for (name, value, sizes) in [
            (&b"max_old_space_size"[..], Some(&b"128"[..]), [128, 0, 0]),
            (b"--max-old-space-size", Some(b"128"), [128, 0, 0]),
            (b"max-heap-size", Some(b"256"), [0, 256, 0]),
            (b"max_semi_space_size", Some(b"16"), [0, 0, 16]),
            (b"max-semi_space-size", Some(b"+16"), [0, 0, 16]),
            // V8 sets a size before it finds the negation or the letters wrong.
            (b"no-max-old-space-size", Some(b"128"), [128, 0, 0]),
            (b"nomax_heap_size", Some(b"64"), [0, 64, 0]),
            (b"no_max_semi_space_size", Some(b"8"), [0, 0, 8]),
            (b"max_old_space_size", Some(b"12abc"), [12, 0, 0]),
            // The value of another flag holds more arguments.
            (b"stack_size", Some(b"512 --max-heap-size=64"), [0, 64, 0]),
            (b"max_old_space_size 96", None, [96, 0, 0]),
            (b"max_old_space_size", Some(b"0"), [0, 0, 0]),
            (b"max_old_space_size", Some(b"-5"), [0, 0, 0]),
            (
                b"max_old_space_size",
                Some(b"99999999999999999999"),
                [0, 0, 0],
            ),
            (b"max_old_space_size", None, [0, 0, 0]),
            (b"max_young_generation_size", Some(b"64"), [0, 0, 0]),
            (b"initial_heap_size", Some(b"64"), [0, 0, 0]),
            (b"MAX_OLD_SPACE_SIZE", Some(b"64"), [0, 0, 0]),
            (b"use_strict -- --max-heap-size=64", None, [0, 0, 0]),
        ] {
            assert_eq!(
                sizes_after(&[(name, value)]),
                sizes,
                "{}",
                String::from_utf8_lossy(name)
            );
        }
    }

    #[test]
    fn the_last_size_v8_reads_wins() {
        assert_eq!(
            sizes_after(&[
                (b"max_old_space_size", Some(b"128")),
                (b"max-old-space-size", Some(b"0")),
            ]),
            [0, 0, 0]
        );
        // V8 stops at the first error in a string, and refuses a negative size.
        assert_eq!(
            sizes_after(&[
                (b"max_heap_size", Some(b"64")),
                (b"max_heap_size", Some(b"-1 --max_old_space_size=8")),
            ]),
            [0, 64, 0]
        );
        assert_eq!(
            sizes_after(&[(b"max_heap_size", Some(b"1x --max_old_space_size=8"))]),
            [0, 1, 0]
        );
    }

    // Flags freeze at the first initialization in a process, so this runs in
    // a process of its own even where other tests share one.
    #[test]
    fn rejects_flags_after_initialization() {
        const ALONE: &str = "MINI_RACER_RS_TEST_ALONE";
        if std::env::var_os(ALONE).is_none() {
            let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "process::platform::tests::rejects_flags_after_initialization",
                    "--exact",
                ])
                .env(ALONE, "1")
                .status()
                .expect("run the test binary");
            assert!(status.success(), "{status}");
            return;
        }
        assert_eq!(push_flag(b"use_strict", None), Ok(()));
        assert_eq!(freeze(), vec!["--use_strict".to_owned()]);
        assert!(is_initialized());
        assert_eq!(
            push_flag(b"expose_gc", None),
            Err(FlagError::AlreadyInitialized)
        );
    }

    // The limit and the flags are process state, so this runs alone too.
    #[test]
    fn a_heap_limit_and_a_sizing_flag_refuse_each_other() {
        const ALONE: &str = "MINI_RACER_RS_TEST_ALONE";
        if std::env::var_os(ALONE).is_none() {
            let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
                .args([
                    "process::platform::tests::a_heap_limit_and_a_sizing_flag_refuse_each_other",
                    "--exact",
                ])
                .env(ALONE, "1")
                .status()
                .expect("run the test binary");
            assert!(status.success(), "{status}");
            return;
        }
        assert_eq!(heap_limit(), None);
        assert_eq!(
            set_heap_limit(Some(MIN_HEAP_LIMIT - 1)),
            Err(HeapLimitError::Range)
        );
        assert_eq!(
            set_heap_limit(Some(MAX_HEAP_LIMIT + 1)),
            Err(HeapLimitError::Range)
        );
        assert_eq!(set_heap_limit(Some(64 << 20)), Ok(()));
        assert_eq!(heap_limit(), Some(64 << 20));
        assert_eq!(
            push_flag(b"no-max-heap-size", Some(b"64")),
            Err(FlagError::OverridesHeapLimit("max_heap_size"))
        );
        assert_eq!(push_flag(b"max_old_space_size", Some(b"0")), Ok(()));
        assert_eq!(push_flag(b"initial_heap_size", Some(b"64")), Ok(()));
        assert_eq!(set_heap_limit(None), Ok(()));
        assert_eq!(heap_limit(), None);
        assert_eq!(push_flag(b"max_semi_space_size", Some(b"4")), Ok(()));
        assert_eq!(
            set_heap_limit(Some(64 << 20)),
            Err(HeapLimitError::Overridden("max_semi_space_size"))
        );
        assert_eq!(heap_limit(), None);
        assert_eq!(push_flag(b"max_semi_space_size", Some(b"0")), Ok(()));
        assert_eq!(set_heap_limit(Some(MAX_HEAP_LIMIT)), Ok(()));
        assert_eq!(set_heap_limit(Some(MIN_HEAP_LIMIT)), Ok(()));
    }
}
