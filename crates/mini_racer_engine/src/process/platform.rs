use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::FlagError;

const MAX_FLAG_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    MultiThreaded,
    SingleThreaded,
}

struct Flags {
    list: Vec<String>,
    frozen: bool,
    mode: Mode,
}

static FLAGS: Mutex<Flags> = Mutex::new(Flags {
    list: Vec::new(),
    frozen: false,
    mode: Mode::MultiThreaded,
});

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
    if let Some(mode) = normalized.mode {
        flags.mode = mode;
    }
    flags.list.push(normalized.flag);
    drop(flags);
    Ok(())
}

#[must_use]
pub fn mode() -> Mode {
    flags().mode
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

    #[test]
    fn rejects_flags_after_initialization() {
        assert_eq!(push_flag(b"use_strict", None), Ok(()));
        assert_eq!(freeze(), vec!["--use_strict".to_owned()]);
        assert!(is_initialized());
        assert_eq!(
            push_flag(b"expose_gc", None),
            Err(FlagError::AlreadyInitialized)
        );
    }
}
