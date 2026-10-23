use std::time::Duration;

use mini_racer_engine::{ContextConfig, InvalidSnapshot, SnapshotBlob};

/// An option value, as far as the rules below need to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionValue {
    Fixnum(i64),
    Nil,
    False,
    /// A `MiniRacer::Snapshot`, by the blob it shares or why that blob is
    /// refused; given only for `snapshot:`.
    Snapshot(Result<SnapshotBlob, InvalidSnapshot>),
    /// Anything else, by the name Ruby's type errors give it.
    Other(String),
}

impl OptionValue {
    // Ruby's type errors name nil and false by value.
    fn type_name(&self) -> String {
        match self {
            Self::Fixnum(_) => "Integer".to_owned(),
            Self::Nil => "nil".to_owned(),
            Self::False => "false".to_owned(),
            Self::Snapshot(_) => "MiniRacer::Snapshot".to_owned(),
            Self::Other(name) => name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OptionError {
    #[error("wrong argument type {0} (expected Integer)")]
    Type(String),
    #[error("wrong argument type {0} (expected mini_racer/snapshot)")]
    SnapshotType(String),
    #[error("bad {0}")]
    Range(&'static str),
    #[error("bad keyword: {0}")]
    Keyword(String),
    #[error(transparent)]
    InvalidSnapshot(InvalidSnapshot),
}

const INT32_MAX: i64 = i32::MAX as i64;
const UINT32_MAX: i64 = u32::MAX as i64;

fn fixnum(value: &OptionValue) -> Result<i64, OptionError> {
    match value {
        OptionValue::Fixnum(n) => Ok(*n),
        other => Err(OptionError::Type(other.type_name())),
    }
}

fn within(
    value: &OptionValue,
    name: &'static str,
    range: impl std::ops::RangeBounds<i64>,
) -> Result<i64, OptionError> {
    let n = fixnum(value)?;
    if range.contains(&n) {
        Ok(n)
    } else {
        Err(OptionError::Range(name))
    }
}

/// Milliseconds as `timeout:` takes them: an Integer up to 2**31 - 1, where 0
/// means no timeout. `MiniRacerRs.timeout` takes the same.
pub fn timeout(value: &OptionValue) -> Result<Option<Duration>, OptionError> {
    let ms = within(value, "timeout", 0..=INT32_MAX)?;
    Ok(u64::try_from(ms)
        .ok()
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis))
}

/// `Context.new` keywords in the order given; `mini_racer` 0.22.1 checks them
/// last to first and raises on the first bad one.
pub fn configure(
    options: impl DoubleEndedIterator<Item = (String, OptionValue)>,
) -> Result<ContextConfig, OptionError> {
    let mut config = ContextConfig::default();
    let mut refused = None;
    for (key, value) in options.rev() {
        match key.as_str() {
            "timeout" => config.timeout = timeout(&value)?,
            "marshal_stack_depth" => {
                fixnum(&value)?;
            }
            "ensure_gc_after_idle" => {
                let ms = within(&value, "ensure_gc_after_idle", 0..=INT32_MAX)?;
                config.gc_after_idle = u64::try_from(ms)
                    .ok()
                    .filter(|ms| *ms > 0)
                    .map(Duration::from_millis);
            }
            "max_memory" => {
                let bytes = within(&value, "max_memory", 0..UINT32_MAX)?;
                config.max_memory = u64::try_from(bytes).ok().filter(|bytes| *bytes > 0);
            }
            "snapshot" => match value {
                OptionValue::Nil => {}
                // mini_racer treats an empty blob as no snapshot.
                OptionValue::Snapshot(Ok(blob)) => {
                    config.snapshot = (!blob.is_empty()).then_some(blob);
                }
                // mini_racer's V8 reads the blob only once every option passes.
                OptionValue::Snapshot(Err(invalid)) => refused = Some(invalid),
                other => return Err(OptionError::SnapshotType(other.type_name())),
            },
            // mini_racer turns it on for any value but nil and false.
            "verbose_exceptions" => {
                config.verbose_exceptions = !matches!(value, OptionValue::Nil | OptionValue::False);
            }
            _ => return Err(OptionError::Keyword(key)),
        }
    }
    refused.map_or(Ok(config), |invalid| {
        Err(OptionError::InvalidSnapshot(invalid))
    })
}

/// The most contexts one `MiniRacerRs::Pool` holds. A pool builds every
/// context before `new` returns, each an isolate and a runner thread, so a
/// size near what `timeout:` allows would exhaust the process instead.
pub const MAX_POOL_SIZE: i64 = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolLimits {
    pub size: usize,
    pub max_uses: Option<u64>,
}

/// `MiniRacerRs::Pool.new`'s `size:` and `max_uses:` (nil for no limit),
/// checked as `Context.new` checks its integers.
pub fn pool(size: &OptionValue, max_uses: &OptionValue) -> Result<PoolLimits, OptionError> {
    let size = within(size, "size", 1..=MAX_POOL_SIZE)?;
    let max_uses = match max_uses {
        OptionValue::Nil => None,
        other => Some(within(other, "max_uses", 1..)?),
    };
    Ok(PoolLimits {
        size: usize::try_from(size).map_err(|_| OptionError::Range("size"))?,
        max_uses: max_uses
            .map(u64::try_from)
            .transpose()
            .map_err(|_| OptionError::Range("max_uses"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_takes_one_to_max_contexts_and_any_positive_use_limit() {
        let limits = pool(&OptionValue::Fixnum(4), &OptionValue::Nil);
        assert_eq!(
            limits,
            Ok(PoolLimits {
                size: 4,
                max_uses: None
            })
        );
        let limits = pool(
            &OptionValue::Fixnum(MAX_POOL_SIZE),
            &OptionValue::Fixnum(i64::MAX),
        );
        assert_eq!(
            limits.map(|limits| (limits.size, limits.max_uses)),
            Ok((1024, Some(i64::MAX.cast_unsigned())))
        );
        for size in [0, -1, MAX_POOL_SIZE + 1] {
            assert_eq!(
                pool(&OptionValue::Fixnum(size), &OptionValue::Nil),
                Err(OptionError::Range("size"))
            );
        }
        assert_eq!(
            pool(&OptionValue::Fixnum(1), &OptionValue::Fixnum(0))
                .map_err(|error| error.to_string()),
            Err("bad max_uses".to_owned())
        );
    }

    #[test]
    fn a_pool_names_a_value_of_the_wrong_type_as_context_new_does() {
        assert_eq!(
            pool(&OptionValue::Nil, &OptionValue::Nil).map_err(|error| error.to_string()),
            Err("wrong argument type nil (expected Integer)".to_owned())
        );
        assert_eq!(
            pool(&OptionValue::Fixnum(2), &OptionValue::False).map_err(|error| error.to_string()),
            Err("wrong argument type false (expected Integer)".to_owned())
        );
        assert_eq!(
            pool(
                &OptionValue::Other("String".to_owned()),
                &OptionValue::Other("Float".to_owned())
            ),
            Err(OptionError::Type("String".to_owned()))
        );
    }

    fn configure_pairs(pairs: &[(&str, OptionValue)]) -> Result<ContextConfig, OptionError> {
        configure(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone())),
        )
    }

    fn message(pairs: &[(&str, OptionValue)]) -> Option<String> {
        configure_pairs(pairs).err().map(|e| e.to_string())
    }

    #[test]
    fn timeout_is_milliseconds_and_zero_means_none() {
        let config = configure_pairs(&[("timeout", OptionValue::Fixnum(250))]);
        assert_eq!(
            config.map(|c| c.timeout),
            Ok(Some(Duration::from_millis(250)))
        );
        let config = configure_pairs(&[("timeout", OptionValue::Fixnum(0))]);
        assert_eq!(config.map(|c| c.timeout), Ok(None));
    }

    #[test]
    fn range_and_type_errors_match_mini_racer() {
        assert_eq!(
            message(&[("timeout", OptionValue::Fixnum(-1))]),
            Some("bad timeout".to_owned())
        );
        assert_eq!(
            configure_pairs(&[("timeout", OptionValue::Fixnum(INT32_MAX + 1))]).err(),
            Some(OptionError::Range("timeout"))
        );
        assert_eq!(
            message(&[("timeout", OptionValue::Nil)]),
            Some("wrong argument type nil (expected Integer)".to_owned())
        );
        assert_eq!(
            message(&[(
                "marshal_stack_depth",
                OptionValue::Other("String".to_owned())
            )]),
            Some("wrong argument type String (expected Integer)".to_owned())
        );
        assert!(configure_pairs(&[("marshal_stack_depth", OptionValue::Fixnum(-5))]).is_ok());
        assert_eq!(
            message(&[("max_memory", OptionValue::Fixnum(UINT32_MAX))]),
            Some("bad max_memory".to_owned())
        );
        assert_eq!(
            message(&[("ensure_gc_after_idle", OptionValue::False)]),
            Some("wrong argument type false (expected Integer)".to_owned())
        );
    }

    #[test]
    fn max_memory_is_bytes_and_zero_means_none() {
        let config = configure_pairs(&[("max_memory", OptionValue::Fixnum(200_000_000))]);
        assert_eq!(config.map(|c| c.max_memory), Ok(Some(200_000_000)));
        let config = configure_pairs(&[("max_memory", OptionValue::Fixnum(0))]);
        assert_eq!(config.map(|c| c.max_memory), Ok(None));
    }

    #[test]
    fn ensure_gc_after_idle_is_milliseconds_and_zero_means_none() {
        let config = configure_pairs(&[("ensure_gc_after_idle", OptionValue::Fixnum(25))]);
        assert_eq!(
            config.map(|c| c.gc_after_idle),
            Ok(Some(Duration::from_millis(25)))
        );
        let config = configure_pairs(&[("ensure_gc_after_idle", OptionValue::Fixnum(0))]);
        assert_eq!(config.map(|c| c.gc_after_idle), Ok(None));
    }

    #[test]
    fn keys_are_checked_last_to_first() {
        let pairs = [
            ("timeout", OptionValue::Other("String".to_owned())),
            ("bogus", OptionValue::Fixnum(1)),
        ];
        assert_eq!(message(&pairs), Some("bad keyword: bogus".to_owned()));
        let pairs = [
            ("bogus", OptionValue::Fixnum(1)),
            ("timeout", OptionValue::Other("String".to_owned())),
        ];
        assert_eq!(
            configure_pairs(&pairs).err(),
            Some(OptionError::Type("String".to_owned()))
        );
    }

    #[test]
    fn verbose_exceptions_is_any_value_but_nil_and_false() {
        for (value, verbose) in [
            (OptionValue::Nil, false),
            (OptionValue::False, false),
            (OptionValue::Other("TrueClass".to_owned()), true),
            (OptionValue::Other("String".to_owned()), true),
            (OptionValue::Fixnum(0), true),
        ] {
            let config = configure_pairs(&[("verbose_exceptions", value)]);
            assert_eq!(config.map(|c| c.verbose_exceptions), Ok(verbose));
        }
    }

    #[test]
    fn a_snapshot_must_be_one_and_nil_or_an_empty_one_is_none() {
        let empty = configure_pairs(&[(
            "snapshot",
            OptionValue::Snapshot(Ok(SnapshotBlob::default())),
        )]);
        assert_eq!(empty.map(|config| config.snapshot), Ok(None));
        let nil = configure_pairs(&[("snapshot", OptionValue::Nil)]);
        assert_eq!(nil.map(|config| config.snapshot), Ok(None));
        assert_eq!(
            configure_pairs(&[("snapshot", OptionValue::False)]).map(drop),
            Err(OptionError::SnapshotType("false".to_owned()))
        );
        assert_eq!(
            configure_pairs(&[(
                "snapshot",
                OptionValue::Other("mini_racer/context".to_owned())
            )])
            .map(drop),
            Err(OptionError::SnapshotType("mini_racer/context".to_owned()))
        );
    }

    #[test]
    fn a_refused_blob_raises_only_once_every_option_passes() {
        let refused = OptionValue::Snapshot(Err(InvalidSnapshot::Version));
        assert_eq!(
            configure_pairs(&[("snapshot", refused.clone())]).err(),
            Some(OptionError::InvalidSnapshot(InvalidSnapshot::Version))
        );
        for pairs in [
            [("timeout", OptionValue::Nil), ("snapshot", refused.clone())],
            [("snapshot", refused), ("timeout", OptionValue::Nil)],
        ] {
            assert_eq!(
                message(&pairs),
                Some("wrong argument type nil (expected Integer)".to_owned())
            );
        }
    }
}
