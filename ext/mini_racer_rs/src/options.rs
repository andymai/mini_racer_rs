use std::sync::Arc;
use std::time::Duration;

use mini_racer_engine::ContextConfig;

/// An option value, as far as the rules below need to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionValue {
    Fixnum(i64),
    Nil,
    False,
    /// A `MiniRacer::Snapshot`, by its blob.
    Snapshot(Arc<[u8]>),
    /// Anything else, by its class name.
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
    #[error("Context option {0} is not supported yet")]
    Unsupported(String),
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

/// `Context.new` keywords in the order given; `mini_racer` 0.22.1 checks them
/// last to first and raises on the first bad one.
pub fn configure(
    options: impl DoubleEndedIterator<Item = (String, OptionValue)>,
) -> Result<ContextConfig, OptionError> {
    let mut config = ContextConfig::default();
    for (key, value) in options.rev() {
        match key.as_str() {
            "timeout" => {
                let ms = within(&value, "timeout", 0..=INT32_MAX)?;
                config.timeout = u64::try_from(ms)
                    .ok()
                    .filter(|ms| *ms > 0)
                    .map(Duration::from_millis);
            }
            "marshal_stack_depth" => {
                fixnum(&value)?;
            }
            "ensure_gc_after_idle" => {
                within(&value, "ensure_gc_after_idle", 0..=INT32_MAX)?;
                return Err(OptionError::Unsupported(key));
            }
            "max_memory" => {
                within(&value, "max_memory", 0..UINT32_MAX)?;
                return Err(OptionError::Unsupported(key));
            }
            "snapshot" => match value {
                OptionValue::Nil => {}
                // mini_racer treats an empty blob as no snapshot.
                OptionValue::Snapshot(blob) => config.snapshot = (!blob.is_empty()).then_some(blob),
                other => return Err(OptionError::SnapshotType(other.type_name())),
            },
            // nil and false are what leaving it out means.
            "verbose_exceptions" if matches!(value, OptionValue::Nil | OptionValue::False) => {}
            "verbose_exceptions" => return Err(OptionError::Unsupported(key)),
            _ => return Err(OptionError::Keyword(key)),
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn default_values_of_unbuilt_options_are_accepted() {
        assert!(configure_pairs(&[("snapshot", OptionValue::Nil)]).is_ok());
        assert!(configure_pairs(&[("verbose_exceptions", OptionValue::False)]).is_ok());
        assert!(configure_pairs(&[("verbose_exceptions", OptionValue::Nil)]).is_ok());
    }

    #[test]
    fn a_snapshot_must_be_one_and_an_empty_one_is_none() {
        let blob: Arc<[u8]> = Arc::from(&b"blob"[..]);
        let config = configure_pairs(&[("snapshot", OptionValue::Snapshot(Arc::clone(&blob)))]);
        assert_eq!(config.map(|config| config.snapshot), Ok(Some(blob)));
        let empty = configure_pairs(&[("snapshot", OptionValue::Snapshot(Arc::from(&b""[..])))]);
        assert_eq!(empty.map(|config| config.snapshot), Ok(None));
        assert_eq!(
            configure_pairs(&[("snapshot", OptionValue::False)]).map(drop),
            Err(OptionError::SnapshotType("false".to_owned()))
        );
    }

    #[test]
    fn options_not_built_yet_say_so() {
        for pairs in [
            [("max_memory", OptionValue::Fixnum(1))],
            [("ensure_gc_after_idle", OptionValue::Fixnum(1))],
            [(
                "verbose_exceptions",
                OptionValue::Other("TrueClass".to_owned()),
            )],
        ] {
            assert!(matches!(
                configure_pairs(&pairs),
                Err(OptionError::Unsupported(_))
            ));
        }
    }
}
